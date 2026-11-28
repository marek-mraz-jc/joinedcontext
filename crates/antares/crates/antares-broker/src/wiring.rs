// SPDX-License-Identifier: EUPL-1.2
//! Bus wiring: roles × bus. The composition root is the ONLY
//! place that knows both the bus variant and which consumers exist.
//!
//! bus=local — single process, all roles: the store's change hook feeds the
//! in-process matcher (`antares_api::wire`), temporal recording stays synchronous
//! in the write path. Exactly v0's behaviour; the ETSI pipeline runs this.
//!
//! bus=nats — the scale-out spine:
//!   api role      produces: same-tx outbox rows, the
//!                 outbox drain publishing to `ANTARES_CHANGES` with
//!                 `Nats-Msg-Id` dedup, subscription CUD → KV,
//!                 registration CUD → `ANTARES_REGISTRY`, and the
//!                 per-instance registration mirror its federation path reads.
//!   matcher /     one shared DURABLE ("matcher"): decode → process_change →
//!   notifier      ack AFTER processing; the KV-watched subscription mirror;
//!                 the interval loop (single-winner by row-lock claim).
//!   temporal      gates nothing: auto-recording is synchronous in the write
//!                 path, and the partition job in lib.rs `spawn_background`
//!                 runs on every pod whatever its roles.
//!
//! Concurrent drains on N api pods double-publish only within the stream's
//! duplicate window, where `Nats-Msg-Id` = outbox seq absorbs them — that is
//! the design, not an accident (at-least-once, engineered idempotent).

use antares_api::AppState;
use antares_bus::nats::{self, NatsBus};
use antares_bus::ChangeEvent;
use antares_model::TenantId;
use antares_sql::store::Kind;
use futures_util::StreamExt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct Roles {
    pub api: bool,
    pub matcher: bool,
    pub notifier: bool,
    pub temporal: bool,
    pub registry: bool,
}

impl Roles {
    pub fn parse(spec: &str) -> Result<Self, String> {
        if spec == "all" {
            return Ok(Self {
                api: true,
                matcher: true,
                notifier: true,
                temporal: true,
                registry: true,
            });
        }
        let mut r = Self {
            api: false,
            matcher: false,
            notifier: false,
            temporal: false,
            registry: false,
        };
        for part in spec.split(',') {
            match part.trim() {
                "api" => r.api = true,
                "matcher" => r.matcher = true,
                "notifier" => r.notifier = true,
                "temporal" => r.temporal = true,
                "registry" => r.registry = true,
                other => {
                    return Err(format!(
                        "unknown role {other:?} (api|matcher|notifier|temporal|registry|all)"
                    ))
                }
            }
        }
        Ok(r)
    }

    pub fn all(&self) -> bool {
        self.api && self.matcher && self.notifier && self.temporal && self.registry
    }
}

/// ANTARES_OUTBOX_DRAIN: `on` (the default) or `off`, and nothing else. A
/// typo'd `of` read as "on" under the old permissive parse, quietly defeating
/// the dedicated-drainer split and the crash drill it exists for.
pub fn outbox_drain_enabled() -> Result<bool, String> {
    match std::env::var("ANTARES_OUTBOX_DRAIN") {
        Err(std::env::VarError::NotPresent) => Ok(true),
        Err(e) => Err(format!("ANTARES_OUTBOX_DRAIN is unreadable: {e}")),
        Ok(v) => match v.as_str() {
            "on" => Ok(true),
            "off" => Ok(false),
            other => Err(format!(
                "ANTARES_OUTBOX_DRAIN must be on|off, got {other:?}"
            )),
        },
    }
}

/// KV key for one mirrored subscription: tenant verbatim (token-safe by
/// construction), id hashed (URNs carry `:` — illegal in KV keys). The VALUE
/// carries the real tenant/id, so the key only needs uniqueness. Kind-scoped:
/// a Subscription and a Context Source Registration Subscription may carry the
/// same client-chosen id in one tenant (5.5.10), and sharing a key would let
/// one overwrite the other's mirror entry.
fn kv_key(tenant: &str, kind: Kind, id: &str) -> String {
    format!(
        "{tenant}.{}{:016x}",
        if kind == Kind::CSourceSubscription {
            "c"
        } else {
            ""
        },
        antares_bus::subjects::fnv1a64(id.as_bytes())
    )
}

