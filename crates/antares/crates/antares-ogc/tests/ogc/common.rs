// SPDX-License-Identifier: EUPL-1.2
//! The fixture every `antares-ogc` test reads: a broker with the surface mounted, the Tenant
//! `city`'s `LinkML` schema, four views and a handful of Cars and Stops.
//!
//! - `city` shows `Car` (`speed` in km/h, `location`, `seen`, `at` a Stop, `home` a Depot,
//!   `colour`, `tags`) and `Stop` (`name`); `Car.secret` and the class `Depot` are in the
//!   schema and hidden.
//! - `slow` shows the same, narrowed to `speed<50` by the view.
//! - `empty` shows `Stop` only.
//! - `fleet` shows the same as `city` and offers every write Part 4 maps.
//! - `reads` shows `Car` and allows Retrieve Entity alone, which OGC reads never use.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::{q_filter, TestBroker};
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

pub const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"dc": {"prefix_prefix": "dc", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"},
               "ngsi": {"prefix_prefix": "ngsi", "prefix_reference": "https://uri.etsi.org/ngsi-ld/"}},
  "default_prefix": "dc",
  "classes": {
    "Car": {"slots": ["speed", "location", "seen", "secret", "at", "home", "colour", "tags"], "description": "A car of the fleet"},
    "Stop": {"slots": ["name"]},
    "Depot": {"slots": ["name"]}
  },
  "slots": {
    "speed": {"range": "float", "unit": {"ucum_code": "km/h"}, "description": "how fast"},
    "at": {"range": "Stop"},
    "home": {"range": "Depot"},
    "colour": {"range": "Colour"},
    "tags": {"range": "string", "multivalued": true},
    "location": {"slot_uri": "ngsi:location", "range": "string",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
    "seen": {"range": "string", "annotations": {"ogc_role": {"tag": "ogc_role", "value": "primary-instant"}}},
    "secret": {"range": "string"},
    "name": {"range": "string"}
  },
  "enums": {"Colour": {"permissible_values": {"red": {}, "blue": {}}}}
}"#;

pub struct Fixture {
    /// Kept for the fixture's lifetime.
    pub _broker: TestBroker,
    pub app: Router,
}

pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
    pub bytes: usize,
}

pub async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    if !body.is_empty() {
        req = req.header("Content-Length", body.len());
    }
    if !body.is_empty()
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        req = req.header("Content-Type", "application/json");
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap_or_default())
        .await;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    Answer {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        bytes: bytes.len(),
    }
}

pub async fn get(app: &Router, uri: &str) -> Answer {
    send(app, "GET", uri, "", &[]).await
}

/// The fixture broker, every Car and Stop stored in the Tenant `city`.
pub async fn fixture() -> Result<Fixture, String> {
    schemas_on();
    fixture_with(TestBroker::new().await?).await
}

/// The operations of the `fleet` view: the reads and the four writes of Part 4.
pub const WRITES: [&str; 5] = [
    "query_entities",
    "create_entity",
    "replace_entity",
    "merge_entity",
    "delete_entity",
];

/// The broker takes schemas only when their bound is set, read when it starts.
pub fn schemas_on() {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
}

/// [`fixture`] over a given broker (one with a policy engine).
pub async fn fixture_with(broker: TestBroker) -> Result<Fixture, String> {
    let app = broker.serve(antares_ogc::Ogc)?;
    let views = [
        ("city", json!(["Car", "Stop"]), None, "query_entities"),
        (
            "slow",
            json!(["Car", "Stop"]),
            Some("speed<50"),
            "query_entities",
        ),
        ("empty", json!(["Stop"]), None, "query_entities"),
        ("reads", json!(["Car"]), None, "retrieve_entity"),
    ];
    let views = views
        .into_iter()
        .map(|(id, selector, q, op)| (id, selector, q, json!([op])))
        .chain([("fleet", json!(["Car", "Stop"]), None, json!(WRITES))]);
    for (id, selector, q, ops) in views {
        let mut view = json!({
            "id": id, "tenant": "city", "selector": selector,
            "rules": [{"types": ["Car"], "attributes": ["speed", "location", "seen", "at", "home", "colour", "tags"]},
                      {"types": ["Stop"], "attributes": ["name"]}],
            "operations": ops
        });
        if let (Some(q), Some(map)) = (q, view.as_object_mut()) {
            map.insert("q".into(), json!(q));
        }
        let a = send(
            &app,
            "PUT",
            &format!("/q/views/{id}"),
            &view.to_string(),
            &[],
        )
        .await;
        if a.status != StatusCode::CREATED {
            return Err(format!("view {id}: {} {}", a.status, a.body));
        }
    }
    let a = send(&app, "PUT", "/q/schemas/city", SCHEMA, &[]).await;
    if a.status != StatusCode::CREATED {
        return Err(format!("schema: {} {}", a.status, a.body));
    }
    for entity in entities() {
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
    Ok(Fixture {
        _broker: broker,
        app,
    })
}

fn point(lon: f64, lat: f64) -> Value {
    json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [lon, lat]}})
}

