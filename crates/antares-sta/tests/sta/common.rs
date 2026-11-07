// SPDX-License-Identifier: EUPL-1.2
//! The fixture every `antares-sta` test reads: a broker with the surface mounted and the Tenant
//! `city`.
//!
//! - `city` shows `Car` (`speed`, `NO2`, `location`, `name`, `refDevice`, `refPointOfInterest`),
//!   `Stop` (`name`, `location`), `Device` (`name`, `controlledProperty`) and `Vehicle`
//!   (`location`); `Car.secret` (a number) and the types `Radar` and `Depot` are stored and
//!   hidden.
//! - `every` shows every type and Attribute.
//! - `writes` allows Create Entity alone, so every read is refused.
//! - `void` shows `Car` of the Tenant `void`, which holds nothing.
//!
//! `Car:1` measures `speed` (a history of three) and `NO2` (a default instance and one of
//! dataset `a`), points at `Device:1`; `Car:2` points at the hidden `Radar:1` and at `Stop:1`;
//! `Device:1` at the hidden `Depot:1` as its point of interest;
//! `Vehicle:1` has a history of three locations.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

pub struct Fixture {
    /// Kept for the fixture's lifetime.
    pub _broker: TestBroker,
    pub app: Router,
}

pub struct Answer {
    pub status: StatusCode,
    pub body: Value,
    pub allow: Option<String>,
    pub bytes: usize,
    /// The body as text, for an answer that is no JSON (`$value`).
    pub text: String,
    pub content_type: Option<String>,
}

pub async fn send(app: &Router, method: &str, uri: &str, body: &str) -> Answer {
    // A test writes a space and a letter outside ASCII as it reads; the request line takes neither.
    let uri: String = uri
        .bytes()
        .map(|b| match b {
            b' ' => "%20".to_owned(),
            b if b.is_ascii() => char::from(b).to_string(),
            b => format!("%{b:02X}"),
        })
        .collect();
    // The writes name the Tenant; a view serves its own and refuses a request naming another.
    let tenant = uri.starts_with("/ngsi-ld");
    let mut req = Request::builder().method(method).uri(uri);
    if tenant {
        req = req.header("NGSILD-Tenant", "city");
    }
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap_or_default())
        .await;
    let status = resp.status();
    let allow = resp
        .headers()
        .get("allow")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    Answer {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        allow,
        bytes: bytes.len(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
        content_type,
    }
}

/// A GET of `path` below the `city` view's STA root.
pub async fn sta(app: &Router, path: &str) -> Answer {
    send(app, "GET", &format!("/x/views/city/sta/v1.1{path}"), "").await
}

/// The ids of a collection's `value`, in order.
pub fn ids(body: &Value) -> Vec<String> {
    body.get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.get("@iot.id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// A member of a document by JSON pointer.
pub fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or_default()
}

/// A link as the path and query the test router takes.
pub fn local(link: &str) -> String {
    link.trim_start_matches("http://localhost:9090").to_owned()
}

fn prop(v: &Value) -> Value {
    json!({"type": "Property", "value": v})
}

fn rel(object: &str) -> Value {
    json!({"type": "Relationship", "object": object})
}

fn point(lon: f64, lat: f64) -> Value {
    json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [lon, lat]}})
}

pub const T1: &str = "2026-01-01T00:00:00Z";

/// [`T1`] as an id and a time carry it (`docs/src/sta.md`, "Resource ids").
pub const T1_ID: &str = "2026-01-01T00:00:00.000Z";

