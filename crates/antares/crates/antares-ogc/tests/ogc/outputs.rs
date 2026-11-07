// SPDX-License-Identifier: EUPL-1.2
//! Named OGC outputs (docs API/11 EP-159, T-5216): a view's `representations.ogc` list serves
//! one collection per output over one class, its geometry, time and properties from the
//! source's roles, read only, its filter a conjunct of every read, never past the view.

use crate::common::{get, ids, schemas_on, send};
use antares_ops::test_kit::TestBroker;
use axum::http::StatusCode;
use axum::Router;
use serde_json::{json, Value};

const SCHEMA: &str = r#"{
  "id": "https://example.org/events", "name": "events",
  "prefixes": {"dc": {"prefix_prefix": "dc", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"},
               "ngsi": {"prefix_prefix": "ngsi", "prefix_reference": "https://uri.etsi.org/ngsi-ld/"}},
  "default_prefix": "dc",
  "classes": {"Event": {"slots": ["name", "category", "location", "operationSpace", "startDate", "endDate"]},
              "Venue": {"slots": ["name", "location"]}},
  "slots": {
    "name": {"range": "string"},
    "category": {"range": "string"},
    "location": {"slot_uri": "ngsi:location", "range": "string",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
    "operationSpace": {"slot_uri": "ngsi:operationSpace", "range": "string",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
    "startDate": {"range": "datetime"},
    "endDate": {"range": "datetime"}
  }
}"#;

const ITEMS: &str = "/x/views/ev/ogc/features/collections";

async fn fixture() -> Result<(TestBroker, Router), String> {
    schemas_on();
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_ogc::Ogc)?;
    let source = |mapping: Value| json!([{"type": "Event", "mapping": mapping}]);
    let ogc = json!([
        {"name": "venues", "title": {"sk": "Miesta konania", "en": "Venues"},
         "sources": source(json!({"geometry": "location", "instant": "startDate"}))},
        {"name": "catchment", "description": {"en": "Where the sport events reach"},
         "filter": {"q": "category==\"sport\""},
         "sources": source(json!({"geometry": "operationSpace", "start": "startDate", "end": "endDate",
                                  "properties": ["name"]}))},
        {"name": "loose", "sources": [{"type": "Event"}]},
        {"name": "starts", "sources": source(json!({"start": "startDate"}))},
        {"name": "scoped", "filter": {"scopeQ": "/bb"}, "sources": [{"type": "Event"}]},
        {"name": "ghost", "sources": source(json!({"geometry": "name"}))},
        {"name": "nobody", "sources": source(json!({"properties": ["nope"]}))},
        {"name": "untimed", "sources": source(json!({"instant": "nope"}))},
        {"name": "unended", "sources": source(json!({"end": "nope"}))},
        {"name": "elsewhere", "sources": [{"type": "Hall"}]},
        {"name": "places", "sources": [{"type": "Venue"}]},
        {"name": "broken", "sources": []}
    ]);
    let shown = json!([
        "name",
        "category",
        "location",
        "operationSpace",
        "startDate",
        "endDate"
    ]);
    let view = json!({"id": "ev", "tenant": "city", "selector": ["Event", "Venue"], "q": "category!=\"hidden\"",
        "operations": ["query_entities", "create_entity"],
        "rules": [{"types": ["Event"], "attributes": shown}, {"types": ["Venue"], "attributes": ["name", "location"]}],
        "representations": {"ogc": ogc}});
    let a = send(&app, "PUT", "/q/views/ev", &view.to_string(), &[]).await;
    if a.status != StatusCode::CREATED {
        return Err(format!("view: {} {}", a.status, a.body));
    }
    let a = send(&app, "PUT", "/q/schemas/city", SCHEMA, &[]).await;
    if a.status != StatusCode::CREATED {
        return Err(format!("schema: {} {}", a.status, a.body));
    }
    let prop = |v: &str| json!({"type": "Property", "value": v});
    let point = |x: f64| json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, 48.7]}});
    let area = json!({"type": "GeoProperty", "value": {"type": "Polygon",
        "coordinates": [[[19.0, 48.0], [19.3, 48.0], [19.3, 48.9], [19.0, 48.9], [19.0, 48.0]]]}});
    let entities = [
        json!({"id": "urn:ngsi-ld:Event:1", "type": "Event", "scope": "/bb", "name": prop("Koncert"),
               "category": prop("kultura"), "location": point(19.1), "startDate": prop("2026-11-01T18:00:00Z")}),
        json!({"id": "urn:ngsi-ld:Event:2", "type": "Event", "name": prop("Futbal"), "category": prop("sport"),
               "location": point(19.2), "operationSpace": area, "startDate": prop("2026-11-02T10:00:00Z"),
               "endDate": prop("2026-11-02T12:00:00Z")}),
        json!({"id": "urn:ngsi-ld:Event:3", "type": "Event", "name": prop("Tajné"), "category": prop("hidden"),
               "location": point(19.3)}),
    ];
    for entity in entities {
        let a = send(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            &entity.to_string(),
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        if a.status != StatusCode::CREATED {
            return Err(format!("{entity}: {} {}", a.status, a.body));
        }
    }
    Ok((broker, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn each_output_is_a_collection_and_one_that_does_not_hold_is_none() -> Result<(), String> {
    let (_broker, app) = fixture().await?;
    let a = send(&app, "GET", ITEMS, "", &[("Accept-Language", "sk")]).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let listed: Vec<&str> = a
        .body
        .get("collections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(
        listed,
        [
            "Event",
            "Venue",
            "catchment",
            "loose",
            "places",
            "scoped",
            "starts",
            "venues"
        ]
    );
    // the API definition offers no write on an output
    let a = get(&app, "/x/views/ev/ogc/features/api").await;
    assert_eq!(a.status, StatusCode::OK);
    let paths = a.body.get("paths");
    let catchment = paths.and_then(|p| p.get("/collections/catchment/items"));
    let event = paths.and_then(|p| p.get("/collections/Event/items"));
    assert_eq!(catchment.and_then(|c| c.get("post")), None, "{catchment:?}");
    assert!(event.and_then(|c| c.get("post")).is_some(), "{event:?}");
    let a = send(
        &app,
        "GET",
        &format!("{ITEMS}/venues"),
        "",
        &[("Accept-Language", "sk")],
    )
    .await;
    assert_eq!(
        a.body.get("title"),
        Some(&json!("Miesta konania")),
        "{}",
        a.body
    );
    let a = get(&app, &format!("{ITEMS}/catchment")).await;
    assert_eq!(
        (a.body.get("title"), a.body.get("description")),
        (
            Some(&json!("catchment")),
            Some(&json!("Where the sport events reach"))
        )
    );
    for gone in [
        "ghost",
        "nobody",
        "untimed",
        "unended",
        "elsewhere",
        "broken",
    ] {
        let a = get(&app, &format!("{ITEMS}/{gone}/items")).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{gone}: {}", a.body);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn two_outputs_over_one_type_serve_different_geometries_and_times() -> Result<(), String> {
    let (_broker, app) = fixture().await?;
    let a = get(&app, &format!("{ITEMS}/venues/items")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:1", "urn:ngsi-ld:Event:2"]);
    let first = a.body.pointer("/features/0/geometry/type");
    assert_eq!(first, Some(&json!("Point")));
    let a = get(&app, &format!("{ITEMS}/catchment/items")).await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:2"]);
    let feature = a.body.pointer("/features/0").cloned().unwrap_or_default();
    assert_eq!(feature.pointer("/geometry/type"), Some(&json!("Polygon")));
    let names: Vec<&String> = feature
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(names, ["endDate", "name", "startDate"], "{feature}");
    let a = get(
        &app,
        &format!("{ITEMS}/catchment/items?datetime=2026-11-02T11:00:00Z"),
    )
    .await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:2"], "{}", a.body);
    let a = get(
        &app,
        &format!("{ITEMS}/venues/items?datetime=2026-11-02T10:00:00Z"),
    )
    .await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:2"], "{}", a.body);
    let a = get(
        &app,
        &format!("{ITEMS}/starts/items?datetime=2026-11-01T00:00:00Z/2026-11-01T23:00:00Z"),
    )
    .await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:1"], "{}", a.body);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_output_only_narrows_and_writes_nothing() -> Result<(), String> {
    let (_broker, app) = fixture().await?;
    // the filter holds for one Feature as for a page, and the view's own q under it
    let a = get(
        &app,
        &format!("{ITEMS}/catchment/items/urn:ngsi-ld:Event:1"),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    let a = get(
        &app,
        &format!("{ITEMS}/catchment/items/urn:ngsi-ld:Event:2"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = get(&app, &format!("{ITEMS}/loose/items")).await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:1", "urn:ngsi-ld:Event:2"]);
    let a = get(
        &app,
        &format!("{ITEMS}/catchment/items?ngsild.q=category==%22kultura%22"),
    )
    .await;
    assert_eq!(ids(&a.body).len(), 0, "{}", a.body);
    // a q that closes its own group would OR past the filter: refused, never joined
    let a = get(
        &app,
        &format!(
            "{ITEMS}/catchment/items?ngsild.q=category==%22kultura%22%29%7C%28category==%22x%22"
        ),
    )
    .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.body);
    let a = get(&app, &format!("{ITEMS}/scoped/items")).await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:1"], "{}", a.body);
    let a = get(&app, &format!("{ITEMS}/scoped/items?ngsild.scopeQ=/zz")).await;
    assert_eq!(
        (a.status, ids(&a.body).len()),
        (StatusCode::OK, 0),
        "{}",
        a.body
    );
    let a = get(&app, &format!("{ITEMS}/scoped/items?ngsild.scopeQ=,")).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.body);
    let a = get(
        &app,
        &format!("{ITEMS}/scoped/items?ngsild.scopeQ=/bb&filter=name%3D%27Koncert%27"),
    )
    .await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Event:1"], "{}", a.body);
    let feature = json!({"type": "Feature", "id": "urn:ngsi-ld:Event:9", "geometry": null,
                         "properties": {"name": "Nové", "category": "sport"}});
    let a = send(
        &app,
        "POST",
        &format!("{ITEMS}/catchment/items"),
        &feature.to_string(),
        &[("Content-Type", "application/geo+json")],
    )
    .await;
    assert_eq!(a.status, StatusCode::METHOD_NOT_ALLOWED, "{}", a.body);
    let a = get(&app, &format!("{ITEMS}/loose/items/urn:ngsi-ld:Event:3")).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    Ok(())
}
