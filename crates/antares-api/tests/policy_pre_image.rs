// SPDX-License-Identifier: EUPL-1.2
//! What a write by id hands the policy engine about the Entity it lands on
//! (ADR-0023 rule 7, operation matrix item 2, T-3483).
//!
//! An update, a delete or a replace names an id and, at most, the members
//! it changes; ADR-N-041 forbids reading a type or a right off the URN. So
//! the gate hands the engine the stored Entity's image (types, scope,
//! Attribute names, expanded) as `Operation::stored`, and an engine judges
//! a write by what the Entity is, not by what the caller says it is.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, StoredImage, Subject,
};
use antares_api::AppState;
use antares_sql::store::any::AnyStore;
use antares_sql::store::Store;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
const VEHICLE: &str = "urn:ngsi-ld:Vehicle:v";
const SECRET: &str = "urn:ngsi-ld:Secret:s";
const ABSENT: &str = "urn:ngsi-ld:Vehicle:absent";

type Log = Arc<Mutex<Vec<(String, Vec<String>, Option<StoredImage>)>>>;

/// Records the clause, the ids and the stored image of every decision;
/// refuses a write whose STORED Entity is a `Secret`, whatever the request
/// names, so the refusal can only come from the pre-image.
struct Recording(Log);

impl PolicyEngine for Recording {
    fn name(&self) -> &str {
        "recording"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        if let Ok(mut v) = self.0.lock() {
            v.push((
                op.clause.to_owned(),
                op.ids.iter().map(|i| (*i).to_owned()).collect(),
                op.stored.cloned(),
            ));
        }
        let secret = format!("{DC}Secret");
        Box::pin(std::future::ready(
            match op.stored.is_some_and(|s| s.types.contains(&secret)) {
                true => Decision::Deny("the stored Entity is a Secret".into()),
                false => Decision::Allow,
            },
        ))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn state() -> (AppState, Log) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut st = AppState::new("me".into()).with_policy(Arc::new(Recording(Arc::clone(&seen))));
    antares_api::wire(&mut st).await;
    (st, seen)
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> StatusCode {
    let mut b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len());
            b.body(Body::from(payload)).expect("request")
        }
        None => b.body(Body::empty()).expect("request"),
    };
    antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response")
        .status()
}

/// A scoped Vehicle every write below lands on, and the Secret the engine
/// guards.
async fn seed(st: &AppState) {
    for doc in [
        json!({"id": VEHICLE, "type": "Vehicle", "scope": "/hel/a",
               "speed": {"type": "Property", "value": 1},
               "colour": {"type": "Property", "value": "red"}}),
        json!({"id": SECRET, "type": "Secret",
               "code": {"type": "Property", "value": 7}}),
    ] {
        assert_eq!(
            send(st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await,
            StatusCode::CREATED
        );
    }
}

fn vehicle_image() -> StoredImage {
    StoredImage {
        types: vec![format!("{DC}Vehicle")],
        scope: vec!["/hel/a".into()],
        attribute_names: vec![format!("{DC}colour"), format!("{DC}speed")],
    }
}

/// The image the engine was shown for the last decision on `clause`, with
/// its Attribute names sorted (a document's member order is the store's).
fn image_for(seen: &Log, clause: &str) -> Option<StoredImage> {
    let log = seen.lock().unwrap();
    let (_, _, stored) = log
        .iter()
        .rev()
        .find(|(c, _, _)| c == clause)
        .unwrap_or_else(|| panic!("the engine was not asked about {clause}: {log:?}"));
    stored.clone().map(|mut s| {
        s.attribute_names.sort();
        s
    })
}

/// Every write by id on an existing Entity shows the engine its stored
/// types, scope and Attribute names, whatever the request body carries.
#[tokio::test(flavor = "multi_thread")]
async fn every_write_by_id_shows_the_engine_the_stored_entity() {
    let attrs = format!("/ngsi-ld/v1/entities/{VEHICLE}/attrs");
    let property = |v: Value| json!({"type": "Property", "value": v});
    for (clause, method, uri, body) in [
        (
            "5.6.2",
            "PATCH",
            attrs.clone(),
            Some(json!({"speed": property(json!(2))})),
        ),
        (
            "5.6.3",
            "POST",
            attrs.clone(),
            Some(json!({"weight": property(json!(3))})),
        ),
        (
            "5.6.4",
            "PATCH",
            format!("{attrs}/speed"),
            Some(property(json!(4))),
        ),
        (
            "5.6.19",
            "PUT",
            format!("{attrs}/speed"),
            Some(json!({"speed": property(json!(5))})),
        ),
        (
            "5.6.17",
            "PATCH",
            format!("/ngsi-ld/v1/entities/{VEHICLE}"),
            Some(json!({"speed": property(json!(6))})),
        ),
        (
            "5.6.18",
            "PUT",
            format!("/ngsi-ld/v1/entities/{VEHICLE}"),
            Some(json!({"id": VEHICLE, "type": "Vehicle", "speed": property(json!(7))})),
        ),
        // Set Attribute Value (TS 104 175 10.2.6) is judged as Replace Attribute
        (
            "5.6.19",
            "PUT",
            format!("{attrs}/speed/value"),
            Some(json!(8)),
        ),
        ("5.6.5", "DELETE", format!("{attrs}/colour"), None),
        (
            "5.6.6",
            "DELETE",
            format!("/ngsi-ld/v1/entities/{VEHICLE}"),
            None,
        ),
    ] {
        // a fresh Entity each time: a replace or a delete changes it
        let (st, seen) = state().await;
        seed(&st).await;
        let code = send(&st, method, &uri, body).await;
        assert!(code.is_success(), "{clause} {method} {uri}: {code}");
        assert_eq!(
            image_for(&seen, clause),
            Some(vehicle_image()),
            "{clause} did not show the engine the stored Entity"
        );
    }
}

/// An engine that refuses by the stored type refuses the write even when
/// the request names another type, and the store is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn an_engine_refusing_on_the_stored_type_leaves_the_store_unchanged() {
    let (st, _) = state().await;
    seed(&st).await;
    let entity = format!("/ngsi-ld/v1/entities/{SECRET}");
    assert_eq!(
        send(&st, "DELETE", &entity, None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &st,
            "PUT",
            &entity,
            Some(json!({"id": SECRET, "type": "Vehicle",
                        "code": {"type": "Property", "value": 0}})),
        )
        .await,
        StatusCode::FORBIDDEN,
        "a body naming another type does not move the Entity out of the engine's rule"
    );
    assert_eq!(
        send(&st, "DELETE", &format!("{entity}/attrs/code"), None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &st,
            "PUT",
            &format!("{entity}/attrs/code/value"),
            Some(json!(0))
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&st, "GET", &entity, None).await,
        StatusCode::OK,
        "the Secret was removed"
    );
}