/// Wire everything bus=nats needs onto the state. Async: connects, hydrates
/// mirrors, creates consumers, asserts topology — all before the broker
/// starts accepting traffic, so a mis-shapen topology is a startup failure,
/// never a silent runtime drift. The bus is handed back for the channels
/// that ride its core request/reply (the WebSocket relay).
pub async fn wire_nats(
    state: &mut AppState,
    url: &str,
    roles: Roles,
) -> Result<Arc<NatsBus>, Box<dyn std::error::Error>> {
    let bus = Arc::new(NatsBus::connect(url).await?);
    // /q/health `bus` member: live connection state + reconnect count.
    // Installed HERE so bus=local never carries the member at all.
    {
        let b = bus.clone();
        state.bus_stats = Some(Arc::new(move || {
            serde_json::json!({
                "mode": "nats",
                "connected": b.connected(),
                "reconnects": b.reconnects(),
            })
        }));
    }
    // Multi-process mode: interval firings need the single-winner claim
    // — keyed off this flag, not off mirror presence (local mode wires a
    // mirror too).
    state.nats = true;
    // Entity writes now enqueue their events in the write transaction.
    state.store.set_outbox(true);
    // Auto-recording stays SYNCHRONOUS in the write path in every bus mode:
    // every write goes through an api-role pod that has the
    // shared store, so recording in-request gives read-your-writes — the
    // ETSI suite asserts history immediately after a write — and kills the
    // late-replay resurrection race (a consumer re-applying a pre-delete
    // event AFTER a direct temporal delete). The recorder consumer this
    // replaced double-applied by design; it bought nothing but the races.

    // The drain nudge: a same-process write pokes its own drain, so publish
    // latency is ~1 ms, not the idle-poll interval. Cross-pod writes are
    // still covered by each pod's own nudge; the poll below stays as the
    // crash-recovery fallback.
    let nudge = Arc::new(tokio::sync::Notify::new());
    {
        let n = nudge.clone();
        // Auto-recording rides the same synchronous hook here as in bus=local
        // (one choke point for every write, no handler can forget), then nudges
        // the outbox drain.
        let st_rec = state.clone();
        state.store.set_change_hook(Arc::new(
            move |tenant: &TenantId,
                  before: Option<serde_json::Value>,
                  after: Option<serde_json::Value>|
                  -> antares_store::HookFuture<'_> {
                let st_rec = st_rec.clone();
                let n = n.clone();
                Box::pin(async move {
                    antares_api::notify::record_temporal_change(
                        &st_rec,
                        tenant,
                        before.as_ref(),
                        after.as_ref(),
                    )
                    .await;
                    n.notify_one();
                })
            },
        ));
    }

    let mut durables: Vec<&'static str> = Vec::new();

    if roles.api {
        // Subscription write side: subscription CUD → KV. The value names the
        // row and whether it is there; the watcher reads the document from
        // the store, so the credentials it holds stay in the sealed row.
        let kv = bus.subs_kv().await?;
        let kv_for_hook = kv.clone();
        let bus_for_subs = bus.clone();
        let sub_queue = ordered_sync(
            "subscription",
            move |key: String, (value, present): (serde_json::Value, bool)| {
                let kv = kv_for_hook.clone();
                let bus = bus_for_subs.clone();
                async move {
                    let bytes = serde_json::to_vec(&value).unwrap_or_default();
                    // The store row is already committed. A lost put leaves this
                    // subscription invisible to every matcher pod — silently, and
                    // until the next restart, because mirrors hydrate from the
                    // store only at process start. So retry rather than warn once.
                    // Named ceiling: after the last attempt the divergence stands
                    // until a restart; closing that needs periodic reconciliation.
                    for attempt in 0..MIRROR_SYNC_ATTEMPTS {
                        match kv.put(key.clone(), bytes.clone().into()).await {
                            Ok(_) if present => return,
                            Ok(revision) => {
                                // A delete: once the watchers had it, drop the
                                // entry so the bucket stays O(live) (T-7449).
                                // A failed purge leaves it to SUBS_MAX_AGE.
                                // Off the ordered queue: the grace must not
                                // hold the writes behind it.
                                tokio::spawn(async move {
                                    tokio::time::sleep(nats::SUBS_TOMBSTONE_GRACE).await;
                                    if let Err(e) =
                                        bus.retire_subscription_key(&key, revision).await
                                    {
                                        tracing::warn!("sub KV retire of {key} failed: {e}");
                                    }
                                });
                                return;
                            }
                            Err(e) => {
                                tracing::warn!("sub KV sync attempt {} failed: {e}", attempt + 1);
                                tokio::time::sleep(mirror_sync_backoff(attempt)).await;
                            }
                        }
                    }
                    tracing::error!("sub KV sync gave up for {key} — {SUB_UNMIRRORED}");
                }
            },
        );
        state.sub_sync = Some(Arc::new(
            move |tenant: &TenantId, kind: Kind, id: &str, doc: Option<&serde_json::Value>| {
                let present = doc.is_some();
                let key = kv_key(tenant.as_str(), kind, id);
                let value = serde_json::json!({
                    "tenant": tenant.as_str(), "id": id, "present": present,
                    "csub": kind == Kind::CSourceSubscription,
                });
                sub_queue(key, (value, present));
            },
        ));

        // Registration write side: registration CUD → ANTARES_REGISTRY delta,
        // ids only like the subscription mirror (contextSourceInfo stays in
        // the sealed row).
        let bus_for_reg = bus.clone();
        let reg_queue = ordered_sync(
            "registration",
            move |(tenant, id): (String, String), delta: serde_json::Value| {
                let bus = bus_for_reg.clone();
                async move {
                    // Same contract as the subscription mirror: the row is
                    // committed, so a lost delta makes the registration invisible
                    // to every federation path until a restart re-hydrates.
                    for attempt in 0..MIRROR_SYNC_ATTEMPTS {
                        match bus.publish_registry(&tenant, &delta).await {
                            Ok(()) => return,
                            Err(e) => {
                                let n = attempt + 1;
                                tracing::warn!("registry delta publish attempt {n} failed: {e}");
                                tokio::time::sleep(mirror_sync_backoff(attempt)).await;
                            }
                        }
                    }
                    tracing::error!("registry delta publish gave up for {id} — {REG_UNMIRRORED}");
                }
            },
        );
        state.reg_sync = Some(Arc::new(
            move |tenant: &TenantId, id: &str, doc: Option<&serde_json::Value>| {
                let delta = serde_json::json!({
                    "tenant": tenant.as_str(), "id": id, "present": doc.is_some(),
                });
                reg_queue((tenant.as_str().to_owned(), id.to_owned()), delta);
            },
        ));

        // 5.2.34 write side: a cooldown stamp is broadcast to the other api
        // pods on the registry stream (seconds-scale state, deliberately not
        // persisted) — per-process stamps re-dial a failed source from every
        // pod behind the LB.
        let bus_for_cool = bus.clone();
        state.reg_fail_sync = Some(Arc::new(move |reg_id: &str, ok: bool| {
            let bus = bus_for_cool.clone();
            let delta = serde_json::json!({"cooldownReg": reg_id, "ok": ok});
            tokio::spawn(async move {
                if let Err(e) = bus.publish_registry("cooldown", &delta).await {
                    tracing::warn!("cooldown stamp publish failed: {e}");
                }
            });
        }));

        // Registration read side: the ONE compiled registration mirror this instance's
        // federation path reads. Consumer created BEFORE the hydrate so no
        // delta can fall between them; last-writer-wins per key converges.
        let reg_mirror = Arc::new(antares_api::mirror::DocMirror::default());
        let mut reg_consumer = bus.consume_registry_broadcast().await?;
        // Installed only if it is whole. A mirror that is present and SHORT
        // is read as the truth — `reg_docs` asks it and never the store — so
        // half a hydrate silently drops Context Sources for the life of the
        // process. Left uninstalled, federation matching falls back to the
        // store's own indexed narrowing: correct, and merely slower.
        match antares_api::notify::seed_mirror(
            &*state.store,
            reg_mirror.as_ref(),
            Kind::Registration,
        )
        .await
        {
            Ok(()) => state.reg_mirror = Some(reg_mirror.clone()),
            Err(e) => tracing::error!("registration mirror hydrate failed ({e}); {REG_FALLBACK}"),
        }
        let egress_for_cool = state.egress.clone();
        let store_for_reg = state.store.clone();
        let bus_for_broadcast = bus.clone();
        tokio::spawn(async move {
            // The consumer is ephemeral, so a NATS restart or an inactivity
            // gap deletes it server-side and the next pull errors. Ending the
            // task there froze this pod's registration mirror — and with it
            // all federation matching — for the process lifetime, while
            // /q/health still reported the bus connected. Re-open instead,
            // and re-hydrate from the store because a fresh consumer starts
            // at NEW and never replays what the gap dropped.
            loop {
                match reg_consumer.messages().await {
                    Ok(mut msgs) => {
                        while let Some(delta) = nats::next_delta(&mut msgs).await {
                            // 5.2.34 read side: a cooldown stamp updates this pod's
                            // map (a marker delta has no tenant/id — apply_delta
                            // ignores it on pods that predate the member).
                            if let Some(rid) =
                                delta.get("cooldownReg").and_then(serde_json::Value::as_str)
                            {
                                egress_for_cool
                                    .reg_record(rid, delta["ok"].as_bool().unwrap_or(false));
                                continue;
                            }
                            apply_delta(
                                reg_mirror.as_ref(),
                                &*store_for_reg,
                                Kind::Registration,
                                &delta,
                            )
                            .await;
                        }
                    }
                    Err(e) => tracing::warn!("registry broadcast consumer stream failed: {e}"),
                }
                tracing::warn!("registry broadcast consumer stream ended — reopening");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                // The ended stream's consumer is gone server-side: a pull on
                // its handle ends at once again, so the reopen needs a new one.
                match bus_for_broadcast.consume_registry_broadcast().await {
                    Ok(fresh) => reg_consumer = fresh,
                    Err(e) => {
                        tracing::warn!("registry broadcast consumer re-create failed: {e}");
                        continue;
                    }
                }
                // Already installed, so this one cannot be withheld. A failed
                // re-hydrate leaves the mirror holding what it had before the
                // gap, which the federation path will serve as current: say
                // so at error level rather than let it pass as a warning.
                if let Err(e) = antares_api::notify::reseed_mirror(
                    &*store_for_reg,
                    reg_mirror.as_ref(),
                    Kind::Registration,
                )
                .await
                {
                    tracing::error!("registration mirror re-hydrate failed ({e}); {REG_STALE}");
                }
            }
        });

        // The outbox drain. Runs on every api pod; concurrent drains are
        // absorbed by Nats-Msg-Id dedup within the duplicate window.
        // ANTARES_OUTBOX_DRAIN=off leaves the rows for another pod's drain —
        // the crash-drill lever and the dedicated-drainer split.
        let drain_on = outbox_drain_enabled()?;
        if !drain_on {
            tracing::warn!("outbox drain OFF on this pod (ANTARES_OUTBOX_DRAIN=off)");
        }
        if drain_on {
            let store = state.store.clone();
            let bus_for_drain = bus.clone();
            tokio::spawn(async move {
                loop {
                    let rows = match store.outbox_peek(64).await {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!("outbox peek failed: {e}");
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    if rows.is_empty() {
                        // woken by the same-process write hook, or the fallback
                        // poll for rows another pod failed to publish
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_millis(250),
                            nudge.notified(),
                        )
                        .await;
                        continue;
                    }
                    // Ack the EXACT published seqs — a blanket
                    // up-to-max delete loses a lower-seq row whose
                    // transaction commits between peek and ack.
                    let mut acked: Vec<i64> = Vec::new();
                    // Rows whose bodies were too big for the bus: the message
                    // carries a reference, and this row is the only copy of
                    // what it references. Kept, not deleted, and taken out of
                    // the next page by the same stamp.
                    let mut retained: Vec<(TenantId, i64)> = Vec::new();
                    let mut events: Vec<ChangeEvent> = Vec::with_capacity(rows.len());
                    for (seq, _tenant, event) in rows {
                        match serde_json::from_value::<ChangeEvent>(event) {
                            Ok(mut ev) => {
                                ev.seq = seq;
                                events.push(ev);
                            }
                            Err(e) => {
                                // an undecodable row would wedge the drain forever
                                tracing::error!("outbox row {seq} undecodable ({e}) — skipped");
                                acked.push(seq);
                            }
                        }
                    }
                    // The whole page is written before its acks are awaited
                    // (T-6608); what the stream did not acknowledge stays in
                    // the outbox and is retried from there next round.
                    let (published, failed) = bus_for_drain.publish_all(&events).await;
                    if let Some(e) = failed {
                        let of = events.len();
                        tracing::warn!("outbox publish failed after {published} of {of}: {e}");
                    }
                    for ev in events.into_iter().take(published) {
                        if ev.claim_checked_at(antares_bus::CLAIM_CHECK_BYTES) {
                            retained.push((ev.tenant, ev.seq));
                        } else {
                            acked.push(ev.seq);
                        }
                    }
                    // Retain BEFORE ack: both statements are separate
                    // transactions, and a crash between them must leave a
                    // claim-check row alive rather than published and gone.
                    // One statement per row, under that row's tenant — the
                    // outbox UPDATE takes no service escape (0005), and an
                    // event over the bus ceiling is rare enough that grouping
                    // the page by tenant would cost more code than statements.
                    for (tenant, seq) in &retained {
                        if let Err(e) = store.outbox_retain(tenant, &[*seq]).await {
                            tracing::warn!("outbox retain of seq {seq} failed: {e}");
                        }
                    }
                    if !acked.is_empty() {
                        if let Err(e) = store.outbox_ack(&acked).await {
                            tracing::warn!("outbox ack {acked:?} failed: {e}");
                        }
                    }
                }
            });
        }
    }

    if roles.matcher || roles.notifier {
        // Subscription read side: consumer-before-hydrate, same convergence argument.
        let sub_mirror = Arc::new(antares_api::mirror::SubMirror::default());
        let kv = bus.subs_kv().await?;
        let watch = kv.watch_all().await?;
        // Same rule as the registration mirror, and the same one `bus=local`
        // applies in `antares_api::wire`: a subscription absent from an installed
        // mirror never fires again, because the matcher reads candidates
        // from the mirror alone.
        match antares_api::notify::seed_mirror(
            &*state.store,
            sub_mirror.as_ref(),
            Kind::Subscription,
        )
        .await
        {
            Ok(()) => state.sub_mirror = Some(sub_mirror.clone()),
            Err(e) => tracing::error!("subscription mirror hydrate failed ({e}); {SUB_FALLBACK}"),
        }
        let store = state.store.clone();
        tokio::spawn(async move {
            // Same restart contract as the registry consumer: the watch ends
            // on a NATS restart, and a task that returns there stops seeing
            // every subscription change — i.e. this pod silently stops
            // notifying — for the process lifetime. The reopen reseeds the
            // mirror from the store: `watch_all` replays only what the bucket
            // still holds, and a delete's entry is gone after the grace.
            let mut watch = watch;
            loop {
                while let Some(entry) = watch.next().await {
                    let Ok(entry) = entry else { continue };
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&entry.value) {
                        apply_delta(sub_mirror.as_ref(), &*store, Kind::Subscription, &v).await;
                    }
                }
                tracing::warn!("subscription KV watch ended — reopening");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                match reopen_sub_watch(&kv, &*store, sub_mirror.as_ref()).await {
                    Ok(w) => watch = w,
                    Err(e) => tracing::warn!("subscription KV watch reopen failed: {e}"),
                }
            }
        });

        // The balanced matcher durable: decode → process_changes → ack AFTER.
        durables.push("matcher");
        let consumer = bus.consume_balanced("matcher").await?;
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let mut msgs = match consumer.messages().await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("matcher consumer stream failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                };
                while let Some(Ok(first)) = msgs.next().await {
                    // What the consumer already holds rides the same pass, as
                    // on the local bus (T-6608): one pass matches up to
                    // MATCH_WIDTH changes at once and folds a subscription's
                    // matches into one notification, where one message at a
                    // time matched serially on this task.
                    let mut batch = vec![first];
                    let mut ended = false;
                    while batch.len() < antares_api::notify::CHANGE_BATCH {
                        match futures_util::FutureExt::now_or_never(msgs.next()) {
                            Some(Some(Ok(m))) => batch.push(m),
                            Some(Some(Err(_)) | None) => {
                                ended = true;
                                break;
                            }
                            None => break,
                        }
                    }
                    let mut changes = Vec::with_capacity(batch.len());
                    let mut done = Vec::with_capacity(batch.len());
                    for msg in batch {
                        // Change lag = stream-publish → matcher-processing
                        // age, from the JetStream metadata timestamp.
                        if let Ok(info) = msg.info() {
                            // OffsetDateTime → SystemTime (impl in `time`/std),
                            // so no direct `time` dependency here.
                            let published: std::time::SystemTime = info.published.into();
                            if let Ok(age) = published.elapsed() {
                                metrics::histogram!("antares_change_lag_seconds")
                                    .record(age.as_secs_f64());
                            }
                        }
                        if let Some(ev) = nats::decode(&msg) {
                            // a store fault: no ack, the server redelivers it
                            let Some((before, after)) = resolve_payloads(&st, &ev).await else {
                                continue;
                            };
                            changes.push((ev.tenant.as_str().to_owned(), before, after));
                        }
                        done.push(msg);
                    }
                    antares_api::notify::process_changes(&st, changes).await;
                    for msg in done {
                        let _ = msg.ack().await;
                    }
                    if ended {
                        break;
                    }
                }
                // an errored or ended stream reopens after a pause, not in a
                // tight loop against a server that keeps refusing it
                tracing::warn!("matcher consumer stream ended — reopening");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });

        // Interval subscriptions: every matcher pod ticks; the row-lock claim
        // in interval_tick makes each firing single-winner.
        let st = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
            loop {
                tick.tick().await;
                antares_api::notify::interval_tick(&st).await;
            }
        });
    }

    // The temporal role carries no bus consumer: auto-recording is
    // synchronous in the write path (see above), and plain-mode partition
    // maintenance runs from lib.rs `spawn_background` regardless of bus mode.

    // The server must agree these are shared durables.
    bus.assert_topology(&durables).await?;
    tracing::info!(?roles, "bus=nats wired");
    Ok(bus)
}

