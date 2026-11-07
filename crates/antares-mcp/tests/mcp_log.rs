// SPDX-License-Identifier: EUPL-1.2
//! MC19: the log line of a tool call. A process of its own: tracing caches
//! each callsite's interest per process, so a subscriber set for one test
//! of a shared binary can miss a line another test's thread registered.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// A writer the log lines of this test go to.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> (StatusCode, String) {
    let bytes = body.to_string();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// One line per call with the view, the tool, the operation and its status,
/// and none of the argument values.
#[tokio::test]
async fn a_tool_call_is_logged_without_its_arguments() -> Result<(), String> {
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_mcp::Mcp)?;
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": {"type": "Property", "value": 1}});
    let (status, text) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entities",
        &[("NGSILD-Tenant", "city")],
        &car,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let view = json!({"id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}], "operations": ["query_entities"]});
    let (status, text) = send(&app, "PUT", "/q/views/cars", &[], &view).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let modern = "2026-07-28";
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "query_entities", "arguments": {"type": ["Car"], "q": "speed<31337"},
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": modern,
            "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    }});
    let (status, text) = send(
        &app,
        "POST",
        "/x/views/cars/mcp",
        &[
            ("Host", "localhost"),
            ("Accept", "application/json, text/event-stream"),
            ("MCP-Protocol-Version", modern),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "query_entities"),
        ],
        &call,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(text.contains("\"isError\":false"), "{text}");
    let log = lines
        .0
        .lock()
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default();
    let line = log
        .lines()
        .find(|l| l.contains("mcp tool call"))
        .unwrap_or_default();
    for want in [
        "view=cars",
        "tool=\"query_entities\"",
        "operation=\"query_entities\"",
        "status=200",
    ] {
        assert!(line.contains(want), "{want} in {line}");
    }
    assert!(!log.contains("31337"), "{log}");
    Ok(())
}
