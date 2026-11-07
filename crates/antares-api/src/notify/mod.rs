// SPDX-License-Identifier: EUPL-1.2
//! Subscription matching + HTTP notification delivery (5.8.6, 5.3.1).
//!
//! Change detection: the store's change hook feeds every entity write here as
//! a (before, after) pair; attribute-level changes are derived by diffing —
//! one hook point instead of one call per write handler.
//!
//! Candidate lookup is index-shaped. `SubMirror` keeps inverted
//! (tenant, type) and (tenant, watched-attr) maps next to the docs, so one
//! change evaluates only the subscriptions that could possibly fire — never
//! a scan over all of a tenant's subscriptions. Subscriptions the index
//! cannot classify exactly (4.17 type-selection expressions) fall into a
//! `broad` bucket that is always evaluated: the index may over-select,
//! never under-select. Full evaluation (selector/q/geo/scope/triggers)
//! stays the truth for every candidate.

mod coalesce;
mod csource;
mod delivery;
pub(crate) use coalesce::settle_bookings;
pub(crate) use delivery::kv_pairs;
mod interval;
mod matching;
mod payload;
mod retry;
#[cfg(test)]
mod tests;
mod throttle;

use crate::mirror::{index_keys, Change, Keys, Mirror, SubMirror, CSUB_SWEEP_BACKSTOP_MS};
use crate::negotiate::{inject_context, link_header_value};
use crate::state::{now_iso, AppState};
use antares_jsonld::Context;
use antares_model::{dt_key, TenantId};
use antares_store::CurrentStateDriverExt;
use antares_store::Kind;
use antares_store::{TemporalEvent, TemporalOp};
pub use csource::*;
pub(crate) use delivery::*;
pub use interval::*;
pub(crate) use matching::*;
pub(crate) use payload::*;
pub(crate) use retry::*;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
pub use throttle::*;

/// `notification.endpoint.uri` of a Subscription.
pub(crate) fn endpoint_uri(sub: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    sub.get("notification")?
        .get("endpoint")?
        .get("uri")?
        .as_str()
}

const DEFAULT_TRIGGERS: &[&str] = &["attributeCreated", "attributeUpdated"];
/// Depth of the change→matcher queue, the same ring size the local bus uses.
/// Held with the other published ceilings so `/q/health` reports it.
use crate::bounds::{CHANGE_QUEUE, DELIVERY_WIDTH};
// the per-tenant slots of the native delivery pool (`tenant_slots`) and its lanes
#[cfg(not(target_arch = "wasm32"))]
use crate::bounds::{DELIVERY_LANE, DELIVERY_WIDTH_PER_TENANT};
static CHANGES_DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TASK_PANICS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Hand a batch to the matcher queue: counted as pending on acceptance,
/// counted as dropped when the queue is full. Not the durable outbox
/// (`antares_sql::store::pg::outbox::enqueue`), which is a row inside the
/// caller's transaction: this ring lives in the process and a full one
/// drops, which is why the drop is counted.
fn queue_for_matching(
    tx: &tokio::sync::mpsc::Sender<(u64, Vec<Change>)>,
    pending: &std::sync::atomic::AtomicUsize,
    changes: Vec<Change>,
) {
    // stamped now, after the write: a subscription made later never matches it
    if tx.try_send((crate::mirror::tick(), changes)).is_ok() {
        pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    } else {
        note_drop();
    }
}

