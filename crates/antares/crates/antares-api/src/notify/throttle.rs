// SPDX-License-Identifier: EUPL-1.2
//! The change drain and its delivery lanes: batching, per-(tenant, subscription) folding, the byte ledger and the per-tenant delivery slots.

use super::*;

/// Groups dropped from a full delivery lane, since process start (T-4086).
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn lane_drops() -> u64 {
    LANE_DROPS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Changes one drain of the queue folds into one delivery pass — a batch
/// request's N writes arrive as N events back to back and leave as ONE
/// notification per matching subscription. Bounded so a flood cannot hold
/// the first notification back indefinitely.
pub const CHANGE_BATCH: usize = 256;

/// The estimated heap one drain's groups may hold before they leave for their
/// lanes (T-4562): an eighth of what the lanes hold together.
#[cfg(not(target_arch = "wasm32"))]
pub(super) static DRAIN_BYTES: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| *crate::bounds::DELIVERY_LANE_BYTES / 8);

/// One matched (subscription, entity) pair before delivery.
pub(super) struct Matched {
    pub(super) tenant: TenantId,
    /// Shared with the mirror that holds it: a change is evaluated against
    /// every candidate subscription, so owning a copy here made the cost of
    /// a change scale with the size of those documents.
    pub(super) sub: Arc<Value>,
    pub(super) ctx: Arc<Context>,
    pub(super) data: Vec<Value>,
}

pub async fn process_change(
    st: &AppState,
    tenant_str: &str,
    before: Option<Value>,
    after: Option<Value>,
) {
    process_changes(st, vec![(tenant_str.to_owned(), before, after)]).await;
}

/// 5.8.6: "the Notification ... data ... shall contain the Entities that
/// match" — every change of one drain that matches the same subscription
/// travels in one notification, so a batch of N entities is one POST with N
/// data entries (and timesSent moves by one), never N POSTs.
pub async fn process_changes(st: &AppState, changes: Vec<Change>) {
    process_queued(
        st,
        changes.into_iter().map(|c| (u64::MAX, c)).collect(),
        None,
    )
    .await;
}

/// [`process_changes`] of changes each stamped with the [`crate::mirror::tick`]
/// they were queued at.
///
/// With `done` (the matcher queue's drain), each group is handed to its
/// subscription's lane and the drain goes on to the next batch: a slow or
/// dead endpoint holds its own lane, never the matching of every other
/// change (T-4086). `done` falls when the last group of the batch has left.
/// Without it (a bus event, a direct call), the groups are delivered before
/// this returns.
pub(super) async fn process_queued(
    st: &AppState,
    changes: Vec<(u64, Change)>,
    done: Option<Arc<BatchDone>>,
) {
    let mut groups: Vec<Matched> = Vec::new();
    // (tenant, subscription id) → its group. A scan for the group would be
    // linear in the subscriptions already matched, and a drain of
    // CHANGE_BATCH changes over S matching subscriptions walks it S times per
    // change: quadratic in the one dimension this broker is built to grow
    // (100 000 subscriptions), on the notification hot path.
    let mut index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    // estimated heap of the groups this drain holds (T-4562)
    #[cfg(not(target_arch = "wasm32"))]
    let mut held = 0usize;
    let mut matched_in_order = matched_in_order(st, changes);
    while let Some(matched) = futures_util::StreamExt::next(&mut matched_in_order).await {
        for m in matched {
            #[cfg(not(target_arch = "wasm32"))]
            {
                held = m
                    .data
                    .iter()
                    .fold(held, |n, e| n.saturating_add(heap_estimate(e)));
            }
            let key = (
                m.tenant.as_str().to_owned(),
                m.sub
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            );
            match index.get(&key) {
                Some(&i) => {
                    if let Some(group) = groups.get_mut(i) {
                        group.data.extend(m.data);
                    }
                }
                None => {
                    index.insert(key, groups.len());
                    groups.push(m);
                }
            }
        }
        // A drain of CHANGE_BATCH large changes over many subscriptions held
        // a shaped copy per (subscription, entity) until its end: 256 changes
        // of 3 KB entities times 20 subscriptions was 750 MB (T-4562). Past
        // DRAIN_BYTES the groups so far leave for their lanes; a lane is
        // FIFO, so a subscription's changes still leave in drain order.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(done) = &done {
            if held > *DRAIN_BYTES {
                for g in groups.drain(..) {
                    to_lane(st, g, done);
                }
                index.clear();
                held = 0;
            }
        }
    }
    // Groups are distinct subscriptions, so they leave concurrently; a
    // subscription's changes in this drain are already one group. On a lane
    // its groups leave in drain order; awaited, the next pass starts only
    // after this one. Either way per-subscription order holds.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(done) = done {
        for g in groups {
            to_lane(st, g, &done);
        }
        return;
    }
    // Each group is its own task: a group's bookkeeping reads and writes
    // await the store, and one task's `for_each_concurrent` would still put
    // every group's round-trips on a single task's poll budget.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let tenants: std::collections::HashSet<TenantId> =
            groups.iter().map(|g| g.tenant.clone()).collect();
        let mut set = tokio::task::JoinSet::new();
        for g in groups {
            let st = st.clone();
            set.spawn(async move { deliver_group(&st, g).await });
        }
        while let Some(joined) = set.join_next().await {
            if joined.is_err() {
                note_panic();
            }
        }
        for tenant in &tenants {
            super::coalesce::settle_bookings(st, tenant).await;
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        drop(done);
        use futures_util::StreamExt;
        futures_util::stream::iter(groups)
            .for_each_concurrent(*DELIVERY_WIDTH, |g| async move {
                deliver(st, &g.tenant, &g.sub, g.data, &g.ctx).await;
            })
            .await;
    }
}

