// SPDX-License-Identifier: EUPL-1.2
//! S4-22 and 5.16 at the exact step a fault lands: a fill or a clone whose
//! lease another fill takes between two of its steps stops and writes
//! nothing; a clone whose source read fails reads it again; an update whose
//! write fails is not reported as applied; an Evolution the temporal store
//! no longer holds is copied from the query's answer.

use crate::common::{self, DocCall};

use antares_api::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::{filter, MutateFn, TemporalDriver};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
        }
        None => b.body(Body::empty()),
    }
    .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A state over a store whose Snapshot calls `hook` steers.
async fn faulting(hook: common::DocHook) -> AppState {
    let mut st = AppState::new("antares-snap-faults".into());
    st.store = Arc::new(common::Double::faulting_docs(st.store.clone(), hook));
    antares_api::wire(&mut st).await;
    st
}

async fn status_of(st: &AppState, id: &str) -> Value {
    let (status, body) = send(st, "GET", &format!("/ngsi-ld/v1/snapshots/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The Snapshot once it left `preparing`.
async fn settled(st: &AppState, id: &str) -> Value {
    for _ in 0..200 {
        let body = status_of(st, id).await;
        if body["snapshotStatus"] != "preparing" {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("snapshot {id} never left preparing");
}

/// Wait until `calls` reached `n`.
async fn reached(calls: &AtomicUsize, n: usize) {
    for _ in 0..400 {
        if calls.load(Ordering::SeqCst) >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("only {} calls, {n} expected", calls.load(Ordering::SeqCst));
}

async fn vehicle(st: &AppState, id: &str) {
    let (status, body) = send(
        st,
        "POST",
        "/ngsi-ld/v1/temporal/entities",
        Some(json!({"id": id, "type": "Vehicle",
            "speed": [{"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"}]})),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    let (status, body) = send(
        st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": id, "type": "Vehicle"})),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
}

/// S4-22: the fill renews its lease before each query and before it writes
/// its outcome. Taken by another fill at any of those three steps, it stops
/// there: the Snapshot stays `preparing` under the other fill, and no
/// outcome is written over it.
#[tokio::test(flavor = "multi_thread")]
async fn a_fill_that_loses_its_lease_between_steps_writes_nothing() {
    // 1 = the claim at the start, 2 = before the query, 3 = before the
    // temporal query, 4 = before the outcome
    for step in [2usize, 3, 4] {
        let id = format!("urn:ngsi-ld:Snapshot:stolen-{step}");
        let calls = Arc::new(AtomicUsize::new(0));
        let (c, own) = (calls.clone(), id.clone());
        let st = faulting(Arc::new(move |call| match call {
            DocCall::Mutate(antares_store::Kind::Snapshot, i) if i == own => {
                c.fetch_add(1, Ordering::SeqCst) + 1 == step
            }
            _ => false,
        }))
        .await;
        vehicle(&st, "urn:ngsi-ld:Vehicle:s1").await;
        let (status, body) = send(
            &st,
            "POST",
            "/ngsi-ld/v1/snapshots",
            Some(json!({"id": id, "type": "Snapshot",
                "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}],
                "snapshotTemporalQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}],
                    "temporalQ": {"timerel": "after", "timeAt": "2000-01-01T00:00:00Z"}}]})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        reached(&calls, step).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            step,
            "the fill stopped at {step}"
        );
        let body = status_of(&st, &id).await;
        assert_eq!(body["snapshotStatus"], "preparing", "step {step}: {body}");
        assert!(body.get("snapshotQueriesDetails").is_none(), "{body}");
    }
}

/// S4-22 for a clone: one whose lease another fill takes while it copies
/// stops before writing its outcome.
#[tokio::test(flavor = "multi_thread")]
async fn a_clone_that_loses_its_lease_while_copying_writes_nothing() {
    let src = "urn:ngsi-ld:Snapshot:clone-src";
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    // the clone's claim is its first write, the check after the copy its second
    let st = faulting(Arc::new(move |call| match call {
        DocCall::Mutate(antares_store::Kind::Snapshot, "urn:ngsi-ld:Snapshot:clone-dst") => {
            c.fetch_add(1, Ordering::SeqCst) + 1 == 2
        }
        _ => false,
    }))
    .await;
    vehicle(&st, "urn:ngsi-ld:Vehicle:c1").await;
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/snapshots",
        Some(json!({"id": src, "type": "Snapshot",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(settled(&st, src).await["snapshotStatus"], "success");
    let (status, body) = send(
        &st,
        "POST",
        &format!("/ngsi-ld/v1/snapshots/{src}/clone"),
        Some(json!({"id": "urn:ngsi-ld:Snapshot:clone-dst"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    reached(&calls, 2).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "no outcome written");
    let body = status_of(&st, "urn:ngsi-ld:Snapshot:clone-dst").await;
    assert_eq!(body["snapshotStatus"], "preparing", "{body}");
}

/// clause 5.16.2, item 4 (S4-22): a source read that fails is read again at the next
/// poll, never taken for a deleted source — the clone still copies.
#[tokio::test(flavor = "multi_thread")]
async fn a_clone_reads_a_source_again_after_a_failed_read() {
    let src = "urn:ngsi-ld:Snapshot:flaky-src";
    let reads = Arc::new(AtomicUsize::new(0));
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (r, a) = (reads.clone(), armed.clone());
    // armed after the clone request's own read: the clone's first poll fails
    let st = faulting(Arc::new(move |call| match call {
        DocCall::Get(antares_store::Kind::Snapshot, i) if i == src && a.load(Ordering::SeqCst) => {
            r.fetch_add(1, Ordering::SeqCst) == 1
        }
        _ => false,
    }))
    .await;
    vehicle(&st, "urn:ngsi-ld:Vehicle:f1").await;
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/snapshots",
        Some(json!({"id": src, "type": "Snapshot",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(settled(&st, src).await["snapshotStatus"], "success");
    armed.store(true, Ordering::SeqCst);
    let (status, body) = send(
        &st,
        "POST",
        &format!("/ngsi-ld/v1/snapshots/{src}/clone"),
        Some(json!({"id": "urn:ngsi-ld:Snapshot:flaky-dst"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let done = settled(&st, "urn:ngsi-ld:Snapshot:flaky-dst").await;
    assert_eq!(done["snapshotStatus"], "success", "{done}");
    assert!(
        reads.load(Ordering::SeqCst) >= 3,
        "read, refused, read again"
    );
}

/// clause 5.16.4, item 4: an Update Snapshot whose write the store refuses is not
/// answered 200 — the Snapshot is reported as not there to update.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_whose_write_fails_is_not_applied() {
    let mut st = AppState::new("antares-snap-faults".into());
    let double = Arc::new(common::Double::passthrough(st.store.clone()));
    st.store = double.clone();
    antares_api::wire(&mut st).await;
    let id = "urn:ngsi-ld:Snapshot:unwritable";
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/snapshots",
        Some(json!({"id": id, "type": "Snapshot",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    settled(&st, id).await;
    double.fail_snapshot_writes(1);
    let (status, body) = send(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/snapshots/{id}"),
        Some(json!({"snapshotPriority": 9})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["detail"], format!("snapshot {id} not found"), "{body}");
    assert_ne!(status_of(&st, id).await["snapshotPriority"], 9);
}

/// A temporal driver whose Evolutions are listed by a query but read as
/// absent one by one: the Evolution went between the two reads.
struct Vanishing(Arc<dyn TemporalDriver>);

#[async_trait::async_trait]
impl TemporalDriver for Vanishing {
    async fn temporal_append(
        &self,
        t: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        self.0.temporal_append(t, id, shell, additions).await
    }
    async fn query_temporal(
        &self,
        t: &TenantId,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<filter::TemporalOutcome, NgsiError> {
        self.0.query_temporal(t, f).await
    }
    async fn get_temporal(
        &self,
        _: &TenantId,
        _: &str,
        _: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        Ok(None)
    }
    async fn get(&self, t: &TenantId, id: &str) -> Result<Option<Value>, NgsiError> {
        self.0.get(t, id).await
    }
    async fn create(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.0.create(t, id, doc).await
    }
    async fn upsert(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.0.upsert(t, id, doc).await
    }
    async fn delete(&self, t: &TenantId, id: &str) -> Result<bool, NgsiError> {
        self.0.delete(t, id).await
    }
    async fn list(&self, t: &TenantId) -> Result<Vec<Value>, NgsiError> {
        self.0.list(t).await
    }
    async fn mutate_boxed<'a>(
        &self,
        t: &TenantId,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.0.mutate_boxed(t, id, f).await
    }
}

/// clause 5.16.1, item 4: an Evolution the query listed but the store reads as absent
/// by the time it is copied is copied from the query's own answer, as one a
/// Context Source sent is.
#[tokio::test(flavor = "multi_thread")]
async fn an_evolution_gone_between_the_query_and_the_copy_is_copied_from_the_answer() {
    let mut st = AppState::new("antares-snap-faults".into());
    antares_api::wire(&mut st).await;
    vehicle(&st, "urn:ngsi-ld:Vehicle:v1").await;
    let real = st.temporal.clone();
    st.temporal = Arc::new(Vanishing(real.clone()));
    let id = "urn:ngsi-ld:Snapshot:vanished";
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/snapshots",
        Some(json!({"id": id, "type": "Snapshot",
            "snapshotTemporalQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}],
                "temporalQ": {"timerel": "after", "timeAt": "2000-01-01T00:00:00Z"}}]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let done = settled(&st, id).await;
    assert_eq!(done["snapshotStatus"], "success", "{done}");
    let snaps = st
        .store
        .list(&TenantId::default(), antares_store::Kind::Snapshot)
        .await
        .expect("list");
    let synth = snaps
        .iter()
        .find(|s| s["id"] == id)
        .and_then(|s| s["__tenant"].as_str())
        .expect("the copy's tenant");
    let copy = real
        .get(
            &TenantId::new_internal(synth).expect("synth"),
            "urn:ngsi-ld:Vehicle:v1",
        )
        .await
        .expect("store")
        .expect("the answer was copied");
    assert!(copy.to_string().contains("speed"), "{copy}");
}
