// SPDX-License-Identifier: EUPL-1.2
//! `antares-gtfsrt` against a broker (T-4489): the view narrows what a feed serves, the Policy
//! decides each read, the surface writes nothing, the protobuf decodes and equals the JSON, and
//! the mirror's field numbers are the reference `tests/gtfsrt/gtfs-realtime.proto`'s.
//!
//! The Tenant `city` holds the buses `fresh` (on route 1055), `noroute` and `stale` (an hour old)
//! and the hidden `Secret` `s`. The view `city` shows `Vehicle` without its `fleetVehicleId`;
//! `secret` shows `Secret` alone; `writes` allows Create Entity alone; `void` reads the empty
//! Tenant `void`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use antares_gtfsrt::feed::{
    FeedEntity, FeedHeader, FeedMessage, Position, TripDescriptor, VehicleDescriptor,
    VehiclePosition,
};
use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::Surface as _;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use prost::Message as _;
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

const PB: &str = "/x/views/city/gtfsrt/v1/vehicle-positions.pb";
const JSON: &str = "/x/views/city/gtfsrt/v1/vehicle-positions.json";

async fn fixture_with(broker: TestBroker) -> Router {
    let app = broker.serve(antares_gtfsrt::GtfsRt).expect("mounted");
    let reads = json!(["query_entities"]);
    let shown = json!(["location", "speed", "heading", "route", "occupancy"]);
    let views = [
        json!({"id": "city", "tenant": "city", "selector": ["Vehicle"], "operations": reads,
               "rules": [{"types": ["Vehicle"], "attributes": shown}]}),
        json!({"id": "secret", "tenant": "city", "selector": ["Secret"], "operations": reads,
               "rules": [{"types": ["Secret"], "attributes": ["name"]}]}),
        json!({"id": "writes", "tenant": "city", "selector": ["Vehicle"], "operations": ["create_entity"],
               "rules": [{"types": ["Vehicle"], "attributes": ["location"]}]}),
        json!({"id": "every", "tenant": "city", "selector": ["*"], "operations": reads,
               "rules": [{"types": ["*"], "attributes": ["*"]}]}),
        json!({"id": "void", "tenant": "void", "selector": ["Vehicle"], "operations": reads,
               "rules": [{"types": ["Vehicle"], "attributes": shown}]}),
    ];
    for view in views {
        let id = view["id"].as_str().expect("id");
        let (status, body) = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "view {id}: {body}");
    }
    let now = chrono::Utc::now();
    let stamp = |age: i64| {
        now.checked_sub_signed(chrono::Duration::seconds(age))
            .expect("a time")
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    let bus = |id: &str, at: String, route: bool| {
        let mut v = json!({"id": format!("urn:ngsi-ld:Vehicle:{id}"), "type": "Vehicle",
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.94, 60.17]}, "observedAt": at},
            "speed": {"type": "Property", "value": 10.5, "unitCode": "MTS", "observedAt": at},
            "fleetVehicleId": {"type": "Property", "value": "hidden-fleet-id"}});
        if route {
            v["route"] = json!({"type": "Property", "value": "1055", "observedAt": at});
        }
        v
    };
    let entities = [
        bus("fresh", stamp(10), true),
        bus("noroute", stamp(20), false),
        bus("stale", stamp(3_600), true),
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

async fn bytes(app: &Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let req = Request::get(uri).body(Body::empty()).expect("request");
    let resp = app.clone().oneshot(req).await.expect("infallible");
    let (status, headers) = (resp.status(), resp.headers().clone());
    let body = resp.into_body().collect().await.expect("body").to_bytes();
    (status, headers, body.to_vec())
}

#[tokio::test]
async fn the_protobuf_decodes_equals_the_json_and_holds_what_the_view_shows() {
    let app = fixture().await;
    let (status, headers, pb) = bytes(&app, PB).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/x-protobuf");
    let message = FeedMessage::decode(pb.as_slice()).expect("a FeedMessage");
    let (status, body) = send(&app, "GET", JSON, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(antares_gtfsrt::feed::json(&message), got);
    assert!(
        !body.contains("hidden-fleet-id"),
        "a hidden Attribute leaked: {body}"
    );
    let ids: Vec<&str> = message.entity.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    let fresh = message
        .entity
        .iter()
        .find(|e| e.id.ends_with(":fresh"))
        .expect("fresh");
    let position = fresh.vehicle.as_ref().expect("vehicle");
    assert_eq!(
        position.trip.as_ref().and_then(|t| t.route_id.as_deref()),
        Some("1055")
    );
    assert_eq!(
        position.vehicle.as_ref().and_then(|v| v.id.as_deref()),
        Some("urn:ngsi-ld:Vehicle:fresh")
    );
    let noroute = message
        .entity
        .iter()
        .find(|e| e.id.ends_with(":noroute"))
        .expect("noroute");
    assert_eq!(
        noroute.vehicle.as_ref().map(|v| v.trip.is_none()),
        Some(true),
        "no route, no trip"
    );
    assert_eq!(
        message.header.timestamp, position.timestamp,
        "the header is the newest position"
    );
    let (_, every) = send(
        &app,
        "GET",
        "/x/views/every/gtfsrt/v1/vehicle-positions.json",
        "",
    )
    .await;
    assert!(
        every.contains("hidden-fleet-id"),
        "a view showing it names the fleet id: {every}"
    );
    let (status, body) = send(&app, "HEAD", PB, "").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
    let info = antares_gtfsrt::GtfsRt.version_info();
    assert_eq!(
        info["endpoint"],
        "/x/views/{view}/gtfsrt/v1/vehicle-positions.{pb|json}"
    );
}

#[tokio::test]
async fn a_position_outside_the_window_is_dropped_and_counted() {
    let app = fixture().await;
    let (_, headers, pb) = bytes(&app, PB).await;
    assert_eq!(headers[antares_gtfsrt::SKIPPED_HEADER], "1");
    let message = FeedMessage::decode(pb.as_slice()).expect("a FeedMessage");
    assert!(message.entity.iter().all(|e| !e.id.ends_with(":stale")));
    let (_, headers, pb) = bytes(&app, &format!("{PB}?maxAge=15")).await;
    assert_eq!(headers[antares_gtfsrt::SKIPPED_HEADER], "2");
    assert_eq!(
        FeedMessage::decode(pb.as_slice())
            .expect("decoded")
            .entity
            .len(),
        1
    );
    let (_, headers, _) = bytes(&app, &format!("{PB}?maxAge=86400")).await;
    assert_eq!(headers[antares_gtfsrt::SKIPPED_HEADER], "0");
}

/// `(message, field, number)` of every field the mirror writes, as `gtfs-realtime.proto` has it.
const FIELDS: [(&str, &str, u32); 17] = [
    ("FeedMessage", "header", 1),
    ("FeedMessage", "entity", 2),
    ("FeedHeader", "gtfs_realtime_version", 1),
    ("FeedHeader", "incrementality", 2),
    ("FeedHeader", "timestamp", 3),
    ("FeedEntity", "id", 1),
    ("FeedEntity", "vehicle", 4),
    ("VehiclePosition", "trip", 1),
    ("VehiclePosition", "position", 2),
    ("VehiclePosition", "timestamp", 5),
    ("VehiclePosition", "vehicle", 8),
    ("VehiclePosition", "occupancy_percentage", 10),
    ("Position", "latitude", 1),
    ("Position", "longitude", 2),
    ("Position", "bearing", 3),
    ("Position", "speed", 5),
    ("TripDescriptor", "route_id", 5),
];

/// The field numbers of the top-level messages of a `.proto` text, by `(message, field)`.
fn numbers(proto: &str) -> std::collections::BTreeMap<(String, String), u32> {
    let mut found = std::collections::BTreeMap::new();
    let (mut stack, mut depth): (Vec<(String, usize)>, usize) = (Vec::new(), 0);
    for line in proto
        .lines()
        .map(|l| l.split("//").next().unwrap_or("").trim())
    {
        let words: Vec<&str> = line.split_whitespace().collect();
        if let ["message", name, ..] = words.as_slice() {
            stack.push(((*name).to_owned(), depth));
        }
        if let ([label, _, field, "=", number, ..], Some((message, at))) =
            (words.as_slice(), stack.last())
        {
            let number = number.trim_end_matches(';');
            if ["optional", "required", "repeated"].contains(label)
                && at.checked_add(1) == Some(depth)
            {
                found.insert(
                    (message.clone(), (*field).to_owned()),
                    number.parse().expect("number"),
                );
            }
        }
        depth = depth.saturating_add(line.matches('{').count());
        depth = depth.saturating_sub(line.matches('}').count());
        while stack
            .last()
            .is_some_and(|(_, at)| *at >= depth && line.contains('}'))
        {
            stack.pop();
        }
    }
    found
}

#[test]
fn the_mirror_carries_the_reference_field_numbers() {
    let proto = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/gtfsrt/gtfs-realtime.proto"
    ))
    .expect("the pinned proto");
    let reference = numbers(&proto);
    for (message, field, number) in FIELDS {
        assert_eq!(
            reference.get(&(message.to_owned(), field.to_owned())),
            Some(&number),
            "{message}.{field}"
        );
    }
    assert_eq!(
        reference.get(&("VehicleDescriptor".to_owned(), "id".to_owned())),
        Some(&1)
    );
    assert_eq!(
        reference.get(&("VehicleDescriptor".to_owned(), "license_plate".to_owned())),
        Some(&3)
    );
    // and the encoder writes exactly those numbers: walk the wire format of a full message

    let full = FeedMessage {
        header: FeedHeader {
            gtfs_realtime_version: "2.0".into(),
            incrementality: Some(0),
            timestamp: Some(1),
        },
        entity: vec![FeedEntity {
            id: "a".into(),
            vehicle: Some(VehiclePosition {
                trip: Some(TripDescriptor {
                    route_id: Some("r".into()),
                }),
                position: Some(Position {
                    latitude: 1.0,
                    longitude: 2.0,
                    bearing: Some(3.0),
                    speed: Some(4.0),
                }),
                timestamp: Some(5),
                vehicle: Some(VehicleDescriptor {
                    id: Some("v".into()),
                    license_plate: Some("p".into()),
                }),
                occupancy_percentage: Some(6),
            }),
        }],
    };
    let mut seen = Vec::new();
    walk(&full.encode_to_vec(), "FeedMessage", &mut seen);
    let mut expected: Vec<(String, u32)> = FIELDS
        .iter()
        .map(|(m, _, n)| ((*m).to_owned(), *n))
        .collect();
    expected.extend([
        ("VehicleDescriptor".to_owned(), 1),
        ("VehicleDescriptor".to_owned(), 3),
    ]);
    seen.sort();
    expected.sort();
    assert_eq!(seen, expected);
}

