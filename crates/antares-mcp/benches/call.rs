// SPDX-License-Identifier: EUPL-1.2
//! What a tool call costs on top of the operation it runs (MCP section 8,
//! surfaces-test-plan section 5): `tools/list` with its engine probe, and
//! `tools/call query_entities` against the same query on the NGSI-LD root
//! and through the view, which is most of what the call costs. It prints
//! the ADR-0027 section 6 row "MCP tool call", the call's mean against the
//! root query's mean + 0.2 ms + 1 µs per Entity; it reports and never fails
//! (the `bench-surfaces` job decides that, T-3523).
//!
//!     cargo bench -p antares-mcp --bench call

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::io::Write as _;
use std::time::Instant;
use tower::ServiceExt;

const MODERN: &str = "2026-07-28";

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> StatusCode {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let _ = resp.into_body().collect().await;
    status
}

async fn rpc(app: &Router, method: &str, name: Option<&str>, params: Value) -> StatusCode {
    let mut params = params;
    if let Some(o) = params.as_object_mut() {
        o.insert(
            "_meta".into(),
            json!({
                "io.modelcontextprotocol/protocolVersion": MODERN,
                "io.modelcontextprotocol/clientInfo": {"name": "bench", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {}
            }),
        );
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let mut headers = vec![
        ("Host", "localhost"),
        ("Accept", "application/json, text/event-stream"),
        ("MCP-Protocol-Version", MODERN),
        ("Mcp-Method", method),
    ];
    if let Some(n) = name {
        headers.push(("Mcp-Name", n));
    }
    send(app, "POST", "/x/views/cars/mcp", &headers, Some(&body)).await
}

/// Runs `f` `n` times, reports microseconds per run and returns them.
async fn time<F: std::future::Future<Output = StatusCode>>(
    name: &str,
    n: u32,
    mut f: impl FnMut() -> F,
) -> std::io::Result<u128> {
    let start = Instant::now();
    for _ in 0..n {
        let status = f().await;
        if status != StatusCode::OK {
            return Err(std::io::Error::other(format!("{name}: {status}")));
        }
    }
    let per = start
        .elapsed()
        .as_micros()
        .checked_div(u128::from(n))
        .unwrap_or(0);
    writeln!(std::io::stdout(), "{name}: {per} µs/op ({n} runs)")?;
    Ok(per)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_mcp::Mcp)?;
    for n in 0..100 {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "Car",
            "speed": {"type": "Property", "value": n}});
        send(&app, "POST", "/ngsi-ld/v1/entities", &[], Some(&car)).await;
    }
    let view = json!({"id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities", "retrieve_entity", "retrieve_entity_types"]});
    send(&app, "PUT", "/q/views/cars", &[], Some(&view)).await;
    let n = 300;
    let root = time("NGSI-LD root: query 20 Cars", n, || {
        send(
            &app,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&limit=20",
            &[],
            None,
        )
    })
    .await?;
    let viewed = broker.serve_view("cars")?;
    time("view: query 20 Cars", n, || {
        send(
            &viewed,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&limit=20&offset=0&count=true",
            &[],
            None,
        )
    })
    .await?;
    time("tools/list", n, || rpc(&app, "tools/list", None, json!({}))).await?;
    let call = time("tools/call query_entities: 20 Cars", n, || {
        rpc(
            &app,
            "tools/call",
            Some("query_entities"),
            json!({"name": "query_entities", "arguments": {"type": ["Car"]}}),
        )
    })
    .await?;
    let budget = root.saturating_add(200 + 20);
    writeln!(
        std::io::stdout(),
        "budget MCP tool call (20 Entities): mean {call} µs of {budget} µs {}",
        if call <= budget { "within" } else { "over" }
    )?;
    Ok(())
}
