// SPDX-License-Identifier: EUPL-1.2
//! `$filter` rows F21, F22, F29, F31 and F32 (odata-linkml-mapping.md 5.2)
//! through a view: each answers the Entities the NGSI-LD `q` or `geoQ` it
//! maps to selects, and a hidden Attribute stays the 400 an unknown name is
//! (V1, O1).

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";
const ROOT: &str = "/x/views/cars/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "tags", "label", "status", "location", "secret"]}},
  "slots": {"speed": {"range": "float"}, "secret": {"range": "float"},
            "tags": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}},
            "label": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "LanguageProperty"}}},
            "status": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "VocabProperty"}}},
            "location": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}}}
}"#;

async fn send(app: &Router, method: Method, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
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

fn car(n: u32, observed: &str, tags: &[&str], label: &Value, status: &str, at: [f64; 2]) -> Value {
    json!({
        "id": format!("urn:ngsi-ld:Car:{n}"),
        "type": format!("{CITY}Car"),
        format!("{CITY}speed"): {"type": "Property", "value": 10.0 * f64::from(n), "observedAt": observed},
        format!("{CITY}secret"): {"type": "Property", "value": 1.0, "observedAt": observed},
        format!("{CITY}tags"): {"type": "ListProperty", "valueList": tags},
        format!("{CITY}label"): {"type": "LanguageProperty", "languageMap": label},
        format!("{CITY}status"): {"type": "VocabProperty", "vocab": format!("{CITY}{status}")},
        format!("{CITY}location"): {"type": "GeoProperty", "value": {"type": "Point", "coordinates": at}},
    })
}

async fn broker() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city",
        "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"],
                   "attributes": ["city:speed", "city:tags", "city:label", "city:status", "city:location"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    let (status, body) = send(&app, Method::PUT, "/q/views/cars", Some(&view.to_string())).await;
    if status != StatusCode::CREATED {
        return Err(format!("view: {status} {body}"));
    }
    let (status, body) = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA)).await;
    if status != StatusCode::CREATED {
        return Err(format!("schema: {status} {body}"));
    }
    for entity in [
        car(
            1,
            "2026-05-01T08:00:00Z",
            &["a", "b"],
            &json!({"en": "red", "cs": "červená"}),
            "Active",
            [0.5, 0.5],
        ),
        car(
            2,
            "2026-05-02T08:00:00Z",
            &["c"],
            &json!({"en": "blue"}),
            "Parked",
            [5.0, 5.0],
        ),
    ] {
        let (status, body) = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some(&entity.to_string()),
        )
        .await;
        if status != StatusCode::CREATED {
            return Err(format!(
                "{}: {status} {body}",
                entity.get("id").unwrap_or(&Value::Null)
            ));
        }
    }
    Ok((b, app))
}

