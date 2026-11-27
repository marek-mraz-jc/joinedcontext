#![forbid(unsafe_code)]
//! Scheduled harvests as prod's CronJobs ran them: one run per source at a time (the Running
//! row is the lock), a deadline that stops a run and frees the source, up to three retries of a
//! failed run, and the catalogue's harvest principal as the only one allowed to run.

mod common;

use common::*;
use dataroom_core::ops::HarvestRunner;
use dataroom_core::ops::{execute_operation, OpContext};
use dataroom_core::store::BoxFuture;
use dataroom_core::{
    ActivityRow, Catalogue, CatalogueId, CataloguePlugin, DataroomError, Dataset, Distribution,
    HarvestRun, HarvestRunState, HarvestSource, OrgId, PolicyAuthorizer, ProfileRecord, Publisher,
    RequestCtx, ResourceView, Result, ShaclValidationReport, StaticPage, Store,
};
use dataroom_harvest::{
    cron_matches, harvest_principal, FetchResponse, HarvestEngine, HarvestScheduler, MapFetcher,
    OutboundFetcher, ValidatedTarget, RETRIES,
};
use dataroom_store_memory::MemoryStore;
use jc_time::TimestampTz;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

fn running(w: &World, id: &str, started: TimestampTz) -> HarvestRun {
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
    let held = running(&w, "run-held", TimestampTz::now());
    assert!(w
        .store
        .claim_harvest_run(
            &w.ctx,
            held.clone(),
            TimestampTz::from_second(TimestampTz::now().as_second() - 3_600).expect("in range")
        )
        .await
        .unwrap());
    // A second claim of the same source is refused while the first is live.
    assert!(!w
        .store
        .claim_harvest_run(
            &w.ctx,
            running(&w, "run-second", TimestampTz::now()),
            TimestampTz::from_second(TimestampTz::now().as_second() - 3_600).expect("in range")
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
async fn the_scheduler_does_not_retry_a_source_another_run_holds() {
    let (w, store, f) = scheduled_world(0).await;
    let held = running(&w, "run-held", TimestampTz::now());
    let since = TimestampTz::from_second(TimestampTz::now().as_second() - 3_600).expect("in range");
    assert!(store.claim_harvest_run(&w.ctx, held, since).await.unwrap());
    let out = scheduler(&w, store, f.clone(), true)
        .run_due(at(3, 23))
        .await;
    assert!(
        matches!(out[0].1, Err(DataroomError::Conflict(_))),
        "{:?}",
        out[0].1
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 0, "no fetch and no retry");
}

#[tokio::test]
async fn a_running_run_past_the_deadline_is_marked_failed_and_frees_the_source() {
    let w = world(&[]).await;
    let src = source("arc-gis-hub", "hub", HUB, &["hub.city.example"]);
    let stale = running(
        &w,
        "run-stale",
        TimestampTz::from_second(TimestampTz::now().as_second() - 2 * 3_600).expect("in range"),
    );
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

fn at(h: i8, m: i8) -> TimestampTz {
    let at = jiff::civil::date(2026, 11, 9).at(h, m, 0, 0);
    TimestampTz(jiff::tz::Offset::UTC.to_timestamp(at).expect("in range"))
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
async fn a_catalogue_added_while_the_scheduler_runs_is_read_on_its_next_check() {
    let (w, store, f) = scheduled_world(0).await;
    let (_, grant) = harvest_principal(w.org, w.cat);
    let s = HarvestScheduler::new(
        Arc::new(engine_on(f.clone())),
        store.clone(),
        Arc::new(PolicyAuthorizer::new(vec![grant])),
        Vec::new(),
    );
    assert!(s.run_all().await.is_empty(), "no catalogue yet");
    s.catalogues()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((w.org, w.cat));
    assert_eq!(s.run_all().await.len(), 1);
}

#[tokio::test]
async fn a_source_whose_schedule_does_not_read_is_skipped_and_the_others_run() {
    let (w, store, f) = scheduled_world(0).await;
    let mut bad = source("arc-gis-hub", "bad", HUB, &["hub.city.example"]);
    bad.schedule = "61 * * * *".into();
    store.upsert_harvest_source(&w.ctx, bad).await.unwrap();
    let out = scheduler(&w, store, f, true).run_due(at(3, 23)).await;
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["hub"]);
}

#[test]
fn sunday_reads_as_seven_and_a_range_end_that_is_no_number_is_refused() {
    // 2026-11-08 is a Sunday.
    let sunday = TimestampTz(
        jiff::tz::Offset::UTC
            .to_timestamp(jiff::civil::date(2026, 11, 8).at(3, 23, 0, 0))
            .expect("in range"),
    );
    assert!(cron_matches("23 3 * * 7", sunday).unwrap());
    assert!(cron_matches("23 3 * * 6-7", sunday).unwrap());
    assert!(!cron_matches("23 3 * * 7", at(3, 23)).unwrap(), "a Monday");
    assert!(cron_matches("23 3 * * 1-x", sunday).is_err());
    // A range start that is no number, in any field, is refused the same way.
    assert!(cron_matches("23 3 * * x-5", sunday).is_err());
    assert!(cron_matches("x-5 3 * * *", sunday).is_err());
    assert!(cron_matches("23 -3 * * *", sunday).is_err());
}

#[test]
fn a_day_of_month_alone_fires_on_that_day_whatever_its_weekday() {
    let first = TimestampTz(
        jiff::tz::Offset::UTC
            .to_timestamp(jiff::civil::date(2026, 11, 1).at(0, 0, 0, 0))
            .expect("in range"),
    );
    assert!(cron_matches("0 0 1 * *", first).unwrap());
    assert!(!cron_matches("0 0 1 * *", at(0, 0)).unwrap(), "the 9th");
    // both restricted: either matches (the 9th is a Monday)
    assert!(cron_matches("0 0 1 * 1", at(0, 0)).unwrap());
    assert!(!cron_matches("0 0 2 * 2", at(0, 0)).unwrap());
    // a field out of its range, too few fields, or an empty expression is refused
    assert!(cron_matches("0 0 32 * *", first).is_err());
    assert!(cron_matches("0 0 1 *", first).is_err());
    assert!(cron_matches("", first).is_err());
}

/// The spawned loop runs every enabled source in its first minute (paused time: the sleeps pass
/// at once while the wall clock stays in one minute, so later checks run nothing).
#[tokio::test(start_paused = true)]
async fn the_spawned_scheduler_runs_every_source_in_its_first_minute() {
    let (w, store, f) = scheduled_world(0).await;
    let handle = scheduler(&w, store, f.clone(), true).spawn();
    // A virtual second per look: the runtime idles, so paused time moves to the next timer.
    for _ in 0..600 {
        if !active_names(&w).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // Later checks in the same wall-clock minute run nothing.
    for _ in 0..180 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    handle.abort();
    assert_eq!(active_names(&w).await, vec!["city-cesty-aaa111".to_owned()]);
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        1,
        "one run, no second in the same minute"
    );
}

/// After the first minute's run of everything, each later minute runs the sources due in it:
/// a clock stepping one minute per check reaches 03:23, the hub source's minute, and runs it
/// once more; the minutes around it run nothing.
#[tokio::test(start_paused = true)]
async fn the_spawned_scheduler_runs_the_sources_due_in_each_later_minute() {
    let (w, store, f) = scheduled_world(0).await;
    // Each check reads the clock twice (the wait, then the minute): two reads per minute.
    let reads = Arc::new(AtomicUsize::new(0));
    let clock = {
        let reads = reads.clone();
        move || {
            let n = i64::try_from(reads.fetch_add(1, Ordering::SeqCst) / 2).expect("small");
            TimestampTz(
                at(3, 22)
                    .0
                    .checked_add(jiff::SignedDuration::from_mins(n))
                    .expect("in range"),
            )
        }
    };
    let handle = scheduler(&w, store, f.clone(), true).spawn_with_clock(clock);
    for _ in 0..600 {
        if f.calls.load(Ordering::SeqCst) >= 2 && reads.load(Ordering::SeqCst) >= 8 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    handle.abort();
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        2,
        "the start's run at 03:22, the due run at 03:23, none at 03:24"
    );
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

/// The harvest grant runs and reads a catalogue's sources; it changes, removes or administers
/// none of them.
#[test]
fn the_harvest_grant_runs_and_reads_sources_only() {
    use dataroom_core::{Action, Authorizer, ObjectTarget, RequestCtx};
    let org = dataroom_core::OrgId::from_uuid(uuid::Uuid::new_v4());
    let cat = dataroom_core::CatalogueId::from_uuid(uuid::Uuid::new_v4());
    let (subject, grant) = harvest_principal(org, cat);
    let authz = PolicyAuthorizer::new(vec![grant]);
    let ctx = RequestCtx::new(org, cat, subject);
    let target = ObjectTarget::HarvestSource {
        org_id: org,
        catalogue_id: cat,
        source_id: "hs-hub".into(),
    };
    for action in [Action::Harvest, Action::Read] {
        assert!(authz.check(&ctx, action, &target).allowed, "{action:?}");
    }
    for action in [
        Action::Propose,
        Action::Approve,
        Action::Delete,
        Action::Admin,
    ] {
        assert!(!authz.check(&ctx, action, &target).allowed, "{action:?}");
    }
}

/// Answers the Hub page once the test opens the gate.
struct Gated {
    open: AtomicBool,
}

impl OutboundFetcher for Gated {
    fn fetch_hop<'a>(
        &'a self,
        _target: &'a ValidatedTarget,
        _accept: &'a str,
        _max_bytes: usize,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        Box::pin(async move {
            while !self.open.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(FetchResponse::Ok {
                content_type: Some("application/json".into()),
                body: hub_answer(),
            })
        })
    }
}

async fn run_state(w: &World, run_id: &str) -> HarvestRun {
    w.store
        .get_harvest_run(&w.ctx, run_id)
        .await
        .unwrap()
        .expect("run row")
}

/// A fetcher whose read panics, as a bug in a parser or a store would end a run's task.
struct Panicky;

impl OutboundFetcher for Panicky {
    fn fetch_hop<'a>(
        &'a self,
        _target: &'a ValidatedTarget,
        _accept: &'a str,
        _max_bytes: usize,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        Box::pin(async { panic!("the read broke") })
    }
}

/// The memory store with named methods failing on demand, as Postgres fails when its
/// connection drops.
struct FailingStore {
    inner: MemoryStore,
    failing: std::sync::Mutex<std::collections::BTreeSet<&'static str>>,
}

impl FailingStore {
    fn new(inner: MemoryStore) -> Self {
        Self {
            inner,
            failing: std::sync::Mutex::default(),
        }
    }

    fn fail(&self, method: &'static str) {
        self.failing.lock().unwrap().insert(method);
    }

    fn refusal(&self, method: &str) -> Option<DataroomError> {
        self.failing
            .lock()
            .unwrap()
            .contains(method)
            .then(|| DataroomError::Storage(format!("{method} failed")))
    }
}

macro_rules! forward {
    ($( $name:ident<$lt:lifetime>($($arg:ident : $ty:ty),*) -> $ret:ty; )*) => {
        $(
            fn $name<$lt>(&$lt self, $($arg: $ty),*) -> BoxFuture<$lt, Result<$ret>> {
                if let Some(e) = self.refusal(stringify!($name)) {
                    return Box::pin(async move { Err(e) });
                }
                self.inner.$name($($arg),*)
            }
        )*
    };
}

impl Store for FailingStore {
    forward! {
        resolve_host<'a>(host: &'a str) -> Option<Catalogue>;
        get_catalogue<'a>(org_id: OrgId, catalogue_id: CatalogueId) -> Option<Catalogue>;
        list_catalogues<'a>(org_id: OrgId) -> Vec<Catalogue>;
        upsert_catalogue<'a>(catalogue: Catalogue) -> Catalogue;
        save_catalogue<'a>(catalogue: Catalogue, expected: Option<TimestampTz>, activity: ActivityRow) -> Catalogue;
        delete_catalogue<'a>(org_id: OrgId, catalogue_id: CatalogueId) -> bool;
        list_publishers<'a>(ctx: &'a RequestCtx) -> Vec<Publisher>;
        get_publisher<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<Publisher>;
        upsert_publisher<'a>(ctx: &'a RequestCtx, publisher: Publisher) -> Publisher;
        save_publisher<'a>(ctx: &'a RequestCtx, publisher: Publisher, expected: Option<TimestampTz>, activity: ActivityRow) -> Publisher;
        merge_publishers<'a>(ctx: &'a RequestCtx, source: &'a str, target: &'a str, activity: ActivityRow) -> Publisher;
        withdraw_publisher<'a>(ctx: &'a RequestCtx, id_or_name: &'a str, activity: ActivityRow) -> Publisher;
        list_datasets<'a>(ctx: &'a RequestCtx) -> Vec<Dataset>;
        get_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<Dataset>;
        upsert_dataset<'a>(ctx: &'a RequestCtx, dataset: Dataset) -> Dataset;
        save_dataset<'a>(ctx: &'a RequestCtx, dataset: Dataset, expected: Option<TimestampTz>, activity: ActivityRow) -> Dataset;
        withdraw_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Dataset;
        purge_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str, activity: ActivityRow) -> ();
        get_distribution<'a>(ctx: &'a RequestCtx, id: &'a str) -> Option<(Dataset, Distribution)>;
        list_resource_views<'a>(ctx: &'a RequestCtx, resource_id: &'a str) -> Vec<ResourceView>;
        get_resource_view<'a>(ctx: &'a RequestCtx, view_id: &'a str) -> Option<ResourceView>;
        upsert_resource_view<'a>(ctx: &'a RequestCtx, view: ResourceView) -> ResourceView;
        save_resource_view<'a>(ctx: &'a RequestCtx, view: ResourceView, activity: ActivityRow) -> ResourceView;
        delete_resource_view<'a>(ctx: &'a RequestCtx, view_id: &'a str, activity: ActivityRow) -> ();
        list_harvest_sources<'a>(ctx: &'a RequestCtx) -> Vec<HarvestSource>;
        get_harvest_source<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<HarvestSource>;
        upsert_harvest_source<'a>(ctx: &'a RequestCtx, source: HarvestSource) -> HarvestSource;
        save_harvest_source<'a>(ctx: &'a RequestCtx, source: HarvestSource, activity: ActivityRow) -> HarvestSource;
        delete_harvest_source<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> bool;
        list_harvest_runs<'a>(ctx: &'a RequestCtx, source_id: Option<&'a str>) -> Vec<HarvestRun>;
        get_harvest_run<'a>(ctx: &'a RequestCtx, run_id: &'a str) -> Option<HarvestRun>;
        upsert_harvest_run<'a>(ctx: &'a RequestCtx, run: HarvestRun) -> HarvestRun;
        claim_harvest_run<'a>(ctx: &'a RequestCtx, run: HarvestRun, stale_before: TimestampTz) -> bool;
        record_activity<'a>(ctx: &'a RequestCtx, row: ActivityRow) -> ();
        list_activity<'a>(ctx: &'a RequestCtx, object_id: Option<&'a str>, limit: usize) -> Vec<ActivityRow>;
        upsert_shacl_report<'a>(ctx: &'a RequestCtx, report: ShaclValidationReport) -> ShaclValidationReport;
        list_shacl_reports<'a>(ctx: &'a RequestCtx, dataset_id: Option<&'a str>, profile_id: Option<&'a str>) -> Vec<ShaclValidationReport>;
        list_profiles<'a>(ctx: &'a RequestCtx) -> Vec<ProfileRecord>;
        upsert_profile<'a>(ctx: &'a RequestCtx, profile: ProfileRecord) -> ProfileRecord;
        save_profile<'a>(ctx: &'a RequestCtx, profile: ProfileRecord, activity: ActivityRow) -> ProfileRecord;
        list_pages<'a>(ctx: &'a RequestCtx) -> Vec<StaticPage>;
        upsert_page<'a>(ctx: &'a RequestCtx, page: StaticPage) -> StaticPage;
        list_plugins<'a>(ctx: &'a RequestCtx) -> Vec<CataloguePlugin>;
        upsert_plugin<'a>(ctx: &'a RequestCtx, plugin: CataloguePlugin) -> CataloguePlugin;
        save_plugin<'a>(ctx: &'a RequestCtx, plugin: CataloguePlugin, activity: ActivityRow) -> CataloguePlugin;
        odata_table<'a>(ctx: &'a RequestCtx, set_name: &'a str) -> Arc<dyn odata_table::Table>;
    }
}

