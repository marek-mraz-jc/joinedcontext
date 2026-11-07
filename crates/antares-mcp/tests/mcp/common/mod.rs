// SPDX-License-Identifier: EUPL-1.2
//! One broker with the MCP surface mounted, the Entities every test reads,
//! and the views over them.
//!
//! Tenant `city` holds `Car:1` (speed 10, `note` → `Car:3`), `Car:2`
//! (speed 50, `note` → `Car:1`) and `Car:3` (speed 90, out of view `cars`:
//! no rule holds `speed<80`), each with a `secret` no rule shows, and
//! `Bike:b` of a type no rule shows. Tenant `other` holds `Car:9`. Tenant
//! `big` holds [`BIG`] Cars whose `note` is 10 000 characters and
//! `Car:huge`, whose `note` alone is past the result budget.
//!
//! | view | tenant | shows | operations |
//! |---|---|---|---|
//! | `cars` | `city` | Cars below 80: `speed`, `colour`, `note` | every read |
//! | `few` | `city` | the same | `query_entities` only |
//! | `elsewhere` | `other` | Cars: `speed` | every read |
//! | `big` | `big` | Cars: `speed`, `note` | every read |
//! | `watch` | `city` | the same as `cars` | reading an Entity, making and deleting a Subscription |
//! | `writes`, `writes2` | `w` | Cars: `speed`, `colour` | every read and [`WRITES`] |
//!
//! Tenant `w` holds `Car:w1` (speed 1). A subscription made through a tool
//! may notify below [`HOOKS`] only.

use antares_ops::policy::PolicyEngine;
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// The protocol revision a modern client speaks.
pub const MODERN: &str = "2026-07-28";

/// Every read operation of the view root, by its matrix id.
pub const READS: [&str; 13] = [
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
];

/// Every write operation a tool offers, by its matrix id.
pub const WRITES: [&str; 17] = [
    "create_entity",
    "update_attributes",
    "append_attributes",
    "partial_attribute_update",
    "delete_attribute",
    "delete_entity",
    "merge_entity",
    "replace_entity",
    "replace_attribute",
    "batch_create",
    "batch_upsert",
    "batch_update",
    "batch_merge",
    "batch_delete",
    "create_subscription",
    "update_subscription",
    "delete_subscription",
];

/// The endpoint prefix the fixture lets a subscription notify.
pub const HOOKS: &str = "https://hooks.example/";

/// How many 10 000-character Cars tenant `big` holds.
pub const BIG: usize = 12;

/// A broker and its router.
pub struct Fixture {
    /// The broker, kept alive: its reload loop serves `/q/views`.
    pub _broker: TestBroker,
    /// Its router.
    pub app: Router,
}

/// One HTTP answer.
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
    /// The JSON-RPC `result`.
    #[must_use]
    pub fn result(&self) -> &Value {
        self.json.get("result").unwrap_or(&Value::Null)
    }

    /// The JSON-RPC `error`.
    #[must_use]
    pub fn error(&self) -> &Value {
        self.json.get("error").unwrap_or(&Value::Null)
    }

    /// The `structuredContent` of a tool result.
    #[must_use]
    pub fn structured(&self) -> &Value {
        self.result()
            .get("structuredContent")
            .unwrap_or(&Value::Null)
    }

    /// Whether the tool result is an error.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.result().get("isError") == Some(&Value::Bool(true))
    }
}

