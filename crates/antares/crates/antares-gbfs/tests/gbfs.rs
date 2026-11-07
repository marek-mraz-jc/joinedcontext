// SPDX-License-Identifier: EUPL-1.2
//! `antares-gbfs` against a broker (T-4488): the view narrows what a feed serves, the Policy
//! decides each read, the surface writes nothing, and each file is the one in `tests/gbfs/golden`,
//! which `tests/gbfs/validate.py` checks against the `MobilityData` GBFS 3.0 JSON schemas.
//!
//! The Tenant `city` holds the stations `kamppi` (live), `toolo` (no live status) and the hidden
//! `Secret` `s`. The view `city` shows the stations without their `owner` and lists the output
//! `bikes`; `bare` lists none; `broken` one that does not hold; `writes` allows Create Entity
//! alone; `void` reads the empty Tenant `void`.

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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

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

const FEED: &str = "/x/views/city/gbfs/v1/bikes";

fn output() -> Value {
    json!([{"name": "bikes", "systemId": "hsl-citybikes", "language": "fi",
            "title": {"fi": "Kaupunkipyörät", "en": "City bikes"}, "timezone": "Europe/Helsinki",
            "feedContactEmail": "open@hel.example", "openingHours": "Apr 1-Oct 31", "ttl": 60}])
}

