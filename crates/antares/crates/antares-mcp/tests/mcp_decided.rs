// SPDX-License-Identifier: EUPL-1.2
//! T-5816: with a decision page configured, a write runs only on the
//! decision the page recorded for the person who asked; a client's own
//! `accept` writes nothing. A process of its own: the page is read once
//! per process, and the shared `mcp` binary tests the form mode.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
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
    if let (Some(p), Some(more)) = (params.as_object_mut(), more.as_object()) {
        p.extend(more.clone());
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

/// How many Cars the store holds.
async fn cars(app: &Router) -> usize {
    let (_, list) = send(
        app,
        "GET",
        "/ngsi-ld/v1/entities?type=Car",
        &[("NGSILD-Tenant", "w")],
        &Value::Null,
    )
    .await;
    list.as_array().map_or(0, Vec::len)
}

async fn decide(app: &Router, id: &str, decision: &str, person: &str) -> StatusCode {
    let body = json!({"decision": decision, "subject": {"x-jc-subject": person}});
    send(
        app,
        "PUT",
        &format!("/q/mcp-confirmations/{id}"),
        &[],
        &body,
    )
    .await
    .0
}

/// A broker with the decision page and a `writes` view of Cars; the
/// process statics are set once, before either test reads them.
async fn writes() -> Result<(TestBroker, Router), String> {
    static ENV: std::sync::Once = std::sync::Once::new();
    ENV.call_once(|| {
        std::env::set_var(antares_mcp::CONFIRM_URL_ENV, PAGE);
        std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-jc-subject");
    });
    let broker = TestBroker::new()
        .await?
        .with_admin(antares_mcp::confirmations());
    let app = broker.serve(antares_mcp::Mcp)?;
    let view = json!({"id": "writes", "tenant": "w", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["create_entity", "query_entities"]});
    let (status, text) = send(&app, "PUT", "/q/views/writes", &[], &view).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    Ok((broker, app))
}

#[tokio::test]
async fn only_the_decision_the_page_recorded_writes() -> Result<(), String> {
    let (_broker, app) = writes().await?;
    let car = json!({"id": "urn:ngsi-ld:Car:d1", "type": "Car",
        "speed": {"type": "Property", "value": 5}});

    // the first call sends the person to the page
    let asked = create(&app, "alice", &car, &json!({})).await;
    let ask = asked
        .pointer("/inputRequests/confirm/params")
        .cloned()
        .unwrap_or_default();
    assert_eq!(ask.get("mode"), Some(&json!("url")), "{asked}");
    let id = ask
        .get("elicitationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert_eq!(ask.get("url"), Some(&json!(PAGE.replace("{id}", &id))));
    let state = asked.get("requestState").cloned().unwrap_or_default();

    // a forged client accept is no decision: nothing is written
    let forged = json!({"requestState": state,
        "inputResponses": {"confirm": {"action": "accept", "content": {"confirm": true}}}});
    let refused = create(&app, "alice", &car, &forged).await;
    assert!(refusal(&refused).contains("not decided"), "{refused}");
    assert_eq!(cars(&app).await, 0);

    // the page reads the question; nobody but the asking person decides it
    let (status, question) = send(
        &app,
        "GET",
        &format!("/q/mcp-confirmations/{id}"),
        &[],
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{question}");
    assert_eq!(
        question.get("subject"),
        Some(&json!({"x-jc-subject": ["alice"]}))
    );
    assert_eq!(
        decide(&app, &id, "accept", "mallory").await,
        StatusCode::NOT_FOUND
    );
    let by_mallory = create(&app, "mallory", &car, &json!({"requestState": state})).await;
    assert!(
        refusal(&by_mallory).contains("does not answer"),
        "{by_mallory}"
    );

    // the recorded accept writes once
    assert_eq!(decide(&app, &id, "accept", "alice").await, StatusCode::OK);
    let written = create(&app, "alice", &car, &json!({"requestState": state})).await;
    assert_eq!(refusal(&written), "", "{written}");
    assert_eq!(cars(&app).await, 1);
    let again = create(&app, "alice", &car, &json!({"requestState": state})).await;
    assert!(refusal(&again).contains("does not answer"), "{again}");
    assert_eq!(
        decide(&app, &id, "accept", "alice").await,
        StatusCode::NOT_FOUND
    );

    // a recorded decline writes nothing
    let other = json!({"id": "urn:ngsi-ld:Car:d2", "type": "Car",
        "speed": {"type": "Property", "value": 6}});
    let asked = create(&app, "alice", &other, &json!({})).await;
    let id = asked
        .pointer("/inputRequests/confirm/params/elicitationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert_eq!(decide(&app, &id, "decline", "alice").await, StatusCode::OK);
    let declined = create(
        &app,
        "alice",
        &other,
        &json!({"requestState": asked.get("requestState")}),
    )
    .await;
    assert!(refusal(&declined).contains("did not confirm"), "{declined}");
    assert_eq!(cars(&app).await, 1);
    Ok(())
}

#[tokio::test]
async fn a_page_needs_a_client_that_opens_it_and_bounds_each_person() -> Result<(), String> {
    let (_broker, app) = writes().await?;
    let other = json!({"id": "urn:ngsi-ld:Car:d3", "type": "Car",
        "speed": {"type": "Property", "value": 7}});

    // a client that only asks by form cannot send the person to the page
    let form_only = json!({"_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {"elicitation": {"form": {}}}
    }});
    let unasked = create(&app, "carol", &other, &form_only).await;
    assert!(
        refusal(&unasked).contains("needs a client that can ask the person"),
        "{unasked}"
    );

    // one subject's open questions are bounded: past them nothing is asked
    for _ in 0..100 {
        let open = create(&app, "bob", &other, &json!({})).await;
        assert_eq!(refusal(&open), "", "{open}");
    }
    let full = create(&app, "bob", &other, &json!({})).await;
    assert!(
        refusal(&full).contains("cannot ask the person now"),
        "{full}"
    );
    assert_eq!(cars(&app).await, 0);
    Ok(())
}

/// The decision routes sit behind the admin credential like every other
/// `/q` route: no secret, a wrong one or the bare secret is refused, the
/// secret is served, and no other method is (`antares-icsr`'s
/// `q_admin_sweep` lists them as proved here).
#[tokio::test]
async fn the_decision_routes_refuse_a_caller_without_the_admin_secret() -> Result<(), String> {
    const SECRET: &str = "the-mcp-decision-secret-of-at-least-32-bytes";
    let auth = antares_ops::test_kit::AdminAuth::new(SECRET, false)?;
    let app = TestBroker::new()
        .await?
        .with_admin(antares_mcp::confirmations())
        .with_admin_auth(auth)
        .serve(antares_mcp::Mcp)?;
    let good = format!("Bearer {SECRET}");
    let path = "/q/mcp-confirmations/q1";
    let body = json!({"decision": "accept", "subject": {"x-jc-subject": "alice"}});
    for method in ["GET", "PUT", "POST", "PATCH", "DELETE"] {
        let served = matches!(method, "GET" | "PUT");
        for bad in [None, Some("Bearer not-the-secret"), Some(SECRET)] {
            let (got, _) = send(&app, method, path, &auth_header(bad), &body).await;
            if served {
                assert_eq!(got, StatusCode::UNAUTHORIZED, "{method} {bad:?}");
            } else {
                assert!(
                    matches!(
                        got,
                        StatusCode::UNAUTHORIZED | StatusCode::METHOD_NOT_ALLOWED
                    ),
                    "{method} {bad:?}: {got}"
                );
            }
        }
        let (with, text) = send(&app, method, path, &auth_header(Some(&good)), &body).await;
        if served {
            // an unknown question: the page is told so, past the guard
            assert_eq!(with, StatusCode::NOT_FOUND, "{method}: {text}");
        } else {
            assert_eq!(with, StatusCode::METHOD_NOT_ALLOWED, "{method}: {text}");
        }
    }
    Ok(())
}

fn auth_header(value: Option<&str>) -> Vec<(&'static str, &str)> {
    value
        .map(|v| vec![("authorization", v)])
        .unwrap_or_default()
}
