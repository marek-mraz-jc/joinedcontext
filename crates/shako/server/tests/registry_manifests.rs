#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7523: a manifest is a blob. The database holds its digest, size and media type, a tag list
//! reads no manifest bytes and the quota counts them.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection. Unset: prints `SKIP`.

#[path = "../../../shared/test-support/pg_meter.rs"]
mod pg_meter;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal,
    RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_pg::PgStore;
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};
use std::str::FromStr as _;
use std::sync::Arc;

const TAGS: usize = 200;
const PADDING: usize = 64 * 1024;

async fn send(app: &Router, method: &str, path: &str, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
    send_as(app, "city-token", method, path, body).await
}

async fn send_as(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
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

/// A registry of two Organizations on a database of its own, read through a meter.
struct Registry {
    server: PgPool,
    admin: PgPool,
    name: String,
    meter: pg_meter::PgMeter,
    repositories: tempfile::TempDir,
    store: Arc<PgStore>,
    state: ShakoState,
    app: Router,
}

const INSTANCE: InstanceId = InstanceId::from_u128(1);
const CITY: OrgId = OrgId::from_u128(10);
const TOWN: OrgId = OrgId::from_u128(20);

impl Registry {
    async fn start() -> Option<Self> {
        let Ok(url) = std::env::var("SHAKO_TEST_DATABASE_URL") else {
            println!("SKIP: SHAKO_TEST_DATABASE_URL is unset");
            return None;
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
        let meter = pg_meter::PgMeter::start(options.get_host(), options.get_port()).await;
        let store = PgStore::connect_with(
            options
                .host("127.0.0.1")
                .port(meter.port)
                .ssl_mode(PgSslMode::Disable)
                .database(&name)
                .username(&name)
                .password(&password),
            4,
        )
        .await
        .expect("store");
        let store = Arc::new(store);
        let repositories = tempfile::tempdir().expect("tempdir");
        let orgs = [("city", CITY), ("town", TOWN)];
        let state = ShakoState::new(
            INSTANCE,
            CITY,
            repositories.path().to_path_buf(),
            store.clone(),
            Arc::new(LocalAuthorizer::new(
                orgs.map(|(name, org_id)| RoleBinding {
                    org_id,
                    principal: Principal::User(name.into()),
                    actions: vec![Action::Admin, Action::Write, Action::Publish],
                    project_id: None,
                    repo: None,
                })
                .to_vec(),
            )),
            TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(
                orgs.map(|(name, org)| {
                    StaticTokenBinding::new(
                        &format!("{name}-token"),
                        Subject::authenticated(name, name, vec![org]),
                    )
                })
                .to_vec(),
            ),
            OutboundGuard::new(OutboundPolicy::default()),
        );
        for (name, org) in orgs {
            state.register_owner_org(name, org);
        }
        let mount = jc_router::Mount::new("shako", "https", "city.example", "/git").expect("mount");
        let app = router(state.clone(), mount);
        Some(Self {
            server,
            admin,
            name,
            meter,
            repositories,
            store,
            state,
            app,
        })
    }

    async fn quota(&self) -> u64 {
        self.state
            .store()
            .get_quota_usage(INSTANCE, CITY)
            .await
            .expect("quota")
            .package_bytes
    }

    async fn stop(self) {
        drop((self.app, self.state));
        self.store.pool().close().await;
        self.admin.close().await;
        let name = &self.name;
        sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
            .execute(&self.server)
            .await
            .expect("drop database");
        sqlx::raw_sql(AssertSqlSafe(format!("DROP ROLE {name}")))
            .execute(&self.server)
            .await
            .expect("drop role");
    }
}

fn manifest(tag: usize, padding: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [],
        "annotations": { "tag": tag.to_string(), "padding": "p".repeat(padding) }
    }))
    .expect("manifest")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_hundred_manifests_leave_no_bytes_in_postgres_and_count_in_the_quota() {
    let Some(registry) = Registry::start().await else {
        return;
    };
    let app = registry.app.clone();
    for tag in 0..TAGS {
        let path = format!("/v2/city/image/manifests/t{tag}");
        let (status, body) = send(&app, "PUT", &path, manifest(tag, PADDING)).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&body)
        );
    }
    let first = manifest(0, PADDING);
    let digest = shako_core::sha256_hex(&first);
    let path = format!("/v2/city/image/manifests/sha256:{digest}");
    assert_eq!(
        send(&app, "GET", &path, Vec::new()).await,
        (StatusCode::OK, first.clone())
    );
    for (token, path) in [
        ("town-token", path.clone()),
        (
            "town-token",
            format!("/v2/town/image/manifests/sha256:{digest}"),
        ),
        (
            "town-token",
            format!("/v2/town/image/blobs/sha256:{digest}"),
        ),
    ] {
        let (status, _) = send_as(&app, token, "GET", &path, Vec::new()).await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::UNAUTHORIZED,
            "another Organization reads {path}: {status}"
        );
    }

    let (column, log): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT sum(octet_length(manifest_bytes)) FROM shako.package_version)::bigint,
                (SELECT sum(length(op::text)) FROM shako.store_log)::bigint",
    )
    .fetch_one(&registry.admin)
    .await
    .expect("sizes");
    let quota = registry.quota().await;

    registry.meter.take();
    let (status, list) = send(&app, "GET", "/v2/city/image/tags/list", Vec::new()).await;
    let listed = registry.meter.take();
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_slice(&list).expect("tags");
    let tags = list["tags"].as_array().expect("tags").clone();
    let versions = registry
        .state
        .store()
        .list_package_versions(INSTANCE, CITY, None, None, None)
        .await
        .expect("versions");
    let projected = registry.meter.take();
    println!(
        "{TAGS} manifests of 64 KiB: a tag list reads {listed:?} (rows, bytes), every version \
         {projected:?}, the column holds {column}, the log {log}, the quota counts {quota}"
    );
    assert_eq!(tags.len(), TAGS * 2, "each tag and its signature tag");
    assert_eq!(versions.len(), TAGS * 2);

    let (status, page) = send(&app, "GET", "/v2/city/image/tags/list?n=3", Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    let page: Value = serde_json::from_slice(&page).expect("page");
    assert_eq!(
        page["tags"].as_array().expect("tags").as_slice(),
        &tags[..3]
    );
    let last = tags[2].as_str().expect("tag");
    let next = format!("/v2/city/image/tags/list?n=3&last={last}");
    let (_, page) = send(&app, "GET", &next, Vec::new()).await;
    let page: Value = serde_json::from_slice(&page).expect("page");
    assert_eq!(
        page["tags"].as_array().expect("tags").as_slice(),
        &tags[3..6]
    );

    // The file is hashed again before it is sent.
    let file = shako_server::blob_path(
        registry.repositories.path(),
        INSTANCE,
        "city",
        &format!("sha256:{digest}"),
    );
    std::fs::write(&file, b"{}").expect("damage");
    let (status, body) = send(&app, "GET", &path, Vec::new()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.len() < 1024, "no damaged bytes are sent");

    drop(app);
    registry.stop().await;

    let padded = (TAGS * PADDING) as u64;
    assert_eq!(column, 0, "the column holds no new manifest");
    assert!(listed.1 < 64 * 1024, "a list reads {} bytes", listed.1);
    assert!(
        projected.1 < 1024 * 1024,
        "the versions are {} bytes",
        projected.1
    );
    assert!((log as u64) < 2 * 1024 * 1024, "the log holds {log} bytes");
    assert!(
        quota >= padded,
        "the quota counts {quota} of {padded} bytes"
    );
}

