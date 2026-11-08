#![forbid(unsafe_code)]
//! Refusals of the native admin operations through OData, MCP and the Keycloak-compatible routes,
//! served with the `Mount` extension the `jc` router hands over: missing or malformed parameters
//! answer 400, an unknown target 404 (W5), and the cross-realm master admin reaches a realm that
//! does not exist only as 404.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;
use common::*;

const HOST: &str = "https://idm.dev.example.test";
const BASE: &str = "/api/v1/realms/dev/odata";
const STEWARD: &str = "de300000-0000-4000-8000-000000000001";
const ANALYST: &str = "de300000-0000-4000-8000-000000000010";

struct Api {
    app: Router,
    store: std::sync::Arc<kaisatsu_store_memory::MemoryStore>,
}

impl Api {
    async fn new() -> Self {
        let (state, store, _) = setup_dev_realm_state().await;
        let mount = Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap();
        Self {
            app: router(state, mount.clone()).layer(Extension(mount)),
            store,
        }
    }

    async fn token_in(&self, realm: &str, sub: &str, extra: Value) -> String {
        let keys = self.store.list_signing_keys(realm).await.unwrap();
        let key = keys.iter().find(|k| k.status == "ACTIVE").unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut claims = json!({ "iss": format!("{HOST}/realms/{realm}"), "sub": sub,
                                 "azp": "admin-cli", "typ": "Bearer", "iat": now, "exp": now + 600 });
        for (k, v) in extra.as_object().unwrap() {
            claims[k] = v.clone();
        }
        sign_jwt(key, self.store.kek(), Some("JWT"), &claims).unwrap()
    }

    async fn admin(&self) -> String {
        self.token_in(
            "dev",
            "admin-1",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await
    }

    async fn call_with(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, Value, String) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let req = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                b.body(Body::from(v.to_string())).unwrap()
            }
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (st, serde_json::from_str(&text).unwrap_or(Value::Null), text)
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, String) {
        self.call_with(method, uri, token, body, &[]).await
    }

    /// Asserts an OData error with the given status and a message containing `needle`.
    async fn refused(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
        status: StatusCode,
        needle: &str,
    ) {
        let (st, err, raw) = self.call(method, uri, token, body).await;
        assert_eq!(st, status, "{uri}: {raw}");
        let msg = err["error"]["message"].as_str().unwrap_or(&raw).to_owned();
        assert!(msg.contains(needle), "{uri}: {needle:?} not in {raw}");
    }
}

#[tokio::test]
async fn bound_action_parameter_refusals() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let person = |op: &str| format!("{BASE}/People('{STEWARD}')/{op}");
    let bad = StatusCode::BAD_REQUEST;

    api.refused(
        Method::POST,
        &format!("{BASE}/People('')/AddToGroup"),
        &admin,
        Some(json!({ "groupId": "g" })),
        bad,
        "entity key",
    )
    .await;
    api.refused(
        Method::POST,
        &person("AddToGroup"),
        &admin,
        Some(json!({})),
        bad,
        "groupId is required",
    )
    .await;
    api.refused(
        Method::POST,
        &person("MapClientRoles"),
        &admin,
        Some(json!({ "roles": ["x"] })),
        bad,
        "clientUuid is required",
    )
    .await;
    api.refused(
        Method::POST,
        &person("MapClientRoles"),
        &admin,
        Some(json!({ "clientUuid": "any", "roles": "not-a-list" })),
        bad,
        "must be a list",
    )
    .await;

    // People writes with a body that is no person.
    api.refused(
        Method::POST,
        &format!("{BASE}/People"),
        &admin,
        Some(json!({ "username": 7 })),
        bad,
        "invalid person",
    )
    .await;
    api.refused(
        Method::PATCH,
        &format!("{BASE}/People('{STEWARD}')"),
        &admin,
        Some(json!({ "enabled": "sometimes" })),
        bad,
        "invalid person",
    )
    .await;

    // Realm/Apply needs a realm document.
    api.refused(
        Method::POST,
        &format!("{BASE}/Realms('dev')/Apply"),
        &admin,
        Some(json!({ "configuration": "realm.json", "dryRun": true })),
        bad,
        "configuration must be a realm object",
    )
    .await;

    // Unknown targets answer 404.
    let nf = StatusCode::NOT_FOUND;
    api.refused(
        Method::POST,
        &format!("{BASE}/Consents('no-such-consent')/Revoke"),
        &admin,
        None,
        nf,
        "consent not found",
    )
    .await;
    api.refused(
        Method::POST,
        &format!("{BASE}/EmailOutbox('no-such-item')/Retry"),
        &admin,
        None,
        nf,
        "outbox item not found",
    )
    .await;
    api.refused(
        Method::POST,
        &format!("{BASE}/Nowhere('x')/Rotate"),
        &admin,
        None,
        nf,
        "unknown entity set",
    )
    .await;

    // Function literals: an unquoted word is no OData literal.
    api.refused(
        Method::GET,
        &format!("{BASE}/Allowed(operation='read',object=everything)"),
        &admin,
        None,
        bad,
        "bad literal",
    )
    .await;
}

