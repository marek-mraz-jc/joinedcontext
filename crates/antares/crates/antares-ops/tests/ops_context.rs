// SPDX-License-Identifier: EUPL-1.2
//! `OpsRequest::context`: a surface names what the terms of its operation
//! mean, so a `q` reaches Attributes outside the core @context, and the
//! outer client's `Link` header cannot redefine them.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// `/x/asker/{with}`: Cars with `speed>1`, `speed` defined by the surface
/// when `{with}` is `with`, by the request's context otherwise.
struct Asker;

impl Surface for Asker {
    fn name(&self) -> &'static str {
        "asker"
    }
    fn prefix(&self) -> &'static str {
        "/x/asker"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{with}",
            get(move |caller: Caller, Path(with): Path<String>| async move {
                let mut req = OpsRequest::new(
                    Method::GET,
                    "/ngsi-ld/v1/entities?type=https%3A%2F%2Fexample.org%2FCar&q=speed%3E1",
                );
                if with == "with" {
                    req = req.context(json!({"speed": "https://example.org/speed"}));
                }
                match ops.run(OperationId::QueryEntities, &caller, req).await {
                    Ok(answer) => answer,
                    Err(refusal) => axum::response::IntoResponse::into_response(refusal.status()),
                }
            }),
        )
    }
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let Ok(resp) = app.clone().oneshot(req).await;
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

fn get_req(uri: &str, link: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().uri(uri).header("NGSILD-Tenant", "city");
    if let Some(link) = link {
        req = req.header("Link", link);
    }
    req.body(Body::empty()).unwrap_or_default()
}

async fn broker() -> Result<(TestBroker, Router), String> {
    let b = TestBroker::new().await?;
    let app = b.serve(Asker)?;
    // Rule M4: a surface stays closed until a view loads.
    let view = r#"{"id":"cars","tenant":"city","selector":["Car"],
        "rules":[{"types":["Car"],"attributes":["speed"]}],"operations":["query_entities"]}"#;
    let req = Request::put("/q/views/cars")
        .header("Content-Type", "application/json")
        .header("Content-Length", view.len())
        .body(Body::from(view))
        .map_err(|e| e.to_string())?;
    let (status, answer) = send(&app, req).await;
    if status != StatusCode::CREATED {
        return Err(format!("view: {status} {answer}"));
    }
    for (n, speed) in [(1, 5), (2, 0)] {
        let body = json!({
            "id": format!("urn:ngsi-ld:Car:{n}"), "type": "https://example.org/Car",
            "https://example.org/speed": {"type": "Property", "value": speed},
            "speed": {"type": "Property", "value": 9},
        })
        .to_string();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/ngsi-ld/v1/entities")
            .header("NGSILD-Tenant", "city")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .map_err(|e| e.to_string())?;
        let (status, answer) = send(&app, req).await;
        if status != StatusCode::CREATED {
            return Err(format!("{status} {answer}"));
        }
    }
    Ok((b, app))
}

fn ids(answer: &Value) -> Vec<&str> {
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_surface_defines_its_terms_and_the_client_link_does_not_redefine_them() {
    let (_b, app) = broker().await.expect("broker");
    let (status, answer) = send(&app, get_req("/x/asker/with", None)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(ids(&answer), vec!["urn:ngsi-ld:Car:1"], "{answer}");
    let link = r#"<https://example.invalid/evil.jsonld>; rel="http://www.w3.org/ns/json-ld#context"; type="application/ld+json""#;
    let (status, answer) = send(&app, get_req("/x/asker/with", Some(link))).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        ids(&answer),
        vec!["urn:ngsi-ld:Car:1"],
        "the client's Link is not read"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_context_the_request_reads_its_names_as_before() {
    let (_b, app) = broker().await.expect("broker");
    let (status, answer) = send(&app, get_req("/x/asker/without", None)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        ids(&answer),
        vec!["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"],
        "speed is the core default term, 9 on both: {answer}"
    );
}
