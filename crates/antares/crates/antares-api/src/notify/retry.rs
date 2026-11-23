// SPDX-License-Identifier: EUPL-1.2
//! Retry policy and dead letters: settle a failed delivery, park it, replay it.

use super::*;

pub(super) static DEAD_LETTERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Dead letters written by this process since start (`/q/health`
/// deadLetters); the letters themselves live in the store.
pub(crate) fn dead_letters_written() -> u64 {
    DEAD_LETTERS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Retry tasks one tenant may have sleeping at once (S4-32). Each holds its
/// notification body, so the count bounds what one tenant's failing
/// endpoints keep in memory; a failure past it is dead-lettered at once.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const RETRIES_PER_TENANT: usize = 64;

/// The estimated body bytes one tenant's sleeping retries may hold: an
/// eighth of [`RETRY_LEDGER`]'s budget, so one tenant's large failing
/// bodies leave room for the others' retries.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) static RETRY_BYTES_PER_TENANT: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| *crate::bounds::DELIVERY_LANE_BYTES / 8);

/// What every sleeping retry holds, against
/// [`crate::bounds::DELIVERY_LANE_BYTES`]: a ledger of its own, so retries
/// never take the room fresh notifications need on the delivery lanes.
#[cfg(not(target_arch = "wasm32"))]
pub(super) static RETRY_LEDGER: super::throttle::Ledger = super::throttle::Ledger(
    std::sync::atomic::AtomicUsize::new(0),
    "antares_notification_retry_bytes",
);

/// One tenant's retries in flight on one broker.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct TenantRetries {
    subs: std::collections::HashSet<String>,
    bytes: usize,
}

/// Retries in flight by (broker, tenant).
#[cfg(not(target_arch = "wasm32"))]
type Retrying = std::collections::HashMap<(usize, String), TenantRetries>;

/// The subscriptions with a retry task in flight.
#[cfg(not(target_arch = "wasm32"))]
static RETRYING: std::sync::LazyLock<std::sync::Mutex<Retrying>> =
    std::sync::LazyLock::new(Default::default);

/// One subscription's place in [`RETRYING`], its bytes on the tenant's
/// share and its body's charge on [`RETRY_LEDGER`], all given back when the
/// retry task ends however it ends.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct RetrySlot {
    key: (usize, String),
    sub_id: String,
    bytes: usize,
    _held: super::throttle::Held,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for RetrySlot {
    fn drop(&mut self) {
        let mut map = RETRYING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(t) = map.get_mut(&self.key) {
            t.subs.remove(&self.sub_id);
            t.bytes = t.bytes.saturating_sub(self.bytes);
            if t.subs.is_empty() {
                map.remove(&self.key);
            }
        }
    }
}

/// A retry slot for `sub_id`, or why there is none: one retry in flight per
/// subscription, [`RETRIES_PER_TENANT`] and [`RETRY_BYTES_PER_TENANT`] per
/// tenant (a body larger than the share still gets a retry when the tenant
/// has none sleeping), and the body must fit [`RETRY_LEDGER`].
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn take_retry_slot(
    st: &AppState,
    tenant: &TenantId,
    sub_id: &str,
    outbound: &Outbound,
) -> Result<RetrySlot, &'static str> {
    let key = (st.broker(), tenant.as_str().to_owned());
    let mut map = RETRYING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let held = map.entry(key.clone()).or_default();
    if held.subs.contains(sub_id) {
        return Err("a retry for this subscription is already in flight");
    }
    if held.subs.len() >= RETRIES_PER_TENANT {
        return Err("the tenant has its most retries in flight");
    }
    let bytes = super::throttle::heap_estimate(&outbound.body);
    if held.bytes > 0 && held.bytes.saturating_add(bytes) > *RETRY_BYTES_PER_TENANT {
        return Err("the tenant's retry memory share is spent");
    }
    let Some(charge) = RETRY_LEDGER.take(bytes, *crate::bounds::DELIVERY_LANE_BYTES) else {
        return Err("the retry memory budget is spent");
    };
    held.subs.insert(sub_id.to_owned());
    held.bytes = held.bytes.saturating_add(bytes);
    Ok(RetrySlot {
        key,
        sub_id: sub_id.to_owned(),
        bytes,
        _held: charge,
    })
}

