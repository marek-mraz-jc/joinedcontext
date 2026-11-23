#![forbid(unsafe_code)]
//! S6-11: an outbox event is marked dispatched only when every matching hook took it with a 2xx
//! or refused it for good; a failed delivery keeps the event due, its next pass sends only to
//! the hooks that did not take it, and after `MAX_DELIVERY_ATTEMPTS` passes it is given up with
//! its failed deliveries on record. An event that cannot be recorded is the caller's error.

use serde_json::json;
use shako_api::outbox::{MAX_DELIVERY_ATTEMPTS, retry_delay};
use shako_api::{dispatch_pending_outbox, emit_and_dispatch_event, record_event};
use shako_core::{
    InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RepoId, ShakoError,
    ShakoSigningKey, Store, TokenVerifier, WebhookFormat, WebhookId, WebhookRecord,
};
use shako_server::ShakoState;
use shako_store_memory::MemoryStore;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const INST: InstanceId = InstanceId::from_u128(1);
const ORG: OrgId = OrgId::from_u128(10);
const FLAKY: &str = "https://receiver.example/flaky";
const STEADY: &str = "https://receiver.example/steady";

struct World {
    _tmp: tempfile::TempDir,
    state: ShakoState,
    store: Arc<MemoryStore>,
    repo: RepoId,
    /// The flaky hook's answers, oldest first; empty answers 202.
    flaky: Arc<parking_lot::Mutex<VecDeque<u16>>>,
    flaky_hits: Arc<AtomicUsize>,
    steady_hits: Arc<AtomicUsize>,
    published: Arc<AtomicUsize>,
}

fn hook(repo: RepoId, url: &str) -> WebhookRecord {
    WebhookRecord {
        id: WebhookId::new(),
        instance_id: INST,
        org_id: ORG,
        repo_id: repo,
        url: url.into(),
        events: vec![],
        active: true,
        secret_ref: "hook-secret".into(),
        previous_secret_ref: None,
        format: WebhookFormat::Shako,
        created_at: jc_time::TimestampTz::now(),
    }
}

async fn world() -> World {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(MemoryStore::new());
    let flaky: Arc<parking_lot::Mutex<VecDeque<u16>>> = Arc::default();
    let (flaky_hits, steady_hits) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (answers, f_hits, s_hits) = (flaky.clone(), flaky_hits.clone(), steady_hits.clone());
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec!["receiver.example".into()]),
        allow_loopback_for_tests: false,
        mock_handler: Some(Arc::new(move |_method, url, _headers, _body| {
            if url == FLAKY {
                f_hits.fetch_add(1, Ordering::SeqCst);
                return Some((answers.lock().pop_front().unwrap_or(202), Vec::new()));
            }
            if url == STEADY {
                s_hits.fetch_add(1, Ordering::SeqCst);
                return Some((202, Vec::new()));
            }
            None
        })),
        ..OutboundPolicy::default()
    });
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"));
    let state = ShakoState::new(
        INST,
        tmp.path().to_path_buf(),
        store.clone(),
        Arc::new(LocalAuthorizer::new(Vec::new())),
        verifier,
        guard,
    );
    state
        .secret_store
        .put_secret(ORG, "hook-secret", "s3cret-for-tests")
        .await
        .expect("secret kept");
    let published = Arc::new(AtomicUsize::new(0));
    let seen = published.clone();
    state.subscribe_events(Arc::new(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    }));
    let repo = RepoId::new();
    for url in [FLAKY, STEADY] {
        store.create_webhook(hook(repo, url)).await.expect("hook");
    }
    World {
        _tmp: tmp,
        state,
        store,
        repo,
        flaky,
        flaky_hits,
        steady_hits,
        published,
    }
}

impl World {
    async fn emit(&self, key: &str) -> uuid::Uuid {
        emit_and_dispatch_event(
            &self.state,
            ORG,
            Some(self.repo),
            "push",
            key.into(),
            json!({ "ref": "refs/heads/main" }),
        )
        .await
        .expect("recorded")
        .id
    }

    async fn dispatched(&self, id: uuid::Uuid) -> bool {
        self.store
            .list_outbox_events(INST, ORG)
            .await
            .unwrap()
            .iter()
            .any(|e| e.id == id && e.dispatched)
    }

    /// The waiting event is due now (the dispatcher's own wait is minutes).
    async fn due_now(&self, id: uuid::Uuid) {
        assert!(
            self.store
                .retry_outbox(INST, ORG, id, Duration::ZERO)
                .await
                .unwrap()
        );
    }

    async fn deliveries(&self, url: &str) -> Vec<String> {
        let hooks = self.store.list_webhooks(INST, ORG, Some(self.repo)).await;
        let id = hooks
            .unwrap()
            .into_iter()
            .find(|h| h.url == url)
            .unwrap()
            .id;
        self.store
            .list_deliveries(INST, ORG, Some(id))
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.status)
            .collect()
    }
}