/// How many changes of one drain are matched at once (T-6371): the cores the
/// process may use, at most 8. Matching one change against 1 000 candidate
/// Subscriptions takes milliseconds, and one serial matcher capped the whole
/// process's notification rate at one core while the delivery lanes waited.
#[cfg(not(target_arch = "wasm32"))]
pub(super) static MATCH_WIDTH: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(8)));

/// The matches of each change, in the order the changes came. Natively up to
/// [`MATCH_WIDTH`] changes are matched at once, each on its own task and
/// bounded by [`STEP_DEADLINE`]; their results still leave in change order,
/// so a subscription's notifications keep the order of its changes. A match
/// that panics or stalls is counted and its change matches nothing; the rest
/// of the drain goes on.
#[cfg(not(target_arch = "wasm32"))]
fn matched_in_order(
    st: &AppState,
    changes: Vec<(u64, Change)>,
) -> impl futures_util::Stream<Item = Vec<Matched>> + Unpin {
    use futures_util::StreamExt as _;
    let st = st.clone();
    futures_util::stream::iter(changes)
        .map(move |(queued, (tenant_str, before, after))| {
            let st = st.clone();
            tokio::spawn(async move {
                let mut ctx_memo = CtxMemo::new();
                let matched = matches_for(&st, &tenant_str, before, after, queued, &mut ctx_memo);
                match tokio::time::timeout(STEP_DEADLINE, matched).await {
                    Ok(m) => m,
                    Err(_) => {
                        note_stall("match", &tenant_str, "");
                        Vec::new()
                    }
                }
            })
        })
        .buffered(*MATCH_WIDTH)
        .map(|joined| {
            joined.unwrap_or_else(|_| {
                note_panic();
                Vec::new()
            })
        })
}

/// Wasm32 has one thread: the changes are matched one after the other.
#[cfg(target_arch = "wasm32")]
fn matched_in_order<'a>(
    st: &'a AppState,
    changes: Vec<(u64, Change)>,
) -> impl futures_util::Stream<Item = Vec<Matched>> + Unpin + 'a {
    use futures_util::StreamExt as _;
    Box::pin(futures_util::stream::iter(changes).then(
        move |(queued, (tenant_str, before, after))| async move {
            let mut ctx_memo = CtxMemo::new();
            matches_for(st, &tenant_str, before, after, queued, &mut ctx_memo).await
        },
    ))
}

/// One group out: the delivery permits, then the delivery, each bounded by
/// [`STEP_DEADLINE`].
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn deliver_group(st: &AppState, g: Matched) {
    let id = g.sub.get("id").and_then(Value::as_str).unwrap_or_default();
    // The slots bound how many deliveries can wait on a peer. An in-process
    // sink has no peer to wait on, and its store calls join its tenant's
    // shared statements (T-5159), so a fan-out to 1 000 sockets no longer
    // queues behind 8 slots for work that never leaves the process.
    let _permits = if in_process_sink(st, &g.sub) {
        None
    } else {
        match tokio::time::timeout(STEP_DEADLINE, delivery_permits(&g.tenant)).await {
            Ok(permits) => Some(permits),
            Err(_) => {
                note_stall("permits", g.tenant.as_str(), id);
                tell_lost(st, &g);
                return;
            }
        }
    };
    let delivered = deliver(st, &g.tenant, &g.sub, g.data, &g.ctx);
    if tokio::time::timeout(STEP_DEADLINE, delivered)
        .await
        .is_err()
    {
        note_stall("deliver", g.tenant.as_str(), id);
    }
}

