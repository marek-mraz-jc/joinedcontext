//! The ways a write's and a delete's relationship checks end that a healthy broker never shows
//! (DM-70, DM-71): the broker failing a read, answering a page the gateway cannot place, or
//! hanging up on a change the delete rules make. Each broker here is scripted request by request.

use crate::raw_broker::{spawn, Reply, Req, Scripted};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use context_gateway::app::{router, Gateway};
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::relationships::RelationshipRules;
use context_gateway::resolver::{DeclaredTypes, Endpoint};
use jc_core::kinds::{Audience, PolicySpec, Representation};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const SLUG: &str = "k7vbn3tdw6hcy5pajfr4m8qz2x";
const SPACE: &str = "skoly";
const DOMAIN: &str = "banskabystrica.sk";

fn urn(kind: &str, local: &str) -> String {
    format!("urn:ngsi-ld:{kind}:{DOMAIN}:{SPACE}:{local}")
}

fn rules() -> RelationshipRules {
    let end = |target: &str, cardinality: &str, on_delete: &str| {
        json!({ "target": target, "inverse": "x", "cardinality": cardinality,
                "onDelete": on_delete, "unique": cardinality == "one-to-one" })
    };
    let single = |target: &str, cardinality: &str, on_delete: &str| {
        json!({ "type": "string", "x-ngsi-ld-kind": "Relationship",
                "x-ngsi-ld-relationship": end(target, cardinality, on_delete) })
    };
    RelationshipRules::from_schema(&json!({ "definitions": {
        "School": { "properties": {
            "building": single("Building", "many-to-one", "restrict"),
            "name": { "type": "string" } } },
        "Teacher": {
            "required": ["school"],
            "properties": {
                "desk": single("Desk", "one-to-one", "set-null"),
                "school": single("School", "many-to-one", "cascade"),
                "courses": { "type": "array", "items": { "type": "string" },
                    "x-ngsi-ld-kind": "Relationship",
                    "x-ngsi-ld-relationship": end("Course", "many-to-many", "cascade") },
                "name": { "type": "string" } } },
        "Lesson": { "properties": {
            "teacher": single("Teacher", "many-to-one", "cascade"),
            "name": { "type": "string" } } },
        "Building": { "properties": { "name": { "type": "string" } } },
        "Desk": { "properties": { "name": { "type": "string" } } },
        "Course": { "properties": { "name": { "type": "string" } } }
    } }))
}

const CLASSES: &[&str] = &["School", "Teacher", "Lesson", "Building", "Desk", "Course"];

fn policy(operations: &str, types: &[&str]) -> PolicySpec {
    let entities: String = types
        .iter()
        .map(|kind| format!("\x20     - type: {kind}\n"))
        .collect();
    serde_norway::from_str(&format!(
        "contextSpaceRef: {SPACE}\n\
         assigner: did:web:{DOMAIN}\n\
         assignee: {{ kind: role, id: public }}\n\
         operations: [{operations}]\n\
         information:\n\
         \x20 - entities:\n{entities}"
    ))
    .expect("the policy spec parses")
}

const WRITES: &str = "createEntity, createBatch, upsertBatch, appendAttrs, updateAttrs, \
                      deleteAttrs, deleteEntity, deleteBatch";

fn endpoint_with(policies: Vec<PolicySpec>) -> Endpoint {
    Endpoint {
        declared_types: Some(
            DeclaredTypes {
                model: "bb-skoly".into(),
                classes: CLASSES.iter().map(|class| (*class).to_owned()).collect(),
                units: Default::default(),
                relationships: rules(),
                quality: Vec::new(),
                schema: None,
            }
            .into(),
        ),
        catalog: None,
        deprecation: None,
        feeds: Vec::new(),
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        open_data: false,
        roles: Default::default(),
        slug: SLUG.to_owned(),
        title: Default::default(),
        description: Default::default(),
        space: SPACE.to_owned(),
        tenant: context_gateway::middleware::tenancy::Tenant::segment(SPACE).ok(),
        project: SPACE.to_owned(),
        audience: Audience::Public,
        allowed_projects: Vec::new(),
        representations: vec![Representation::NgsiLd],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: ["secret".to_owned()].into(),
        projection: None,
        view_mapping: None,
        base_path: format!("/api/endpoint/{SLUG}"),
        models: Vec::new(),
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies,
    }
}