fn ids(answer: &Value) -> Vec<&str> {
    let mut ids: Vec<&str> = answer
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .collect();
    ids.sort_unstable();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mapped_rows_select_what_their_ngsi_ld_query_selects() {
    let (_b, app) = broker().await.expect("broker");
    let polygon = "geography'SRID=4326;Polygon((0%200,1%200,1%201,0%201,0%200))'";
    let vocab = |name: &str| {
        let iri = format!("{CITY}{name}")
            .replace(':', "%3A")
            .replace('/', "%2F");
        format!("status%20eq%20'{iri}'")
    };
    for (row, filter, want) in [
        (
            "F21",
            "speed/@NgsiLd.observedAt%20ge%202026-05-01T12:00:00Z".to_owned(),
            vec!["urn:ngsi-ld:Car:2"],
        ),
        (
            "F21",
            "speed/@NgsiLd.observedAt%20lt%202026-05-01T12:00:00Z".to_owned(),
            vec!["urn:ngsi-ld:Car:1"],
        ),
        (
            "F22",
            "tags/any(x:x%20eq%20'a')".to_owned(),
            vec!["urn:ngsi-ld:Car:1"],
        ),
        (
            "F22",
            "tags/any(x:x%20in%20('c','z'))".to_owned(),
            vec!["urn:ngsi-ld:Car:2"],
        ),
        ("F22", "tags/any(x:x%20eq%20'z')".to_owned(), vec![]),
        (
            "F29",
            format!("geo.intersects(location,{polygon})"),
            vec!["urn:ngsi-ld:Car:1"],
        ),
        (
            "F29",
            format!("geo.intersects(location,{polygon})%20and%20speed%20gt%2015"),
            vec![],
        ),
        // T-5206 finding 6: in the language the read serves, `ngsild.lang` or `@none`
        (
            "F31",
            "label%20eq%20'%C4%8Derven%C3%A1'&ngsild.lang=cs".to_owned(),
            vec!["urn:ngsi-ld:Car:1"],
        ),
        (
            "F31",
            "label%20eq%20'%C4%8Derven%C3%A1'&ngsild.lang=en".to_owned(),
            vec![],
        ),
        (
            "F31",
            "label%20eq%20'blue'&ngsild.lang=en".to_owned(),
            vec!["urn:ngsi-ld:Car:2"],
        ),
        ("F31", "label%20eq%20'blue'".to_owned(), vec![]),
        ("F32", vocab("Active"), vec!["urn:ngsi-ld:Car:1"]),
        ("F32", vocab("Parked"), vec!["urn:ngsi-ld:Car:2"]),
    ] {
        let (status, answer) = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$filter={filter}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{row} {filter}: {answer}");
        assert_eq!(ids(&answer), want, "{row} {filter}: {answer}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vocab_value_matches_as_the_read_shows_it() {
    let (_b, app) = broker().await.expect("broker");
    let (_, one) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car('urn:ngsi-ld:Car:2')"),
        None,
    )
    .await;
    // F31 is in `a_language_property_reads_and_filters_in_one_language`
    for name in ["status"] {
        let shown = one.get(name).and_then(Value::as_str).unwrap_or_default();
        assert!(!shown.is_empty(), "{name}: {one}");
        let literal: String = shown
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect();
        let (status, answer) = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$filter={name}%20eq%20'{literal}'"),
            None,
        )
        .await;
        assert_eq!(
            (status, ids(&answer)),
            (StatusCode::OK, vec!["urn:ngsi-ld:Car:2"]),
            "{name} shown as {shown}: {answer}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_attribute_in_an_evaluated_filter_is_the_400_of_an_unknown_name() {
    let (_b, app) = broker().await.expect("broker");
    for form in [
        "length({})%20eq%201",
        "{}%20add%201%20gt%200",
        "{}%20eq%20speed",
        "{}/any(x:x%20eq%201)",
    ] {
        let (hidden_status, hidden) = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$filter={}", form.replace("{}", "secret")),
            None,
        )
        .await;
        let (unknown_status, unknown) = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$filter={}", form.replace("{}", "nothing")),
            None,
        )
        .await;
        assert_eq!(hidden_status, StatusCode::BAD_REQUEST, "{form}: {hidden}");
        assert_eq!(
            (hidden_status, &hidden),
            (unknown_status, &unknown),
            "{form}: V1, O1"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_attributes_observed_at_is_the_400_of_an_unknown_name() {
    let (_b, app) = broker().await.expect("broker");
    let at = |name: &str| {
        format!("{ROOT}/Car?$filter={name}/@NgsiLd.observedAt%20ge%202026-05-01T12:00:00Z")
    };
    let (hidden_status, hidden) = send(&app, Method::GET, &at("secret"), None).await;
    let (unknown_status, unknown) = send(&app, Method::GET, &at("nothing"), None).await;
    assert_eq!(hidden_status, StatusCode::BAD_REQUEST, "{hidden}");
    assert_eq!(
        (hidden_status, &hidden),
        (unknown_status, &unknown),
        "V1, O1"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_forms_these_rows_do_not_cover_stay_501() {
    let (_b, app) = broker().await.expect("broker");
    let polygon = "geography'SRID=4326;Polygon((0%200,1%200,1%201,0%201,0%200))'";
    // a geo test under `or` is evaluated by the residual (T-3758)
    let (status, answer) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=geo.intersects(location,{polygon})%20or%20speed%20gt%2015"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let ids: Vec<&str> = answer["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(ids, ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"], "{answer}");
    // F23: any lambda over a list is evaluated (T-3749)
    for (filter, want) in [
        (
            "tags/any(x:x%20ne%20'a')",
            &["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"][..],
        ),
        ("tags/all(x:x%20eq%20'c')", &["urn:ngsi-ld:Car:2"][..]),
    ] {
        let (status, answer) = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?$filter={filter}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{filter}: {answer}");
        let ids: Vec<&str> = answer["value"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str())
            .collect();
        assert_eq!(ids, want, "{filter}: {answer}");
    }
    let (status, answer) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=label%20ne%20'blue'"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{answer}");
}

/// T-3758: a caller's `ngsild.georel` takes the one `geoQ`, so the OData geo test is evaluated;
/// a polygon outside WGS84 is the caller's error.
#[tokio::test(flavor = "multi_thread")]
async fn a_callers_geoq_moves_the_geo_filter_to_the_residual() {
    let (_b, app) = broker().await.expect("broker");
    let polygon = "geography'SRID=4326;Polygon((0%200,1%200,1%201,0%201,0%200))'";
    let near = "ngsild.georel=near%3BmaxDistance==2000000&ngsild.geometry=Point&ngsild.coordinates=%5B5,5%5D&ngsild.geoproperty=city:location";
    let (status, answer) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=geo.intersects(location,{polygon})&{near}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let ids: Vec<&str> = answer["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(ids, ["urn:ngsi-ld:Car:1"], "{answer}");
    let outside = "geography'SRID=4326;Polygon((0%200,500%200,500%201,0%201,0%200))'";
    let (status, answer) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=not%20geo.intersects(location,{outside})"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
}

/// T-4031: an evaluation past `antares_residual::MAX_STEPS` (a nested `any` over a list of
/// 1100 items) is a filter too heavy to answer, the 400 `TooComplexQuery`, not a bare refusal.
#[tokio::test(flavor = "multi_thread")]
async fn an_evaluation_past_the_step_budget_is_too_complex_query() {
    let (_b, app) = broker().await.unwrap_or_else(|e| panic!("{e}"));
    let many: Vec<String> = (0..1100).map(|i| format!("t{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let heavy = car(
        3,
        "2026-05-03T08:00:00Z",
        &many,
        &json!({"en": "grey"}),
        "Active",
        [1.0, 1.0],
    );
    let (status, body) = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some(&heavy.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=tags/any(a:tags/any(b:b%20eq%20'zz'))"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "TooComplexQuery", "{body}");

    // the same data under a filter within the budget answers
    let (status, body) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$filter=tags/any(a:a%20eq%20't7')"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["urn:ngsi-ld:Car:3"]);
}

/// T-5206 finding 6: `ngsild.lang` picks the text a LanguageProperty reads as, and the filter
/// matches that text; without it the `@none` entry, and a map without one reads `null` with a note.
#[tokio::test(flavor = "multi_thread")]
async fn a_language_property_reads_and_filters_in_one_language() {
    let (_b, app) = broker().await.expect("broker");
    let one = "Car('urn:ngsi-ld:Car:1')";
    // the ngsild options are a collection read's
    let (status, cs) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?ngsild.lang=cs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cs}");
    let car1 = cs["value"]
        .as_array()
        .and_then(|v| v.iter().find(|c| c["id"] == "urn:ngsi-ld:Car:1"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(car1["label"], "červená", "{cs}");
    let (_, none) = send(&app, Method::GET, &format!("{ROOT}/{one}"), None).await;
    assert_eq!(none["label"], Value::Null, "{none}");
    assert!(none["label@Antares.NgsiLd.Unserved"].is_string(), "{none}");
    let (status, bad) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?ngsild.lang=c%3Bs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
}