/// A read that breaks the store's run writes, then panics: the lost run cannot be released.
struct BreaksThenPanics(Arc<FailingStore>);

impl OutboundFetcher for BreaksThenPanics {
    fn fetch_hop<'a>(
        &'a self,
        _target: &'a ValidatedTarget,
        _accept: &'a str,
        _max_bytes: usize,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        self.0.fail("upsert_harvest_run");
        Box::pin(async { panic!("the read broke") })
    }
}

/// A started run whose task panics is recorded failed at once and frees its source, without
/// waiting for the deadline.
#[tokio::test]
async fn a_run_whose_task_panics_is_recorded_failed_and_frees_the_source() {
    let w = world(&["hub.city.example"]).await;
    w.store
        .upsert_harvest_source(
            &w.ctx,
            source("arc-gis-hub", "hub", HUB, &["hub.city.example"]),
        )
        .await
        .unwrap();
    let (subject, grant) = harvest_principal(w.org, w.cat);
    let ctx = RequestCtx::new(w.org, w.cat, subject);
    let op_ctx = OpContext {
        store: Arc::new(w.store.clone()),
        authorizer: Arc::new(PolicyAuthorizer::new(vec![grant])),
        validator: None,
        harvester: Some(Arc::new(engine_on(Arc::new(Panicky)))),
        sources_by_portal: false,
        branding: None,
    };
    let answer = execute_operation(&op_ctx, &ctx, "harvest_run.start", json!({"source": "hub"}))
        .await
        .expect("started");
    let run_id = answer["id"].as_str().expect("run id").to_owned();
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let run = run_state(&w, &run_id).await;
            if run.state != HarvestRunState::Running {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the lost run is released");
    assert_eq!(done.state, HarvestRunState::Failed);
    assert_eq!(done.error.as_deref(), Some("the run stopped unexpectedly"));
    assert_eq!(
        done.logs.last().map(|l| l.fingerprint.as_str()),
        Some("harvest.lost")
    );
    assert!(done.finished_at.is_some());
}

/// A fetcher that changes the store under the run, then panics: `Finish` records the run
/// succeeded first (as a run that ended just before its task broke), `Delete` removes the
/// source and with it the run row.
#[derive(Clone, Copy)]
enum Before {
    Finish,
    Delete,
}

struct PanicsAfter {
    store: MemoryStore,
    ctx: RequestCtx,
    before: Before,
}

impl OutboundFetcher for PanicsAfter {
    fn fetch_hop<'a>(
        &'a self,
        _target: &'a ValidatedTarget,
        _accept: &'a str,
        _max_bytes: usize,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        Box::pin(async move {
            match self.before {
                Before::Finish => {
                    let runs = self.store.list_harvest_runs(&self.ctx, None).await.unwrap();
                    for mut run in runs {
                        run.state = HarvestRunState::Succeeded;
                        run.finished_at = Some(TimestampTz::now());
                        self.store.upsert_harvest_run(&self.ctx, run).await.unwrap();
                    }
                }
                Before::Delete => {
                    assert!(self
                        .store
                        .delete_harvest_source(&self.ctx, "hub")
                        .await
                        .unwrap());
                }
            }
            panic!("the read broke after the store moved")
        })
    }
}