/// The caller writes and deletes every class and reads every class.
fn endpoint() -> Endpoint {
    endpoint_with(vec![
        policy(WRITES, CLASSES),
        policy("queryEntity, retrieveEntity", CLASSES),
    ])
}

fn link(object: &str) -> Value {
    json!({ "type": "Relationship", "object": object })
}

fn teacher(local: &str, school: &str, desk: Option<&str>) -> Value {
    let mut teacher = json!({ "id": urn("Teacher", local), "type": "Teacher",
        "school": link(&urn("School", school)) });
    if let Some(desk) = desk {
        teacher["desk"] = link(&urn("Desk", desk));
    }
    teacher
}

/// The entities of `ids` as a `pick=id` read answers them.
fn found(ids: &[String]) -> Reply {
    let held: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "id": id, "type": id.split(':').nth(2).unwrap_or_default() }))
        .collect();
    Reply::Json(200, Value::Array(held))
}

fn is_pointing_read(request: &Req) -> bool {
    request.method == "GET" && request.param("q").is_some()
}

/// The target a pointing read asks about: `slot=="urn"`.
fn asked_object(request: &Req) -> String {
    request
        .param("q")
        .and_then(|q| q.split('"').nth(1).map(str::to_owned))
        .unwrap_or_default()
}

fn nothing(_: &Req, _: usize) -> Reply {
    Reply::Bytes(204, "application/json", Vec::new())
}

async fn call(
    endpoint: Endpoint,
    broker: &Scripted,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let raw = body.map(|body| body.to_string());
    call_raw(endpoint, broker, method, uri, raw).await
}

