// SPDX-License-Identifier: EUPL-1.2
//! Part 1 `GeoJSON` (mapping rows C20, M2 to M5).

use crate::common::{fixture, get, href};
use axum::http::StatusCode;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread")]
async fn feature_collection_and_feature_shape() -> Result<(), String> {
    let f = fixture().await?;
    let one = get(
        &f.app,
        "/x/views/city/ogc/features/collections/Car/items/urn:ngsi-ld:Car:1",
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.body);
    assert_eq!(
        one.headers
            .get("content-type")
            .map(axum::http::HeaderValue::as_bytes),
        Some(&b"application/geo+json"[..])
    );
    assert_eq!(one.body.get("type"), Some(&json!("Feature")));
    assert_eq!(
        one.body.get("geometry"),
        Some(&json!({"type": "Point", "coordinates": [17.1, 48.1]}))
    );
    assert_eq!(
        one.body.get("properties"),
        Some(&json!({"speed": 10, "seen": "2026-01-01T00:00:00Z"})),
        "the geometry is not a property; secret is hidden"
    );
    for rel in ["self", "collection"] {
        assert!(href(&one.body, rel).is_some(), "{rel}");
    }
    let no_place = get(
        &f.app,
        "/x/views/city/ogc/features/collections/Car/items/urn:ngsi-ld:Car:3",
    )
    .await;
    assert_eq!(
        no_place.body.get("geometry"),
        Some(&json!(null)),
        "M4: no location is geometry null"
    );
    assert_eq!(no_place.body.get("properties"), Some(&json!({"speed": 30})));
    let stop = get(
        &f.app,
        "/x/views/city/ogc/features/collections/Stop/items/urn:ngsi-ld:Stop:1",
    )
    .await;
    assert_eq!(
        (stop.body.get("geometry"), stop.body.get("properties")),
        (Some(&json!(null)), Some(&json!({"name": "Hlavná"})))
    );
    let page = get(&f.app, "/x/views/city/ogc/features/collections/Car/items").await;
    let members: Vec<&String> = page
        .body
        .as_object()
        .map(|m| m.keys().collect())
        .unwrap_or_default();
    assert_eq!(
        members,
        [
            "features",
            "links",
            "numberMatched",
            "numberReturned",
            "timeStamp",
            "type"
        ],
        "C17: timeStamp is the response time"
    );
    let stamp = page
        .body
        .get("timeStamp")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        antares_ops::residual::DateTime::parse(stamp).is_some(),
        "C17: an RFC 3339 instant, got {stamp:?}"
    );
    assert_eq!(
        stamp.len(),
        "2026-10-28T11:00:00Z".len(),
        "C17: whole seconds, {stamp}"
    );
    assert_eq!(page.body.get("type"), Some(&json!("FeatureCollection")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_language_property_is_one_text_in_the_callers_language() -> Result<(), String> {
    let f = fixture().await?;
    let stop = json!({"id": "urn:ngsi-ld:Stop:2", "type": "Stop",
        "name": {"type": "LanguageProperty", "languageMap": {"fi": "Kaivopuisto", "sv": "Brunnsparken"}}});
    let a = crate::common::send(
        &f.app,
        "POST",
        "/ngsi-ld/v1/entities",
        &stop.to_string(),
        &[("NGSILD-Tenant", "city")],
    )
    .await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    let item = "/x/views/city/ogc/features/collections/Stop/items/urn:ngsi-ld:Stop:2";
    // past 200 characters even valid ranges read as `*`
    let long = "sv,".repeat(70);
    for (accept, name) in [
        ("fi", "Kaivopuisto"),
        (long.as_str(), "Kaivopuisto"),
        ("sv-FI, fi;q=0.5", "Brunnsparken"),
        ("de", "Kaivopuisto"),
        // a header that is no language range reads as `*`
        ("<script>", "Kaivopuisto"),
        ("", "Kaivopuisto"),
    ] {
        let a = crate::common::send(&f.app, "GET", item, "", &[("Accept-Language", accept)]).await;
        assert_eq!(a.status, StatusCode::OK, "{}", a.body);
        let got = a.body.pointer("/properties/name");
        assert_eq!(got, Some(&json!(name)), "{accept}: {}", a.body);
    }
    // without the header one text still, never the languageMap object
    let a = get(&f.app, "/x/views/city/ogc/features/collections/Stop/items").await;
    let features = a.body.get("features").and_then(serde_json::Value::as_array);
    let names: Vec<Option<&serde_json::Value>> = features
        .into_iter()
        .flatten()
        .map(|f| f.pointer("/properties/name"))
        .collect();
    assert!(!names.is_empty(), "{}", a.body);
    assert!(
        names
            .iter()
            .all(|n| n.is_some_and(serde_json::Value::is_string)),
        "{names:?}"
    );
    Ok(())
}
