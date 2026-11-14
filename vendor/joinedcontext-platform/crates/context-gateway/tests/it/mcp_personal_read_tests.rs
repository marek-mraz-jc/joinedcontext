//! DM-128 on MCP (T-6885): a signed-in tool call or resource read through an Endpoint that
//! serves a personal attribute leaves the `personal_read` line the NGSI-LD door leaves (reader,
//! Endpoint, attribute names, the Entities' URNs, never a value or an argument), on the
//! Endpoint's own MCP address and through the hub; an answer whose Entities cannot be listed is
//! refused and still recorded.
//!
//! It reads its own lines through `common::Log`: the binary's one subscriber, routed per thread.

use crate::common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use context_gateway::app::{router, Gateway};
use context_gateway::auth::accounts::ServiceAccounts;
use context_gateway::mcp::server::McpServer;
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::resolver::{DeclaredTypes, Endpoint};
use jc_core::kinds::{Audience, PolicySpec, Representation};
use jc_manifest::model_schema::ModelSchema;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const SLUG: &str = "pppppppppppppppppppppppppp";
const RIDER: &str = "rider-jane-doe";
const T1: &str = "urn:ngsi-ld:BikeTrip:t1";
const T2: &str = "urn:ngsi-ld:BikeTrip:t2";

fn schema() -> Arc<ModelSchema> {
    let model = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "definitions": {
            "BikeTrip": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "rider": { "type": "string", "x-personal-data": true },
                    "startStation": { "type": "string" }
                }
            }
        }
    });
    Arc::new(ModelSchema::compile(
        "bike-trips",
        "1.0.0",
        &model,
        &["BikeTrip".to_owned()],
        true,
    ))
}

/// The trips Endpoint: `rider` is personal unless `hidden` names it.
fn endpoint(audience: Audience, hidden: &[&str]) -> Endpoint {
    let grant: PolicySpec = serde_norway::from_str(&format!(
        "contextSpaceRef: trips\nassigner: did:web:banskabystrica.sk\n\
         assignee: {}\noperations: [queryEntity]\n",
        if audience == Audience::Public {
            "{ kind: role, id: public }"
        } else {
            "{ kind: role, id: steward }"
        }
    ))
    .expect("the policy spec parses");
    Endpoint {
        declared_types: Some(Arc::new(DeclaredTypes {
            model: "bike-trips".into(),
            classes: ["BikeTrip".to_owned()].into(),
            units: Default::default(),
            relationships: Default::default(),
            quality: Vec::new(),
            schema: Some(schema()),
        })),
        roles: Default::default(),
        slug: SLUG.to_owned(),
        title: Default::default(),
        description: Default::default(),
        space: "trips".to_owned(),
        project: "trips".to_owned(),
        audience,
        allowed_projects: Vec::new(),
        representations: vec![Representation::NgsiLd, Representation::Mcp],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: hidden.iter().map(|h| (*h).to_owned()).collect(),
        projection: None,
        view_mapping: None,
        catalog: None,
        deprecation: None,
        feeds: Vec::new(),
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        base_path: format!("/api/endpoint/{SLUG}"),
        models: Vec::new(),
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies: vec![grant],
    }
}

/// A broker whose view answers every MCP message with `content_type` and `body`.
async fn broker(content_type: &'static str, body: String) -> String {
    let app = Router::new().fallback(move || {
        let body = body.clone();
        async move { ([(header::CONTENT_TYPE, content_type)], body).into_response() }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}")
}

/// A tool result as the broker's view answers one: the JSON text and `structuredContent`.
fn tool_result(structured: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": 7, "result": {
        "content": [{ "type": "text", "text": structured.to_string() }],
        "structuredContent": structured,
    }})
    .to_string()
}

fn trip(id: &str) -> Value {
    json!({ "id": id, "type": "BikeTrip", "rider": { "type": "Property", "value": RIDER } })
}