/// A run whose task panics after its row was finished, or after its row is gone, is left as
/// it is: the lost-run release writes nothing over a finished run and invents no row.
#[tokio::test]
async fn a_panicked_run_that_already_finished_or_vanished_is_left_alone() {
    for before in [Before::Finish, Before::Delete] {
        let w = world(&["hub.city.example"]).await;
        w.store
            .upsert_harvest_source(
                &w.ctx,
                source("arc-gis-hub", "hub", HUB, &["hub.city.example"]),
            )
            .await
            .unwrap();
        let (subject, _) = harvest_principal(w.org, w.cat);
        let ctx = RequestCtx::new(w.org, w.cat, subject);
        let fetcher = PanicsAfter {
            store: w.store.clone(),
            ctx: ctx.clone(),
            before,
        };
        let engine = Arc::new(engine_on(Arc::new(fetcher)));
        let run = engine
            .clone()
            .start_source(ctx.clone(), Arc::new(w.store.clone()), {
                w.store
                    .get_harvest_source(&ctx, "hub")
                    .await
                    .unwrap()
                    .unwrap()
            })
            .await
            .expect("started");
        // the task panics at once; give the release its turn
        let settled = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let row = w.store.get_harvest_run(&ctx, &run.id).await.unwrap();
                match (before, &row) {
                    (Before::Finish, Some(r)) if r.state != HarvestRunState::Running => break row,
                    (Before::Delete, None) => break row,
                    _ => tokio::time::sleep(Duration::from_millis(5)).await,
                }
            }
        })
        .await
        .expect("the store moved");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let after = w.store.get_harvest_run(&ctx, &run.id).await.unwrap();
        match before {
            Before::Finish => {
                let r = after.expect("the finished row stays");
                assert_eq!(r.state, HarvestRunState::Succeeded);
                assert!(r.error.is_none());
                assert!(r.logs.iter().all(|l| l.fingerprint != "harvest.lost"));
                assert_eq!(settled.map(|s| s.state), Some(HarvestRunState::Succeeded));
            }
            Before::Delete => assert!(after.is_none(), "no row is invented: {after:?}"),
        }
    }
}