#[tokio::test]
async fn a_failed_delivery_stays_due_and_is_sent_again_to_that_hook_only() {
    let w = world().await;
    w.flaky.lock().push_back(503);
    let id = w.emit("push:1").await;
    assert!(
        !w.dispatched(id).await,
        "a 503 leaves the event undispatched"
    );
    assert_eq!(w.deliveries(FLAKY).await, ["failed"]);
    assert_eq!(w.deliveries(STEADY).await, ["delivered"]);
    // Waiting out its delay: a pass now sends nothing.
    assert_eq!(dispatch_pending_outbox(&w.state, ORG).await.unwrap(), 0);
    assert_eq!(w.flaky_hits.load(Ordering::SeqCst), 1);

    w.due_now(id).await;
    assert_eq!(dispatch_pending_outbox(&w.state, ORG).await.unwrap(), 1);
    assert!(w.dispatched(id).await);
    assert_eq!(w.deliveries(FLAKY).await, ["failed", "delivered"]);
    assert_eq!(
        w.steady_hits.load(Ordering::SeqCst),
        1,
        "the hook that took the event is not sent it again"
    );
    assert_eq!(
        w.published.load(Ordering::SeqCst),
        1,
        "in-process subscribers hear an event once"
    );
}

#[tokio::test]
async fn a_waiting_event_holds_back_its_repositorys_events_only() {
    let w = world().await;
    w.flaky.lock().push_back(503);
    let first = w.emit("push:1").await;
    let second = w.emit("push:2").await;
    assert!(!w.dispatched(first).await);
    assert!(
        !w.dispatched(second).await,
        "a repository's events keep their order"
    );
    assert_eq!(w.flaky_hits.load(Ordering::SeqCst), 1);
    let elsewhere = emit_and_dispatch_event(
        &w.state,
        ORG,
        Some(RepoId::new()),
        "push",
        "push:elsewhere".into(),
        json!({}),
    )
    .await
    .unwrap()
    .id;
    assert!(
        w.dispatched(elsewhere).await,
        "another repository's event went out"
    );

    w.due_now(first).await;
    assert_eq!(dispatch_pending_outbox(&w.state, ORG).await.unwrap(), 2);
    assert!(w.dispatched(first).await && w.dispatched(second).await);
}

#[tokio::test]
async fn an_event_is_given_up_after_the_last_attempt_with_its_failures_recorded() {
    let w = world().await;
    w.flaky
        .lock()
        .extend(std::iter::repeat_n(503, 2 * MAX_DELIVERY_ATTEMPTS as usize));
    let id = w.emit("push:dead").await;
    let mut passes = 1;
    while !w.dispatched(id).await {
        assert!(passes <= MAX_DELIVERY_ATTEMPTS, "given up in time");
        w.due_now(id).await;
        dispatch_pending_outbox(&w.state, ORG).await.unwrap();
        passes += 1;
    }
    let failed = w.deliveries(FLAKY).await;
    assert!(failed.iter().all(|s| s == "failed"));
    assert!(failed.len() > 1, "retried before it was given up");
    assert_eq!(w.steady_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_receivers_refusal_is_final_and_a_throttle_is_retried() {
    let w = world().await;
    w.flaky.lock().push_back(410);
    let gone = w.emit("push:gone").await;
    assert!(w.dispatched(gone).await, "a 410 is the receiver's verdict");
    w.flaky.lock().push_back(429);
    let throttled = w.emit("push:throttled").await;
    assert!(
        !w.dispatched(throttled).await,
        "a 429 asks for a later pass"
    );
}

#[test]
fn the_retry_delay_doubles_up_to_a_cap() {
    assert_eq!(retry_delay(0), Duration::from_secs(10));
    assert_eq!(retry_delay(1), Duration::from_secs(20));
    assert_eq!(retry_delay(6), Duration::from_secs(640));
    assert_eq!(retry_delay(7), Duration::from_secs(900));
    assert_eq!(retry_delay(u32::MAX), Duration::from_secs(900));
}

/// The append's error reaches the caller instead of a silent success (the write path answers
/// it): here a store whose database cannot be reached.
#[tokio::test]
async fn an_event_that_cannot_be_recorded_is_the_callers_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("lazy pool");
    let state = ShakoState::new(
        INST,
        tmp.path().to_path_buf(),
        Arc::new(shako_store_pg::PgStore::from_pool_unchecked(pool)),
        Arc::new(LocalAuthorizer::new(Vec::new())),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")),
        OutboundGuard::new(OutboundPolicy::default()),
    );
    let res = record_event(&state, ORG, None, "push", "push:x".into(), json!({})).await;
    assert!(matches!(res, Err(ShakoError::Storage(_))), "{res:?}");
}
