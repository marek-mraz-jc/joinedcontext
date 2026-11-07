// SPDX-License-Identifier: EUPL-1.2
//! T-5206: each NGSI-LD attribute kind round-trips through the OData surface under the IRI its
//! `$metadata` states; a 4.0 client (Power BI, Excel) reads `Edm.Untyped` members as dynamic
//! properties; a value the member cannot hold reads `null` with a note, never cut; a named
//! instance is never served as the member, and a `PUT` never deletes one.

mod writes;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use writes::{city, odata, send, stored, CITY, ROOT};

/// A request as a client sends it: no `OData-MaxVersion` is a 4.0 client.
async fn get(app: &Router, path: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
    let mut req = Request::builder()
        .uri(format!("{ROOT}/{path}"))
        .header("NGSILD-Tenant", "city");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn create(app: &Router, entity: &Value) {
    let (status, answer) = send(
        app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some(&entity.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
}

fn car(n: u32, attrs: &Value) -> Value {
    let mut car =
        json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"), "scope": "/a"});
    if let (Some(car), Some(attrs)) = (car.as_object_mut(), attrs.as_object()) {
        for (k, v) in attrs {
            car.insert(format!("{CITY}{k}"), v.clone());
        }
    }
    car
}

/// Written through NGSI-LD, read through OData: each member's value, and the `NgsiLd.Iri` its
/// `$metadata` states is the IRI the Entity holds it under.
#[tokio::test(flavor = "multi_thread")]
async fn each_kind_round_trips_under_the_iri_the_metadata_states() {
    let (_b, app) = city().await.expect("broker");
    let point = json!({"type": "Point", "coordinates": [19.1, 48.7]});
    create(
        &app,
        &car(
            50,
            &json!({
                "speed": {"type": "Property", "value": 3.5},
                "location": {"type": "GeoProperty", "value": point},
                "notes": {"type": "JsonProperty", "json": {"a": [1, 2]}},
                "colour": {"type": "VocabProperty", "vocab": "red"},
                "tags": {"type": "ListProperty", "valueList": ["x", "y"]},
                "address": {"type": "Property", "value": {"street": "Hlavná 1"}},
                "ownerRef": {"type": "Property", "value": "p1"}
            }),
        ),
    )
    .await;
    let (status, read) = odata(&app, Method::GET, "Car('urn:ngsi-ld:Car:50')", None).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["speed"], 3.5, "{read}");
    assert_eq!(
        read["location"]["coordinates"],
        json!([19.1, 48.7]),
        "{read}"
    );
    assert_eq!(read["notes"], json!({"a": [1, 2]}), "{read}");
    assert!(
        read["colour"].as_str().is_some_and(|c| c.ends_with("red")),
        "{read}"
    );
    assert_eq!(read["tags"], json!(["x", "y"]), "{read}");
    assert_eq!(read["address"], json!({"street": "Hlavná 1"}), "{read}");

    let (status, xml) = get(&app, "$metadata", &[("OData-MaxVersion", "4.01")]).await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    for name in ["speed", "location", "notes", "colour", "tags", "address"] {
        let iri = format!(r#"<Property Name="{name}""#);
        let at = xml.find(&iri).unwrap_or_else(|| panic!("{name} in {xml}"));
        let rest = xml.get(at..).unwrap_or_default();
        let end = rest.find("</Property>").unwrap_or(rest.len());
        let stated = format!(r#"String="{CITY}{name}""#);
        assert!(
            rest.get(..end).unwrap_or_default().contains(&stated),
            "{name}: {stated} in {}",
            rest.get(..end).unwrap_or_default()
        );
    }
    let (_, held) = stored(&app, "urn:ngsi-ld:Car:50").await;
    for name in ["speed", "location", "notes", "colour", "tags", "address"] {
        assert!(
            held.get(format!("{CITY}{name}")).is_some(),
            "{name}: {held}"
        );
    }
}

/// Finding 5: a 4.0 `$metadata` (Power BI, Excel) has no `Edm.Untyped` and an open type, and the
/// 4.0 read and `$select` serve the untyped member as a dynamic property.
#[tokio::test(flavor = "multi_thread")]
async fn a_4_0_client_reads_and_selects_an_untyped_member_as_dynamic() {
    let (_b, app) = city().await.expect("broker");
    create(
        &app,
        &car(
            51,
            &json!({"address": {"type": "Property", "value": {"street": "Dolná 2"}}}),
        ),
    )
    .await;
    let (status, xml) = get(&app, "$metadata", &[]).await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert!(xml.contains(r#"Version="4.0""#), "{xml}");
    assert!(!xml.contains("Edm.Untyped"), "{xml}");
    assert!(xml.contains(r#"OpenType="true""#), "{xml}");
    let (status, one) = get(&app, "Car('urn:ngsi-ld:Car:51')", &[]).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    let one: Value = serde_json::from_str(&one).unwrap_or_default();
    assert_eq!(one["address"]["street"], "Dolná 2", "{one}");
    let (status, picked) = get(&app, "Car?$select=address", &[]).await;
    assert_eq!(status, StatusCode::OK, "{picked}");
    assert!(picked.contains("Dolná 2"), "{picked}");
    // The open type is how 4.0 serves the model's own untyped members, never any name: a name
    // no model holds stays 400 in a query option and 404 in a path (8.1.6, 11.2.3; T-5206 bounce).
    for (path, want) in [
        ("Car?$select=width", StatusCode::BAD_REQUEST),
        ("Car?$filter=width%20eq%201", StatusCode::BAD_REQUEST),
        ("Car?$orderby=width", StatusCode::BAD_REQUEST),
        ("Car('urn:ngsi-ld:Car:51')/width", StatusCode::NOT_FOUND),
    ] {
        let (status, body) = get(&app, path, &[]).await;
        assert_eq!(status, want, "{path}: {body}");
    }
}

/// Findings 7, 8 and 9: an array under a single-valued member and a stored instance of another
/// kind read `null` with a note; only the default instance is served; `observedAt` and
/// `unitCode` come as instance annotations when the client asks for them.
#[tokio::test(flavor = "multi_thread")]
async fn values_a_member_cannot_hold_are_noted_never_cut() {
    let (_b, app) = city().await.expect("broker");
    create(
        &app,
        &car(
            52,
            &json!({
                "speed": [{"type": "Property", "value": 9.0, "datasetId": "urn:ngsi-ld:Dataset:radar"}],
                "label": {"type": "Property", "value": ["a", "b"]},
                "colour": {"type": "Property", "value": "red"},
                "ownerRef": {"type": "Property", "value": "p1", "observedAt": "2026-10-22T08:00:00Z",
                             "unitCode": "C62"}
            }),
        ),
    )
    .await;
    let (status, read) = odata(&app, Method::GET, "Car('urn:ngsi-ld:Car:52')", None).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        read["speed"],
        Value::Null,
        "a named instance is not the member: {read}"
    );
    assert_eq!(read["label"], Value::Null, "never the first item: {read}");
    assert!(read["label@Antares.NgsiLd.Unserved"].is_string(), "{read}");
    assert_eq!(read["colour"], Value::Null, "{read}");
    assert!(read["colour@Antares.NgsiLd.Unserved"].is_string(), "{read}");
    assert_eq!(
        read.get("ownerRef@Antares.NgsiLd.observedAt"),
        None,
        "not asked: {read}"
    );
    let (status, annotated) = get(
        &app,
        "Car('urn:ngsi-ld:Car:52')",
        &[
            ("OData-MaxVersion", "4.01"),
            ("Prefer", r#"odata.include-annotations="Antares.NgsiLd.*""#),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{annotated}");
    let annotated: Value = serde_json::from_str(&annotated).unwrap_or_default();
    assert_eq!(
        annotated["ownerRef@Antares.NgsiLd.observedAt"], "2026-10-22T08:00:00Z",
        "{annotated}"
    );
    assert_eq!(
        annotated["ownerRef@Antares.NgsiLd.unitCode"], "C62",
        "{annotated}"
    );
}

/// Finding 8 and the task's Security line: a `PUT` never deletes an instance with a
/// `datasetId`, nor an Attribute the view hides; both are refused and the Entity is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn a_put_never_deletes_named_instances_or_hidden_attributes() {
    let (_b, app) = city().await.expect("broker");
    create(
        &app,
        &car(
            53,
            &json!({"speed": [{"type": "Property", "value": 1.0},
                              {"type": "Property", "value": 9.0, "datasetId": "urn:ngsi-ld:Dataset:radar"}]}),
        ),
    )
    .await;
    let mut hidden = car(54, &json!({"speed": {"type": "Property", "value": 1.0}}));
    hidden[format!("{CITY}vin")] = json!({"type": "Property", "value": "secret-vin"});
    create(&app, &hidden).await;
    let body = json!({"speed": 2.0, "scope": ["/a"]}).to_string();
    let (status, answer) = odata(&app, Method::PUT, "Car('urn:ngsi-ld:Car:53')", Some(&body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    let (_, held) = stored(&app, "urn:ngsi-ld:Car:53").await;
    assert_eq!(
        held[format!("{CITY}speed")].as_array().map(Vec::len),
        Some(2),
        "{held}"
    );
    let (status, answer) = odata(&app, Method::PUT, "Car('urn:ngsi-ld:Car:54')", Some(&body)).await;
    assert!(status.is_client_error(), "{status} {answer}");
    let (_, held) = stored(&app, "urn:ngsi-ld:Car:54").await;
    assert_eq!(held[format!("{CITY}vin")]["value"], "secret-vin", "{held}");
    let patch = json!({"speed": 3.0}).to_string();
    let (status, answer) = odata(
        &app,
        Method::PATCH,
        "Car('urn:ngsi-ld:Car:53')",
        Some(&patch),
    )
    .await;
    assert!(
        status.is_success(),
        "PATCH keeps the named instance: {status} {answer}"
    );
}

/// T-5333: a `range: Any` Property (a KPI's `currentValue`: a number, or the string
/// `not measured`) reads at 4.0 as a dynamic property and filters at 4.0 and 4.01 by its JSON
/// value, as `q` does: a value of another type matches only `ne`.
#[tokio::test(flavor = "multi_thread")]
async fn an_untyped_value_reads_at_4_0_and_filters_by_its_json_value() {
    let (_b, app) = city().await.expect("broker");
    for (n, value) in [
        (60, json!(9000)),
        (61, json!("not measured")),
        (62, json!(3.5)),
    ] {
        create(
            &app,
            &car(n, &json!({"reading": {"type": "Property", "value": value}})),
        )
        .await;
    }
    let ids = |body: &str| -> Vec<String> {
        let body: Value = serde_json::from_str(body).unwrap_or_default();
        let mut ids: Vec<String> = body["value"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str().map(str::to_owned))
            .collect();
        ids.sort();
        ids
    };
    let car = |n: u32| format!("urn:ngsi-ld:Car:{n}");
    let (status, one) = get(&app, "Car('urn:ngsi-ld:Car:61')", &[]).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    let one: Value = serde_json::from_str(&one).unwrap_or_default();
    assert_eq!(one["reading"], "not measured", "{one}");
    for version in [&[][..], &[("OData-MaxVersion", "4.01")][..]] {
        for (filter, want) in [
            ("reading%20ge%208372", vec![car(60)]),
            ("reading%20lt%2010", vec![car(62)]),
            ("reading%20eq%20'not%20measured'", vec![car(61)]),
            ("reading%20ne%209000", vec![car(1), car(61), car(62)]),
            (
                "not%20(reading%20ge%208372)",
                vec![car(1), car(61), car(62)],
            ),
        ] {
            let path = format!("Car?$filter={filter}&$select=id,reading");
            let (status, body) = get(&app, &path, version).await;
            assert_eq!(status, StatusCode::OK, "{version:?} {path}: {body}");
            assert_eq!(ids(&body), want, "{version:?} {path}: {body}");
        }
        // a literal no JSON value is, a JsonProperty and a complex value still cannot hold
        for filter in [
            "reading%20eq%202026-10-22",
            "notes%20eq%201",
            "address%20gt%201",
            "address%20eq%20'x'",
        ] {
            let path = format!("Car?$filter={filter}");
            let (status, body) = get(&app, &path, version).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{version:?} {path}: {body}"
            );
        }
    }
    let (status, picked) = get(&app, "Car?$filter=reading%20ge%208372&$select=reading", &[]).await;
    assert_eq!(status, StatusCode::OK, "{picked}");
    assert!(picked.contains("9000"), "{picked}");
    // $metadata says so: `reading` is filterable, never sortable; `notes` and `address` neither
    let (_, xml) = get(&app, "$metadata", &[("OData-MaxVersion", "4.01")]).await;
    let section = |term: &str| {
        let at = xml.find(term).unwrap_or_else(|| panic!("{term} in {xml}"));
        let rest = xml.get(at..).unwrap_or_default();
        rest.get(..rest.find("</Annotation>").unwrap_or(rest.len()))
            .unwrap_or_default()
            .to_owned()
    };
    let filter = section("Capabilities.FilterRestrictions");
    assert!(!filter.contains(">reading<"), "{filter}");
    assert!(filter.contains(">notes<"), "{filter}");
    assert!(filter.contains(">address<"), "{filter}");
    assert!(section("Capabilities.SortRestrictions").contains(">reading<"));
}
