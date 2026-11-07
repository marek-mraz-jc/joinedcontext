// SPDX-License-Identifier: EUPL-1.2
//! Periodic (timeInterval) subscriptions: due times, the cross-replica claim and the sweep tick.

use super::*;

/// The pages of one sweep waiting to be delivered at once (F-R2ANT1-010).
#[cfg(not(target_arch = "wasm32"))]
const PAGES_IN_FLIGHT: usize = 4;

/// When an interval subscription is next due, in epoch millis: one
/// `timeInterval` after the last Notification it sent (Table 5.2.14.2-1
/// `lastNotification`), or after its creation while it has sent none. Without
/// either anchor it is due immediately.
pub(super) fn due_at_ms(sub: &Value, interval: f64) -> i64 {
    let anchor = sub
        .get("notification")
        .and_then(|n| n.get("lastNotification"))
        .and_then(Value::as_str)
        .or_else(|| sub.get("createdAt").and_then(Value::as_str));
    match anchor.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
        Some(last) => last
            .timestamp_millis()
            .saturating_add(interval_offset_ms(interval)),
        None => i64::MIN,
    }
}

/// One period of a periodic Subscription (5.2.12 `timeInterval`) in
/// milliseconds.
///
/// Table 5.2.12-1 bounds the member only as greater than 0, so the seconds a
/// client names can exceed what epoch milliseconds hold. The cast saturates,
/// and every caller adds it with `saturating_add`, which puts an interval the
/// broker cannot schedule at the end of representable time. Adding it plainly
/// wraps the sum negative, and a negative firing instant reads as permanently
/// due: the subscription then fires its whole query on every tick, and the
/// same value poisons the process-wide sweep clock those minima feed.
pub(super) fn interval_offset_ms(interval: f64) -> i64 {
    (interval * 1000.0) as i64
}

/// timeInterval subscriptions: fire when due, with all matching entities.
/// Multi-instance: claim one interval firing under the subscription row
/// lock — N matcher pods race, exactly one wins (single-winner by
/// lock, no leader election). The due-check reruns INSIDE the lock; the
/// winner stamps `lastNotification` as its claim, losers see not-due and
/// roll back. Only engaged in bus=nats mode — single-process behaviour (and
/// its 046_12 bookkeeping ordering) is untouched.
///
/// `None` = the firing is not this pod's. `Some(prev)` = claimed, carrying
/// the `lastNotification` the claim overwrote: a firing that turns out to
/// have nothing to send gives it back through [`release_interval`], because
/// Table 5.2.14.2-1 stamps the instant a notification was SENT and 5.8.6
/// sends none when nothing matches.
pub(super) async fn claim_interval(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    sub: &Value,
    interval: f64,
) -> Option<Option<Value>> {
    let id = sub.get("id").and_then(Value::as_str)?;
    let mut prev: Option<Value> = None;
    let res = st
        .store
        .mutate(tenant, kind, id, |doc| {
            if chrono::Utc::now().timestamp_millis() < due_at_ms(doc, interval) {
                return Err(());
            }
            let Some(sub_doc) = doc.as_object_mut() else {
                // a stored Subscription is an object; one that is not carries no
                // notification member to stamp, and Err(()) is the same "nothing
                // written" the not-due branch above returns
                return Err(());
            };
            if let Some(n) = sub_doc
                .entry("notification")
                .or_insert_with(|| json!({}))
                .as_object_mut()
            {
                prev = n.insert("lastNotification".into(), Value::String(now_iso()));
            }
            Ok(())
        })
        .await;
    matches!(res, Ok(Some(Ok(())))).then_some(prev)
}

/// Give a claimed firing back (5.8.6: nothing matched, so nothing was sent).
/// The stamp returns to what [`claim_interval`] found, which both keeps
/// `lastNotification` truthful and leaves the subscription due, exactly as
/// the single-process path does.
pub(super) async fn release_interval(
    st: &AppState,
    tenant: &TenantId,
    kind: Kind,
    id: &str,
    prev: Option<Value>,
) {
    let res = st
        .store
        .mutate::<(), ()>(tenant, kind, id, |doc| {
            if let Some(n) = doc
                .as_object_mut()
                .and_then(|o| o.get_mut("notification"))
                .and_then(Value::as_object_mut)
            {
                match &prev {
                    Some(v) => n.insert("lastNotification".into(), v.clone()),
                    None => n.remove("lastNotification"),
                };
            }
            Ok(())
        })
        .await;
    if let Err(e) = res {
        tracing::warn!("releasing the interval claim for {id} failed: {e}");
    }
}

