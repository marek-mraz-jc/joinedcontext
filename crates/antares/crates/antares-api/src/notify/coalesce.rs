// SPDX-License-Identifier: EUPL-1.2
//! Group commit for the store calls of in-process deliveries (T-5159).
//!
//! A fan-out of one change to 1 000 WebSocket subscriptions on one view used
//! to cost 1 000 reads of the same Entities and 1 000 bookkeeping
//! transactions, each waiting for a pool connection. Here a delivery that
//! reaches the store joins the NEXT statement of its tenant: while one
//! statement runs, the deliveries arriving behind it queue, and the next
//! statement serves all of them. A delivery never joins a statement already
//! running, so the Entities it reads are read after it asked (ADR-0023
//! rule 9: the view as the table serves it now). A tenant of a store has at
//! most one booking and one read statement in flight, whatever its fan-out.

use super::*;
use antares_store::Delivery;

/// What waits for a tenant's next statement: each question with the channel
/// its answer goes back on.
#[cfg(not(target_arch = "wasm32"))]
type Waiting<Q, A> = Vec<(Q, tokio::sync::oneshot::Sender<A>)>;

/// A queue's key: the store the statement runs on and the tenant. The store
/// part keeps two brokers of one process (tests run several, with the same
/// tenants) apart: a statement runs on the first caller's store, so a
/// question of another store answered there found nothing and its
/// notification was dropped (T-5245). A runner holds its store, so no other
/// store reuses the address while the entry lives.
#[cfg(not(target_arch = "wasm32"))]
type Key = (usize, String);

#[cfg(not(target_arch = "wasm32"))]
fn key(st: &AppState, tenant: &TenantId) -> Key {
    (
        Arc::as_ptr(&st.store).cast::<()>().addr(),
        tenant.as_str().to_owned(),
    )
}

