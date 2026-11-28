// SPDX-License-Identifier: EUPL-1.2
//! 4.3.6.1: "Ultimately, all constraints specified in the registration shall
//! be respected." A source registered for `Vehicle` is not a source of
//! `OffStreetParking`, even when a multi-type query reaches it and it
//! answers with one: that entity is not this registration's data, and
//! importing it lets it override the source that is registered for it
//! (ETSI IOP_003_02_02, IOP_003_03_02, run 38806308784).

use crate::common::Double;
use antares_api::{AppState, BuiltForward, ForwardAnswer, ForwardFuture, ForwardTransport};
use antares_model::TenantId;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// A Context Source answering every request with `body`.
fn source(body: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 65536];
            let _ = s.read(&mut buf);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(reply.as_bytes());
        }
    });
    port
}

async fn call(st: &AppState, req: Request<Body>) -> (StatusCode, Value) {
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// What a `type=OffStreetParking,Vehicle` query returns when its only data
/// comes from one source, registered once per element of `registrations`
/// (each an `information` array), that answers with a Vehicle and an
/// OffStreetParking carrying `speed` and `totalSpotsNumber`.
async fn query_through(registrations: &[Value]) -> Value {
    antares_jsonld::allow_private_egress(true);
    let port = source(
        r#"[{"id":"urn:ngsi-ld:Vehicle:1","type":"Vehicle","speed":{"type":"Property","value":1}},
            {"id":"urn:ngsi-ld:OffStreetParking:1","type":"OffStreetParking","speed":{"type":"Property","value":2},
             "totalSpotsNumber":{"type":"Property","value":3}}]"#,
    );
    let st = AppState::new("antares-fed-types".into());
    for (n, information) in registrations.iter().enumerate() {
        let reg = json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:src{n}"),
            "type": "ContextSourceRegistration",
            "mode": "inclusive",
            "operations": ["federationOps"],
            "information": information,
            "endpoint": format!("http://127.0.0.1:{port}"),
        })
        .to_string();
        let (status, _) = call(
            &st,
            Request::builder()
                .method("POST")
                .uri("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", reg.len())
                .body(Body::from(reg))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, body) = call(
        &st,
        Request::builder()
            .method("GET")
            .uri("/ngsi-ld/v1/entities?type=OffStreetParking,Vehicle")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn ids_through(information: Value) -> Vec<String> {
    let body = query_through(&[information]).await;
    let mut ids: Vec<String> = body
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    ids.sort();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entity_outside_the_registered_types_is_not_imported() {
    let ids = ids_through(json!([{"entities": [{"type": "Vehicle"}]}])).await;
    assert_eq!(ids, ["urn:ngsi-ld:Vehicle:1"]);
}

/// An attributes-only RegistrationInfo names no type, so it admits every
/// type, even beside a typed one.
#[tokio::test(flavor = "multi_thread")]
async fn an_info_without_entities_admits_every_type() {
    let ids = ids_through(json!([
        {"entities": [{"type": "Vehicle"}]},
        {"propertyNames": ["speed"]}
    ]))
    .await;
    assert_eq!(
        ids,
        ["urn:ngsi-ld:OffStreetParking:1", "urn:ngsi-ld:Vehicle:1"]
    );
}

/// One forward as the source saw it: method, URL, query pairs, JSON body.
type Asked = (String, String, Vec<(String, String)>, Option<Value>);

/// A Context Source behind the forward transport: it answers what it was
/// last told to and keeps what it was asked.
struct Scripted {
    answer: Mutex<(u16, Value)>,
    asked: Mutex<Vec<Asked>>,
}

impl Scripted {
    fn say(&self, status: u16, body: Value) {
        *self.answer.lock().expect("lock") = (status, body);
    }

    fn take(&self) -> Vec<Asked> {
        std::mem::take(&mut *self.asked.lock().expect("lock"))
    }
}

impl ForwardTransport for Scripted {
    fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        self.asked.lock().expect("lock").push((
            req.method().to_string(),
            req.url().to_owned(),
            req.query().to_vec(),
            req.body().and_then(|b| serde_json::from_slice(b).ok()),
        ));
        let (status, body) = self.answer.lock().expect("lock").clone();
        let answer: ForwardAnswer = (status, body, Vec::new());
        Some(Box::pin(std::future::ready(answer)))
    }
}

/// A broker whose forwards all go to one [`Scripted`] source, with
/// `registrations` created through the API.
async fn scripted(registrations: &[Value]) -> (AppState, Arc<Scripted>) {
    let peer = Arc::new(Scripted {
        answer: Mutex::new((200, json!([]))),
        asked: Mutex::default(),
    });
    let mut st = AppState::new("fed-arms".into());
    antares_api::wire(&mut st).await;
    let st = st
        .with_forward_transport(Arc::clone(&peer) as Arc<dyn ForwardTransport>)
        .expect("a transport on a fresh state");
    for reg in registrations {
        let body = reg.to_string();
        let (status, answer) = call(
            &st,
            Request::builder()
                .method("POST")
                .uri("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
    }
    (st, peer)
}

/// A broker with no registration mirror, its forwards going to one
/// [`Scripted`] source: every forward reads the registrations the store
/// holds, rows written around the API included.
fn unwired() -> (AppState, Arc<Scripted>) {
    let peer = Arc::new(Scripted {
        answer: Mutex::new((200, json!([]))),
        asked: Mutex::default(),
    });
    let st = AppState::new("fed-arms".into())
        .with_forward_transport(Arc::clone(&peer) as Arc<dyn ForwardTransport>)
        .expect("a transport on a fresh state");
    (st, peer)
}

/// One write with a JSON body: its status and its body.
async fn write(st: &AppState, method: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
    let body = body.to_string();
    call(
        st,
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request"),
    )
    .await
}

fn registration(name: &str, endpoint: &str, operations: &[&str], information: Value) -> Value {
    json!({
        "id": format!("urn:ngsi-ld:ContextSourceRegistration:{name}"),
        "type": "ContextSourceRegistration",
        "mode": "inclusive",
        "operations": operations,
        "information": information,
        "endpoint": endpoint,
    })
}

/// A registration written around the API, as a row from before a rule would
/// be: it names what the API would refuse.
async fn store_registration(st: &AppState, name: &str, extra: Value, information: Value) {
    let id = format!("urn:ngsi-ld:ContextSourceRegistration:{name}");
    let mut row = registration(
        name,
        "http://peer-a.invalid",
        &["federationOps"],
        information,
    );
    if let (Some(row), Some(extra)) = (row.as_object_mut(), extra.as_object()) {
        row.extend(extra.clone());
    }
    st.store
        .create(&TenantId::default(), Kind::Registration, &id, row)
        .await
        .expect("registration stored");
}

/// The status, the `NGSILD-Warning` values and the body of one GET.
async fn get(st: &AppState, uri: &str) -> (StatusCode, Vec<String>, Value) {
    let resp = antares_api::router(st.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = resp.status();
    let warnings = resp
        .headers()
        .get_all("NGSILD-Warning")
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        warnings,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const VEHICLE_1: &str = "urn:ngsi-ld:Vehicle:1";

/// An entity that expands to nothing: its Relationship has no URI object.
fn invalid_entity(id: &str) -> Value {
    json!({"id": id, "type": "Vehicle",
           "rel": {"type": "Relationship", "object": "not a uri"}})
}

/// 5.7.2 step 4: a source offering queryBatch is asked by a Query body, one
/// offering queryEntity by query parameters; either carries the `id` list
/// the registration can match, as one string or an array, and neither
/// invents a `type` the client did not give. A registered conformance
/// version amends payloads only, so a GET passes unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn a_query_is_asked_of_each_source_in_the_shape_its_operations_allow() {
    let info = json!([{"entities": [{"type": "Vehicle"}]}]);
    let mut plain = registration(
        "plain",
        "http://peer-b.invalid",
        &["queryEntity"],
        info.clone(),
    );
    plain["contextSourceInfo"] = json!([{"key": "ngsildConformance", "value": "1.6"}]);
    let (st, peer) = scripted(&[
        registration("batch", "http://peer-a.invalid", &["queryBatch"], info),
        plain,
    ])
    .await;
    let from = |asked: &[Asked], host: &str| -> Asked {
        asked
            .iter()
            .find(|a| a.1.starts_with(host))
            .cloned()
            .expect("the source was asked")
    };

    let (status, _, body) = get(
        &st,
        "/ngsi-ld/v1/entities?type=Vehicle&id=urn:ngsi-ld:Vehicle:1,urn:ngsi-ld:Vehicle:2",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let asked = peer.take();
    let sent = from(&asked, "http://peer-a.invalid")
        .3
        .expect("a Query body");
    assert_eq!(
        sent["entities"][0]["id"],
        json!(["urn:ngsi-ld:Vehicle:1", "urn:ngsi-ld:Vehicle:2"]),
        "{sent}"
    );

    let (status, _, body) = get(
        &st,
        "/ngsi-ld/v1/entities?type=Vehicle&id=urn:ngsi-ld:Vehicle:1",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let asked = peer.take();
    let sent = from(&asked, "http://peer-a.invalid")
        .3
        .expect("a Query body");
    assert_eq!(sent["entities"][0]["id"], VEHICLE_1, "{sent}");

    let (status, _, body) = get(&st, "/ngsi-ld/v1/entities?attrs=speed").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let asked = peer.take();
    let sent = from(&asked, "http://peer-a.invalid")
        .3
        .expect("a Query body");
    assert!(
        sent.get("entities").is_none(),
        "no selection to send: {sent}"
    );
    let query = from(&asked, "http://peer-b.invalid").2;
    assert!(!query.iter().any(|(k, _)| k == "type"), "{query:?}");
}

/// 6.3.17, Table 6.3.17-1: a member of a source's answer that is no NGSI-LD
/// Entity is not merged and is warned about (111).
#[tokio::test(flavor = "multi_thread")]
async fn a_source_answering_an_invalid_entity_is_warned_about() {
    let (st, peer) = scripted(&[registration(
        "plain",
        "http://peer-a.invalid",
        &["queryEntity"],
        json!([{"entities": [{"type": "Vehicle"}]}]),
    )])
    .await;
    peer.say(200, json!([invalid_entity("urn:ngsi-ld:Vehicle:9")]));
    let (status, warnings, body) = get(&st, "/ngsi-ld/v1/entities?type=Vehicle").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        warnings.iter().any(|w| w.starts_with("111 ")),
        "{warnings:?}"
    );
    assert_eq!(body, json!([]));
}

/// 5.7.1 step 4: a Retrieve Entity forwarded to a source whose registration names
/// no type asks for any; what comes back that is no object, names another
/// Entity or is no Entity adds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_retrieve_ignores_answers_that_are_not_the_entity() {
    let (st, peer) = scripted(&[registration(
        "retrieve",
        "http://peer-a.invalid",
        &["retrieveEntity"],
        json!([{"entities": [{"type": "Vehicle"}]}]),
    )])
    .await;
    let uri = format!("/ngsi-ld/v1/entities/{VEHICLE_1}");
    for answer in [
        Value::Null,
        json!([{"id": "urn:ngsi-ld:Vehicle:2", "type": "Vehicle"}]),
        invalid_entity(VEHICLE_1),
    ] {
        peer.say(200, answer);
        let (status, _, body) = get(&st, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(peer.take().len(), 1, "the source was asked once");
    }
}

/// 5.7.3 step 4: the same for a Retrieve Temporal Evolution: an answer naming
/// another Entity, or one that is no Temporal Entity, adds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_temporal_retrieve_ignores_answers_that_are_not_the_entity() {
    let (st, peer) = scripted(&[registration(
        "temporal",
        "http://peer-a.invalid",
        &["retrieveTemporal"],
        json!([{"entities": [{"type": "Vehicle"}]}]),
    )])
    .await;
    let uri = format!("/ngsi-ld/v1/temporal/entities/{VEHICLE_1}");
    for (code, answer) in [
        (500, Value::Null),
        (
            200,
            json!({"id": "urn:ngsi-ld:Vehicle:2", "type": "Vehicle"}),
        ),
        (200, invalid_entity(VEHICLE_1)),
    ] {
        peer.say(code, answer);
        let (status, _, body) = get(&st, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(peer.take().len(), 1, "the source was asked once");
    }
}

/// 5.7.4 step 4: a temporal query carries the client's Attributes to a source
/// that registered none, and takes nothing from an answer that failed, is
/// no array, or holds no Temporal Entity (111).
#[tokio::test(flavor = "multi_thread")]
async fn a_temporal_query_takes_only_what_a_source_answers_well() {
    let (st, peer) = scripted(&[registration(
        "temporal",
        "http://peer-a.invalid",
        &["queryTemporal"],
        json!([{"entities": [{"type": "Vehicle"}]}]),
    )])
    .await;
    let uri = "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=2020-01-01T00:00:00Z&attrs=speed";
    for (status, answer, warned) in [
        (500, Value::Null, false),
        (200, json!({}), false),
        (200, json!([invalid_entity("urn:ngsi-ld:Vehicle:9")]), true),
    ] {
        peer.say(status, answer);
        let (code, warnings, body) = get(&st, uri).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(body, json!([]));
        assert_eq!(
            warnings.iter().any(|w| w.starts_with("111 ")),
            warned,
            "{warnings:?}"
        );
        let asked = peer.take();
        assert_eq!(asked.len(), 1);
        assert!(
            asked[0]
                .2
                .contains(&("attrs".to_owned(), "speed".to_owned())),
            "{:?}",
            asked[0].2
        );
    }
}

/// A registration may name several types for one EntityInfo (5.2.8): a
/// query for either reaches the source.
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_info_naming_several_types_is_asked_for_any_of_them() {
    let (st, peer) = scripted(&[registration(
        "several",
        "http://peer-a.invalid",
        &["queryEntity"],
        json!([{"entities": [{"type": ["Vehicle", "Car"]}]}]),
    )])
    .await;
    let (status, _, body) = get(&st, "/ngsi-ld/v1/entities?type=Car").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(peer.take().len(), 1, "the source was asked");
}

/// clause 5.7.1, step 4: a registration whose information names no Entity type admits
/// every type, so a retrieve forwarded as a query asks for any.
#[tokio::test(flavor = "multi_thread")]
async fn a_retrieve_through_a_registration_naming_no_type_asks_for_any() {
    let (st, peer) = scripted(&[registration(
        "untyped",
        "http://peer-a.invalid",
        &["queryEntity"],
        json!([{"propertyNames": ["speed"]}]),
    )])
    .await;
    let (status, _, body) = get(&st, &format!("/ngsi-ld/v1/entities/{VEHICLE_1}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let asked = peer.take();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0].2.contains(&("type".to_owned(), "*".to_owned())),
        "{:?}",
        asked[0].2
    );
}

/// clause 4.3.6, step 1: an Append whose fragment names a type outside the registration's
/// EntityInfo is not that source's data: nothing of it is forwarded.
#[tokio::test(flavor = "multi_thread")]
async fn an_append_naming_a_type_outside_the_registration_is_not_forwarded() {
    let (st, peer) = scripted(&[registration(
        "vehicles",
        "http://peer-a.invalid",
        &["appendAttrs"],
        json!([{"entities": [{"id": VEHICLE_1, "type": "Vehicle"}]}]),
    )])
    .await;
    let (status, body) = write(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        &json!({"id": VEHICLE_1, "type": "Vehicle"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    peer.take();
    let (status, body) = write(
        &st,
        "POST",
        &format!("/ngsi-ld/v1/entities/{VEHICLE_1}/attrs"),
        &json!({"type": "Car", "speed": {"type": "Property", "value": 1}}),
    )
    .await;
    assert!(status.as_u16() < 500, "{status} {body}");
    assert!(peer.take().is_empty(), "nothing went to the source");
}

/// clause 5.6.12, step 4: an append forwarded to an inclusive source of an Entity this
/// broker holds no history of reports the local half as not found and the
/// forwarded half as the source answered.
#[tokio::test(flavor = "multi_thread")]
async fn a_temporal_append_held_only_by_a_source_is_not_found_locally() {
    let (st, peer) = scripted(&[registration(
        "history",
        "http://peer-a.invalid",
        &["appendAttrsTemporal"],
        json!([{"entities": [{"id": VEHICLE_1, "type": "Vehicle"}]}]),
    )])
    .await;
    peer.say(204, Value::Null);
    let (status, body) = write(
        &st,
        "POST",
        &format!("/ngsi-ld/v1/temporal/entities/{VEHICLE_1}/attrs"),
        &json!({"speed": [{"type": "Property", "value": 1,
                           "observedAt": "2026-01-01T00:00:00Z"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert!(body.to_string().contains("not found locally"), "{body}");
    assert_eq!(peer.take().len(), 1, "the source was asked");
}

/// clause 5.14.4, step 4: with splitEntities=true a peer's EntityMap is asked without the
/// value filters, which are applied on the merged Entities.
#[tokio::test(flavor = "multi_thread")]
async fn a_split_entity_map_is_asked_without_its_value_filters() {
    let (st, peer) = scripted(&[registration(
        "maps",
        "http://peer-a.invalid",
        &["createEntityMapQueryEntity"],
        json!([{"entities": [{"type": "Vehicle"}]}]),
    )])
    .await;
    peer.say(
        200,
        json!({"id": "urn:ngsi-ld:EntityMap:peer", "type": "EntityMap", "entityMap": {}}),
    );
    let (status, _, body) = get(
        &st,
        "/ngsi-ld/v1/entityMaps?type=Vehicle&q=speed%3E1&splitEntities=true",
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    let asked = peer.take();
    assert_eq!(asked.len(), 1);
    assert!(
        !asked[0].2.iter().any(|(k, _)| k == "q"),
        "{:?}",
        asked[0].2
    );
    assert!(
        asked[0]
            .2
            .contains(&("type".to_owned(), "Vehicle".to_owned())),
        "{:?}",
        asked[0].2
    );
}

/// clause 4.5.23, step 1: a Relationship whose objectType a registration holds joins its
/// target through that registration, an auxiliary one too when it is the
/// only source.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_held_only_by_an_auxiliary_source_joins_its_answer() {
    let person = "urn:ngsi-ld:Person:1";
    let mut aux = registration(
        "people",
        "http://peer-a.invalid",
        &["retrieveEntity"],
        json!([{"entities": [{"id": person, "type": "Person"}]}]),
    );
    aux["mode"] = json!("auxiliary");
    let (st, peer) = scripted(&[aux]).await;
    let (status, body) = write(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        &json!({"id": VEHICLE_1, "type": "Vehicle",
                "owner": {"type": "Relationship", "object": person, "objectType": "Person"}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    peer.say(
        200,
        json!({"id": person, "type": "Person", "name": {"type": "Property", "value": "P"}}),
    );
    let (status, _, body) = get(
        &st,
        &format!("/ngsi-ld/v1/entities/{VEHICLE_1}?join=inline&joinLevel=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["owner"]["entity"]["name"]["value"], "P", "{body}");
}

/// clause 4.5.23, step 1: a link target its only source fails to answer is left
/// unjoined: the Entity itself still answers, with the Relationship as stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_its_source_fails_to_answer_stays_unjoined() {
    let person = "urn:ngsi-ld:Person:2";
    let mut aux = registration(
        "people",
        "http://peer-a.invalid",
        &["retrieveEntity"],
        json!([{"entities": [{"id": person, "type": "Person"}]}]),
    );
    aux["mode"] = json!("auxiliary");
    let (st, peer) = scripted(&[aux]).await;
    let (status, body) = write(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        &json!({"id": VEHICLE_1, "type": "Vehicle",
                "owner": {"type": "Relationship", "object": person, "objectType": "Person"}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    peer.say(
        500,
        json!({"type": "https://uri.etsi.org/ngsi-ld/errors/InternalError", "title": "down"}),
    );
    let (status, _, body) = get(
        &st,
        &format!("/ngsi-ld/v1/entities/{VEHICLE_1}?join=inline&joinLevel=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["owner"]["object"], person, "{body}");
    assert!(body["owner"].get("entity").is_none(), "{body}");
}

/// clause 4.5.23, step 1: a join whose registration read fails, for the target's type or
/// for the target's own id, leaves the link unjoined; the Entity still answers.
#[tokio::test(flavor = "multi_thread")]
async fn a_join_whose_registration_read_fails_leaves_the_link_unjoined() {
    let person = "urn:ngsi-ld:Person:3";
    let reg = registration(
        "people",
        "http://peer-a.invalid",
        &["retrieveEntity"],
        json!([{"entities": [{"id": person, "type": "Person"}]}]),
    );
    for failing in ["matching_registrations_typed", "matching_registrations"] {
        let (mut st, peer) = scripted(std::slice::from_ref(&reg)).await;
        let (status, body) = write(
            &st,
            "POST",
            "/ngsi-ld/v1/entities",
            &json!({"id": VEHICLE_1, "type": "Vehicle",
                    "owner": {"type": "Relationship", "object": person, "objectType": "Person"}}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        peer.say(200, json!({"id": person, "type": "Person"}));
        // the Entity's own read by id is the first; the join's are the later ones
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&reads);
        st.store = Arc::new(Double::faulting(st.store.clone(), move |method, _| {
            method == failing
                && (failing.ends_with("_typed")
                    || counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0)
        }));
        // the registrations are read from the store, not from the wired mirror
        st.reg_mirror = None;
        let (status, _, body) = get(
            &st,
            &format!("/ngsi-ld/v1/entities/{VEHICLE_1}?join=inline&joinLevel=1"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{failing}: {body}");
        assert_eq!(body["owner"]["object"], person, "{failing}: {body}");
        assert!(body["owner"].get("entity").is_none(), "{failing}: {body}");
    }
}

/// T-7306: a row that names a Tenant of another Organization, written before
/// that rule, forwards nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_naming_another_organizations_tenant_forwards_nothing() {
    let (st, peer) = unwired();
    store_registration(
        &st,
        "foreign",
        json!({"operations": ["queryEntity"], "tenant": "sk_b__x"}),
        json!([{"entities": [{"type": "Vehicle"}]}]),
    )
    .await;
    let (status, _, body) = get(&st, "/ngsi-ld/v1/entities?type=Vehicle").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(peer.take().is_empty(), "nothing was forwarded");
}

/// ETSI IOP_003_02_02: one source registered twice, (OffStreetParking ×
/// totalSpotsNumber) and (Vehicle × all). Its OffStreetParking contributes
/// totalSpotsNumber only; its `speed` belongs to no registration.
#[tokio::test(flavor = "multi_thread")]
async fn two_registrations_of_one_source_do_not_cross_their_scopes() {
    let body = query_through(&[
        json!([{"entities": [{"type": "OffStreetParking"}], "propertyNames": ["totalSpotsNumber"]}]),
        json!([{"entities": [{"type": "Vehicle"}]}]),
    ])
    .await;
    let parking = body
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["id"] == "urn:ngsi-ld:OffStreetParking:1")
        .unwrap_or_else(|| panic!("{body}"));
    assert!(parking.get("totalSpotsNumber").is_some(), "{body}");
    assert!(parking.get("speed").is_none(), "{body}");
}