/// Attempts a mirror write gets before the divergence is logged as an error.
const MIRROR_SYNC_ATTEMPTS: u32 = 5;

/// Mirror writes one process holds while the bus is away (S5-13); past it a
/// write is dropped and logged, the divergence a give-up already names.
const MIRROR_SYNC_QUEUE: usize = 4096;

// What a mirror that fell behind means for the pod, in its log lines.
const SUB_UNMIRRORED: &str = "this subscription is not mirrored until the next restart";
const REG_UNMIRRORED: &str = "this registration is not mirrored until the next restart";
const QUEUE_FULL: &str = "a write is not mirrored until the next restart";
const REG_FALLBACK: &str = "federation matching falls back to a store read per request";
const REG_STALE: &str = "this pod is matching against registrations from before the gap";
const SUB_FALLBACK: &str = "matching falls back to a store scan per change";
const SUB_STALE: &str = "this pod may notify subscriptions deleted during the gap";

/// One worker per mirror hook, fed through a bounded queue of keys (S5-13):
/// the writes of one row reach the bus in the order they committed, retries
/// included, so an older put can no longer land after a newer one or a
/// delete. A row written again while it waits keeps its place and sends only
/// its newest state, so a bus outage holds at most [`MIRROR_SYNC_QUEUE`]
/// rows, not one task per write. Across pods the consumer side converges
/// anyway: every delta makes it read the row from the store
/// ([`apply_delta`]).
fn ordered_sync<K, V, F, Fut>(what: &'static str, run: F) -> impl Fn(K, V) + Send + Sync + 'static
where
    K: Eq + std::hash::Hash + Clone + Send + 'static,
    V: Send + 'static,
    F: Fn(K, V) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    type Queue<K, V> = (
        std::collections::VecDeque<K>,
        std::collections::HashMap<K, V>,
    );
    let queue: Arc<std::sync::Mutex<Queue<K, V>>> = Arc::default();
    let wake = Arc::new(tokio::sync::Notify::new());
    {
        let (queue, wake) = (queue.clone(), wake.clone());
        tokio::spawn(async move {
            loop {
                let next = {
                    let mut q = queue
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let (order, latest) = &mut *q;
                    order
                        .pop_front()
                        .and_then(|k| latest.remove(&k).map(|v| (k, v)))
                };
                match next {
                    Some((k, v)) => run(k, v).await,
                    None => wake.notified().await,
                }
            }
        });
    }
    move |key, value| {
        {
            let mut q = queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (order, latest) = &mut *q;
            match latest.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    e.insert(value);
                }
                std::collections::hash_map::Entry::Vacant(_)
                    if order.len() >= MIRROR_SYNC_QUEUE =>
                {
                    metrics::counter!("antares_mirror_sync_dropped_total", "mirror" => what)
                        .increment(1);
                    tracing::error!("{what} mirror sync queue is full: {QUEUE_FULL}");
                    return;
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    order.push_back(e.key().clone());
                    e.insert(value);
                }
            }
        }
        wake.notify_one();
    }
}