/// Changes dropped because the matcher queue was full, since process start.
pub(crate) fn changes_dropped() -> u64 {
    CHANGES_DROPPED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Panics absorbed at the notification-task boundary, since process start.
pub(crate) fn task_panics() -> u64 {
    TASK_PANICS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Entity-level members that are NOT Attributes. Table 5.2.12-1 scopes
/// watchedAttributes to "Properties or Relationships", so the entity's own
/// system members (including 4.22 `expiresAt` and `deletedAt`) must never be
/// diffed as attribute-level changes.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ChangeClass {
    Created,
    Updated,
    Deleted,
}

/// Fill a mirror from the store, or say why it could not be filled.
///
/// Every document of every tenant has to be in the mirror before it is
/// installed. `CurrentStateDriver::subscription_tenants` states the rule for
/// the data path — "A SUBSET is a silent outage: a tenant missing here never
/// fires a periodic notification and never reaches the mirror" — and an
/// error absorbed into an empty list is the same subset by another route.
/// A connection failure at startup refuses it, so this is reachable rather
/// than theoretical.
///
/// Paged, not `list`: 5.5.6 licenses TooManyResults for "a query operation
/// … producing so many results that can potentially exhaust client or
/// server resources", and the seed is not one — it must see every
/// document of every tenant or it is the silent outage above. Reading it
/// through the ceiling `list` carries for client queries made one tenant's
/// stored volume decide whether OTHER tenants are matched at all.
///
/// One function for every mirror and both bus modes. `bus=local` seeds the
/// subscription mirror here; `bus=nats` seeds a subscription mirror and a
/// registration mirror, and re-seeds the registration one after a consumer
/// gap. Those were a second copy of this walk that kept the ceiling and
/// swallowed the error into an empty list, which is how a rule fixed in one
/// place stayed broken in the others.
///
/// The domain is `subscription_tenants`, whose contract covers every kind a
/// mirror is built from. A tenant holding nothing of this kind costs one
/// empty page.
pub async fn seed_mirror(
    store: &dyn antares_store::CurrentStateDriver,
    mirror: &dyn Mirror,
    kind: Kind,
) -> Result<(), antares_model::NgsiError> {
    for tenant_str in store.subscription_tenants().await? {
        // The store's own enumeration, so the grammar is all that is left to
        // check: 5.5.15 permits a Subscription inside a Snapshot, and its
        // tenant is the synthetic one no client may name.
        let tenant = TenantId::new_internal(&tenant_str)?;
        let mut after: Option<String> = None;
        loop {
            let page = store
                .list_page(&tenant, kind, after.as_deref(), SEED_PAGE)
                .await?;
            let short = page.len() < SEED_PAGE;
            let before = after.clone();
            for doc in page {
                if let Some(id) = doc.get("id").and_then(Value::as_str) {
                    let id = id.to_owned();
                    after = Some(id.clone());
                    mirror.apply(&tenant_str, &id, Some(doc));
                }
            }
            // A short page is the end. A cursor that did not move is also the
            // end, and it is the load-bearing half: only a document carrying
            // an `id` can advance it, so a full page without one would
            // otherwise re-read the same page forever. No write path stores
            // such a document — which is exactly why the loop may not depend
            // on that staying true.
            if short || after == before {
                break;
            }
        }
    }
    Ok(())
}

/// Documents per mirror-seed page: the peak transient allocation of the
/// walk, paid once per tenant at startup.
const SEED_PAGE: usize = 1_000;

/// Install the subscription mirror, the store change hook that feeds the
/// matcher, and the background tasks the pipeline needs. Called once at
/// startup, by `crate::wire`.
pub(crate) async fn wire_matcher(state: &mut AppState) {
    // bus=local: the same indexed mirror the nats wiring builds, fed
    // synchronously by the CUD hook — the matcher never rescans the store.
    let mirror = Arc::new(SubMirror::default());
    match seed_mirror(state.store.as_ref(), mirror.as_ref(), Kind::Subscription).await {
        Ok(()) => state.sub_mirror = Some(mirror.clone()),
        // Not installed, so `subs_for` takes the store scan it documents as
        // the missing-mirror fallback. Installing what the seed managed to
        // read would be the one outcome that is neither correct nor slow:
        // the matcher reads candidates from the mirror alone, so a
        // subscription absent from it never fires again in this process.
        Err(e) => tracing::error!(
            "subscription mirror seed failed ({e}); \
             matching falls back to a store scan per change"
        ),
    }
    let m = mirror.clone();
    state.sub_sync = Some(Arc::new(
        move |tenant: &TenantId, kind: Kind, id: &str, doc: Option<&Value>| match kind {
            Kind::CSourceSubscription => m.csub_written(),
            _ => m.apply(tenant.as_str(), id, doc.cloned()),
        },
    ));

    // bus=local: the registration mirror the nats wiring builds, fed in
    // process by the registration CUD hook, so a federated query no longer
    // reads `csource_registrations` per request. Installed only when the
    // hydrate is whole, by the rule the subscription mirror follows above:
    // `reg_docs` reads a present mirror as the truth.
    let regs = Arc::new(crate::mirror::DocMirror::default());
    match seed_mirror(state.store.as_ref(), regs.as_ref(), Kind::Registration).await {
        Ok(()) => {
            state.reg_mirror = Some(regs.clone());
            state.reg_sync = Some(Arc::new(move |tenant: &TenantId, id: &str, doc| {
                regs.apply(tenant.as_str(), id, doc.cloned());
            }));
        }
        Err(e) => tracing::error!(
            "registration mirror hydrate failed ({e}); \
             federation matching falls back to a store read per request"
        ),
    }

    // The queue carries whole before+after payloads and is drained one
    // inline delivery at a time, so behind one slow subscriber an unbounded
    // queue grows until the process dies. Bounded instead: a full queue drops
    // the change and counts it.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u64, Vec<Change>)>(CHANGE_QUEUE);
    let flush_tx = tx.clone();
    let flush_pending = state.pending_changes.clone();
    state.change_flush = Some(Arc::new(move |changes: Vec<Change>| {
        queue_for_matching(&flush_tx, &flush_pending, changes)
    }));
    // Temporal auto-recording runs SYNCHRONOUSLY on the hook (read-your-writes:
    // the ETSI suite queries history immediately after a write); the matcher
    // work is handed to the async task below. One choke point for every write.
    let st_rec = state.clone();
    let hook_pending = state.pending_changes.clone();
    state.store.set_change_hook(Arc::new(
        move |tenant: &TenantId,
              before: Option<Value>,
              after: Option<Value>|
              -> antares_store::HookFuture<'_> {
            let st_rec = st_rec.clone();
            let tx = tx.clone();
            let hook_pending = hook_pending.clone();
            Box::pin(async move {
                record_temporal_change(&st_rec, tenant, before.as_ref(), after.as_ref()).await;
                // inside a request the change rides the request's buffer and
                // reaches the matcher with the rest of that request's changes
                let Some(change) =
                    crate::history::buffer_change((tenant.as_str().to_owned(), before, after))
                else {
                    return;
                };
                queue_for_matching(&tx, &hook_pending, vec![change]);
            })
        },
    ));
    let st = state.clone();
    let pending = state.pending_changes.clone();
    crate::spawn_loop(async move {
        while let Some((queued, first)) = rx.recv().await {
            let mut batch: Vec<(u64, Change)> = first.into_iter().map(|c| (queued, c)).collect();
            let mut taken: usize = 1;
            // everything already queued behind it rides the same pass
            while batch.len() < CHANGE_BATCH {
                match rx.try_recv() {
                    Ok((queued, c)) => {
                        batch.extend(c.into_iter().map(|c| (queued, c)));
                        taken = taken.saturating_add(1);
                    }
                    Err(_) => break,
                }
            }
            let st = st.clone();
            // The batch stays pending until its last group is delivered: the
            // groups leave on their subscriptions' lanes, after this drain.
            let done = Arc::new(BatchDone {
                pending: pending.clone(),
                taken,
            });
            guarded(async move { process_queued(&st, batch, Some(done)).await }).await;
        }
    });
    let st = state.clone();
    crate::spawn_loop(async move {
        loop {
            // Tokio's timer natively; the browser's own timer on wasm32
            // (tokio time never fires without a reactor there).
            #[cfg(not(target_arch = "wasm32"))]
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            #[cfg(target_arch = "wasm32")]
            gloo_timers::future::TimeoutFuture::new(500).await;
            let st = st.clone();
            guarded(async move { interval_tick(&st).await }).await;
        }
    });
}

