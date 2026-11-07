// SPDX-License-Identifier: EUPL-1.2
//! Matrix 7.1 (`operation-matrix.yaml`, `odata_without_ngsi_ld_operation`):
//! every OData feature no NGSI-LD operation stands behind is refused with
//! the status the matrix gives it, before any gate or store access
//! (surfaces-threat-model O13, O19). The rows are read from the matrix, so
//! a row added there without a request here fails this test.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const ROOT: &str = "/x/views/cars/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "tags", "owner"]}, "Person": {"slots": ["name"]}},
  "slots": {"speed": {"range": "float"}, "owner": {"range": "Person"}, "name": {},
            "tags": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}}}
}"#;

/// One request per row: method, path below the service root, headers.
/// Request headers of a row.
type Headers = &'static [(&'static str, &'static str)];

const ROWS: &[(&str, &str, &str, Headers)] = &[
    ("odata_index", "GET", "Car?$index=1", &[]),
    ("odata_schemaversion", "GET", "Car?$schemaversion=2", &[]),
    (
        "odata_actions",
        "POST",
        "Car('urn:ngsi-ld:Car:1')/ns.act",
        &[],
    ),
    ("odata_functions", "GET", "Car/ns.fn()", &[]),
    ("odata_crossjoin", "GET", "$crossjoin(Car,Car)", &[]),
    ("odata_all", "GET", "$all", &[]),
    ("odata_each", "PATCH", "Car/$each", &[]),
    ("odata_delta_update", "PATCH", "Car", &[]),
    (
        "odata_media_entities",
        "GET",
        "Car('urn:ngsi-ld:Car:1')/$value",
        &[],
    ),
    (
        "odata_stream_properties",
        "PUT",
        "Car('urn:ngsi-ld:Car:1')/speed/$value",
        &[],
    ),
    ("odata_batch_atomicity", "POST", "$batch", &[]),
    ("odata_upsert", "PATCH", "Car('urn:ngsi-ld:Car:99')", &[]),
    ("odata_derived_types", "GET", "Car/ns.Derived", &[]),
    (
        "odata_expand_options",
        "GET",
        "Car?$expand=owner($top=1)",
        &[],
    ),
    ("odata_deltatoken", "GET", "Car?$deltatoken=x", &[]),
    (
        "odata_root",
        "GET",
        "Car?$filter=$root/Car('x')/speed%20gt%201",
        &[],
    ),
    (
        "odata_it_this",
        "GET",
        "Car?$filter=$it/speed%20gt%201",
        &[],
    ),
    ("odata_custom_option", "GET", "Car?colour=red", &[]),
    (
        "odata_filter_segment",
        "GET",
        "Car/$filter(speed%20gt%201)",
        &[],
    ),
    (
        "odata_ordinal",
        "GET",
        "Car('urn:ngsi-ld:Car:1')/tags/1",
        &[],
    ),
    (
        "odata_deep_update",
        "PATCH",
        "Car('urn:ngsi-ld:Car:1')",
        &[],
    ),
    (
        "odata_collection_append",
        "POST",
        "Car('urn:ngsi-ld:Car:1')/tags",
        &[],
    ),
    ("odata_delete_collection", "DELETE", "Car", &[]),
    ("odata_batch_references", "POST", "$batch", &[]),
    (
        "odata_conditional_write",
        "PATCH",
        "Car('urn:ngsi-ld:Car:1')",
        &[("If-Match", "*")],
    ),
    ("odata_write_readonly", "POST", "$metadata", &[]),
];

/// Every further request a row's binding names, with the same answer.
const MORE: &[(&str, &str, &str)] = &[
    ("odata_write_readonly", "POST", ""),
    ("odata_write_readonly", "PUT", "Car/$count"),
    ("odata_write_readonly", "DELETE", "$metadata"),
    ("odata_write_readonly", "PUT", "Car/$query"),
    ("odata_upsert", "PUT", "Car('urn:ngsi-ld:Car:99')"),
    ("odata_each", "DELETE", "Car/$each"),
];