async fn fixture_with(broker: TestBroker) -> Router {
    let app = broker.serve(antares_gbfs::Gbfs).expect("mounted");
    let reads = json!(["query_entities"]);
    let ty = "BikeHireDockingStation";
    let shown = json!([
        "name",
        "location",
        "availableBikeNumber",
        "freeSlotNumber",
        "totalSlotNumber",
        "status"
    ]);
    let views = [
        json!({"id": "city", "tenant": "city", "selector": [ty], "operations": reads,
               "rules": [{"types": [ty], "attributes": shown}], "representations": {"gbfs": output()}}),
        json!({"id": "bare", "tenant": "city", "selector": [ty], "operations": reads,
               "rules": [{"types": [ty], "attributes": shown}]}),
        json!({"id": "broken", "tenant": "city", "selector": [ty], "operations": reads,
               "rules": [{"types": [ty], "attributes": shown}], "representations": {"gbfs": [{"name": "bikes"}]}}),
        json!({"id": "secret", "tenant": "city", "selector": ["Secret"], "operations": reads,
               "rules": [{"types": ["Secret"], "attributes": ["name"]}], "representations": {"gbfs": output()}}),
        json!({"id": "writes", "tenant": "city", "selector": [ty], "operations": ["create_entity"],
               "rules": [{"types": [ty], "attributes": ["name"]}], "representations": {"gbfs": output()}}),
        json!({"id": "every", "tenant": "city", "selector": ["*"], "operations": reads,
               "rules": [{"types": ["*"], "attributes": ["*"]}], "representations": {"gbfs": output()}}),
        json!({"id": "void", "tenant": "void", "selector": [ty], "operations": reads,
               "rules": [{"types": [ty], "attributes": shown}], "representations": {"gbfs": output()}}),
    ];
    for view in views {
        let id = view["id"].as_str().expect("id");
        let (status, body) = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "view {id}: {body}");
    }
    let at = "2026-10-23T10:00:00Z";
    let number = |n: u64| json!({"type": "Property", "value": n, "observedAt": at});
    let point = |x: f64, y: f64| json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, y]}});
    let entities = [
        json!({"id": "urn:ngsi-ld:BikeHireDockingStation:kamppi", "type": ty,
               "name": {"type": "LanguageProperty", "languageMap": {"fi": "Kamppi", "sv": "Kampen"}},
               "location": point(24.9316, 60.1690), "availableBikeNumber": number(4), "freeSlotNumber": number(6),
               "totalSlotNumber": number(10), "status": {"type": "Property", "value": "working", "observedAt": at},
               "owner": {"type": "Property", "value": "hidden-owner"}}),
        json!({"id": "urn:ngsi-ld:BikeHireDockingStation:toolo", "type": ty,
               "name": {"type": "LanguageProperty", "languageMap": {"fi": "Töölöntori"}},
               "location": point(24.9230, 60.1850)}),
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

/// The committed file `name` of `tests/gbfs/golden`.
fn golden(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../tests/gbfs/golden/{name}"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("golden file")).expect("json")
}

#[tokio::test]
async fn each_file_is_the_validated_golden_one_without_what_the_view_hides() {
    let app = fixture().await;
    for file in antares_gbfs::FILES {
        let (status, body) = send(&app, "GET", &format!("{FEED}/{file}.json"), "").await;
        assert_eq!(status, StatusCode::OK, "{file}: {body}");
        assert!(
            !body.contains("hidden-owner"),
            "a hidden Attribute leaked: {body}"
        );
        let got: Value = serde_json::from_str(&body).expect("json");
        // a changed mapping: write the served file below into the golden one, then run
        // tests/gbfs/validate.py on it
        assert_eq!(
            got,
            golden(&format!("{file}.json")),
            "{file}: served {got:#}"
        );
        // one read, one moment: every file carries the newest station report and the output's ttl
        assert_eq!(
            (got["last_updated"].clone(), got["ttl"].clone()),
            (json!("2026-10-23T10:00:00Z"), json!(60))
        );
    }
    let (status, body) = send(
        &app,
        "GET",
        "/x/views/every/gbfs/v1/bikes/station_status.json",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("kamppi"),
        "a view of every type shows the stations: {body}"
    );
    let (status, body) = send(&app, "HEAD", &format!("{FEED}/gbfs.json"), "").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
    let info = antares_gbfs::Gbfs.version_info();
    assert_eq!(info["endpoint"], "/x/views/{view}/gbfs/v1/{name}/gbfs.json");
}

#[tokio::test]
async fn a_station_without_a_live_status_is_counted_and_left_out_of_station_status() {
    let app = fixture().await;
    let req = Request::get(format!("{FEED}/station_status.json"))
        .body(Body::empty())
        .expect("request");
    let resp = app.clone().oneshot(req).await.expect("infallible");
    assert_eq!(resp.headers()[antares_gbfs::SKIPPED_HEADER], "1");
    assert_eq!(resp.headers()["cache-control"], "private, max-age=60");
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let body = String::from_utf8(bytes.to_vec()).expect("utf-8");
    assert!(!body.contains("toolo"), "{body}");
    let (_, info) = send(&app, "GET", &format!("{FEED}/station_information.json"), "").await;
    assert!(
        info.contains("Töölöntori"),
        "the station is still listed: {info}"
    );
}

#[tokio::test]
async fn a_hidden_type_an_unknown_view_output_or_file_and_a_view_without_reads_are_404() {
    let app = fixture().await;
    for uri in [
        "/x/views/secret/gbfs/v1/bikes/gbfs.json",
        "/x/views/nowhere/gbfs/v1/bikes/gbfs.json",
        "/x/views/writes/gbfs/v1/bikes/gbfs.json",
        "/x/views/bare/gbfs/v1/bikes/gbfs.json",
        "/x/views/city/gbfs/v1/cars/gbfs.json",
        "/x/views/city/gbfs/v1/bikes/vehicle_status.json",
        "/x/views/city/gbfs/v1/bikes/gbfs",
    ] {
        let (status, body) = send(&app, "GET", uri, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert!(!body.contains("hidden-secret"), "{body}");
    }
    let (status, body) = send(&app, "GET", "/x/views/broken/gbfs/v1/bikes/gbfs.json", "").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body.contains("published again"), "{body}");
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
    let (status, body) = send(&app, "GET", &format!("{FEED}/station_status.json"), "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        !body.contains("kamppi"),
        "a denied read served data: {body}"
    );
}

#[tokio::test]
async fn the_surface_writes_nothing() {
    let app = fixture().await;
    let uri = format!("{FEED}/station_status.json");
    let (status, _) = send(
        &app,
        "POST",
        &uri,
        r#"{"id":"urn:ngsi-ld:BikeHireDockingStation:c","type":"BikeHireDockingStation"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = send(&app, "OPTIONS", &uri, "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:BikeHireDockingStation:c",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_empty_tenant_is_a_feed_without_stations() {
    let app = fixture().await;
    let (status, body) = send(
        &app,
        "GET",
        "/x/views/void/gbfs/v1/bikes/station_information.json",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(got["data"]["stations"], json!([]));
}

#[tokio::test]
async fn a_request_naming_another_tenant_is_refused() {
    let app = fixture().await;
    let req = Request::get(format!("{FEED}/gbfs.json"))
        .header("NGSILD-Tenant", "void")
        .body(Body::empty())
        .expect("request");
    let resp = app.oneshot(req).await.expect("infallible");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