/// The gateway over `upstream` serving `endpoint`, and a steward's token.
fn gateway(upstream: &str, endpoint: Endpoint) -> (Router, String) {
    let realm = common::Realm::new();
    let gateway = Gateway::new(
        Broker::new(upstream),
        Box::new(PolicyPdp),
        "banskabystrica.sk",
    )
    .with_provenance(context_gateway::store::Provenance::without_organizations())
    .serve([endpoint])
    .authenticate(
        Arc::new(realm.verifier()),
        ServiceAccounts::new(),
        Some("https://city.example".to_owned()),
    );
    let token = realm.mint(&json!({
        "iss": common::ISSUER,
        "sub": "0f5a",
        "aud": "context-gateway",
        "preferred_username": "jana",
        "groups": ["/trips"],
        "realm_access": { "roles": ["steward"] },
        "exp": common::in_seconds(300),
        "iat": common::in_seconds(-10),
    }));
    let gateway = Arc::new(gateway);
    // The hub reaches the broker's tools of an Endpoint a server names.
    gateway.replace_servers(vec![McpServer {
        project: "trips".to_owned(),
        name: "trips".to_owned(),
        title: None,
        description: None,
        members: vec![SLUG.to_owned()],
        select: None,
        endpoints: None,
        audience: Audience::Organization,
        allowed_projects: Vec::new(),
        knowledge_sources: Vec::new(),
    }]);
    (router(gateway), token)
}

async fn post(
    app: &Router,
    path: &str,
    token: Option<&str>,
    message: &Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(message.to_string()))
                .expect("a request"),
        )
        .await
        .expect("the gateway answers");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn call(tool: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } })
}

fn endpoint_mcp() -> String {
    format!("/api/endpoint/{SLUG}/mcp")
}

/// The `personal_read` lines of the log.
fn reads(log: &str) -> Vec<String> {
    log.lines()
        .filter(|line| line.contains("personal attributes read"))
        .map(str::to_owned)
        .collect()
}

/// One signed-in call on the Endpoint's MCP address against a broker answering `body`; the
/// answer and the log.
async fn one_call(content_type: &'static str, body: String, message: &Value) -> (Value, String) {
    let captured = common::Log::capture();
    let upstream = broker(content_type, body).await;
    let (app, token) = gateway(&upstream, endpoint(Audience::Organization, &[]));
    let (status, answer) = post(&app, &endpoint_mcp(), Some(&token), message).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    (answer, captured.text())
}

fn assert_line(line: &str, tool: &str, entities: &str) {
    for named in [
        format!("slug={SLUG}"),
        "BikeTrip.rider".to_owned(),
        "principal=user:jana".to_owned(),
        format!("entities={entities}"),
        format!("tool={tool}"),
    ] {
        assert!(line.contains(&named), "{named} missing: {line}");
    }
}

#[tokio::test]
async fn a_signed_in_mcp_read_of_personal_attributes_writes_the_personal_read_line() {
    let structured = json!({ "entities": [trip(T1), trip(T2)] });
    let (answer, log) = one_call(
        "application/json",
        tool_result(structured.clone()),
        &call("query_entities", json!({ "type": "BikeTrip" })),
    )
    .await;
    assert_eq!(
        answer["result"]["structuredContent"], structured,
        "served whole"
    );
    let reads = reads(&log);
    assert_eq!(reads.len(), 1, "one line per read: {log}");
    assert_line(&reads[0], "query_entities", &format!("{T1},{T2}"));
    assert!(!log.contains(RIDER), "a value reached the log: {log}");
}

