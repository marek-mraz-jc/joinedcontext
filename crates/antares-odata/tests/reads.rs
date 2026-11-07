// SPDX-License-Identifier: EUPL-1.2
//! Reads through a view (odata-linkml-mapping.md 5.1 rows Q13, Q29, Q30 and
//! S3): an EntitySet, one Entity by key, `/$count`, `$select`, `$top`,
//! `$skip`, `$count`, `$filter` (5.2), each the narrowed answer of one
//! operation, and the
//! forms of later rounds refused before any operation runs.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "secret", "owner", "seen", "tags", "label"]}, "Person": {"slots": ["name"]}},
  "slots": {"speed": {"range": "float"}, "secret": {"range": "integer"},
            "owner": {"range": "Person"}, "seen": {"range": "datetime"},
            "tags": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}},
            "name": {}, "label": {"range": "string"}}
}"#;

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap_or_default(),
        )
        .await;
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    Answer {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn car(n: u32, scope: &str, speed: f64) -> String {
    json!({
        "id": format!("urn:ngsi-ld:Car:{n}"),
        "type": format!("{CITY}Car"),
        "scope": scope,
        format!("{CITY}speed"): {"type": "Property", "value": speed},
        format!("{CITY}secret"): {"type": "Property", "value": 7},
        format!("{CITY}owner"): {"type": "Relationship", "object": "urn:ngsi-ld:Person:1"},
        format!("{CITY}seen"): {"type": "Property", "value": {"@type": "DateTime", "@value": "2026-10-11T08:00:00Z"}},
        format!("{CITY}tags"): {"type": "ListProperty", "valueList": ["a", "b"]},
        format!("{CITY}label"): {"type": "Property", "value": format!("car.{n}")},
    })
    .to_string()
}

async fn broker() -> Result<(TestBroker, Router), String> {
    bounded(antares_ops::bounds::MAX_ANSWER_BYTES).await
}

/// [`broker`] buffering at most `bytes` of one in-process answer.
async fn bounded(bytes: usize) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::with_max_answer_bytes(bytes).await?;
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city",
        "@context": {"city": CITY},
        "selector": ["city:Car"],
        "scopeQ": "/a",
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:owner", "city:seen", "city:tags", "city:label"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    let put = send(
        &app,
        Method::PUT,
        "/q/views/cars",
        Some(&view.to_string()),
        &[],
    )
    .await;
    if put.status != StatusCode::CREATED {
        return Err(format!("view: {} {}", put.status, put.body));
    }
    let put = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA), &[]).await;
    if put.status != StatusCode::CREATED {
        return Err(format!("schema: {} {}", put.status, put.body));
    }
    for (n, scope, speed) in [
        (1, "/a", 12.5),
        (2, "/a", 30.0),
        (3, "/a", 5.0),
        (4, "/b", 99.0),
    ] {
        let made = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some(&car(n, scope, speed)),
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        if made.status != StatusCode::CREATED {
            return Err(format!("car {n}: {} {}", made.status, made.body));
        }
    }
    Ok((b, app))
}

const ROOT: &str = "/x/views/cars/odata/v4";
/// The base a request without `Host` is answered under (no
/// `ANTARES_PUBLIC_URL` in tests): context URLs are absolute.
const PUBLIC: &str = "http://localhost:9090";

#[tokio::test(flavor = "multi_thread")]
async fn an_entity_set_is_the_narrowed_query_mapped_through_the_model() {
    let (_b, app) = broker().await.expect("broker");
    let got = send(&app, Method::GET, &format!("{ROOT}/Car"), None, &[]).await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    assert_eq!(
        got.headers["content-type"],
        "application/json;odata.metadata=minimal"
    );
    let body = got.json();
    assert_eq!(
        body["@odata.context"],
        format!("{PUBLIC}{ROOT}/$metadata#Car"),
        "{body}"
    );
    let cars = body["value"].as_array().cloned().unwrap_or_default();
    assert_eq!(cars.len(), 3, "the view's scope leaves Car:4 out: {body}");
    let one = cars
        .iter()
        .find(|c| c["id"] == "urn:ngsi-ld:Car:1")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(one["speed"], 12.5, "{one}");
    assert_eq!(one["owner"], "urn:ngsi-ld:Person:1", "R5: the object URI");
    assert_eq!(
        one["seen"], "2026-10-11T08:00:00Z",
        "a typed literal is its value"
    );
    assert_eq!(
        one["tags"],
        json!(["a", "b"]),
        "a ListProperty is a collection"
    );
    assert_eq!(one["_types"], json!(["Car"]));
    assert_eq!(one["scope"], json!(["/a"]));
    // the entities, not the control information: the metadata ETag is a hash
    let entities = body["value"].to_string();
    assert!(
        !entities.contains("secret") && !entities.contains('7'),
        "{entities}"
    );
}

