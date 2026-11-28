// SPDX-License-Identifier: EUPL-1.2
//! The form confirmation (no decision page configured) over a confirmation
//! store that fails when the retry spends its state: nothing is written and
//! the person is told to call again; healed, the same answer writes once. A
//! process of its own: the asking mode is read once per process.

#[path = "../../antares-ops/tests/support/fault_store.rs"]
mod fault_store;

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use fault_store::FaultStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const MODERN: &str = "2026-07-28";

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> (StatusCode, Value) {
    let bytes = body.to_string();
    let mut req = Request::builder()
        .method(method)
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

/// `create_entity` of `car` by alice from a client that asks by form, with
/// `more` params.
async fn create(app: &Router, car: &Value, more: &Value) -> Value {
    let mut params = json!({"name": "create_entity", "arguments": {"entity": car},
    "_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {"elicitation": {"form": {}}}
    }});
    if let (Some(p), Some(m)) = (params.as_object_mut(), more.as_object()) {
        p.extend(m.clone());
    }
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params});
    let (status, answer) = send(
        app,
        "POST",
        "/x/views/writes/mcp",
        &[
            ("MCP-Protocol-Version", MODERN),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "create_entity"),
            ("x-jc-subject", "alice"),
        ],
        &call,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer.get("result").cloned().unwrap_or_default()
}

/// The answer's tool error text, empty when it is none.
fn refusal(result: &Value) -> &str {
    if result.get("isError") == Some(&json!(true)) {
        result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        ""
    }
}

#[tokio::test]
async fn a_form_answer_the_store_cannot_spend_writes_nothing() -> Result<(), String> {
    std::env::remove_var(antares_mcp::CONFIRM_URL_ENV);
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-jc-subject");
    let (state, fault) = FaultStore::state("antares-test");
    let broker = TestBroker::over(state).await?;
    let app = broker.serve(antares_mcp::Mcp)?;
    let view = json!({"id": "writes", "tenant": "w", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["create_entity", "query_entities"]});
    let (status, text) = send(&app, "PUT", "/q/views/writes", &[], &view).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let car = json!({"id": "urn:ngsi-ld:Car:form1", "type": "Car",
        "speed": {"type": "Property", "value": 5}});
    let asked = create(&app, &car, &json!({})).await;
    let state = asked.get("requestState").cloned().unwrap_or_default();
    assert!(state.is_string(), "a question was asked: {asked}");
    let accept = json!({"requestState": state,
        "inputResponses": {"confirm": {"action": "accept", "content": {"confirm": true}}}});

    fault.fail(|method, _, _| method == "confirmation_spend");
    let retried = create(&app, &car, &accept).await;
    assert!(
        refusal(&retried).contains("could not be read or kept now; nothing was written"),
        "{retried}"
    );
    assert!(!retried.to_string().contains("injected"), "{retried}");

    fault.fail(|_, _, _| false);
    let written = create(&app, &car, &accept).await;
    assert_eq!(refusal(&written), "", "the state was not spent: {written}");
    let again = create(&app, &car, &accept).await;
    assert!(
        !refusal(&again).is_empty(),
        "a spent answer writes once: {again}"
    );
    Ok(())
}