/// Each `(message, field number)` the bytes of `message` hold, nested messages walked.
fn walk(mut bytes: &[u8], message: &str, seen: &mut Vec<(String, u32)>) {
    let varint = |b: &mut &[u8]| -> u64 {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let byte = b[0];
            *b = &b[1..];
            v |= u64::from(byte & 0x7f).checked_shl(shift).expect("a varint");
            shift = shift.saturating_add(7);
            if byte & 0x80 == 0 {
                return v;
            }
        }
    };
    while !bytes.is_empty() {
        let key = varint(&mut bytes);
        let (field, wire) = (u32::try_from(key >> 3).expect("field"), key & 7);
        seen.push((message.to_owned(), field));
        match wire {
            0 => {
                varint(&mut bytes);
            }
            5 => bytes = &bytes[4..],
            2 => {
                let len = usize::try_from(varint(&mut bytes)).expect("len");
                let (inner, rest) = bytes.split_at(len);
                let nested = match (message, field) {
                    ("FeedMessage", 1) => Some("FeedHeader"),
                    ("FeedMessage", 2) => Some("FeedEntity"),
                    ("FeedEntity", 4) => Some("VehiclePosition"),
                    ("VehiclePosition", 1) => Some("TripDescriptor"),
                    ("VehiclePosition", 2) => Some("Position"),
                    ("VehiclePosition", 8) => Some("VehicleDescriptor"),
                    _ => None,
                };
                if let Some(n) = nested {
                    walk(inner, n, seen);
                }
                bytes = rest;
            }
            other => panic!("wire type {other} in {message}.{field}"),
        }
    }
}