/// A drained batch still owed to its subscribers: while any of its groups
/// waits on a lane, the batch counts as pending, so a shutdown drain that
/// waits for zero closes the pool only after the last delivery.
pub(crate) struct BatchDone {
    pub(super) pending: Arc<std::sync::atomic::AtomicUsize>,
    pub(super) taken: usize,
}

impl Drop for BatchDone {
    fn drop(&mut self) {
        self.pending
            .fetch_sub(self.taken, std::sync::atomic::Ordering::SeqCst);
    }
}

/// One group waiting on its subscription's lane, holding its batch pending
/// and its bytes on the lanes' ledger.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct LaneJob {
    pub(super) st: AppState,
    pub(super) group: Matched,
    pub(super) _batch: Arc<BatchDone>,
    pub(super) _held: Held,
}

/// The estimated heap bytes every delivery lane holds, against
/// [`crate::bounds::DELIVERY_LANE_BYTES`] (T-4549).
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Ledger(pub(super) std::sync::atomic::AtomicUsize);

#[cfg(not(target_arch = "wasm32"))]
pub(super) static LANE_LEDGER: Ledger = Ledger(std::sync::atomic::AtomicUsize::new(0));

#[cfg(not(target_arch = "wasm32"))]
impl Ledger {
    /// `bytes` more on the ledger, or `None` when they would take it past
    /// `budget`. A group larger than the whole budget is still taken on an
    /// empty ledger, so one big change is delivered rather than never.
    pub(super) fn take(&'static self, bytes: usize, budget: usize) -> Option<Held> {
        use std::sync::atomic::Ordering;
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                let after = used.saturating_add(bytes);
                (used == 0 || after <= budget).then_some(after)
            })
            .ok()?;
        metrics::gauge!("antares_notification_lane_bytes").increment(bytes as f64);
        Some(Held {
            ledger: self,
            bytes,
        })
    }

    #[cfg(test)]
    pub(super) fn used(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// Bytes a waiting group holds on the [`Ledger`], given back when the group
/// is delivered or dropped.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Held {
    pub(super) ledger: &'static Ledger,
    pub(super) bytes: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Held {
    fn drop(&mut self) {
        self.ledger
            .0
            .fetch_sub(self.bytes, std::sync::atomic::Ordering::AcqRel);
        metrics::gauge!("antares_notification_lane_bytes").decrement(self.bytes as f64);
    }
}

/// What `v` costs on the heap, estimated from how serde_json lays it out:
/// an object is a `BTreeMap`, whose every leaf node is about 640 bytes
/// however few of its eleven slots it fills, so the two-member `{"type",
/// "value"}` of an attribute instance costs a whole node (T-4562: counting
/// a slot per member read 268 MB of lanes that held 620 MB); an array is its
/// capacity of slots; a string its bytes, rounded to the allocator's
/// 16-byte classes. Cheap next to the serialization a delivery does.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn heap_estimate(v: &Value) -> usize {
    std::mem::size_of::<Value>().saturating_add(owned_heap(v))
}

/// The heap `v` owns beyond its own slot.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn owned_heap(v: &Value) -> usize {
    const LEAF: usize = 640;
    const SLOT: usize = std::mem::size_of::<Value>();
    let text = |len: usize| len.next_multiple_of(16);
    match v {
        Value::String(s) => text(s.len()),
        Value::Array(items) => items
            .iter()
            .fold(items.capacity().saturating_mul(SLOT), |n, item| {
                n.saturating_add(owned_heap(item))
            }),
        // nodes fill to about two thirds once a map has more than one
        Value::Object(map) => map.iter().fold(
            LEAF.saturating_mul(map.len().div_ceil(8).max(1)),
            |n, (k, item)| {
                n.saturating_add(text(k.len()))
                    .saturating_add(owned_heap(item))
            },
        ),
        _ => 0,
    }
}

/// (broker, tenant, subscription id) → the lane its groups wait on. An
/// entry lives while its lane has work: the lane's task removes it when it
/// finds the lane empty, under this lock, so a group is never sent to a lane
/// nobody serves. The broker part keeps two brokers of one process (tests
/// run one each, with the same tenants and subscription ids) on lanes of
/// their own, each served on its own runtime (T-5245).
#[cfg(not(target_arch = "wasm32"))]
pub(super) type Lanes = std::sync::Mutex<HashMap<LaneKey, tokio::sync::mpsc::Sender<LaneJob>>>;

/// A lane's key: [`AppState::broker`], tenant, subscription id.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type LaneKey = (usize, String, String);
#[cfg(not(target_arch = "wasm32"))]
pub(super) static LANES: std::sync::LazyLock<Lanes> = std::sync::LazyLock::new(Lanes::default);