/// Retry tasks in flight for `tenant` on this broker, and the body bytes
/// they hold.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) fn retries_held(st: &AppState, tenant: &TenantId) -> (usize, usize) {
    RETRYING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(st.broker(), tenant.as_str().to_owned()))
        .map_or((0, 0), |t| (t.subs.len(), t.bytes))
}

/// Retry tasks in flight for `tenant` on this broker.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) fn retries_in_flight(st: &AppState, tenant: &TenantId) -> usize {
    retries_held(st, tenant).0
}

/// A failed first attempt that got no retry slot, as its `letter`: parked
/// at once, so it stays replayable; `why` is its last error.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn park_unretried(
    st: &AppState,
    tenant: &TenantId,
    sub_id: &str,
    uri: &str,
    letter: Value,
    why: &str,
) {
    metrics::counter!("antares_notifications_retried_total", "outcome" => "no_slot").increment(1);
    if park_dead_letter(st, tenant, letter).await {
        tracing::warn!(
            "notification for {sub_id} to {} dead-lettered without a retry: {why}",
            redact_userinfo(uri)
        );
    }
}

/// The retries the delivery policy grants after a failed first attempt,
/// then the settlement: lastSuccess/status ok on success, a dead letter
/// when the policy is exhausted.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)] // one param per piece of the attempt's state
pub(super) async fn retry_and_settle(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    sub_id: &str,
    uri: &str,
    timeout_ms: u32,
    outbound: Outbound,
    first_err: String,
) {
    let policy = st.delivery;
    let started = std::time::Instant::now();
    let first_at = now_iso();
    let mut made = 1u32;
    let mut last_err = first_err.clone();
    // the decision the first attempt went out under, read on the very body
    // every retry re-sends, so each retry compares like with like
    let Some(sub) = st.store.get(tenant, kind, sub_id).await.ok().flatten() else {
        return;
    };
    let expected = decision_now(st, tenant, &sub, &outbound.body).await;
    if expected == Some(crate::policy::NotifyDecision::Drop) {
        return;
    }
    while let Some(delay) = policy.next_delay(made, started.elapsed()) {
        tokio::time::sleep(delay).await;
        // the subscription may have gone, paused or narrowed meanwhile
        if let Err(why) = still_deliverable(
            st,
            tenant,
            kind,
            sub_id,
            uri,
            &outbound.body,
            expected.as_ref(),
        )
        .await
        {
            tracing::debug!("retry for {sub_id} stopped: {why}");
            return;
        }
        made = made.saturating_add(1);
        // a destination that tripped the breaker meanwhile is not dialled
        // (F-18): the attempt counts as failed, and an exhausted policy
        // parks the letter for a replay as before
        if network_policed(st, uri) && st.egress.is_open(tenant.as_str(), uri) {
            "the destination's circuit breaker is open".clone_into(&mut last_err);
            continue;
        }
        match send_outbound(st, tenant, uri, timeout_ms, &outbound).await {
            Ok(()) => {
                st.egress.record_success(tenant.as_str(), uri);
                metrics::counter!("antares_notifications_retried_total", "outcome" => "ok")
                    .increment(1);
                let ts = now_iso();
                let mut retried_doc: Option<Value> = None;
                st.store
                    .mutate(tenant, kind, sub_id, |doc| {
                        if let Some(o) = doc.as_object_mut() {
                            o.remove("status");
                        }
                        if let Some(n) = doc
                            .as_object_mut()
                            .and_then(|o| o.get_mut("notification"))
                            .and_then(Value::as_object_mut)
                        {
                            n.insert("lastSuccess".into(), Value::String(ts.clone()));
                            n.insert("status".into(), Value::String("ok".into()));
                        }
                        retried_doc = Some(doc.clone());
                        Ok::<(), antares_model::NgsiError>(())
                    })
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("retry bookkeeping writeback failed: {e}");
                        None
                    });
                mirror_bookkeeping(st, tenant, kind, sub_id, retried_doc);
                return;
            }
            Err((unreachable, e)) => {
                if unreachable {
                    st.egress.record_failure(tenant.as_str(), uri);
                } else {
                    st.egress.record_success(tenant.as_str(), uri);
                }
                last_err = e;
            }
        }
    }
    metrics::counter!("antares_notifications_retried_total", "outcome" => "dead").increment(1);
    let letter = dead_letter(
        sub_id, uri, timeout_ms, &outbound, made, &first_err, &last_err, &first_at,
    );
    if park_dead_letter(st, tenant, letter).await {
        tracing::warn!(
            "notification for {sub_id} to {} dead-lettered after {made} attempts: {last_err}",
            redact_userinfo(uri)
        );
    }
}

