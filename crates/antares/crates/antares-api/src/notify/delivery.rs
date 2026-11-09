// SPDX-License-Identifier: EUPL-1.2
//! One notification on the wire: cooldown, endpoint options, receiverInfo, in-process transports and the outbound send.

use super::*;

/// POST the Notification (5.3.1) and write the 5.2.14.2 bookkeeping back.
pub(crate) async fn deliver(
    st: &AppState,
    tenant: &TenantId,
    sub: &Value,
    data: Vec<Value>,
    ctx: &Context,
) {
    // a notification body is bounded the way an inbound body is (6.3.4 wall,
    // MAX_BODY_BYTES): a grouped delivery over the cap leaves as several
    // notifications, each whole entities, never one unbounded POST
    for chunk in chunk_by_bytes(data, *crate::bounds::MAX_BODY_BYTES) {
        deliver_as(
            st,
            tenant,
            Kind::Subscription,
            sub,
            "Notification",
            chunk,
            ctx,
            None,
        )
        .await;
    }
}

/// Split `data` into runs whose serialized sizes stay under `cap`, cutting
/// only at whole items; one item alone over the cap still travels alone.
pub(super) fn chunk_by_bytes(data: Vec<Value>, cap: usize) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = Vec::new();
    let mut size = 0usize;
    for item in data {
        let n = serde_json::to_vec(&item).map(|b| b.len()).unwrap_or(0);
        match out.last_mut() {
            Some(run) if size.saturating_add(n) <= cap => {
                run.push(item);
                size = size.saturating_add(n);
            }
            _ => {
                out.push(vec![item]);
                size = n;
            }
        }
    }
    out
}

/// Table 5.2.15-1 `timeout`: per-endpoint delivery deadline in milliseconds.
/// "The NGSI-LD system can override this value" — clamped to [100 ms, 30 s]
/// so one subscription cannot park a delivery task for minutes. Default 5 s
/// (the previous hard-coded deadline). HTTP only: the clause scopes it to
/// bindings that "always return a response".
pub(super) fn endpoint_timeout_ms(ep: &serde_json::Map<String, Value>) -> u32 {
    ep.get("timeout")
        .and_then(Value::as_f64)
        .filter(|t| *t > 0.0)
        .map(|t| (t as u32).clamp(100, 30_000))
        .unwrap_or(5_000)
}

/// Table 5.2.15-1 `cooldown`: "Once a failure has occurred, minimum period of
/// time in milliseconds which shall elapse before attempting to make a
/// subsequent notification to the same endpoint after failure. If requests
/// are received before the cooldown period has expired, no notification is
/// sent." — i.e. matches inside the window are DROPPED, not queued.
pub(super) fn in_cooldown(sub: &Value, now: chrono::DateTime<chrono::Utc>) -> bool {
    let n = sub.get("notification");
    let Some(cd) = n
        .and_then(|n| n.get("endpoint"))
        .and_then(|e| e.get("cooldown"))
        .and_then(Value::as_f64)
        .filter(|c| *c > 0.0)
    else {
        return false;
    };
    // the gate exists only "once a failure has occurred" and only until a
    // success clears it — notification.status tracks exactly that (5.2.14.2)
    if n.and_then(|n| n.get("status")).and_then(Value::as_str) != Some("failed") {
        return false;
    }
    let Some(lf) = n.and_then(|n| n.get("lastFailure")).and_then(Value::as_str) else {
        return false;
    };
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(lf) else {
        return false;
    };
    let elapsed = now
        .signed_duration_since(t.with_timezone(&chrono::Utc))
        .num_milliseconds();
    (elapsed as f64) < cd
}

