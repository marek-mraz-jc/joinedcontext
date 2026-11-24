#![forbid(unsafe_code)]
//! partialImport is all or nothing (S3-27): a store error after the first writes rolls every
//! write of the request back, since the admin router runs each write request as one PostgreSQL
//! transaction (`jc_audit::layer`). Needs `KAISATSU_TEST_DATABASE_URL`, else prints `SKIP`.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;
#[path = "../../store-pg/tests/common/mod.rs"]
mod pgdb;

async fn admin_token(store: &Arc<dyn Store>, kek: &Kek) -> String {
    let keys = store.list_signing_keys("dev").await.unwrap();
    let key = keys.iter().find(|k| k.status == "ACTIVE").unwrap();
    let now = jc_time::TimestampTz::now().as_second();
    let claims = json!({ "iss": "https://idm.dev.example.test/realms/dev", "sub": "admin-1",
                         "azp": "admin-cli", "typ": "Bearer", "iat": now, "exp": now + 600,
                         "realm_access": { "roles": ["realm-admin"] } });
    sign_jwt(key, kek, Some("JWT"), &claims).unwrap()
}

async fn post(app: &Router, token: &str, uri: &str, body: &Value) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_error_mid_import_writes_nothing() {
    let Some(db) = pgdb::test_db("a_store_error_mid_import_writes_nothing").await else {
        return;
    };
    let kek = Kek::generate_for_tests();
    let store: Arc<dyn Store> = Arc::new(db.store(kek.clone()).await);
    let state = setup_dev_realm_on(store.clone(), kek.clone()).await;
    let mount = Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap();
    let app = router(state, mount.clone()).layer(Extension(mount));
    let token = admin_token(&store, &kek).await;

    // The group and the first person are written; the second person's e-mail is the first's,
    // so the store refuses that write.
    let body = json!({
        "ifResourceExists": "OVERWRITE",
        "groups": [{ "name": "rb-group" }],
        "users": [
            { "id": "rb-1", "username": "rb.one", "email": "rb@example.test" },
            { "id": "rb-2", "username": "rb.two", "email": "rb@example.test" }
        ]
    });
    let (st, text) = post(&app, &token, "/admin/realms/dev/partialImport", &body).await;
    assert_eq!(st, StatusCode::CONFLICT, "{text}");

    assert!(
        store
            .get_group_by_path("dev", None, "/rb-group")
            .await
            .unwrap()
            .is_none(),
        "the group written before the error is rolled back"
    );
    assert!(
        store
            .get_user_by_id("dev", None, "rb-1")
            .await
            .unwrap()
            .is_none(),
        "the person written before the error is rolled back"
    );
    db.drop_db().await;
}