/// Dead letters one tenant keeps (S4-31): writing one past it deletes the
/// oldest, so a dead endpoint costs the store a bounded set per tenant and
/// one tenant's endpoints never evict another tenant's letters.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const MAX_DEAD_LETTERS: usize = 1_000;

/// Stores `letter` and trims the tenant back to [`MAX_DEAD_LETTERS`],
/// oldest first: letter ids sort by the time they were written
/// ([`dead_letter_id`]), so the oldest are the first of the id order.
/// `false` when the letter could not be stored (logged).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn park_dead_letter(st: &AppState, tenant: &TenantId, letter: Value) -> bool {
    let id = letter
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if let Err(e) = st.store.create(tenant, Kind::DeadLetter, &id, letter).await {
        tracing::error!("dead letter {id} could not be stored: {e}");
        return false;
    }
    DEAD_LETTERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let held = match st.store.list_slice(tenant, Kind::DeadLetter, 0, 0).await {
        Ok((_, held)) => held,
        Err(e) => {
            tracing::warn!("dead letters of {tenant} not counted for the cap: {e}");
            return true;
        }
    };
    let over = held.saturating_sub(MAX_DEAD_LETTERS);
    if over == 0 {
        return true;
    }
    let oldest = match st.store.list_slice(tenant, Kind::DeadLetter, 0, over).await {
        Ok((oldest, _)) => oldest,
        Err(e) => {
            tracing::warn!("dead letters of {tenant} past the cap not read: {e}");
            return true;
        }
    };
    for old in oldest {
        let Some(old_id) = old.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Ok(true) = st.store.delete(tenant, Kind::DeadLetter, old_id).await {
            metrics::counter!("antares_dead_letters_evicted_total").increment(1);
        }
    }
    true
}

/// A dead letter's id: the write time in milliseconds, zero-padded so the
/// id order is the time order, then a random part so two letters of one
/// millisecond stay apart. Letters written before this shape carry a bare
/// UUID and sort among these by their hex; the cap still counts them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dead_letter_id() -> String {
    format!(
        "urn:ngsi-ld:DeadLetter:{:013}-{}",
        jiff::Timestamp::now().as_millisecond().max(0),
        uuid::Uuid::new_v4()
    )
}

/// The dead-letter document: everything a replay needs to send the very
/// same request again, plus the attempt history. Written by the native
/// retry loop (`retry_and_settle`) alone.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)] // one param per stored letter member
pub(super) fn dead_letter(
    sub_id: &str,
    uri: &str,
    timeout_ms: u32,
    outbound: &Outbound,
    attempts: u32,
    first_err: &str,
    last_err: &str,
    first_at: &str,
) -> Value {
    json!({
        "id": dead_letter_id(),
        "type": "DeadLetter",
        "subscriptionId": sub_id,
        "uri": uri,
        "timeoutMs": timeout_ms,
        "attempts": attempts,
        "firstError": first_err,
        "lastError": last_err,
        "firstAt": first_at,
        "lastAt": now_iso(),
        "binding": antares_notifier::SinkRegistry::scheme_of(uri),
        "payload": outbound.body.clone(),
        "accept": outbound.accept,
        "link": outbound.link,
        "receiverInfo": outbound.receiver_info,
        "notifierInfo": outbound.notifier_info,
    })
}

/// The deadline a dead letter was written with. It is read back from
/// storage, so a value past u32 caps at the 30 s ceiling of
/// [`endpoint_timeout_ms`] instead of wrapping to a short deadline.
pub(super) fn letter_timeout_ms(letter: &Value) -> u32 {
    letter["timeoutMs"]
        .as_u64()
        .map_or(5_000, |t| u32::try_from(t.min(30_000)).unwrap_or(30_000))
}