#[tokio::test]
async fn a_manifest_past_the_quota_is_denied_and_one_held_still_pushes() {
    let Some(registry) = Registry::start().await else {
        return;
    };
    let app = registry.app.clone();
    let held = manifest(1, 16);
    let path = "/v2/city/image/manifests/one";
    assert_eq!(
        send(&app, "PUT", path, held.clone()).await.0,
        StatusCode::CREATED
    );
    let used = registry.quota().await;
    assert!(used > held.len() as u64, "the signature counts too");

    let store = registry.state.store();
    let mut instance = registry
        .state
        .instance_for(INSTANCE, CITY)
        .await
        .expect("instance");
    instance.max_storage_bytes = used;
    store.upsert_instance(instance).await.expect("limit");

    let refused = manifest(2, 16);
    let (status, body) = send(&app, "PUT", "/v2/city/image/manifests/two", refused).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let body: Value = serde_json::from_slice(&body).expect("OCI error");
    assert_eq!(body["errors"][0]["code"], "DENIED");
    let two = "/v2/city/image/manifests/two";
    assert_eq!(
        send(&app, "GET", two, Vec::new()).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, list) = send(&app, "GET", "/v2/city/image/tags/list", Vec::new()).await;
    let list: Value = serde_json::from_slice(&list).expect("tags");
    assert_eq!(
        list["tags"].as_array().expect("tags").len(),
        2,
        "nothing signed what was refused"
    );
    assert_eq!(send(&app, "PUT", path, held).await.0, StatusCode::CREATED);
    assert_eq!(registry.quota().await, used, "a blob held counts once");

    drop(app);
    registry.stop().await;
}