/// The body a row's request carries, where `{}` is not the form it names.
const BODIES: &[(&str, &str)] = &[
    (
        "odata_deep_update",
        r#"{"owner": {"id": "urn:ngsi-ld:Person:5", "name": "new"}}"#,
    ),
    (
        "odata_batch_atomicity",
        r#"{"requests": [{"id": "1", "method": "get", "url": "Car", "atomicityGroup": "g"}]}"#,
    ),
    (
        "odata_batch_references",
        r#"{"requests": [{"id": "1", "method": "get", "url": "Car('urn:ngsi-ld:Car:1')"},
                         {"id": "2", "method": "get", "url": "$1/speed"}]}"#,
    ),
];

/// The construct each `501` row of the matrix names (T-5907): the message
/// is `<construct>: no NGSI-LD equivalent (ADR-0024 decision 5)`, a fixed
/// text that echoes no request value.
const NAMED: &[(&str, &str)] = &[
    ("odata_index", "$index"),
    (
        "odata_schemaversion",
        "a $schemaversion other than the served one",
    ),
    ("odata_actions", "type cast or bound operation"),
    ("odata_functions", "type cast or bound operation"),
    ("odata_crossjoin", "$crossjoin and $all"),
    ("odata_all", "$crossjoin and $all"),
    ("odata_each", "$each"),
    (
        "odata_delta_update",
        "a write to a whole entity set (delta update)",
    ),
    ("odata_media_entities", "media resource"),
    ("odata_stream_properties", "a write to /$value"),
    ("odata_batch_atomicity", "atomicity groups"),
    ("odata_derived_types", "type cast or bound operation"),
    ("odata_expand_options", "nested query option"),
    ("odata_deltatoken", "$deltatoken"),
    ("odata_root", "$root"),
    ("odata_it_this", "$it and $this"),
    ("odata_filter_segment", "$filter path segment"),
    ("odata_ordinal", "ordinal index"),
    ("odata_deep_update", "deep update"),
    (
        "odata_collection_append",
        "a write appending to a collection property",
    ),
    (
        "odata_batch_references",
        "references to another member's result",
    ),
    (
        "odata_conditional_write",
        "a conditional write (If-Match, If-None-Match)",
    ),
];

/// The `501`s the surface answers after the URL parsed, beside the matrix
/// rows: method, path, body, the construct named.
const AFTER_PARSE: &[(&str, &str, &str, &str)] = &[
    (
        "GET",
        "Car('urn:ngsi-ld:Car:1')?$filter=speed%20gt%201",
        "",
        "$filter, $search or $apply on one Entity",
    ),
    (
        "GET",
        "Car/$count?$apply=filter(speed%20gt%201)",
        "",
        "$apply under /$count",
    ),
    (
        "GET",
        "Car?$apply=filter(speed%20gt%201)&$filter=speed%20gt%202",
        "",
        "$apply beside another query option, under /$ref or /$count",
    ),
    (
        "GET",
        "Car?$apply=groupby((speed))/search(red)",
        "",
        "search after aggregate in $apply",
    ),
    (
        "GET",
        "Car?$select=owner",
        "",
        "$select of a navigation, a path or an annotation",
    ),
    (
        "GET",
        "Car?$expand=owner($levels=2)",
        "",
        "a nested $expand, $levels or a path in $expand",
    ),
    (
        "GET",
        "Car('urn:ngsi-ld:Car:1')/speed?$top=1",
        "",
        "a query option other than $select below one Entity",
    ),
    ("GET", "Car?$id=red", "", "$id outside $entity"),
    (
        "GET",
        "Car?$filter=speed%20gt%20null%20add%201",
        "",
        "this $filter expression",
    ),
    (
        "PATCH",
        "Car('urn:ngsi-ld:Car:1')?$select=speed",
        r#"{"speed": 1}"#,
        "a query option on a write",
    ),
    (
        "POST",
        "Car('urn:ngsi-ld:Car:1')/owner/$ref",
        r#"{"@odata.id": "Person('urn:ngsi-ld:Person:1')"}"#,
        "a write to this resource path",
    ),
    (
        "PUT",
        "$entity?$id=Car('urn:ngsi-ld:Car:1')",
        "{}",
        "a write to this resource path",
    ),
];

fn named(construct: &str) -> String {
    format!("{construct}: no NGSI-LD equivalent (ADR-0024 decision 5)")
}