/// `body` posted to `uri` with `headers`, as raw text.
pub async fn post_raw(app: &Router, uri: &str, headers: &[(&str, &str)], body: &str) -> Answer {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Length", body.len());
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
        text: String::from_utf8_lossy(&bytes).into_owned(),
        json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// The MCP endpoint of `view`.
#[must_use]
pub fn endpoint(view: &str) -> String {
    format!("/x/views/{view}/mcp")
}

/// `params` with the per-request metadata of a 2026-07-28 client.
fn modern(params: Value) -> Value {
    modern_with(params, &json!({}))
}

/// `params` with the metadata of a 2026-07-28 client of `capabilities`.
fn modern_with(mut params: Value, capabilities: &Value) -> Value {
    if let Some(o) = params.as_object_mut() {
        o.insert(
            "_meta".into(),
            json!({
                "io.modelcontextprotocol/protocolVersion": MODERN,
                "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": capabilities
            }),
        );
    }
    params
}

/// A 2026-07-28 request `method` with `params` to `view`, with `extra`
/// headers.
pub async fn rpc_with(
    app: &Router,
    view: &str,
    method: &str,
    params: Value,
    extra: &[(&str, &str)],
) -> Answer {
    send_rpc(app, view, method, modern(params), extra).await
}

/// `tools/call` of `tool` with `arguments` on `view` by a client that can
/// ask the person, with `more` params (`requestState`, `inputResponses`).
pub async fn confirmable(
    app: &Router,
    view: &str,
    tool: &str,
    arguments: Value,
    more: Value,
) -> Answer {
    let mut params = json!({"name": tool, "arguments": arguments});
    if let (Some(p), Value::Object(more)) = (params.as_object_mut(), more) {
        p.extend(more);
    }
    let capabilities = json!({"elicitation": {"form": {}}});
    call_as(app, view, params, &capabilities).await
}

/// `tools/call` with `params` on `view` by a client of `capabilities`.
pub async fn call_as(app: &Router, view: &str, params: Value, capabilities: &Value) -> Answer {
    send_rpc(
        app,
        view,
        "tools/call",
        modern_with(params, capabilities),
        &[],
    )
    .await
}

/// The answer the person gives: accept with `confirm` set to `yes`.
#[must_use]
pub fn answered(state: &str, yes: bool) -> Value {
    json!({
        "requestState": state,
        "inputResponses": {"confirm": {"action": "accept", "content": {"confirm": yes}}}
    })
}

async fn send_rpc(
    app: &Router,
    view: &str,
    method: &str,
    params: Value,
    extra: &[(&str, &str)],
) -> Answer {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let name = body
        .pointer("/params/name")
        .or_else(|| body.pointer("/params/uri"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut headers = vec![("MCP-Protocol-Version", MODERN), ("Mcp-Method", method)];
    if let Some(n) = name.as_deref() {
        headers.push(("Mcp-Name", n));
    }
    headers.extend_from_slice(extra);
    post_raw(app, &endpoint(view), &headers, &body.to_string()).await
}

/// A 2026-07-28 request `method` with `params` to `view`.
pub async fn rpc(app: &Router, view: &str, method: &str, params: Value) -> Answer {
    rpc_with(app, view, method, params, &[]).await
}

/// `tools/call` of `tool` with `arguments` on `view`.
pub async fn call(app: &Router, view: &str, tool: &str, arguments: Value) -> Answer {
    rpc(
        app,
        view,
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
    )
    .await
}

/// The names of the tools `view` lists.
pub async fn tool_names(app: &Router, view: &str) -> Vec<String> {
    let a = rpc(app, view, "tools/list", json!({})).await;
    a.result()
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// A Property of value `v`.
#[must_use]
pub fn prop(v: &Value) -> Value {
    json!({"type": "Property", "value": v})
}

/// `body` sent to `uri` as `tenant`; the status.
pub async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: &str,
    body: &Value,
) -> StatusCode {
    let bytes = body.to_string();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Content-Length", bytes.len());
    if !tenant.is_empty() {
        req = req.header("NGSILD-Tenant", tenant);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    resp.status()
}

/// Create `doc` in `tenant`.
///
/// # Errors
/// When the broker does not create it.
pub async fn create(app: &Router, tenant: &str, doc: &Value) -> Result<(), String> {
    match send(app, Method::POST, "/ngsi-ld/v1/entities", tenant, doc).await {
        StatusCode::CREATED => Ok(()),
        s => Err(format!("create {doc}: {s}")),
    }
}

/// Load `view` through `/q/views`.
///
/// # Errors
/// When `/q/views` does not store it.
pub async fn put_view(app: &Router, view: &Value) -> Result<(), String> {
    let id = view.get("id").and_then(Value::as_str).unwrap_or_default();
    match send(app, Method::PUT, &format!("/q/views/{id}"), "", view).await {
        StatusCode::CREATED | StatusCode::NO_CONTENT => Ok(()),
        s => Err(format!("view {id}: {s}")),
    }
}

/// The rules of `cars` and `few`.
fn car_rules() -> Value {
    json!([{"types": ["Car"], "q": "speed<80", "attributes": ["speed", "colour", "note"]}])
}

/// A broker asking `engine` (none when `None`) with the Entities and the
/// views of this module.
///
/// # Errors
/// The broker's, or a write that did not land.
pub async fn fixture_with(engine: Option<Arc<dyn PolicyEngine>>) -> Result<Fixture, String> {
    // ponytail: the allowlist is read once per process; every test shares it
    std::env::set_var(antares_mcp::SUBSCRIPTION_ENDPOINTS_ENV, HOOKS);
    // `summarize` reads the host's own cgroup and /proc: a loaded test host
    // (the whole target in parallel) crossed the Tight CPU line and answered
    // the count alone (T-5646). No reading crosses these, so every test sees
    // the Normal budget; the states themselves are budget.rs unit tests.
    for (key, never) in [
        (antares_mcp::SUMMARY_BUSY_MEMORY_PCT_ENV, "1000"),
        (antares_mcp::SUMMARY_TIGHT_MEMORY_PCT_ENV, "1000"),
        (antares_mcp::SUMMARY_BUSY_CPU_AVG10_ENV, "101"),
        (antares_mcp::SUMMARY_TIGHT_CPU_AVG10_ENV, "101"),
        (antares_mcp::SUMMARY_BUSY_NODE_MIB_ENV, "1"),
        (antares_mcp::SUMMARY_TIGHT_NODE_MIB_ENV, "1"),
    ] {
        std::env::set_var(key, never);
    }
    // the surface's own sink, as a broker built with `mcp` registers it
    let broker = TestBroker::with_sinks(vec![Box::new(antares_mcp::Sink)], engine).await?;
    let app = broker.serve(antares_mcp::Mcp)?;
    for (id, speed, note) in [("1", 10, "3"), ("2", 50, "1"), ("3", 90, "1")] {
        create(
            &app,
            "city",
            &json!({
                "id": format!("urn:ngsi-ld:Car:{id}"), "type": "Car",
                // Car:1 carries a Rec 20 code, read back with its unit label
                "speed": if id == "1" {
                    json!({"type": "Property", "value": speed, "unitCode": "KMH"})
                } else {
                    prop(&json!(speed))
                },
                "colour": prop(&json!("red")),
                "secret": prop(&json!("s3cr3t")),
                "note": {"type": "Relationship", "object": format!("urn:ngsi-ld:Car:{note}")}
            }),
        )
        .await?;
    }
    for n in 0..BIG {
        create(
            &app,
            "big",
            &json!({
                "id": format!("urn:ngsi-ld:Car:{n:02}"), "type": "Car",
                "speed": prop(&json!(n)), "note": prop(&json!("n".repeat(10_000)))
            }),
        )
        .await?;
    }
    create(
        &app,
        "big",
        &json!({"id": "urn:ngsi-ld:Car:huge", "type": "Car",
                "speed": prop(&json!(999)), "note": prop(&json!("h".repeat(70_000)))}),
    )
    .await?;
    create(
        &app,
        "city",
        &json!({"id": "urn:ngsi-ld:Bike:b", "type": "Bike", "speed": prop(&json!(1))}),
    )
    .await?;
    create(
        &app,
        "other",
        &json!({"id": "urn:ngsi-ld:Car:9", "type": "Car", "speed": prop(&json!(1))}),
    )
    .await?;
    views(&app).await?;
    Ok(Fixture {
        _broker: broker,
        app,
    })
}

/// The views of this module.
async fn views(app: &Router) -> Result<(), String> {
    let app = app.clone();
    put_view(
        &app,
        &json!({"id": "cars", "tenant": "city", "selector": ["Car", "Bike"],
                "rules": car_rules(), "operations": READS}),
    )
    .await?;
    put_view(
        &app,
        &json!({"id": "watch", "tenant": "city", "selector": ["Car", "Bike"],
                "rules": car_rules(),
                "operations": ["retrieve_entity", "create_subscription", "delete_subscription"]}),
    )
    .await?;
    put_view(
        &app,
        &json!({"id": "few", "tenant": "city", "selector": ["Car"],
                "rules": car_rules(), "operations": ["query_entities"]}),
    )
    .await?;
    put_view(
        &app,
        &json!({"id": "elsewhere", "tenant": "other", "selector": ["Car"],
                "rules": [{"types": ["Car"], "attributes": ["speed"]}], "operations": READS}),
    )
    .await?;
    create(
        &app,
        "w",
        &json!({"id": "urn:ngsi-ld:Car:w1", "type": "Car", "speed": prop(&json!(1))}),
    )
    .await?;
    let every: Vec<&str> = READS.iter().chain(WRITES.iter()).copied().collect();
    for id in ["writes", "writes2"] {
        put_view(
            &app,
            &json!({"id": id, "tenant": "w", "selector": ["Car"],
                    "rules": [{"types": ["Car"], "attributes": ["speed", "colour"]}],
                    "operations": every}),
        )
        .await?;
    }
    put_view(
        &app,
        &json!({"id": "big", "tenant": "big", "selector": ["Car"],
                "rules": [{"types": ["Car"], "attributes": ["speed", "note"]}], "operations": READS}),
    )
    .await?;
    Ok(())
}

/// [`fixture_with`] and no engine.
///
/// # Errors
/// The broker's, or a write that did not land.
pub async fn fixture() -> Result<Fixture, String> {
    fixture_with(None).await
}