/// A scheduled run whose task panics is dropped from the outcome; the scheduler itself goes on.
#[tokio::test]
async fn a_scheduled_run_that_panics_is_left_out_of_the_outcome_and_frees_its_source() {
    let w = world(&["hub.city.example"]).await;
    let store = Arc::new(w.store.clone());
    store
        .upsert_harvest_source(
            &w.ctx,
            source("arc-gis-hub", "hub", HUB, &["hub.city.example"]),
        )
        .await
        .unwrap();
    let mut earlier = running(&w, "run-earlier", TimestampTz::now());
    earlier.state = HarvestRunState::Succeeded;
    earlier.finished_at = Some(TimestampTz::now());
    w.store.upsert_harvest_run(&w.ctx, earlier).await.unwrap();
    let (_, grant) = harvest_principal(w.org, w.cat);
    let s = HarvestScheduler::new(
        Arc::new(engine_on(Arc::new(Panicky))),
        store,
        Arc::new(PolicyAuthorizer::new(vec![grant])),
        vec![(w.org, w.cat)],
    )
    .with_retry_delay(Duration::ZERO);
    assert!(s.run_all().await.is_empty());
    // The panicked run's row is Failed at once, so the next scheduled check is not refused
    // with a conflict until the harvest deadline.
    let runs = w.store.list_harvest_runs(&w.ctx, None).await.unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");
    let (earlier, lost): (Vec<_>, Vec<_>) = runs.into_iter().partition(|r| r.id == "run-earlier");
    assert_eq!(
        earlier[0].state,
        HarvestRunState::Succeeded,
        "a finished run is left as it is"
    );
    assert_eq!(lost[0].state, HarvestRunState::Failed);
    assert_eq!(
        lost[0].error.as_deref(),
        Some("the run stopped unexpectedly")
    );
    assert_eq!(lost[0].logs.last().unwrap().fingerprint, "harvest.lost");
}

