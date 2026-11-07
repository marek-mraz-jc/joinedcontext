// SPDX-License-Identifier: EUPL-1.2
//! `$compute` (URL 5.1.10, odata-linkml-mapping Q16, T-3936): each item is evaluated per Car
//! over what the view shows, written as a dynamic property, and its name resolves in `$filter`,
//! `$orderby` and `$select`. The view shows `label`, `owner`, `speed`, `seats`, `since` and
//! `tags` and hides `note`; the narrowing engine hides `owner`, which a computed value then
//! reads as absent.

use antares_ops::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";
const ROOT: &str = "/x/views/cars/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["label", "owner", "note", "speed", "seats", "since", "tags"]}},
  "slots": {"label": {"range": "string"}, "owner": {"range": "string"}, "note": {"range": "string"},
            "speed": {"range": "float"}, "seats": {"range": "integer"},
            "since": {"range": "datetime"}, "tags": {"range": "string", "multivalued": true}}
}"#;

/// `(n, label, owner)` of a Car; its speed is `10 n`, its seats `n`, `since` the 15th of month
/// `n` of 2025 at `n`:30 in UTC+1. `None` leaves the Attribute out.
const CARS: [(u32, Option<&str>, Option<&str>); 4] = [
    (1, Some("Blue Car"), Some("Ana")),
    (2, Some("red car"), Some("Bob")),
    (3, None, Some("Cyd")),
    (4, Some("green"), None),
];

/// Hides `owner` from every query when it hides anything.
struct Hiding(bool);

impl PolicyEngine for Hiding {
    fn name(&self) -> &'static str {
        "hiding"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let decision = if self.0 && op.clause.starts_with("5.7.") {
            Decision::Filter(Filter {
                omit: vec![format!("{CITY}owner")],
                ..Filter::default()
            })
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(decision))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<String>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
    if let Some(b) = &body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default()))
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
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

/// The `tags` of a Car: `["blue"]`, Car 4's followed by 1100 more, a list a nested lambda
/// walks past the evaluator's step bound.
fn tags(n: u32) -> Value {
    let more = (0..1100).map(|i| format!("t{i}")).filter(|_| n == 4);
    Value::from(
        std::iter::once("blue".to_owned())
            .chain(more)
            .collect::<Vec<_>>(),
    )
}

async fn broker(hiding: bool) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new()
        .await?
        .with_policy(Arc::new(Hiding(hiding)));
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"],
                   "attributes": ["city:label", "city:owner", "city:speed", "city:seats",
                                  "city:since", "city:tags"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    let mut writes = vec![
        ("/q/views/cars".to_owned(), view),
        (
            "/q/schemas/city".to_owned(),
            serde_json::from_str(SCHEMA).map_err(|e| e.to_string())?,
        ),
    ];
    for (n, label, owner) in CARS {
        // 4.6.3: a DateTime ends in Z, so every timestamp is UTC; an offset is refused.
        let since = format!("2025-{n:02}-15T{n:02}:30:00Z");
        let mut car = json!({
            "id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"),
            format!("{CITY}speed"): {"type": "Property", "value": f64::from(n) * 10.0},
            format!("{CITY}seats"): {"type": "Property", "value": n},
            format!("{CITY}since"): {"type": "Property",
                                     "value": {"@type": "DateTime", "@value": since}},
            format!("{CITY}note"): {"type": "Property", "value": "secret"},
            format!("{CITY}tags"): {"type": "Property", "value": tags(n)}
        });
        for (name, value) in [("label", label), ("owner", owner)] {
            if let (Some(v), Some(members)) = (value, car.as_object_mut()) {
                members.insert(
                    format!("{CITY}{name}"),
                    json!({"type": "Property", "value": v}),
                );
            }
        }
        writes.push(("/ngsi-ld/v1/entities".to_owned(), car));
    }
    for (uri, doc) in writes {
        let method = if uri.starts_with("/q/") {
            Method::PUT
        } else {
            Method::POST
        };
        let (status, answer) = send(&app, method, &uri, Some(doc.to_string())).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    Ok((b, app))
}

/// The numbers of the Cars a read answered, in id order.
fn cars(body: &Value) -> Vec<u32> {
    let mut out: Vec<u32> = body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str()?.rsplit(':').next()?.parse().ok())
        .collect();
    out.sort_unstable();
    out
}

/// A read of `Car` with `query`, its body.
async fn read(app: &Router, query: &str) -> Result<Value, String> {
    let uri = format!("{ROOT}/Car?{}", query.replace(' ', "%20"));
    let (status, body) = send(app, Method::GET, &uri, None).await;
    if status != StatusCode::OK {
        return Err(format!("{query}: {status} {body}"));
    }
    Ok(body)
}