/// Hand one group to its subscription's lane, opening the lane when there is
/// none. A full lane drops the group for that subscription alone, counted
/// and named (T-4086): the drain never waits for a subscriber.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn to_lane(st: &AppState, group: Matched, batch: &Arc<BatchDone>) {
    use tokio::sync::mpsc::error::TrySendError;
    let key = (
        st.broker(),
        group.tenant.as_str().to_owned(),
        group
            .sub
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    let bytes = group
        .data
        .iter()
        .fold(0usize, |n, entity| n.saturating_add(heap_estimate(entity)));
    let Some(held) = LANE_LEDGER.take(bytes, *crate::bounds::DELIVERY_LANE_BYTES) else {
        // every lane together holds its budget: this subscription loses the
        // group, counted and named like a full lane's, and the process keeps
        // its memory (T-4549)
        note_lane_drop(&key, group.data.len());
        tell_lost(st, &group);
        return;
    };
    let job = LaneJob {
        st: st.clone(),
        group,
        _batch: batch.clone(),
        _held: held,
    };
    // A poisoned map is a panic in some past hand-off; what it holds is
    // senders, rebuilt on the next group either way.
    let mut lanes = LANES.lock().unwrap_or_else(|p| p.into_inner());
    let job = match lanes.get(&key) {
        Some(lane) => match lane.try_send(job) {
            Ok(()) => return,
            Err(TrySendError::Full(job)) => {
                note_lane_drop(&key, job.group.data.len());
                tell_lost(st, &job.group);
                return;
            }
            // its task is gone (a panic past the guard): open a new lane
            Err(TrySendError::Closed(job)) => job,
        },
        None => job,
    };
    let (lane, rx) = tokio::sync::mpsc::channel(DELIVERY_LANE);
    if lane.try_send(job).is_err() {
        // a fresh channel always has room; nothing to serve otherwise
        return;
    }
    lanes.insert(key.clone(), lane);
    drop(lanes);
    tokio::spawn(run_lane(key, rx));
}

/// Serve one subscription's lane in order until it is empty, then retire it.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn run_lane(key: LaneKey, mut rx: tokio::sync::mpsc::Receiver<LaneJob>) {
    // a group the last fold had no room for leads the next notification
    let mut carry = None;
    loop {
        let job = match carry.take().map_or_else(|| rx.try_recv().ok(), Some) {
            Some(job) => job,
            None => {
                // Looked at again under the map's lock, which every sender
                // holds while it sends: a group cannot arrive between this
                // look and the removal.
                let mut lanes = LANES.lock().unwrap_or_else(|p| p.into_inner());
                match rx.try_recv() {
                    Ok(job) => job,
                    Err(_) => {
                        lanes.remove(&key);
                        return;
                    }
                }
            }
        };
        let (st, group, batches, rest) = fold(job, &mut rx);
        carry = rest;
        guarded(async move { deliver_group(&st, group).await }).await;
        drop(batches);
    }
}

/// A fold: the state, the merged group, the batches and ledger bytes it
/// holds until delivered, and the job it had no room for.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type Folded = (
    AppState,
    Matched,
    Vec<(Arc<BatchDone>, Held)>,
    Option<LaneJob>,
);

/// What waits behind `job` on its lane leaves in the same notification, as
/// a drain folds one subscription's changes into one group: a lane keeps up
/// by sending fewer, larger notifications, not by queueing one per change.
/// The fold stops before a group that would take the notification past
/// [`CHANGE_BATCH`] entities and hands that group back, so a backed-up lane
/// sends several bounded POSTs, never its whole depth in one. The newest
/// group carries the subscription as it reads now; the batches stay pending
/// until the delivery ends. A group of another state (a second broker in
/// the process, with the same tenant and subscription id) leads the next
/// notification: each leaves through its own state's sinks.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn fold(job: LaneJob, rx: &mut tokio::sync::mpsc::Receiver<LaneJob>) -> Folded {
    let LaneJob {
        st,
        mut group,
        _batch,
        _held,
    } = job;
    let mut batches = vec![(_batch, _held)];
    while let Ok(next) = rx.try_recv() {
        if group.data.len().saturating_add(next.group.data.len()) > CHANGE_BATCH
            || !next.st.same(&st)
        {
            return (st, group, batches, Some(next));
        }
        let mut data = std::mem::take(&mut group.data);
        data.extend(next.group.data);
        group = Matched { data, ..next.group };
        batches.push((next._batch, next._held));
    }
    (st, group, batches, None)
}