/// A catalogue whose sources cannot be listed is skipped; its sources are not run and no run
/// row is written.
#[tokio::test]
async fn a_catalogue_whose_sources_cannot_be_listed_runs_nothing() {
    let w = world(&["hub.city.example"]).await;
    w.store
        .upsert_harvest_source(
            &w.ctx,
            source("arc-gis-hub", "hub", HUB, &["hub.city.example"]),
        )
        .await
        .unwrap();
    let store = Arc::new(FailingStore::new(w.store.clone()));
    store.fail("list_harvest_sources");
    let (_, grant) = harvest_principal(w.org, w.cat);
    let s = HarvestScheduler::new(
        Arc::new(engine_on(Arc::new(Panicky))),
        store,
        Arc::new(PolicyAuthorizer::new(vec![grant])),
        vec![(w.org, w.cat)],
    );
    assert!(s.run_all().await.is_empty());
    assert!(w
        .store
        .list_harvest_runs(&w.ctx, None)
        .await
        .unwrap()
        .is_empty());
}

/// A panicked run whose release cannot be written stays Running (the deadline frees it
/// later); the scheduler still returns.
#[tokio::test]
async fn a_panicked_run_whose_release_cannot_be_written_stays_running() {
    let w = world(&["hub.city.example"]).await;
    w.store
        .upsert_harvest_source(
            &w.ctx,
            source("arc-gis-hub", "hub", HUB, &["hub.city.example"]),
        )
        .await
        .unwrap();
    let store = Arc::new(FailingStore::new(w.store.clone()));
    let (_, grant) = harvest_principal(w.org, w.cat);
    let s = HarvestScheduler::new(
        Arc::new(engine_on(Arc::new(BreaksThenPanics(store.clone())))),
        store,
        Arc::new(PolicyAuthorizer::new(vec![grant])),
        vec![(w.org, w.cat)],
    )
    .with_retry_delay(Duration::ZERO);
    assert!(s.run_all().await.is_empty());
    let runs = w.store.list_harvest_runs(&w.ctx, None).await.unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].state, HarvestRunState::Running);
}