#[tokio::test]
async fn send_required_actions_link_refusals() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let mut realm = api.store.get_realm("dev").await.unwrap().unwrap();
    realm.smtp_server = Some(SmtpServerConfig::default());
    api.store.upsert_realm(realm).await.unwrap();
    let uri = format!("{BASE}/People('{STEWARD}')/SendRequiredActions");
    let bad = StatusCode::BAD_REQUEST;

    api.refused(
        Method::POST,
        &uri,
        &admin,
        Some(json!({ "actions": ["UPDATE_PASSWORD"], "redirectUri": "https://portal.example.test/" })),
        bad,
        "requires client_id",
    )
    .await;
    api.refused(
        Method::POST,
        &uri,
        &admin,
        Some(
            json!({ "actions": ["UPDATE_PASSWORD"], "clientId": "no-such-client",
                     "redirectUri": "https://portal.example.test/" }),
        ),
        bad,
        "unknown client_id",
    )
    .await;
    api.refused(
        Method::POST,
        &uri,
        &admin,
        Some(json!({ "actions": "UPDATE_PASSWORD" })),
        bad,
        "must be a list",
    )
    .await;
}

#[tokio::test]
async fn passkey_challenge_is_single_use() {
    let api = Api::new().await;
    let me = api.token_in("dev", ANALYST, json!({})).await;
    let uri = format!("{BASE}/People('{ANALYST}')/RegisterPasskey");
    let (st, opts, raw) = api.call(Method::POST, &uri, &me, Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    let issued = opts["value"]["challenge"].as_str().unwrap().to_owned();
    // The first use spends the challenge (and fails on the empty credential) ...
    api.refused(
        Method::POST,
        &uri,
        &me,
        Some(json!({ "credential": {}, "challenge": issued })),
        StatusCode::BAD_REQUEST,
        "credential",
    )
    .await;
    // ... so a replay of it is refused before any credential is parsed.
    api.refused(
        Method::POST,
        &uri,
        &me,
        Some(json!({ "credential": {}, "challenge": issued })),
        StatusCode::BAD_REQUEST,
        "already used",
    )
    .await;
}

#[tokio::test]
async fn set_routes_with_the_mount() {
    let api = Api::new().await;
    let admin = api.admin().await;

    let (st, _, raw) = api
        .call(Method::GET, &format!("{BASE}/People/$count"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(raw.parse::<u64>().unwrap() > 0, "{raw}");
    let (st, rows, raw) = api
        .call(Method::GET, &format!("{BASE}/People/$export"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(!rows.as_array().unwrap().is_empty());
    let (st, dec, raw) = api
        .call(
            Method::GET,
            &format!("{BASE}/People/$allowed?op=update&key={STEWARD}"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(dec.is_object(), "{raw}");
    api.refused(
        Method::GET,
        &format!("{BASE}/Nowhere/$allowed"),
        &admin,
        None,
        StatusCode::NOT_FOUND,
        "unknown entity set",
    )
    .await;
    api.refused(
        Method::GET,
        &format!("{BASE}/People/$allowed?op=juggle"),
        &admin,
        None,
        StatusCode::BAD_REQUEST,
        "unknown operation",
    )
    .await;

    // A key read on a set the service does not have.
    let (st, _, raw) = api
        .call(Method::GET, &format!("{BASE}/Nowhere('x')"), &admin, None)
        .await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::BAD_REQUEST,
        "{st}: {raw}"
    );

    // People writes through the mount: an update of a person outside every set is 404.
    let (st, _, raw) = api
        .call(
            Method::PATCH,
            &format!("{BASE}/People('no-such-person')"),
            &admin,
            Some(json!({ "firstName": "X" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{raw}");

    // MCP with the mount: initialize answers.
    let (st, rpc, raw) = api
        .call(
            Method::POST,
            "/api/v1/realms/dev/mcp",
            &admin,
            Some(json!({ "jsonrpc": "2.0", "id": 7, "method": "initialize" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert_eq!(rpc["result"]["serverInfo"]["name"], "kaisatsu-mcp");
}

#[tokio::test]
async fn compat_context_and_lookups() {
    let api = Api::new().await;
    let admin = api.admin().await;

    // A second factor claimed through `amr` and an organization picked by header: a realm admin
    // keeps the realm-wide read with or without the header.
    let otp = api
        .token_in(
            "dev",
            "admin-otp",
            json!({ "amr": ["pwd", "otp"], "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;
    let (st, _, raw) = api
        .call_with(
            Method::GET,
            "/admin/realms/dev/users?max=1",
            &otp,
            None,
            &[("x-kaisatsu-org", " helsinki ")],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    let (st, _, raw) = api
        .call(Method::GET, "/admin/realms/dev/users?max=1", &otp, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");

    // A person who is no realm admin mapping roles of a client nobody has: 404, not a write.
    let member = api.token_in("dev", ANALYST, json!({})).await;
    let (st, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/MapClientRoles"),
            &member,
            Some(json!({ "clientUuid": "no-such-client", "roles": ["r"] })),
        )
        .await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN,
        "{st}: {raw}"
    );

    // A mapper created twice under one name replaces the first.
    let client = api
        .store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .expect("edge client");
    let uri = format!(
        "/admin/realms/dev/clients/{}/protocol-mappers/models",
        client.id
    );
    let mapper = json!({ "name": "t-errors-aud", "protocol": "openid-connect",
                         "protocolMapper": "oidc-audience-mapper",
                         "config": { "included.custom.audience": "one" } });
    for _ in 0..2 {
        let (st, _, raw) = api
            .call(Method::POST, &uri, &admin, Some(mapper.clone()))
            .await;
        assert_eq!(st, StatusCode::CREATED, "{raw}");
    }
    let stored = api
        .store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored
            .protocol_mappers
            .iter()
            .filter(|m| m.name == "t-errors-aud")
            .count(),
        1
    );

    // The master realm admin acts across realms; a realm that does not exist is 404.
    api.store
        .upsert_realm(RealmConfig {
            id: "master".into(),
            realm: "master".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let root = api
        .token_in(
            "master",
            "root",
            json!({ "realm_access": { "roles": ["admin"] } }),
        )
        .await;
    let (st, _, raw) = api
        .call(Method::GET, "/admin/realms/nowhere", &root, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{raw}");
    let (st, _, raw) = api
        .call(
            Method::POST,
            "/api/v1/realms/nowhere/odata/SigningKeys/Rotate",
            &root,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{raw}");
}
