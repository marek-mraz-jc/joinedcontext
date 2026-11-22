#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7525: a token reads the scopes it names and a People search reads a page, whatever the
//! realm holds.
//!
//! `KAISATSU_TEST_DATABASE_URL` names a superuser connection. Unset: prints `SKIP`.

mod common;
#[path = "../../store-pg/tests/common/mod.rs"]
mod pg;
#[path = "../../../shared/test-support/pg_meter.rs"]
mod pg_meter;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use http_body_util::BodyExt as _;
use kaisatsu_core::Store as _;
use kaisatsu_crypto::{Kek, sign_jwt};
use kaisatsu_store_pg::PgStore;
use serde_json::{Value, json};
use sqlx::postgres::PgSslMode;
use std::sync::Arc;
use tower::ServiceExt as _;

const HOST: &str = "https://idm.dev.example.test";
const SCOPES: i64 = 500;
const USERS: i64 = 5000;

async fn read(app: &axum::Router, request: Request<Body>) -> Value {
    let answer = app.clone().oneshot(request).await.expect("answer");
    let status = answer.status();
    let body = answer.into_body().collect().await.expect("body").to_bytes();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("JSON")
}

fn token_request() -> Request<Body> {
    // The fixture's client and its secret (common/mod.rs).
    let (client, word) = ("apisix-gateway", "apisix");
    let fixture = format!("{client}:{word}-secret-123");
    let basic = base64::engine::general_purpose::STANDARD.encode(fixture);
    Request::post("/realms/dev/protocol/openid-connect/token")
        .header(header::AUTHORIZATION, format!("Basic {basic}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("grant_type=client_credentials"))
        .expect("request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_and_a_people_search_do_not_read_the_realm() {
    let Some(db) = pg::test_db("scale").await else {
        return;
    };
    let options = db.app_options();
    let meter = pg_meter::PgMeter::start(options.get_host(), options.get_port()).await;
    let options = options
        .host("127.0.0.1")
        .port(meter.port)
        .ssl_mode(PgSslMode::Disable);
    let kek = Kek::generate_for_tests();
    let store = Arc::new(
        PgStore::connect_with(options, 4, kek.clone())
            .await
            .expect("metered store"),
    );
    let state = common::setup_dev_realm_on(store.clone(), kek.clone()).await;
    let mount =
        jc_router::Mount::new("kaisatsu", "https", "idm.dev.example.test", "").expect("mount");
    let app = kaisatsu_server::router(state.clone(), mount.clone()).layer(axum::Extension(mount));

    read(&app, token_request()).await;
    meter.take();
    read(&app, token_request()).await;
    let small = meter.take();

    let root = db.superuser_pool().await;
    sqlx::query(
        "INSERT INTO kaisatsu.client_scopes (realm_id, id, name, doc)
         SELECT 'dev', 'scale-' || n, 'scale-' || n,
                doc || jsonb_build_object('id', 'scale-' || n, 'name', 'scale-' || n)
         FROM (SELECT doc FROM kaisatsu.client_scopes WHERE realm_id = 'dev' LIMIT 1) s,
              generate_series(1, $1) n",
    )
    .bind(SCOPES)
    .execute(&root)
    .await
    .expect("scopes");
    sqlx::query(
        "INSERT INTO kaisatsu.people (realm_id, id, username_lower, email_lower, doc)
         SELECT 'dev', 'scale-' || n, 'person-' || n, 'person-' || n || '@city.example',
                doc || jsonb_build_object('id', 'scale-' || n, 'username', 'person-' || n,
                                          'email', 'person-' || n || '@city.example')
         FROM (SELECT doc FROM kaisatsu.people WHERE realm_id = 'dev' AND username_lower NOT LIKE 'service-account-%'
               ORDER BY id LIMIT 1) p, generate_series(1, $1) n",
    )
    .bind(USERS)
    .execute(&root)
    .await
    .expect("people");
    root.close().await;

    meter.take();
    read(&app, token_request()).await;
    let large = meter.take();

    let keys = store.list_signing_keys("dev").await.expect("keys");
    let key = keys
        .iter()
        .find(|k| k.status == "ACTIVE")
        .expect("active key");
    let now = jc_time::TimestampTz::now().as_second();
    let admin = sign_jwt(
        key,
        &kek,
        Some("JWT"),
        &json!({ "iss": format!("{HOST}/realms/dev"), "sub": "admin-1", "azp": "admin-cli",
                 "typ": "Bearer", "iat": now, "exp": now + 600,
                 "realm_access": { "roles": ["realm-admin"] } }),
    )
    .expect("admin token");
    let mut searches = Vec::new();
    for query in [
        "search=person&first=100&max=50",
        "max=50",
        "email=person-4321%40city.example&exact=true",
    ] {
        meter.take();
        let page = read(
            &app,
            Request::get(format!("/admin/realms/dev/users?{query}"))
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        let found = page.as_array().expect("people").len() as u64;
        assert!(found > 0, "{query} finds people");
        searches.push((query, found, meter.take()));
    }
    println!(
        "token: {small:?} (rows, bytes) with the fixture, {large:?} with {SCOPES} more scopes"
    );
    for (query, found, read) in &searches {
        println!("{USERS} people, {query}: {found} answered, {read:?} (rows, bytes) read");
    }
    drop((app, state, store));
    db.drop_db().await;

    assert!(
        large.0 <= small.0 + 5,
        "a token reads {} rows, was {}",
        large.0,
        small.0
    );
    for (query, found, (rows, _)) in searches {
        assert!(
            rows <= 3 * found + 30,
            "{query} reads {rows} rows for {found}"
        );
    }
}