/// Run one pipeline step on its own task so a panic inside it cannot end
/// notification delivery for the whole process: the task boundary absorbs
/// the panic, it is counted and logged, and the caller keeps consuming — a
/// later matching change still notifies (5.8.6). The step is awaited, so
/// delivery stays as serial as it was.
#[cfg(not(target_arch = "wasm32"))]
async fn guarded<F>(fut: F)
where
    F: std::future::Future<Output = ()>,
{
    use futures_util::FutureExt as _;
    // Caught here rather than on a spawned task: a task boundary would demand
    // Send + 'static of the step, and the interval step holds state that is
    // not Sync. Unwinding in place absorbs the panic just as well and keeps
    // the step running on this task, so delivery stays exactly as serial.
    if std::panic::AssertUnwindSafe(fut)
        .catch_unwind()
        .await
        .is_err()
    {
        note_panic();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn note_panic() {
    TASK_PANICS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    metrics::counter!("antares_notification_task_panics_total").increment(1);
    tracing::error!("notification pipeline task panicked; this change is lost");
}

/// Wasm32 has no task boundary to catch with (single-threaded executor, and
/// the browser profile aborts on panic); the step runs inline.
#[cfg(target_arch = "wasm32")]
async fn guarded<F>(fut: F)
where
    F: std::future::Future<Output = ()>,
{
    fut.await;
}

/// Auto-record a current-state change into the temporal representation
/// (5.6.11). Driven by the store's SYNCHRONOUS change hook, which fires inside
/// every entity write (create, update, partial update, merge, replace, batch)
/// for both the memory and postgres stores — so a new write path records
/// without the handler having to remember. The per-handler `mirror_record`
/// this replaced was the forgettable trap that left Partial Attribute Update
/// (5.6.4) and Replace Attribute (5.6.19) silently unrecorded.
///
/// Append-only, instance-precise: only the instances that are new or changed
/// (by datasetId, ignoring volatile members) are appended, so a multi-instance
/// attribute does not re-record unchanged datasets. An attribute or instance
/// the write removed is recorded here through `mirror_delete_attr` (the typed
/// null shape is not a plain append); entity DELETION keeps
/// `mirror_delete_entity`, which the delete handlers call.
pub async fn record_temporal_change(
    st: &AppState,
    tenant: &TenantId,
    before: Option<&Value>,
    after: Option<&Value>,
) {
    if !st.record_locally() {
        return;
    }
    let Some(after) = after else {
        return; // entity deletion — handled by mirror_delete_entity
    };
    let Some(id) = after.get("id").and_then(Value::as_str) else {
        return;
    };
    if !crate::history::records_history(st, tenant, after) {
        return;
    }
    let mut shell = Map::new();
    for k in ["id", "type", "createdAt", "modifiedAt", "scope"] {
        if let Some(v) = after.get(k) {
            shell.insert(k.to_string(), v.clone());
        }
    }
    let shell = Value::Object(shell);
    let event = |op, attr: &str, instance| TemporalEvent {
        op,
        tenant: tenant.clone(),
        entity_id: id.to_owned(),
        shell: shell.clone(),
        attr: attr.to_owned(),
        instance,
    };
    // 4.5.6: the Scope of a Temporal Evolution is represented as a temporal
    // Property whose only sub-properties are the non-reified createdAt,
    // modifiedAt, deletedAt and observedAt; when it "is updated as the result
    // of a change from the Core API, the observedAt sub-Property should be
    // set as a copy of the modifiedAt sub-Property".
    let ts = || {
        after
            .get("modifiedAt")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(now_iso)
    };
    if before.is_some_and(|b| b.get("scope") != after.get("scope")) {
        // 4.18: a deleted scope reads as the NGSI-LD Null
        if after.get("scope").is_none() {
            crate::history::mirror_delete_attr(st, tenant, id, "scope", None, &ts()).await;
        }
        if let Some(scope) = after.get("scope") {
            let ts = ts();
            let inst = json!({
                "type": "Property",
                "value": scope.clone(),
                "instanceId": format!("urn:ngsi-ld:Instance:{}", uuid::Uuid::new_v4()),
                "createdAt": ts, "modifiedAt": ts, "observedAt": ts,
            });
            crate::history::push(st, event(TemporalOp::ScopeChanged, "scope", inst)).await;
        }
    }
    for (k, class) in diff(before, Some(after)) {
        let op = match class {
            ChangeClass::Created => TemporalOp::AttrCreated,
            ChangeClass::Updated => TemporalOp::AttrModified,
            ChangeClass::Deleted => {
                // 4.5.7/4.5.8: each instance the write removed is recorded
                // as the NGSI-LD Null with deletedAt, whichever route
                // removed it (Delete Attribute, a replace, a merge-patch Null)
                let ts = ts();
                for ds in removed_datasets(before.and_then(|b| b.get(&k)), after.get(&k)) {
                    crate::history::mirror_delete_attr(st, tenant, id, &k, ds.as_deref(), &ts)
                        .await;
                }
                continue;
            }
        };
        let Some(av) = after.get(&k) else { continue };
        // gate 1, value-change: an unchanged instance produces no event
        let types: Vec<&str> = match after.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let operation = if op == TemporalOp::AttrCreated {
            "create"
        } else {
            "update"
        };
        let was = before.and_then(|b| b.get(&k));
        for mut inst in changed_instances(was, av) {
            if !crate::history::admits_change(st, tenant, &types, id, &k, was, &inst) {
                continue;
            }
            if let Some(o) = inst.as_object_mut() {
                let iid = instance_id(id, &k, &*o);
                o.entry("instanceId".to_owned())
                    .or_insert_with(|| Value::String(iid));
                // Who wrote it, for the store's column (T-4665); a store
                // takes it out before it keeps the instance. The row's
                // provenance says who, so the current state's copy stays out.
                o.remove(antares_jsonld::compact::MODIFIED_BY_KEY);
                o.insert(
                    antares_store::PROVENANCE_KEY.to_owned(),
                    crate::history::provenance(operation),
                );
            }
            crate::history::keep_replaced(st, tenant, &shell, &k, was, &inst).await;
            crate::history::push(st, event(op, &k, inst)).await;
        }
    }
}

/// 4.5.7: an instance is the Attribute "at a particular point in time",
/// recorded as its observedAt. The id of an observed instance is therefore
/// derived from (entity, attribute, datasetId, observedAt), so a re-send for
/// the same instant lands on the same row — the temporal store's upsert key —
/// and corrects it instead of appending a duplicate. Without observedAt there
/// is no instant to key on: a fresh random id, append-only.
///
/// The instant is keyed through `dt_key`, not through the stamp as written.
/// 4.6.3 leaves the seconds fraction optional and accepts a comma separator
/// in requests, and the broker stores a DateTime exactly as the client wrote
/// it, so one instant arrives under several spellings; keying the raw text
/// gave each spelling its own instance and left the correction's target in
/// place beside it — the failure 4.5.7 calls severe "in the case of
/// modification or deletion requests for legal reasons".
pub(crate) fn instance_id(
    entity: &str,
    attr: &str,
    inst: &serde_json::Map<String, Value>,
) -> String {
    let u = match inst.get("observedAt").and_then(Value::as_str) {
        Some(at) => {
            let ds = inst
                .get("datasetId")
                .and_then(Value::as_str)
                .unwrap_or("@none");
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                format!("{entity}\n{attr}\n{ds}\n{}", dt_key(at)).as_bytes(),
            )
        }
        None => uuid::Uuid::new_v4(),
    };
    format!("urn:ngsi-ld:Instance:{u}")
}

/// The datasetIds (None: the default instance) of the instances in `before`
/// that `after` no longer holds; every one of them when the Attribute went.
fn removed_datasets(before: Option<&Value>, after: Option<&Value>) -> Vec<Option<String>> {
    let after_arr = after.and_then(Value::as_array);
    before
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|bi| {
            !after_arr.is_some_and(|a| a.iter().any(|ai| instance_ds(ai) == instance_ds(bi)))
        })
        .map(|bi| {
            bi.get("datasetId")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect()
}

/// The instances in `after` that are new or changed vs `before` — matched by
/// datasetId, compared via `stable_eq` (volatile members ignored). A newly
/// created attribute (`before` None) contributes every instance.
fn changed_instances(before: Option<&Value>, after: &Value) -> Vec<Value> {
    let before_arr: Vec<&Value> = before
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    after
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|ai| {
            match before_arr
                .iter()
                .find(|bi| instance_ds(bi) == instance_ds(ai))
            {
                None => true,
                Some(bi) => !stable_eq(bi, ai),
            }
        })
        .collect()
}

pub(crate) use antares_matcher::{
    conditions_match, geo_params, is_active, selector_match, throttled,
};

pub(crate) use antares_notifier::{redact_userinfo, Outbound};
