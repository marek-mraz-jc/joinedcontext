// SPDX-License-Identifier: EUPL-1.2
//! One broker with the view root mounted, the Entities every test reads, and
//! the views over them.
//!
//! Tenant `city` holds three Cars, a Person and an Organization:
//!
//! | id | speed | shown through `cars` |
//! |---|---|---|
//! | `Car:1` | 10 | `speed`, `colour`, `owner` → `Person:p`, `driver` → `Org:o` |
//! | `Car:2` | 90 then 50 | `speed`, `colour` |
//! | `Car:3` | 50 then 90 | out of view: no rule holds (`speed<80`) |
//! | `Person:p` | | `name`, `employer` → `Org:o`; never `salary` |
//! | `Org:o` | | out of view: not in the selector |
//!
//! Every Car has a `secret` no rule shows. Tenant `other` holds `Car:9`.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The root of view `cars`.
pub const CARS: &str = "/x/views/cars/ngsi-ld/v1";
/// The root of view `history`.
pub const HISTORY: &str = "/x/views/history/ngsi-ld/v1";
/// The root of view `fleet`.
pub const FLEET: &str = "/x/views/fleet/ngsi-ld/v1";

/// Every write the view root serves, by its matrix id.
pub const WRITES: [&str; 15] = [
    "batch_create",
    "batch_upsert",
    "batch_update",
    "batch_merge",
    "batch_delete",
    "create_entity",
    "update_attributes",
    "append_attributes",
    "partial_attribute_update",
    "set_attribute_value",
    "delete_attribute",
    "delete_entity",
    "merge_entity",
    "replace_entity",
    "replace_attribute",
];

/// Every Subscription operation the view root serves, by its matrix id.
pub const SUBSCRIPTIONS: [&str; 5] = [
    "create_subscription",
    "update_subscription",
    "retrieve_subscription",
    "query_subscriptions",
    "delete_subscription",
];

/// Every read the view root serves, by its matrix id.
pub const READS: [&str; 14] = [
    "retrieve_entity",
    "query_entities",
    "query_entities_post",
    "retrieve_attribute_value",
    "retrieve_entity_types",
    "retrieve_entity_type_details",
    "retrieve_entity_type_info",
    "retrieve_attributes",
    "retrieve_attribute_details",
    "retrieve_attribute_info",
    "temporal_retrieve",
    "temporal_query",
    "temporal_query_post",
    "list_contexts",
];

/// A broker and its router: the NGSI-LD API, `/q` and the view root.
pub struct Fixture {
    /// The broker, kept alive: its reload loop serves `/q/views`.
    pub _broker: TestBroker,
    /// Its router.
    pub app: Router,
}

/// One answer.
pub struct Answer {
    /// Its status.
    pub status: StatusCode,
    /// Its headers.
    pub headers: HeaderMap,
    /// Its body as text.
    pub text: String,
    /// Its body as JSON, `Null` when it is none.
    pub json: Value,
}

impl Answer {
    /// The ids of a list answer, in its order.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.json
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    }

    /// The target of the `Link` of relation `rel`.
    #[must_use]
    pub fn link(&self, rel: &str) -> Option<String> {
        let want = format!("rel=\"{rel}\"");
        self.headers
            .get_all("link")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.contains(&want))
            .and_then(|v| v.split_once('<'))
            .and_then(|(_, rest)| rest.split_once('>'))
            .map(|(uri, _)| uri.to_owned())
    }

    /// The value of `name`, when it has exactly one.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<String> {
        let mut all = self.headers.get_all(name).iter();
        match (all.next(), all.next()) {
            (Some(v), None) => v.to_str().ok().map(str::to_owned),
            _ => None,
        }
    }
}