async fn call_raw(
    endpoint: Endpoint,
    broker: &Scripted,
    method: Method,
    uri: &str,
    body: Option<String>,
) -> (StatusCode, Value) {
    let gateway = Arc::new(
        Gateway::new(Broker::new(&broker.url), Box::new(PolicyPdp), DOMAIN).serve([endpoint]),
    );
    let request = Request::builder()
        .method(method)
        .uri(format!("/api/endpoint/{SLUG}/ngsi-ld/v1{uri}"))
        .header("jc-reason", "test")
        .header("content-type", "application/ld+json")
        .body(body.map_or_else(Body::empty, Body::from))
        .expect("a request");
    let response = router(gateway)
        .oneshot(request)
        .await
        .expect("the gateway answers");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("the body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// --- writes ----------------------------------------------------------------------------------

/// A broker that fails the read of a write's targets is a 502 that stores nothing.
#[tokio::test]
async fn a_broker_failing_the_read_of_the_targets_fails_the_write_and_stores_nothing() {
    let broker = spawn(|request, _| {
        if request.method == "GET" {
            Reply::Json(500, json!({ "detail": "postgres://held" }))
        } else {
            nothing(request, 0)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entities",
        Some(teacher("t9", "s1", None)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(!body.to_string().contains("postgres://"), "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// The targets exist but the read of who holds a one-to-one target fails: a 502 again.
#[tokio::test]
async fn a_broker_failing_the_read_of_a_taken_target_fails_the_write() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(500, json!({}))
        } else if request.method == "GET" {
            found(&request.ids())
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entities",
        Some(teacher("t9", "s1", Some("d1"))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// A batch whose holders' read fails is a 502 before anything is written.
#[tokio::test]
async fn a_broker_failing_the_holders_read_of_a_batch_fails_the_batch() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(503, json!({}))
        } else if request.method == "GET" {
            found(&request.ids())
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/upsert",
        Some(json!([teacher("t9", "s1", Some("d1"))])),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// A holders' read that names an entity without an id, one that holds another target and one the
/// asked targets do not place leaves the claim to a read of its own, which can fail like any.
#[tokio::test]
async fn holders_the_gateway_cannot_place_are_read_one_claim_at_a_time() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            let first = n == 2;
            if first {
                Reply::Json(
                    200,
                    json!([
                        { "type": "Teacher", "desk": link(&urn("Desk", "d1")) },
                        { "id": urn("Teacher", "x"), "type": "Teacher",
                          "desk": link(&urn("Desk", "d2")) },
                        { "id": urn("Teacher", "y"), "type": "Teacher",
                          "desk": json!([link(&urn("Desk", "d1")), link(&urn("Desk", "d2"))]) }
                    ]),
                )
            } else {
                Reply::Json(500, json!({}))
            }
        } else if request.method == "GET" {
            found(&request.ids())
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/upsert",
        Some(json!([teacher("t9", "s1", Some("d1"))])),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(broker.count("GET /ngsi-ld/v1/entities?type=Teacher"), 2);
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// A target held by hundreds of entities fills every page of the holders' read: the claim is
/// then read on its own and refused as taken, never written.
#[tokio::test]
async fn a_target_held_by_more_entities_than_the_holders_pages_is_taken() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            let desk = asked_object(request);
            let page: Vec<Value> = (0..200)
                .map(|i| {
                    json!({ "id": urn("Teacher", &format!("h{n}-{i}")), "type": "Teacher",
                            "desk": link(&desk) })
                })
                .collect();
            Reply::Json(200, Value::Array(page))
        } else if request.method == "GET" {
            found(&request.ids())
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/upsert",
        Some(json!([teacher("t9", "s1", Some("d1"))])),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert!(body.to_string().contains("target-taken"), "{body}");
    // Four full pages of the holders' read, then the claim's own read.
    assert_eq!(broker.count("GET /ngsi-ld/v1/entities?type=Teacher"), 5);
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// One entity of a batch names a target the space does not hold: it is refused by name and the
/// other is written.
#[tokio::test]
async fn a_batch_member_naming_a_missing_target_is_refused_and_the_rest_is_written() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([]))
        } else if request.method == "GET" {
            let held: Vec<String> = request
                .ids()
                .into_iter()
                .filter(|id| !id.ends_with(":s9"))
                .collect();
            found(&held)
        } else {
            let _ = n;
            Reply::Json(201, json!([urn("Teacher", "t1")]))
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/upsert",
        Some(json!([
            teacher("t1", "s1", None),
            teacher("t2", "s9", None)
        ])),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(body["success"], json!([urn("Teacher", "t1")]), "{body}");
    assert_eq!(
        body["errors"][0]["entityId"],
        urn("Teacher", "t2"),
        "{body}"
    );
    assert_eq!(
        body["errors"][0]["error"]["rule"], "target-missing",
        "{body}"
    );
}

/// What the judge refused and what the relationships refuse are both in the 207's errors.
#[tokio::test]
async fn a_batch_refused_by_the_judge_and_by_the_relationships_answers_both() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([]))
        } else if request.method == "GET" {
            let held: Vec<String> = request
                .ids()
                .into_iter()
                .filter(|id| !id.ends_with(":s9"))
                .collect();
            found(&held)
        } else {
            let _ = n;
            Reply::Json(201, json!([urn("Teacher", "t1")]))
        }
    })
    .await;
    let mut hidden = teacher("t3", "s1", None);
    hidden["secret"] = json!({ "type": "Property", "value": 1 });
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/upsert",
        Some(json!([
            teacher("t1", "s1", None),
            teacher("t2", "s9", None),
            hidden
        ])),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(body["success"], json!([urn("Teacher", "t1")]), "{body}");
    assert_eq!(body["errors"].as_array().map(Vec::len), Some(2), "{body}");
}

/// A partial write that names no relationship reads nothing, and a write with no body is
/// forwarded as it is.
#[tokio::test]
async fn a_partial_write_without_relationships_asks_the_broker_nothing() {
    let broker = spawn(nothing).await;
    let attrs = format!("/entities/{}/attrs", urn("Teacher", "t1"));
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        &attrs,
        Some(json!({ "name": { "type": "Property", "value": "Jana" } })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(broker.count("GET "), 0, "{:?}", broker.calls);

    let (status, body) = call_raw(endpoint(), &broker, Method::POST, &attrs, None).await;
    assert!(!status.is_server_error(), "{status} {body}");
    assert_eq!(broker.count("GET "), 0, "{:?}", broker.calls);
}

#[tokio::test]
async fn a_body_that_is_no_json_is_a_400_and_asks_the_broker_nothing() {
    let broker = spawn(nothing).await;
    let (status, body) = call_raw(
        endpoint(),
        &broker,
        Method::POST,
        "/entities",
        Some("{ not json".to_owned()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(broker.calls.lock().expect("the call log").is_empty());
}

// --- deletes ---------------------------------------------------------------------------------

/// A batch delete whose body is no list, or whose entries are objects naming an id, plans nothing.
#[tokio::test]
async fn a_batch_delete_that_is_no_list_of_ids_plans_nothing_and_is_forwarded() {
    let broker = spawn(nothing).await;
    for body in [json!({}), json!([{ "id": urn("Desk", "d1") }])] {
        let (status, answer) = call(
            endpoint(),
            &broker,
            Method::POST,
            "/entityOperations/delete",
            Some(body.clone()),
        )
        .await;
        assert!(!status.is_server_error(), "{body}: {status} {answer}");
    }
    assert_eq!(broker.count("GET "), 0, "{:?}", broker.calls);
    assert_eq!(broker.writes().len(), 2, "{:?}", broker.calls);
}

/// A batch delete whose plan hits a failing broker is a 502 and deletes nothing.
#[tokio::test]
async fn a_broker_failing_the_plan_of_a_batch_delete_fails_the_batch() {
    let broker = spawn(|request, n| {
        if request.method == "GET" {
            Reply::Json(500, json!({}))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/delete",
        Some(json!([urn("School", "s1")])),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// A batch delete the rules change nothing for goes to the broker whole.
#[tokio::test]
async fn a_batch_delete_nothing_points_at_is_forwarded_whole() {
    let broker = spawn(|request, n| {
        if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/delete",
        Some(json!([urn("Desk", "d1"), urn("Desk", "d2")])),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(broker.writes().len(), 1, "{:?}", broker.calls);
}

/// A pointing read that lists an entity without an id, and the same entity twice, deletes it once.
#[tokio::test]
async fn an_entity_named_twice_or_without_an_id_is_cascaded_once() {
    let held = teacher("t1", "s1", None);
    let broker = spawn(move |request, n| {
        if is_pointing_read(request) && request.param("type").as_deref() == Some("Teacher") {
            Reply::Json(
                200,
                json!([{ "type": "Teacher", "school": link(&urn("School", "s1")) }, held, held]),
            )
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("School", "s1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let writes = broker.writes();
    assert_eq!(writes.len(), 2, "{writes:?}");
    assert!(
        writes[0].starts_with("DELETE") && writes[0].contains(":s1"),
        "{writes:?}"
    );
    assert!(
        writes[1].starts_with("DELETE") && writes[1].contains("Teacher"),
        "{writes:?}"
    );
}

/// A link instance naming the deleted target alone goes, one naming it with others keeps the
/// others, both as the gateway's own writes.
#[tokio::test]
async fn a_many_end_loses_the_deleted_target_by_instance_and_by_rewrite() {
    let c1 = urn("Course", "c1");
    let c2 = urn("Course", "c2");
    let holder = json!({ "id": urn("Teacher", "t1"), "type": "Teacher",
        "school": link(&urn("School", "s1")),
        "courses": [
            { "type": "Relationship", "object": [c1.clone(), c2.clone()],
              "datasetId": "urn:ngsi-ld:Dataset:shared" },
            { "type": "Relationship", "object": c1.clone() }
        ] });
    let broker = spawn(move |request, n| {
        if is_pointing_read(request) && request.param("type").as_deref() == Some("Teacher") {
            Reply::Json(200, json!([holder]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::DELETE,
        &format!("/entities/{c1}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let writes = broker.writes();
    assert_eq!(writes.len(), 3, "{writes:?}");
    assert!(
        writes[1].starts_with("DELETE") && writes[1].ends_with("/attrs/courses"),
        "{writes:?}"
    );
    assert!(
        writes[2].starts_with("PATCH") && writes[2].ends_with("/attrs/courses"),
        "{writes:?}"
    );
}

/// The change a rewrite makes is sent as JSON, with the objects that stay.
#[tokio::test]
async fn a_rewrite_is_sent_as_json_with_the_targets_that_stay() {
    let c1 = urn("Course", "c1");
    let c2 = urn("Course", "c2");
    let holder = json!({ "id": urn("Teacher", "t1"), "type": "Teacher",
        "school": link(&urn("School", "s1")),
        "courses": [{ "type": "Relationship", "object": [c1.clone(), c2.clone()],
                      "datasetId": "urn:ngsi-ld:Dataset:shared" }] });
    let patched = Arc::new(std::sync::Mutex::new(None));
    let seen = Arc::clone(&patched);
    let broker = spawn(move |request, n| {
        if request.method == "PATCH" {
            *seen.lock().expect("the patch") =
                Some((request.header("content-type"), request.body.clone()));
        }
        if is_pointing_read(request) && request.param("type").as_deref() == Some("Teacher") {
            Reply::Json(200, json!([holder]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::DELETE,
        &format!("/entities/{c1}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (media, sent) = patched
        .lock()
        .expect("the patch")
        .clone()
        .expect("a patch was sent");
    assert_eq!(media.as_deref(), Some("application/json"));
    let sent: Value = serde_json::from_str(&sent).expect("a JSON body");
    assert_eq!(sent["object"], json!([c2]), "{sent}");
    assert_eq!(sent["datasetId"], "urn:ngsi-ld:Dataset:shared", "{sent}");
}

/// A target that more than the delete limit's worth of entities point at is refused, read page
/// by page.
#[tokio::test]
async fn a_target_pointed_at_by_more_entities_than_a_delete_may_reach_is_refused() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            let offset: usize = request
                .param("offset")
                .and_then(|o| o.parse().ok())
                .unwrap_or(0);
            let page: Vec<Value> = (offset..offset + 100)
                .map(|i| teacher(&format!("p{i}"), "s1", None))
                .collect();
            Reply::Json(200, Value::Array(page))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("School", "s1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(broker.count("GET /ngsi-ld/v1/entities?type=Teacher"), 6);
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// A caller no Policy lets delete this type is refused before a rule changes a thing.
#[tokio::test]
async fn a_caller_granted_no_delete_of_the_type_may_not_have_the_rules_delete_for_them() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([teacher("t1", "s1", Some("d1"))]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let reads_only = endpoint_with(vec![
        policy("deleteEntity", &["Building"]),
        policy("queryEntity, retrieveEntity", CLASSES),
    ]);
    let (status, body) = call(
        reads_only,
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("Desk", "d1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// The caller may delete the desk but no Policy lets them delete an attribute of a teacher: the
/// set-null rule keeps the desk.
#[tokio::test]
async fn a_set_null_the_caller_has_no_grant_for_keeps_the_delete() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([teacher("t1", "s1", Some("d1"))]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let desk_only = endpoint_with(vec![
        policy("deleteEntity", &["Desk"]),
        policy("queryEntity, retrieveEntity", CLASSES),
    ]);
    let (status, body) = call(
        desk_only,
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("Desk", "d1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// A cascade into an entity whose grant lists attributes is held on what the entity stores; one
/// the broker no longer holds cannot be shown to be covered, so the delete is kept.
#[tokio::test]
async fn a_cascade_into_an_entity_that_cannot_be_read_back_is_kept() {
    let lesson = json!({ "id": urn("Lesson", "l1"), "type": "Lesson",
        "teacher": link(&urn("Teacher", "t1")) });
    let broker = spawn(move |request, n| {
        if is_pointing_read(request) && request.param("type").as_deref() == Some("Lesson") {
            Reply::Json(200, json!([lesson]))
        } else if request.method == "GET" && request.path().contains("/entities/") {
            Reply::Json(404, json!({}))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let listed = endpoint_with(vec![
        policy("deleteEntity", &["Teacher"]),
        policy("queryEntity, retrieveEntity", CLASSES),
        serde_norway::from_str(&format!(
            "contextSpaceRef: {SPACE}\nassigner: did:web:{DOMAIN}\nassignee: {{ kind: role, id: public }}\n\
             operations: [deleteEntity]\ninformation:\n  - entities: [{{ type: Lesson }}]\n    \
             propertyNames: [name]\n    relationshipNames: [teacher]\n"
        ))
        .expect("the grant parses"),
    ]);
    let (status, body) = call(
        listed,
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("Teacher", "t1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// A restrict refusal says no id to a caller who may read none of what points at the target.
#[tokio::test]
async fn a_restrict_names_no_id_to_a_caller_with_no_read_grant() {
    let school = json!({ "id": urn("School", "s1"), "type": "School",
        "building": link(&urn("Building", "b1")) });
    let broker = spawn(move |request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([school]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let writer_only = endpoint_with(vec![policy("deleteEntity", &["Building"])]);
    let (status, body) = call(
        writer_only,
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("Building", "b1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["ids"], json!([]), "{body}");
    assert_eq!(body["count"], 1, "{body}");
}

/// A broker that answers a batch delete with a 207 has the rules run for the ids it deleted, and
/// for none when its answer lists none.
#[tokio::test]
async fn a_207_batch_delete_runs_the_rules_for_the_ids_it_deleted() {
    for (listed, unlinked) in [(true, 1), (false, 0)] {
        let broker = spawn(move |request, n| {
            if is_pointing_read(request) {
                Reply::Json(200, json!([teacher("t1", "s1", Some("d1"))]))
            } else if request.method == "GET" {
                Reply::Json(200, json!([]))
            } else if request.is("POST", "/ngsi-ld/v1/entityOperations/delete") {
                if listed {
                    Reply::Json(207, json!({ "success": [urn("Desk", "d1")], "errors": [] }))
                } else {
                    Reply::Bytes(207, "application/json", b"not json".to_vec())
                }
            } else {
                nothing(request, n)
            }
        })
        .await;
        let (status, _) = call(
            endpoint(),
            &broker,
            Method::POST,
            "/entityOperations/delete",
            Some(json!([urn("Desk", "d1")])),
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert_eq!(broker.writes().len(), 1 + unlinked, "{:?}", broker.writes());
    }
}

/// A broker that hangs up on a change a delete rule makes stops the rest and says how far the
/// rules got.
#[tokio::test]
async fn a_broker_hanging_up_on_an_unlink_is_a_502_naming_how_far_it_got() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([teacher("t1", "s1", Some("d1"))]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else if request.method == "DELETE" && request.path().contains("/attrs/") {
            Reply::Hangup
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("Desk", "d1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["changed"], 0, "{body}");
}

/// A pointing read the broker answers with something other than a list reads as nobody pointing.
#[tokio::test]
async fn a_pointing_read_answered_with_no_list_means_nobody_points() {
    let broker = spawn(|request, n| {
        if request.method == "GET" {
            Reply::Json(200, json!({ "detail": "no list" }))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("Desk", "d1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

fn holder_of_courses(instances: Value) -> Value {
    json!({ "id": urn("Teacher", "t1"), "type": "Teacher",
        "school": link(&urn("School", "s1")), "courses": instances })
}

/// A rewrite of a link instance with no `datasetId` is sent without one.
#[tokio::test]
async fn a_rewrite_of_an_instance_without_a_dataset_is_sent_without_one() {
    let (c1, c2) = (urn("Course", "c1"), urn("Course", "c2"));
    let holder =
        holder_of_courses(json!([{ "type": "Relationship", "object": [c1.clone(), c2.clone()] }]));
    let broker = spawn(move |request, n| {
        if is_pointing_read(request) && request.param("type").as_deref() == Some("Teacher") {
            Reply::Json(200, json!([holder]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::DELETE,
        &format!("/entities/{c1}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(broker.writes().len(), 2, "{:?}", broker.writes());
}

/// A rewrite needs the update grant of the attribute, which a delete grant does not give.
#[tokio::test]
async fn a_rewrite_the_caller_has_no_update_grant_for_keeps_the_delete() {
    let (c1, c2) = (urn("Course", "c1"), urn("Course", "c2"));
    let holder =
        holder_of_courses(json!([{ "type": "Relationship", "object": [c1.clone(), c2.clone()] }]));
    let broker = spawn(move |request, n| {
        if is_pointing_read(request) && request.param("type").as_deref() == Some("Teacher") {
            Reply::Json(200, json!([holder]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let no_update = endpoint_with(vec![
        policy("deleteEntity", &["Course"]),
        policy("deleteAttrs", &["Teacher"]),
        policy("queryEntity, retrieveEntity", CLASSES),
    ]);
    let (status, body) = call(
        no_update,
        &broker,
        Method::DELETE,
        &format!("/entities/{c1}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}

/// The rules of a 207 batch delete stop at a change the broker never answers, and say so.
#[tokio::test]
async fn a_207_batch_delete_whose_unlink_the_broker_drops_is_a_502() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([teacher("t1", "s1", Some("d1"))]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else if request.is("POST", "/ngsi-ld/v1/entityOperations/delete") {
            Reply::Json(207, json!({ "success": [urn("Desk", "d1")], "errors": [] }))
        } else if request.method == "DELETE" {
            Reply::Hangup
        } else {
            nothing(request, n)
        }
    })
    .await;
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/delete",
        Some(json!([urn("Desk", "d1")])),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
}

/// A broker answering the forwarded part of a divided batch with a 207 of its own has its lists
/// merged with the members the gateway refused.
#[tokio::test]
async fn a_divided_batch_the_broker_answers_with_a_207_is_merged_member_by_member() {
    let t1 = urn("Teacher", "t1");
    let listed = t1.clone();
    let broker = spawn(move |request, _| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([]))
        } else if request.method == "GET" {
            found(&request.ids())
        } else {
            Reply::Json(207, json!({ "success": [listed], "errors": [] }))
        }
    })
    .await;
    let mut hidden = teacher("t3", "s1", None);
    hidden["secret"] = json!({ "type": "Property", "value": 1 });
    let (status, body) = call(
        endpoint(),
        &broker,
        Method::POST,
        "/entityOperations/upsert",
        Some(json!([teacher("t1", "s1", None), hidden])),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(body["success"], json!([t1]), "{body}");
    assert_eq!(body["errors"].as_array().map(Vec::len), Some(1), "{body}");
}

/// A model that ships its `@context` is read for the identifiers of a write; a plain write passes.
#[tokio::test]
async fn a_write_through_an_endpoint_whose_model_ships_a_context_is_judged_by_it() {
    let broker = spawn(|request, n| {
        if request.method == "GET" {
            found(&request.ids())
        } else {
            nothing(request, n)
        }
    })
    .await;
    let mut endpoint = endpoint();
    endpoint.models = vec![context_gateway::resolver::Model {
        name: "bb-skoly".into(),
        version: "1.0.0".into(),
        major: 1,
        classes: CLASSES.iter().map(|class| (*class).to_owned()).collect(),
        json_schema: None,
        context: Some(Arc::new(
            json!({ "@context": { "Teacher": "https://example.org/Teacher" } }),
        )),
    }];
    let (status, body) = call(
        endpoint,
        &broker,
        Method::POST,
        "/entities",
        Some(teacher("t9", "s1", None)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// One entity naming its one-to-one target twice claims it once.
#[tokio::test]
async fn an_entity_naming_its_one_to_one_target_twice_claims_it_once() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([]))
        } else if request.method == "GET" {
            found(&request.ids())
        } else {
            nothing(request, n)
        }
    })
    .await;
    let mut twice = teacher("t9", "s1", None);
    let desk = urn("Desk", "d1");
    twice["desk"] = json!([
        { "type": "Relationship", "object": desk.clone(), "datasetId": "urn:ngsi-ld:Dataset:a" },
        { "type": "Relationship", "object": desk, "datasetId": "urn:ngsi-ld:Dataset:b" }
    ]);
    let (status, body) = call(endpoint(), &broker, Method::POST, "/entities", Some(twice)).await;
    assert!(!status.is_server_error(), "{status} {body}");
}

/// A prohibition of deleting the type keeps the rules from deleting the root for the caller.
#[tokio::test]
async fn a_prohibited_root_is_not_deleted_by_the_rules_for_the_caller() {
    let broker = spawn(|request, n| {
        if is_pointing_read(request) {
            Reply::Json(200, json!([teacher("t1", "s1", Some("d1"))]))
        } else if request.method == "GET" {
            Reply::Json(200, json!([]))
        } else {
            nothing(request, n)
        }
    })
    .await;
    let prohibited = endpoint_with(vec![
        policy(WRITES, CLASSES),
        policy("queryEntity, retrieveEntity", CLASSES),
        serde_norway::from_str(&format!(
            "effect: prohibition\ncontextSpaceRef: {SPACE}\nassigner: did:web:{DOMAIN}\n\
             assignee: {{ kind: role, id: public }}\noperations: [deleteEntity]\n\
             information:\n  - entities: [{{ type: Desk }}]\n"
        ))
        .expect("the prohibition parses"),
    ]);
    let (status, body) = call(
        prohibited,
        &broker,
        Method::DELETE,
        &format!("/entities/{}", urn("Desk", "d1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(broker.writes().is_empty(), "{:?}", broker.writes());
}
