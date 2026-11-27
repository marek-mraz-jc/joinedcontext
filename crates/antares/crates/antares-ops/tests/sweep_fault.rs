// SPDX-License-Identifier: EUPL-1.2
//! The quality and retention sweeps over a store that fails one call: the
//! sweep logs, skips what it could not read or write, and goes on with the
//! next sweep. A process of its own: the platform default is read from the
//! environment when the broker installs.

#[path = "support/fault_store.rs"]
mod fault_store;

use antares_model::{NgsiError, TenantId};
use antares_ops::test_kit::TestBroker;
use antares_ops::{BuiltForward, ForwardFuture, ForwardTransport, Ops};
use antares_sql::store::{any::AnyStore, Store};
use antares_store::{Kind, MutateFn, TemporalDriver};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use fault_store::FaultStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const V: &str = "https://example.org/Vehicle";
const OWN: &str = antares_store::HISTORY_RETENTION;
const LOCATION: &str = "https://example.org/position";
const RULES: &str = r#"{"rules":[{"name":"stale-position","type":"https://example.org/Vehicle",
    "attribute":"https://example.org/position","maxAge":"PT10M"}]}"#;

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: Option<&str>,
    body: Option<String>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    if let Some(b) = &body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default()))
                .unwrap_or_default(),
        )
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn ago(minutes: i64) -> String {
    let now = jc_time::TimestampTz::now();
    now.checked_sub(jiff::SignedDuration::from_mins(minutes))
        .unwrap_or(now)
        .to_rfc3339_opts(jc_time::SecondsFormat::Millis, true)
}

