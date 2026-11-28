// SPDX-License-Identifier: EUPL-1.2
//! T-5816 with a confirmation store that fails: the page's routes answer a
//! 500 that names no store detail, and a retry whose decision cannot be
//! taken writes nothing and says to call again. A process of its own: the
//! page is read once per process.

#[path = "../../antares-ops/tests/support/fault_store.rs"]
mod fault_store;

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use fault_store::FaultStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const PAGE: &str = "https://portal.example.org/mcp-confirmations/{id}";
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

/// `create_entity` of `car` by `person`, with `more` params, from a client
/// that opens URLs for the person.
async fn create(app: &Router, person: &str, car: &Value, more: &Value) -> Value {
    let mut params = json!({"name": "create_entity", "arguments": {"entity": car},
    "_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {"elicitation": {"url": {}}}
    }});
    let more = more.as_object().cloned().unwrap_or_default();
    params
        .as_object_mut()
        .into_iter()
        .for_each(|p| p.extend(more.clone()));
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params});
    let (status, answer) = send(
        app,
        "POST",
        "/x/views/writes/mcp",
        &[
            ("MCP-Protocol-Version", MODERN),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "create_entity"),
            ("x-jc-subject", person),
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

async fn decide(app: &Router, id: &str, decision: &str, person: &str) -> (StatusCode, Value) {
    let body = json!({"decision": decision, "subject": {"x-jc-subject": person}});
    send(
        app,
        "PUT",
        &format!("/q/mcp-confirmations/{id}"),
        &[],
        &body,
    )
    .await
}

/// A broker over the fault store with the decision page and a `writes`
/// view of Cars.
async fn writes() -> Result<(TestBroker, Router, Arc<FaultStore>), String> {
    std::env::set_var(antares_mcp::CONFIRM_URL_ENV, PAGE);
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-jc-subject");
    let (state, fault) = FaultStore::state("antares-test");
    let broker = TestBroker::over(state).await?;
    let routes = antares_mcp::confirmations(broker.confirmations());
    let broker = broker.with_admin(routes);
    let app = broker.serve(antares_mcp::Mcp)?;
    let view = json!({"id": "writes", "tenant": "w", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["create_entity", "query_entities"]});
    let (status, text) = send(&app, "PUT", "/q/views/writes", &[], &view).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    Ok((broker, app, fault))
}

/// The question `asked` opened: its id and the state a retry carries.
fn question(asked: &Value) -> (String, Value) {
    let id = asked
        .pointer("/inputRequests/confirm/params/elicitationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    (id, asked.get("requestState").cloned().unwrap_or_default())
}

const FAILED: &str = "the questions cannot be read or kept now; try again shortly";

#[tokio::test]
async fn a_failing_confirmation_store_answers_500_and_writes_nothing() -> Result<(), String> {
    let (_broker, app, fault) = writes().await?;
    let car = json!({"id": "urn:ngsi-ld:Car:f1", "type": "Car",
        "speed": {"type": "Property", "value": 5}});
    let (id, state) = question(&create(&app, "alice", &car, &json!({})).await);
    assert_ne!(id, "", "a question was opened");

    // reads fail: the page can neither show nor decide the question
    fault.fail(|method, _, _| method == "confirmation_find");
    let path = format!("/q/mcp-confirmations/{id}");
    let (status, body) = send(&app, "GET", &path, &[], &Value::Null).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body.get("detail"), Some(&json!(FAILED)), "{body}");
    assert!(!body.to_string().contains("injected"), "{body}");
    let (status, body) = decide(&app, &id, "accept", "alice").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    // the write of the decision fails after its read
    fault.fail(|method, _, _| method == "confirmation_decide");
    let (status, body) = decide(&app, &id, "accept", "alice").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body.get("detail"), Some(&json!(FAILED)), "{body}");

    // healed: the decision is recorded, and may change until it is taken
    fault.fail(|_, _, _| false);
    let (status, body) = decide(&app, &id, "decline", "alice").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("decision"), Some(&json!("decline")), "{body}");
    assert_eq!(decide(&app, &id, "accept", "alice").await.0, StatusCode::OK);

    // the retry cannot take the decision: nothing is written, call again
    fault.fail(|method, _, _| method == "confirmation_take");
    let retried = create(&app, "alice", &car, &json!({"requestState": state})).await;
    assert!(
        refusal(&retried).contains("could not be read or kept now; nothing was written"),
        "{retried}"
    );
    fault.fail(|_, _, _| false);
    let written = create(&app, "alice", &car, &json!({"requestState": state})).await;
    assert_eq!(
        refusal(&written),
        "",
        "the decision is still there: {written}"
    );
    Ok(())
}