/// S11-14: `harvest_run.start` answers once the source is held, and the run goes on when the
/// request that started it is dropped (a client or edge timeout); it then frees the source.
#[tokio::test]
async fn a_dropped_start_request_leaves_the_run_going_and_frees_the_source_at_its_end() {
    let w = world(&["hub.city.example"]).await;
    w.store
        .upsert_harvest_source(
            &w.ctx,
            source("arc-gis-hub", "hub", HUB, &["hub.city.example"]),
        )
        .await
        .unwrap();
    let gate = Arc::new(Gated {
        open: AtomicBool::new(false),
    });
    let (subject, grant) = harvest_principal(w.org, w.cat);
    let ctx = RequestCtx::new(w.org, w.cat, subject);
    let op_ctx = Arc::new(OpContext {
        store: Arc::new(w.store.clone()),
        authorizer: Arc::new(PolicyAuthorizer::new(vec![grant])),
        validator: None,
        harvester: Some(Arc::new(engine_on(gate.clone()))),
        sources_by_portal: false,
        branding: None,
    });
    let start = || {
        let (op_ctx, ctx) = (op_ctx.clone(), ctx.clone());
        async move {
            execute_operation(&op_ctx, &ctx, "harvest_run.start", json!({"source": "hub"})).await
        }
    };

    let request = tokio::spawn(start());
    let run_id = loop {
        let runs = w
            .store
            .list_harvest_runs(&w.ctx, Some("hs-hub"))
            .await
            .unwrap();
        if let Some(run) = runs.first() {
            break run.id.clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    request.abort();
    if let Ok(answer) = request.await {
        let answer = answer.unwrap();
        assert_eq!(
            (answer["id"].as_str(), answer["state"].as_str()),
            (Some(run_id.as_str()), Some("running"))
        );
    }
    // While the run holds the source, another start is refused at once.
    let held = start().await.unwrap_err();
    assert!(matches!(held, DataroomError::Conflict(_)), "{held}");

    gate.open.store(true, Ordering::SeqCst);
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let run = run_state(&w, &run_id).await;
            if run.state != HarvestRunState::Running {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the run ends although its request was dropped");
    assert_eq!(done.state, HarvestRunState::Succeeded, "{:?}", done.error);
    assert_eq!(active_names(&w).await, vec!["city-cesty-aaa111".to_owned()]);
    let again = start().await.unwrap();
    assert_eq!(again["state"], "running", "the source is free again");
}