/// The values of `name` the read answered, Car by Car in its order.
async fn values(app: &Router, query: &str, name: &str) -> Result<Vec<Value>, String> {
    let body = read(app, query).await?;
    Ok(body
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|car| car.get(name).cloned().unwrap_or(json!("missing")))
        .collect())
}

#[tokio::test]
async fn each_construct_is_computed_per_car() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    let by_id = "&$orderby=id";
    for (compute, want) in [
        // arithmetic: a float by an integer, integer division, a number by zero is unknown
        ("speed mul 2 as v", json!([20.0, 40.0, 60.0, 80.0])),
        ("seats add 1 as v", json!([2, 3, 4, 5])),
        ("seats div 2 as v", json!([0, 1, 1, 2])),
        ("seats mod 3 as v", json!([1, 2, 0, 1])),
        ("-seats as v", json!([-1, -2, -3, -4])),
        ("speed divby 0 as v", json!([null, null, null, null])),
        // strings: an absent label is unknown, never ""
        (
            "concat(label,'!') as v",
            json!(["Blue Car!", "red car!", null, "green!"]),
        ),
        ("tolower(owner) as v", json!(["ana", "bob", "cyd", null])),
        ("length(label) as v", json!([8, 7, null, 5])),
        (
            "startswith(label,'red') as v",
            json!([false, true, null, false]),
        ),
        // dates as they were written, in UTC
        ("year(since) as v", json!([2025, 2025, 2025, 2025])),
        ("month(since) as v", json!([1, 2, 3, 4])),
        ("hour(since) as v", json!([1, 2, 3, 4])),
        (
            "date(since) as v",
            json!(["2025-01-15", "2025-02-15", "2025-03-15", "2025-04-15"]),
        ),
        (
            "time(since) as v",
            json!(["01:30:00", "02:30:00", "03:30:00", "04:30:00"]),
        ),
        // a comparison is a Boolean value; a literal a constant one
        ("seats gt 2 as v", json!([false, false, true, true])),
        ("'x' as v", json!(["x", "x", "x", "x"])),
    ] {
        let query = format!("$compute={compute}{by_id}");
        assert_eq!(json!(values(&app, &query, "v").await?), want, "{compute}");
    }
    Ok(())
}

#[tokio::test]
async fn a_computed_name_filters_orders_and_is_selected() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    let double = "$compute=speed mul 2 as double";
    // $filter wherever $compute stands, also inside a lambda and beside a pushed conjunct
    let body = read(&app, &format!("$filter=double gt 30&{double}")).await?;
    assert_eq!(cars(&body), [2, 3, 4], "{body}");
    let body = read(
        &app,
        &format!(
            "{double},'blue' as b&$filter=tags/any(t: t eq b) and seats lt 4 and double ge 40"
        ),
    )
    .await?;
    assert_eq!(cars(&body), [2, 3], "{body}");
    // $orderby, descending, with $top and $count over the whole answer
    let ordered = values(
        &app,
        &format!("{double}&$orderby=double desc&$top=3&$count=true"),
        "id",
    )
    .await?;
    assert_eq!(
        ordered,
        [
            "urn:ngsi-ld:Car:4",
            "urn:ngsi-ld:Car:3",
            "urn:ngsi-ld:Car:2"
        ]
        .map(Value::from)
    );
    // an unknown value sorts first ascending
    let ordered = values(&app, "$compute=length(label) as n&$orderby=n,id", "id").await?;
    assert_eq!(
        ordered,
        [
            "urn:ngsi-ld:Car:3",
            "urn:ngsi-ld:Car:4",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:1"
        ]
        .map(Value::from)
    );
    // $select: named, it is written and listed in the context; not named, it is not written;
    // without $select every computed value is
    let body = read(
        &app,
        &format!("{double}&$select=label,double&$filter=seats eq 1"),
    )
    .await?;
    assert_eq!(body["value"][0]["double"], json!(20.0), "{body}");
    assert!(body["value"][0].get("speed").is_none(), "{body}");
    let context = body["@odata.context"].as_str().unwrap_or_default();
    assert!(context.ends_with("#Car(label,double)"), "{context}");
    // a selected member the value reads is asked for once
    let body = read(
        &app,
        &format!("{double}&$select=speed,double&$filter=seats eq 1"),
    )
    .await?;
    assert_eq!(
        (&body["value"][0]["speed"], &body["value"][0]["double"]),
        (&json!(10.0), &json!(20.0)),
        "{body}"
    );
    let body = read(&app, &format!("{double}&$select=label&$filter=seats eq 1")).await?;
    assert!(body["value"][0].get("double").is_none(), "{body}");
    let body = read(&app, &format!("{double}&$select=*&$filter=seats eq 1")).await?;
    assert_eq!(body["value"][0]["double"], json!(20.0), "{body}");
    // /$count and one Entity
    let (status, count) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car/$count?{double}&$filter=double%20gt%2030").replace(' ', "%20"),
        None,
    )
    .await;
    assert_eq!((status, count), (StatusCode::OK, json!(3)));
    let (status, one) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car('urn:ngsi-ld:Car:2')?{double}&$select=double").replace(' ', "%20"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["double"], json!(40.0), "{one}");
    Ok(())
}