/// Exponential backoff between mirror-sync attempts: 0.2 s doubling to 3.2 s,
/// so the whole ladder outlives a bus reconnect without holding a task for
/// minutes.
fn mirror_sync_backoff(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_millis(200u64 << attempt.min(4))
}

/// What one mirror delta asks for.
#[derive(Debug, PartialEq)]
pub(crate) enum Delta<'a> {
    /// A Context Source Registration Subscription was written: it is matched
    /// against registrations rather than entities, so the delta only wakes
    /// the interval sweep (5.11.7).
    Csub,
    /// The row `id` of `tenant` was written (`present`) or deleted.
    Row {
        tenant: TenantId,
        id: &'a str,
        present: bool,
    },
}

/// Decode one `{tenant, id, present, csub}` delta. A delta from a pod that
/// predates `present` carries `doc` (null for a tombstone) and is read as
/// present when that is not null; its document is not used either.
pub(crate) fn decode_delta(delta: &serde_json::Value) -> Option<Delta<'_>> {
    if delta.get("csub").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some(Delta::Csub);
    }
    let (Some(tenant), Some(id)) = (
        delta.get("tenant").and_then(serde_json::Value::as_str),
        delta.get("id").and_then(serde_json::Value::as_str),
    ) else {
        return None;
    };
    // Hydration validates the tenant before it touches the mirror; the delta
    // path must agree. A name outside the grammar can only add entries that no
    // lookup will ever hit, so an unvalidated one is unbounded growth keyed by
    // whatever reached the bus. The grammar is the whole check here: what
    // arrives is a tenant a peer broker wrote, and a write inside a Snapshot
    // (5.5.15) carries the synthetic tenant a client may not name — refusing
    // that one would drop the mirror delta the snapshot-scoped subscription
    // matches on.
    let tenant = TenantId::new_internal(tenant).ok()?;
    let present = delta
        .get("present")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| delta.get("doc").is_some_and(|d| !d.is_null()));
    Some(Delta::Row {
        tenant,
        id,
        present,
    })
}

/// Reopen the subscription KV watch, then reseed the mirror from the store.
///
/// Watch first, so a change made during the reseed waits in the new watch.
/// The reseed is what drops a subscription deleted while the old watch was
/// down: its `present:false` entry is purged `SUBS_TOMBSTONE_GRACE` after the
/// delete, so a gap longer than that replays nothing for it (T-7451).
async fn reopen_sub_watch(
    kv: &nats::KvStore,
    store: &dyn antares_store::CurrentStateDriver,
    mirror: &antares_api::mirror::SubMirror,
) -> Result<nats::KvWatch, nats::KvWatchError> {
    let watch = kv.watch_all().await?;
    if let Err(e) = antares_api::notify::reseed_mirror(store, mirror, Kind::Subscription).await {
        tracing::error!("subscription mirror re-hydrate failed ({e}); {SUB_STALE}");
    }
    Ok(watch)
}

/// Apply one delta to a mirror of `kind`: the row is read from the store,
/// which opens its sealed credentials, so the bus never carries them. A row
/// the store no longer has is a delete. A tombstone is read too (S5-13): a
/// delete's delta that arrives after the row was written again must not drop
/// the live row, so whatever order the deltas arrive in, the mirror ends at
/// what the store holds. A store that keeps failing leaves a written entry
/// as it was and logs it, the divergence the write side's retry ladder
/// names; a tombstone then still removes the entry (a removed entry stops
/// forwards and notifications, the safe side).
async fn apply_delta(
    mirror: &dyn antares_api::mirror::Mirror,
    store: &dyn antares_store::CurrentStateDriver,
    kind: Kind,
    delta: &serde_json::Value,
) {
    let (tenant, id, present) = match decode_delta(delta) {
        None => return,
        Some(Delta::Csub) => return mirror.csub_written(),
        Some(Delta::Row {
            tenant,
            id,
            present,
        }) => (tenant, id, present),
    };
    let t = &tenant;
    let tenant = t.as_str();
    for attempt in 0..MIRROR_SYNC_ATTEMPTS {
        match store.get(t, kind, id).await {
            Ok(doc) => return mirror.apply(tenant, id, doc),
            Err(e) => {
                let n = attempt + 1;
                tracing::warn!("mirror read of {kind:?} {id} attempt {n} failed: {e}");
                tokio::time::sleep(mirror_sync_backoff(attempt)).await;
            }
        }
    }
    if !present {
        tracing::error!("mirror read of {kind:?} {id} gave up: the tombstone removes it");
        return mirror.apply(tenant, id, None);
    }
    tracing::error!("mirror read of {kind:?} {id} gave up: its entry is stale until a restart");
}

