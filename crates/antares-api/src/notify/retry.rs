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
    while let Some(delay) = policy.next_delay(made, started.elapsed()) {
        tokio::time::sleep(delay).await;
        // the subscription may have gone, or its endpoint may have tripped
        // the breaker meanwhile — a retry is still one more attempt
        if st
            .store
            .get(tenant, kind, sub_id)
            .await
            .ok()
            .flatten()
            .is_none()
        {
            return;
        }
        made = made.saturating_add(1);
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

/// Replay one dead letter through the same binding, once. `Ok` = delivered
/// (the caller deletes the letter); `Err` carries the failure text.
pub(crate) async fn replay_dead_letter(
    st: &AppState,
    tenant: &TenantId,
    letter: &Value,
) -> Result<(), String> {
    let uri = letter["uri"].as_str().ok_or("dead letter without uri")?;
    let timeout_ms = letter_timeout_ms(letter);
    let outbound = Outbound::from_dead_letter(letter)?;
    // the egress policy of the moment applies, exactly as for a fresh send
    if in_process(st, uri).is_none() && st.sinks.sink_for_uri(uri).is_some_and(|s| s.network()) {
        st.egress
            .check_destination(uri)
            .await
            .map_err(|e| e.to_string())?;
    }
    send_outbound(st, tenant, uri, timeout_ms, &outbound)
        .await
        .map_err(|(_, e)| e)
}