/// The asker's retry takes an accepted question between the page's read of it and the write of
/// a changed decision: the write finds no open question, so the page answers the 404 an unknown
/// one gets, and the write the retry made is the one the person accepted.
#[tokio::test]
async fn a_question_taken_between_the_read_and_the_decision_is_not_open() -> Result<(), String> {
    let (_broker, app, fault) = writes().await?;
    let car = json!({"id": "urn:ngsi-ld:Car:f9", "type": "Car",
        "speed": {"type": "Property", "value": 9}});
    let (id, state) = question(&create(&app, "alice", &car, &json!({})).await);
    assert_ne!(id, "", "a question was opened");
    assert_eq!(decide(&app, &id, "accept", "alice").await.0, StatusCode::OK);
    let (retry, retried) = (app.clone(), car.clone());
    *fault
        .before
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((
        "confirmation_decide",
        Box::pin(async move {
            let written = create(&retry, "alice", &retried, &json!({"requestState": state})).await;
            assert_eq!(refusal(&written), "", "{written}");
        }),
    ));
    let (status, body) = decide(&app, &id, "decline", "alice").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        fault.before.lock().is_ok_and(|b| b.is_none()),
        "the retry ran"
    );
    let (status, stored) = send(
        &app,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:f9",
        &[("NGSILD-Tenant", "w")],
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    Ok(())
}

/// `area_overview` over a store that cannot read: the first read of a type
/// fails, and the tool answers an error whole rather than a part of one.
#[tokio::test]
async fn an_area_read_the_store_cannot_serve_is_an_error_not_a_part() -> Result<(), String> {
    let (_broker, app, fault) = writes().await?;
    let view = json!({"id": "geo", "tenant": "w", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "location"]}],
        "operations": ["query_entities"]});
    let (status, text) = send(&app, "PUT", "/q/views/geo", &[], &view).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    // a Tenant that holds nothing answers before any read; one Car makes the read reach the store
    let car = json!({"id": "urn:ngsi-ld:Car:area", "type": "Car",
        "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.94, 60.17]}}});
    let (status, text) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entities",
        &[("NGSILD-Tenant", "w")],
        &car,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let params = json!({"name": "area_overview",
    "arguments": {"geometry": "Point", "coordinates": [24.94, 60.17], "maxDistance": 500},
    "_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    }});
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params});
    let ask = || {
        send(
            &app,
            "POST",
            "/x/views/geo/mcp",
            &[
                ("MCP-Protocol-Version", MODERN),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "area_overview"),
                ("x-jc-subject", "alice"),
            ],
            &call,
        )
    };
    fault.fail(|method, _, _| method.starts_with("query_"));
    let (status, answer) = ask().await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer.pointer("/result/isError"),
        Some(&json!(true)),
        "{answer}"
    );
    assert!(!answer.to_string().contains("injected"), "{answer}");
    fault.fail(|_, _, _| false);
    let (status, answer) = ask().await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_ne!(
        answer.pointer("/result/isError"),
        Some(&json!(true)),
        "{answer}"
    );
    Ok(())
}

#[tokio::test]
async fn a_question_the_store_cannot_keep_is_refused_and_writes_nothing() -> Result<(), String> {
    let (_broker, app, fault) = writes().await?;
    let car = json!({"id": "urn:ngsi-ld:Car:f2", "type": "Car",
        "speed": {"type": "Property", "value": 5}});
    fault.fail(|method, _, _| method == "confirmation_ask");
    let asked = create(&app, "alice", &car, &json!({})).await;
    assert!(
        refusal(&asked).contains("could not be read or kept now; nothing was written"),
        "{asked}"
    );
    assert_eq!(question(&asked).0, "", "no question was opened: {asked}");
    fault.fail(|_, _, _| false);
    let (id, _) = question(&create(&app, "alice", &car, &json!({})).await);
    assert_ne!(id, "", "healed: a question is opened");
    Ok(())
}
