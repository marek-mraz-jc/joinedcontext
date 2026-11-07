// SPDX-License-Identifier: EUPL-1.2
//! `$search` (URL 5.1.7, odata-linkml-mapping Q14, T-3750): a word or phrase matches a Car when
//! one of the single-valued String properties the view shows holds it, case-insensitively, with
//! `NOT` before `AND` before `OR`. The view shows `label` and `owner` and hides `note`; a Truck is
//! outside the view. The answer pages, counts and combines with `$filter` as any filtered read,
//! and a property the engine hides from the caller never matches.

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
  "classes": {"Car": {"slots": ["label", "owner", "note", "speed", "tags"]}},
  "slots": {"label": {"range": "string"}, "owner": {"range": "string"}, "note": {"range": "string"},
            "speed": {"range": "float"}, "tags": {"range": "string", "multivalued": true}}
}"#;

/// `(n, label, owner, note, speed)` of a Car; `None` leaves the Attribute out.
type Car = (
    u32,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
    f64,
);

const CARS: [Car; 6] = [
    (1, Some("Blue Car"), Some("Ana"), None, 10.0),
    (2, Some("red car"), Some("Bob"), Some("blue"), 20.0),
    (3, Some("Dark Red"), Some("Blue Corp"), None, 30.0),
    (4, Some("green"), None, Some("blue"), 40.0),
    (5, None, None, None, 50.0),
    (6, Some("C++ fan"), Some("Ana"), None, 60.0),
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
                   "attributes": ["city:label", "city:owner", "city:speed", "city:tags"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    let mut writes = vec![
        ("/q/views/cars".to_owned(), view),
        (
            "/q/schemas/city".to_owned(),
            serde_json::from_str(SCHEMA).map_err(|e| e.to_string())?,
        ),
    ];
    for (n, label, owner, note, speed) in CARS {
        let mut car = json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"),
                             format!("{CITY}speed"): {"type": "Property", "value": speed},
                             format!("{CITY}tags"): {"type": "Property", "value": ["blue"]}});
        for (name, value) in [("label", label), ("owner", owner), ("note", note)] {
            if let (Some(v), Some(members)) = (value, car.as_object_mut()) {
                members.insert(
                    format!("{CITY}{name}"),
                    json!({"type": "Property", "value": v}),
                );
            }
        }
        writes.push(("/ngsi-ld/v1/entities".to_owned(), car));
    }
    // outside the view's selector
    writes.push((
        "/ngsi-ld/v1/entities".to_owned(),
        json!({"id": "urn:ngsi-ld:Truck:1", "type": format!("{CITY}Truck"),
               format!("{CITY}label"): {"type": "Property", "value": "blue truck"}}),
    ));
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

async fn search(app: &Router, query: &str) -> Result<Vec<u32>, String> {
    let uri = format!("{ROOT}/Car?{}", query.replace(' ', "%20"));
    let (status, body) = send(app, Method::GET, &uri, None).await;
    if status != StatusCode::OK {
        return Err(format!("{query}: {status} {body}"));
    }
    Ok(cars(&body))
}

#[tokio::test]
async fn search_matches_the_string_properties_the_view_shows() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    for (query, want) in [
        // case-insensitive, over label and owner; the hidden note and the Truck never match
        ("$search=blue", vec![1, 3]),
        ("$search=BLUE", vec![1, 3]),
        ("$search=car", vec![1, 2]),
        // side by side is AND; a phrase keeps its words together
        ("$search=red car", vec![2]),
        ("$search=red%20AND%20car", vec![2]),
        ("$search=%22dark red%22", vec![3]),
        ("$search=%22red car%22", vec![2]),
        ("$search=%22car red%22", vec![]),
        ("$search=blue OR green", vec![1, 3, 4]),
        ("$search=blue+OR+green", vec![1, 3, 4]),
        // NOT before AND before OR; NOT holds where no property holds the term
        ("$search=NOT blue", vec![2, 4, 5, 6]),
        ("$search=NOT blue NOT car", vec![4, 5, 6]),
        ("$search=red car OR ana", vec![1, 2, 6]),
        ("$search=NOT (red OR blue)", vec![4, 5, 6]),
        ("$search=C%2B%2B", vec![6]),
        ("$search='dark red'", vec![3]),
        ("$search=nothing", vec![]),
        // the ListProperty `tags` holds "blue" on every Car: only single values are searched
        ("$search=NOT blue&$top=10", vec![2, 4, 5, 6]),
    ] {
        assert_eq!(search(&app, query).await?, want, "{query}");
    }
    Ok(())
}