/// The rows the operation they stand for answers, so its gate is asked:
/// an upsert is the Merge or Replace Entity of a key the store does not
/// hold (W7), whose 404 is the one a hidden Entity gets.
const GATED: &[&str] = &["odata_upsert"];

/// Counts every decision asked of it, and allows.
#[derive(Default)]
struct Counting(AtomicUsize);

impl PolicyEngine for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// `(id, status, code)` of every OData row of matrix 7.1 that refuses.
fn matrix_rows() -> Vec<(String, u16, String)> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/analysis/operation-matrix.yaml"
    );
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut rows = Vec::new();
    let mut inside = false;
    let (mut id, mut status) = (String::new(), None::<u16>);
    for line in text.lines() {
        if line.starts_with("odata_without_ngsi_ld_operation:") {
            inside = true;
            continue;
        }
        if inside && !line.starts_with(' ') && !line.starts_with('-') {
            break;
        }
        let field = |name: &str| {
            line.trim()
                .strip_prefix(name)
                .map(|v| v.trim().trim_matches('\'').to_owned())
        };
        if let Some(v) = field("- id:") {
            id = v;
        } else if let Some(v) = field("http_status:") {
            status = v.parse().ok();
        } else if let Some(code) = field("code:") {
            if let Some(s) = status.take() {
                rows.push((id.clone(), s, code));
            }
        }
    }
    rows
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&'static str>,
) -> (StatusCode, Value) {
    let body = match body {
        Some(body) => body,
        None if method == "GET" || method == "DELETE" => "",
        None => "{}",
    };
    let mut req = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).unwrap_or(Method::GET))
        .uri(uri)
        .header("OData-MaxVersion", "4.01");
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap_or_default())
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

async fn broker(engine: Arc<Counting>) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?.with_policy(engine);
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": "https://example.org/city/"},
        "selector": ["city:Car", "city:Person"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:tags", "city:owner"]},
                  {"types": ["city:Person"], "attributes": ["city:name"]}],
        "operations": ["query_entities", "retrieve_entity", "merge_entity", "replace_entity"]
    })
    .to_string();
    for (uri, body) in [
        ("/q/views/cars", view.as_str()),
        ("/q/schemas/city", SCHEMA),
    ] {
        let req = Request::put(uri)
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body.to_owned()))
            .map_err(|e| e.to_string())?;
        let Ok(resp) = app.clone().oneshot(req).await;
        if resp.status() != StatusCode::CREATED {
            return Err(format!("{uri}: {}", resp.status()));
        }
    }
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_refused_features_answer_501_before_any_gate() {
    let engine = Arc::new(Counting::default());
    let (_b, app) = broker(engine.clone()).await.expect("broker");
    let (served, body) = send(&app, "GET", &format!("{ROOT}/Car"), &[], None).await;
    assert_eq!(served, StatusCode::OK, "{body}");
    assert_eq!(
        body["value"],
        json!([]),
        "a Tenant with nothing yet is an empty set"
    );
    let car = r#"{"id":"urn:ngsi-ld:Car:1","type":"https://example.org/city/Car"}"#;
    let made = Request::post("/ngsi-ld/v1/entities")
        .header("NGSILD-Tenant", "city")
        .header("Content-Type", "application/json")
        .header("Content-Length", car.len())
        .body(Body::from(car))
        .unwrap_or_default();
    let Ok(made) = app.clone().oneshot(made).await;
    assert_eq!(made.status(), StatusCode::CREATED);
    let before = engine.0.load(Ordering::SeqCst);
    let rows = matrix_rows();
    assert!(rows.len() >= 25, "the matrix rows are read: {rows:?}");
    let refused = rows.iter().filter(|(id, ..)| !GATED.contains(&id.as_str()));
    let gated = rows.iter().filter(|(id, ..)| GATED.contains(&id.as_str()));
    let mut asked = Vec::new();
    for (pass, (id, want_status, want_code)) in refused.map(|r| (0, r)).chain(gated.map(|r| (1, r)))
    {
        if pass == 1 && asked.is_empty() {
            asked.push(engine.0.load(Ordering::SeqCst));
        }
        let requests: Vec<(&str, &str, Headers)> = ROWS
            .iter()
            .filter(|(r, ..)| r == id)
            .map(|(_, m, p, h)| (*m, *p, *h))
            .chain(
                MORE.iter()
                    .filter(|(r, ..)| r == id)
                    .map(|(_, m, p)| (*m, *p, &[][..])),
            )
            .collect();
        assert!(
            !requests.is_empty(),
            "{id}: the matrix names a refusal this test does not send"
        );
        for (method, path, headers) in requests {
            let body = BODIES.iter().find(|(r, _)| r == id).map(|(_, b)| *b);
            let (got, body) = send(&app, method, &format!("{ROOT}/{path}"), headers, body).await;
            assert_eq!(got.as_u16(), *want_status, "{id}: {method} {path}: {body}");
            if got == StatusCode::NOT_IMPLEMENTED {
                let construct = NAMED.iter().find(|(r, _)| r == id).map(|(_, c)| named(c));
                assert_eq!(
                    body["error"]["message"].as_str().map(str::to_owned),
                    construct,
                    "{id}: {method} {path}: {body}"
                );
            }
            assert_eq!(
                body["error"]["code"],
                want_code.as_str(),
                "{id}: {method} {path}: {body}"
            );
        }
    }
    assert_eq!(
        asked.first().copied(),
        Some(before),
        "no refused request reached a gate"
    );
    assert!(
        engine.0.load(Ordering::SeqCst) > before,
        "an upsert is its operation, gated"
    );
    for (id, ..) in ROWS {
        assert!(
            rows.iter().any(|(r, ..)| r == id),
            "{id} is no refusing row of the matrix"
        );
    }
    let (served, body) = send(&app, "GET", &format!("{ROOT}/Car"), &[], None).await;
    assert_eq!(served, StatusCode::OK, "{body}");
    assert_eq!(body["value"][0]["id"], "urn:ngsi-ld:Car:1", "{body}");
    assert!(
        engine.0.load(Ordering::SeqCst) > before,
        "a served read is gated: the count counts"
    );
}