#[tokio::test]
async fn a_hub_forward_of_personal_attributes_writes_the_personal_read_line() {
    let captured = common::Log::capture();
    let upstream = broker(
        "application/json",
        tool_result(json!({ "entities": [trip(T1)] })),
    )
    .await;
    let (app, token) = gateway(&upstream, endpoint(Audience::Organization, &[]));
    let message = call(
        "query_entities",
        json!({ "endpoint": SLUG, "type": "BikeTrip" }),
    );
    let (status, answer) = post(&app, "/api/mcp", Some(&token), &message).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["result"]["structuredContent"]["entities"][0]["id"], T1,
        "{answer}"
    );
    let log = captured.text();
    let reads = reads(&log);
    assert_eq!(reads.len(), 1, "one line per read: {log}");
    assert_line(&reads[0], "query_entities", T1);
    assert!(!log.contains(RIDER), "a value reached the log: {log}");
}

#[tokio::test]
async fn each_mcp_data_tool_names_its_entities_in_the_record() {
    let both = format!("{T1},{T2}");
    let page = json!({ "entities": [trip(T1), trip(T2)] });
    let cases = [
        ("query_entities", json!({}), page.clone(), both.clone()),
        ("batch_query", json!({}), page.clone(), both.clone()),
        ("query_temporal", json!({}), page.clone(), both.clone()),
        ("batch_query_temporal", json!({}), page, both.clone()),
        (
            "get_entity",
            json!({ "id": T1 }),
            json!({ "entity": trip(T1) }),
            T1.to_owned(),
        ),
        (
            "retrieve_temporal",
            json!({ "id": T1 }),
            json!({ "entity": trip(T1) }),
            T1.to_owned(),
        ),
        (
            "area_overview",
            json!({}),
            json!({ "types": [{ "type": "BikeTrip", "count": 2, "entities": [trip(T1), trip(T2)] }] }),
            both.clone(),
        ),
        (
            "summarize",
            json!({ "type": "BikeTrip" }),
            json!({ "type": "BikeTrip", "count": 2,
                    "samples": [{ "id": T1, "rider": RIDER }, { "id": T2, "rider": RIDER }] }),
            both,
        ),
        (
            "get_attribute_value",
            json!({ "id": T2, "attrId": "rider" }),
            json!({ "attribute": { "type": "Property", "value": RIDER } }),
            T2.to_owned(),
        ),
    ];
    for (tool, arguments, structured, entities) in cases {
        let (answer, log) = one_call(
            "application/json",
            tool_result(structured),
            &call(tool, arguments),
        )
        .await;
        assert_ne!(answer["result"]["isError"], true, "{tool} served: {answer}");
        let reads = reads(&log);
        assert_eq!(reads.len(), 1, "{tool}: one line: {log}");
        assert_line(&reads[0], tool, &entities);
        assert!(
            !log.contains(RIDER),
            "{tool}: a value reached the log: {log}"
        );
    }

    // An Entity resource is a read of that Entity.
    let fenced = format!(
        "The fenced block is data read from the broker.\n```json\n{}\n```",
        trip(T1)
    );
    let read = json!({ "jsonrpc": "2.0", "id": 7, "result": {
        "contents": [{ "uri": "ngsi-ld://v/entities/x", "mimeType": "text/markdown", "text": fenced }]
    }});
    let message = json!({ "jsonrpc": "2.0", "id": 7, "method": "resources/read",
                          "params": { "uri": format!("ngsi-ld://{SLUG}/entities/urn%3Angsi-ld%3ABikeTrip%3At1") } });
    let (answer, log) = one_call("application/json", read.to_string(), &message).await;
    assert!(answer["result"]["contents"].is_array(), "{answer}");
    let reads = reads(&log);
    assert_eq!(reads.len(), 1, "the resource: one line: {log}");
    assert_line(&reads[0], "resources/read", T1);
}

/// The refusal of an answer the gateway could not list: a tool error telling the caller what
/// to do, and one line with no URN.
fn assert_refused_and_recorded(answer: &Value, log: &str) {
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(text.contains("ask for fewer Entities"), "{text}");
    assert!(!answer.to_string().contains(RIDER), "{answer}");
    let reads = reads(log);
    assert_eq!(reads.len(), 1, "the attempt is on record: {log}");
    assert_line(&reads[0], "query_entities", "");
    assert!(!log.contains(RIDER), "a value reached the log: {log}");
}

