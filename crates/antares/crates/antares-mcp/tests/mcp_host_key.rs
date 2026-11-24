// SPDX-License-Identifier: EUPL-1.2
//! T-7507: the key a host hands over seals `requestState` where no key file
//! is named. A process of its own: the codec is built once per process, and
//! a key shorter than 32 bytes leaves no codec, so every write is refused.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const MODERN: &str = "2026-07-28";

async fn send(
    app: &axum::Router,
    uri: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> (StatusCode, Value) {
    let bytes = body.to_string();
    let mut req = Request::builder()
        .method(if uri.starts_with("/q/") {
            "PUT"
        } else {
            "POST"
        })
        .uri(uri)
        .header("Host", "localhost")
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("Content-Length", bytes.len());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_host_key_too_short_to_seal_refuses_every_write() -> Result<(), String> {
    antares_mcp::default_request_state_key(b"eight by");
    // only the first key counts
    antares_mcp::default_request_state_key(&[7; 32]);
    let app = TestBroker::new().await?.serve(antares_mcp::Mcp)?;
    let view = json!({"id": "writes", "tenant": "w", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["create_entity"]});
    let (status, text) = send(&app, "/q/views/writes", &[], &view).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car",
        "speed": {"type": "Property", "value": 1}});
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
    "name": "create_entity", "arguments": {"entity": car},
    "_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {"elicitation": {"form": {}}}
    }}});
    let headers = [
        ("MCP-Protocol-Version", MODERN),
        ("Mcp-Method", "tools/call"),
        ("Mcp-Name", "create_entity"),
    ];
    let (status, answer) = send(&app, "/x/views/writes/mcp", &headers, &call).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer.pointer("/result/isError"),
        Some(&json!(true)),
        "{answer}"
    );
    assert_eq!(
        answer.pointer("/result/content/0/text"),
        Some(&json!(
            "this endpoint cannot ask the person now; nothing was written"
        )),
        "{answer}"
    );
    Ok(())
}