/// One sweep of the interval subscriptions (5.8.6, 5.11.7): "If a
/// Subscription defines a timeInterval member, a Notification shall be sent
/// periodically, when the time interval (in seconds) specified in such value
/// field is reached, regardless of Attribute changes."
///
/// The sweep runs on a fixed tick, so its idle cost is what has to stay
/// small. Two things keep it off the store. A tick that cannot fire anything
/// returns before enumerating tenants: each sweep records the earliest instant
/// a subscription it saw can next be due, and a write zeroes that clock —
/// through the mirror for Subscriptions, through `csub_written` for the
/// Context Source Registration Subscription half, which is mirrored by clock
/// rather than by document. A lost signal is repaired within
/// `CSUB_SWEEP_BACKSTOP_MS`. And a due subscription reads only the Entities
/// its own selector can match instead of its tenant's entity set.
pub async fn interval_tick(st: &AppState) {
    use std::sync::atomic::Ordering::Relaxed;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let clocks = st.sub_mirror.as_ref().map(|m| {
        (
            m.next_sub_sweep_ms.load(Relaxed),
            m.next_csub_sweep_ms.load(Relaxed),
        )
    });
    let (sweep_subs, sweep_csubs) = match clocks {
        Some((sub_clock, csub_clock)) => (now_ms >= sub_clock, now_ms >= csub_clock),
        // Never-wired fallback: no clock to keep, so every tick sweeps.
        None => (true, true),
    };
    if !sweep_subs && !sweep_csubs {
        return;
    }
    // Earliest next-due instant seen by this sweep, per half.
    let mut next_sub = i64::MAX;
    let mut next_csub = i64::MAX;
    // One sweep visits every tenant, and a delivery costs up to the
    // endpoint's whole timeout (Table 5.2.15-1, 30 s at the ceiling). Awaited
    // in turn, one unresponsive endpoint becomes the deadline of every other
    // subscriber's periodic notification. The deliveries of a tick therefore
    // run together, under the same width the change path uses, and the tick
    // still does not return until they have settled: ticks never overlap, so
    // a subscription cannot be fired twice for one period.
    #[cfg(not(target_arch = "wasm32"))]
    let mut sending = tokio::task::JoinSet::new();
    for tenant_str in read_or_warn(
        st.store.subscription_tenants().await,
        "the tenants with subscriptions",
    ) {
        let Ok(tenant) = TenantId::new_internal(&tenant_str) else {
            continue;
        };
        // Same source the matcher reads: the indexed mirror, with the store
        // list only as the never-wired fallback.
        let subs = match (&st.sub_mirror, sweep_subs) {
            (_, false) => Vec::new(),
            (Some(m), _) => m.periodic_docs(tenant.as_str()),
            (None, _) => read_or_warn(
                st.store.list(&tenant, Kind::Subscription).await,
                "the periodic Subscriptions",
            )
            .into_iter()
            .map(Arc::new)
            .collect(),
        };
        for sub in subs {
            let Some(interval) = sub.get("timeInterval").and_then(Value::as_f64) else {
                continue;
            };
            if !is_active(&sub) {
                continue;
            }
            let due_at = due_at_ms(&sub, interval);
            if now_ms < due_at {
                next_sub = next_sub.min(due_at);
                continue;
            }
            // Due: the following firing is one interval away. Recorded before
            // the claim, so a pod that LOSES the race (another one is firing
            // this subscription right now) keeps sweeping on the interval
            // instead of parking on an anchor only the winner advanced.
            next_sub = next_sub.min(now_ms.saturating_add(interval_offset_ms(interval)));
            // a stored q that no longer parses (one percent-encoded before
            // 5.2.12's read-as-written) matches nothing, as conditions_match
            // decides: no Entity to read and no Notification to send, so
            // nothing to claim either (F-ANT3-009: the claim stamped
            // lastNotification for a firing that never ran)
            if sub_str(&sub, "q").is_some_and(|q| antares_ql::regex::q_node(q).is_none()) {
                continue;
            }
            let claim = if st.nats {
                match claim_interval(st, &tenant, Kind::Subscription, &sub, interval).await {
                    Some(prev) => Some(prev),
                    None => continue,
                }
            } else {
                None
            };
            let ctx = sub_context(st, &tenant, &sub).await;
            let now = now_iso();
            // 5.8.6: the periodic Notification "shall include all the
            // subscribed Entities that match the query, geoquery and Scope
            // query conditions" — so the read is exactly this subscription's
            // own selector (5.2.33) and conditions, never the tenant's entity
            // set. Only predicates a store reproduces without hiding a
            // candidate are offered (ids when every selector entry names one,
            // types when the index proves them plain, q/scopeQ/geoQ under the
            // store's own rule that SQL removes rows and never decides them);
            // the selector_match/conditions_match pair below stays the
            // arbiter, exactly as on the query path.
            let type_groups: Vec<Vec<String>> = match index_keys(&sub) {
                Keys::Types(ts) => ts.into_iter().map(|t| vec![t]).collect(),
                _ => Vec::new(),
            };
            let ids = selector_ids(&sub);
            // The filter borrows a term expander that is not Sync, so it lives
            // and dies inside this block: held across the delivery await it
            // would make the whole interval task non-Send.
            // F-ANT3-020: the match set is read a page at a time and each
            // page's matches leave as their own Notification, so a type-only
            // selector over a large tenant never holds its whole set; a store
            // that cannot decide the filter answers it whole, once.
            let page_size = st.max_limit.max(1);
            // keyset (F-R2ANT1-011, as T-4683): each page seeks past the last id read, so a
            // walk reads every row once instead of re-reading each skipped one
            let mut after: Option<String> = None;
            let mut sent = false;
            loop {
                let (rows, last) = {
                    let expand = |t: &str| ctx.expand_key(t);
                    let id_refs: Vec<&str> = ids.iter().flatten().map(String::as_str).collect();
                    // q is read as written (5.2.12); parses shared per distinct
                    // expression text, as in conditions_match — the sweep
                    // re-runs per due subscription
                    let q_ast = sub_str(&sub, "q").and_then(antares_ql::regex::q_node);
                    let geo = sub.get("geoQ").and_then(Value::as_object).and_then(|g| {
                        let key = serde_json::to_string(g).unwrap_or_default();
                        antares_ql::regex::geo_query(&key, || {
                            antares_ql::geo::GeoQuery::from_params(&geo_params(g))
                                .ok()
                                .flatten()
                        })
                    });
                    let geo_spec = geo.as_ref().and_then(|g| g.to_sql_spec(&ctx));
                    let filter = antares_store::filter::EntityFilter {
                        ids: ids.as_ref().map(|_| id_refs.as_slice()),
                        types: (!type_groups.is_empty()).then_some(type_groups.as_slice()),
                        q: q_ast.as_deref(),
                        scope_q: sub_str(&sub, "scopeQ"),
                        geo: geo_spec.as_slice(),
                        expand: &expand,
                        page: Some(antares_store::filter::Page {
                            offset: 0,
                            limit: i64::try_from(page_size).unwrap_or(i64::MAX),
                            count: false,
                        }),
                        after_id: after.as_deref(),
                        ..Default::default()
                    };
                    match st.store.query_entities(&tenant, &filter).await {
                        Ok(o) => {
                            let last = !o.paged || o.rows.len() < page_size;
                            (o.rows, last)
                        }
                        Err(e) => {
                            tracing::warn!("notification path: reading the Entities a periodic Subscription notifies about failed: {e}");
                            (Vec::new(), true)
                        }
                    }
                };
                after = rows
                    .last()
                    .and_then(|d| d.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let mut matching: Vec<Value> = Vec::new();
                for d in rows {
                    if !selector_match(&sub, &d, &ctx)
                        || !linked_eval(st, &tenant, None, |l| conditions_match(&sub, &d, &ctx, l))
                            .await
                    {
                        continue;
                    }
                    matching.extend(
                        build_data(st, &tenant, &sub, &ctx, None, Some(&d), &[], false, &now).await,
                    );
                }
                if !matching.is_empty() {
                    sent = true;
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        // F-R2ANT1-010: a page waits for an earlier one to leave before the
                        // next is read, so a slow receiver holds a few pages, not the set
                        while sending.len() >= PAGES_IN_FLIGHT {
                            if let Some(Err(_)) = sending.join_next().await {
                                note_panic();
                            }
                        }
                        let (st, tenant, ctx, sub) =
                            (st.clone(), tenant.clone(), Arc::clone(&ctx), sub.clone());
                        sending.spawn(async move {
                            let _permits = delivery_permits(&tenant).await;
                            deliver(&st, &tenant, &sub, matching, &ctx).await;
                        });
                    }
                    #[cfg(target_arch = "wasm32")]
                    deliver(st, &tenant, &sub, matching, &ctx).await;
                }
                if last {
                    break;
                }
            }
            if !sent {
                // 5.8.6: "If there are no matching Entities, no Notification
                // is sent" — lastNotification stays untouched, so this
                // subscription is still due and every following tick
                // re-checks it. A claim taken to win the firing is given back
                // here, or the multi-pod path would stamp an instant nothing
                // was sent at and park the subscription for a whole interval.
                if let (Some(prev), Some(id)) = (claim, sub.get("id").and_then(Value::as_str)) {
                    release_interval(st, &tenant, Kind::Subscription, id, prev).await;
                }
                next_sub = next_sub.min(due_at);
            }
        }
        if !sweep_csubs {
            continue;
        }
        // csource timeInterval subs: periodic CSourceNotification with all
        // matching registrations, independent of changes (5.11.7)
        for sub in read_or_warn(
            docs_where(st, &tenant, Kind::CSourceSubscription, |s| {
                s.get("timeInterval").is_some()
            })
            .await,
            "the periodic Context Source Registration Subscriptions",
        ) {
            let Some(interval) = sub.get("timeInterval").and_then(Value::as_f64) else {
                continue;
            };
            if !is_active(&sub) {
                continue;
            }
            let due_at = due_at_ms(&sub, interval);
            if now_ms < due_at {
                next_csub = next_csub.min(due_at);
                continue;
            }
            next_csub = next_csub.min(now_ms.saturating_add(interval_offset_ms(interval)));
            // 5.11.7 sends the periodic CSourceNotification whatever the
            // matching set is, so this claim is never given back.
            if st.nats
                && claim_interval(st, &tenant, Kind::CSourceSubscription, &sub, interval)
                    .await
                    .is_none()
            {
                continue;
            }
            let ctx = sub_context(st, &tenant, &sub).await;
            let spec = crate::registry::spec_for_subscription(&sub);
            let data: Vec<Value> = read_or_warn(
                docs_where(st, &tenant, Kind::Registration, |r| {
                    crate::registry::csr_matches_subscription(&sub, r, &ctx)
                })
                .await,
                "the registrations a periodic Context Source Notification carries",
            )
            .into_iter()
            .map(|r| {
                let mut p = crate::registry::present_registration(
                    &filter_csr(&spec, &r, &ctx),
                    &ctx,
                    false,
                );
                arrayify_entity_types(&mut p);
                p
            })
            .collect();
            #[cfg(not(target_arch = "wasm32"))]
            {
                let (st, tenant, ctx) = (st.clone(), tenant.clone(), Arc::clone(&ctx));
                sending.spawn(async move {
                    // the Tenant's share and a slot, as for Subscriptions
                    // (F-ANT3-007: a bare slot let one Tenant hold them all)
                    let _permits = delivery_permits(&tenant).await;
                    deliver_csource(&st, &tenant, &sub, data, &ctx, "newlyMatching").await;
                });
            }
            #[cfg(target_arch = "wasm32")]
            deliver_csource(st, &tenant, &sub, data, &ctx, "newlyMatching").await;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    while let Some(joined) = sending.join_next().await {
        if joined.is_err() {
            note_panic();
        }
    }
    if let (Some(m), Some((sub_clock, csub_clock))) = (&st.sub_mirror, clocks) {
        // A periodic subscription written DURING the sweep has already zeroed
        // the clock: the exchange then fails, the zero stands and the next
        // tick sweeps rather than waiting out an interval computed without it.
        if sweep_subs {
            let _ = m
                .next_sub_sweep_ms
                .compare_exchange(sub_clock, next_sub, Relaxed, Relaxed);
        }
        if sweep_csubs {
            let _ = m.next_csub_sweep_ms.compare_exchange(
                csub_clock,
                next_csub.min(now_ms.saturating_add(CSUB_SWEEP_BACKSTOP_MS)),
                Relaxed,
                Relaxed,
            );
        }
    }
}