/// Resolve claim-check references: bodies the bus could not carry come back
/// from the outbox row the drain kept, read by the event's own `seq`.
///
/// The store's current row is NOT that source. It answers with the entity as
/// it stands now, which is the after-image: resolving `prev_payload_ref` from
/// it hands the matcher two copies of the same document, `diff` finds nothing
/// changed and the change reaches no subscriber. It stays the fallback for
/// `payload_ref` alone, where being newer than the referenced version is the
/// ordinary at-least-once reality the matcher already tolerates.
/// `None` when the store failed to answer: the message stays unacked and
/// comes back, where reading the fault as "row gone" would drop the change.
async fn resolve_payloads(
    st: &AppState,
    ev: &ChangeEvent,
) -> Option<(Option<serde_json::Value>, Option<serde_json::Value>)> {
    if ev.payload_ref.is_none() && ev.prev_payload_ref.is_none() {
        return Some((ev.prev_payload.clone(), ev.payload.clone()));
    }
    // 0 is the local bus, which never claim-checks: it hands the payloads to
    // the matcher in process.
    let kept = match ev.seq {
        0 => None,
        seq => match st.store.outbox_event(seq, &ev.tenant).await {
            Ok(v) => v.and_then(|v| serde_json::from_value::<ChangeEvent>(v).ok()),
            Err(e) => {
                tracing::warn!("claim-check read for seq {seq} failed ({e}); redelivering");
                return None;
            }
        },
    };
    if let Some(kept) = kept {
        return Some((kept.prev_payload, kept.payload));
    }
    // Past the retention window, or a deployment whose store keeps no outbox.
    // The after-image is still recoverable; the before-image is not, and a
    // guess in its place is a notification that reports a change that did not
    // happen.
    let after = match ev.payload.clone() {
        Some(v) => Some(v),
        None => match ev.payload_ref.as_ref() {
            Some(r) => match st
                .store
                .get(&ev.tenant, Kind::Entity, r.entity_id.as_str())
                .await
            {
                Ok(doc) => doc,
                Err(e) => {
                    tracing::warn!("claim-check entity read failed ({e}); redelivering");
                    return None;
                }
            },
            None => None,
        },
    };
    if ev.prev_payload_ref.is_some() {
        metrics::counter!("antares_claim_check_unresolved_total").increment(1);
        let (seq, id) = (ev.seq, ev.entity_id.as_str());
        tracing::warn!("claim-check row for seq {seq} is gone: the change to {id} notifies nobody");
    }
    Some((ev.prev_payload.clone(), after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env;
    use antares_api::mirror::DocMirror;
    use antares_bus::{ChangeOp, PayloadRef};
    use antares_model::EntityId;
    use serde_json::json;

    /// What this thread logs while the guard lives: the bus and mirror paths
    /// report through `tracing` alone, so a test reads the line they owe.
    #[derive(Clone, Default)]
    struct Logged(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Logged {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log").extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Logged {
        fn start() -> (Self, tracing::subscriber::DefaultGuard) {
            let log = Self::default();
            let writer = log.clone();
            let guard = tracing::subscriber::set_default(
                tracing_subscriber::fmt()
                    .with_ansi(false)
                    .with_max_level(tracing::Level::DEBUG)
                    .with_writer(move || writer.clone())
                    .finish(),
            );
            (log, guard)
        }

        fn text(&self) -> String {
            std::io::Write::flush(&mut self.clone()).expect("flush");
            String::from_utf8_lossy(&self.0.lock().expect("log")).into_owned()
        }
    }

    /// The mirror-sync retry waits 0.2 s and doubles to 3.2 s, where it stays
    /// however many attempts follow (no shift overflow at u32::MAX).
    #[test]
    fn mirror_sync_backoff_doubles_to_a_ceiling() {
        let ms = |a| mirror_sync_backoff(a).as_millis();
        assert_eq!(
            (0..6).map(ms).collect::<Vec<_>>(),
            [200, 400, 800, 1600, 3200, 3200]
        );
        assert_eq!(ms(u32::MAX), 3200);
    }

    #[test]
    fn roles_parse_accepts_the_role_set_and_refuses_anything_else() {
        assert!(Roles::parse("all").expect("all").all());
        let r = Roles::parse("api").expect("api");
        assert!(r.api && !r.matcher && !r.notifier && !r.temporal && !r.registry);
        assert!(
            !r.all(),
            "a single role must never claim to be the full set"
        );
        let r = Roles::parse(" matcher , notifier ").expect("padded list");
        assert!(r.matcher && r.notifier && !r.api);
        // The enumerated full set is the same thing as "all" — a role split
        // that happens to name every role must still pass the bus=local gate.
        assert!(Roles::parse("api,matcher,notifier,temporal,registry")
            .expect("full list")
            .all());

        for bad in ["", "api,", "ALL", "apis", "api;matcher", "worker", " "] {
            let err = Roles::parse(bad).expect_err(&format!("ANTARES_ROLES={bad:?} must be fatal"));
            assert!(err.starts_with("unknown role"), "{bad:?}: {err}");
        }
    }

    /// The KV key must be legal for a NATS KV bucket (`:` from a URN is not),
    /// stable across calls, and collision-free per id, per tenant and per
    /// kind — 5.5.10 leaves the id to the client, so one URN can name both a
    /// Subscription and a Context Source Registration Subscription.
    #[test]
    fn kv_key_is_bucket_legal_and_stable() {
        let sub = Kind::Subscription;
        let k = kv_key("default", sub, "urn:ngsi-ld:Subscription:1");
        assert_eq!(
            k,
            kv_key("default", sub, "urn:ngsi-ld:Subscription:1"),
            "stable"
        );
        assert!(
            k.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'),
            "illegal KV key character in {k:?}"
        );
        assert!(k.starts_with("default."), "tenant scoping lost: {k}");
        assert_ne!(
            kv_key("default", sub, "urn:ngsi-ld:Subscription:1"),
            kv_key("default", sub, "urn:ngsi-ld:Subscription:2")
        );
        assert_ne!(
            kv_key("t1", sub, "urn:ngsi-ld:Subscription:1"),
            kv_key("t2", sub, "urn:ngsi-ld:Subscription:1")
        );
        assert_ne!(
            kv_key("default", sub, "urn:ngsi-ld:Subscription:1"),
            kv_key(
                "default",
                Kind::CSourceSubscription,
                "urn:ngsi-ld:Subscription:1"
            ),
            "one id naming both kinds must not share a mirror entry"
        );
    }

    /// A Context Source Registration Subscription delta carries no document:
    /// it wakes the interval sweep and touches nothing the matcher reads.
    #[tokio::test]
    async fn a_csub_delta_only_wakes_the_sweep() {
        let st = AppState::new("antares".into());
        let m = antares_api::mirror::SubMirror::default();
        apply_delta(
            &m,
            &*st.store,
            Kind::Subscription,
            &serde_json::json!({
                "tenant": "default", "id": "urn:ngsi-ld:CSourceSubscription:1",
                "present": true, "csub": true,
            }),
        )
        .await;
        assert!(
            m.docs("default").is_empty(),
            "a csource subscription must not enter the candidate index"
        );
    }

    /// T-7296: a delta names a row; the mirror takes the document the store
    /// holds for it, never one the delta carries, and a row the store no
    /// longer has leaves the mirror.
    #[tokio::test]
    async fn a_delta_takes_the_document_from_the_store() {
        let st = AppState::new("antares".into());
        let stored = json!({"id": "urn:x:1", "type": "ContextSourceRegistration",
                            "endpoint": "http://cs.example"});
        st.store
            .create(
                &TenantId::default(),
                Kind::Registration,
                "urn:x:1",
                stored.clone(),
            )
            .await
            .expect("seed");
        let m = DocMirror::default();
        // a delta from a pod before `present` still carries a document
        apply_delta(
            &m,
            &*st.store,
            Kind::Registration,
            &json!({"tenant": "default", "id": "urn:x:1",
                    "doc": {"id": "urn:x:1", "endpoint": "http://elsewhere"}}),
        )
        .await;
        let docs = m.docs("default");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["endpoint"], stored["endpoint"], "{:?}", docs[0]);
        st.store
            .delete(&TenantId::default(), Kind::Registration, "urn:x:1")
            .await
            .expect("delete");
        apply_delta(
            &m,
            &*st.store,
            Kind::Registration,
            &json!({"tenant": "default", "id": "urn:x:1", "present": true}),
        )
        .await;
        assert!(m.docs("default").is_empty(), "a row gone from the store");
    }

    /// S5-13: a registration deleted and written again, its two deltas
    /// arriving in the other order: the late tombstone does not drop the
    /// row the store holds, and a late write of a deleted row does not bring
    /// it back.
    #[tokio::test]
    async fn deltas_in_any_order_leave_the_mirror_at_the_store() {
        let st = AppState::new("antares".into());
        let t = TenantId::default();
        let m = DocMirror::default();
        let row = json!({"id": "urn:x:2", "type": "ContextSourceRegistration",
                         "endpoint": "http://cs.example"});
        st.store
            .create(&t, Kind::Registration, "urn:x:2", row)
            .await
            .expect("written again");
        for present in [true, false] {
            apply_delta(
                &m,
                &*st.store,
                Kind::Registration,
                &json!({"tenant": "default", "id": "urn:x:2", "present": present}),
            )
            .await;
        }
        assert_eq!(m.docs("default").len(), 1, "the live row stays");
        st.store
            .delete(&t, Kind::Registration, "urn:x:2")
            .await
            .expect("delete");
        for present in [false, true] {
            apply_delta(
                &m,
                &*st.store,
                Kind::Registration,
                &json!({"tenant": "default", "id": "urn:x:2", "present": present}),
            )
            .await;
        }
        assert!(m.docs("default").is_empty(), "the deleted row stays gone");
    }

    /// S5-13: the sync worker sends rows in the order they were handed
    /// over, a slow first write included; a row written again while it
    /// waits sends only its newest state and takes no second place; a full
    /// queue refuses a new row instead of growing.
    #[tokio::test]
    async fn mirror_sync_keeps_order_coalesces_and_bounds_its_queue() {
        let (log, _guard) = Logged::start();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let sync = {
            let (seen, gate) = (seen.clone(), gate.clone());
            ordered_sync("test", move |k: usize, v: usize| {
                let (seen, gate) = (seen.clone(), gate.clone());
                async move {
                    if k == 0 {
                        let _ = gate.acquire().await;
                    }
                    seen.lock().expect("seen").push((k, v));
                }
            })
        };
        sync(0, 0);
        // the worker takes the first row and holds it; the queue fills behind
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        for k in 1..=MIRROR_SYNC_QUEUE + 1 {
            sync(k, k);
        }
        sync(1, 999);
        gate.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while seen.lock().expect("seen").len() < MIRROR_SYNC_QUEUE + 1 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("drained");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let seen = seen.lock().expect("seen").clone();
        let mut want: Vec<(usize, usize)> = (0..=MIRROR_SYNC_QUEUE).map(|k| (k, k)).collect();
        want[1] = (1, 999);
        assert_eq!(seen, want, "in order, coalesced, the overflow refused");
        assert!(
            log.text()
                .contains("test mirror sync queue is full: a write is not mirrored"),
            "a refused row is logged: {}",
            log.text()
        );
    }

    /// The KV/registry mirrors are fed from the bus. A malformed or hostile
    /// delta must be dropped — never panic a consumer task, and never grow
    /// the mirror under a key no request can ever address (the tenant is
    /// validated on every request, so an unvalidated one is pure ballast).
    #[tokio::test]
    async fn apply_delta_ignores_malformed_and_hostile_deltas() {
        let st = AppState::new("antares".into());
        st.store
            .create(
                &TenantId::default(),
                Kind::Registration,
                "urn:x:1",
                json!({"id": "urn:x:1"}),
            )
            .await
            .expect("seed");
        let m = DocMirror::default();
        let apply = |delta: serde_json::Value| {
            let (m, store) = (&m, st.store.clone());
            async move { apply_delta(m, &*store, Kind::Registration, &delta).await }
        };
        apply(json!({"tenant": "default", "id": "urn:x:1", "present": true})).await;
        assert_eq!(m.docs("default").len(), 1, "a good delta must apply");

        // tombstone, sent once the row is gone
        st.store
            .delete(&TenantId::default(), Kind::Registration, "urn:x:1")
            .await
            .expect("delete");
        apply(json!({"tenant": "default", "id": "urn:x:1", "present": false})).await;
        assert!(m.docs("default").is_empty(), "a tombstone must delete");
        assert!(m.tenants().is_empty(), "an emptied tenant must not linger");

        // Shapes that must be ignored without panicking.
        for junk in [
            json!({}),
            json!(null),
            json!(42),
            json!("scalar"),
            json!([1, 2, 3]),
            json!({"tenant": "default"}),
            json!({"id": "urn:x:1"}),
            json!({"tenant": 7, "id": "urn:x:1", "present": true}),
            json!({"tenant": "default", "id": null, "present": true}),
            json!({"cooldownReg": "urn:reg:1", "ok": false}),
        ] {
            apply(junk).await;
        }
        assert!(m.tenants().is_empty(), "junk deltas grew the mirror");

        // Hostile tenants: not addressable by any request (the header is
        // validated to [A-Za-z0-9_-]{1,128}), so they may not take memory.
        for hostile in [
            "a".repeat(4096),
            "../../etc".into(),
            "a b".into(),
            "".into(),
        ] {
            apply(json!({"tenant": hostile, "id": "urn:x:1", "present": true})).await;
        }
        assert!(
            m.tenants().is_empty(),
            "an unaddressable tenant grew the mirror without bound: {:?}",
            m.tenants()
        );
    }

    async fn state_with(entity: Option<serde_json::Value>) -> AppState {
        let st = AppState::new("antares".into());
        if let Some(doc) = entity {
            let id = doc["id"].as_str().expect("id").to_owned();
            st.store
                .create(&TenantId::default(), Kind::Entity, &id, doc)
                .await
                .expect("seed");
        }
        st
    }

    fn event(payload: Option<serde_json::Value>, r#ref: Option<PayloadRef>) -> ChangeEvent {
        ChangeEvent {
            tenant: TenantId::default(),
            entity_id: EntityId::new("urn:ngsi-ld:T:1").expect("id"),
            types: vec!["T".into()],
            op: ChangeOp::Update,
            changed_attrs: vec![],
            payload,
            prev_payload: None,
            version: 1,
            incarnation: String::new(),
            seq: 0,
            payload_ref: r#ref,
            prev_payload_ref: None,
        }
    }

    /// Claim-check resolution: inline wins, a reference is fetched, and a
    /// reference to a row that is gone resolves to None instead of panicking
    /// the matcher task.
    #[tokio::test]
    async fn resolve_payloads_prefers_inline_and_tolerates_a_dangling_reference() {
        let doc = json!({"id": "urn:ngsi-ld:T:1", "type": "T"});
        let st = state_with(Some(doc.clone())).await;

        let (before, after) = resolve_payloads(&st, &event(Some(json!({"inline": true})), None))
            .await
            .expect("resolved");
        assert_eq!(after, Some(json!({"inline": true})), "inline payload wins");
        assert_eq!(before, None);

        let r = PayloadRef {
            entity_id: EntityId::new("urn:ngsi-ld:T:1").expect("id"),
            version: 1,
        };
        let (_, after) = resolve_payloads(&st, &event(None, Some(r.clone())))
            .await
            .expect("resolved");
        assert_eq!(
            after.as_ref().and_then(|a| a["id"].as_str()),
            Some("urn:ngsi-ld:T:1"),
            "a claim-check reference must be fetched from the store"
        );

        // The row was deleted between publish and consumption.
        let gone = state_with(None).await;
        let (before, after) = resolve_payloads(&gone, &event(None, Some(r)))
            .await
            .expect("resolved");
        assert_eq!(after, None, "a dangling reference must resolve to None");
        assert_eq!(before, None);

        let (before, after) = resolve_payloads(&st, &event(None, None))
            .await
            .expect("resolved");
        assert!(before.is_none() && after.is_none(), "no payload, no fetch");
    }

    /// A before-image that only a claim check carried, with no outbox row
    /// left to read it from, cannot be guessed: the after-image still
    /// resolves (inline, or nothing when the event carried none), the
    /// before-image stays empty, and the loss is counted and logged.
    #[tokio::test]
    async fn a_before_image_past_the_outbox_is_left_empty_and_logged() {
        let (log, _guard) = Logged::start();
        let st = state_with(None).await;
        let r = PayloadRef {
            entity_id: EntityId::new("urn:ngsi-ld:T:1").expect("id"),
            version: 1,
        };
        let mut ev = event(Some(json!({"inline": true})), None);
        ev.prev_payload_ref = Some(r.clone());
        let (before, after) = resolve_payloads(&st, &ev).await.expect("resolved");
        assert_eq!(before, None, "no guessed before-image");
        assert_eq!(
            after,
            Some(json!({"inline": true})),
            "inline after-image kept"
        );

        let mut ev = event(None, None);
        ev.prev_payload_ref = Some(r);
        let (before, after) = resolve_payloads(&st, &ev).await.expect("resolved");
        assert!(before.is_none() && after.is_none(), "nothing to resolve");
        assert!(
            log.text().contains(
                "claim-check row for seq 0 is gone: the change to urn:ngsi-ld:T:1 notifies nobody"
            ),
            "{}",
            log.text()
        );
    }

    /// A store that keeps failing the mirror's read: the delta is retried
    /// on the backoff ladder, then a tombstone still removes its entry (the
    /// safe side) while a written row leaves the entry as it was, each with
    /// the line that names the divergence.
    #[tokio::test]
    async fn a_failing_store_retries_then_a_tombstone_still_removes_the_entry() {
        let (log, _guard) = Logged::start();
        let pool = antares_sql::sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect_lazy("postgres://nobody@localhost:1/antares_no_such_db")
            .expect("lazy pool");
        let store =
            antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(pool));
        let m = DocMirror::default();
        m.apply("default", "urn:x:1", Some(json!({"id": "urn:x:1"})));
        m.apply("default", "urn:x:2", Some(json!({"id": "urn:x:2"})));
        // both ladders at once: 6.2 s of backoff each
        let written = json!({"tenant": "default", "id": "urn:x:2", "present": true});
        let tombstone = json!({"tenant": "default", "id": "urn:x:1", "present": false});
        tokio::join!(
            apply_delta(&m, &store, Kind::Registration, &written),
            apply_delta(&m, &store, Kind::Registration, &tombstone),
        );
        let left: Vec<_> = m
            .docs("default")
            .into_iter()
            .map(|d| d["id"].clone())
            .collect();
        assert_eq!(
            left,
            [json!("urn:x:2")],
            "the tombstone removed its entry though no read answered; the written row's stays"
        );
        let text = log.text();
        assert!(
            text.contains("mirror read of Registration urn:x:2 attempt 5 failed"),
            "{text}"
        );
        assert!(
            text.contains("urn:x:2 gave up: its entry is stale until a restart"),
            "{text}"
        );
        assert!(
            text.contains("urn:x:1 gave up: the tombstone removes it"),
            "{text}"
        );
    }

    /// A store fault while resolving a claim check is not a row that is
    /// gone: the change must come back (no ack), not notify nobody.
    #[tokio::test]
    async fn a_store_fault_resolving_a_claim_check_leaves_the_change_for_redelivery() {
        let pool = antares_sql::sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy("postgres://nobody@localhost:1/antares_no_such_db")
            .expect("lazy pool");
        let st = AppState::with_store(
            "antares".into(),
            std::sync::Arc::new(antares_sql::store::any::AnyStore::Pg(
                antares_sql::store::any::PgBackend::new(pool),
            )),
            "postgres",
        );
        let r = PayloadRef {
            entity_id: EntityId::new("urn:ngsi-ld:T:1").expect("id"),
            version: 2,
        };
        let mut ev = event(None, Some(r.clone()));
        ev.prev_payload_ref = Some(r);
        ev.seq = 7;
        assert_eq!(resolve_payloads(&st, &ev).await, None, "outbox read fault");
        ev.seq = 0;
        assert_eq!(resolve_payloads(&st, &ev).await, None, "entity read fault");
    }

    /// A claim-check fetch is scoped to the EVENT's tenant: a reference must
    /// never resolve against another tenant's row of the same id.
    #[tokio::test]
    async fn resolve_payloads_never_crosses_a_tenant_boundary() {
        let st = state_with(Some(json!({"id": "urn:ngsi-ld:T:1", "type": "T"}))).await;
        let mut ev = event(
            None,
            Some(PayloadRef {
                entity_id: EntityId::new("urn:ngsi-ld:T:1").expect("id"),
                version: 1,
            }),
        );
        ev.tenant = TenantId::new("other").expect("tenant");
        let (_, after) = resolve_payloads(&st, &ev).await.expect("resolved");
        assert_eq!(
            after, None,
            "a reference resolved another tenant's entity: {after:?}"
        );
    }

    /// A change whose bodies were both too big for the bus reaches the
    /// matcher with the before-image the write actually replaced. Resolving
    /// the reference against the store's current row instead hands back the
    /// after-image twice, `diff` finds nothing and the change notifies
    /// nobody. Needs ANTARES_TEST_DATABASE_URL (ci.yml antares-test sets it) — the outbox is a
    /// Postgres table, and the memory arm has none.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_retained_row_gives_the_matcher_the_before_image_the_write_replaced() {
        let url = test_env::db();
        let pool = antares_sql::store::pg::connect(&url, 5)
            .await
            .expect("connect");
        let tenant = TenantId::new("claimcheck").expect("tenant");
        antares_sql::store::pg::ensure_tenant(&pool, &tenant)
            .await
            .expect("tenant row");

        let id = "urn:ngsi-ld:T:oversized";
        let wide = |v: &str| {
            json!({"id": id, "type": ["T"],
                   "https://uri.etsi.org/ngsi-ld/default-context/note":
                       [{"type": "Property", "value": format!("{v}{}", "x".repeat(300 * 1024))}]})
        };
        let before_doc = wide("before-");
        let after_doc = wide("after-");

        let mut ev = ChangeEvent {
            tenant: tenant.clone(),
            entity_id: EntityId::new(id).expect("id"),
            types: vec!["T".into()],
            op: ChangeOp::Update,
            changed_attrs: vec![],
            payload: Some(after_doc.clone()),
            prev_payload: Some(before_doc.clone()),
            version: 2,
            incarnation: String::new(),
            seq: 0,
            payload_ref: None,
            prev_payload_ref: None,
        };
        let mut tx = pool.begin().await.expect("tx");
        antares_sql::store::pg::set_tenant(&mut tx, &tenant)
            .await
            .expect("set tenant");
        let seq = antares_sql::store::pg::outbox::enqueue(
            &mut tx,
            &tenant,
            &serde_json::to_value(&ev).expect("event json"),
        )
        .await
        .expect("enqueue");
        tx.commit().await.expect("commit");
        antares_sql::store::pg::outbox::retain(&pool, &tenant, &[seq])
            .await
            .expect("retain");

        let st = AppState::with_store(
            "antares".into(),
            std::sync::Arc::new(antares_sql::store::any::AnyStore::Pg(
                antares_sql::store::any::PgBackend::new(pool.clone()),
            )),
            "postgres",
        );
        // The current row is the AFTER image — the document the old
        // resolution handed back for both halves.
        let _ = st.store.delete(&tenant, Kind::Entity, id).await;
        st.store
            .create(&tenant, Kind::Entity, id, after_doc.clone())
            .await
            .expect("seed current row");

        ev.seq = seq;
        let wire = ev.claim_check(antares_bus::CLAIM_CHECK_BYTES);
        assert!(
            wire.prev_payload_ref.is_some() && wire.payload_ref.is_some(),
            "the fixture must be over the claim-check ceiling"
        );
        let (before, after) = resolve_payloads(&st, &wire).await.expect("resolved");
        assert_eq!(before.as_ref(), Some(&before_doc), "before-image lost");
        assert_eq!(after.as_ref(), Some(&after_doc));
        assert_ne!(
            before, after,
            "both halves resolved to the current row: the change notifies nobody"
        );

        let _ = st.store.delete(&tenant, Kind::Entity, id).await;
        let _ = antares_sql::store::pg::outbox::reap_published(&pool, 0).await;
    }

    /// T-7451, against a live NATS (env-gated on ANTARES_TEST_NATS_URL). Pod A's
    /// watch is down while pod B deletes a subscription, and the gap outlasts
    /// the grace: B's retire has purged the `present:false` entry, so A's
    /// reopened `watch_all` replays nothing for it. The reseed is what takes
    /// the subscription out of A's mirror; the matcher reads candidates from
    /// that mirror alone, so an empty candidate set is A sending nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_watch_reopened_past_the_grace_drops_a_subscription_deleted_meanwhile() {
        let Some(url) = test_env::nats() else { return };
        const VEHICLE: &str = "https://example.org/Vehicle";
        let bus = NatsBus::connect(&url).await.expect("connect");
        let kv = bus.subs_kv().await.expect("kv bucket");
        let st = AppState::new("antares".into());
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let name = format!("t7451x{nonce}");
        let tenant = TenantId::new(&name).expect("tenant");
        let id = "urn:ngsi-ld:Subscription:revoked";
        let doc = json!({"id": id, "type": "Subscription", "entities": [{"type": VEHICLE}],
                         "notification": {"endpoint": {"uri": "http://receiver.invalid/n"}}});
        st.store
            .create(&tenant, Kind::Subscription, id, doc)
            .await
            .expect("create");
        let key = kv_key(&name, Kind::Subscription, id);
        let entry = |present: bool| {
            serde_json::to_vec(&json!({"tenant": name, "id": id, "present": present}))
                .expect("json")
        };
        kv.put(&key, entry(true).into()).await.expect("put");
        let mirror = antares_api::mirror::SubMirror::default();
        antares_api::notify::seed_mirror(&*st.store, &mirror, Kind::Subscription)
            .await
            .expect("seed");
        assert_eq!(mirror.candidates(&name, &[VEHICLE], &[]).len(), 1);

        // A's watch is down. B deletes the subscription, and its retire runs:
        // the step the sub_sync hook takes once SUBS_TOMBSTONE_GRACE is over.
        st.store
            .delete(&tenant, Kind::Subscription, id)
            .await
            .expect("delete");
        let revision = kv.put(&key, entry(false).into()).await.expect("tombstone");
        bus.retire_subscription_key(&key, revision)
            .await
            .expect("retire");

        let mut watch = reopen_sub_watch(&kv, &*st.store, &mirror)
            .await
            .expect("reopen");
        while let Ok(Some(Ok(e))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), watch.next()).await
        {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&e.value) {
                apply_delta(&mirror, &*st.store, Kind::Subscription, &v).await;
            }
        }
        assert!(
            mirror.candidates(&name, &[VEHICLE], &[]).is_empty(),
            "a change to a Vehicle still reaches the subscription B deleted during the gap"
        );
    }

    /// A store that cannot list the Tenants holding subscriptions leaves both
    /// mirrors uninstalled, so federation and matching read the store; an
    /// outbox it cannot peek is retried after a pause. Wiring still succeeds.
    #[tokio::test]
    async fn a_store_that_cannot_seed_leaves_the_mirrors_uninstalled() {
        let Some(url) = test_env::nats() else { return };
        let (log, _guard) = Logged::start();
        let (mut state, fault) = crate::fault_store::FaultStore::state("wiring.test");
        fault.fail(|m, _, _| matches!(m, "subscription_tenants" | "outbox_peek"));
        let roles = Roles::parse("all").expect("roles");
        wire_nats(&mut state, &url, roles).await.expect("wired");
        assert!(
            state.reg_mirror.is_none(),
            "a half mirror is never installed"
        );
        assert!(
            state.sub_mirror.is_none(),
            "a half mirror is never installed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
        let text = log.text();
        for line in [
            "registration mirror hydrate failed",
            "subscription mirror hydrate failed",
            "outbox peek failed",
        ] {
            assert!(text.contains(line), "{line}: {text}");
        }
    }

    /// The outbox drain over a Postgres store that refuses to keep or delete what it
    /// published: the event over the bus ceiling it cannot retain and the rows it cannot ack
    /// are each logged and left in the outbox, to be published again, and the drain goes on.
    /// Needs ANTARES_TEST_DATABASE_URL (ci.yml antares-test) and a live NATS.
    #[tokio::test]
    async fn an_outbox_the_store_cannot_retain_or_ack_is_logged_and_kept() {
        let Some(url) = test_env::nats() else { return };
        let pool = antares_sql::store::pg::connect(&test_env::db(), 5)
            .await
            .expect("connect");
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let tenant = TenantId::new(&format!("outboxfault{nonce}")).expect("tenant");
        antares_sql::store::pg::ensure_tenant(&pool, &tenant)
            .await
            .expect("tenant row");
        let event = |id: &str, note: usize| {
            let doc = json!({"id": id, "type": ["T"],
                "https://uri.etsi.org/ngsi-ld/default-context/note":
                    [{"type": "Property", "value": "x".repeat(note)}]});
            ChangeEvent {
                tenant: tenant.clone(),
                entity_id: EntityId::new(id).expect("id"),
                types: vec!["T".into()],
                op: ChangeOp::Create,
                changed_attrs: vec![],
                payload: Some(doc),
                prev_payload: None,
                version: 1,
                incarnation: String::new(),
                seq: 0,
                payload_ref: None,
                prev_payload_ref: None,
            }
        };
        let mut seqs = Vec::new();
        for (id, note) in [
            ("urn:ngsi-ld:T:small", 8),
            ("urn:ngsi-ld:T:wide", 600 * 1024),
        ] {
            let mut tx = pool.begin().await.expect("tx");
            antares_sql::store::pg::set_tenant(&mut tx, &tenant)
                .await
                .expect("set tenant");
            let value = serde_json::to_value(event(id, note)).expect("event json");
            seqs.push(
                antares_sql::store::pg::outbox::enqueue(&mut tx, &tenant, &value)
                    .await
                    .expect("enqueue"),
            );
            tx.commit().await.expect("commit");
        }
        let (log, _guard) = Logged::start();
        let store = Arc::new(antares_sql::store::any::AnyStore::Pg(
            antares_sql::store::any::PgBackend::new(pool.clone()),
        ));
        let fault = Arc::new(crate::fault_store::FaultStore {
            inner: store.clone(),
            rule: std::sync::Mutex::new(None),
            unsupported: std::sync::Mutex::new(None),
            before: std::sync::Mutex::new(None),
        });
        fault.fail(|m, _, _| matches!(m, "outbox_retain" | "outbox_ack"));
        let mut state =
            AppState::with_drivers("wiring.test".into(), fault.clone(), store, "postgres");
        wire_nats(&mut state, &url, Roles::parse("api").expect("roles"))
            .await
            .expect("wired");
        let wide = seqs.last().copied().unwrap_or_default();
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(20))
            .expect("deadline");
        while !(log
            .text()
            .contains(&format!("outbox retain of seq {wide} failed"))
            && log.text().contains("outbox ack"))
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let text = log.text();
        assert!(
            text.contains(&format!("outbox retain of seq {wide} failed")),
            "{text}"
        );
        assert!(
            text.contains("outbox ack") && text.contains("failed"),
            "{text}"
        );
        let kept = antares_sql::store::pg::outbox::event(
            &pool,
            seqs.first().copied().unwrap_or_default(),
            &tenant,
        )
        .await
        .expect("read");
        assert!(kept.is_some(), "a row whose ack failed left the outbox");
        let _ = antares_sql::store::pg::outbox::ack(&pool, &seqs).await;
    }

    /// A claim-checked change whose Entity the store cannot read now reaches the matcher, which
    /// leaves it unacknowledged for the server to deliver again rather than match a change it
    /// cannot see; the failed read is logged. Needs a live NATS.
    #[tokio::test]
    async fn a_claim_checked_change_the_store_cannot_read_is_left_for_redelivery() {
        let Some(url) = test_env::nats() else { return };
        let (log, _guard) = Logged::start();
        let (mut state, fault) = crate::fault_store::FaultStore::state("wiring.test");
        wire_nats(&mut state, &url, Roles::parse("matcher").expect("roles"))
            .await
            .expect("wired");
        fault.fail(|m, _, kind| m == "get" && kind == Some(Kind::Entity));
        let bus = NatsBus::connect(&url).await.expect("connect");
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let tenant = TenantId::new(&format!("claimfault{nonce}")).expect("tenant");
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(20))
            .expect("deadline");
        let mut n = 0u32;
        while !log.text().contains("claim-check entity read failed")
            && std::time::Instant::now() < deadline
        {
            n = n.saturating_add(1);
            let id = format!("urn:ngsi-ld:T:far{n}");
            let ev = ChangeEvent {
                tenant: tenant.clone(),
                entity_id: EntityId::new(&id).expect("id"),
                types: vec!["T".into()],
                op: ChangeOp::Update,
                changed_attrs: vec![],
                payload: None,
                prev_payload: None,
                version: 2,
                incarnation: String::new(),
                seq: 0,
                payload_ref: Some(PayloadRef {
                    entity_id: EntityId::new(&id).expect("id"),
                    version: 2,
                }),
                prev_payload_ref: None,
            };
            bus.publish(&ev).await.expect("publish");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let text = log.text();
        assert!(text.contains("claim-check entity read failed"), "{text}");
        assert!(text.contains("redelivering"), "{text}");
    }

    /// The outbox-drain switch is a config value like any other: the two
    /// documented spellings decide, anything else is fatal instead of
    /// silently leaving the drain on (a typo'd `ANTARES_OUTBOX_DRAIN=of`
    /// would otherwise read as "on" and quietly defeat the crash drill).
    #[test]
    fn outbox_drain_switch_is_total() {
        std::env::remove_var("ANTARES_OUTBOX_DRAIN");
        assert!(outbox_drain_enabled().expect("default"), "default is on");
        std::env::set_var("ANTARES_OUTBOX_DRAIN", "off");
        assert!(!outbox_drain_enabled().expect("off"));
        std::env::set_var("ANTARES_OUTBOX_DRAIN", "on");
        assert!(outbox_drain_enabled().expect("on"));
        for bad in ["", "of", "false", "0", "OFF", "no"] {
            std::env::set_var("ANTARES_OUTBOX_DRAIN", bad);
            let err = outbox_drain_enabled()
                .expect_err(&format!("ANTARES_OUTBOX_DRAIN={bad:?} must be fatal"));
            assert!(err.contains("ANTARES_OUTBOX_DRAIN"), "{err}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            std::env::set_var("ANTARES_OUTBOX_DRAIN", std::ffi::OsStr::from_bytes(&[0xff]));
            let err = outbox_drain_enabled().expect_err("a non-UTF-8 value is fatal");
            assert!(
                err.starts_with("ANTARES_OUTBOX_DRAIN is unreadable"),
                "{err}"
            );
        }
        std::env::remove_var("ANTARES_OUTBOX_DRAIN");
    }
}
