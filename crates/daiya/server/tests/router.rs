#![forbid(unsafe_code)]
//! The mountable router's own routes (R10, R11): `GET /mcp/tools` and `POST /mcp/call` answer
//! with the same tools and decisions as the in-process MCP dispatcher, errors keep their HTTP
//! status and OData error body, every answer carries the security headers, and the CLI
//! dispatcher refuses a command the registry does not know.

use axum::body::Body;
use axum::http::Request;
use daiya_api::TokenBinding;
use daiya_model::{InstallationId, InstanceId, OrgId, RequestCtx, Subject};
use daiya_server::{DaiyaState, ServerConfig, execute_cli_command, router};
use daiya_store_memory::MemoryStore;
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const ORG: OrgId = OrgId::from_u128(5);

fn app() -> (DaiyaState, axum::Router) {
    let state = DaiyaState::for_standalone(Arc::new(MemoryStore::new()), ORG).with_tokens(vec![
        TokenBinding::new("t", Subject::authenticated("admin", vec![ORG], vec![])),
    ]);
    let app = router(
        state.clone(),
        Mount::new("daiya", "https", "dash.example", "/d").unwrap(),
    );
    (state, app)
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, axum::http::HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn mcp_over_http_lists_tools_and_calls_them_as_the_caller() {
    let (state, app) = app();
    let (st, headers, tools) = send(&app, "GET", "/mcp/tools", None, None).await;
    assert_eq!(st, 200);
    assert_eq!(
        tools["tools"].as_array().unwrap().len(),
        state.registry.operations.len() + 1
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
    assert_eq!(
        headers["referrer-policy"],
        "strict-origin-when-cross-origin"
    );
    assert_eq!(headers["x-daiya-public-base"], "https://dash.example/d");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'self'")
    );

    let (st, _, decision) = send(
        &app,
        "POST",
        "/mcp/call",
        Some("t"),
        Some(json!({ "name": "daiya_allowed", "arguments": { "operation": "apply" } })),
    )
    .await;
    assert_eq!(st, 200, "{decision}");
    assert_eq!(decision["allowed"], true, "{decision}");

    // The same call without the token runs as the anonymous subject.
    let (st, _, decision) = send(
        &app,
        "POST",
        "/mcp/call",
        None,
        Some(json!({ "name": "daiya_allowed", "arguments": { "operation": "apply" } })),
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(decision["allowed"], false, "{decision}");

    // Errors keep their status and an OData error body; `arguments` may be left out.
    let (st, _, err) = send(
        &app,
        "POST",
        "/mcp/call",
        Some("t"),
        Some(json!({ "name": "daiya_export_dashboard" })),
    )
    .await;
    assert_eq!(st, 400, "{err}");
    assert!(err["error"]["message"].is_string(), "{err}");
    let (st, _, err) = send(
        &app,
        "POST",
        "/mcp/call",
        Some("t"),
        Some(json!({ "name": "nope", "arguments": {} })),
    )
    .await;
    assert_eq!(st, 404, "{err}");
}

#[tokio::test]
async fn mount_info_reports_the_default_mount() {
    let (_, app) = app();
    let (st, headers, info) = send(&app, "GET", "/mount-info", None, None).await;
    assert_eq!(st, 200);
    assert_eq!(info["module"], "daiya");
    assert_eq!(info["pathPrefix"], "/d");
    assert_eq!(info["odataUrl"], "https://dash.example/d/odata/v4");
    assert_eq!(
        headers["x-daiya-cookie-name"].to_str().unwrap(),
        info["sessionCookieName"].as_str().unwrap()
    );
}

#[tokio::test]
async fn the_cli_dispatcher_refuses_an_unknown_command() {
    let (state, _) = app();
    let ctx = RequestCtx::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        Subject::authenticated("admin", vec![ORG], vec![]),
    );
    let err = execute_cli_command(&state, &ctx, "drop-everything", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 404);
    assert!(err.to_string().contains("drop-everything"), "{err}");
}

#[test]
fn the_default_server_config_listens_on_loopback_only() {
    let cfg = ServerConfig::default();
    assert!(cfg.listen_addr.ip().is_loopback());
    assert_eq!(cfg.listen_addr.port(), 3000);
    let round: ServerConfig = serde_json::from_value(serde_json::to_value(&cfg).unwrap()).unwrap();
    assert_eq!(round.listen_addr, cfg.listen_addr);
}
