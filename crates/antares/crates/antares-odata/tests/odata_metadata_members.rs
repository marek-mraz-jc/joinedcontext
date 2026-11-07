// SPDX-License-Identifier: EUPL-1.2
//! The metadata members a grant passes per Attribute (platform ADR-N-066 2.4, DM-139):
//! OData reads through the NGSI-LD API in process, so its rows carry only the members the
//! engine passes, and a `$filter` on a stripped one is refused naming it.

use antares_ops::policy::{
    Decision, DecisionFuture, Filter, Metadata, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tower::ServiceExt;

const ROOT: &str = "/x/views/cars/odata/v4";
const CITY: &str = "https://example.org/city/";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed"]}},
  "slots": {"speed": {"range": "float"}}
}"#;

/// `speed` passes `unitCode` only: its `observedAt` is stripped.
struct UnitOnly;

impl PolicyEngine for UnitOnly {
    fn name(&self) -> &'static str {
        "unit-only"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let answer = if ["5.7.1", "5.7.2"].contains(&op.clause) {
            Decision::Filter(Filter {
                metadata: Some(Box::new(Metadata {
                    named: BTreeMap::from([(
                        format!("{CITY}speed"),
                        BTreeSet::from(["unitCode".to_owned()]),
                    )]),
                    rest: BTreeSet::new(),
                })),
                ..Filter::default()
            })
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(answer))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(app: &Router, method: Method, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city")
        .header("Prefer", "odata.include-annotations=\"*\"");
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap_or_default(),
        )
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

async fn broker() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?.with_policy(Arc::new(UnitOnly));
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed"]}],
        "operations": ["query_entities", "retrieve_entity"]
    })
    .to_string();
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": format!("{CITY}Car"),
                     format!("{CITY}speed"): {"type": "Property", "value": 50.0,
                         "unitCode": "KMH", "observedAt": "2026-11-01T10:00:00Z"}})
    .to_string();
    for (method, uri, body) in [
        (Method::PUT, "/q/views/cars", view),
        (Method::PUT, "/q/schemas/city", SCHEMA.to_owned()),
        (Method::POST, "/ngsi-ld/v1/entities", car),
    ] {
        let (status, answer) = send(&app, method, uri, Some(&body)).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_carries_only_the_annotations_the_grant_passes() -> Result<(), String> {
    let (_b, app) = broker().await?;
    let (status, body) = send(&app, Method::GET, &format!("{ROOT}/Car"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = body["value"][0].as_object().cloned().unwrap_or_default();
    assert_eq!(row.get("speed"), Some(&json!(50.0)), "{body}");
    let annotations: Vec<&String> = row.keys().filter(|k| k.contains('@')).collect();
    assert!(
        annotations.iter().any(|k| k.contains("unitCode")),
        "unitCode passes: {body}"
    );
    assert!(
        !annotations.iter().any(|k| k.contains("observedAt")),
        "observedAt is stripped: {body}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filter_on_a_stripped_member_is_refused_naming_it() -> Result<(), String> {
    let (_b, app) = broker().await?;
    let (status, body) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=speed/@NgsiLd.observedAt%20ge%202026-01-01T00:00:00Z"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("observedAt"), "{body}");
    let (status, body) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=speed%20gt%2010"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Ok(())
}