/// T-3773: an NGSI-LD answer past the broker's buffer bound is the OData
/// 413, never a 200 holding the whole body or an empty set.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_past_the_buffer_bound_is_413() {
    let (_b, app) = bounded(512).await.expect("broker");
    let got = send(&app, Method::GET, &format!("{ROOT}/Car"), None, &[]).await;
    assert_eq!(got.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", got.body);
    assert_eq!(
        got.json()["error"]["code"],
        "PayloadTooLarge",
        "{}",
        got.body
    );
    assert!(!got.body.contains("urn:ngsi-ld:Car"), "{}", got.body);
    let one = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$top=1&$select=speed"),
        None,
        &[],
    )
    .await;
    assert_eq!(
        one.status,
        StatusCode::OK,
        "a page under the bound still reads: {}",
        one.body
    );
}

/// A path-only context URL is read as a file path by Microsoft.OData.Core
/// on Linux (T-3688): the documents name the service by absolute URL.
#[tokio::test(flavor = "multi_thread")]
async fn the_context_and_next_links_are_absolute_under_the_requests_host() {
    let (_b, app) = broker().await.expect("broker");
    let headers = [
        ("Host", "broker.example:8443"),
        ("Prefer", "odata.maxpagesize=1"),
    ];
    let got = send(&app, Method::GET, &format!("{ROOT}/Car"), None, &headers).await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    let body = got.json();
    let root = format!("http://broker.example:8443{ROOT}");
    assert_eq!(
        body["@odata.context"],
        format!("{root}/$metadata#Car"),
        "{body}"
    );
    let next = body["@odata.nextLink"].as_str().unwrap_or_default();
    assert!(next.starts_with(&format!("{root}/Car?")), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_count_is_the_narrowed_count() {
    let (_b, app) = broker().await.expect("broker");
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$count=true&$top=0"),
        None,
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    assert_eq!(got.json()["@odata.count"], 3, "{}", got.body);
    assert_eq!(got.json()["value"], json!([]));
    let got = send(&app, Method::GET, &format!("{ROOT}/Car/$count"), None, &[]).await;
    assert_eq!((got.status, got.body.as_str()), (StatusCode::OK, "3"));
    assert_eq!(got.headers["content-type"], "text/plain");
}

#[tokio::test(flavor = "multi_thread")]
async fn top_and_skip_page_and_the_next_link_carries_the_rest() {
    let (_b, app) = broker().await.expect("broker");
    let one = [("Prefer", "odata.maxpagesize=1")];
    let first = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$top=2&$filter=speed%20gt%201"),
        None,
        &one,
    )
    .await;
    let body = first.json();
    assert_eq!(body["value"].as_array().map(Vec::len), Some(1), "{body}");
    let next = body["@odata.nextLink"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        next.starts_with(&format!(
            "{PUBLIC}{ROOT}/Car?$filter=speed%20gt%201&$skiptoken=2.1.0.1."
        )),
        "{next}"
    );
    assert!(
        !next.contains("$top") && !next.contains("$skip="),
        "the token carries both: {next}"
    );
    let second = send(&app, Method::GET, &next, None, &one).await;
    let body = second.json();
    assert_eq!(body["value"].as_array().map(Vec::len), Some(1), "{body}");
    assert!(
        body.get("@odata.nextLink").is_none(),
        "$top is used up: {body}"
    );
    assert_ne!(
        body["value"][0]["id"],
        first.json()["value"][0]["id"],
        "the next page"
    );

    let got = send(&app, Method::GET, &format!("{ROOT}/Car?$skip=2"), None, &[]).await;
    let body = got.json();
    assert_eq!(body["value"].as_array().map(Vec::len), Some(1), "{body}");
    assert!(
        body.get("@odata.nextLink").is_none(),
        "the last page: {body}"
    );

    let mut ids = Vec::new();
    let mut link = Some(format!("{ROOT}/Car"));
    while let Some(uri) = link.take() {
        let page = send(
            &app,
            Method::GET,
            &uri,
            None,
            &[("Prefer", "odata.maxpagesize=2")],
        )
        .await;
        let body = page.json();
        assert_eq!(page.status, StatusCode::OK, "{uri}: {body}");
        ids.extend(
            body["value"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| e["id"].clone()),
        );
        link = body["@odata.nextLink"].as_str().map(str::to_owned);
    }
    assert_eq!(ids.len(), 3, "every Entity once: {ids:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_entity_by_key_and_hidden_answers_like_absent() {
    let (_b, app) = broker().await.expect("broker");
    for uri in [
        format!("{ROOT}/Car('urn:ngsi-ld:Car:2')"),
        format!("{ROOT}/Car(id='urn:ngsi-ld:Car:2')"),
        format!("{ROOT}/Car(@k)?@k='urn:ngsi-ld:Car:2'"),
    ] {
        let got = send(
            &app,
            Method::GET,
            &uri,
            None,
            &[("OData-MaxVersion", "4.01")],
        )
        .await;
        assert_eq!(got.status, StatusCode::OK, "{uri}: {}", got.body);
        let body = got.json();
        assert_eq!(
            body["@context"],
            format!("{PUBLIC}{ROOT}/$metadata#Car/$entity"),
            "{body}"
        );
        assert_eq!(body["speed"], 30.0, "{body}");
    }
    let absent = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car('urn:ngsi-ld:Car:9')"),
        None,
        &[],
    )
    .await;
    let hidden = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car('urn:ngsi-ld:Car:4')"),
        None,
        &[],
    )
    .await;
    assert_eq!(absent.status, StatusCode::NOT_FOUND, "{}", absent.body);
    assert_eq!(
        (hidden.status, hidden.body),
        (absent.status, absent.body),
        "out of the view = absent"
    );
    let other_key = send(&app, Method::GET, &format!("{ROOT}/Car(id=1)"), None, &[]).await;
    assert_eq!(
        other_key.status,
        StatusCode::NOT_FOUND,
        "{}",
        other_key.body
    );
    let not_a_uri = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car('not%20a%20uri')"),
        None,
        &[],
    )
    .await;
    assert_eq!(
        not_a_uri.status,
        StatusCode::BAD_REQUEST,
        "{}",
        not_a_uri.body
    );
    assert_eq!(
        not_a_uri.json()["error"]["message"],
        "the request was refused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn select_projects_and_asks_only_for_what_it_names() {
    let (_b, app) = broker().await.expect("broker");
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car('urn:ngsi-ld:Car:1')?$select=speed,createdAt"),
        None,
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    let body = got.json();
    assert_eq!(
        body["@odata.context"],
        format!("{PUBLIC}{ROOT}/$metadata#Car(speed,createdAt)/$entity"),
        "{body}"
    );
    let mut names: Vec<&str> = body
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k.as_str())
        .filter(|k| !k.starts_with('@'))
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["createdAt", "id", "speed"], "{body}");
    assert_eq!(body["speed"], 12.5, "{body}");
    assert!(body["createdAt"].is_string(), "{body}");
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$select=*"),
        None,
        &[],
    )
    .await;
    assert!(got.json()["value"][0].get("tags").is_some(), "{}", got.body);
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$select=scope,_types"),
        None,
        &[],
    )
    .await;
    assert_eq!(
        got.json()["value"][0]["scope"],
        json!(["/a"]),
        "{}",
        got.body
    );
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$select=owner,tags"),
        None,
        &[],
    )
    .await;
    let first = &got.json()["value"][0];
    assert_eq!(first["owner"], "urn:ngsi-ld:Person:1", "{}", got.body);
    assert_eq!(first["tags"], json!(["a", "b"]), "{}", got.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_or_unknown_name_is_the_same_400() {
    let (_b, app) = broker().await.expect("broker");
    let hidden = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$select=secret"),
        None,
        &[],
    )
    .await;
    let unknown = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$select=nosuch"),
        None,
        &[],
    )
    .await;
    assert_eq!(hidden.status, StatusCode::BAD_REQUEST, "{}", hidden.body);
    assert_eq!(
        hidden.json()["error"]["code"],
        unknown.json()["error"]["code"]
    );
    assert!(!hidden.body.contains("secret"), "{}", hidden.body);
    let set = send(&app, Method::GET, &format!("{ROOT}/Person"), None, &[]).await;
    assert_eq!(
        set.status,
        StatusCode::NOT_FOUND,
        "a hidden type has no set: {}",
        set.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_forms_of_later_rounds_are_501_and_read_only_writes_405() {
    let (_b, app) = broker().await.expect("broker");
    // `not` is evaluated by the residual (T-3758)
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=not%20(speed%20gt%201)"),
        None,
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    assert_eq!(got.json()["value"], json!([]), "{}", got.body);
    for uri in [
        "Car('urn:ngsi-ld:Car:1')?$filter=speed%20gt%201",
        "Car?$schemaversion=2",
        "$batch",
    ] {
        let got = send(&app, Method::GET, &format!("{ROOT}/{uri}"), None, &[]).await;
        assert_eq!(
            got.status,
            StatusCode::NOT_IMPLEMENTED,
            "{uri}: {}",
            got.body
        );
        assert_eq!(got.json()["error"]["code"], "NotImplemented", "{uri}");
    }
    // T-5207: `*` and the version `Core.SchemaVersion` states are served; another is not.
    for current in ["*", "0.m1"] {
        let got = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$schemaversion={current}"),
            None,
            &[],
        )
        .await;
        assert_eq!(got.status, StatusCode::OK, "{current}: {}", got.body);
    }
    let got = send(&app, Method::POST, &format!("{ROOT}/Car"), Some("{}"), &[]).await;
    assert_eq!(
        got.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "a write the view does not offer, before its body is read: {}",
        got.body
    );
    let got = send(
        &app,
        Method::POST,
        &format!("{ROOT}/Car/$count"),
        Some("{}"),
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::METHOD_NOT_ALLOWED, "{}", got.body);
    assert_eq!(got.headers["allow"], "GET");
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$format=xml"),
        None,
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::NOT_ACCEPTABLE, "{}", got.body);
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car('urn:ngsi-ld:Car:1')/speed"),
        None,
        &[],
    )
    .await;
    assert_eq!(
        (got.status, &got.json()["error"]["code"]),
        (StatusCode::METHOD_NOT_ALLOWED, &json!("MethodNotAllowed")),
        "a property read the view's operations do not offer: {}",
        got.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_without_the_read_operations_is_refused_by_its_allowlist() {
    let (_b, app) = broker().await.expect("broker");
    let view = json!({
        "id": "counts", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed"]}],
        "operations": ["retrieve_entity_types"]
    });
    let put = send(
        &app,
        Method::PUT,
        "/q/views/counts",
        Some(&view.to_string()),
        &[],
    )
    .await;
    assert_eq!(put.status, StatusCode::CREATED, "{}", put.body);
    let got = send(&app, Method::GET, "/x/views/counts/odata/v4/Car", None, &[]).await;
    assert!(got.status.is_client_error(), "{}: {}", got.status, got.body);
    assert_eq!(
        got.json()["error"]["message"],
        "the request was refused",
        "no broker detail"
    );
}

fn ids(answer: &Answer) -> Vec<String> {
    let mut ids: Vec<String> = answer
        .json()
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    ids.sort();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filter_selects_through_q_and_the_count_follows() {
    let (_b, app) = broker().await.expect("broker");
    let car = |n: u32| format!("urn:ngsi-ld:Car:{n}");
    for (filter, want) in [
        ("speed%20gt%2010", vec![car(1), car(2)]),
        ("10%20lt%20speed%20and%20speed%20lt%2020", vec![car(1)]),
        ("speed%20ne%2030", vec![car(1), car(3)]),
        ("speed%20eq%20null", vec![]),
        ("speed%20in%20(5,30)", vec![car(2), car(3)]),
        (
            "seen%20eq%202026-10-11T08:00:00Z%20or%20speed%20eq%201",
            vec![car(1), car(2), car(3)],
        ),
        ("id%20eq%20'urn:ngsi-ld:Car:2'", vec![car(2)]),
        ("startswith(label,'car.1')", vec![car(1)]),
        (
            "endswith(label,'.3')%20or%20contains(label,'2')",
            vec![car(2), car(3)],
        ),
        ("not%20contains(label,'.1')", vec![car(2), car(3)]),
        ("contains(label,'.')", vec![car(1), car(2), car(3)]),
        (
            "contains(label,'r.')%20and%20not%20(label%20eq%20null)",
            vec![car(1), car(2), car(3)],
        ),
        (
            "id%20in%20('urn:ngsi-ld:Car:3','urn:ngsi-ld:Car:4')",
            vec![car(3)],
        ),
    ] {
        let got = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$filter={filter}&$count=true"),
            None,
            &[],
        )
        .await;
        assert_eq!(got.status, StatusCode::OK, "{filter}: {}", got.body);
        assert_eq!(ids(&got), want, "{filter}: {}", got.body);
        assert_eq!(got.json()["@odata.count"], want.len(), "{filter}");
        let count = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car/$count?$filter={filter}"),
            None,
            &[],
        )
        .await;
        assert_eq!(count.body, want.len().to_string(), "{filter}");
    }
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=speed%20gt%2020"),
        None,
        &[],
    )
    .await;
    assert_eq!(
        got.json()["value"][0]["speed"],
        30.0,
        "the answer read under the filter's terms: {}",
        got.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quote_or_backslash_in_a_string_literal_is_matched_not_cut() {
    let (_b, app) = broker().await.expect("broker");
    let mut odd: Value = serde_json::from_str(&car(5, "/a", 1.0)).unwrap_or(Value::Null);
    odd[format!("{CITY}label")]["value"] = json!(r#"say "hi" \ ok"#);
    let made = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some(&odd.to_string()),
        &[("NGSILD-Tenant", "city")],
    )
    .await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let five = vec!["urn:ngsi-ld:Car:5".to_owned()];
    for (filter, want) in [
        ("label%20eq%20'say%20%22hi%22%20%5C%20ok'", five.clone()),
        ("label%20eq%20'say%20%22hi'", vec![]),
        ("contains(label,'%22hi%22')", five.clone()),
        ("startswith(label,'say%20%22')", five.clone()),
        ("endswith(label,'%5C%20ok')", five.clone()),
        ("contains(label,'%22%22')", vec![]),
    ] {
        let got = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$filter={filter}"),
            None,
            &[],
        )
        .await;
        assert_eq!(got.status, StatusCode::OK, "{filter}: {}", got.body);
        assert_eq!(ids(&got), want, "{filter}: {}", got.body);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_clients_link_header_does_not_change_what_a_filter_means() {
    let (_b, app) = broker().await.expect("broker");
    let uri = format!("{ROOT}/Car?$filter=speed%20gt%2010");
    let plain = send(&app, Method::GET, &uri, None, &[]).await;
    let link = r#"<https://example.invalid/c.jsonld>; rel="http://www.w3.org/ns/json-ld#context"; type="application/ld+json""#;
    let linked = send(&app, Method::GET, &uri, None, &[("Link", link)]).await;
    assert_eq!(
        (plain.status, ids(&plain)),
        (linked.status, ids(&linked)),
        "{}",
        linked.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_filter_on_hidden_attribute_equals_filter_on_unknown_name() {
    let (_b, app) = broker().await.expect("broker");
    let hidden = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=secret%20eq%207"),
        None,
        &[],
    )
    .await;
    let unknown = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=nosuch%20eq%207"),
        None,
        &[],
    )
    .await;
    assert_eq!(hidden.status, StatusCode::BAD_REQUEST, "{}", hidden.body);
    assert_eq!(
        (hidden.status, &hidden.body),
        (unknown.status, &unknown.body)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_bisecting_a_hidden_value_learns_nothing() {
    let (_b, app) = broker().await.expect("broker");
    let mut answers = Vec::new();
    for bound in ["0", "6", "7", "8", "100"] {
        for op in ["gt", "lt", "eq"] {
            let got = send(
                &app,
                Method::GET,
                &format!("{ROOT}/Car?$filter=secret%20{op}%20{bound}"),
                None,
                &[],
            )
            .await;
            answers.push((got.status, got.body));
        }
    }
    answers.dedup();
    assert_eq!(answers.len(), 1, "every bound answers alike: {answers:?}");
    assert!(
        !answers[0].1.contains('7'),
        "no literal echoed: {}",
        answers[0].1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filter_past_the_bounds_is_400_before_any_operation() {
    let (_b, app) = broker().await.expect("broker");
    let deep = format!("{}speed%20gt%201{}", "(".repeat(40), ")".repeat(40));
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter={deep}"),
        None,
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::BAD_REQUEST, "{}", got.body);
    let long = "x".repeat(1000);
    let ors: Vec<String> = (0..5)
        .map(|_| format!("contains(label,'{long}')"))
        .collect();
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter={}", ors.join("%20or%20")),
        None,
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::BAD_REQUEST, "{}", got.body);
    assert_eq!(
        got.json()["error"]["code"],
        "TooComplexQuery",
        "{}",
        got.body
    );
    let inexact = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=speed%20eq%209007199254740993"),
        None,
        &[],
    )
    .await;
    assert_eq!(
        inexact.status,
        StatusCode::BAD_REQUEST,
        "F26: {}",
        inexact.body
    );
    assert_eq!(
        inexact.json()["error"]["code"],
        "BadRequest",
        "{}",
        inexact.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_filter_ne_doubling_is_checked_on_the_translated_tree() {
    let (_b, app) = broker().await.expect("broker");
    // 30 `ne` terms, as deep as max_expr_depth lets an `and` chain go;
    // each is `!t|t!=v` in q, which the broker bounds again.
    let terms: Vec<String> = (0..30).map(|n| format!("speed%20ne%20{n}")).collect();
    let got = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter={}", terms.join("%20and%20")),
        None,
        &[],
    )
    .await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    assert_eq!(
        ids(&got),
        vec!["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"],
        "5 is one of 0..30"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn orderby_sorts_through_order_by_and_pages_keep_the_order() {
    let (_b, app) = broker().await.expect("broker");
    let order = |answer: &Answer| -> Vec<String> {
        answer
            .json()
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect()
    };
    let car = |n: u32| format!("urn:ngsi-ld:Car:{n}");
    for (orderby, want) in [
        ("speed%20desc", vec![car(2), car(1), car(3)]),
        ("speed", vec![car(3), car(1), car(2)]),
        ("label%20desc", vec![car(3), car(2), car(1)]),
        ("id%20desc", vec![car(3), car(2), car(1)]),
        ("seen,speed%20desc", vec![car(2), car(1), car(3)]),
    ] {
        let got = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$orderby={orderby}"),
            None,
            &[],
        )
        .await;
        assert_eq!(got.status, StatusCode::OK, "{orderby}: {}", got.body);
        assert_eq!(order(&got), want, "{orderby}");
    }
    let first = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$orderby=speed%20desc&$filter=speed%20gt%201"),
        None,
        &[("Prefer", "odata.maxpagesize=1")],
    )
    .await;
    assert_eq!(order(&first), vec![car(2)], "{}", first.body);
    let next = first.json()["@odata.nextLink"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(next.contains("$orderby=speed%20desc"), "{next}");
    let second = send(
        &app,
        Method::GET,
        &next,
        None,
        &[("Prefer", "odata.maxpagesize=1")],
    )
    .await;
    assert_eq!(order(&second), vec![car(1)], "{}", second.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_orderby_hidden_is_400_like_unknown() {
    let (_b, app) = broker().await.expect("broker");
    let hidden = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$orderby=secret"),
        None,
        &[],
    )
    .await;
    let unknown = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$orderby=nosuch"),
        None,
        &[],
    )
    .await;
    assert_eq!(hidden.status, StatusCode::BAD_REQUEST, "{}", hidden.body);
    assert_eq!(
        (hidden.status, &hidden.body),
        (unknown.status, &unknown.body)
    );
    let collection = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$orderby=tags"),
        None,
        &[],
    )
    .await;
    assert_eq!(
        collection.status,
        StatusCode::BAD_REQUEST,
        "C12: {}",
        collection.body
    );
    let later = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$orderby=owner"),
        None,
        &[],
    )
    .await;
    assert_eq!(later.status, StatusCode::NOT_IMPLEMENTED, "{}", later.body);
}
