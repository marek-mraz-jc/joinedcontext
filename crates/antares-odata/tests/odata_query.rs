// SPDX-License-Identifier: EUPL-1.2
//! `POST {resource}/$query` (Protocol 11.2.5.2, odata-linkml-mapping Q22,
//! surfaces-threat-model O24): the options in the body are served as the
//! GET they stand for, so the engine judges the same `q`, never a body it
//! does not see.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
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

/// Allows, and keeps the `q` of every query it is asked about.
#[derive(Default)]
struct Seen(Mutex<Vec<String>>);

impl PolicyEngine for Seen {
    fn name(&self) -> &'static str {
        "seen"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        if op.clause == "5.7.2" {
            if let Ok(mut seen) = self.0.lock() {
                seen.push(format!("{:?}", op.q));
            }
        }
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

impl Seen {
    fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut s| std::mem::take(&mut *s))
            .unwrap_or_default()
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<(&str, &str)>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
    if let Some((kind, b)) = body {
        req = req
            .header("Content-Type", kind)
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(
                body.map(|(_, b)| b).unwrap_or_default().to_owned(),
            ))
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

async fn broker(engine: Arc<Seen>) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?.with_policy(engine);
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed"]}],
        "operations": ["query_entities", "retrieve_entity"]
    })
    .to_string();
    let mut writes = vec![
        ("/q/views/cars".to_owned(), view),
        ("/q/schemas/city".to_owned(), SCHEMA.to_owned()),
    ];
    for n in 1..=3 {
        writes.push((
            "/ngsi-ld/v1/entities".to_owned(),
            json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"),
                   format!("{CITY}speed"): {"type": "Property", "value": f64::from(n) * 10.0}})
            .to_string(),
        ));
    }
    for (uri, body) in &writes {
        let method = if uri.starts_with("/q/") {
            Method::PUT
        } else {
            Method::POST
        };
        let (status, answer) = send(&app, method, uri, Some(("application/json", body))).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    Ok((b, app))
}

fn ids(body: &Value) -> Vec<&str> {
    body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_query_segment_reaches_the_engine_with_q() {
    let engine = Arc::new(Seen::default());
    let (_b, app) = broker(engine.clone()).await.expect("broker");
    let options = "$filter=speed%20gt%2015&$orderby=speed%20desc&$count=true";
    let (status, by_get) = send(&app, Method::GET, &format!("{ROOT}/Car?{options}"), None).await;
    assert_eq!(status, StatusCode::OK, "{by_get}");
    let get_seen = engine.take();
    for kind in [
        "text/plain",
        "application/x-www-form-urlencoded; charset=utf-8",
    ] {
        let (status, by_post) = send(
            &app,
            Method::POST,
            &format!("{ROOT}/Car/$query"),
            Some((kind, options)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{kind}: {by_post}");
        assert_eq!(by_post, by_get, "{kind}: the same answer as the GET");
        assert_eq!(engine.take(), get_seen, "{kind}: the engine saw the same q");
    }
    assert!(
        get_seen.iter().all(|q| q.starts_with("Some(")),
        "the filter reached the engine: {get_seen:?}"
    );
    assert_eq!(ids(&by_get), ["urn:ngsi-ld:Car:3", "urn:ngsi-ld:Car:2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_body_joins_the_url_options_and_is_bounded() {
    let engine = Arc::new(Seen::default());
    let (_b, app) = broker(engine).await.expect("broker");
    let (status, body) = send(
        &app,
        Method::POST,
        &format!("{ROOT}/Car/$query?$top=1"),
        Some(("application/x-www-form-urlencoded", "$orderby=speed+desc")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["urn:ngsi-ld:Car:3"], "{body}");
    let (status, body) = send(
        &app,
        Method::POST,
        &format!("{ROOT}/Car/$query"),
        Some(("text/plain", "")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "an empty body is no option: {body}");
    assert_eq!(ids(&body).len(), 3, "{body}");
    for (kind, text, want) in [
        ("application/json", "{}", StatusCode::UNSUPPORTED_MEDIA_TYPE),
        ("text/plain", "$filter=speed gt 1", StatusCode::BAD_REQUEST),
        ("text/plain", "$top=1&$top=2", StatusCode::BAD_REQUEST),
        ("text/plain", "$index=1", StatusCode::NOT_IMPLEMENTED),
    ] {
        let (status, body) = send(
            &app,
            Method::POST,
            &format!("{ROOT}/Car/$query"),
            Some((kind, text)),
        )
        .await;
        assert_eq!(status, want, "{kind} {text}: {body}");
        assert!(
            body["error"]["code"].is_string(),
            "{kind} {text}: an OData error: {body}"
        );
    }
    let long = format!("$filter=speed%20eq%20{}", "1".repeat(9000));
    let (status, body) = send(
        &app,
        Method::POST,
        &format!("{ROOT}/Car/$query"),
        Some(("text/plain", &long)),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
}

/// T-5062: a read without `$count` asks the broker one Entity past the page
/// instead of its count; the next link still appears exactly when Entities
/// are left, at a server page of 2 and of 3 over the three Cars, and under a
/// `$top` that ends inside the answer.
#[tokio::test(flavor = "multi_thread")]
async fn the_next_link_without_count_follows_the_entities_left() {
    let engine = Arc::new(Seen::default());
    let (_b, app) = broker(engine).await.expect("broker");
    for (page, top, first, linked) in [
        (2, None, 2, true),
        (3, None, 3, false),
        (10, None, 3, false),
        (1, Some(2), 1, true),
        (2, Some(2), 2, false),
    ] {
        let top = top.map(|t| format!("&$top={t}")).unwrap_or_default();
        let resp = app
            .clone()
            .oneshot(
                Request::get(format!("{ROOT}/Car?$select=speed{top}"))
                    .header("NGSILD-Tenant", "city")
                    .header("Prefer", format!("odata.maxpagesize={page}"))
                    .body(Body::empty())
                    .unwrap_or_default(),
            )
            .await;
        let Ok(resp) = resp;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map(http_body_util::Collected::to_bytes)
            .unwrap_or_default();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(ids(&body).len(), first, "page {page}{top}: {body}");
        assert_eq!(
            body.get("@odata.nextLink").is_some(),
            linked,
            "page {page}{top}: {body}"
        );
        assert!(
            body.get("@odata.count").is_none(),
            "no $count, no count: {body}"
        );
    }
}
