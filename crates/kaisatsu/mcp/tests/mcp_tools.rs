#![forbid(unsafe_code)]
//! The MCP door (W5): `initialize`, `tools/list` filtered by the caller's rights (realm tools only
//! for a realm admin), `tools/call` through the same executor and decision as OData, and JSON-RPC
//! errors for an unknown tool or method.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_admin::AdminState;
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, sign_jwt};
use kaisatsu_mcp::mcp_router;
use kaisatsu_oidc::{OidcState, OutboundGuard};
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use tower::ServiceExt;

struct Fixture {
    app: axum::Router,
    realm_admin: String,
    org_admin: String,
}

async fn fixture() -> Fixture {
    let kek = Kek::generate_for_tests();
    let store = Arc::new(MemoryStore::new(kek.clone()));
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .upsert_organization(Organization {
            id: "helsinki".into(),
            realm_id: "dev".into(),
            name: "Helsinki".into(),
            alias: "helsinki".into(),
            enabled: true,
            description: String::new(),
            domains: Vec::new(),
            attributes: Default::default(),
            managed_by: None,
        })
        .await
        .unwrap();
    store
        .upsert_membership(Membership {
            id: "m-hel".into(),
            realm_id: "dev".into(),
            org_id: "helsinki".into(),
            person_id: "org-admin-hel".into(),
            org_roles: vec!["org-admin".into()],
            created_at: jc_time::TimestampTz::now(),
        })
        .await
        .unwrap();
    let key = store.list_signing_keys("dev").await.unwrap().remove(0);
    let now = jc_time::TimestampTz::now().as_second();
    let token = |sub: &str, extra: Value| {
        let mut claims = json!({
            "iss": "https://idm.localhost/realms/dev", "sub": sub, "azp": "admin-cli",
            "typ": "Bearer", "iat": now, "exp": now + 3600,
        });
        claims
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        sign_jwt(&key, &kek, Some("JWT"), &claims).unwrap()
    };
    let realm_admin = token(
        "admin-1",
        json!({ "realm_access": { "roles": ["realm-admin"] } }),
    );
    let org_admin = token(
        "org-admin-hel",
        json!({ "azp": "portal-api", "jc_org": ["helsinki"] }),
    );
    let oidc = OidcState {
        store: store.clone(),
        kek,
        guard: OutboundGuard::default(),
        default_mount: Mount::default(),
        second_factor: Default::default(),
    };
    let app = mcp_router(AdminState::new(oidc, Arc::new(LocalAuthorizer)));
    Fixture {
        app,
        realm_admin,
        org_admin,
    }
}

async fn rpc(app: &axum::Router, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req =
        Request::post("/api/v1/realms/dev/mcp").header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn call(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn tool_names(answer: &Value) -> BTreeSet<String> {
    answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn initialize_answers_the_server_and_its_protocol() {
    let f = fixture().await;
    let (st, answer) = rpc(
        &f.app,
        Some(&f.realm_admin),
        call(7, "initialize", json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{answer}");
    assert_eq!(answer["id"], 7);
    assert_eq!(answer["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(answer["result"]["serverInfo"]["name"], "kaisatsu-mcp");
    assert!(answer.get("error").is_none());
}

#[tokio::test]
async fn a_caller_without_a_token_is_refused() {
    let f = fixture().await;
    let (st, _) = rpc(&f.app, None, call(1, "initialize", json!({}))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tools_list_shows_realm_tools_to_a_realm_admin_only() {
    let f = fixture().await;
    let realm_tools: BTreeSet<String> = OperationRegistry::new()
        .operations()
        .iter()
        .filter(|op| matches!(op.entity_set, Some("Realms" | "SigningKeys")))
        .map(|op| op.name.to_string())
        .collect();
    assert!(!realm_tools.is_empty());
    let (_, answer) = rpc(
        &f.app,
        Some(&f.realm_admin),
        call(2, "tools/list", json!({})),
    )
    .await;
    let admin = tool_names(&answer);
    assert!(realm_tools.is_subset(&admin), "{admin:?}");
    let (_, answer) = rpc(&f.app, Some(&f.org_admin), call(3, "tools/list", json!({}))).await;
    let org = tool_names(&answer);
    assert!(org.is_disjoint(&realm_tools), "{org:?}");
    assert!(!org.is_empty(), "an org admin keeps the person tools");
    assert!(org.is_subset(&admin));
    let tool = &answer["result"]["tools"][0];
    assert!(
        tool["description"].is_string() && tool["inputSchema"].is_object(),
        "{tool}"
    );
}

#[tokio::test]
async fn tools_call_runs_the_operation_or_answers_a_json_rpc_error() {
    let f = fixture().await;
    let (st, answer) = rpc(
        &f.app,
        Some(&f.realm_admin),
        call(4, "tools/call", json!({ "name": "allowed", "arguments": { "payload": { "operation": "keys_rotate" } } })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{answer}");
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let text: Value =
        serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(text.is_object() || text.is_boolean(), "{text}");

    let (_, answer) = rpc(
        &f.app,
        Some(&f.org_admin),
        call(
            5,
            "tools/call",
            json!({ "name": "keys_rotate", "arguments": {} }),
        ),
    )
    .await;
    assert_eq!(answer["error"]["code"], -32000, "{answer}");
    let status = answer["error"]["data"]["status"].as_u64().unwrap();
    assert!(status == 403 || status == 404, "{answer}");
    assert!(answer.get("result").is_none());

    let (_, answer) = rpc(
        &f.app,
        Some(&f.realm_admin),
        call(6, "tools/call", json!({ "name": "drop_tables" })),
    )
    .await;
    assert_eq!(
        answer["error"],
        json!({ "code": -32601, "message": "Unknown tool" })
    );
    let (_, answer) = rpc(
        &f.app,
        Some(&f.realm_admin),
        call(8, "resources/list", json!({})),
    )
    .await;
    assert_eq!(
        answer["error"],
        json!({ "code": -32601, "message": "Method not found" })
    );
    assert_eq!(answer["id"], 8);
}
