// SPDX-License-Identifier: EUPL-1.2
//! Context source registration notifications (5.11, csourceNotification): triggers, the fan-out and the initial notification.

use super::*;

/// 5.11.7: which csource subs care about a registration change, and why.
pub(super) fn csource_trigger(
    sub: &Value,
    before: Option<&Value>,
    after: Option<&Value>,
    ctx: &Context,
) -> Option<&'static str> {
    let m = |d: Option<&Value>| {
        d.is_some_and(|d| crate::registry::csr_matches_subscription(sub, d, ctx))
    };
    match (m(before), m(after)) {
        (false, true) => Some("newlyMatching"),
        (true, true) => Some("updated"),
        (true, false) => Some("noLongerMatching"),
        (false, false) => None,
    }
}

/// The notification validator reads EntityInfo.type as an array
/// (`entities[0]["type"][0]`) — normalize to array form in notification data.
pub(super) fn arrayify_entity_types(reg: &mut Value) {
    let Some(infos) = reg.get_mut("information").and_then(Value::as_array_mut) else {
        return;
    };
    for info in infos {
        let Some(es) = info.get_mut("entities").and_then(Value::as_array_mut) else {
            continue;
        };
        for e in es {
            if let Some(t) = e.get("type").filter(|t| t.is_string()).cloned() {
                if let Some(o) = e.as_object_mut() {
                    o.insert("type".into(), Value::Array(vec![t]));
                }
            }
        }
    }
}

/// One prepared CSource notification, ready to send.
pub struct CsourceJob {
    sub: Value,
    presented: Value,
    ctx: std::sync::Arc<antares_jsonld::Context>,
    reason: &'static str,
}

/// Registration create/update/delete → CSourceNotification fan-out (5.11.7),
/// in two phases: `prepare_csource_jobs` runs IN the request path (store
/// reads + matching + payload build — so job order is the handlers' commit
/// order even on a slower store), and the caller spawns `send_csource_jobs`
/// (network only — the ack must not block on the receiver: the ETSI mock
/// replies only when the robot side wakes).
pub(crate) async fn prepare_csource_jobs(
    st: &AppState,
    tenant: &TenantId,
    before: Option<Value>,
    after: Option<Value>,
) -> Vec<CsourceJob> {
    let mut jobs = Vec::new();
    // 5.8.1.4: the Registration Subscriptions the distributed half owns are
    // not client resources, so they live under Kind::DistSub beside the
    // mapping documents — which carry no `type`. A document under the
    // client kind in the internal id namespace is a leftover of a release
    // that stored the internal ones there, and drives nothing.
    let client = read_or_warn(
        docs_where(st, tenant, Kind::CSourceSubscription, |d| {
            sub_str(d, "id")
                .is_some_and(|id| crate::registry::csr_kind(id) == Kind::CSourceSubscription)
        })
        .await,
        "the Context Source Registration Subscriptions of a changed registration",
    )
    .into_iter();
    let internal = read_or_warn(
        docs_where(st, tenant, Kind::DistSub, |d| {
            d.get("type").and_then(Value::as_str) == Some("Subscription")
        })
        .await,
        "the internal Registration Subscriptions of a changed registration",
    )
    .into_iter();
    for sub in client.chain(internal) {
        if !is_active(&sub) || sub.get("timeInterval").is_some() {
            continue;
        }
        let ctx = sub_context(st, tenant, &sub).await;
        let Some(reason) = csource_trigger(&sub, before.as_ref(), after.as_ref(), &ctx) else {
            continue;
        };
        let spec = crate::registry::spec_for_subscription(&sub);
        let source = if reason == "noLongerMatching" {
            &before
        } else {
            &after
        };
        let Some(reg) = source.as_ref().or(before.as_ref()) else {
            continue;
        };
        let filtered = filter_csr(&spec, reg, &ctx);
        let mut presented = crate::registry::present_registration(&filtered, &ctx, false);
        arrayify_entity_types(&mut presented);
        jobs.push(CsourceJob {
            sub,
            presented,
            ctx,
            reason,
        });
    }
    jobs
}

pub(crate) async fn send_csource_jobs(st: &AppState, tenant: &TenantId, jobs: Vec<CsourceJob>) {
    for job in jobs {
        // 5.11.7: re-check the subscription still exists right
        // before the send — a deleted subscription must never notify.
        let sub_id = job
            .sub
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = crate::registry::csr_kind(sub_id);
        if !matches!(st.store.get(tenant, kind, sub_id).await, Ok(Some(_))) {
            continue;
        }
        deliver_as(
            st,
            tenant,
            kind,
            &job.sub,
            "ContextSourceNotification",
            vec![job.presented],
            &job.ctx,
            Some(job.reason),
        )
        .await;
    }
}

/// Registration writes prepare one job per CSource subscription of the
/// tenant (5.11.7) and every subscription with localOnly != true owns one,
/// so a registration stream against many subscriptions queues
/// subscriptions × registrations jobs faster than the sources drain them —
/// at 10 000 × 100 the queued jobs were a 3 GB broker peak. The permit is
/// taken in the request path: a write waits for a fan-out slot instead of
/// stacking jobs, and the prepare order (the handlers' commit order) holds.
pub(super) const CSOURCE_FANOUT_WIDTH: usize = 64;
pub(super) static CSOURCE_FANOUT: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(CSOURCE_FANOUT_WIDTH);