/// What the release before leaves: manifests in the log and in the column, no blob of them.
#[tokio::test]
async fn manifests_of_the_release_before_are_pulled_by_digest_after_the_copy() {
    let Some(registry) = Registry::start().await else {
        return;
    };
    let (logged, kept) = (manifest(1, 4096), manifest(2, 4096));
    let record = |tag: &str, bytes: &[u8], digest: String| {
        json!({
            "id": uuid::Uuid::now_v7(), "instanceId": INSTANCE, "orgId": CITY,
            "packageId": uuid::Uuid::now_v7(), "owner": "city", "packageType": "container",
            "packageName": "image", "version": tag, "digest": digest,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "subjectDigest": null, "artifactType": null, "sizeBytes": bytes.len(),
            "signed": false, "createdAt": "2026-11-01T00:00:00Z", "manifestBytes": bytes,
        })
    };
    let digest_of = |bytes: &[u8]| format!("sha256:{}", shako_core::sha256_hex(bytes));
    for (tag, bytes, digest) in [
        ("logged", &logged, digest_of(&logged)),
        // The column says another digest than its bytes: it stays where it is.
        ("damaged", &kept, format!("sha256:{}", "0".repeat(64))),
        ("kept", &kept, digest_of(&kept)),
    ] {
        sqlx::query("INSERT INTO shako.store_log (instance_id, org_id, op) VALUES ($1, $2, $3)")
            .bind(INSTANCE.as_uuid())
            .bind(CITY.as_uuid())
            .bind(json!({"op": "upsert_package_version", "args": record(tag, bytes, digest)}))
            .execute(&registry.admin)
            .await
            .expect("log entry");
    }
    assert_eq!(
        registry
            .store
            .hydrate(INSTANCE, CITY)
            .await
            .expect("replay"),
        3
    );

    let root = registry.repositories.path();
    let copy = || {
        registry
            .store
            .copy_manifests(INSTANCE, CITY, |owner, digest, bytes| {
                let path = shako_server::blob_path(root, INSTANCE, &owner, &digest);
                shako_server::write_blob_file(path, bytes)
            })
    };
    assert_eq!(copy().await.expect("copy"), 2);
    assert_eq!(copy().await.expect("again"), 0, "it may run again");
    assert_eq!(registry.quota().await, (logged.len() + kept.len()) as u64);

    for (tag, bytes) in [("logged", &logged), ("kept", &kept)] {
        for reference in [tag.to_owned(), digest_of(bytes)] {
            let path = format!("/v2/city/image/manifests/{reference}");
            assert_eq!(
                send(&registry.app, "GET", &path, Vec::new()).await,
                (StatusCode::OK, bytes.clone()),
                "{reference} is byte for byte what was pushed"
            );
        }
    }
    let damaged = "/v2/city/image/manifests/damaged";
    assert_eq!(
        send(&registry.app, "GET", damaged, Vec::new()).await.0,
        StatusCode::NOT_FOUND
    );

    // The release before still serves them after a rollback (STO-11).
    let column: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM shako.package_version WHERE octet_length(manifest_bytes) > 0",
    )
    .fetch_one(&registry.admin)
    .await
    .expect("column");
    assert_eq!(column, 3, "the column keeps its bytes for one release");

    registry.stop().await;
}