/// `method uri` with `headers` and `body` (as JSON).
pub async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> Answer {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
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
        text: String::from_utf8_lossy(&bytes).into_owned(),
        json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// `GET uri` with no header.
pub async fn get(app: &Router, uri: &str) -> Answer {
    send(app, Method::GET, uri, &[], None).await
}

/// A Property of value `v`.
#[must_use]
pub fn prop(v: &Value) -> Value {
    json!({"type": "Property", "value": v})
}

/// A Relationship to `target`.
#[must_use]
pub fn rel(target: &str) -> Value {
    json!({"type": "Relationship", "object": target})
}

async fn expect(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: &str,
    body: &Value,
    want: StatusCode,
) -> Result<(), String> {
    let a = send(app, method, uri, &[("NGSILD-Tenant", tenant)], Some(body)).await;
    if a.status == want {
        Ok(())
    } else {
        Err(format!("{uri}: {} {}", a.status, a.text))
    }
}

async fn create(app: &Router, tenant: &str, doc: &Value) -> Result<(), String> {
    expect(
        app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        tenant,
        doc,
        StatusCode::CREATED,
    )
    .await
}

async fn speed(app: &Router, id: &str, v: u32) -> Result<(), String> {
    let uri = format!("/ngsi-ld/v1/entities/{id}/attrs");
    let body = json!({"speed": prop(&json!(v))});
    expect(
        app,
        Method::PATCH,
        &uri,
        "city",
        &body,
        StatusCode::NO_CONTENT,
    )
    .await
}

/// The view `cars`: Cars below 80 show `speed`, `colour`, `owner` and
/// `driver`; Persons show `name` and `employer`; every read is allowed.
#[must_use]
pub fn cars_view() -> Value {
    json!({
        "id": "cars", "tenant": "city", "selector": ["Car", "Person"],
        "rules": [
            {"types": ["Car"], "q": "speed<80", "attributes": ["speed", "colour", "owner", "driver"]},
            {"types": ["Person"], "attributes": ["name", "employer"]}
        ],
        "operations": READS
    })
}

/// The view `history`: Cars whose speed is below 80 now, showing `speed`
/// and `colour` (ADR-0023 rule 8, membership by current state).
#[must_use]
pub fn history_view() -> Value {
    json!({
        "id": "history", "tenant": "city", "selector": ["Car"], "q": "speed<80",
        "rules": [{"types": ["Car"], "attributes": ["speed", "colour"]}],
        "operations": ["temporal_retrieve", "temporal_query", "query_entities"]
    })
}

/// The view `fleet`: `cars`' rules, every read, write and Subscription
/// operation it serves.
#[must_use]
pub fn fleet_view() -> Value {
    let mut v = cars_view();
    if let Some(o) = v.as_object_mut() {
        o.insert("id".into(), json!("fleet"));
        o.insert(
            "operations".into(),
            json!(READS
                .iter()
                .chain(&WRITES)
                .chain(&SUBSCRIPTIONS)
                .collect::<Vec<_>>()),
        );
    }
    v
}

/// The Entity `id` as tenant `city` stores it, through the NGSI-LD root.
pub async fn stored(app: &Router, id: &str) -> Answer {
    send(
        app,
        Method::GET,
        &format!("/ngsi-ld/v1/entities/{id}"),
        &[("NGSILD-Tenant", "city")],
        None,
    )
    .await
}

/// Load `view` through `/q/views`.
///
/// # Errors
/// When `/q/views` does not store it.
pub async fn put_view(app: &Router, view: &Value) -> Result<(), String> {
    let id = view.get("id").and_then(Value::as_str).unwrap_or_default();
    let a = send(app, Method::PUT, &format!("/q/views/{id}"), &[], Some(view)).await;
    if a.status == StatusCode::CREATED || a.status == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err(format!("view {id}: {} {}", a.status, a.text))
    }
}

/// A broker with the view root, the Entities of this module, and the views
/// `cars` and `history` loaded.
///
/// # Errors
/// The broker's, or a write that did not land.
pub async fn fixture() -> Result<Fixture, String> {
    fixture_with(TestBroker::new().await?).await
}

/// [`fixture`] on `broker`, which may carry a policy engine.
///
/// # Errors
/// The broker's, or a write that did not land.
pub async fn fixture_with(broker: TestBroker) -> Result<Fixture, String> {
    let app = broker.serve(antares_views::Views)?;
    seed(&app).await?;
    put_view(&app, &cars_view()).await?;
    put_view(&app, &history_view()).await?;
    put_view(&app, &fleet_view()).await?;
    Ok(Fixture {
        _broker: broker,
        app,
    })
}

async fn seed(app: &Router) -> Result<(), String> {
    create(
        app,
        "city",
        &json!({
            "id": "urn:ngsi-ld:Car:1", "type": "Car",
            "speed": prop(&json!(10)), "colour": prop(&json!("red")), "secret": prop(&json!(1)),
            "owner": rel("urn:ngsi-ld:Person:p"), "driver": rel("urn:ngsi-ld:Org:o")
        }),
    )
    .await?;
    create(
        app,
        "city",
        &json!({
            "id": "urn:ngsi-ld:Car:2", "type": "Car",
            "speed": prop(&json!(90)), "colour": prop(&json!("blue")), "secret": prop(&json!(5))
        }),
    )
    .await?;
    create(
        app,
        "city",
        &json!({
            "id": "urn:ngsi-ld:Car:3", "type": "Car",
            "speed": prop(&json!(50)), "secret": prop(&json!(9))
        }),
    )
    .await?;
    speed(app, "urn:ngsi-ld:Car:2", 50).await?;
    speed(app, "urn:ngsi-ld:Car:3", 90).await?;
    create(
        app,
        "city",
        &json!({
            "id": "urn:ngsi-ld:Person:p", "type": "Person",
            "name": prop(&json!("Pat")), "salary": prop(&json!(987_654.5)),
            "employer": rel("urn:ngsi-ld:Org:o")
        }),
    )
    .await?;
    create(
        app,
        "city",
        &json!({
            "id": "urn:ngsi-ld:Org:o", "type": "Org", "name": prop(&json!("Acme"))
        }),
    )
    .await?;
    create(
        app,
        "other",
        &json!({
            "id": "urn:ngsi-ld:Car:9", "type": "Car", "speed": prop(&json!(1))
        }),
    )
    .await
}