/// A broker over the fault store, the quality rule of `city`, and a stale
/// Vehicle there.
async fn seeded() -> Result<(TestBroker, Router, Arc<FaultStore>), String> {
    std::env::set_var(antares_ops::views::HISTORY_DEFAULT_ENV, "PT10M");
    let (state, fault) = FaultStore::state("antares-test");
    let broker = TestBroker::over(state).await?;
    let app = broker.serve_mounted(Vec::new())?;
    let (status, body) = send(
        &app,
        Method::PUT,
        "/q/quality/city",
        None,
        Some(RULES.into()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let car = json!({"id": "urn:v:old", "type": V,
        LOCATION: {"type": "Property", "value": 1, "observedAt": ago(30)}});
    let (status, body) = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some("city"),
        Some(car.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Ok((broker, app, fault))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quality_sweep_the_store_cannot_serve_changes_nothing_and_runs_again(
) -> Result<(), String> {
    let (broker, _app, fault) = seeded().await?;

    // the write of the mark fails: nothing is marked
    fault.fail(|method, _, _| method == "batch_mutate_boxed");
    assert_eq!(broker.sweep_quality(jiff::Timestamp::now()).await.len(), 0);

    // healed: the next sweep marks it
    fault.fail(|_, _, _| false);
    let changed = broker.sweep_quality(jiff::Timestamp::now()).await;
    assert_eq!(changed.len(), 1, "{changed:?}");

    // the read of the Entities fails, for the rule and for the orphan lift
    fault.fail(|method, _, _| method == "query_entities");
    assert_eq!(broker.sweep_quality(jiff::Timestamp::now()).await.len(), 0);

    // a catalogue that cannot answer is scanned all the same
    fault.fail(|method, _, _| method == "entity_attr_pairs");
    assert_eq!(broker.sweep_quality(jiff::Timestamp::now()).await.len(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retention_sweep_the_store_cannot_serve_deletes_nothing_and_runs_again(
) -> Result<(), String> {
    let (broker, _app, fault) = seeded().await?;

    // the Entities with their own retention cannot be read
    fault.fail(|method, _, _| method == "history_overrides");
    assert_eq!(
        broker.sweep_retention(jiff::Timestamp::now()).await.len(),
        0
    );

    // the Tenants the platform default reaches cannot be listed
    fault.fail(|method, _, _| method == "tenant_ids");
    assert_eq!(
        broker.sweep_retention(jiff::Timestamp::now()).await.len(),
        0
    );

    // healed: the sweep runs
    fault.fail(|_, _, _| false);
    let _ = broker.sweep_retention(jiff::Timestamp::now()).await;
    Ok(())
}

/// A temporal driver with no history whose audit sweep answers what a test
/// sets: `Some(n)` deleted `n` rows, `None` fails.
struct AuditStub {
    answer: std::sync::Mutex<Option<u64>>,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl TemporalDriver for AuditStub {
    fn supported(&self) -> bool {
        false
    }
    async fn retain_audit(&self, _before: &str, _batch: usize) -> Result<u64, NgsiError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let answer = *self
            .answer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        answer.ok_or_else(|| NgsiError::InternalError("injected".into()))
    }
    async fn temporal_append(
        &self,
        t: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        let _ = (t, id, shell, additions);
        Ok(())
    }
    async fn query_temporal(
        &self,
        t: &TenantId,
        f: &antares_store::filter::TemporalFilter<'_>,
    ) -> Result<antares_store::filter::TemporalOutcome, NgsiError> {
        let _ = (t, f);
        Ok(antares_store::filter::TemporalOutcome {
            rows: Vec::new(),
            paged: false,
            total: None,
            aggregated: false,
        })
    }
    async fn get_temporal(
        &self,
        t: &TenantId,
        id: &str,
        f: &antares_store::filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        let _ = (t, id, f);
        Ok(None)
    }
    async fn get(&self, _t: &TenantId, _id: &str) -> Result<Option<Value>, NgsiError> {
        Ok(None)
    }
    async fn create(&self, _t: &TenantId, _id: &str, _doc: Value) -> Result<bool, NgsiError> {
        Ok(true)
    }
    async fn upsert(&self, _t: &TenantId, _id: &str, _doc: Value) -> Result<bool, NgsiError> {
        Ok(false)
    }
    async fn delete(&self, _t: &TenantId, _id: &str) -> Result<bool, NgsiError> {
        Ok(false)
    }
    async fn list(&self, _t: &TenantId) -> Result<Vec<Value>, NgsiError> {
        Ok(Vec::new())
    }
    async fn mutate_boxed<'a>(
        &self,
        _t: &TenantId,
        _id: &str,
        _f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        Ok(None)
    }
}

/// A broker over a memory store whose history is the [`AuditStub`].
async fn stubbed() -> Result<(TestBroker, Arc<AuditStub>, Arc<AnyStore>), String> {
    let store = Arc::new(AnyStore::Mem(Store::default()));
    let stub = Arc::new(AuditStub {
        answer: std::sync::Mutex::new(Some(0)),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let state = antares_api::AppState::with_drivers(
        "antares-test".into(),
        store.clone(),
        stub.clone(),
        "memory",
    );
    Ok((TestBroker::over(state).await?, stub, store))
}

/// The audit sweep runs on every retention sweep: rows it deleted are
/// counted, and a store that fails it is logged and tried again.
#[tokio::test(flavor = "multi_thread")]
async fn the_audit_sweep_counts_what_went_and_survives_a_failing_store() -> Result<(), String> {
    let (broker, stub, _store) = stubbed().await?;
    for answer in [Some(3), None, Some(0)] {
        *stub
            .answer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = answer;
        let _ = broker.sweep_retention(jiff::Timestamp::now()).await;
    }
    assert_eq!(stub.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    Ok(())
}

/// An Entity whose own `historyRetention` is no duration is left to its
/// type's rule; the sweep goes on with the others.
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_with_an_unreadable_own_retention_is_left_to_its_type() -> Result<(), String> {
    let (broker, _stub, store) = stubbed().await?;
    let tenant = TenantId::new("city").map_err(|e| e.to_string())?;
    let items = [("urn:o:bad", "sometimes"), ("urn:o:good", "P1D")]
        .into_iter()
        .map(|(id, value)| {
            (
                id.to_owned(),
                json!({"id": id, "type": [V],
                    OWN: {"type": "Property", "value": value}}),
            )
        })
        .collect();
    antares_store::CurrentStateDriver::batch_create(&*store, &tenant, items)
        .await
        .map_err(|e| e.to_string())?;
    let _ = broker.sweep_retention(jiff::Timestamp::now()).await;
    Ok(())
}

/// More Entities with their own retention than one sweep reads: only their
/// own retention runs, the types' rules wait for the next sweep.
#[tokio::test(flavor = "multi_thread")]
async fn more_own_retentions_than_one_sweep_reads_still_sweep() -> Result<(), String> {
    let (broker, _stub, store) = stubbed().await?;
    let tenant = TenantId::new("city").map_err(|e| e.to_string())?;
    for chunk in 0..10 {
        let items = (0..1000)
            .map(|n| {
                let id = format!("urn:o:{chunk}:{n}");
                (
                    id.clone(),
                    json!({"id": id, "type": [V],
                        OWN: {"type": "Property", "value": "P1D"}}),
                )
            })
            .collect();
        antares_store::CurrentStateDriver::batch_create(&*store, &tenant, items)
            .await
            .map_err(|e| e.to_string())?;
    }
    let _ = broker.sweep_retention(jiff::Timestamp::now()).await;
    Ok(())
}

/// Serves no forward; the test keeps the `Ops` it was handed.
struct Keep;

impl ForwardTransport for Keep {
    fn forward<'a>(&'a self, _req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        None
    }
}

/// A view of every type whose Tenant's type list the store cannot give
/// pushes no `type=`, but the whole local store.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_of_every_type_pushes_the_local_store_when_the_type_list_fails() -> Result<(), String>
{
    let (state, fault) = FaultStore::state("antares-test");
    let kept: Arc<std::sync::Mutex<Option<Ops>>> = Arc::default();
    let slot = kept.clone();
    let broker = TestBroker::over(state)
        .await?
        .with_forward_transport(move |ops| {
            *slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ops);
            Arc::new(Keep)
        })?;
    let every = json!({"id": "every", "tenant": "city", "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": ["query_entities"]});
    broker.store_view_doc("every", every).await?;
    let ops = kept
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or("the transport is made with the broker")?;
    let view = ops.view("every").ok_or("the view loaded")?;
    fault.fail(|method, _, _| method == "entity_type_names");
    assert_eq!(
        view.type_selector(&["Vehicle"]).await.as_deref(),
        Some("local=true")
    );
    Ok(())
}

/// A store beside a temporal driver that keeps no history passes the Tenant isolation walk:
/// the history half of it has nothing to walk.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_without_history_passes_the_tenant_isolation_contract() -> Result<(), String> {
    let (_broker, fault, stub) = isolated().await?;
    let (a, b) = tenants()?;
    antares_store::contract::run_tenant_isolation_contract(&*fault, &*stub, &a, &b, "iso").await;
    Ok(())
}

/// A store that does not offer a Tenant purge passes the walk: its rows of `a` are
/// deleted one by one instead, and `b` stays whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_without_a_tenant_purge_passes_the_tenant_isolation_contract() -> Result<(), String>
{
    let (_broker, fault, stub) = isolated().await?;
    let (a, b) = tenants()?;
    *fault
        .unsupported
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some("purge_tenant");
    antares_store::contract::run_tenant_isolation_contract(&*fault, &*stub, &a, &b, "isonp").await;
    Ok(())
}

/// A purge the store fails for a reason other than not supporting it fails the walk.
#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "purge_tenant")]
async fn a_failing_purge_fails_the_tenant_isolation_contract() {
    let (_broker, fault, stub) = isolated().await.expect("a broker");
    let (a, b) = tenants().expect("tenants");
    fault.fail(|method, _, _| method == "purge_tenant");
    antares_store::contract::run_tenant_isolation_contract(&*fault, &*stub, &a, &b, "isofail")
        .await;
}

fn tenants() -> Result<(TenantId, TenantId), String> {
    let name = |n: &str| TenantId::new(n).map_err(|e| e.to_string());
    Ok((name("iso-a")?, name("iso-b")?))
}

/// The fault store beside an [`AuditStub`], which keeps no history.
async fn isolated() -> Result<(TestBroker, Arc<FaultStore>, Arc<AuditStub>), String> {
    let (state, fault) = FaultStore::state("antares-test");
    let stub = Arc::new(AuditStub {
        answer: std::sync::Mutex::new(Some(0)),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    Ok((TestBroker::over(state).await?, fault, stub))
}

/// A view document the store cannot read is a 500 that names no cause: the
/// store's own message stays in the log.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_the_store_cannot_read_answers_500_without_the_cause() -> Result<(), String> {
    let (_broker, app, fault) = seeded().await?;
    fault.fail(|method, _, kind| method == "get" && kind == Some(Kind::View));
    let (status, body) = send(&app, Method::GET, "/q/views/town", None, None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["detail"], "the view store failed", "{body}");
    assert!(!body.to_string().contains("injected"), "{body}");
    fault.fail(|_, _, _| false);
    let (status, _) = send(&app, Method::GET, "/q/views/town", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

/// The minute loop sweeps by itself, and a reload the store fails leaves
/// the table as it was; a clock that jumps runs a minute at once.
#[tokio::test(start_paused = true)]
async fn the_background_loops_sweep_and_survive_a_failing_view_store() -> Result<(), String> {
    let (_broker, app, fault) = seeded().await?;
    fault.fail(|method, _, _| method == "list_digests");
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    fault.fail(|_, _, _| false);
    tokio::time::sleep(std::time::Duration::from_secs(61)).await;
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    let (status, marked) = send(
        &app,
        Method::GET,
        "/ngsi-ld/v1/entities/urn:v:old",
        Some("city"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    assert_eq!(
        marked["quarantineRules"]["value"],
        json!(["stale-position"]),
        "the loop's sweep marked the stale Entity: {marked}"
    );
    Ok(())
}