/// Every `501` past the parser names its construct and echoes no request
/// value: `red` in a request never reaches the message (T-5907).
#[tokio::test(flavor = "multi_thread")]
async fn odata_each_501_names_its_construct() {
    let engine = Arc::new(Counting::default());
    let (_b, app) = broker(engine).await.expect("broker");
    for (method, path, body, construct) in AFTER_PARSE {
        let body = (!body.is_empty()).then_some(*body);
        let (got, answer) = send(&app, method, &format!("{ROOT}/{path}"), &[], body).await;
        assert_eq!(
            got,
            StatusCode::NOT_IMPLEMENTED,
            "{method} {path}: {answer}"
        );
        assert_eq!(answer["error"]["code"], "NotImplemented", "{path}");
        assert_eq!(
            answer["error"]["message"].as_str(),
            Some(named(construct).as_str()),
            "{method} {path}: {answer}"
        );
        assert!(!answer.to_string().contains("red"), "{path}: {answer}");
    }
    // $metadata says so: a navigation takes $select and no other option
    let req = Request::get(format!("{ROOT}/$metadata"))
        .body(Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let xml = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let xml = String::from_utf8_lossy(&xml);
    let restriction = xml
        .split(r#"Term="Capabilities.NavigationRestrictions""#)
        .nth(1)
        .and_then(|rest| rest.split("</Annotation>").next())
        .unwrap_or_default();
    assert!(
        restriction.contains(r#"NavigationPropertyPath="owner""#),
        "{xml}"
    );
    for member in ["Filterable", "Searchable", "Sortable"] {
        assert!(
            restriction.contains(&format!(
                r#"<PropertyValue Property="{member}" Bool="false"/>"#
            )),
            "{member}: {restriction}"
        );
    }
    assert!(!restriction.contains("CountRestrictions"), "{restriction}");
    for member in ["TopSupported", "SkipSupported", "IndexableByKey"] {
        assert!(
            restriction.contains(&format!(
                r#"<PropertyValue Property="{member}" Bool="false"/>"#
            )),
            "{member}: {restriction}"
        );
    }
}