#[tokio::test]
async fn a_hidden_type_an_unknown_view_or_file_and_a_view_without_reads_are_404() {
    let app = fixture().await;
    for uri in [
        "/x/views/secret/gtfsrt/v1/vehicle-positions.pb",
        "/x/views/nowhere/gtfsrt/v1/vehicle-positions.pb",
        "/x/views/writes/gtfsrt/v1/vehicle-positions.pb",
        "/x/views/city/gtfsrt/v1/trip-updates.pb",
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
    let (status, body) = send(&app, "GET", JSON, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        !body.contains("urn:ngsi-ld:Vehicle"),
        "a denied read served data: {body}"
    );
}

#[tokio::test]
async fn the_surface_writes_nothing() {
    let app = fixture().await;
    let (status, _) = send(
        &app,
        "POST",
        PB,
        r#"{"id":"urn:ngsi-ld:Vehicle:c","type":"Vehicle"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = send(&app, "OPTIONS", PB, "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:c",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_parameters_are_400_and_an_empty_tenant_an_empty_feed() {
    let app = fixture().await;
    for query in ["maxAge=0", "maxAge=x", "maxAge=1&maxAge=2", "limit=1"] {
        let (status, body) = send(&app, "GET", &format!("{PB}?{query}"), "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    let (status, body) = send(
        &app,
        "GET",
        "/x/views/void/gtfsrt/v1/vehicle-positions.json",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(got["entity"], json!([]));
}

#[tokio::test]
async fn a_request_naming_another_tenant_is_refused() {
    let app = fixture().await;
    let req = Request::get(PB)
        .header("NGSILD-Tenant", "void")
        .body(Body::empty())
        .expect("request");
    let resp = app.oneshot(req).await.expect("infallible");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