/// One queue per store and tenant. An entry exists exactly while a runner
/// serves it, so a caller that finds none starts one, and the map holds only
/// queues with a statement in flight.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Coalescer<Q, A> {
    queues: std::sync::Mutex<HashMap<Key, Waiting<Q, A>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<Q: Send + 'static, A: Send + 'static> Coalescer<Q, A> {
    pub(super) fn new() -> Self {
        Self {
            queues: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// `q`'s answer from the next statement `run` makes for `tenant`, one
    /// answer per question in order. `None` when the runner died before
    /// answering (a panic inside `run`).
    ///
    /// A caller that finds its tenant idle runs its own statement on its own
    /// task, so an uncontended delivery costs what it did before; only the
    /// questions that queued behind that statement get a runner task.
    pub(super) async fn call<F, Fut>(
        &'static self,
        st: &AppState,
        tenant: &TenantId,
        q: Q,
        run: F,
    ) -> Option<A>
    where
        F: Fn(AppState, TenantId, Vec<Q>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Vec<A>> + Send,
    {
        let key = key(st, tenant);
        let waiting = {
            // A poisoned map is a panic in some past runner; its guard
            // already removed that runner's entry.
            let mut queues = self.queues.lock().unwrap_or_else(|p| p.into_inner());
            match queues.get_mut(&key) {
                Some(waiting) => {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    waiting.push((q, tx));
                    Err(rx)
                }
                None => {
                    queues.insert(key.clone(), Vec::new());
                    Ok(q)
                }
            }
        };
        let q = match waiting {
            Ok(q) => q,
            Err(rx) => return rx.await.ok(),
        };
        // Dropped mid-statement (its delivery's deadline) or done, the
        // caller hands what queued behind it to a runner: a question that
        // waited is never lost because the caller ahead of it went away.
        let respawn = {
            let (st, tenant, run) = (st.clone(), tenant.clone(), run.clone());
            move || {
                tokio::spawn(async move { self.serve(&st, &tenant, run).await });
            }
        };
        let _handoff = Retire {
            queues: &self.queues,
            key: &key,
            armed: true,
            respawn: Some(Box::new(respawn)),
        };
        run(st.clone(), tenant.clone(), vec![q]).await.pop()
    }

    /// Serve `tenant`'s queue until it is empty, one statement per round.
    async fn serve<F, Fut>(&self, st: &AppState, tenant: &TenantId, run: F)
    where
        F: Fn(AppState, TenantId, Vec<Q>) -> Fut,
        Fut: std::future::Future<Output = Vec<A>>,
    {
        let key = key(st, tenant);
        let mut guard = Retire {
            queues: &self.queues,
            key: &key,
            armed: true,
            respawn: None,
        };
        loop {
            let batch = {
                let mut queues = self.queues.lock().unwrap_or_else(|p| p.into_inner());
                match queues.get_mut(&key) {
                    Some(waiting) if !waiting.is_empty() => std::mem::take(waiting),
                    _ => {
                        queues.remove(&key);
                        guard.armed = false;
                        return;
                    }
                }
            };
            let (qs, txs): (Vec<Q>, Vec<_>) = batch.into_iter().unzip();
            let answers = run(st.clone(), tenant.clone(), qs).await;
            for (tx, answer) in txs.into_iter().zip(answers) {
                // the caller gave up (its delivery deadline): nothing to tell
                let _gone = tx.send(answer);
            }
        }
    }
}

/// What a runner leaves behind. With `respawn` (the caller that ran its own
/// statement), questions still waiting go to a new runner, whether the caller
/// finished or was dropped. Without it, or when unwinding from a panic, the
/// entry goes, so the tenant's next caller starts afresh instead of queueing
/// behind a dead runner; the questions still queued then get `None`.
#[cfg(not(target_arch = "wasm32"))]
struct Retire<'a, Q, A> {
    queues: &'a std::sync::Mutex<HashMap<Key, Waiting<Q, A>>>,
    key: &'a Key,
    armed: bool,
    respawn: Option<Box<dyn FnOnce() + Send + 'a>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<Q, A> Drop for Retire<'_, Q, A> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut queues = self.queues.lock().unwrap_or_else(|p| p.into_inner());
        let waiting = queues.get(self.key).is_some_and(|w| !w.is_empty());
        match self.respawn.take() {
            Some(respawn) if waiting && !std::thread::panicking() => {
                drop(queues);
                respawn();
            }
            _ => {
                queues.remove(self.key);
            }
        }
    }
}

/// How long the stamps of in-process deliveries gather before one statement
/// books them (T-6371). A fan-out of 10 000 notifications a second over 1 000
/// sockets wrote 10 000 rows a second, and every socket waited for its row:
/// one statement busy all the time, the largest single share of write-to-socket
/// time. Over a second the same fan-out writes each Subscription once.
#[cfg(not(target_arch = "wasm32"))]
const BOOK_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

/// The notifications of one window per Subscription: how many left, and the
/// instant of the last. At most one entry per Subscription in the mirror.
#[cfg(not(target_arch = "wasm32"))]
type Window = HashMap<String, (i64, String)>;

#[cfg(not(target_arch = "wasm32"))]
static WINDOWS: std::sync::LazyLock<std::sync::Mutex<HashMap<Key, Window>>> =
    std::sync::LazyLock::new(Default::default);

/// Taken around a window's statement, so a reader waits for the stamps that
/// already left the window as well as for those still in it. ponytail: one
/// turn for the process; a turn per tenant when one tenant's window is seen
/// holding up another's reads.
#[cfg(not(target_arch = "wasm32"))]
static SETTLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether this delivery may leave before its stamp is written. The mirror
/// answers "does the Subscription still exist" (5.8.6: a deleted one is not
/// notified) without a statement. A Subscription that reads its own stamp
/// to decide stays booked first: `throttling` and `timeInterval` read
/// `lastNotification`.
pub(super) fn may_defer(st: &AppState, tenant: &TenantId, sub: &Value, id: &str) -> bool {
    cfg!(not(target_arch = "wasm32"))
        && sub.get("throttling").is_none()
        && sub.get("timeInterval").is_none()
        && st
            .sub_mirror
            .as_ref()
            .is_some_and(|m| m.holds(tenant.as_str(), id))
}

/// Book one sent notification with its window. The window's task counts as a
/// pending change, so a shutdown drain writes it before the pool closes; a
/// crash loses at most one window of `timesSent`, never a notification.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn defer_booking(st: &AppState, tenant: &TenantId, sub_id: &str, now: &str) {
    let key = key(st, tenant);
    let mut windows = WINDOWS.lock().unwrap_or_else(|p| p.into_inner());
    let opens = !windows.contains_key(&key);
    let (times, last) = windows
        .entry(key)
        .or_default()
        .entry(sub_id.to_owned())
        .or_default();
    *times = times.saturating_add(1);
    now.clone_into(last);
    drop(windows);
    if opens {
        st.pending_changes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let done = BatchDone {
            pending: st.pending_changes.clone(),
            taken: 1,
        };
        let (st, tenant) = (st.clone(), tenant.clone());
        tokio::spawn(async move {
            tokio::time::sleep(BOOK_WINDOW).await;
            settle_bookings(&st, &tenant).await;
            drop(done);
        });
    }
}

