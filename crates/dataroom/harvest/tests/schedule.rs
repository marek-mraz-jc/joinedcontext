#![forbid(unsafe_code)]
//! Scheduled harvests as prod's CronJobs ran them: one run per source at a time (the Running
//! row is the lock), a deadline that stops a run and frees the source, up to three retries of a
//! failed run, and the catalogue's harvest principal as the only one allowed to run.

mod common;

use chrono::{TimeZone, Utc};
use common::*;
use dataroom_core::ops::HarvestRunner;
use dataroom_core::store::BoxFuture;
use dataroom_core::{DataroomError, HarvestRun, HarvestRunState, PolicyAuthorizer, Result, Store};
use dataroom_harvest::{
    harvest_principal, FetchResponse, HarvestEngine, HarvestScheduler, MapFetcher, OutboundFetcher,
    ValidatedTarget, RETRIES,
};
use dataroom_store_memory::MemoryStore;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const HUB: &str = "https://hub.city.example";
const HUB_PAGE1: &str =
    "https://hub.city.example/api/search/v1/collections/dataset/items?limit=100&sortBy=properties.created";

fn hub_answer() -> String {
    json!({"numberMatched": 1, "features": [hub_item("aaa111", "Cesty", 1)]}).to_string()
}

/// Fails the first `fail` fetches, then answers the Hub page; hangs instead when `hang`.
struct Flaky {
    fail: usize,
    hang: bool,
    calls: AtomicUsize,
}

impl OutboundFetcher for Flaky {
    fn fetch_hop<'a>(
        &'a self,
        _target: &'a ValidatedTarget,
        _accept: &'a str,
        _max_bytes: usize,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.hang {
                std::future::pending::<()>().await;
            }
            if n < self.fail {
                return Err(DataroomError::OutboundRefused("HTTP 503".into()));
            }
            Ok(FetchResponse::Ok {
                content_type: Some("application/json".into()),
                body: hub_answer(),
            })
        })
    }
}

fn engine_on(f: Arc<dyn OutboundFetcher>) -> HarvestEngine {
    HarvestEngine::new(f)
        .with_custom_dns(|_, port| Ok(vec![SocketAddr::from(([93, 184, 216, 34], port))]))
}

fn running(w: &World, id: &str, started: chrono::DateTime<Utc>) -> HarvestRun {
    serde_json::from_value(json!({
        "orgId": w.org, "catalogueId": w.cat, "id": id, "sourceId": "hs-hub",
        "state": "running", "startedAt": started, "createdCount": 0, "updatedCount": 0,
        "unchangedCount": 0, "withdrawnCount": 0, "skipped": [], "logs": []
    }))
    .expect("run")
}

#[tokio::test]
async fn a_source_with_a_live_running_run_is_refused_and_nothing_is_written() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let held = running(&w, "run-held", Utc::now());
    assert!(w
        .store
        .claim_harvest_run(
            &w.ctx,
            held.clone(),
            Utc::now() - chrono::Duration::hours(1)
        )
        .await
        .unwrap());
    // A second claim of the same source is refused while the first is live.
    assert!(!w
        .store
        .claim_harvest_run(
            &w.ctx,
            running(&w, "run-second", Utc::now()),
            Utc::now() - chrono::Duration::hours(1)
        )
        .await
        .unwrap());

    let f = MapFetcher::new();
    f.insert(HUB_PAGE1, Some("application/json"), hub_answer());
    let err = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::Conflict(_)), "{err}");
    assert!(
        f.pinned_seen().is_empty(),
        "no fetch while the source is held"
    );
    assert!(active_names(&w).await.is_empty());
    let runs = w
        .store
        .list_harvest_runs(&w.ctx, Some("hs-hub"))
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
}

#[tokio::test]
async fn a_running_run_past_the_deadline_is_marked_failed_and_frees_the_source() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let stale = running(&w, "run-stale", Utc::now() - chrono::Duration::hours(2));
    w.store.upsert_harvest_run(&w.ctx, stale).await.unwrap();

    let f = MapFetcher::new();
    f.insert(HUB_PAGE1, Some("application/json"), hub_answer());
    let run = engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("the stale run does not hold the source");
    assert_eq!(run.state, HarvestRunState::Succeeded);
    let old = w
        .store
        .get_harvest_run(&w.ctx, "run-stale")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.state, HarvestRunState::Failed);
    assert!(old.error.unwrap().contains("abandoned"));
}

#[tokio::test]
async fn a_run_past_its_deadline_is_stopped_recorded_failed_and_releases_the_lock() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let hang = Arc::new(Flaky {
        fail: 0,
        hang: true,
        calls: AtomicUsize::new(0),
    });
    let e = engine_on(hang).with_deadline(Duration::from_millis(50));
    let err = e.run_source(&w.ctx, &w.store, &src).await.unwrap_err();
    assert!(err.to_string().contains("deadline"), "{err}");
    let runs = w
        .store
        .list_harvest_runs(&w.ctx, Some("hs-hub"))
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, HarvestRunState::Failed);
    assert_eq!(runs[0].logs.last().unwrap().fingerprint, "harvest.deadline");

    // The source is free again: the next run goes ahead.
    let f = MapFetcher::new();
    f.insert(HUB_PAGE1, Some("application/json"), hub_answer());
    engine(&f)
        .run_source(&w.ctx, &w.store, &src)
        .await
        .expect("next run");
}