/// Security (T-3936): the engine hides `owner`; a value computed from it is the value of an
/// absent Attribute, and neither `$filter` nor `$orderby` on it tells the Cars apart.
#[tokio::test]
async fn a_computed_value_never_reads_what_the_engine_hides() -> Result<(), String> {
    let (_b, app) = broker(true).await?;
    let low = "$compute=tolower(owner) as low,concat(owner,label) as both&$orderby=id";
    assert_eq!(
        json!(values(&app, low, "low").await?),
        json!([null, null, null, null])
    );
    assert_eq!(
        json!(values(&app, low, "both").await?),
        json!([null, null, null, null])
    );
    let body = read(&app, "$compute=tolower(owner) as low&$filter=low eq 'ana'").await?;
    assert_eq!(cars(&body), Vec::<u32>::new(), "{body}");
    let body = read(&app, "$compute=owner as o&$filter=o eq null").await?;
    assert_eq!(cars(&body), [1, 2, 3, 4], "{body}");
    let ordered = values(&app, "$compute=owner as o&$orderby=o desc,id", "id").await?;
    assert_eq!(
        ordered,
        [
            "urn:ngsi-ld:Car:1",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:3",
            "urn:ngsi-ld:Car:4"
        ]
        .map(Value::from),
        "the hidden owners order nothing"
    );
    Ok(())
}

#[tokio::test]
async fn compute_is_refused_where_it_does_not_apply() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    for (uri, want) in [
        // below one Entity
        (
            "Car('urn:ngsi-ld:Car:1')/label?$compute=1 as x",
            StatusCode::NOT_IMPLEMENTED,
        ),
        // a list value and null are later rounds'
        ("Car?$compute=tags as x", StatusCode::NOT_IMPLEMENTED),
        ("Car?$compute=null as x", StatusCode::NOT_IMPLEMENTED),
        // no item, a name the type has, a hidden or unknown member, a mistyped use
        ("Car?$compute=speed", StatusCode::BAD_REQUEST),
        ("Car?$compute=speed as label", StatusCode::BAD_REQUEST),
        ("Car?$compute=nothing as x", StatusCode::BAD_REQUEST),
        // the view hides `note`: the 400 of an unknown name, as in $filter
        ("Car?$compute=note as x", StatusCode::BAD_REQUEST),
        ("Car?$compute=label add 1 as x", StatusCode::BAD_REQUEST),
        (
            "Car?$compute=label as l&$filter=l gt 1",
            StatusCode::BAD_REQUEST,
        ),
        (
            "Car?$compute=1 as x&$compute=2 as y",
            StatusCode::BAD_REQUEST,
        ),
        // a $select a later round maps, beside $compute, on the set and on one Entity
        (
            "Car?$compute=1 as x&$select=@Core.Description",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car('urn:ngsi-ld:Car:1')?$compute=1 as x&$select=@Core.Description",
            StatusCode::NOT_IMPLEMENTED,
        ),
        // past the evaluator's step bound on Car 4's long list: refused, never a guess
        (
            "Car?$compute=tags/any(a: tags/any(b: b eq 'zz')) as x",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let uri = uri.replace(' ', "%20");
        let (status, body) = send(&app, Method::GET, &format!("{ROOT}/{uri}"), None).await;
        assert_eq!(status, want, "{uri}: {body}");
        assert!(body["error"]["code"].is_string(), "{uri}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn the_metadata_says_compute_is_supported() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    let (status, body) = send(&app, Method::GET, &format!("{ROOT}/$metadata"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = body.as_str().unwrap_or_default();
    assert!(
        text.contains("<PropertyValue Property=\"ComputeSupported\" Bool=\"true\"/>")
            || text.contains("<Annotation Term=\"Capabilities.ComputeSupported\" Bool=\"true\"/>"),
        "{text}"
    );
    Ok(())
}
