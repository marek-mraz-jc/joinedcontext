#![allow(dead_code)]
//! Shared fixtures for the server integration tests: the RI `dev` realm applied from
//! `import/tests/fixtures/ri_dev.json`, a form POST helper and JWT payload decoding.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_import::{ApplyMode, ApplyOptions, apply_realm_files};
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::KaisatsuState;
use kaisatsu_store_memory::MemoryStore;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

pub fn decode_jwt_payload(jwt: &str) -> Value {
    let payload = jwt.split('.').nth(1).expect("jwt payload segment");
    let bytes = URL_SAFE_NO_PAD.decode(payload).expect("base64url decode");
    serde_json::from_slice(&bytes).expect("json payload")
}

pub async fn setup_dev_realm_state() -> (KaisatsuState, Arc<MemoryStore>, Kek) {
    let kek = Kek::generate_for_tests();
    let store = Arc::new(MemoryStore::new(kek.clone()));
    let state = setup_dev_realm_on(store.clone(), kek.clone()).await;
    (state, store, kek)
}

/// The realm of `ri_dev.json` on `store` (memory or PostgreSQL), and the state serving it.
pub async fn setup_dev_realm_on(store: Arc<dyn Store>, kek: Kek) -> KaisatsuState {
    let guard = OutboundGuard::default();

    let ri_raw = include_str!("../../../import/tests/fixtures/ri_dev.json");
    let ri_doc: Value = serde_json::from_str(ri_raw).expect("parse ri_dev.json");
    let opts = ApplyOptions {
        mode: ApplyMode::NoDelete,
        dry_run: false,
        allow_literal_secrets: false,
        secrets_dir: None,
        env_vars: BTreeMap::from([
            (
                "USER_PASSWORD_DEMO_STEWARD".into(),
                "StewardPass123!Valid".into(),
            ),
            (
                "USER_TOTP_DEMO_STEWARD".into(),
                "steward-totp-seed-123456".into(),
            ),
            (
                "USER_PASSWORD_DEMO_ORGMEMBER".into(),
                "MemberPass123!Valid".into(),
            ),
            (
                "USER_PASSWORD_DEMO_ANALYST".into(),
                "AnalystPass123!Valid".into(),
            ),
            ("CLIENT_SECRET_EDGE".into(), "edge-secret-123".into()),
            (
                "CLIENT_SECRET_APISIX_GATEWAY".into(),
                "apisix-secret-123".into(),
            ),
            ("CLIENT_SECRET_GRAFANA".into(), "grafana-secret-123".into()),
            ("CLIENT_SECRET_GITEA".into(), "gitea-secret-123".into()),
            ("CLIENT_SECRET_CKAN".into(), "ckan-secret-123".into()),
        ]),
    };
    apply_realm_files(&store, &[("ri_dev.json".into(), ri_doc)], &opts)
        .await
        .expect("apply ri_dev.json");

    // Also link `endpoint:counters1234` as an optional scope of `portal-api` (as sync-admin-permissions.sh does)
    let ep_scope = store
        .upsert_client_scope(ClientScopeRepresentation {
            id: "sc-ep-counters".into(),
            realm_id: "dev".into(),
            name: "endpoint:counters1234".into(),
            protocol: "openid-connect".into(),
            attributes: BTreeMap::from([("include.in.token.scope".into(), "true".into())]),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut portal_api = store
        .get_client_by_client_id("dev", "portal-api")
        .await
        .unwrap()
        .unwrap();
    portal_api.optional_client_scopes.push(ep_scope.name);
    store.upsert_client(portal_api).await.unwrap();

    KaisatsuState::new(store, kek, guard, Arc::new(LocalAuthorizer))
}

pub async fn post_form(
    app: &axum::Router,
    uri: &str,
    pairs: &[(&str, &str)],
    cookies: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(
            header::USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0",
        );
    if let Some(ck) = cookies {
        builder = builder.header(header::COOKIE, ck);
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(encoded)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

pub fn extract_hidden_input(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = html.find(&marker).expect("find input marker") + marker.len();
    let end = html[start..].find('"').expect("find input quote") + start;
    html[start..end].to_owned()
}

/// The `name=value` pair of the auth session cookie set by `authorize` (K1: every login step
/// sends it back, as a browser does).
pub fn auth_cookie(headers: &axum::http::HeaderMap) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.contains("wk_auth="))
        .and_then(|v| v.split(';').next())
        .expect("authorize sets the auth session cookie")
        .to_owned()
}