#[tokio::test]
async fn search_joins_filter_count_order_and_paging() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    assert_eq!(
        search(&app, "$search=car OR ana&$filter=speed%20gt%2015").await?,
        [2, 6]
    );
    let (status, body) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$search=NOT%20blue&$count=true&$top=1&$orderby=speed%20desc"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["@odata.count"], 4, "{body}");
    assert_eq!(cars(&body), [6], "{body}");
    let (status, count) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car/$count?$search=blue"),
        None,
    )
    .await;
    assert_eq!((status, count), (StatusCode::OK, json!(2)));
    // each page carries the search in its next link; the pages are the unpaged answer
    let mut uri = Some(format!("{ROOT}/Car?$search=NOT%20blue&$top=3&$skip=0"));
    let mut seen = Vec::new();
    let mut pages = 0;
    while let Some(u) = uri.take() {
        let req = Request::get(&u)
            .header("NGSILD-Tenant", "city")
            .header("Prefer", "odata.maxpagesize=1")
            .body(Body::empty())
            .map_err(|e| e.to_string())?;
        let Ok(resp) = app.clone().oneshot(req).await;
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map(http_body_util::Collected::to_bytes)
            .unwrap_or_default();
        let page: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        seen.extend(cars(&page));
        uri = page["@odata.nextLink"]
            .as_str()
            .map(|l| l[l.find("/x/views/").unwrap_or(0)..].to_owned());
        pages += 1;
        assert!(pages < 10, "the links never end");
    }
    assert_eq!(seen, [2, 4, 5], "three of NOT blue, a page each");
    // $query carries it as the GET does
    let (status, body) = send_text(&app, "$search=blue").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(cars(&body), [1, 3]);
    Ok(())
}

async fn send_text(app: &Router, text: &str) -> (StatusCode, Value) {
    let req = Request::post(format!("{ROOT}/Car/$query"))
        .header("NGSILD-Tenant", "city")
        .header("Content-Type", "text/plain")
        .header("Content-Length", text.len())
        .body(Body::from(text.to_owned()))
        .unwrap_or_default();
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

/// Security (T-3750): the engine hides `owner`, so a search never matches through it, and the
/// answer does not reveal which Car holds it.
#[tokio::test]
async fn a_property_the_engine_hides_never_matches() -> Result<(), String> {
    let (_b, app) = broker(true).await?;
    assert_eq!(search(&app, "$search=ana").await?, Vec::<u32>::new());
    assert_eq!(
        search(&app, "$search=blue").await?,
        [1],
        "Blue Corp is an owner"
    );
    assert_eq!(search(&app, "$search=NOT ana").await?, [1, 2, 3, 4, 5, 6]);
    let (_, count) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car/$count?$search=ana"),
        None,
    )
    .await;
    assert_eq!(count, json!(0));
    Ok(())
}

#[tokio::test]
async fn search_is_refused_where_it_does_not_apply() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    for (uri, want) in [
        // one Entity, or below it: a collection option
        (
            "Car('urn:ngsi-ld:Car:1')?$search=blue",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Car('urn:ngsi-ld:Car:1')/label?$search=blue",
            StatusCode::NOT_IMPLEMENTED,
        ),
        // no searchExpr
        ("Car?$search=", StatusCode::BAD_REQUEST),
        ("Car?$search=(blue", StatusCode::BAD_REQUEST),
        ("Car?$search=%22blue", StatusCode::BAD_REQUEST),
        ("Car?$search=a&$search=b", StatusCode::BAD_REQUEST),
    ] {
        let (status, body) = send(&app, Method::GET, &format!("{ROOT}/{uri}"), None).await;
        assert_eq!(status, want, "{uri}: {body}");
        assert!(body["error"]["code"].is_string(), "{uri}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn the_metadata_says_every_set_is_searchable() -> Result<(), String> {
    let (_b, app) = broker(false).await?;
    let (status, body) = send(&app, Method::GET, &format!("{ROOT}/$metadata"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = body.as_str().unwrap_or_default();
    let searchable = text
        .split("<Annotation Term=\"Capabilities.SearchRestrictions\">")
        .skip(1)
        .map(|rest| rest.split("</Annotation>").next().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(!searchable.is_empty(), "{text}");
    for record in searchable {
        assert!(
            record.contains("<PropertyValue Property=\"Searchable\" Bool=\"true\"/>"),
            "{record}"
        );
    }
    Ok(())
}