/// Wasm32 books every delivery before it leaves ([`may_defer`]).
#[cfg(target_arch = "wasm32")]
pub(super) fn defer_booking(_: &AppState, _: &TenantId, _: &str, _: &str) {}

/// Write what `tenant`'s window holds now and wait for it. A reader of
/// 5.2.14.2 members comes here first, so nobody who holds a notification
/// reads a `timesSent` that does not count it.
pub(crate) async fn settle_bookings(st: &AppState, tenant: &TenantId) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let key = key(st, tenant);
        let _turn = SETTLING.lock().await;
        let window = WINDOWS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        let attempts: Vec<(String, String, i64)> = window
            .into_iter()
            .flatten()
            .map(|(id, (times, last))| (id, last, times))
            .collect();
        for ((id, _, _), booked) in attempts.iter().zip(book(st, tenant, &attempts).await) {
            let doc = booked.map(|b| b.doc);
            super::delivery::mirror_bookkeeping(st, tenant, Kind::Subscription, id, doc);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (st, tenant);
}

/// Book a deferred delivery that failed: after its window, so the failure
/// lands on top of every success before it and rolls `lastSuccess` back to
/// the last of them. What the failure write-back needs: the `lastSuccess`
/// this attempt overwrote.
pub(super) async fn book_failed_attempt(
    st: &AppState,
    tenant: &TenantId,
    sub_id: &str,
    now: &str,
) -> Option<Value> {
    settle_bookings(st, tenant).await;
    let booked = book_in_process(st, tenant, sub_id, now).await?;
    super::delivery::mirror_bookkeeping(st, tenant, Kind::Subscription, sub_id, Some(booked.doc));
    booked.prev_success
}

/// The 5.2.14.2 stamp of one in-process delivery, written with every other
/// in-process delivery of its tenant waiting for the same statement. `None`:
/// the subscription is gone or the write failed, and nothing may be sent.
pub(super) async fn book_in_process(
    st: &AppState,
    tenant: &TenantId,
    sub_id: &str,
    now: &str,
) -> Option<Delivery> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static BOOKINGS: std::sync::LazyLock<Coalescer<(String, String, i64), Option<Delivery>>> =
            std::sync::LazyLock::new(Coalescer::new);
        BOOKINGS
            .call(
                st,
                tenant,
                (sub_id.to_owned(), now.to_owned(), 1),
                |st, tenant, attempts| async move { book(&st, &tenant, &attempts).await },
            )
            .await
            .flatten()
    }
    #[cfg(target_arch = "wasm32")]
    {
        book(st, tenant, &[(sub_id.to_owned(), now.to_owned(), 1)])
            .await
            .pop()
            .flatten()
    }
}

/// One `record_deliveries` per round of distinct subscriptions. Two waiting
/// attempts on one subscription (two notifications in flight for it) go in
/// consecutive rounds, so each books exactly one notification and gets back
/// the `lastSuccess` it overwrote.
pub(super) async fn book(
    st: &AppState,
    tenant: &TenantId,
    attempts: &[(String, String, i64)],
) -> Vec<Option<Delivery>> {
    let mut out: Vec<Option<Delivery>> = vec![None; attempts.len()];
    let mut left: Vec<usize> = (0..attempts.len()).collect();
    while !left.is_empty() {
        let mut seen = std::collections::HashSet::new();
        let (round, later): (Vec<usize>, Vec<usize>) = left.into_iter().partition(|i| {
            attempts
                .get(*i)
                .is_some_and(|(id, _, _)| seen.insert(id.as_str()))
        });
        let stamps: Vec<(&str, &str, i64)> = round
            .iter()
            .filter_map(|i| attempts.get(*i))
            .map(|(id, now, times)| (id.as_str(), now.as_str(), *times))
            .collect();
        match st
            .store
            .record_deliveries(tenant, Kind::Subscription, &stamps)
            .await
        {
            Ok(found) => {
                for (i, d) in round.into_iter().zip(found) {
                    if let Some(slot) = out.get_mut(i) {
                        *slot = d;
                    }
                }
            }
            Err(e) => tracing::warn!("bookkeeping writeback failed: {e}"),
        }
        left = later;
    }
    out
}

