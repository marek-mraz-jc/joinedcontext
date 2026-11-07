// SPDX-License-Identifier: EUPL-1.2
//! Part 5 (draft OGC 23-058r1): schemas, core roles, sortables and `sortby`, `properties`,
//! feature references and `profile` (mapping rows S1 to S3, S5 to S7).

use crate::common::{fixture, get, href, ids, local, send};
use axum::http::StatusCode;
use serde_json::{json, Value};

const ROOT: &str = "/x/views/city/ogc/features";
const CARS: &str = "/x/views/city/ogc/features/collections/Car/items";
const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

fn keys(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// The property names of a schema in `x-ogc-propertySeq` order.
fn in_order(schema: &Value) -> Vec<String> {
    let mut named: Vec<(u64, String)> = schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, p)| {
            let seq = p.get("x-ogc-propertySeq").and_then(Value::as_u64);
            (seq.unwrap_or(u64::MAX), name.clone())
        })
        .collect();
    named.sort();
    named.into_iter().map(|(_, n)| n).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn schema_is_json_schema_2020_12() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, &format!("{ROOT}/collections/Car/schema")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(
        a.headers
            .get("content-type")
            .map(axum::http::HeaderValue::as_bytes),
        Some(&b"application/schema+json"[..])
    );
    assert_eq!(
        a.body.get("$schema"),
        Some(&json!("https://json-schema.org/draft/2020-12/schema"))
    );
    assert_eq!(
        a.body.get("$id"),
        Some(&json!(format!(
            "http://localhost:9090{ROOT}/collections/Car/schema"
        )))
    );
    assert_eq!(
        (
            a.body.get("type"),
            a.body.get("title"),
            a.body.get("description")
        ),
        (
            Some(&json!("object")),
            Some(&json!("Car")),
            Some(&json!("A car of the fleet"))
        )
    );
    assert_eq!(
        in_order(&a.body),
        ["id", "geometry", "speed", "seen", "at", "home", "colour", "tags"],
        "schema order; secret is hidden"
    );
    let p = |name: &str| a.body.pointer(&format!("/properties/{name}")).cloned();
    assert_eq!(
        p("speed"),
        Some(
            json!({"type": "number", "title": "speed", "description": "how fast",
                    "x-ogc-unit": "km/h", "x-ogc-definition": format!("{DC}speed"),
                    "x-ogc-propertySeq": 3})
        )
    );
    assert_eq!(
        p("colour").and_then(|c| c.get("enum").cloned()),
        Some(json!(["blue", "red"]))
    );
    assert_eq!(
        p("tags").and_then(|t| t.get("items").cloned()),
        Some(json!({"type": "string"}))
    );
    let stop = get(&f.app, &format!("{ROOT}/collections/Stop/schema")).await;
    assert_eq!(in_order(&stop.body), ["id", "name"], "no geometry, no time");
    for hidden in ["Depot", "Nope"] {
        let a = get(&f.app, &format!("{ROOT}/collections/{hidden}/schema")).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{hidden}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn roles_follow_the_annotations() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, &format!("{ROOT}/collections/Car/schema")).await;
    let role = |name: &str| {
        a.body
            .pointer(&format!("/properties/{name}/x-ogc-role"))
            .cloned()
    };
    assert_eq!(role("id"), Some(json!("id")));
    assert_eq!(role("geometry"), Some(json!("primary-geometry")));
    assert_eq!(role("seen"), Some(json!("primary-instant")));
    assert_eq!(role("speed"), None);
    assert_eq!(role("at"), Some(json!("reference")));
    assert_eq!(role("home"), Some(json!("reference")));
    assert_eq!(
        a.body.pointer("/properties/at/x-ogc-collectionId"),
        Some(&json!("Stop"))
    );
    assert_eq!(
        a.body.pointer("/properties/home/x-ogc-collectionId"),
        None,
        "Depot is no collection of the view"
    );
    assert_eq!(
        a.body.pointer("/properties/geometry/x-ogc-definition"),
        Some(&json!("https://uri.etsi.org/ngsi-ld/location"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn schema_link_and_response() -> Result<(), String> {
    let f = fixture().await?;
    let c = get(&f.app, &format!("{ROOT}/collections/Car")).await;
    for (rel, path) in [
        ("http://www.opengis.net/def/rel/ogc/1.0/schema", "schema"),
        (
            "http://www.opengis.net/def/rel/ogc/1.0/sortables",
            "sortables",
        ),
    ] {
        let at = href(&c.body, rel).ok_or(rel)?;
        assert!(at.ends_with(&format!("/collections/Car/{path}")), "{at}");
        let a = get(&f.app, &local(&at)).await;
        assert_eq!(a.status, StatusCode::OK, "{at}");
        assert_eq!(a.body.get("$id"), Some(&json!(at)));
    }
    let conformance = get(&f.app, &format!("{ROOT}/conformance")).await;
    let classes = conformance
        .body
        .get("conformsTo")
        .cloned()
        .unwrap_or_default();
    for class in [
        "schemas",
        "core-roles-features",
        "returnables-and-receivables",
        "sortables",
    ] {
        let uri = format!("http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/{class}");
        assert!(
            classes.as_array().is_some_and(|c| c.contains(&json!(uri))),
            "{class}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sortables_and_sortby_pushed_to_order_by() -> Result<(), String> {
    let f = fixture().await?;
    let s = get(&f.app, &format!("{ROOT}/collections/Car/sortables")).await;
    assert_eq!(
        keys(s.body.get("properties")),
        ["colour", "seen", "speed"],
        "no geometry, reference, array or hidden property"
    );
    let down = get(&f.app, &format!("{CARS}?sortby=-speed")).await;
    assert_eq!(
        ids(&down.body),
        [
            "urn:ngsi-ld:Car:4",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:3",
            "urn:ngsi-ld:Car:1"
        ]
    );
    for up in ["speed", "%2Bspeed", "+speed"] {
        let a = get(&f.app, &format!("{CARS}?sortby={up}")).await;
        assert_eq!(
            ids(&a.body),
            [
                "urn:ngsi-ld:Car:1",
                "urn:ngsi-ld:Car:3",
                "urn:ngsi-ld:Car:2",
                "urn:ngsi-ld:Car:4"
            ],
            "{up}"
        );
    }
    let first = get(&f.app, &format!("{CARS}?sortby=-speed&limit=2")).await;
    let next = href(&first.body, "next").ok_or("no next link")?;
    assert!(next.contains("sortby=-speed"), "{next}");
    let second = get(&f.app, &local(&next)).await;
    assert_eq!(
        ids(&second.body),
        ["urn:ngsi-ld:Car:3", "urn:ngsi-ld:Car:1"]
    );
    let unknown = get(&f.app, &format!("{CARS}?sortby=wheels")).await;
    for refused in ["secret", "tags", "at", "location", "-", "speed,"] {
        let a = get(&f.app, &format!("{CARS}?sortby={refused}")).await;
        assert_eq!(
            (a.status, &a.body),
            (StatusCode::BAD_REQUEST, &unknown.body),
            "{refused}: no ordering oracle on a hidden Attribute"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn properties_pushed_to_pick() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(
        &f.app,
        &format!("{CARS}?properties=speed&sortby=speed&limit=1"),
    )
    .await;
    assert_eq!(
        a.body.pointer("/features/0"),
        Some(&json!({"type": "Feature", "id": "urn:ngsi-ld:Car:1",
                     "geometry": {"type": "Point", "coordinates": [17.1, 48.1]},
                     "properties": {"speed": 10},
                     "links": [{"rel": "describedby", "type": "application/schema+json",
                                "href": "http://localhost:9090/x/views/city/ogc/features/collections/Car/schema"}]})),
        "{}",
        a.body
    );
    let none = get(&f.app, &format!("{CARS}/urn:ngsi-ld:Car:1?properties=")).await;
    assert_eq!(none.status, StatusCode::OK, "{}", none.body);
    assert_eq!(none.body.get("properties"), Some(&json!({})));
    assert_eq!(none.body.get("id"), Some(&json!("urn:ngsi-ld:Car:1")));
    let two = get(
        &f.app,
        &format!("{CARS}/urn:ngsi-ld:Car:2?properties=colour,tags"),
    )
    .await;
    assert_eq!(
        two.body.get("properties"),
        Some(&json!({"colour": "red", "tags": ["a", "b"]}))
    );
    let unknown = get(&f.app, &format!("{CARS}?properties=wheels")).await;
    for refused in ["secret", "speed,", "location"] {
        let a = get(&f.app, &format!("{CARS}?properties={refused}")).await;
        assert_eq!(
            (a.status, &a.body),
            (StatusCode::BAD_REQUEST, &unknown.body),
            "{refused}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn relationship_profiles() -> Result<(), String> {
    let f = fixture().await?;
    let car2 = format!("{CARS}/urn:ngsi-ld:Car:2");
    let stop =
        format!("http://localhost:9090{ROOT}/collections/Stop/items/urn%3Angsi-ld%3AStop%3A1");
    let profile = |p: &Value| href(p, "profile");
    let key = get(&f.app, &car2).await;
    assert_eq!(
        (
            key.body.pointer("/properties/at"),
            key.body.pointer("/properties/home")
        ),
        (
            Some(&json!("urn:ngsi-ld:Stop:1")),
            Some(&json!("urn:ngsi-ld:Depot:1"))
        )
    );
    assert_eq!(
        profile(&key.body).as_deref(),
        Some("http://www.opengis.net/def/profile/OGC/0/rel-as-key")
    );
    for uri in [
        "rel-as-uri",
        "http://www.opengis.net/def/profile/ogc/0/rel-as-uri",
        "[ogc-profile:rel-as-uri]",
    ] {
        let a = get(&f.app, &format!("{car2}?profile={uri}")).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}: {}", a.body);
        assert_eq!(
            a.body.pointer("/properties/at"),
            Some(&json!(stop)),
            "{uri}"
        );
        assert_eq!(
            a.body.pointer("/properties/home"),
            Some(&json!("urn:ngsi-ld:Depot:1")),
            "never a link to a type the view hides"
        );
    }
    let link = get(
        &f.app,
        &format!("{CARS}?profile=rel-as-link&sortby=-speed&limit=2"),
    )
    .await;
    assert_eq!(
        link.body.pointer("/features/1/properties/at"),
        Some(&json!({"href": stop, "title": "urn:ngsi-ld:Stop:1"}))
    );
    assert_eq!(
        profile(&link.body).as_deref(),
        Some("http://www.opengis.net/def/profile/OGC/0/rel-as-link")
    );
    let stops = get(
        &f.app,
        &format!("{ROOT}/collections/Stop/items?profile=rel-as-link"),
    )
    .await;
    assert_eq!(profile(&stops.body), None, "no reference, no profile link");
    Ok(())
}

/// A cube measure named `value` is refused at the schema PUT, naming the
/// slot (T-3956); renamed, it shows in the collection's schema beside the
/// other slots.
#[tokio::test(flavor = "multi_thread")]
async fn a_slot_renamed_from_value_shows_in_the_schema() -> Result<(), String> {
    let f = fixture().await?;
    let view = json!({"id": "cube", "tenant": "city", "selector": ["R"],
        "rules": [{"types": ["R"], "attributes": ["pmValue", "pm"]}], "operations": ["query_entities"]});
    let a = send(&f.app, "PUT", "/q/views/cube", &view.to_string(), &[]).await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    // The fixture's schema (its default prefix maps R onto the Entity type)
    // with a class R of two measures.
    let schema = |measure: &str| {
        let mut s: Value = serde_json::from_str(crate::common::SCHEMA).unwrap_or_default();
        if let Some(classes) = s.get_mut("classes").and_then(Value::as_object_mut) {
            classes.insert("R".into(), json!({"slots": [measure, "pm"]}));
        }
        if let Some(slots) = s.get_mut("slots").and_then(Value::as_object_mut) {
            slots.insert(measure.into(), json!({"range": "float"}));
            slots.insert("pm".into(), json!({"range": "float"}));
        }
        s.to_string()
    };
    let a = send(&f.app, "PUT", "/q/schemas/city", &schema("value"), &[]).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.body);
    assert!(
        a.body
            .get("detail")
            .and_then(Value::as_str)
            .is_some_and(|d| d.contains("\"value\"")),
        "{}",
        a.body
    );
    let a = send(&f.app, "PUT", "/q/schemas/city", &schema("pmValue"), &[]).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.body);
    let a = get(&f.app, "/x/views/cube/ogc/features/collections/R/schema").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(
        keys(a.body.get("properties"))
            .iter()
            .filter(|k| *k == "pmValue" || *k == "pm")
            .count(),
        2,
        "{}",
        a.body
    );
    Ok(())
}
