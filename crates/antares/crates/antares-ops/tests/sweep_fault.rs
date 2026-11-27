// SPDX-License-Identifier: EUPL-1.2
//! The quality and retention sweeps over a store that fails one call: the
//! sweep logs, skips what it could not read or write, and goes on with the
//! next sweep. A process of its own: the platform default is read from the
//! environment when the broker installs.

#[path = "support/fault_store.rs"]
mod fault_store;

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use fault_store::FaultStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const V: &str = "https://example.org/Vehicle";
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