/// The failure write-back of one in-process delivery
/// ([`super::delivery::record_failure`]), queued per tenant behind the one in
/// flight, so overflowing sockets of one fan-out never hold more than one
/// pool connection for it. Rounds write in arrival order, so two failures of
/// one subscription roll `lastSuccess` back in the order they were booked.
pub(super) async fn fail_in_process(
    st: &AppState,
    tenant: &TenantId,
    sub_id: &str,
    prev_success: Option<Value>,
    ts: &str,
) -> Option<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        type Failure = (String, Option<Value>, String);
        static FAILURES: std::sync::LazyLock<Coalescer<Failure, Option<Value>>> =
            std::sync::LazyLock::new(Coalescer::new);
        FAILURES
            .call(
                st,
                tenant,
                (sub_id.to_owned(), prev_success, ts.to_owned()),
                |st, tenant, failures: Vec<Failure>| async move {
                    let mut docs = Vec::with_capacity(failures.len());
                    for (id, prev, ts) in failures {
                        docs.push(
                            super::delivery::record_failure(
                                &st,
                                &tenant,
                                Kind::Subscription,
                                &id,
                                prev,
                                &ts,
                            )
                            .await,
                        );
                    }
                    docs
                },
            )
            .await
            .flatten()
    }
    #[cfg(target_arch = "wasm32")]
    {
        super::delivery::record_failure(st, tenant, Kind::Subscription, sub_id, prev_success, ts)
            .await
    }
}

/// The stored Entities `ids` names, read in one statement with every other
/// delivery of the tenant waiting for a read. `None`: the read failed (logged
/// once for the whole statement).
pub(super) async fn read_entities(
    st: &AppState,
    tenant: &TenantId,
    ids: Vec<String>,
) -> Option<Arc<HashMap<String, Value>>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        type Read = Option<Arc<HashMap<String, Value>>>;
        static READS: std::sync::LazyLock<Coalescer<Vec<String>, Read>> =
            std::sync::LazyLock::new(Coalescer::new);
        READS
            .call(
                st,
                tenant,
                ids,
                |st, tenant, asks: Vec<Vec<String>>| async move {
                    let found = read(&st, &tenant, asks.iter().flatten()).await;
                    vec![found; asks.len()]
                },
            )
            .await
            .flatten()
    }
    #[cfg(target_arch = "wasm32")]
    {
        read(st, tenant, ids.iter()).await
    }
}

/// The union of `ids` in reads of at most `chunk` ids each (F-ANT3-021):
/// a large fan-out of folded deliveries never becomes one statement.
fn read_batches<'a>(ids: impl Iterator<Item = &'a String>, chunk: usize) -> Vec<Vec<&'a str>> {
    let union: std::collections::BTreeSet<&str> = ids.map(String::as_str).collect();
    let union: Vec<&str> = union.into_iter().collect();
    union.chunks(chunk.max(1)).map(<[&str]>::to_vec).collect()
}

/// `get_entities` over the union of `ids`, one batch at a time; a failed
/// batch fails the whole read, since a partial map would read as deletions.
async fn read<'a>(
    st: &AppState,
    tenant: &TenantId,
    ids: impl Iterator<Item = &'a String>,
) -> Option<Arc<HashMap<String, Value>>> {
    let mut found = HashMap::new();
    for batch in read_batches(ids, *crate::bounds::MAX_BATCH_ITEMS) {
        match st.store.get_entities(tenant, &batch).await {
            Ok(part) => found.extend(part),
            Err(e) => {
                tracing::warn!("notification path: reading the notified Entities failed: {e}");
                return None;
            }
        }
    }
    Some(Arc::new(found))
}

#[cfg(test)]
mod tests {
    use super::read_batches;

    /// 1000 asks of 256 ids each, overlapping, read as their union in
    /// batches no larger than the chunk.
    #[test]
    fn a_large_union_is_read_in_bounded_batches() {
        let asks: Vec<Vec<String>> = (0..1000)
            .map(|a| (0..256).map(|i| format!("urn:x:{}", a * 100 + i)).collect())
            .collect();
        let batches = read_batches(asks.iter().flatten(), 1000);
        assert!(batches.iter().all(|b| b.len() <= 1000));
        let total: usize = batches.iter().map(Vec::len).sum();
        assert_eq!(total, 999 * 100 + 256, "each id of the union once");
    }
}