/// The fan-out slots one tenant may hold at once (F-ANT3-006): a tenant
/// whose CSource subscription endpoints hang keeps at most this many of the
/// [`CSOURCE_FANOUT_WIDTH`] busy, so another tenant's registration writes
/// still find a slot.
pub(super) const CSOURCE_FANOUT_PER_TENANT: usize = CSOURCE_FANOUT_WIDTH / 8;

type FanoutShares =
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Semaphore>>>;
static FANOUT_SHARES: std::sync::LazyLock<FanoutShares> =
    std::sync::LazyLock::new(FanoutShares::default);

/// Both permits one fan-out holds until its sends settle: the tenant's
/// share first, as `delivery_permits` takes it, so a tenant at its share
/// waits there and never inside the broker's slots. A share lives only
/// while a fan-out holds it: creating one drops every entry nothing holds,
/// so the map is bounded by the width, not by the tenants ever seen.
pub(super) async fn fanout_permits(tenant: &TenantId) -> impl Sized {
    let shares = {
        // a poisoned map is a past panic; what it holds is rebuildable
        let mut map = FANOUT_SHARES.lock().unwrap_or_else(|p| p.into_inner());
        match map.get(tenant.as_str()) {
            Some(s) => std::sync::Arc::clone(s),
            None => {
                map.retain(|_, s| std::sync::Arc::strong_count(s) > 1);
                let s = std::sync::Arc::new(tokio::sync::Semaphore::new(CSOURCE_FANOUT_PER_TENANT));
                map.insert(tenant.as_str().to_owned(), std::sync::Arc::clone(&s));
                s
            }
        }
    };
    // never closed, so acquire only fails if it were — then send unbounded
    let share = shares.acquire_owned().await.ok();
    (share, CSOURCE_FANOUT.acquire().await.ok())
}

/// Registration create/update/delete → prepare in the request path, send
/// spawned (the ack must not block on the receiver), bounded as above.
pub(crate) async fn csource_fanout(
    st: &AppState,
    tenant: &TenantId,
    before: Option<Value>,
    after: Option<Value>,
) {
    let permit = fanout_permits(tenant).await;
    let jobs = prepare_csource_jobs(st, tenant, before, after).await;
    let (st2, t2) = (st.clone(), tenant.clone());
    crate::spawn(async move {
        send_csource_jobs(&st2, &t2, jobs).await;
        drop(permit);
    });
}

/// POST a CSourceNotification (5.3.2) under the same body bound as every
/// other one: 5.11.2.4 sends "all matching Context Source Registrations",
/// and a broker holding 100 000 of them must not turn that into one
/// unbounded request. Over the cap the set leaves as several notifications,
/// each carrying whole registrations — the trade [`deliver`] already makes
/// for entity Notifications.
pub(super) async fn deliver_csource(
    st: &AppState,
    tenant: &TenantId,
    sub: &Value,
    data: Vec<Value>,
    ctx: &Context,
    reason: &str,
) {
    let kind = crate::registry::csr_kind(sub_str(sub, "id").unwrap_or_default());
    for chunk in chunk_by_bytes(data, *crate::bounds::MAX_BODY_BYTES) {
        deliver_as(
            st,
            tenant,
            kind,
            sub,
            "ContextSourceNotification",
            chunk,
            ctx,
            Some(reason),
        )
        .await;
    }
}

/// Initial / post-update CSourceNotification with all currently matching
/// registrations (5.11.2.4 / 5.11.3.4).
pub(crate) async fn csource_initial(st: &AppState, tenant: &TenantId, sub_id: &str) {
    let Some(sub) = st
        .store
        .get(tenant, crate::registry::csr_kind(sub_id), sub_id)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    if !is_active(&sub) {
        return;
    }
    let ctx = sub_context(st, tenant, &sub).await;
    let spec = crate::registry::spec_for_subscription(&sub);
    let data: Vec<Value> = read_or_warn(
        docs_where(st, tenant, Kind::Registration, |r| {
            crate::registry::csr_matches_subscription(&sub, r, &ctx)
        })
        .await,
        "the registrations an initial Context Source Notification carries",
    )
    .into_iter()
    .map(|r| {
        let mut p =
            crate::registry::present_registration(&filter_csr(&spec, &r, &ctx), &ctx, false);
        arrayify_entity_types(&mut p);
        p
    })
    .collect();
    if data.is_empty() {
        return; // nothing currently matching ⇒ no initial notification
    }
    deliver_csource(st, tenant, &sub, data, &ctx, "newlyMatching").await;
}

/// Registration copy reduced to the matching RegistrationInfo elements
/// (5.10.2.5 / 5.11.7 "filtered Context Source Registrations").
pub(super) fn filter_csr(spec: &crate::registry::CsrSpec, reg: &Value, ctx: &Context) -> Value {
    let mut out = reg.clone();
    let matching: Vec<Value> = crate::registry::matching_infos(spec, reg, ctx)
        .into_iter()
        .cloned()
        .collect();
    if !matching.is_empty() {
        if let Some(o) = out.as_object_mut() {
            o.insert("information".into(), Value::Array(matching));
        }
    }
    out
}
