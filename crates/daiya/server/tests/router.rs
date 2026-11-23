#![forbid(unsafe_code)]
//! The mountable router's own routes (R10, R11): `/mount-info`, the security headers on every
//! answer, no MCP door of daiya's own, and the CLI dispatcher refuses a command the registry
//! does not know.

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
async fn daiya_serves_no_mcp_of_its_own() {
    let (_, app) = app();
    for (method, uri) in [
        ("POST", "/mcp"),
        ("GET", "/mcp/tools"),
        ("POST", "/mcp/call"),
    ] {
        let call = json!({ "name": "allowed", "arguments": { "operation": "apply" } });
        let (st, _, err) = send(&app, method, uri, None, Some(call.clone())).await;
        assert_eq!(st, 401, "{method} {uri}: {err}");
        let (st, _, err) = send(&app, method, uri, Some("forged"), Some(call.clone())).await;
        assert_eq!(st, 401, "{method} {uri}: {err}");
        let (st, headers, err) = send(&app, method, uri, Some("t"), Some(call)).await;
        assert_eq!(st, 404, "{method} {uri}: {err}");
        assert!(err["error"]["message"].is_string(), "{err}");
        assert_eq!(headers["cache-control"], "private, no-store");
    }
}

#[tokio::test]
async fn mount_info_reports_the_default_mount() {
    let (_, app) = app();
    let (st, headers, info) = send(&app, "GET", "/mount-info", None, None).await;
    assert_eq!(st, 200);
    assert_eq!(info["module"], "daiya");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
    // Every answer defaults to not being stored by an intermediary.
    assert_eq!(headers["cache-control"], "private, no-store");
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
    assert_eq!(info["pathPrefix"], "/d");
    assert_eq!(info["odataUrl"], "https://dash.example/d/odata/v4");
    assert_eq!(
        headers["x-daiya-cookie-name"].to_str().unwrap(),
        info["sessionCookieName"].as_str().unwrap()
    );
}

/// A mount whose host holds a byte no header may carry still answers; it leaves out the public
/// base it cannot write rather than send a broken value.
#[tokio::test]
async fn a_mount_that_is_no_header_value_answers_without_its_public_base() {
    let (state, _) = app();
    let app = router(
        state,
        Mount::new("daiya", "https", "dash\u{1}.example", "/d\u{1}").unwrap(),
    );
    let (st, headers, info) = send(&app, "GET", "/mount-info", None, None).await;
    assert_eq!(st, 200, "{info}");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers.get("x-daiya-public-base").is_none(), "{headers:?}");
    // The cookie name keeps only the characters a cookie name may hold.
    assert_eq!(headers["x-daiya-cookie-name"], "__Secure-wk_cr_session");
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
