// SPDX-License-Identifier: EUPL-1.2
//! Part 4 (draft OGC 20-002r1): writes through the view (mapping rows W1 to W7), on the
//! `fleet` view, which offers them, and the `city` view, which does not.

use crate::common::{fixture, get, read_only, send};
use axum::http::StatusCode;
use serde_json::{json, Value};

const FLEET: &str = "/x/views/fleet/ogc/features/collections/Car/items";
const GEO: (&str, &str) = ("Content-Type", "application/geo+json");
const PATCH: (&str, &str) = ("Content-Type", "application/merge-patch+json");

fn car(speed: u32, extra: &Value) -> String {
    let mut f = json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [17.0, 48.0]},
                       "properties": {"speed": speed}});
    if let (Some(f), Some(extra)) = (f.as_object_mut(), extra.as_object()) {
        f.extend(extra.clone());
    }
    f.to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn post_creates_and_answers_location() -> Result<(), String> {
    let f = fixture().await?;
    let a = send(&f.app, "POST", FLEET, &car(42, &json!({})), &[GEO]).await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    let at = a
        .headers
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or("no Location")?
        .to_owned();
    assert!(
        at.starts_with(&format!(
            "http://localhost:9090{FLEET}/urn%3Angsi-ld%3ACar%3A"
        )),
        "{at}"
    );
    let read = get(&f.app, at.trim_start_matches("http://localhost:9090")).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.body);
    assert_eq!(read.body.pointer("/properties/speed"), Some(&json!(42)));
    assert_eq!(
        read.body.pointer("/geometry/coordinates"),
        Some(&json!([17.0, 48.0]))
    );
    let named = send(
        &f.app,
        "POST",
        FLEET,
        &car(5, &json!({"id": "urn:ngsi-ld:Car:named",
                         "properties": {"speed": 5, "at": "urn:ngsi-ld:Stop:1", "tags": ["x"], "colour": "blue"}})),
        &[("Content-Type", "application/json")],
    )
    .await;
    assert_eq!(named.status, StatusCode::CREATED, "{}", named.body);
    let read = get(&f.app, &format!("{FLEET}/urn:ngsi-ld:Car:named")).await;
    assert_eq!(
        read.body.get("properties"),
        Some(&json!({"speed": 5, "at": "urn:ngsi-ld:Stop:1", "tags": ["x"], "colour": "blue"}))
    );
    let again = send(
        &f.app,
        "POST",
        FLEET,
        &car(5, &json!({"id": "urn:ngsi-ld:Car:named"})),
        &[GEO],
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);
    let stop = send(
        &f.app,
        "POST",
        "/x/views/fleet/ogc/features/collections/Stop/items",
        &json!({"type": "Feature", "geometry": null, "properties": {"name": "Nová"}}).to_string(),
        &[GEO],
    )
    .await;
    assert_eq!(stop.status, StatusCode::CREATED, "{}", stop.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_outside_the_schema_is_refused_before_it_is_written() -> Result<(), String> {
    let f = fixture().await?;
    let unknown = send(
        &f.app,
        "POST",
        FLEET,
        &car(1, &json!({"properties": {"wheels": 4}})),
        &[GEO],
    )
    .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    let hidden = send(
        &f.app,
        "POST",
        FLEET,
        &car(1, &json!({"properties": {"secret": "x"}})),
        &[GEO],
    )
    .await;
    assert_eq!(
        (hidden.status, &hidden.body),
        (StatusCode::BAD_REQUEST, &unknown.body),
        "hidden reads as unknown"
    );
    for (body, why) in [
        (
            car(1, &json!({"geometry": {"coordinates": [1, 2]}})),
            "geometry without type",
        ),
        (
            car(1, &json!({"geometry": "POINT(1 2)"})),
            "geometry not GeoJSON",
        ),
        (
            car(1, &json!({"type": "FeatureCollection"})),
            "not a Feature",
        ),
        (
            car(1, &json!({"properties": [1]})),
            "properties not an object",
        ),
        (car(1, &json!({"id": 7})), "id not a URI"),
        ("[]".to_owned(), "not an object"),
        ("{".to_owned(), "not JSON"),
    ] {
        let a = send(&f.app, "POST", FLEET, &body, &[GEO]).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{why}: {}", a.body);
    }
    let invalid = send(
        &f.app,
        "POST",
        FLEET,
        &car(
            1,
            &json!({"geometry": {"type": "Point", "coordinates": "here"}}),
        ),
        &[GEO],
    )
    .await;
    assert_eq!(
        invalid.status,
        StatusCode::BAD_REQUEST,
        "the broker refuses an invalid geometry: {}",
        invalid.body
    );
    let stop = send(
        &f.app,
        "POST",
        "/x/views/fleet/ogc/features/collections/Stop/items",
        &json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [1, 2]}, "properties": {}}).to_string(),
        &[GEO],
    )
    .await;
    assert_eq!(stop.status, StatusCode::BAD_REQUEST, "Stop has no geometry");
    let text = send(
        &f.app,
        "POST",
        FLEET,
        &car(1, &json!({})),
        &[("Content-Type", "text/plain")],
    )
    .await;
    assert_eq!(text.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let big = format!(
        "{{\"type\": \"Feature\", \"pad\": \"{}\"}}",
        "x".repeat(1 << 20)
    );
    let big = send(&f.app, "POST", FLEET, &big, &[GEO]).await;
    assert_eq!(big.status, StatusCode::PAYLOAD_TOO_LARGE);
    let all = get(&f.app, &format!("{FLEET}?limit=100")).await;
    assert_eq!(
        all.body.get("numberMatched"),
        Some(&json!(4)),
        "nothing stored"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn put_replaces_and_absent_is_404() -> Result<(), String> {
    let f = fixture().await?;
    let hidden = send(
        &f.app,
        "PUT",
        &format!("{FLEET}/urn:ngsi-ld:Car:2"),
        &car(61, &json!({})),
        &[GEO],
    )
    .await;
    assert_eq!(
        hidden.status,
        StatusCode::FORBIDDEN,
        "a replace would drop secret, which the view hides: {}",
        hidden.body
    );
    let one = format!("{FLEET}/urn:ngsi-ld:Car:4");
    let a = send(
        &f.app,
        "PUT",
        &one,
        &car(61, &json!({"id": "urn:ngsi-ld:Car:4"})),
        &[GEO],
    )
    .await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.body);
    let read = get(&f.app, &one).await;
    assert_eq!(
        read.body.get("properties"),
        Some(&json!({"speed": 61})),
        "replaced: the rest is gone"
    );
    let gone = send(
        &f.app,
        "PUT",
        &one,
        &car(
            61,
            &json!({"geometry": null, "properties": {"speed": 61, "colour": null}}),
        ),
        &[GEO],
    )
    .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.body);
    assert_eq!(
        get(&f.app, &one).await.body.get("geometry"),
        Some(&json!(null))
    );
    let other = send(
        &f.app,
        "PUT",
        &one,
        &car(1, &json!({"id": "urn:ngsi-ld:Car:3"})),
        &[GEO],
    )
    .await;
    assert_eq!(
        other.status,
        StatusCode::BAD_REQUEST,
        "an id that is not the URL's"
    );
    let absent = send(
        &f.app,
        "PUT",
        &format!("{FLEET}/urn:ngsi-ld:Car:99"),
        &car(1, &json!({})),
        &[GEO],
    )
    .await;
    let stop = send(
        &f.app,
        "PUT",
        &format!("{FLEET}/urn:ngsi-ld:Stop:1"),
        &car(1, &json!({})),
        &[GEO],
    )
    .await;
    for a in [&absent, &stop] {
        assert_eq!(
            (a.status, a.body.get("detail")),
            (
                StatusCode::NOT_FOUND,
                Some(&json!("no such Feature in this collection"))
            )
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn patch_is_merge_patch() -> Result<(), String> {
    let f = fixture().await?;
    let one = format!("{FLEET}/urn:ngsi-ld:Car:2");
    let patch = json!({"properties": {"speed": 99, "colour": null}, "geometry": {"type": "Point", "coordinates": [1.0, 2.0]}});
    let a = send(&f.app, "PATCH", &one, &patch.to_string(), &[PATCH]).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.body);
    let read = get(&f.app, &one).await;
    assert_eq!(read.body.pointer("/properties/speed"), Some(&json!(99)));
    assert_eq!(
        read.body.pointer("/properties/colour"),
        None,
        "null deletes (RFC 7396)"
    );
    assert_eq!(
        read.body.pointer("/properties/seen"),
        Some(&json!("2026-06-01T00:00:00Z")),
        "the rest stays"
    );
    assert_eq!(
        read.body.pointer("/geometry/coordinates"),
        Some(&json!([1.0, 2.0]))
    );
    let drop_place = send(&f.app, "PATCH", &one, r#"{"geometry": null}"#, &[PATCH]).await;
    assert_eq!(
        drop_place.status,
        StatusCode::NO_CONTENT,
        "{}",
        drop_place.body
    );
    assert_eq!(
        get(&f.app, &one).await.body.get("geometry"),
        Some(&json!(null))
    );
    let json = send(
        &f.app,
        "PATCH",
        &one,
        r#"{"properties": {"speed": 1}}"#,
        &[GEO],
    )
    .await;
    assert_eq!(
        json.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "a PATCH is a merge patch"
    );
    let absent = send(
        &f.app,
        "PATCH",
        &format!("{FLEET}/urn:ngsi-ld:Car:99"),
        "{}",
        &[PATCH],
    )
    .await;
    assert_eq!(absent.status, StatusCode::NOT_FOUND, "{}", absent.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_and_hidden_is_404() -> Result<(), String> {
    let f = fixture().await?;
    let one = format!("{FLEET}/urn:ngsi-ld:Car:4");
    let a = send(&f.app, "DELETE", &one, "", &[]).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.body);
    assert_eq!(get(&f.app, &one).await.status, StatusCode::NOT_FOUND);
    for absent in ["urn:ngsi-ld:Car:4", "urn:ngsi-ld:Depot:1", "a%zz"] {
        let a = send(&f.app, "DELETE", &format!("{FLEET}/{absent}"), "", &[]).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{absent}: {}", a.body);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn options_lists_offered_methods() -> Result<(), String> {
    let f = fixture().await?;
    let allow = |a: &crate::common::Answer| {
        a.headers
            .get("allow")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    for (path, want) in [
        (FLEET.to_owned(), "GET, HEAD, OPTIONS, POST"),
        (
            format!("{FLEET}/urn:ngsi-ld:Car:1"),
            "GET, HEAD, OPTIONS, PUT, PATCH, DELETE",
        ),
        (
            "/x/views/fleet/ogc/features/collections".to_owned(),
            "GET, HEAD, OPTIONS",
        ),
        (
            "/x/views/city/ogc/features/collections/Car/items/urn:ngsi-ld:Car:1".to_owned(),
            "GET, HEAD, OPTIONS",
        ),
    ] {
        let a = send(&f.app, "OPTIONS", &path, "", &[]).await;
        assert_eq!(
            (a.status, allow(&a).as_deref()),
            (StatusCode::OK, Some(want)),
            "{path}"
        );
    }
    let refused = send(
        &f.app,
        "POST",
        &format!("{FLEET}/urn:ngsi-ld:Car:1"),
        &car(1, &json!({})),
        &[GEO],
    )
    .await;
    assert_eq!(refused.status, StatusCode::METHOD_NOT_ALLOWED);
    let other = send(
        &f.app,
        "DELETE",
        &format!("{FLEET}/urn:ngsi-ld:Car:1"),
        "",
        &[("NGSILD-Tenant", "other")],
    )
    .await;
    assert!(
        other.status.is_client_error(),
        "a view serves one Tenant: {}",
        other.status
    );
    assert_eq!(
        get(&f.app, &format!("{FLEET}/urn:ngsi-ld:Car:1"))
            .await
            .status,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preconditions_are_501() -> Result<(), String> {
    let f = fixture().await?;
    let one = format!("{FLEET}/urn:ngsi-ld:Car:1");
    for header in [
        ("If-Match", "\"x\""),
        ("If-Unmodified-Since", "Sat, 10 Oct 2026 00:00:00 GMT"),
    ] {
        let a = send(&f.app, "PUT", &one, &car(1, &json!({})), &[GEO, header]).await;
        assert_eq!(a.status, StatusCode::NOT_IMPLEMENTED, "{}", header.0);
    }
    assert_eq!(
        get(&f.app, &one).await.body.pointer("/properties/speed"),
        Some(&json!(10)),
        "nothing written"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_refusal_stores_nothing() -> Result<(), String> {
    let f = read_only().await?;
    let one = format!("{FLEET}/urn:ngsi-ld:Car:1");
    for (method, path, body, header) in [
        (
            "POST",
            FLEET.to_owned(),
            car(1, &json!({"id": "urn:ngsi-ld:Car:new"})),
            GEO,
        ),
        ("PUT", one.clone(), car(1, &json!({})), GEO),
        (
            "PATCH",
            one.clone(),
            r#"{"properties": {"speed": 1}}"#.to_owned(),
            PATCH,
        ),
        ("DELETE", one.clone(), String::new(), GEO),
    ] {
        let a = send(&f.app, method, &path, &body, &[header]).await;
        assert_eq!(a.status, StatusCode::FORBIDDEN, "{method}: {}", a.body);
        assert_eq!(
            a.body.get("detail"),
            Some(&json!("the view refused the write")),
            "{method}"
        );
    }
    let read = get(&f.app, &one).await;
    assert_eq!(read.body.pointer("/properties/speed"), Some(&json!(10)));
    assert_eq!(
        get(&f.app, &format!("{FLEET}/urn:ngsi-ld:Car:new"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}