fn rel(object: &str) -> Value {
    json!({"type": "Relationship", "object": object})
}

fn prop(v: &Value) -> Value {
    json!({"type": "Property", "value": v})
}

/// Car 1 (slow, in Bratislava, seen in January), Car 2 (fast, east of the antimeridian
/// line at 179.5, seen in June, at Stop 1, home Depot 1, red, tagged), Car 3 (slow, no
/// location, never seen), Car 4 (fast, west of it at -179.5), Stop 1, Depot 1.
pub fn entities() -> Vec<Value> {
    vec![
        json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": prop(&json!(10)), "location": point(17.1, 48.1), "seen": prop(&json!("2026-01-01T00:00:00Z")), "secret": prop(&json!("s1"))}),
        json!({"id": "urn:ngsi-ld:Car:2", "type": "Car", "speed": prop(&json!(60)), "location": point(179.5, 0.0), "seen": prop(&json!("2026-06-01T00:00:00Z")), "secret": prop(&json!("s2")),
               "at": rel("urn:ngsi-ld:Stop:1"), "home": rel("urn:ngsi-ld:Depot:1"), "colour": prop(&json!("red")), "tags": prop(&json!(["a", "b"]))}),
        json!({"id": "urn:ngsi-ld:Car:3", "type": "Car", "speed": prop(&json!(30)), "secret": prop(&json!("s3"))}),
        json!({"id": "urn:ngsi-ld:Car:4", "type": "Car", "speed": prop(&json!(70)), "location": point(-179.5, 0.0)}),
        json!({"id": "urn:ngsi-ld:Stop:1", "type": "Stop", "name": prop(&json!("Hlavná"))}),
        json!({"id": "urn:ngsi-ld:Depot:1", "type": "Depot", "name": prop(&json!("North"))}),
    ]
}

/// The ids of a `FeatureCollection`, in order.
pub fn ids(body: &Value) -> Vec<String> {
    body.get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// The `href` of the link with `rel`.
pub fn href(body: &Value, rel: &str) -> Option<String> {
    body.get("links")?
        .as_array()?
        .iter()
        .find(|l| l.get("rel").and_then(Value::as_str) == Some(rel))?
        .get("href")?
        .as_str()
        .map(str::to_owned)
}

/// A link's `href` as a path and query the test router takes.
pub fn local(href: &str) -> String {
    href.trim_start_matches("http://localhost:9090").to_owned()
}

/// An engine that narrows every read to `speed<50`, as a Policy would.
pub struct SlowOnly(pub antares_ops::policy::Filter);

impl PolicyEngine for SlowOnly {
    fn name(&self) -> &'static str {
        "slow-only"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(if op.clause.starts_with("5.7.") {
            Decision::Filter(self.0.clone())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

pub async fn narrowed() -> Result<Fixture, String> {
    schemas_on();
    fixture_with(
        TestBroker::new()
            .await?
            .with_policy(Arc::new(SlowOnly(q_filter("speed<50")?))),
    )
    .await
}

/// An engine that, once locked, refuses every write (clause 5.6) and allows every read, as a
/// read-only Policy would; unlocked while the fixture seeds.
#[derive(Default)]
pub struct ReadOnly(pub std::sync::atomic::AtomicBool);

impl PolicyEngine for ReadOnly {
    fn name(&self) -> &'static str {
        "read-only"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let locked = self.0.load(std::sync::atomic::Ordering::SeqCst);
        Box::pin(std::future::ready(
            if locked && op.clause.starts_with("5.6.") {
                Decision::Deny("read only".into())
            } else {
                Decision::Allow
            },
        ))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// [`fixture`] under a [`ReadOnly`] engine, locked once seeded.
pub async fn read_only() -> Result<Fixture, String> {
    schemas_on();
    let engine = Arc::new(ReadOnly::default());
    let f = fixture_with(TestBroker::new().await?.with_policy(engine.clone())).await?;
    engine.0.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(f)
}
