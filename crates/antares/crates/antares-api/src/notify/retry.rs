// SPDX-License-Identifier: EUPL-1.2
//! Retry policy and dead letters: settle a failed delivery, park it, replay it.

use super::*;

pub(super) static DEAD_LETTERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Dead letters written by this process since start (`/q/health`
/// deadLetters); the letters themselves live in the store.
pub(crate) fn dead_letters_written() -> u64 {
    DEAD_LETTERS.load(std::sync::atomic::Ordering::Relaxed)
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
            Err((timed_out, e)) => {
                if timed_out {
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
    let id = letter
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    match st.store.create(tenant, Kind::DeadLetter, &id, letter).await {
        Ok(_) => {
            DEAD_LETTERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                "notification for {sub_id} to {} dead-lettered after {made} attempts: {last_err}",
                redact_userinfo(uri)
            );
        }
        Err(e) => tracing::error!("dead letter for {sub_id} could not be stored: {e}"),
    }
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
        "id": format!("urn:ngsi-ld:DeadLetter:{}", uuid::Uuid::new_v4()),
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