/// A group this process dropped, told to the subscription's binding, which
/// says so in band where it can (a WebSocket's `x-gap`, T-4921).
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn tell_lost(st: &AppState, g: &Matched) {
    if let (Some(uri), Some(id)) = (
        g.sub.as_object().and_then(super::endpoint_uri),
        g.sub.get("id").and_then(Value::as_str),
    ) {
        if let Some(sink) = st.sinks.sink_for_uri(uri) {
            sink.lost(uri, g.tenant.as_str(), id);
        }
    }
}

/// Groups a full lane refused: counted by entity, and named by tenant and
/// subscription in the log, so an operator can tell which subscriber fell
/// behind and lost changes (T-4086).
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn note_lane_drop(key: &LaneKey, entities: usize) {
    let n = LANE_DROPS
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .saturating_add(1);
    metrics::counter!("antares_notification_lane_dropped_total").increment(entities as u64);
    if n == 1 || n.is_multiple_of(100) {
        tracing::warn!(
            "subscription {} of tenant {} is slower than its changes: {entities} entities dropped from its delivery lane ({n} groups so far, all subscriptions)",
            key.2,
            key.1
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) static LANE_DROPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long one step of a drain may take: matching one change, waiting for
/// one group's delivery permits, or that delivery. A drain waits for all of
/// its groups and the next drain for it, so a step that never ends stops
/// notifications for every Subscription of the process (T-4102). A send is
/// at most 30 s (`endpoint_timeout_ms`), so a live step stays well inside.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const STEP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// A step abandoned at [`STEP_DEADLINE`]: counted and named, so the wait
/// that never ended is in the log.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn note_stall(step: &'static str, tenant: &str, sub: &str) {
    metrics::counter!("antares_notification_stalls_total", "step" => step).increment(1);
    tracing::warn!(
        "notification step {step} abandoned after {}s (tenant {tenant}, subscription {sub}); the drain goes on",
        STEP_DEADLINE.as_secs()
    );
}

/// A change the full queue refused: counted, and said once per thousand so
/// a 40 % delivery gap is visible in the log and not only on the counter.
pub(super) fn note_drop() {
    let n = CHANGES_DROPPED
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .saturating_add(1);
    metrics::counter!("antares_notification_changes_dropped_total").increment(1);
    if n == 1 || n.is_multiple_of(1000) {
        tracing::warn!("notification change queue full: {n} changes dropped so far (delivery slower than the write rate)");
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) static DELIVERY_SLOTS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(*DELIVERY_WIDTH));

#[cfg(not(target_arch = "wasm32"))]
pub(super) type TenantSlots =
    std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Semaphore>>>;
#[cfg(not(target_arch = "wasm32"))]
pub(super) static TENANT_SLOTS: std::sync::LazyLock<TenantSlots> =
    std::sync::LazyLock::new(TenantSlots::default);

/// The tenant's share, created on first use. An entry lives only while a
/// delivery is holding it: a lookup that has to create one first drops every
/// entry nothing else still references, so this map is bounded by the
/// delivery width and not by how many of the broker's 10 000 tenants have
/// ever had a subscription fire.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn tenant_slots(tenant: &TenantId) -> Arc<tokio::sync::Semaphore> {
    // A poisoned map is a panic in some past delivery, not a reason to stop
    // delivering: what it holds is rebuildable and bounded either way.
    let mut map = TENANT_SLOTS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(s) = map.get(tenant.as_str()) {
        return Arc::clone(s);
    }
    map.retain(|_, s| Arc::strong_count(s) > 1);
    let slots = Arc::new(tokio::sync::Semaphore::new(*DELIVERY_WIDTH_PER_TENANT));
    map.insert(tenant.as_str().to_owned(), Arc::clone(&slots));
    slots
}

/// Both permits one delivery holds until it settles. The tenant's share is
/// taken first, so a tenant already at its share waits there instead of
/// waiting inside one of the broker's slots.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn delivery_permits(tenant: &TenantId) -> impl Sized {
    let share = tenant_slots(tenant).acquire_owned().await;
    (share, DELIVERY_SLOTS.acquire().await)
}

/// Whether the Subscription's notifications stay in this process: its
/// binding opens no socket (a WebSocket) or the forward transport takes them
/// (ADR-0025 rule 7). The same test `deliver_as` polices egress by.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn in_process_sink(st: &AppState, sub: &Value) -> bool {
    sub.as_object()
        .and_then(super::endpoint_uri)
        .is_some_and(|uri| {
            in_process(st, uri).is_some()
                || st.sinks.sink_for_uri(uri).is_some_and(|s| !s.network())
        })
}