async fn scheduled_world(fail: usize) -> (World, Arc<MemoryStore>, Arc<Flaky>) {
    let w = world(&[]).await;
    let store = Arc::new(w.store.clone());
    store
        .upsert_harvest_source(
            &w.ctx,
            source("arc-gis-hub", "hub", HUB, &["hub.city.example"]),
        )
        .await
        .unwrap();
    let f = Arc::new(Flaky {
        fail,
        hang: false,
        calls: AtomicUsize::new(0),
    });
    (w, store, f)
}

fn scheduler(w: &World, store: Arc<MemoryStore>, f: Arc<Flaky>, granted: bool) -> HarvestScheduler {
    let (_, grant) = harvest_principal(w.org, w.cat);
    let grants = if granted { vec![grant] } else { Vec::new() };
    HarvestScheduler::new(
        Arc::new(engine_on(f)),
        store,
        Arc::new(PolicyAuthorizer::new(grants)),
        vec![(w.org, w.cat)],
    )
    .with_retry_delay(Duration::ZERO)
}

const AT_0323: (i32, u32, u32, u32, u32) = (2026, 11, 9, 3, 23);

fn at(h: u32, m: u32) -> chrono::DateTime<Utc> {
    let (y, mo, d, _, _) = AT_0323;
    Utc.with_ymd_and_hms(y, mo, d, h, m, 0).unwrap()
}

#[tokio::test]
async fn the_scheduler_runs_an_enabled_source_only_in_its_minute() {
    let (w, store, f) = scheduled_world(0).await;
    let s = scheduler(&w, store.clone(), f.clone(), true);
    assert!(s.run_due(at(3, 22)).await.is_empty());
    let out = s.run_due(at(3, 23)).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.as_ref().unwrap().state, HarvestRunState::Succeeded);
    assert_eq!(active_names(&w).await, vec!["city-cesty-aaa111".to_owned()]);
    // Disabled: nothing runs.
    let mut src = store
        .get_harvest_source(&w.ctx, "hub")
        .await
        .unwrap()
        .unwrap();
    src.enabled = false;
    store.upsert_harvest_source(&w.ctx, src).await.unwrap();
    assert!(s.run_due(at(3, 23)).await.is_empty());
}

/// A start runs every enabled source whatever its schedule (gate 4: the 17 harvested datasets
/// were missing because no run had happened before 03:23; a restart after the import lists the
/// sheets the imported rows give), each tried again after a failure, a disabled one never.
#[tokio::test]
async fn a_start_runs_every_enabled_source_off_schedule() {
    let (w, store, f) = scheduled_world(usize::MAX).await;
    let s = scheduler(&w, store.clone(), f.clone(), true);
    let out = s.run_all().await;
    assert!(out[0].1.is_err());
    assert_eq!(f.calls.load(Ordering::SeqCst), 4);
    assert!(active_names(&w).await.is_empty());

    let (w, store, f) = scheduled_world(0).await;
    let s = scheduler(&w, store.clone(), f.clone(), true);
    let out = s.run_all().await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.as_ref().unwrap().state, HarvestRunState::Succeeded);
    assert_eq!(active_names(&w).await, vec!["city-cesty-aaa111".to_owned()]);
    // The next start runs it again, and the run finds it unchanged.
    let out = s.run_all().await;
    assert_eq!(out[0].1.as_ref().unwrap().unchanged_count, 1);
    let mut src = store
        .get_harvest_source(&w.ctx, "hub")
        .await
        .unwrap()
        .unwrap();
    src.enabled = false;
    store.upsert_harvest_source(&w.ctx, src).await.unwrap();
    assert!(s.run_all().await.is_empty());
    // The catalogue's harvest principal still has to hold the grant.
    let (w, store, f) = scheduled_world(0).await;
    let out = scheduler(&w, store, f.clone(), false).run_all().await;
    assert!(matches!(out[0].1, Err(DataroomError::Forbidden(_))));
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_failed_run_is_retried_three_times_then_logged_and_the_next_cycle_runs() {
    // Fails three times, the fourth attempt (third retry) succeeds.
    let (w, store, f) = scheduled_world(RETRIES as usize).await;
    let out = scheduler(&w, store.clone(), f.clone(), true)
        .run_due(at(3, 23))
        .await;
    assert_eq!(out[0].1.as_ref().unwrap().state, HarvestRunState::Succeeded);
    assert_eq!(f.calls.load(Ordering::SeqCst), 4);

    // Fails every time: four attempts, four Failed rows, the error comes back; the next
    // cycle tries again.
    let (w, store, f) = scheduled_world(usize::MAX).await;
    let s = scheduler(&w, store.clone(), f.clone(), true);
    let out = s.run_due(at(3, 23)).await;
    assert!(out[0].1.is_err());
    assert_eq!(f.calls.load(Ordering::SeqCst), 4);
    let runs = store
        .list_harvest_runs(&w.ctx, Some("hs-hub"))
        .await
        .unwrap();
    assert_eq!(runs.len(), 4);
    assert!(runs.iter().all(|r| r.state == HarvestRunState::Failed));
    s.run_due(at(3, 23)).await;
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        8,
        "the next cycle runs again"
    );
}

#[tokio::test]
async fn without_the_harvest_grant_the_scheduler_runs_nothing() {
    let (w, store, f) = scheduled_world(0).await;
    let out = scheduler(&w, store.clone(), f.clone(), false)
        .run_due(at(3, 23))
        .await;
    assert!(matches!(out[0].1, Err(DataroomError::Forbidden(_))));
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    // The grant is `harvest` on this catalogue only.
    let (subject, grant) = harvest_principal(w.org, w.cat);
    assert_eq!(grant.actions, vec![dataroom_core::Action::Harvest]);
    assert_eq!(grant.catalogue_id, Some(w.cat));
    assert_eq!(subject.actor_id.as_deref(), Some(grant.principal.as_str()));
}
