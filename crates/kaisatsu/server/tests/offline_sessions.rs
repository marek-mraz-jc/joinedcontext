#![forbid(unsafe_code)]
//! Offline sessions keep the realm's lifetimes (S3-22): the maximum counts from the first token,
//! whatever the refreshes since, and an offline family outlives the SSO session it began in
//! until a password change signs out the other devices.

use axum::http::StatusCode;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_server::router;
use serde_json::Value;
use std::sync::Arc;

mod common;
use common::*;

const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const INTROSPECT: &str = "/realms/dev/protocol/openid-connect/token/introspect";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("json body ({e}): {body}"))
}

/// The dev realm with `offline_access` offered to the gateway client and the given offline
/// maximum, in seconds.
async fn offline_app(max_lifespan: i64) -> (axum::Router, Arc<kaisatsu_store_memory::MemoryStore>) {
    let (state, store, _) = setup_dev_realm_state().await;
    let mut gateway = store
        .get_client_by_client_id("dev", "apisix-gateway")
        .await
        .unwrap()
        .unwrap();
    gateway.optional_client_scopes.push("offline_access".into());
    store.upsert_client(gateway).await.unwrap();
    let mut realm = store.get_realm("dev").await.unwrap().unwrap();
    realm.offline_session_max_lifespan_enabled = true;
    realm.offline_session_max_lifespan = max_lifespan;
    store.upsert_realm(realm).await.unwrap();
    (router(state, mount()), store)
}

async fn offline_tokens(app: &axum::Router) -> Value {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "apisix-gateway"),
            ("client_secret", "apisix-secret-123"),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
            ("scope", "openid offline_access"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    json(&body)
}

async fn refresh(app: &axum::Router, token: &str) -> (StatusCode, Value) {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "apisix-gateway"),
            ("client_secret", "apisix-secret-123"),
            ("refresh_token", token),
        ],
        None,
    )
    .await;
    (st, json(&body))
}

async fn active(app: &axum::Router, token: &str) -> bool {
    let (st, _, body) = post_form(
        app,
        INTROSPECT,
        &[
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("token", token),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    json(&body)["active"] == true
}

#[tokio::test]
async fn refreshing_does_not_extend_the_offline_maximum() {
    let (app, store) = offline_app(3).await;
    let first = offline_tokens(&app).await;
    let started = decode_jwt_payload(first["access_token"].as_str().unwrap())["iat"]
        .as_i64()
        .unwrap();
    let (st, renewed) = refresh(&app, first["refresh_token"].as_str().unwrap()).await;
    assert_eq!(st, StatusCode::OK, "{renewed}");
    let access = decode_jwt_payload(renewed["access_token"].as_str().unwrap());
    assert!(
        access["exp"].as_i64().unwrap() <= started + 4,
        "an access token ends with its offline session: {access}"
    );
    let rt = renewed["refresh_token"].as_str().unwrap();
    let family = store
        .get_refresh_family("dev", rt.split_once('.').unwrap().0)
        .await
        .unwrap()
        .unwrap();
    assert!(family.expires_at.as_second() <= started + 4, "{family:?}");

    tokio::time::sleep(std::time::Duration::from_millis(4100)).await;
    let (st, refused) = refresh(&app, rt).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "invalid_grant");
}

#[tokio::test]
async fn an_offline_session_survives_the_end_of_its_sso_session() {
    let (app, store) = offline_app(5_184_000).await;
    let first = offline_tokens(&app).await;
    let rt = first["refresh_token"].as_str().unwrap();
    let family = store
        .get_refresh_family("dev", rt.split_once('.').unwrap().0)
        .await
        .unwrap()
        .unwrap();
    let sid = family.sso_session_id.clone();
    assert!(
        store.get_sso_session("dev", &sid).await.unwrap().is_some(),
        "the grant began in a session: {sid}"
    );

    // The person signs out of the browser: the SSO session ends.
    store.delete_sso_session("dev", &sid).await.unwrap();
    let (st, renewed) = refresh(&app, rt).await;
    assert_eq!(st, StatusCode::OK, "{renewed}");
    let access = renewed["access_token"].as_str().unwrap();
    assert!(
        decode_jwt_payload(access).get("sid").is_none(),
        "an offline token names no SSO session"
    );
    assert!(renewed.get("session_state").is_none());
    assert!(active(&app, access).await, "introspection keeps it active");

    // A password change that signs out the other devices ends the offline session too.
    store
        .revoke_offline_families_for_user("dev", &family.user_id)
        .await
        .unwrap();
    assert!(!active(&app, access).await);
    let (st, _) = refresh(&app, renewed["refresh_token"].as_str().unwrap()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}
