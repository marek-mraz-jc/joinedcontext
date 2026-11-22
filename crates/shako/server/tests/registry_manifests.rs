#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7523: a manifest is a blob. The database holds its digest, size and media type, a tag list
//! reads no manifest bytes and the quota counts them.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection. Unset: prints `SKIP`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Store as _, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_pg::PgStore;
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr as _;
use std::sync::Arc;

const TAGS: usize = 200;
const PADDING: usize = 64 * 1024;

async fn send(app: &Router, method: &str, path: &str, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", "Bearer city-token")
        .header("content-type", "application/vnd.oci.image.manifest.v1+json")
        .body(Body::from(body))
        .expect("request");
    let answer = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("answer");
    let status = answer.status();
    let bytes = answer.into_body().collect().await.expect("body").to_bytes();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn two_hundred_manifests_leave_no_bytes_in_postgres_and_count_in_the_quota() {
    let Ok(url) = std::env::var("SHAKO_TEST_DATABASE_URL") else {
        println!("SKIP: SHAKO_TEST_DATABASE_URL is unset");
        return;
    };
    let options = PgConnectOptions::from_str(&url).expect("valid SHAKO_TEST_DATABASE_URL");
    let server = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .expect("SHAKO_TEST_DATABASE_URL is set but unreachable");
    let name = format!("shako_manifests_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&server)
        .await
        .expect("create");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone().database(&name))
        .await
        .expect("connect");
    PgStore::run_migrations(&admin).await.expect("migrate");
    let password = uuid::Uuid::new_v4().simple().to_string();
    sqlx::raw_sql(AssertSqlSafe(format!(
        "CREATE ROLE {name} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{password}';
         GRANT shako_app TO {name};"
    )))
    .execute(&admin)
    .await
    .expect("login");
    let store = PgStore::connect_with(
        options.database(&name).username(&name).password(&password),
        4,
    )
    .await
    .expect("store");

    let repositories = tempfile::tempdir().expect("tempdir");
    let (instance, org) = (InstanceId::from_u128(1), OrgId::from_u128(10));
    let state = ShakoState::new(
        instance,
        org,
        repositories.path().to_path_buf(),
        Arc::new(store),
        Arc::new(LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: "city".into(),
            actions: vec![Action::Admin, Action::Write, Action::Publish],
            project_id: None,
            repo: None,
        }])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
            StaticTokenBinding::new(
                "city-token",
                Subject::authenticated("city", "city", vec![org], vec![]),
            ),
        ]),
        OutboundGuard::new(OutboundPolicy::default()),
    );
    state.register_owner_org("city", org);
    let mount = jc_router::Mount::new("shako", "https", "city.example", "/git").expect("mount");
    let app = router(state.clone(), mount);

    let mut first = Vec::new();
    for tag in 0..TAGS {
        let manifest = serde_json::to_vec(&json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [],
            "annotations": { "tag": tag.to_string(), "padding": "p".repeat(PADDING) }
        }))
        .expect("manifest");
        let path = format!("/v2/city/image/manifests/t{tag}");
        let (status, body) = send(&app, "PUT", &path, manifest.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&body)
        );
        if tag == 0 {
            first = manifest;
        }
    }
    let digest = shako_core::sha256_hex(&first);
    let path = format!("/v2/city/image/manifests/sha256:{digest}");
    assert_eq!(
        send(&app, "GET", &path, Vec::new()).await,
        (StatusCode::OK, first)
    );

    let (rows, log): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT sum(length(row_to_json(t)::text)) FROM shako.package_version t)::bigint,
                (SELECT sum(length(op::text)) FROM shako.store_log)::bigint",
    )
    .fetch_one(&admin)
    .await
    .expect("sizes");
    let quota = state
        .store()
        .get_quota_usage(instance, org)
        .await
        .expect("quota")
        .package_bytes;
    let (status, list) = send(&app, "GET", "/v2/city/image/tags/list", Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_slice(&list).expect("tags");
    let tags = list["tags"].as_array().expect("tags").len();
    println!(
        "{TAGS} manifests of 64 KiB: a tag list reads {rows} bytes of rows, the log holds {log}, \
         the quota counts {quota}"
    );
    assert_eq!(tags, TAGS * 2, "each tag and its signature tag");

    drop((app, state));
    admin.close().await;
    sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
        .execute(&server)
        .await
        .expect("drop database");
    sqlx::raw_sql(AssertSqlSafe(format!("DROP ROLE {name}")))
        .execute(&server)
        .await
        .expect("drop role");

    let padded = (TAGS * PADDING) as u64;
    assert!(rows < 1024 * 1024, "a list reads {rows} bytes");
    assert!((log as u64) < 2 * 1024 * 1024, "the log holds {log} bytes");
    assert!(
        quota >= padded,
        "the quota counts {quota} of {padded} bytes"
    );
}