pub fn entities() -> Vec<Value> {
    vec![
        json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "name": prop(&json!("Žltá")),
               "speed": {"type": "Property", "value": 10, "unitCode": "KMH", "observedAt": T1},
               "NO2": [{"type": "Property", "value": 15.0},
                       {"type": "Property", "value": 16.0, "datasetId": "urn:ngsi-ld:Dataset:a"}],
               "location": point(17.1, 48.1), "refDevice": rel("urn:ngsi-ld:Device:1"),
               "secret": prop(&json!(99))}),
        json!({"id": "urn:ngsi-ld:Car:2", "type": "Car", "speed": prop(&json!(60)),
               "refDevice": rel("urn:ngsi-ld:Radar:1"), "refPointOfInterest": rel("urn:ngsi-ld:Stop:1")}),
        json!({"id": "urn:ngsi-ld:Stop:1", "type": "Stop", "name": prop(&json!("Hlavná")), "location": point(17.2, 48.2)}),
        json!({"id": "urn:ngsi-ld:Device:1", "type": "Device", "name": prop(&json!("radar 1")),
               "controlledProperty": prop(&json!(["speed"])), "refPointOfInterest": rel("urn:ngsi-ld:Depot:1")}),
        json!({"id": "urn:ngsi-ld:Radar:1", "type": "Radar", "name": prop(&json!("hidden"))}),
        json!({"id": "urn:ngsi-ld:Depot:1", "type": "Depot", "capacity": prop(&json!(5))}),
        json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "location": point(17.0, 48.0)}),
    ]
}

/// The histories: two earlier speeds of `Car:1` (its creation is the third), three locations of
/// `Vehicle:1`.
pub fn histories() -> Vec<Value> {
    let speed = |v: i64, at: &str| json!({"type": "Property", "value": v, "observedAt": at});
    let moved = |lon: f64, at: &str| json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [lon, 48.0]}, "observedAt": at});
    vec![
        json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": [
            speed(8, "2025-12-30T00:00:00Z"), speed(9, "2025-12-31T00:00:00Z")]}),
        json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "location": [
            moved(16.8, "2025-12-30T00:00:00Z"), moved(16.9, "2025-12-31T00:00:00Z"), moved(17.0, T1)]}),
    ]
}

/// The fixture broker with every Entity and history stored in the Tenant `city`.
pub async fn fixture() -> Result<Fixture, String> {
    fixture_with(TestBroker::new().await?, &entities(), &histories()).await
}

/// [`fixture`] over a given broker, with the given Entities and histories.
pub async fn fixture_with(
    broker: TestBroker,
    entities: &[Value],
    histories: &[Value],
) -> Result<Fixture, String> {
    let app = broker.serve(antares_sta::Sta)?;
    let reads = json!(["query_entities", "temporal_retrieve", "temporal_query"]);
    let rules = json!([
        {"types": ["Car"], "attributes": ["speed", "NO2", "location", "name", "refDevice", "refPointOfInterest"]},
        {"types": ["Stop"], "attributes": ["name", "location"]},
        {"types": ["Device"], "attributes": ["name", "controlledProperty"]},
        {"types": ["Vehicle"], "attributes": ["location"]}
    ]);
    let views = [
        json!({"id": "city", "tenant": "city", "selector": ["Car", "Stop", "Device", "Vehicle"], "rules": rules, "operations": reads}),
        json!({"id": "every", "tenant": "city", "selector": ["*"], "rules": [{"types": ["*"], "attributes": ["*"]}], "operations": reads}),
        json!({"id": "writes", "tenant": "city", "selector": ["Car"], "rules": rules, "operations": ["create_entity"]}),
        json!({"id": "void", "tenant": "void", "selector": ["Car"], "rules": [{"types": ["Car"], "attributes": ["location"]}], "operations": reads}),
    ];
    for view in views {
        let id = view
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let a = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        if a.status != StatusCode::CREATED {
            return Err(format!("view {id}: {} {}", a.status, a.body));
        }
    }
    for entity in entities {
        let a = send(&app, "POST", "/ngsi-ld/v1/entities", &entity.to_string()).await;
        if a.status != StatusCode::CREATED {
            return Err(format!("{entity}: {} {}", a.status, a.body));
        }
    }
    for history in histories {
        let a = send(
            &app,
            "POST",
            "/ngsi-ld/v1/temporal/entities",
            &history.to_string(),
        )
        .await;
        if !a.status.is_success() {
            return Err(format!("{history}: {} {}", a.status, a.body));
        }
    }
    Ok(Fixture {
        _broker: broker,
        app,
    })
}
