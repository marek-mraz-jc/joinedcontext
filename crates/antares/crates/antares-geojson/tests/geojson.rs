// SPDX-License-Identifier: EUPL-1.2
//! `antares-geojson` against a broker, generated with the crate by `dev/new-surface.py`: the view
//! narrows what the surface serves, the Policy decides each read, the surface writes nothing,
//! and its parameters are checked. The assertions read ids and names in the answer's text, so
//! they hold whatever [`render`](antares_geojson) writes; a surface adds its own beside them.
//!
//! The Tenant `city` holds the `Place`s `a` and `b` and the hidden `Secret` `s`. The view
//! `city` shows `Place` without its `owner`; `every` shows every type; `writes` allows Create
//! Entity alone; `void` reads the empty Tenant `void`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::Surface as _;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const PLACES: &str = "/x/views/city/geojson/v1/Place";

async fn send(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if uri.starts_with("/ngsi-ld") {
        req = req.header("NGSILD-Tenant", "city");
    }
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("infallible");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8(bytes.to_vec()).expect("utf-8"))
}

/// Denies every Query Entities (clause 5.7) once locked, as a Policy granting no read would.
#[derive(Default)]
struct NoReads(AtomicBool);

impl PolicyEngine for NoReads {
    fn name(&self) -> &'static str {
        "no-reads"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let locked = self.0.load(Ordering::SeqCst);
        Box::pin(std::future::ready(
            if locked && op.clause.starts_with("5.7.") {
                Decision::Deny("no read".into())
            } else {
                Decision::Allow
            },
        ))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn fixture_with(broker: TestBroker) -> Router {
    let app = broker.serve(antares_geojson::GeoJson).expect("mounted");
    let reads = json!(["query_entities"]);
    let views = [
        json!({"id": "city", "tenant": "city", "selector": ["Place"], "operations": reads,
               "rules": [{"types": ["Place"], "attributes": ["name", "location"]}]}),
        json!({"id": "writes", "tenant": "city", "selector": ["Place"], "operations": ["create_entity"],
               "rules": [{"types": ["Place"], "attributes": ["name"]}]}),
        json!({"id": "every", "tenant": "city", "selector": ["*"], "operations": reads,
               "rules": [{"types": ["*"], "attributes": ["*"]}]}),
        json!({"id": "void", "tenant": "void", "selector": ["Place"], "operations": reads,
               "rules": [{"types": ["Place"], "attributes": ["name"]}]}),
    ];
    for view in views {
        let id = view["id"].as_str().expect("id");
        let (status, body) = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "view {id}: {body}");
    }
    let point = |x: f64, y: f64| json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, y]}});
    let entities = [
        json!({"id": "urn:ngsi-ld:Place:a", "type": "Place", "name": {"type": "Property", "value": "Námestie SNP"},
               "location": point(19.146, 48.736), "owner": {"type": "Property", "value": "hidden-owner"}}),
        json!({"id": "urn:ngsi-ld:Place:b", "type": "Place", "name": {"type": "Property", "value": "Urpín"},
               "location": point(19.152, 48.739)}),
        json!({"id": "urn:ngsi-ld:Secret:s", "type": "Secret", "name": {"type": "Property", "value": "hidden-secret"}}),
    ];
    for entity in entities {
        let (status, body) = send(&app, "POST", "/ngsi-ld/v1/entities", &entity.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    app
}

async fn fixture() -> Router {
    fixture_with(TestBroker::new().await.expect("broker")).await
}

#[tokio::test]
async fn a_type_reads_through_the_view_without_what_it_hides() {
    let app = fixture().await;
    let (status, body) = send(&app, "GET", PLACES, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for shown in ["urn:ngsi-ld:Place:a", "urn:ngsi-ld:Place:b", "Námestie SNP"] {
        assert!(body.contains(shown), "{shown} missing: {body}");
    }
    assert!(
        !body.contains("hidden-owner"),
        "a hidden Attribute leaked: {body}"
    );
    let (status, body) = send(&app, "GET", "/x/views/every/geojson/v1/Secret", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("urn:ngsi-ld:Secret:s"),
        "a view of every type shows it: {body}"
    );
    let (status, body) = send(&app, "HEAD", PLACES, "").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
    let info = antares_geojson::GeoJson.version_info();
    assert_eq!(info["endpoint"], "/x/views/{view}/geojson/v1/{type}");
    assert_eq!(info["csv"], "/x/views/{view}/csv/v1/{name}.csv");
}

#[tokio::test]
async fn a_hidden_type_an_unknown_view_and_a_view_without_reads_are_404() {
    let app = fixture().await;
    for uri in [
        "/x/views/city/geojson/v1/Secret",
        "/x/views/nowhere/geojson/v1/Place",
        "/x/views/writes/geojson/v1/Place",
    ] {
        let (status, body) = send(&app, "GET", uri, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert!(!body.contains("hidden-secret"), "{body}");
    }
}

#[tokio::test]
async fn the_policy_decides_every_read() {
    let engine = Arc::new(NoReads::default());
    let broker = TestBroker::new()
        .await
        .expect("broker")
        .with_policy(engine.clone());
    let app = fixture_with(broker).await;
    engine.0.store(true, Ordering::SeqCst);
    let (status, body) = send(&app, "GET", PLACES, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        !body.contains("urn:ngsi-ld:Place:a"),
        "a denied read served data: {body}"
    );
}

#[tokio::test]
async fn the_surface_writes_nothing() {
    let app = fixture().await;
    let (status, _) = send(
        &app,
        "POST",
        PLACES,
        r#"{"id":"urn:ngsi-ld:Place:c","type":"Place"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = send(&app, "OPTIONS", PLACES, "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Place:c", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_and_q_narrow_the_answer() {
    let app = fixture().await;
    let (_, first) = send(&app, "GET", &format!("{PLACES}?limit=1"), "").await;
    let (_, second) = send(&app, "GET", &format!("{PLACES}?limit=1&offset=1"), "").await;
    for (page, other) in [(&first, &second), (&second, &first)] {
        let ids = ["urn:ngsi-ld:Place:a", "urn:ngsi-ld:Place:b"].map(|id| page.contains(id));
        assert_eq!(
            ids.iter().filter(|x| **x).count(),
            1,
            "one Place a page: {page}"
        );
        assert_ne!(page, other);
    }
    let (status, body) = send(
        &app,
        "GET",
        &format!("{PLACES}?q=name==%22Urp%C3%ADn%22"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("urn:ngsi-ld:Place:b") && !body.contains("urn:ngsi-ld:Place:a"),
        "{body}"
    );
}

#[tokio::test]
async fn bad_parameters_are_400_and_an_empty_tenant_an_empty_page() {
    let app = fixture().await;
    for query in [
        "limit=0",
        "limit=101",
        "limit=x",
        "offset=-1",
        "limit=1&limit=2",
        "attrs=name",
        "q=(((",
    ] {
        let (status, body) = send(&app, "GET", &format!("{PLACES}?{query}"), "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    let (status, body) = send(&app, "GET", "/x/views/void/geojson/v1/Place", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains("urn:ngsi-ld:"), "{body}");
}

#[tokio::test]
async fn a_request_naming_another_tenant_is_refused() {
    let app = fixture().await;
    let req = Request::get(PLACES)
        .header("NGSILD-Tenant", "void")
        .body(Body::empty())
        .expect("request");
    let resp = app.oneshot(req).await.expect("infallible");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_page_is_an_rfc_7946_feature_collection() {
    let app = fixture().await;
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("{PLACES}?limit=1"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(resp.headers()["content-type"], "application/geo+json");
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let page: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(page["type"], "FeatureCollection");
    assert_eq!(page["nextOffset"], 1);
    assert_eq!(
        page["features"],
        json!([{"type": "Feature", "id": "urn:ngsi-ld:Place:a",
                "geometry": {"type": "Point", "coordinates": [19.146, 48.736]},
                "properties": {"type": "Place", "name": "Námestie SNP"}}])
    );
    let (_, last) = send(&app, "GET", &format!("{PLACES}?offset=1"), "").await;
    assert_eq!(
        serde_json::from_str::<Value>(&last).expect("json")["nextOffset"],
        Value::Null
    );
}

#[tokio::test]
async fn every_attribute_kind_is_a_plain_property_and_no_location_a_null_geometry() {
    let app = fixture().await;
    let point = |x: f64| json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, 48.7]}, "datasetId": format!("urn:ngsi-ld:Dataset:{x}")});
    let stops = [
        json!({"id": "urn:ngsi-ld:Stop:1", "type": "Stop",
               "location": [point(19.0), {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [20.0, 48.7]}}],
               "servedBy": {"type": "Relationship", "object": "urn:ngsi-ld:Line:6"},
               "label": {"type": "LanguageProperty", "languageMap": {"sk": "Zastávka", "en": "Stop"}},
               "seats": [{"type": "Property", "value": 4, "datasetId": "urn:ngsi-ld:Dataset:a"},
                         {"type": "Property", "value": 2, "datasetId": "urn:ngsi-ld:Dataset:b"}]}),
        json!({"id": "urn:ngsi-ld:Stop:2", "type": "Stop", "name": {"type": "Property", "value": "Bez polohy"}}),
    ];
    for stop in stops {
        let (status, body) = send(&app, "POST", "/ngsi-ld/v1/entities", &stop.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, body) = send(&app, "GET", "/x/views/every/geojson/v1/Stop", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Value = serde_json::from_str(&body).expect("json");
    let features = page["features"].as_array().expect("features");
    let stop = |id: &str| features.iter().find(|f| f["id"] == id).expect(id).clone();
    let one = stop("urn:ngsi-ld:Stop:1");
    // the default instance, as NGSI-LD geo+json picks it (TS 104 175 4.5.16.1), not the first
    assert_eq!(
        one["geometry"],
        json!({"type": "Point", "coordinates": [20.0, 48.7]})
    );
    let props = &one["properties"];
    assert_eq!(props["servedBy"], "urn:ngsi-ld:Line:6");
    assert_eq!(props["label"], json!({"sk": "Zastávka", "en": "Stop"}));
    let mut seats: Vec<i64> = props["seats"]
        .as_array()
        .expect("instances")
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    seats.sort_unstable();
    assert_eq!(seats, [2, 4]);
    assert_eq!(stop("urn:ngsi-ld:Stop:2")["geometry"], Value::Null);
}

/// The view `ev` shows `Event` and `SportEvent` and lists `GeoJSON` and CSV outputs (docs API/11
/// EP-159, T-5216); `Secret` stays hidden from it, and `secretNote` from every Event.
async fn events() -> Router {
    let app = fixture().await;
    let geojson = json!([
        {"name": "sport", "lang": "sk", "filter": {"q": "eventCategory==\"sport\""},
         "sources": [{"type": "SportEvent", "mapping": {"instant": "startDate"}}, {"type": "Event"}]},
        {"name": "catchment", "title": {"sk": "Spádové územie"},
         "sources": [{"type": "Event", "mapping": {"geometry": ["operationSpace", "location"],
            "properties": ["name", "address.addressLocality"], "start": "startDate", "end": "endDate"}}]},
        {"name": "leak", "sources": [{"type": "Secret"}, {"type": "Event"}]},
        {"name": "hidden", "sources": [{"type": "Secret"}]},
        {"name": "broken", "sources": []},
        {"name": "ends", "filter": {"scopeQ": "/bb"},
         "sources": [{"type": "SportEvent", "mapping": {"end": "startDate"}}]},
        {"name": "near", "limit": 1, "sources": [{"type": "Event"}],
         "filter": {"georel": "near;maxDistance==1000", "geometry": "Point", "coordinates": "[19.146,48.736]"}},
    ]);
    let csv = json!([
        {"name": "events", "sources": [{"type": "Event", "mapping": {"columns": ["name", "startDate"]}}]},
        {"name": "all", "sources": [{"type": "Event", "mapping": {"columns": ["name"]}}, {"type": "SportEvent"}]},
        {"name": "one", "limit": 1, "sources": [{"type": "Event", "mapping": {"columns": ["name"]}}]},
    ]);
    let shown = json!([
        "name",
        "location",
        "operationSpace",
        "startDate",
        "endDate",
        "eventCategory",
        "address"
    ]);
    let view = json!({"id": "ev", "tenant": "city", "selector": ["Event", "SportEvent"],
        "operations": ["query_entities"], "rules": [{"types": ["Event", "SportEvent"], "attributes": shown}],
        "representations": {"geojson": geojson, "csv": csv}});
    let (status, body) = send(&app, "PUT", "/q/views/ev", &view.to_string()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let prop = |v: Value| json!({"type": "Property", "value": v});
    let point = |x: f64, y: f64| json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, y]}});
    let area = json!({"type": "GeoProperty", "value": {"type": "Polygon",
        "coordinates": [[[19.0, 48.0], [19.3, 48.0], [19.3, 48.9], [19.0, 48.9], [19.0, 48.0]]]}});
    let entities = [
        json!({"id": "urn:ngsi-ld:Event:e1", "type": "Event", "name": prop(json!("Koncert")),
               "eventCategory": prop(json!("kultura")), "startDate": prop(json!("2026-11-01T18:00:00Z")),
               "endDate": prop(json!("2026-11-01T20:00:00Z")), "location": point(19.146, 48.736),
               "operationSpace": area, "address": prop(json!({"addressLocality": "Banská Bystrica"})),
               "secretNote": prop(json!("hidden-note"))}),
        json!({"id": "urn:ngsi-ld:Event:e2", "type": "Event", "name": prop(json!("=Futbal")),
               "eventCategory": prop(json!("sport")), "startDate": prop(json!("2026-11-02T10:00:00Z")),
               "location": point(20.0, 49.0)}),
        json!({"id": "urn:ngsi-ld:SportEvent:s1", "type": "SportEvent", "scope": "/bb", "name": prop(json!("Beh")),
               "eventCategory": prop(json!("sport")), "startDate": prop(json!("2026-11-03T09:00:00Z")),
               "location": point(19.2, 48.7)}),
    ];
    for entity in entities {
        let (status, body) = send(&app, "POST", "/ngsi-ld/v1/entities", &entity.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    app
}

/// The Features of an answer, by id.
fn by_id(body: &str) -> BTreeMap<String, Value> {
    let page: Value = serde_json::from_str(body).expect("json");
    page["features"]
        .as_array()
        .expect("features")
        .iter()
        .map(|f| (f["id"].as_str().expect("id").to_owned(), f.clone()))
        .collect()
}

#[tokio::test]
async fn an_output_serves_its_sources_under_its_filter_and_the_callers_q() {
    let app = events().await;
    let (status, body) = send(&app, "GET", "/x/views/ev/geojson/v1/sport", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sport = by_id(&body);
    let ids: Vec<&str> = sport.keys().map(String::as_str).collect();
    assert_eq!(ids, ["urn:ngsi-ld:Event:e2", "urn:ngsi-ld:SportEvent:s1"]);
    assert_eq!(
        sport["urn:ngsi-ld:SportEvent:s1"]["time"],
        json!({"instant": "2026-11-03T09:00:00Z"})
    );
    assert_eq!(sport["urn:ngsi-ld:Event:e2"].get("time"), None);
    let (_, body) = send(
        &app,
        "GET",
        "/x/views/ev/geojson/v1/sport?q=name==%22Beh%22",
        "",
    )
    .await;
    let ids: Vec<String> = by_id(&body).into_keys().collect();
    assert_eq!(ids, ["urn:ngsi-ld:SportEvent:s1"]);
    // a caller's q that would widen the output still only narrows it
    let (_, body) = send(
        &app,
        "GET",
        "/x/views/ev/geojson/v1/sport?q=eventCategory==%22kultura%22",
        "",
    )
    .await;
    assert_eq!(by_id(&body).len(), 0, "{body}");
    // a q that closes its own group would OR past the filter: refused, never joined
    let (status, body) = send(
        &app,
        "GET",
        "/x/views/ev/geojson/v1/sport?q=eventCategory==%22sport%22%29%7C%28eventCategory==%22kultura%22",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn an_output_maps_geometry_time_and_properties_by_roles() {
    let app = events().await;
    let (status, body) = send(&app, "GET", "/x/views/ev/geojson/v1/catchment", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let all = by_id(&body);
    let e1 = &all["urn:ngsi-ld:Event:e1"];
    assert_eq!(e1["geometry"]["type"], "Polygon");
    assert_eq!(
        e1["properties"],
        json!({"type": "Event", "name": "Koncert", "address.addressLocality": "Banská Bystrica"})
    );
    assert_eq!(
        e1["time"],
        json!({"interval": ["2026-11-01T18:00:00Z", "2026-11-01T20:00:00Z"]})
    );
    let e2 = &all["urn:ngsi-ld:Event:e2"];
    assert_eq!(
        e2["geometry"],
        json!({"type": "Point", "coordinates": [20.0, 49.0]})
    );
    assert_eq!(
        e2["time"],
        json!({"interval": ["2026-11-02T10:00:00Z", ".."]})
    );
    assert_eq!(e2["properties"]["address.addressLocality"], Value::Null);
    // an interval open at its start; the output's scopeQ intersected with the caller's
    let (_, body) = send(
        &app,
        "GET",
        "/x/views/ev/geojson/v1/ends?scopeQ=/bb/%23",
        "",
    )
    .await;
    let ends = by_id(&body);
    assert_eq!(
        ends["urn:ngsi-ld:SportEvent:s1"]["time"],
        json!({"interval": ["..", "2026-11-03T09:00:00Z"]})
    );
    let (status, _) = send(&app, "GET", "/x/views/ev/geojson/v1/ends?scopeQ=,", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.contains("hidden-note"), "{body}");
}

#[tokio::test]
async fn an_output_never_serves_what_the_view_hides() {
    let app = events().await;
    let (status, body) = send(&app, "GET", "/x/views/ev/geojson/v1/leak", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(by_id(&body).len(), 2, "{body}");
    assert!(
        !body.contains("hidden-secret") && !body.contains("hidden-note"),
        "{body}"
    );
    for (uri, expected) in [
        ("/x/views/ev/geojson/v1/hidden", StatusCode::NOT_FOUND),
        (
            "/x/views/ev/geojson/v1/broken",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        ("/x/views/ev/csv/v1/broken.csv", StatusCode::NOT_FOUND),
        (
            "/x/views/ev/geojson/v1/near?georel=within",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, body) = send(&app, "GET", uri, "").await;
        assert_eq!(status, expected, "{uri}: {body}");
        assert!(!body.contains("hidden-secret"), "{body}");
    }
    let (status, body) = send(&app, "GET", "/x/views/ev/geojson/v1/near", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<String> = by_id(&body).into_keys().collect();
    assert_eq!(ids, ["urn:ngsi-ld:Event:e1"]);
}

#[tokio::test]
async fn a_csv_output_is_one_type_or_a_union_with_a_type_column() {
    let app = events().await;
    let (status, body) = send(&app, "GET", "/x/views/ev/csv/v1/events.csv", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        "id,name,startDate\r\n\
         urn:ngsi-ld:Event:e1,Koncert,2026-11-01T18:00:00Z\r\n\
         urn:ngsi-ld:Event:e2,'=Futbal,2026-11-02T10:00:00Z\r\n"
    );
    let (_, body) = send(&app, "GET", "/x/views/ev/csv/v1/all.csv", "").await;
    let mut lines = body.lines();
    assert_eq!(
        lines.next(),
        Some("id,type,name,eventCategory,location,scope,startDate")
    );
    assert_eq!(lines.count(), 3, "{body}");
    assert!(
        body.contains("urn:ngsi-ld:SportEvent:s1,SportEvent,Beh,sport,"),
        "{body}"
    );
    let (_, body) = send(&app, "GET", "/x/views/ev/csv/v1/one.csv", "").await;
    assert_eq!(body, "id,name\r\nurn:ngsi-ld:Event:e1,Koncert\r\n");
    let (_, body) = send(
        &app,
        "GET",
        "/x/views/ev/csv/v1/events.csv?q=name==%22Koncert%22",
        "",
    )
    .await;
    assert_eq!(body.lines().count(), 2, "{body}");
    // a type no output names: every column the view shows of it
    let (status, body) = send(&app, "GET", "/x/views/city/csv/v1/Place.csv", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.starts_with("id,location,name\r\n"), "{body}");
    assert!(!body.contains("hidden-owner"), "{body}");
    for (uri, expected) in [
        ("/x/views/ev/csv/v1/events.txt", StatusCode::NOT_FOUND),
        ("/x/views/ev/csv/v1/Place.csv", StatusCode::NOT_FOUND),
        (
            "/x/views/ev/csv/v1/events.csv?limit=1",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/x/views/ev/csv/v1/events.csv?q=a&q=b",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, body) = send(&app, "GET", uri, "").await;
        assert_eq!(status, expected, "{uri}: {body}");
    }
    let (status, _) = send(&app, "POST", "/x/views/ev/csv/v1/events.csv", "{}").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn a_csv_past_its_ceiling_is_413_never_cut_short() {
    let app = fixture().await;
    let blob = "x".repeat(900_000);
    for i in 0..5 {
        let big = json!({"id": format!("urn:ngsi-ld:Big:{i}"), "type": "Big",
                         "blob": {"type": "Property", "value": blob}});
        let (status, body) = send(&app, "POST", "/ngsi-ld/v1/entities", &big.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, body) = send(&app, "GET", "/x/views/every/csv/v1/Big.csv", "").await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{}", body.len());
}
