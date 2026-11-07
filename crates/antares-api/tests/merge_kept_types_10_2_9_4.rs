// SPDX-License-Identifier: EUPL-1.2
//! Merge Entity reads a Fragment member by the type of the Attribute it lands
//! on in two cases (T-5466, Coraine patch_entity_url_params,
//! simplified_input_matrix, entity_patch_geoproperty_validation):
//!
//! - 10.2.9.4: "If the Attribute to be merged is represented in a simplified
//!   representation, the type of any pre-existing Attribute in the target
//!   entity shall be preserved." TS 104 176 7.3.3.2 binds the flag to
//!   `format=simplified` (or `keyValues`): "any attempt to modify the type of
//!   an Attribute shall result in a BadRequest error". The flag was accepted
//!   and never read, so `"urn:bob"` turned a Relationship into a Property.
//! - 4.7.1: a GeoProperty's Value "shall be a GeoJSON Geometry". A type-less
//!   `{"value":{"type":"Polygon"}}` on Merge Entity or Partial Attribute
//!   Update stored a geometry with no coordinates.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(st: &AppState, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(format!("/ngsi-ld/v1/{path}"))
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body.to_owned()))
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn seeded(id: &str, attr: Value) -> AppState {
    let st = AppState::new("test".into());
    let body = json!({"id": id, "type": "T", "a": attr}).to_string();
    let (status, resp) = call(&st, "POST", "entities", &body).await;
    assert_eq!(status, StatusCode::CREATED, "seed: {resp}");
    st
}

async fn attr(st: &AppState, id: &str) -> Value {
    let (status, doc) = call(st, "GET", &format!("entities/{id}"), "").await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    doc["a"].clone()
}

async fn merge(st: &AppState, id: &str, query: &str, body: Value) -> (StatusCode, Value) {
    call(
        st,
        "PATCH",
        &format!("entities/{id}{query}"),
        &body.to_string(),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn simplified_merge_keeps_each_stored_attribute_type() {
    let cases = [
        (
            json!({"type": "Relationship", "object": "urn:x:1"}),
            json!("urn:x:9"),
            "object",
            json!("urn:x:9"),
        ),
        (
            json!({"type": "VocabProperty", "vocab": "stop"}),
            json!("go"),
            "vocab",
            json!("go"),
        ),
        (
            json!({"type": "ListProperty", "valueList": [1, 2]}),
            json!([9, 8]),
            "valueList",
            json!([9, 8]),
        ),
        (
            json!({"type": "ListRelationship", "objectList": ["urn:x:1"]}),
            json!(["urn:x:9"]),
            "objectList",
            json!([{"object": "urn:x:9"}]),
        ),
        (
            json!({"type": "JsonProperty", "json": {"k": 1}}),
            json!({"json": {"k": 2}}),
            "json",
            json!({"k": 2}),
        ),
        (
            json!({"type": "Property", "value": 1}),
            json!(7),
            "value",
            json!(7),
        ),
    ];
    for (i, (stored, sent, member, want)) in cases.into_iter().enumerate() {
        let id = format!("urn:ngsi-ld:T:s{i}");
        let kind = stored["type"].clone();
        let st = seeded(&id, stored).await;
        for query in ["?format=simplified", "?options=keyValues"] {
            let (status, body) = merge(&st, &id, query, json!({"a": sent})).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{kind} {query}: {body}");
            let a = attr(&st, &id).await;
            assert_eq!(a["type"], kind, "{query}: type not preserved: {a}");
            assert_eq!(a[member], want, "{query}: {a}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn simplified_string_onto_a_language_property_writes_the_lang_key() {
    let id = "urn:ngsi-ld:T:lang";
    let st = seeded(
        id,
        json!({"type": "LanguageProperty", "languageMap": {"en": "hi", "fr": "salut"}}),
    )
    .await;
    let (status, body) = merge(&st, id, "?format=simplified&lang=es", json!({"a": "hola"})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let a = attr(&st, id).await;
    assert_eq!(a["type"], "LanguageProperty", "{a}");
    assert_eq!(
        a["languageMap"],
        json!({"en": "hi", "fr": "salut", "es": "hola"}),
        "{a}"
    );
    // without lang there is no languageMap key to write
    let (status, body) = merge(&st, id, "?format=simplified", json!({"a": "nolang"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["type"], "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
        "{body}"
    );
    assert_eq!(attr(&st, id).await["languageMap"]["es"], "hola");
}

#[tokio::test(flavor = "multi_thread")]
async fn simplified_value_the_stored_type_cannot_hold_is_refused() {
    let cases = [
        (
            json!({"type": "ListProperty", "valueList": [1, 2]}),
            json!(5),
        ),
        (
            json!({"type": "Relationship", "object": "urn:x:1"}),
            json!(5),
        ),
        (
            json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [1, 2]}}),
            json!("not-a-geometry"),
        ),
    ];
    for (i, (stored, sent)) in cases.into_iter().enumerate() {
        let id = format!("urn:ngsi-ld:T:r{i}");
        let st = seeded(&id, stored.clone()).await;
        let (status, body) = merge(&st, &id, "?format=simplified", json!({"a": sent})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{stored}: {body}");
        assert_eq!(
            attr(&st, &id).await["type"],
            stored["type"],
            "refusal changed {stored}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_format_other_than_simplified_is_refused() {
    let id = "urn:ngsi-ld:T:f";
    let st = seeded(id, json!({"type": "Property", "value": 1})).await;
    let (status, body) = merge(&st, id, "?format=geojson", json!({"a": 2})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(attr(&st, id).await["value"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn typeless_invalid_geometry_onto_a_geoproperty_is_refused() {
    let id = "urn:ngsi-ld:T:geo";
    let point = json!({"type": "Point", "coordinates": [1, 2]});
    let st = seeded(id, json!({"type": "GeoProperty", "value": point})).await;
    let bad = json!({"value": {"type": "Polygon"}});
    let (status, body) = merge(&st, id, "", json!({"a": bad})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "merge: {body}");
    let (status, body) = call(
        &st,
        "PATCH",
        &format!("entities/{id}/attrs/a"),
        &bad.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "partial update: {body}");
    let a = attr(&st, id).await;
    assert_eq!(a["type"], "GeoProperty", "{a}");
    assert_eq!(
        a["value"], point,
        "a refused PATCH changed the geometry: {a}"
    );
    // a valid geometry still lands, and stays a GeoProperty
    let good = json!({"value": {"type": "Point", "coordinates": [5, 6]}});
    let (status, body) = merge(&st, id, "", json!({"a": good})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = call(
        &st,
        "PATCH",
        &format!("entities/{id}/attrs/a"),
        &good.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let a = attr(&st, id).await;
    assert_eq!(
        (a["type"].clone(), a["value"]["coordinates"].clone()),
        (json!("GeoProperty"), json!([5, 6])),
        "{a}"
    );
}