/// The policy engine's decision for `body` as `sub`'s subscriber now; `None`
/// with no engine attached.
async fn decision_now(
    st: &AppState,
    tenant: &TenantId,
    sub: &Value,
    body: &Value,
) -> Option<crate::policy::NotifyDecision> {
    let engine = st.policy.as_ref()?;
    let ctx = sub_context(st, tenant, sub).await;
    Some(crate::policy::pre_notify(
        engine.as_ref(),
        &crate::policy::stored_subject(tenant, sub),
        sub,
        &mut body.clone(),
        &ctx,
    ))
}

/// Whether a notification built earlier may still go to `uri` now, as a
/// retry or a dead-letter replay: its subscription is still there and
/// active (5.2.12), still names `uri`, the view it was made through still
/// exists (ADR-0023 rule 9), and the policy engine still decides `expected`
/// for the stored body (`None`: an unnarrowed Deliver, all a replay can
/// vouch for, since a body already sent cannot be narrowed again). `Err`
/// says why not, in words for the operator.
pub(crate) async fn still_deliverable(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    sub_id: &str,
    uri: &str,
    body: &Value,
    expected: Option<&crate::policy::NotifyDecision>,
) -> Result<(), String> {
    let sub = st
        .store
        .get(tenant, kind, sub_id)
        .await
        .ok()
        .flatten()
        .ok_or("its subscription is gone")?;
    if !is_active(&sub) {
        return Err("its subscription is paused or expired".into());
    }
    let named = sub
        .get("notification")
        .and_then(|n| n.get("endpoint"))
        .and_then(|e| e.get("uri"))
        .and_then(Value::as_str);
    if named != Some(uri) {
        return Err("its subscription names another endpoint now".into());
    }
    #[cfg(feature = "views")]
    if let Some(None) = crate::views::of_subscription(st, tenant, &sub) {
        return Err("the view its subscription was made through is gone".into());
    }
    if let Some(now) = decision_now(st, tenant, &sub, body).await {
        let wanted = expected.unwrap_or(&crate::policy::NotifyDecision::Deliver);
        if &now != wanted {
            return Err("the policy no longer allows this notification as it was built".into());
        }
    }
    Ok(())
}

/// Why a dead letter was not delivered by [`replay_dead_letter`].
#[derive(Debug)]
pub(crate) enum Replay {
    /// Not sent: it may not go now ([`still_deliverable`]).
    Refused(String),
    /// Sent or tried, and it failed; the text says how.
    Failed(String),
}

/// Replay one dead letter through the same binding, once, if it may still
/// go ([`still_deliverable`] with no decision to compare against). A letter
/// keeps no kind: its subscription is looked up as a Subscription, then as
/// the context source kind its id names. `Ok` = delivered (the caller
/// deletes the letter).
pub(crate) async fn replay_dead_letter(
    st: &AppState,
    tenant: &TenantId,
    letter: &Value,
) -> Result<(), Replay> {
    let uri = letter["uri"]
        .as_str()
        .ok_or_else(|| Replay::Failed("dead letter without uri".into()))?;
    let timeout_ms = letter_timeout_ms(letter);
    let outbound = Outbound::from_dead_letter(letter).map_err(Replay::Failed)?;
    let sub_id = letter["subscriptionId"]
        .as_str()
        .ok_or_else(|| Replay::Refused("dead letter without subscriptionId".into()))?;
    let mut kind = Kind::Subscription;
    for k in [Kind::Subscription, crate::registry::csr_kind(sub_id)] {
        if st
            .store
            .get(tenant, k, sub_id)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            kind = k;
            break;
        }
    }
    still_deliverable(st, tenant, kind, sub_id, uri, &outbound.body, None)
        .await
        .map_err(Replay::Refused)?;
    // the egress policy of the moment applies, exactly as for a fresh send
    if in_process(st, uri).is_none() && st.sinks.sink_for_uri(uri).is_some_and(|s| s.network()) {
        st.egress
            .check_destination(uri)
            .await
            .map_err(|e| Replay::Failed(e.to_string()))?;
    }
    send_outbound(st, tenant, uri, timeout_ms, &outbound)
        .await
        .map_err(|(_, e)| Replay::Failed(e))
}