#[allow(clippy::too_many_arguments)] // one param per notification dimension
pub(super) async fn deliver_as(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    sub: &Value,
    ntype: &str,
    data: Vec<Value>,
    ctx: &Context,
    trigger_reason: Option<&str>,
) {
    // 5.2.12: a paused subscription (isActive false) and an expired one send
    // nothing. Every caller that assembles data locally checks this first;
    // the ones that arrive with data already assembled — the 5.8.6 inbound
    // notification among them — did not, so the check belongs here too.
    if !is_active(sub) {
        return;
    }
    let sub_id = sub_str(sub, "id").unwrap_or_default().to_owned();
    let Some(ep) = sub
        .get("notification")
        .and_then(|n| n.get("endpoint"))
        .and_then(Value::as_object)
    else {
        return;
    };
    let Some(uri) = ep.get("uri").and_then(Value::as_str) else {
        return;
    };
    // 5.8.1.4 consumer half: the internal CSR subscription's notifications
    // are handled in-process (urn:antares:distsub:{tenant}\n{own sub id})
    if let Some(own) = uri.strip_prefix("urn:antares:distsub:") {
        if let (Some((_, own_id)), Some(handler)) =
            (own.split_once('\n'), st.csource_notification.as_ref())
        {
            handler(st, tenant, own_id, trigger_reason, &data).await;
        }
        return;
    }
    // 6.3.8: the binding comes from the registry and nowhere else. Creation
    // rejects an endpoint whose scheme no sink serves, so a stored row that
    // still names one was hand-edited — it is dropped, never delivered
    // through some other binding.
    if st.sinks.sink_for_uri(uri).is_none() {
        tracing::warn!(
            "subscription {sub_id} endpoint {} has no registered binding",
            redact_userinfo(uri)
        );
        return;
    }
    // endpoint.cooldown — drop (never queue) while the window is open.
    // Before any bookkeeping: a suppressed notification was never sent, so
    // timesSent/lastNotification must not move.
    if in_cooldown(sub, chrono::Utc::now()) {
        tracing::debug!("subscription {sub_id} in cooldown; notification suppressed (5.2.15)");
        return;
    }
    // An open circuit is the same class of self-inflicted suppression: no
    // request leaves the process, so Table 5.2.14.2-1 timesSent ("number of
    // times that the notification has been sent") and lastNotification ("the
    // instant when the last notification has been sent") must not move
    // either. `is_open` returning false IS the half-open probe, so the
    // check stays exactly once per attempt.
    // A binding that opens no socket has no destination for the policy or
    // the breaker to judge, nor has a delivery the forward transport makes
    // in process; every other network binding is policed below.
    let policed = network_policed(st, uri);
    if policed && st.egress.is_open(tenant.as_str(), uri) {
        tracing::debug!(
            "notification to {} short-circuited (breaker open)",
            redact_userinfo(uri)
        );
        return;
    }
    let accept = ep
        .get("accept")
        .and_then(Value::as_str)
        .unwrap_or("application/json");
    let now = now_iso();
    let mut body = json!({
        "id": format!("urn:ngsi-ld:{ntype}:{}", uuid::Uuid::new_v4()),
        "type": ntype,
        "subscriptionId": sub_id,
        "notifiedAt": now,
        "data": data,
    });
    if let (Some(r), Some(body)) = (trigger_reason, body.as_object_mut()) {
        body.insert("triggerReason".into(), Value::String(r.into()));
    }
    // ADR-0020: the engine sees the notification the broker means to send,
    // before it is encoded — the data entities are still the 5.2.14.1
    // documents the subscription asked for, not a FeatureCollection or a
    // set of @context-injected ones, so a projection lands on entities the
    // engine can name. Before the bookkeeping below, because 5.8.6 moves
    // timesSent for a notification that "shall be sent": one that is
    // dropped here never was, exactly like a cooldown or an open breaker.
    // The subject is the subscriber's, stored when the subscription was
    // created — delivery is broker-initiated and there is no request here
    // to read one off.
    // With no engine attached the answer is Deliver and the subject is
    // never read out of the subscription — this runs once per notification
    // per subscription.
    if let Some(engine) = &st.policy {
        match crate::policy::pre_notify(
            engine.as_ref(),
            &crate::policy::stored_subject(tenant, sub),
            sub,
            &mut body,
            ctx,
        ) {
            crate::policy::NotifyDecision::Deliver => {}
            crate::policy::NotifyDecision::Drop => {
                tracing::debug!("subscription {sub_id} notification dropped by the policy engine");
                return;
            }
            crate::policy::NotifyDecision::Filter(f) => {
                // a condition the notification path cannot re-run is not a
                // narrowing it can claim to have applied
                if f.q.is_some() || f.scope_q.is_some() || f.areas().next().is_some() {
                    tracing::error!(
                        "policy engine {} narrowed a notification with a query; dropping it",
                        engine.name()
                    );
                    return;
                }
                // a linked condition already read the target unprojected
                // (T-3474): the narrowing came too late to apply to it
                if (!f.omit.is_empty() || !f.pick.is_empty() || f.has_rules())
                    && linked_condition(sub)
                {
                    tracing::debug!(
                        "subscription {sub_id} has a linked q under a projecting narrowing; dropping"
                    );
                    return;
                }
                // ADR-N-066 2.4: each notified Attribute keeps the members
                // the subscriber's grants pass
                if let (Some(m), Some(arr)) = (
                    &f.metadata,
                    body.get_mut("data").and_then(Value::as_array_mut),
                ) {
                    for e in arr.iter_mut().filter_map(Value::as_object_mut) {
                        m.strip_entity(e, ctx);
                    }
                }
                let rules = crate::repr::type_rules(&f).filter(|r| !r.rules.is_empty());
                let f = crate::repr::compacted_filter(&f, ctx);
                let n = sub.get("notification");
                let key_values = matches!(
                    n.and_then(|n| n.get("format")).and_then(Value::as_str),
                    Some("keyValues" | "simplified")
                );
                if let Some(arr) = body.get_mut("data").and_then(Value::as_array_mut) {
                    for e in arr.iter_mut() {
                        project_joined(&f, e, key_values);
                    }
                }
                // ADR-0023 rule 11: the rules read each Entity's stored state
                // now, so one that left zone A carries no `speed`, nor a
                // `previousValue` of it, and one no rule holds for is not
                // notified at all
                if let Some(rules) = rules {
                    let shown = shown_now(
                        st,
                        tenant,
                        Some(&rules),
                        None,
                        body.get("data").unwrap_or(&Value::Null),
                        ctx,
                    )
                    .await;
                    if let Some(arr) = body.get_mut("data").and_then(Value::as_array_mut) {
                        arr.retain(|e| matches!(entity_shown(&shown, e), Some(Some(_))));
                        for e in arr.iter_mut() {
                            project_shown(&shown, e, key_values);
                        }
                        if arr.is_empty() {
                            tracing::debug!(
                                "subscription {sub_id}: no notified Entity is in the narrowing now; dropping"
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
    // ADR-0023 rule 9: a Subscription made through a view is delivered
    // under the view as the table serves it now, before the bookkeeping
    // below: a removed view sends nothing, and an Entity the view no longer
    // selects is not notified.
    #[cfg(feature = "views")]
    if let Some(view) = crate::views::of_subscription(st, tenant, sub) {
        let Some(view) = view else {
            tracing::debug!("subscription {sub_id}: its view is gone; dropping");
            return;
        };
        if !view_project(st, tenant, sub, &view, &mut body, ctx).await {
            tracing::debug!("subscription {sub_id}: no notified Entity is in its view; dropping");
            return;
        }
    }
    // 5.8.6: a subscription's ngsildConformance pins the notification format —
    // amend the data entities per the 4.3.6.8 fallbacks.
    if let Some(ver) = sub_str(sub, "ngsildConformance").and_then(crate::conformance::parse_version)
    {
        if let Some(d) = body.get_mut("data") {
            crate::conformance::amend_payload(d, ver);
        }
    }
    if accept == "application/ld+json" {
        // JSON-LD notifications carry the @context inside each data entity
        // (046_14: data[0] must contain @context; no Link header) — same rule
        // over MQTT: with ld+json the @context travels in the body (7.2).
        if let Some(arr) = body.get_mut("data").and_then(Value::as_array_mut) {
            for e in arr.iter_mut() {
                *e = inject_context(e.clone(), ctx);
            }
        }
    }
    if accept == "application/geo+json" {
        // Table 5.3.1-1: with endpoint.accept application/geo+json, data is
        // a FeatureCollection (5.2.30); if receiverInfo carries
        // Prefer=body=json the FeatureCollection carries no @context.
        let prefer_body_json = ep
            .get("receiverInfo")
            .and_then(Value::as_array)
            .is_some_and(|ri| {
                ri.iter().any(|kv| {
                    kv.get("key").and_then(Value::as_str) == Some("Prefer")
                        && kv.get("value").and_then(Value::as_str) == Some("body=json")
                })
            });
        let entities = body
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut fc = crate::repr::to_geojson_collection(entities, None);
        if let (false, Some(fc)) = (prefer_body_json, fc.as_object_mut()) {
            fc.insert("@context".into(), crate::negotiate::served_context(ctx));
        }
        if let Some(body) = body.as_object_mut() {
            body.insert("data".into(), fc);
        }
    }
    let receiver_info = kv_pairs(ep.get("receiverInfo"));

    // 6.3.22: a subscription living under a snapshot's synthetic tenant
    // notifies with the NGSILD-Snapshot header and the OWNER tenant — the
    // internal "snap-…" tenant never leaks.
    let (hdr_tenant, snapshot_id) =
        match crate::snapshots::snapshot_of_synth(st, tenant.as_str()).await {
            Some((owner, sid)) => (owner, Some(sid)),
            None => (tenant.clone(), None),
        };

    // Prepared BEFORE the bookkeeping writeback so the optimistic stamp
    // covers only the in-flight attempt (046_12_01 race). The parts are
    // transport-neutral: the sink registered for the endpoint's scheme turns
    // them into HTTP headers (6.3.8) or an MQTT metadata object (Table
    // 7.2-2).
    let mut info = receiver_info;
    strip_reserved_markers(&mut info);
    if hdr_tenant.as_str() != "default" {
        info.push(("NGSILD-Tenant".into(), hdr_tenant.as_str().to_owned()));
    }
    if let Some(sid) = &snapshot_id {
        info.push(("NGSILD-Snapshot".into(), sid.clone()));
    }
    let outbound = Outbound {
        body,
        accept: accept.to_owned(),
        link: link_header_value(ctx),
        receiver_info: info,
        notifier_info: kv_pairs(ep.get("notifierInfo")),
    };
    // Bookkeeping BEFORE the send (5.8.6/5.2.14.2: lastNotification is the
    // instant the notification is sent). The ETSI mock unblocks the test the
    // moment the request ARRIVES, so a post-response-only writeback races the
    // test's immediate Retrieve Subscription (CI flake on 046_12_01).
    // Optimistic ok; a failed attempt is corrected right below — the transient
    // window is the in-flight attempt itself, and the failure TPs wait for the
    // attempt to resolve before asserting.
    // One store call: the stamp is a fixed mutation, so a backend can write
    // it as a single statement instead of locking the row across a round
    // trip. At fan-out that lock is what serializes delivery.
    // An in-process delivery (a WebSocket, the forward transport) books with
    // every other one of its tenant in one statement (T-5159), after its
    // send where nothing depends on the stamp (T-6371); a network one keeps
    // its own stamp right before its own send (046_12_01).
    let in_process_booking = !policed && kind == Kind::Subscription;
    let deferred = in_process_booking && super::coalesce::may_defer(st, tenant, sub, &sub_id);
    let mut prev_success = None;
    if !deferred {
        let booked = if in_process_booking {
            super::coalesce::book_in_process(st, tenant, &sub_id, &now).await
        } else {
            st.store
                .record_delivery(tenant, kind, &sub_id, &now)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("bookkeeping writeback failed: {e}");
                    None
                })
        };
        // 5.8.6: notifications are sent for the subscriptions the broker
        // holds. No row to book against means the subscription was deleted
        // (or the store failed) between matching and delivery — nothing may
        // be sent.
        let Some(booked) = booked else {
            return;
        };
        prev_success = booked.prev_success;
        mirror_bookkeeping(st, tenant, kind, &sub_id, Some(booked.doc));
    }
    // The notification endpoint is an egress destination like any other
    // — policy check once, breaker consulted before the attempt.
    // A refusal is a delivery failure for bookkeeping (status "failed",
    // lastSuccess rolled back below) but never breaker state: the policy
    // verdict says nothing about the endpoint's health.
    let refused = policed
        && match st.egress.check_destination(uri).await {
            Ok(()) => false,
            Err(e) => {
                tracing::warn!(
                    "notification endpoint {} refused by egress policy: {e}",
                    redact_userinfo(uri)
                );
                true
            }
        };
    // (delivered, timed_out): only a TIMEOUT-class failure feeds the breaker
    // — that protects against peers that eat the deadline. An endpoint
    // that ANSWERS (any status) is alive, costs only its own response time,
    // and 6.3.8 says the notification shall be sent — suppressing sends to a
    // responding host:port starves unrelated subscriptions sharing it.
    let timeout_ms = endpoint_timeout_ms(ep);
    let first = if refused {
        Err((false, "refused by egress policy".to_owned()))
    } else {
        send_outbound(st, tenant, uri, timeout_ms, &outbound).await
    };
    let (ok, timed_out) = match &first {
        Ok(()) => (true, false),
        Err((t, _)) => (false, *t),
    };
    if policed && !refused {
        if ok {
            st.egress.record_success(tenant.as_str(), uri);
        } else if timed_out {
            st.egress.record_failure(tenant.as_str(), uri);
        } else {
            // the destination responded (or refused fast): alive — clear
            // any stale consecutive-timeout state
            st.egress.record_success(tenant.as_str(), uri);
        }
    }
    // Delivery counters by binding (facade — no-op without the broker's
    // recorder). The label is the sink's first scheme, so the two members of
    // a family share one series.
    let scheme = st
        .sinks
        .sink_for_uri(uri)
        .and_then(|s| s.schemes().first().copied())
        .unwrap_or("unknown");
    if ok {
        metrics::counter!("antares_notifications_sent_total", "scheme" => scheme).increment(1);
    } else {
        metrics::counter!("antares_notifications_failed_total", "scheme" => scheme).increment(1);
    }
    if ok && deferred {
        super::coalesce::defer_booking(st, tenant, &sub_id, &now);
    }
    if !ok {
        if deferred {
            prev_success = super::coalesce::book_failed_attempt(st, tenant, &sub_id, &now).await;
        }
        // 5.8.6 / 5.11.7: subscription status → "failed" on delivery failure;
        // roll back the optimistic lastSuccess stamp.
        let ts = now_iso();
        // An in-process failure (a WebSocket queue that overflowed) waits in
        // its tenant's queue like its booking did: a fan-out of overflowing
        // sockets holds one write-back at a time, not one per socket (T-5159).
        let failed_doc = if !policed && kind == Kind::Subscription {
            super::coalesce::fail_in_process(st, tenant, &sub_id, prev_success.take(), &ts).await
        } else {
            record_failure(st, tenant, kind, &sub_id, prev_success.take(), &ts).await
        };
        mirror_bookkeeping(st, tenant, kind, &sub_id, failed_doc);
        // Retries are transport, not new notifications: they run on their
        // own task (never on the request path, never delaying another
        // subscription's delivery) and book only the final outcome — a
        // success sets lastSuccess/status ok without touching timesSent;
        // an exhausted policy leaves a dead letter.
        #[cfg(not(target_arch = "wasm32"))]
        if !refused && st.delivery.attempts > 1 {
            let first_err = first.err().map(|(_, e)| e).unwrap_or_default();
            let (st, tenant, uri) = (st.clone(), tenant.clone(), uri.to_owned());
            crate::spawn(async move {
                retry_and_settle(
                    &st, &tenant, kind, &sub_id, &uri, timeout_ms, outbound, first_err,
                )
                .await;
            });
        }
    }
}

/// 5.8.6 / 5.11.7 after a failed attempt: status "failed", the optimistic
/// `lastSuccess` rolled back to `prev_success`, `lastFailure` and
/// `timesFailed` moved. The stored document as it now stands, `None` when the
/// row is gone or the write failed (logged).
pub(super) async fn record_failure(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    sub_id: &str,
    prev_success: Option<Value>,
    ts: &str,
) -> Option<Value> {
    st.store
        .record_failure(tenant, kind, sub_id, prev_success, ts)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("failure-status writeback failed: {e}");
            None
        })
}

/// A `KeyValuePair[]` member of `endpoint` (Table 5.2.15-1) as owned pairs.
/// A member that is not an array of well-formed pairs contributes nothing:
/// 5.2.12 validation at creation already refused a malformed one.
pub(crate) fn kv_pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|kv| {
                    Some((
                        kv.get("key")?.as_str()?.to_owned(),
                        kv.get("value")?.as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 6.3.22 / 6.3.8: `NGSILD-Tenant` and `NGSILD-Snapshot` on a notification
/// are the broker's own statement of where the data came from, appended to
/// the `receiverInfo` pairs. The HTTP binding appends every pair it is
/// handed, so a subscriber naming one of the two in `receiverInfo` would
/// put a second value of it on the wire beside the broker's, and a receiver
/// reading "the" tenant of a notification could not tell which one the
/// broker meant. Ordinary custom headers are untouched.
pub(super) fn strip_reserved_markers(info: &mut Vec<(String, String)>) {
    info.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("NGSILD-Tenant") && !k.eq_ignore_ascii_case("NGSILD-Snapshot")
    });
}

/// The forward transport, when it delivers the notifications sent to `uri`
/// in process (ADR-0025 rule 7).
pub(super) fn in_process<'a>(
    st: &'a AppState,
    uri: &str,
) -> Option<&'a Arc<dyn crate::ForwardTransport>> {
    st.forward_transport.get().filter(|t| t.notifies(uri))
}

/// One attempt on the wire, through the binding the registry holds for the
/// endpoint's scheme (6.3.8), or in process when the forward transport
/// takes the endpoint (ADR-0025 rule 7), on every attempt, retries and
/// replays included. `Err((timed_out, why))`: only a timeout-class failure
/// feeds the breaker — an endpoint that answers is alive.
/// Whether a delivery to `uri` opens a socket the egress policy and the
/// breaker judge: not one the forward transport makes in process, nor a
/// binding that opens none.
pub(super) fn network_policed(st: &AppState, uri: &str) -> bool {
    in_process(st, uri).is_none() && st.sinks.sink_for_uri(uri).is_some_and(|s| s.network())
}

pub(super) async fn send_outbound(
    st: &AppState,
    tenant: &TenantId,
    uri: &str,
    timeout_ms: u32,
    outbound: &Outbound,
) -> Result<(), (bool, String)> {
    if let Some(transport) = in_process(st, uri) {
        return transport
            .notify(tenant, uri, &outbound.body)
            .await
            .map_err(|e| (false, e));
    }
    let Some(sink) = st.sinks.sink_for_uri(uri) else {
        return Err((
            false,
            format!(
                "no notification binding registered for {}",
                redact_userinfo(uri)
            ),
        ));
    };
    sink.deliver(
        uri,
        outbound,
        std::time::Duration::from_millis(u64::from(timeout_ms)),
    )
    .await
    .map_err(|e| (e.timed_out, e.message))
}

/// The matcher reads subscriptions from the
/// SubMirror, so every notification bookkeeping writeback must be applied
/// there too — otherwise the mirror copy never gains
/// `notification.lastNotification` and 5.2.12 `throttling` suppresses
/// nothing. In-process apply only: a KV write per notification would not
/// scale to the 100k-sub target, so in bus=nats multi-pod deployments the
/// throttling window is per-pod approximate.
/// Known ceiling: exact distributed throttling = per-notification KV sync or a
/// store read in `throttled()`; add if a deployment needs the strict window.
/// The mirror learns the counters from the document the writeback just
/// committed under the row lock; `None` (no row) leaves it untouched.
pub(super) fn mirror_bookkeeping(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    sub_id: &str,
    doc: Option<Value>,
) {
    if kind != Kind::Subscription {
        return;
    }
    if let (Some(m), Some(doc)) = (&st.sub_mirror, doc) {
        m.apply(tenant.as_str(), sub_id, Some(doc));
    }
}