/// A write to an id the tenant does not hold hands the engine no image and
/// answers as it always did.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_to_an_absent_id_hands_no_image_and_answers_as_before() {
    let (st, seen) = state().await;
    seed(&st).await;
    assert_eq!(
        send(
            &st,
            "PATCH",
            &format!("/ngsi-ld/v1/entities/{ABSENT}/attrs"),
            Some(json!({"speed": {"type": "Property", "value": 1}})),
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(image_for(&seen, "5.6.2"), None);
    assert_eq!(
        send(
            &st,
            "DELETE",
            &format!("/ngsi-ld/v1/entities/{ABSENT}"),
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(image_for(&seen, "5.6.6"), None);
}

/// Each member of a batch carries its own image: the stored one where the
/// id exists, none where it does not.
#[tokio::test(flavor = "multi_thread")]
async fn each_batch_member_carries_its_own_image() {
    // TS 104 176 7.8.3.1: 201 when the absent id was created and the
    // stored one updated; 7.11.3.1: 207 when an id did not exist.
    for (clause, path, body, want) in [
        (
            "5.6.8",
            "upsert",
            json!([
                {"id": VEHICLE, "type": "Vehicle", "speed": {"type": "Property", "value": 9}},
                {"id": ABSENT, "type": "Vehicle", "speed": {"type": "Property", "value": 9}},
            ]),
            StatusCode::CREATED,
        ),
        (
            "5.6.10",
            "delete",
            json!([VEHICLE, ABSENT]),
            StatusCode::MULTI_STATUS,
        ),
    ] {
        let (st, seen) = state().await;
        seed(&st).await;
        let code = send(
            &st,
            "POST",
            &format!("/ngsi-ld/v1/entityOperations/{path}"),
            Some(body),
        )
        .await;
        assert_eq!(code, want, "{path}");
        let log = seen.lock().unwrap();
        let images: Vec<(String, Option<Vec<String>>)> = log
            .iter()
            .filter(|(c, _, _)| c == clause)
            .map(|(_, ids, stored)| {
                (
                    ids.first().cloned().unwrap_or_default(),
                    stored.as_ref().map(|s| s.types.clone()),
                )
            })
            .collect();
        assert_eq!(
            images,
            vec![
                (VEHICLE.to_owned(), Some(vec![format!("{DC}Vehicle")])),
                (ABSENT.to_owned(), None),
            ],
            "{path}"
        );
    }
}

/// Without an engine the writes answer exactly as before: nothing reads
/// a pre-image, nothing refuses.
#[tokio::test(flavor = "multi_thread")]
async fn allow_all_answers_unchanged() {
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    seed(&st).await;
    assert_eq!(
        send(
            &st,
            "PATCH",
            &format!("/ngsi-ld/v1/entities/{SECRET}/attrs"),
            Some(json!({"code": {"type": "Property", "value": 8}})),
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &st,
            "DELETE",
            &format!("/ngsi-ld/v1/entities/{SECRET}"),
            None
        )
        .await,
        StatusCode::NO_CONTENT
    );
}

/// The temporal writes by id (5.6.12 to 5.6.16) need the temporal store,
/// so their state is wired with the memory drivers.
async fn temporal_state() -> (AppState, Log) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(AnyStore::Mem(Store::default()));
    let mut st = AppState::with_drivers("me".into(), store.clone(), store, "memory")
        .with_policy(Arc::new(Recording(Arc::clone(&seen))));
    antares_api::wire(&mut st).await;
    (st, seen)
}

/// A Vehicle with a current state and a history, and a Secret held only as
/// a temporal evolution: no current state to read its type from.
async fn seed_temporal(st: &AppState) {
    seed(st).await;
    let at = |v: i64, t: &str| json!({"type": "Property", "value": v, "observedAt": t});
    for doc in [
        json!({"id": VEHICLE, "type": "Vehicle", "scope": "/hel/a",
               "speed": [at(1, "2026-01-01T00:00:00Z"), at(2, "2026-01-02T00:00:00Z")],
               "colour": {"type": "Property", "value": "red", "observedAt": "2026-01-01T00:00:00Z"}}),
        json!({"id": "urn:ngsi-ld:Secret:t", "type": "Secret",
               "code": [at(7, "2026-01-01T00:00:00Z")]}),
    ] {
        let code = send(st, "POST", "/ngsi-ld/v1/temporal/entities", Some(doc)).await;
        assert!(code.is_success(), "temporal seed: {code}");
    }
}

/// The instanceId of the first `attr` instance the temporal store holds.
async fn first_instance(st: &AppState, id: &str, attr: &str) -> String {
    let res = antares_api::router(st.clone())
        .oneshot(
            Request::get(format!("/ngsi-ld/v1/temporal/entities/{id}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let doc: Value = serde_json::from_slice(&bytes).expect("json");
    doc[attr][0]["instanceId"]
        .as_str()
        .unwrap_or_else(|| panic!("no instanceId on {attr}: {doc}"))
        .to_owned()
}

/// Every temporal write by id shows the engine what the Entity is, as the
/// entity writes do (T-3607): a type-keyed rule cannot be stepped around
/// through the temporal API.
#[tokio::test(flavor = "multi_thread")]
async fn every_temporal_write_by_id_shows_the_engine_the_stored_entity() {
    let base = format!("/ngsi-ld/v1/temporal/entities/{VEHICLE}");
    for clause in ["5.6.12", "5.6.13", "5.6.14", "5.6.15", "5.6.16"] {
        let (st, seen) = temporal_state().await;
        seed_temporal(&st).await;
        let instance = first_instance(&st, VEHICLE, "speed").await;
        let (method, uri, body) = match clause {
            "5.6.12" => (
                "POST",
                format!("{base}/attrs"),
                Some(json!({"speed": [{"type": "Property", "value": 3,
                                       "observedAt": "2026-01-03T00:00:00Z"}]})),
            ),
            "5.6.13" => ("DELETE", format!("{base}/attrs/colour"), None),
            "5.6.14" => (
                "PATCH",
                format!("{base}/attrs/speed/{instance}"),
                Some(json!({"speed": {"type": "Property", "value": 5,
                                      "observedAt": "2026-01-01T00:00:00Z"}})),
            ),
            "5.6.15" => ("DELETE", format!("{base}/attrs/speed/{instance}"), None),
            _ => ("DELETE", base.clone(), None),
        };
        let code = send(&st, method, &uri, body).await;
        assert!(code.is_success(), "{clause} {method} {uri}: {code}");
        let image = image_for(&seen, clause).unwrap_or_else(|| panic!("{clause}: no image"));
        assert_eq!(image.types, vec![format!("{DC}Vehicle")], "{clause}");
        assert_eq!(image.scope, vec!["/hel/a".to_owned()], "{clause}");
    }
}

/// An engine that refuses by the stored type refuses a temporal append to a
/// Secret held only as history; an Entity it allows is still written.
#[tokio::test(flavor = "multi_thread")]
async fn an_engine_refusing_on_the_stored_type_refuses_the_temporal_write() {
    let (st, _) = temporal_state().await;
    seed_temporal(&st).await;
    let fragment = json!({"code": [{"type": "Property", "value": 0,
                                    "observedAt": "2026-01-05T00:00:00Z"}]});
    assert_eq!(
        send(
            &st,
            "POST",
            "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Secret:t/attrs",
            Some(fragment),
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &st,
            "DELETE",
            "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Secret:t",
            None
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &st,
            "DELETE",
            &format!("/ngsi-ld/v1/temporal/entities/{VEHICLE}"),
            None
        )
        .await,
        StatusCode::NO_CONTENT
    );
}