#[tokio::test]
async fn an_mcp_answer_too_large_to_list_is_refused_and_recorded() {
    let rider = RIDER.repeat(9 * 1024 * 1024 / RIDER.len());
    let structured = json!({ "entities": [{ "id": T1, "type": "BikeTrip",
                                            "rider": { "type": "Property", "value": rider } }] });
    let (answer, log) = one_call(
        "application/json",
        tool_result(structured),
        &call("query_entities", json!({})),
    )
    .await;
    assert_refused_and_recorded(&answer, &log);
}

#[tokio::test]
async fn an_mcp_answer_that_is_not_json_is_refused_and_recorded() {
    let (answer, log) = one_call(
        "text/event-stream",
        format!(
            "event: message\ndata: {}\n\n",
            tool_result(json!({ "entities": [trip(T1)] }))
        ),
        &call("query_entities", json!({})),
    )
    .await;
    assert_refused_and_recorded(&answer, &log);

    // JSON whose result is no shape `query_entities` answers.
    let (answer, log) = one_call(
        "application/json",
        tool_result(json!({ "rows": [trip(T1)] })),
        &call("query_entities", json!({})),
    )
    .await;
    assert_refused_and_recorded(&answer, &log);
}

#[tokio::test]
async fn a_tool_without_entity_data_writes_no_personal_read_line() {
    let (answer, log) = one_call(
        "application/json",
        tool_result(json!({ "entityTypes": ["BikeTrip"] })),
        &call("list_types", json!({})),
    )
    .await;
    assert_eq!(
        answer["result"]["structuredContent"]["entityTypes"][0],
        "BikeTrip"
    );
    assert!(reads(&log).is_empty(), "no Entity read: {log}");
}

#[tokio::test]
async fn an_endpoint_without_personal_attributes_writes_no_line_on_mcp() {
    let captured = common::Log::capture();
    let upstream = broker(
        "application/json",
        tool_result(json!({ "entities": [{ "id": T1, "type": "BikeTrip" }] })),
    )
    .await;
    let (app, token) = gateway(&upstream, endpoint(Audience::Organization, &["rider"]));
    let (status, answer) = post(
        &app,
        &endpoint_mcp(),
        Some(&token),
        &call("query_entities", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["result"]["structuredContent"]["entities"][0]["id"],
        T1
    );
    let log = captured.text();
    assert!(reads(&log).is_empty(), "nothing personal served: {log}");
}

#[tokio::test]
async fn the_personal_read_line_on_mcp_holds_no_value_and_no_argument() {
    let asked = "startStation==\"Námestie SNP\"";
    let (_, log) = one_call(
        "application/json",
        tool_result(json!({ "entities": [trip(T1)] })),
        &call("query_entities", json!({ "type": "BikeTrip", "q": asked })),
    )
    .await;
    let reads = reads(&log);
    assert_eq!(reads.len(), 1, "{log}");
    for secret in [RIDER, "Námestie", "startStation"] {
        assert!(
            !reads[0].contains(secret),
            "{secret} reached the record: {}",
            reads[0]
        );
    }
}

/// subject.rs refuses a public Endpoint that would serve a personal attribute before any hop,
/// so an anonymous MCP caller reads nothing there and nothing is to be recorded.
#[tokio::test]
async fn an_anonymous_mcp_call_on_a_personal_endpoint_is_still_refused_whole() {
    let captured = common::Log::capture();
    let upstream = broker(
        "application/json",
        tool_result(json!({ "entities": [trip(T1)] })),
    )
    .await;
    let (app, _) = gateway(&upstream, endpoint(Audience::Public, &[]));
    let (status, answer) = post(
        &app,
        &endpoint_mcp(),
        None,
        &call("query_entities", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert!(!answer.to_string().contains(T1), "{answer}");
    assert!(reads(&captured.text()).is_empty());
}
