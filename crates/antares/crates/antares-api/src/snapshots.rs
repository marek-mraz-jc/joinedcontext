// SPDX-License-Identifier: EUPL-1.2
//! 5.16 Snapshots (optional API group; resources 6.36 /snapshots,
//! 6.37 /snapshots/{id}, 6.38 /snapshots/{id}/clone; scoping 6.3.22).
//!
//! A Snapshot freezes the results of a set of queries (5.2.41) into an
//! isolated copy on which Core + Temporal API operations run implicitly
//! local (5.5.15). Implementation shape: each snapshot owns a synthetic
//! internal tenant ("snap-…"); the 6.3.22 NGSILD-Snapshot header is resolved
//! by a middleware that rewrites the request's tenant, so every existing
//! Core/Temporal handler serves snapshot content unchanged — and, because
//! no registrations exist under the synthetic tenant, all operations are
//! naturally local.
//!
//! Snapshot metadata lives in the store (Kind::Snapshot, ADR-0012) —
//! persistent modes serve snapshots across restarts; 5.5.15 still allows
//! dropping them under resource pressure (evict_over_cap).
//! Fills follow the 5.7.2.4 distributed path and page past max_limit, up to
//! the 5.5.6 result ceiling (fill_cap) and only for as long as the snapshot
//! exists — a fill whose snapshot was deleted or evicted frees its copy
//! (fill_cancelled), which nothing else could reach.
//! Resource pressure evicts lowest-priority snapshots (evict_over_cap).

use crate::negotiate::{
    check_params, created, no_content, parse_accept, parse_body, respond, single_header,
    tenant_from, ApiError, ApiResult, BodyKind, CleanParams,
};
use crate::state::{now_iso, AppState};
use antares_model::{NgsiError, TenantId, API_ROOT};
use antares_store::CurrentStateDriverExt;
use antares_store::Kind;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

const DEFAULT_LIFETIME_SECS: i64 = 86_400; // 1 day
const MAX_LIFETIME_SECS: i64 = 604_800; // 7 days — the "configured limit"

fn bad(m: String) -> NgsiError {
    NgsiError::BadRequestData(m)
}

/// 5.5.6: an unexpected failure while filling a snapshot surfaces as
/// InternalError. Its detail reaches the client twice — in
/// snapshotQueriesDetails.problemDetails (5.2.41) and in the 5.3.4
/// SnapshotNotification body — so the internal error text stays in the
/// server log and the client-visible detail is generic.
fn opaque(what: &str, e: &dyn std::fmt::Debug) -> NgsiError {
    tracing::error!("snapshot {what} failed: {e:?}");
    NgsiError::InternalError(format!("{what} failed"))
}

/// 5.2.41: expiresAt from the suggested snapshotLifetime, bounded by the
/// system limit (5.16.1.4 "applying the configured limit"). Every caller
/// passed the members through `validate`, which refuses a lifetime that is
/// no ISO 8601 duration, so a present lifetime always parses here.
fn expires_at(meta: &Map<String, Value>) -> String {
    let secs = meta
        .get("snapshotLifetime")
        .and_then(Value::as_str)
        .and_then(crate::entity_map::iso8601_secs)
        // A zero or negative suggestion would answer 201 with a Location
        // that is already expired, so the broker floor applies as well as
        // the ceiling — the same reasoning the EntityMap lifetime carries.
        .map_or(DEFAULT_LIFETIME_SECS, |s| s.clamp(1, MAX_LIFETIME_SECS));
    // `secs` is clamped to MAX_LIFETIME_SECS above, far inside jiff's range.
    let now = jiff::Timestamp::now();
    crate::state::stamp(
        now.checked_add(jiff::SignedDuration::from_secs(secs))
            .unwrap_or(now),
    )
}

fn expired(meta: &Value) -> bool {
    meta.get("expiresAt")
        .and_then(Value::as_str)
        .and_then(crate::state::instant)
        .is_some_and(|e| e < jiff::Timestamp::now())
}

/// The internal tenant holding the synth-tenant -> (owner, snapshot id)
/// index docs (durable reverse lookup for 6.3.22 notification stamping).
fn snap_index_tenant() -> Option<TenantId> {
    TenantId::new_internal("snap-index").ok()
}

/// 5.2.41: a Snapshot's type is fixed to "Snapshot". The reverse-index
/// marker docs share Kind::Snapshot storage but are not Snapshots, so every
/// place that reads a document as one filters on this — a listing that does
/// not would count and select the markers as snapshots.
fn is_snapshot(meta: &Value) -> bool {
    meta.get("type").and_then(Value::as_str) == Some("Snapshot")
}

/// Registry access with lazy expiry: an expired snapshot is gone to every
/// caller. Reading is a read — the meta row, its reverse-index entry and
/// the synthetic tenant's data are freed by `sweep_expired_snapshots` on
/// the sweep tick, not by the request that found the expiry. Snapshot docs
/// live in the store (Kind::Snapshot) so restarts keep them on persistent
/// store modes.
pub(crate) async fn snap_get(st: &AppState, tenant: &TenantId, id: &str) -> Option<Value> {
    snap_lookup(st, tenant, id).await.ok().flatten()
}

/// [`snap_get`] that tells a store error apart from a Snapshot that is gone
/// (S4-22): the fill reads "gone" as the order to drop its copy, so a read
/// that failed must never look like one.
async fn snap_lookup(
    st: &AppState,
    tenant: &TenantId,
    id: &str,
) -> Result<Option<Value>, NgsiError> {
    Ok(st
        .store
        .get(tenant, Kind::Snapshot, id)
        .await?
        .filter(|meta| is_snapshot(meta) && !expired(meta)))
}

/// The internal member naming the fill that holds a `preparing` Snapshot
/// and until when (S4-22): `{"owner": <run id>, "until": <time>}`. A fill
/// renews it before each query; one whose holder died (a killed replica, a
/// restart) runs out and the sweep starts the fill again under a new owner,
/// so a Snapshot never stays `preparing` because the task that filled it is
/// gone. The store's read-modify-write is the claim, so of two replicas one
/// wins it.
const LEASE_MEMBER: &str = "__fillLease";

/// The internal member a clone keeps its source's id in, so a clone the
/// sweep resumes copies its source again rather than running its queries.
const CLONE_OF_MEMBER: &str = "__cloneOf";

/// How long a fill's lease lasts without a renewal: longer than one query
/// of a fill takes (every page of it, a hundred at most), short enough that
/// a fill a restart killed resumes within minutes.
const FILL_LEASE_SECS: i64 = 600;

/// Fills one process runs at once (S4-22, PRF-04): a burst of creates waits
/// here instead of running every fill's queries against the store together.
static FILLS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

fn lease(owner: &str) -> Value {
    let now = jiff::Timestamp::now();
    let until = now
        .checked_add(jiff::SignedDuration::from_secs(FILL_LEASE_SECS))
        .unwrap_or(now);
    json!({"owner": owner, "until": crate::state::stamp(until)})
}

fn leased_to(meta: &Value, owner: &str) -> bool {
    meta.get(LEASE_MEMBER)
        .and_then(|l| l.get("owner"))
        .and_then(Value::as_str)
        == Some(owner)
}

/// A `preparing` Snapshot whose fill nobody holds: its lease ran out, or it
/// has none (written before leases) and is older than one lease.
fn fill_orphaned(meta: &Value) -> bool {
    if meta.get("snapshotStatus").and_then(Value::as_str) != Some("preparing") {
        return false;
    }
    let since = match meta.get(LEASE_MEMBER) {
        Some(l) => l
            .get("until")
            .and_then(Value::as_str)
            .and_then(crate::state::instant),
        None => meta
            .get("createdAt")
            .and_then(Value::as_str)
            .and_then(crate::state::instant)
            .and_then(|c| {
                c.checked_add(jiff::SignedDuration::from_secs(FILL_LEASE_SECS))
                    .ok()
            }),
    };
    since.is_none_or(|t| t < jiff::Timestamp::now())
}

/// How often a [`Keeper`] renews: four times a lease.
const KEEP_MS: u32 = 150_000;

/// Renews a fill's lease every [`KEEP_MS`] while the fill is inside one
/// query's pages or a clone's copy, between the renewals the fill makes
/// itself, so a query longer than a lease is not taken for a dead fill.
/// Stops once dropped, or once the lease is no longer this fill's.
struct Keeper(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Keeper {
    fn start(st: &AppState, tenant: &TenantId, id: &str, owner: &str) -> Self {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (st, tenant, id, owner, stop) = (
            st.clone(),
            tenant.clone(),
            id.to_owned(),
            owner.to_owned(),
            done.clone(),
        );
        crate::spawn_loop(async move {
            loop {
                crate::sleep_ms(KEEP_MS).await;
                if stop.load(std::sync::atomic::Ordering::Relaxed)
                    || hold(&st, &tenant, &id, &owner).await != Hold::Held
                {
                    return;
                }
            }
        });
        Self(done)
    }
}

impl Drop for Keeper {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// What renewing a fill's lease found.
#[derive(Debug, PartialEq)]
enum Hold {
    /// Still this fill's: go on.
    Held,
    /// The Snapshot was deleted or expired: drop the copy.
    Gone,
    /// Another fill holds it now: stop and touch nothing.
    Lost,
}

/// Renew `owner`'s lease on Snapshot `id`. A store that keeps failing
/// answers [`Hold::Held`]: the copy stays and the fill goes on, since only a
/// Snapshot known to be gone may lose its copy; if the fill cannot finish,
/// its lease runs out and the sweep starts it again.
async fn hold(st: &AppState, tenant: &TenantId, id: &str, owner: &str) -> Hold {
    let r = crate::history::with_retry("snapshot lease", || {
        st.store.mutate(tenant, Kind::Snapshot, id, |d| {
            if !is_snapshot(d) || expired(d) {
                return Err(Hold::Gone);
            }
            if !leased_to(d, owner) {
                return Err(Hold::Lost);
            }
            // a Snapshot is an object (`is_snapshot` read its type)
            d.as_object_mut()
                .ok_or(Hold::Gone)?
                .insert(LEASE_MEMBER.into(), lease(owner));
            Ok(())
        })
    })
    .await;
    match r {
        Ok(None) => Hold::Gone,
        Ok(Some(Ok(()))) => Hold::Held,
        Ok(Some(Err(h))) => h,
        Err(e) => {
            tracing::warn!("snapshot {id}: lease not renewed, the fill goes on: {e}");
            Hold::Held
        }
    }
}

/// Start the fill of every Snapshot of `tenant` nobody holds (S4-22): claim
/// it under a new owner, drop what the dead fill copied, run it again.
async fn resume_orphans(st: &AppState, tenant: &TenantId, ids: Vec<String>) {
    for id in ids {
        let owner = uuid::Uuid::new_v4().to_string();
        let claimed = st
            .store
            .mutate(tenant, Kind::Snapshot, &id, |d| {
                if !is_snapshot(d) || expired(d) || !fill_orphaned(d) {
                    return Err(());
                }
                // a Snapshot is an object (`is_snapshot` read its type)
                d.as_object_mut()
                    .ok_or(())?
                    .insert(LEASE_MEMBER.into(), lease(&owner));
                Ok(d.clone())
            })
            .await;
        let Ok(Some(Ok(meta))) = claimed else {
            continue;
        };
        if let Some(synth) = synth_tenant(&meta) {
            purge_synth(st, &synth).await;
        }
        tracing::info!("snapshot {id}: its fill had no holder, started again");
        let (st2, t2) = (st.clone(), tenant.clone());
        match meta.get(CLONE_OF_MEMBER).and_then(Value::as_str) {
            Some(src) => {
                let src = src.to_owned();
                crate::spawn(async move { clone_fill(&st2, &t2, &src, &id, &owner).await });
            }
            None => crate::spawn(async move { fill_snapshot(&st2, &t2, &id, &owner).await }),
        }
    }
}

/// 5.16.1.4: "If the NGSI-LD endpoint already knows about this Snapshot …
/// an error of type AlreadyExists shall be raised." The store's create is
/// the atomic check-and-insert, so two concurrent creates of the same id
/// cannot both succeed and overwrite each other's synthetic tenant.
async fn snap_insert(
    st: &AppState,
    tenant: &TenantId,
    id: &str,
    meta: &Value,
) -> Result<(), NgsiError> {
    if !st
        .store
        .create(tenant, Kind::Snapshot, id, meta.clone())
        .await?
    {
        return Err(NgsiError::AlreadyExists(format!(
            "snapshot {id} already exists"
        )));
    }
    // Durable reverse index for snapshot_of_synth. The marker carries the
    // synthetic tenant as its `id` as well as its key: it is the only record
    // that names that tenant once the Snapshot document is gone, and a
    // document a listing cannot identify is one nothing can walk.
    if let (Some(synth), Some(idx)) = (
        meta.get("__tenant").and_then(Value::as_str),
        snap_index_tenant(),
    ) {
        let _ = st
            .store
            .create(
                &idx,
                Kind::Snapshot,
                synth,
                json!({"id": synth, "tenant": tenant.as_str(), "snapshot": id}),
            )
            .await;
    }
    Ok(())
}

/// Members an Update Snapshot (5.16.4.4) writes.
const UPDATE_MEMBERS: &[&str] = &[
    "snapshotLifetime",
    "snapshotPriority",
    "endpoint",
    "receiverInfo",
    "expiresAt",
    "modifiedAt",
];

/// Write `members` of `meta` back onto a snapshot that already exists (a
/// member `meta` lacks is removed); a snapshot deleted meanwhile is not
/// resurrected. Only the named members are written, so a fill finishing
/// beside an Update Snapshot keeps the other's members: each writer read
/// the document before the other wrote it. Returns the stored document.
async fn snap_put(
    st: &AppState,
    tenant: &TenantId,
    meta: &Value,
    members: &[&str],
) -> Option<Value> {
    let id = meta.get("id").and_then(Value::as_str)?;
    st.store
        .mutate(tenant, Kind::Snapshot, id, |d| {
            // a Snapshot is an object: every writer of one is this module
            let o = d.as_object_mut().ok_or(())?;
            for k in members {
                match meta.get(*k) {
                    Some(v) => o.insert((*k).to_owned(), v.clone()),
                    None => o.remove(*k),
                };
            }
            Ok::<_, ()>(d.clone())
        })
        .await
        .ok()
        .flatten()
        .and_then(Result::ok)
}

/// Remove a snapshot everywhere: doc, synth-tenant index, data purge.
pub(crate) async fn snap_remove(st: &AppState, tenant: &TenantId, id: &str, meta: &Value) {
    let _ = st.store.delete(tenant, Kind::Snapshot, id).await;
    purge_data_bg(st, meta);
}

/// 4.22 for snapshots: `snap_get` refuses an expired one, and this is what
/// removes it — meta, reverse-index entry and the synthetic tenant's data,
/// through the same `snap_remove` a read used to call, so an expiry
/// collected on the tick frees exactly what an expiry collected on a read
/// freed.
pub(crate) async fn sweep_expired_snapshots(st: &AppState, tenant: &TenantId) -> usize {
    let mut dead: Vec<(String, Value)> = Vec::new();
    let mut orphans: Vec<String> = Vec::new();
    if crate::csource::walk_docs(st, tenant, Kind::Snapshot, |doc| {
        // the reverse-index markers share the kind and carry an id too
        let id = doc.get("id").and_then(Value::as_str).map(str::to_owned);
        if let Some(id) = id.filter(|_| is_snapshot(&doc)) {
            if expired(&doc) {
                dead.push((id, doc));
            } else if fill_orphaned(&doc) {
                orphans.push(id);
            }
        }
        Ok(())
    })
    .await
    .is_err()
    {
        return 0;
    }
    let n = dead.len();
    for (id, meta) in dead {
        snap_remove(st, tenant, &id, &meta).await;
    }
    resume_orphans(st, tenant, orphans).await;
    n
}

/// The synthetic tenants holding the copies of `tenant`'s Snapshots, expired
/// ones included until the sweep frees them: an erasure reaches each.
///
/// # Errors
/// The store's, when the Snapshots cannot be listed.
pub(crate) async fn copies_of(
    st: &AppState,
    tenant: &TenantId,
) -> Result<Vec<TenantId>, NgsiError> {
    let mut copies = Vec::new();
    crate::csource::walk_docs(st, tenant, Kind::Snapshot, |doc| {
        if is_snapshot(&doc) {
            copies.extend(synth_tenant(&doc));
        }
        Ok(())
    })
    .await?;
    Ok(copies)
}

fn synth_tenant(meta: &Value) -> Option<TenantId> {
    meta.get("__tenant")
        .and_then(Value::as_str)
        .and_then(|t| TenantId::new_internal(t).ok())
}

/// Free the snapshot's isolated copy. 5.5.15 permits every Core and
/// Temporal operation on a snapshot, so a snapshot-scoped request can put
/// documents of ANY kind under the synthetic tenant — once the snapshot is
/// gone none of them is reachable (no client may name the internal tenant),
/// so every kind is dropped, not only the filled data.
///
/// The tenant purge, not a per-document sweep: it covers every kind rather
/// than a list that has to be kept in step with `Kind`, and it removes the
/// tenant itself. A sweep leaves the empty tenant behind, and `/q/tenants`
/// reports what the store holds a tenant entry for — so every snapshot ever
/// deleted would leave a `snap-<uuid>` name in the inventory for the life of
/// the deployment.
async fn purge_synth(st: &AppState, synth: &TenantId) {
    // history lives behind its own driver seam
    let _ = st.temporal.purge_tenant(synth).await;
    let _ = st.store.purge_tenant(synth).await;
}

/// The teardown that outlives the request: the copy is dropped, and only
/// then the reverse-index entry that named the synthetic tenant. That order
/// is the recovery path — the Snapshot document is already gone, so the index
/// entry is the last thing that says which tenant holds the copy, and a
/// broker that stops mid-teardown leaves it pointing at data still there
/// rather than data nothing names.
fn purge_data_bg(st: &AppState, meta: &Value) {
    let (Some(synth), Some(idx)) = (synth_tenant(meta), snap_index_tenant()) else {
        return;
    };
    let st = st.clone();
    crate::spawn(async move {
        purge_synth(&st, &synth).await;
        let _ = st.store.delete(&idx, Kind::Snapshot, synth.as_str()).await;
    });
}

/// The snapshot document as presented to clients (internal members hidden).
fn present(meta: &Value) -> Value {
    let mut out = meta.clone();
    crate::policy::strip_internal(&mut out);
    // 5.16.6 sends each receiverInfo pair as a header: a read answers that
    // a credential is set, as a Subscription read does (5.2.15).
    if let Some(ri) = out.get_mut("receiverInfo") {
        crate::pairs::mask_pairs(ri, |_, _| false);
    }
    out
}

enum Mode {
    Create,
    Clone,
    Update,
}

/// 5.2.41 Table 5.2.41-1/-2 validation. Output-only members are IGNORED
/// (stripped); read-only members in the wrong mode are BadRequestData.
fn validate(body: &Value, mode: Mode) -> Result<Map<String, Value>, NgsiError> {
    let mut o = body
        .as_object()
        .cloned()
        .ok_or_else(|| bad("snapshot must be a JSON object".into()))?;
    o.remove("@context");
    // the broker's own members (the synthetic tenant, the creating subject)
    // are not the client's to send — see policy::strip_internal
    o.retain(|k, _| !k.starts_with("__"));
    // output-only members (Table 5.2.41-2) "shall be ignored"
    for k in [
        "snapshotStatus",
        "snapshotQueriesDetails",
        "snapshotTemporalQueriesDetails",
        "createdAt",
        "modifiedAt",
        "expiresAt",
        "lastUsedAt",
    ] {
        o.remove(k);
    }
    match mode {
        Mode::Create => {
            if o.get("type").and_then(Value::as_str) != Some("Snapshot") {
                return Err(bad("type must be \"Snapshot\" (5.2.41)".into()));
            }
            if !o.contains_key("snapshotQueries") && !o.contains_key("snapshotTemporalQueries") {
                return Err(bad(
                    "at least one of snapshotQueries or snapshotTemporalQueries \
                     shall be present (5.2.41)"
                        .into(),
                ));
            }
        }
        Mode::Clone | Mode::Update => {
            // "both shall be omitted when updating the Snapshot status or
            // cloning the Snapshot" — read-only after creation
            if o.contains_key("snapshotQueries") || o.contains_key("snapshotTemporalQueries") {
                return Err(bad(
                    "snapshotQueries/snapshotTemporalQueries are read-only after \
                     creation (5.2.41)"
                        .into(),
                ));
            }
            // Table 5.2.41-1: the id "cannot be later modified in update
            // operations" — of whatever JSON type it is sent as
            if matches!(mode, Mode::Update) && o.contains_key("id") {
                return Err(bad("the snapshot id cannot be modified (5.2.41)".into()));
            }
        }
    }
    for (key, temporal) in [
        ("snapshotQueries", false),
        ("snapshotTemporalQueries", true),
    ] {
        if let Some(qs) = o.get(key) {
            let arr = qs
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or_else(|| bad(format!("{key} must be a non-empty array of Query (5.2.41)")))?;
            for q in arr {
                let qo = q
                    .as_object()
                    .ok_or_else(|| bad(format!("{key} entries must be Query objects (5.2.23)")))?;
                if qo.get("type").and_then(Value::as_str) != Some("Query") {
                    return Err(bad(format!("{key} entries must have type Query (5.2.23)")));
                }
                if temporal != qo.contains_key("temporalQ") {
                    return Err(bad(format!(
                        "{key} entries must {} a temporalQ element (5.2.41)",
                        if temporal { "carry" } else { "not carry" }
                    )));
                }
            }
        }
    }
    if let Some(p) = o.get("snapshotPriority") {
        let ok = p.as_i64().is_some_and(|n| (1..=10).contains(&n));
        if !ok {
            return Err(bad(
                "snapshotPriority must be an integer between 1 and 10 (5.2.41)".into(),
            ));
        }
    }
    if let Some(l) = o.get("snapshotLifetime") {
        let ok = l
            .as_str()
            .and_then(crate::entity_map::iso8601_secs)
            .is_some();
        if !ok {
            return Err(bad(
                "snapshotLifetime must be an ISO 8601 duration (5.2.41)".into(),
            ));
        }
    }
    // Table 5.2.41-1: endpoint is a "Dereferenceable URI", so a string that is
    // no URI at all names nothing the 5.16.6 notification could be sent to.
    if let Some(e) = o.get("endpoint") {
        let uri = e
            .as_str()
            .ok_or_else(|| bad("endpoint must be a URI string (5.2.41)".into()))?;
        antares_model::EntityId::new(uri)
            .map_err(|_| bad(format!("endpoint is not a valid URI: {uri:?} (5.2.41)")))?;
    }
    // Table 5.2.41-1: receiverInfo is a KeyValuePair[] (5.2.22 — both members
    // Strings), and 5.16.6 renders each pair as one header on the
    // SnapshotNotification POST. 6.3.8 binds those to RFC 7230 for the same
    // reason it binds a Subscription's: a pair that cannot be a header leaves
    // a Snapshot whose notification can only ever fail, silently.
    if let Some(ri) = o.get("receiverInfo") {
        let pairs = ri
            .as_array()
            .ok_or_else(|| bad("receiverInfo must be a KeyValuePair array (5.2.41)".into()))?;
        for kv in pairs {
            let (k, v) = (kv["key"].as_str(), kv["value"].as_str());
            if !k.is_some_and(crate::negotiate::is_field_name)
                || !v.is_some_and(crate::negotiate::is_field_value)
            {
                return Err(bad(format!(
                    "receiverInfo entry {kv} is not a valid HTTP header (RFC 7230, 6.3.8)"
                )));
            }
            // The HTTP client owns framing and connection headers, as on a
            // Subscription's receiverInfo (RFC 7230 3.3, 5.4, 6.1).
            if let Some(k) = k.filter(|k| {
                crate::pairs::CLIENT_OWNED_HEADERS
                    .iter()
                    .any(|o| o.eq_ignore_ascii_case(k))
            }) {
                return Err(bad(format!(
                    "receiverInfo may not set the {k} header: the HTTP client sets it \
                     (RFC 7230, 6.3.8)"
                )));
            }
        }
    }
    Ok(o)
}

/// Fresh metadata for a new snapshot (create or clone) — 5.16.1.4/5.16.2.4:
/// timestamps now, status "preparing", priority default 5, bounded expiresAt;
/// the fill lease of `owner`, and for a clone the source it copies.
fn new_meta(
    mut o: Map<String, Value>,
    tenant: &TenantId,
    headers: &HeaderMap,
    owner: &str,
    clone_of: Option<&str>,
) -> Result<(String, Value), NgsiError> {
    let id = match o.get("id").and_then(Value::as_str) {
        Some(id) => {
            antares_model::EntityId::new(id)
                .map_err(|_| bad(format!("snapshot id is not a valid URI: {id:?}")))?;
            id.to_owned()
        }
        None => {
            let id = format!("urn:ngsi-ld:Snapshot:{}", uuid::Uuid::new_v4());
            o.insert("id".into(), Value::String(id.clone()));
            id
        }
    };
    let ts = now_iso();
    o.insert("type".into(), Value::String("Snapshot".into()));
    o.insert("createdAt".into(), Value::String(ts.clone()));
    o.insert("modifiedAt".into(), Value::String(ts));
    o.insert("snapshotStatus".into(), Value::String("preparing".into()));
    // 5.2.41 Table 5.2.41-2: lastUsedAt "is initialized at creation time"
    o.insert("lastUsedAt".into(), Value::String(now_iso()));
    o.insert("expiresAt".into(), Value::String(expires_at(&o)));
    o.entry("snapshotPriority".to_owned())
        .or_insert(Value::Number(5.into()));
    o.insert(
        "__tenant".into(),
        Value::String(format!("snap-{}", uuid::Uuid::new_v4().simple())),
    );
    o.insert(LEASE_MEMBER.into(), lease(owner));
    if let Some(src) = clone_of {
        o.insert(CLONE_OF_MEMBER.into(), Value::String(src.to_owned()));
    }
    // Whose snapshot this is. 5.16.1's fill runs after the request has been
    // answered, so the subject it copies under has to be recorded here or it
    // is gone; and 5.16.1/5.16.2/5.16.7 act on everything the tenant holds,
    // which is why a narrowing answer to any of them is a refusal
    // (policy::WHOLE_TENANT) rather than a smaller snapshot.
    if let Some(subject) =
        crate::policy::subject_member(&crate::policy::subject_of(tenant, headers))
    {
        o.insert(crate::policy::SUBJECT_MEMBER.into(), subject);
    }
    Ok((id, Value::Object(o)))
}

/// 5.2.41: lastUsedAt tracks "the point in time when the snapshot was most
/// recently used" — refreshed on every snapshot-scoped operation.
async fn snap_touch(st: &AppState, tenant: &TenantId, id: &str) {
    let _ = st
        .store
        .mutate(tenant, Kind::Snapshot, id, |meta| {
            // a Snapshot is an object: every writer of one is this module
            let o = meta.as_object_mut().ok_or(())?;
            o.insert("lastUsedAt".into(), Value::String(now_iso()));
            Ok::<_, ()>(())
        })
        .await;
}

/// Reverse lookup: which (owner tenant, snapshot id) does a synthetic
/// "snap-…" tenant belong to? Used to stamp NGSILD-Snapshot on
/// notifications from snapshot-scoped subscriptions (6.3.22) without
/// leaking the internal tenant.
pub(crate) async fn snapshot_of_synth(st: &AppState, synth: &str) -> Option<(TenantId, String)> {
    // only synthetic tenants are indexed; every other tenant skips the lookup
    if !synth.starts_with("snap-") {
        return None;
    }
    let idx = snap_index_tenant()?;
    let doc = st
        .store
        .get(&idx, Kind::Snapshot, synth)
        .await
        .ok()
        .flatten()?;
    let owner = TenantId::new(doc.get("tenant")?.as_str()?).ok()?;
    Some((owner, doc.get("snapshot")?.as_str()?.to_owned()))
}

/// The tenant an operation is judged under. 6.3.22 scopes a request to a
/// Snapshot by swapping the tenant for the snapshot's internal one, so the
/// handlers below it serve the frozen copy; the subject asking is still the
/// owner tenant's. An engine keyed by tenant — which is the shape every
/// rule set has — would otherwise be asked about an id no deployment ever
/// wrote a rule for, and every snapshot-scoped read would step out from
/// under its rules. Only a synthetic tenant costs the lookup.
pub(crate) async fn asking_tenant(st: &AppState, tenant: &TenantId) -> TenantId {
    match snapshot_of_synth(st, tenant.as_str()).await {
        Some((owner, _)) => owner,
        None => tenant.clone(),
    }
}

/// 5.5.15: "If an implementation determines that it is low on resources,
/// it may delete one or more snapshots", considering snapshotPriority
/// (lowest first; earliest expiresAt breaks ties). The resource signal is
/// the per-tenant registry cap (AppState.snapshot_cap); the just-created
/// snapshot is never the victim. Evicted snapshots with an endpoint are
/// notified with expiresAt set before notifiedAt — the 5.3.4 deletion
/// encoding.
async fn evict_over_cap(st: &AppState, tenant: &TenantId, keep: &str) {
    let victims: Vec<(String, Map<String, Value>)> = {
        // a Snapshot is an object with an id: every writer of one is this
        // module, and the markers beside them carry no type
        let mut metas: Vec<(String, Map<String, Value>)> = st
            .store
            .list(tenant, Kind::Snapshot)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(is_snapshot)
            .filter_map(|mut v| v.as_object_mut().map(std::mem::take))
            .filter_map(|o| Some((o.get("id")?.as_str()?.to_owned(), o)))
            .collect();
        if metas.len() <= st.snapshot_cap {
            return;
        }
        let over = metas.len().saturating_sub(st.snapshot_cap);
        metas.sort_by_key(|(_, v)| {
            (
                v.get("snapshotPriority")
                    .and_then(Value::as_i64)
                    .unwrap_or(5),
                v.get("expiresAt")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            )
        });
        metas
            .into_iter()
            .filter(|(id, _)| id != keep)
            .take(over)
            .collect()
    };
    for (id, mut o) in victims {
        // deletion signal: expiresAt strictly before the notification
        o.insert("expiresAt".into(), Value::String(now_iso()));
        let meta = Value::Object(o);
        snap_remove(st, tenant, &id, &meta).await;
        let st2 = st.clone();
        crate::spawn(async move {
            send_notification(&st2, &meta).await;
        });
    }
}

// ---------- 5.16.1 Create Snapshot (POST /snapshots, 6.36.3.1) ----------

pub async fn create_snapshot(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        check_params(&params, &["local"])?;
        gate!(st, &tenant, &headers, "5.16.1").await?;
        // 6.3.5: the @context rules apply to the snapshot body like to any
        // other POST payload (media type, Link header, body @context)
        let v = parse_body(&st, &headers, &body, BodyKind::Standard)
            .await?
            .value;
        let o = validate(&v, Mode::Create)?;
        let owner = uuid::Uuid::new_v4().to_string();
        let (id, meta) = new_meta(o, &tenant, &headers, &owner, None)?;
        snap_insert(&st, &tenant, &id, &meta).await?;
        evict_over_cap(&st, &tenant, &id).await;
        let (st2, t2, id2) = (st.clone(), tenant.clone(), id.clone());
        crate::spawn(async move {
            fill_snapshot(&st2, &t2, &id2, &owner).await;
        });
        Ok::<_, ApiError>(created(format!("/ngsi-ld/v1/snapshots/{id}"), &tenant))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.16.1.4 background fill: execute every (temporal) query, store the
/// results under the synthetic tenant, derive the status, notify.
async fn fill_snapshot(st: &AppState, tenant: &TenantId, id: &str, owner: &str) {
    // FILLS is never closed, so the acquire always hands out a permit
    let permit = FILLS.acquire().await.ok();
    // the lease is renewed before every query below and by the keeper in
    // between; a fill that waited for its permit past it finds another owner
    // and stops
    if hold(st, tenant, id, owner).await != Hold::Held {
        return;
    }
    let keeper = Keeper::start(st, tenant, id, owner);
    let Some(meta) = snap_get(st, tenant, id).await else {
        return;
    };
    let Some(synth) = synth_tenant(&meta) else {
        return;
    };
    let ctx = st.loader.core();
    let (mut n_fail, mut n_res, mut n_empty, mut copied) = (0usize, 0usize, 0usize, 0usize);
    let mut detail = |r: Result<usize, NgsiError>| -> Value {
        match r {
            Ok(0) => {
                n_empty = n_empty.saturating_add(1);
                json!({"resultStatus": "empty"})
            }
            Ok(_) => {
                n_res = n_res.saturating_add(1);
                json!({"resultStatus": "success"})
            }
            Err(e) => {
                n_fail = n_fail.saturating_add(1);
                json!({"resultStatus": "failure",
                       "problemDetails": crate::negotiate::problem_value(&e)})
            }
        }
    };
    let cap = fill_cap(st);
    let mut q_details = Vec::new();
    for q in meta
        .get("snapshotQueries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if fill_cancelled(st, tenant, id, &synth, owner).await {
            return;
        }
        let r = run_query(st, tenant, &synth, q, &ctx, cap.saturating_sub(copied)).await;
        if let Ok(n) = &r {
            copied = copied.saturating_add(*n);
        }
        q_details.push(detail(r));
    }
    let mut tq_details = Vec::new();
    for q in meta
        .get("snapshotTemporalQueries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if fill_cancelled(st, tenant, id, &synth, owner).await {
            return;
        }
        let r = run_temporal_query(st, tenant, &synth, id, q, cap.saturating_sub(copied)).await;
        if let Ok(n) = &r {
            copied = copied.saturating_add(*n);
        }
        tq_details.push(detail(r));
    }
    if fill_cancelled(st, tenant, id, &synth, owner).await {
        return;
    }
    let status = if n_fail == 0 && n_empty == 0 && n_res > 0 {
        "success"
    } else if n_res > 0 {
        "partial"
    } else if n_empty > 0 {
        "empty"
    } else {
        "failure"
    };
    // the outcome's notification is an outbound call: it holds no permit
    drop((keeper, permit));
    finish(st, tenant, id, owner, status, Some((q_details, tq_details))).await;
}

/// One 5.2.23 Query executed per the DISTRIBUTED query behaviour
/// (5.16.1.4 -> 5.7.2.4): local content plus every matching Context
/// Source; results are copied into the snapshot's synthetic tenant. Every
/// page is retrieved ("all pages are to be retrieved completely") within
/// `budget` documents — beyond it nothing is copied (5.5.6).
async fn run_query(
    st: &AppState,
    tenant: &TenantId,
    synth: &TenantId,
    q: &Value,
    ctx: &antares_jsonld::Context,
    budget: usize,
) -> Result<usize, NgsiError> {
    let qo = q
        .as_object()
        .ok_or_else(|| bad("Query must be an object".into()))?;
    let mut vp: HashMap<String, String> = HashMap::new();
    crate::paging::query_doc_params(qo, false, &mut vp)?;
    // the caller's own `q`, held to the caller's caps before the paged
    // filter reads it with the narrowed ones (threat V23)
    if let Some(q) = vp.get("q") {
        antares_ql::parse_q(q)?;
    }
    for k in ["limit", "offset", "count"] {
        vp.remove(k);
    }
    let headers = HeaderMap::new();
    let mut warnings = Vec::new();
    let fed = if crate::federation::active(&vp) {
        crate::federation::fed_query(st, tenant, &headers, ctx, &vp, &mut warnings).await?
    } else {
        Vec::new()
    };
    // 5.16.1.4: "If the size of the respective results require pagination,
    // all pages are to be retrieved completely." They are retrieved one at a
    // time. A query wider than the budget is refused, and it has to be
    // refused without first holding its whole match set in memory to measure
    // it: the store's pre-LIMIT count is that measurement. The count is the
    // match set's only while every predicate reached the store — an
    // idPattern is applied after it (5.2.33), so with one present the
    // documents that survive are counted instead.
    let exact_total = !vp.contains_key("idPattern");
    // Federated candidates are merged after the store answers (5.7.2.4), so
    // that round takes no page and is the whole match set; only a local
    // query walks on.
    let mut fed = fed;
    let mut n = 0usize;
    let mut offset = 0usize;
    loop {
        // Only the first round asks the store to count: the pre-LIMIT total
        // makes it visit the whole match set, and the budget needs that once.
        // Later rounds carry the next-page hint in the same member, which is
        // not a count and is not read as one.
        let counted = offset == 0;
        if counted {
            vp.insert("count".into(), "true".into());
        } else {
            vp.remove("count");
        }
        let batch = crate::entities::filter_entities_paged(
            st,
            tenant,
            &vp,
            ctx,
            std::mem::take(&mut fed),
            crate::entities::Pushdown {
                page: Some((offset, st.max_limit)),
                proj: None,
                order_keys: None,
                q: None,
            },
            // a snapshot under a narrowing is refused at its gate
            None,
        )
        .await
        .map_err(|e| match e {
            ApiError::Ngsi(n) => n,
            other => opaque("query execution", &other),
        })?;
        if exact_total && counted {
            if let Some(total) = batch.total.filter(|_| batch.paged) {
                if total > budget {
                    return Err(too_many(total, budget));
                }
            }
        }
        let rows = batch.rows;
        let got = batch.docs.len();
        if n.saturating_add(got) > budget {
            return Err(too_many(n.saturating_add(got), budget));
        }
        // A write the store refuses fails the query. What this returns is
        // what the caller adds to `copied`, and `copied` decides both the
        // budget left for the next query and whether the synthetic tenant is
        // materialized — an empty snapshot has to read as empty rather than
        // as a tenant that never existed. Counting a copy that never happened
        // costs the snapshot both, and 5.2.41 would publish the query as a
        // success besides.
        // a matched Entity always carries its id
        for (id, doc) in batch
            .docs
            .iter()
            .filter_map(|d| Some((d.get("id")?.as_str()?, d)))
        {
            st.store
                .create(synth, Kind::Entity, id, doc.clone())
                .await?;
        }
        n = n.saturating_add(got);
        match crate::entities::next_scan_offset(offset, rows, 0, batch.paged, st.max_limit) {
            Some(next) => offset = next,
            None => break,
        }
    }
    Ok(n)
}

/// One temporal Query (temporalQ mandatory) — ids via the 5.7.4.4 path,
/// full evolutions copied via the store, every page retrieved but never
/// more than `budget` evolutions (5.5.6).
async fn run_temporal_query(
    st: &AppState,
    tenant: &TenantId,
    synth: &TenantId,
    id: &str,
    q: &Value,
    budget: usize,
) -> Result<usize, NgsiError> {
    let qo = q
        .as_object()
        .ok_or_else(|| bad("Query must be an object".into()))?;
    let mut vp: HashMap<String, String> = HashMap::new();
    crate::paging::query_doc_params(qo, true, &mut vp)?;
    for k in ["limit", "offset", "count"] {
        vp.remove(k);
    }
    // The copy holds what the query selects: its attrs and its window. A
    // whole Evolution would hand a snapshot reader history the query never
    // asked for.
    let tq = crate::temporalq::TemporalQ::from_params(&vp, true)?;
    let ctx = st.loader.core();
    let attrs: Option<Vec<String>> = vp
        .get("attrs")
        .map(|a| a.split(',').map(|t| ctx.expand_key(t.trim())).collect());
    let tf = antares_store::filter::TemporalFilter {
        attrs: attrs.as_deref(),
        range: tq.as_ref().map(|t| antares_store::filter::InstanceRange {
            timerel: &t.timerel,
            time_at: &t.time_at,
            end_time_at: t.end_time_at.as_deref(),
            timeproperty: &t.timeproperty,
        }),
        timeproperty: tq
            .as_ref()
            .map_or("observedAt", |t| t.timeproperty.as_str()),
        ..Default::default()
    };
    // a Tenant name is `[A-Za-z0-9_-]`, always a header value
    let mut headers = HeaderMap::new();
    headers.extend(tenant_header(tenant.as_str()));
    // 5.16.1.4: "If the size of the respective results require pagination,
    // all pages are to be retrieved completely."
    let mut n = 0usize;
    let mut offset = 0usize;
    loop {
        // a snapshot deleted (or evicted) mid-fill stops the paging; what is
        // already copied is reaped by the caller
        if matches!(snap_lookup(st, tenant, id).await, Ok(None)) {
            break;
        }
        vp.insert("limit".into(), st.max_limit.to_string());
        vp.insert("offset".into(), offset.to_string());
        // The fill runs after the request that created the snapshot has
        // been answered, under a header map that is not the client's, so
        // the narrowing the creating subject was given is not in scope here
        // — a snapshot under a policy is P5's box (ADR-0020: a `Filter` on
        // a fill is a `Deny`, and the fill records whose it is).
        // The merged documents come back beside the response: an Evolution
        // this broker holds is copied from the store below, whole, and one a
        // registered Context Source holds exists only here.
        let mut merged: Vec<Value> = Vec::new();
        let resp = crate::temporal::query_temporal_collected(
            st,
            &vp,
            &headers,
            &crate::policy::Filter::default(),
            Some(&mut merged),
        )
        .await
        .map_err(|e| match e {
            ApiError::Ngsi(n) => n,
            other => opaque("temporal query execution", &other),
        })?;
        let bytes = st.read_answer(resp.into_body(), "snapshots").await?;
        let got = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v.as_array().map(Vec::len))
            .unwrap_or_default();
        if n.saturating_add(got) > budget {
            return Err(too_many(n.saturating_add(got), budget));
        }
        // `merged` is the page the answer was built from, one document per
        // Evolution of it, each carrying its Entity's id
        for (eid, sent) in merged
            .iter()
            .filter_map(|m| Some((m.get("id")?.as_str()?, m)))
        {
            // `None` is an Evolution this broker does not hold; an Err
            // is the store refusing the read, which is not the same thing and
            // is not this snapshot's to swallow.
            //
            // The copy REPLACES: a snapshot carrying both kinds of query runs
            // its Entity queries first, and copying an Entity into the
            // synthetic tenant fires the 5.6.11 auto-record hook, which leaves
            // a Temporal Evolution holding only the instant of the copy. An
            // insert-if-absent would find that stub and keep it, so the
            // snapshot would answer temporal reads with one instance of a
            // history the source has many of — and report the query a success.
            let held = st
                .temporal
                .get_temporal(tenant, eid, &tf)
                .await?
                .map(|doc| selected(doc, attrs.as_deref(), tq.as_ref()));
            // Nothing local means the Evolution came from a registered
            // Context Source (clause 5.7.4, item 4), or went since the query read it. The
            // query's merged answer is every
            // instance of it this broker was given, so it is what the
            // snapshot keeps — a local copy is the source's whole history,
            // a remote one is as much of it as the fan-out returned.
            let doc = held.unwrap_or_else(|| sent.clone());
            st.temporal.upsert(synth, eid, doc).await?;
            n = n.saturating_add(1);
        }
        if got < st.max_limit {
            break;
        }
        offset = offset.saturating_add(st.max_limit);
    }
    Ok(n)
}

/// An Evolution cut to the Attributes `attrs` names and the instances in
/// `tq`'s window; meta members stay. The store's window prunes too, but only
/// as a narrowing a backend may skip.
fn selected(
    mut doc: Value,
    attrs: Option<&[String]>,
    tq: Option<&crate::temporalq::TemporalQ>,
) -> Value {
    // an Evolution is an object; its members are what is cut
    doc.as_object_mut().into_iter().for_each(|o| {
        o.retain(|k, v| {
            if antares_model::is_meta(k) {
                return true;
            }
            if attrs.is_some_and(|want| !want.iter().any(|w| w == k)) {
                return false;
            }
            if let (Some(tq), Some(insts)) = (tq, v.as_array_mut()) {
                insts.retain(|i| tq.instance_matches(i));
            }
            true
        });
    });
    doc
}

/// Write the fill's outcome and release its lease, in one read-modify-write
/// that only the lease's holder passes: a fill that lost its Snapshot to a
/// resumed one cannot overwrite the newer outcome. Only the members `finish`
/// owns are written, so an Update Snapshot beside it keeps its own.
async fn finish(
    st: &AppState,
    tenant: &TenantId,
    id: &str,
    owner: &str,
    status: &str,
    details: Option<(Vec<Value>, Vec<Value>)>,
) {
    let r = crate::history::with_retry("snapshot finish", || {
        st.store.mutate(tenant, Kind::Snapshot, id, |d| {
            if !is_snapshot(d) || expired(d) || !leased_to(d, owner) {
                return Err(());
            }
            let o = d.as_object_mut().ok_or(())?;
            o.insert("snapshotStatus".into(), Value::String(status.into()));
            o.insert("modifiedAt".into(), Value::String(now_iso()));
            if let Some((q, tq)) = &details {
                if !q.is_empty() {
                    o.insert("snapshotQueriesDetails".into(), Value::Array(q.clone()));
                }
                if !tq.is_empty() {
                    o.insert(
                        "snapshotTemporalQueriesDetails".into(),
                        Value::Array(tq.clone()),
                    );
                }
            }
            o.remove(LEASE_MEMBER);
            Ok(d.clone())
        })
    })
    .await;
    match r {
        Ok(Some(Ok(meta))) => send_notification(st, &meta).await,
        Ok(_) => {}
        Err(e) => tracing::warn!("snapshot {id}: outcome not written, the fill resumes: {e}"),
    }
}

/// 5.16.6 / 5.3.4 SnapshotNotification (sent only when endpoint is set).
async fn send_notification(st: &AppState, meta: &Value) {
    let Some(uri) = meta.get("endpoint").and_then(Value::as_str) else {
        return;
    };
    if st.egress.check_url(uri).await.is_err() {
        return;
    }
    let mut body = json!({
        "id": format!("urn:ngsi-ld:SnapshotNotification:{}", uuid::Uuid::new_v4()),
        "type": "SnapshotNotification",
        "notifiedAt": now_iso(),
        "snapshotId": meta.get("id").cloned().unwrap_or_default(),
        "snapshotStatus": meta.get("snapshotStatus").cloned().unwrap_or_default(),
        "snapshotPriority": meta.get("snapshotPriority").cloned().unwrap_or_default(),
        "expiresAt": meta.get("expiresAt").cloned().unwrap_or_default(),
    });
    // 5.3.4 Table 5.3.4-1 names the temporal list
    // "temporalSnapshotQueriesDetails" (unlike 5.2.41's
    // "snapshotTemporalQueriesDetails") — the notification datatype's own
    // table governs the notification payload.
    for (from, to) in [
        ("snapshotQueriesDetails", "snapshotQueriesDetails"),
        (
            "snapshotTemporalQueriesDetails",
            "temporalSnapshotQueriesDetails",
        ),
    ] {
        if let (Some(v), Some(body)) = (meta.get(from), body.as_object_mut()) {
            body.insert(to.into(), v.clone());
        }
    }
    let mut req = st.http.post(uri).header("Content-Type", "application/json");
    if let Some(ri) = meta.get("receiverInfo").and_then(Value::as_array) {
        for kv in ri {
            if let (Some(k), Some(v)) = (
                kv.get("key").and_then(Value::as_str),
                kv.get("value").and_then(Value::as_str),
            ) {
                req = req.header(k, v);
            }
        }
    }
    let req = req.body(serde_json::to_vec(&body).unwrap_or_default());
    let _ = antares_jsonld::io_deadline(req.send(), 8_000).await;
}

// ---------- 5.16.7 Purge Snapshots (DELETE /snapshots, 6.36.3.2) ----------

pub async fn purge_snapshots(
    State(st): State<AppState>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        check_params(&params, &["q", "local"])?;
        gate!(st, &tenant, &headers, "5.16.7").await?;
        // 5.16.7.4: the query is mandatory and restricted to Snapshot members
        let q = params
            .get("q")
            .ok_or_else(|| bad("purge requires a q over Snapshot members (5.16.7.4)".into()))?;
        let ast = antares_ql::parse_q(q)?;
        // 5.16.7.4: the query is "restricted to members of the Snapshot
        // data type" (Tables 5.2.41-1/-2)
        const MEMBERS: [&str; 15] = [
            "id",
            "type",
            "snapshotQueries",
            "snapshotTemporalQueries",
            "snapshotLifetime",
            "snapshotPriority",
            "endpoint",
            "receiverInfo",
            "snapshotStatus",
            "snapshotQueriesDetails",
            "snapshotTemporalQueriesDetails",
            "createdAt",
            "modifiedAt",
            "expiresAt",
            "lastUsedAt",
        ];
        if let Some(alien) = ast
            .attribute_paths()
            .into_iter()
            .find(|a| !MEMBERS.contains(a))
        {
            return Err(bad(format!(
                "purge q is restricted to Snapshot members (5.16.7.4): {alien:?}"
            ))
            .into());
        }
        let ctx = st.loader.core();
        let victims: Vec<Value> = st
            .store
            .list(&tenant, Kind::Snapshot)
            .await?
            .into_iter()
            .filter(|meta| is_snapshot(meta) && crate::registry::csf_matches(&ast, meta, &ctx))
            .collect();
        // a Snapshot always carries its id
        for (id, meta) in victims
            .iter()
            .filter_map(|m| Some((m.get("id")?.as_str()?, m)))
        {
            snap_remove(&st, &tenant, id, meta).await;
        }
        Ok::<_, ApiError>(no_content(&tenant))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.16.2.4 / 5.16.3.4 / 5.16.4.4 / 5.16.5.4: every /snapshots/{id} method
/// opens the same way — the tenant it runs in, `local` (6.3.18) as the only
/// parameter this resource takes, and an id that must be a valid URI before
/// the store is touched at all.
fn open_snapshot(
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    id: &str,
) -> ApiResult<TenantId> {
    let tenant = tenant_from(headers)?;
    check_params(params, &["local"])?;
    antares_model::EntityId::new(id)
        .map_err(|_| bad(format!("snapshot id is not a valid URI: {id:?}")))?;
    Ok(tenant)
}

// ---------- 5.16.3/5.16.4/5.16.5: /snapshots/{id} (6.37) ----------

pub async fn retrieve_snapshot(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = open_snapshot(&params, &headers, &id)?;
        gate!(st, &tenant, &headers, "5.16.3", ids: &[&id]).await?;
        let accept = parse_accept(&headers)?;
        let meta = snap_get(&st, &tenant, &id)
            .await
            .ok_or_else(|| NgsiError::ResourceNotFound(format!("snapshot {id} not found")))?;
        let ctx = st.loader.core();
        Ok::<_, ApiError>(respond(
            StatusCode::OK,
            present(&meta),
            &ctx,
            accept,
            &tenant,
        ))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

pub async fn update_snapshot(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        let tenant = open_snapshot(&params, &headers, &id)?;
        gate!(st, &tenant, &headers, "5.16.4", ids: &[&id]).await?;
        let accept = parse_accept(&headers)?;
        // 5.16.4.4 binds this operation to 5.5.4, hence to the 6.3.5 rules
        let v = parse_body(&st, &headers, &body, BodyKind::Standard)
            .await?
            .value;
        let frag = validate(&v, Mode::Update)?;
        let mut meta = snap_get(&st, &tenant, &id)
            .await
            .ok_or_else(|| NgsiError::ResourceNotFound(format!("snapshot {id} not found")))?;
        // read before the merge below writes the new endpoint
        let moved = frag
            .get("endpoint")
            .is_some_and(|e| Some(e) != meta.get("endpoint"));
        // a Snapshot is an object (`snap_get` read its type)
        let o = meta
            .as_object_mut()
            .ok_or(NgsiError::InternalError("snapshot unreadable".into()))?;
        // clause 5.16.4, item 4 / 5.5.8 merge of the updatable members
        for k in [
            "snapshotLifetime",
            "snapshotPriority",
            "endpoint",
            "receiverInfo",
        ] {
            // `validate` refused a null for each of these, so a member
            // sent is a value to merge, never a removal
            if let Some(v) = frag.get(k) {
                let mut v = v.clone();
                if k == "receiverInfo" {
                    crate::pairs::unmask_pairs(k, &mut v, o.get(k), moved)?;
                }
                o.insert(k.into(), v);
            }
        }
        if frag.contains_key("snapshotLifetime") {
            // "it is possible to indirectly update expiresAt" (5.2.41)
            o.insert("expiresAt".into(), Value::String(expires_at(o)));
        }
        o.insert("modifiedAt".into(), Value::String(now_iso()));
        let meta = snap_put(&st, &tenant, &meta, UPDATE_MEMBERS)
            .await
            .ok_or_else(|| NgsiError::ResourceNotFound(format!("snapshot {id} not found")))?;
        // 5.16.6: notifications are also sent after any status update
        let (st2, meta2) = (st.clone(), meta.clone());
        crate::spawn(async move {
            send_notification(&st2, &meta2).await;
        });
        let ctx = st.loader.core();
        Ok::<_, ApiError>(respond(
            StatusCode::OK,
            present(&meta),
            &ctx,
            accept,
            &tenant,
        ))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

pub async fn delete_snapshot(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = open_snapshot(&params, &headers, &id)?;
        gate!(st, &tenant, &headers, "5.16.5", ids: &[&id]).await?;
        let meta = snap_get(&st, &tenant, &id)
            .await
            .ok_or_else(|| NgsiError::ResourceNotFound(format!("snapshot {id} not found")))?;
        snap_remove(&st, &tenant, &id, &meta).await;
        Ok::<_, ApiError>(no_content(&tenant))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

// ---------- 5.16.2 Clone Snapshot (POST /snapshots/{id}/clone, 6.38) ----------

pub async fn clone_snapshot(
    State(st): State<AppState>,
    Path(id): Path<String>,
    CleanParams(params): CleanParams,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let go = async {
        let tenant = open_snapshot(&params, &headers, &id)?;
        gate!(st, &tenant, &headers, "5.16.2", ids: &[&id]).await?;
        let src = snap_get(&st, &tenant, &id)
            .await
            .ok_or_else(|| NgsiError::ResourceNotFound(format!("snapshot {id} not found")))?;
        // 5.16.2.3 makes the clone body optional; a body that IS sent obeys
        // the 6.3.5 @context rules
        let v: Value = if body.is_empty() {
            json!({})
        } else {
            parse_body(&st, &headers, &body, BodyKind::Standard)
                .await?
                .value
        };
        let mut o = validate(&v, Mode::Clone)?;
        // the clone carries the source's (read-only) query lineage
        for k in ["snapshotQueries", "snapshotTemporalQueries"] {
            if let Some(qv) = src.get(k) {
                o.insert(k.into(), qv.clone());
            }
        }
        let owner = uuid::Uuid::new_v4().to_string();
        let (new_id, meta) = new_meta(o, &tenant, &headers, &owner, Some(&id))?;
        snap_insert(&st, &tenant, &new_id, &meta).await?;
        // a clone is a new snapshot: 5.5.15 resource pressure applies to it
        // exactly as it does to a create, so cloning cannot grow the
        // registry past the cap
        evict_over_cap(&st, &tenant, &new_id).await;
        let (st2, t2, sid, nid) = (st.clone(), tenant.clone(), id.clone(), new_id.clone());
        crate::spawn(async move {
            clone_fill(&st2, &t2, &sid, &nid, &owner).await;
        });
        Ok::<_, ApiError>(created(format!("/ngsi-ld/v1/snapshots/{new_id}"), &tenant))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}

/// 5.16.2.4 background copy: all Entity and Temporal data of the source.
/// A source still `preparing` is waited for (its copy is not complete
/// until its fill finishes), for as long as both snapshots exist; a source
/// that ends in `failure` or disappears makes the clone a `failure`. The
/// copy stops at the same ceiling a fill does (5.5.6). The wait renews the
/// clone's own lease, backs off from 50 ms to 5 s, and gives up as a
/// `failure` after [`CLONE_WAIT_MS`] (S4-22); a source read that fails is
/// read again at the next poll, never taken for a deleted source.
async fn clone_fill(st: &AppState, tenant: &TenantId, src_id: &str, new_id: &str, owner: &str) {
    let mut pause = CLONE_POLL_MS;
    let mut waited = 0u32;
    let src = loop {
        match snap_lookup(st, tenant, src_id).await {
            Ok(None) => {
                finish(st, tenant, new_id, owner, "failure", None).await;
                return;
            }
            Ok(Some(src))
                if src.get("snapshotStatus").and_then(Value::as_str) != Some("preparing") =>
            {
                break src;
            }
            Ok(Some(_)) => {}
            Err(e) => tracing::debug!("snapshot clone {new_id}: source read failed: {e}"),
        }
        if hold(st, tenant, new_id, owner).await != Hold::Held {
            return;
        }
        if waited >= CLONE_WAIT_MS {
            finish(st, tenant, new_id, owner, "failure", None).await;
            return;
        }
        crate::sleep_ms(pause).await;
        waited = waited.saturating_add(pause);
        pause = pause.saturating_mul(2).min(CLONE_POLL_MAX_MS);
    };
    // FILLS is never closed, so the acquire always hands out a permit
    let permit = FILLS.acquire().await.ok();
    if hold(st, tenant, new_id, owner).await != Hold::Held {
        return;
    }
    let keeper = Keeper::start(st, tenant, new_id, owner);
    let (Some(from), Some(to)) = (
        synth_tenant(&src),
        snap_get(st, tenant, new_id)
            .await
            .as_ref()
            .and_then(synth_tenant),
    ) else {
        drop((keeper, permit));
        finish(st, tenant, new_id, owner, "failure", None).await;
        return;
    };
    let failed = src.get("snapshotStatus").and_then(Value::as_str) == Some("failure")
        || copy_snapshot_data(st, &from, &to, fill_cap(st))
            .await
            .is_err();
    if fill_cancelled(st, tenant, new_id, &to, owner).await {
        return;
    }
    drop((keeper, permit));
    finish(
        st,
        tenant,
        new_id,
        owner,
        if failed { "failure" } else { "success" },
        None,
    )
    .await;
}

/// How soon a clone first looks again whether its source finished preparing.
const CLONE_POLL_MS: u32 = 50;

/// The longest pause between two of those looks.
const CLONE_POLL_MAX_MS: u32 = 5_000;

/// How long a clone waits for its source to finish preparing: a source
/// whose fill died is resumed by the sweep within a lease, so a wait past
/// three leases is a source that will not finish.
const CLONE_WAIT_MS: u32 = 1_800_000;

/// Copy every Entity (one `list_page` at a time) and every Temporal
/// Evolution of `from` into `to`, at most `cap` documents of each. An
/// Evolution REPLACES what `to` holds: copying an Entity fires the 5.6.11
/// auto-record hook, which leaves a stub Evolution of one instance.
async fn copy_snapshot_data(
    st: &AppState,
    from: &TenantId,
    to: &TenantId,
    cap: usize,
) -> Result<(), NgsiError> {
    let mut n = 0usize;
    let mut after: Option<String> = None;
    loop {
        let page = st
            .store
            .list_page(from, Kind::Entity, after.as_deref(), st.max_limit)
            .await?;
        let Some(last) = page
            .last()
            .and_then(|d| d.get("id"))
            .and_then(Value::as_str)
        else {
            break;
        };
        after = Some(last.to_owned());
        n = n.saturating_add(page.len());
        if n > cap {
            return Err(too_many(n, cap));
        }
        let full = page.len() >= st.max_limit;
        // a stored Entity always carries its id
        for (id, doc) in page
            .iter()
            .filter_map(|d| Some((d.get("id")?.as_str()?, d)))
        {
            st.store.create(to, Kind::Entity, id, doc.clone()).await?;
        }
        if !full {
            break;
        }
    }
    let docs = st.temporal.list(from).await?;
    if docs.len() > cap {
        return Err(too_many(docs.len(), cap));
    }
    // a stored Evolution always carries its Entity's id
    for (id, doc) in docs
        .iter()
        .filter_map(|d| Some((d.get("id")?.as_str()?, d)))
    {
        st.temporal.upsert(to, id, doc.clone()).await?;
    }
    Ok(())
}

// ---------- 6.3.22: NGSILD-Snapshot scoping middleware ----------

/// 6.3.22 / 5.5.15: resolve the NGSILD-Snapshot header to the snapshot's
/// synthetic tenant, so every Core/Temporal handler serves the frozen copy;
/// the header is echoed on the response (and the synthetic tenant never
/// leaks into NGSILD-Tenant).
pub async fn snapshot_layer(
    State(st): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // 6.3.22 gives the header one value, and it decides whether the request
    // is answered from a frozen copy or from live data: an ambiguous one
    // would serve the request against a dataset nobody named.
    let sid = match single_header(req.headers(), "NGSILD-Snapshot") {
        Ok(None) => return next.run(req).await,
        Ok(Some(sid)) => sid,
        Err(e) => return e.into_response(),
    };
    // 6.3.22: "If the HTTP header NGSILD-Snapshot is present in the HTTP
    // request, it shall also be present in HTTP response" — every exit,
    // the unscoped and the error ones included, leaves through here
    let mut resp = scoped(&st, &sid, req, next).await;
    // `sid` was read out of a header value, so it is one again
    resp.headers_mut().remove("NGSILD-Snapshot");
    resp.headers_mut().extend(
        sid.parse()
            .ok()
            .map(|v| (axum::http::HeaderName::from_static("ngsild-snapshot"), v)),
    );
    resp
}

async fn scoped(
    st: &AppState,
    sid: &str,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // the Snapshot API's own resources (6.36-6.38) are never
    // snapshot-scoped — that is the /snapshots RESOURCE, not any path
    // merely containing the word (an Attribute may be named "snapshots")
    if req
        .uri()
        .path()
        .trim_start_matches(API_ROOT)
        .starts_with("/snapshots")
    {
        return next.run(req).await;
    }
    let tenant = match tenant_from(req.headers()) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let Some(meta) = snap_get(st, &tenant, sid).await else {
        return ApiError::from(NgsiError::ResourceNotFound(format!(
            "snapshot {sid} not found"
        )))
        .into_response();
    };
    let Some(synth) = synth_tenant(&meta) else {
        return ApiError::from(NgsiError::InternalError("snapshot without tenant".into()))
            .into_response();
    };
    snap_touch(st, &tenant, sid).await;
    // `extend` appends: the caller's Tenant goes first
    req.headers_mut().remove("NGSILD-Tenant");
    req.headers_mut().extend(tenant_header(synth.as_str()));
    let mut resp = next.run(req).await;
    // restore the caller's tenant view (the synthetic one is internal)
    resp.headers_mut().remove("NGSILD-Tenant");
    if tenant.as_str() != "default" {
        resp.headers_mut().extend(tenant_header(tenant.as_str()));
    }
    resp
}

/// 5.5.6: the fill copies its results into a second (isolated) tenant, so an
/// unbounded result set doubles the stored data set in one request. The
/// implementation threshold — "up to each implementation" — is a hundred
/// full pages per snapshot; beyond it the query reports TooManyResults.
/// The NGSILD-Tenant header naming `tenant`. A Tenant name is
/// `[A-Za-z0-9_-]{1,128}` (`TenantId`), so it always is a header value and
/// the pair is always there.
fn tenant_header(tenant: &str) -> Option<(axum::http::HeaderName, axum::http::HeaderValue)> {
    let v = axum::http::HeaderValue::from_str(tenant).ok()?;
    Some((axum::http::HeaderName::from_static("ngsild-tenant"), v))
}

fn fill_cap(st: &AppState) -> usize {
    st.max_limit.saturating_mul(100)
}

/// 5.16.1.4 + 5.5.15: the copied data is reachable only through its snapshot
/// document, so a fill whose snapshot was deleted (or evicted under resource
/// pressure) must stop and free what it already copied. Renews the fill's
/// lease on the way; a fill that lost the lease stops and leaves the copy to
/// its new holder, and a store error never reads as a deletion (S4-22).
async fn fill_cancelled(
    st: &AppState,
    tenant: &TenantId,
    id: &str,
    synth: &TenantId,
    owner: &str,
) -> bool {
    match hold(st, tenant, id, owner).await {
        Hold::Held => false,
        Hold::Lost => true,
        Hold::Gone => {
            purge_synth(st, synth).await;
            true
        }
    }
}

/// 5.5.6: "When a query operation is producing so many results that can
/// potentially exhaust client or server resources … implementations shall
/// raise an error of type TooManyResults" — reported per query in the
/// 5.2.42 ExecutionResultDetails of the snapshot.
fn too_many(got: usize, budget: usize) -> NgsiError {
    NgsiError::TooManyResults(format!(
        "snapshot query yields {got} results, {budget} left of the snapshot ceiling"
    ))
}

#[cfg(test)]
mod clause_5_5_6 {
    use super::*;

    /// 5.5.6 InternalError: the client-visible `detail` — served in
    /// snapshotQueriesDetails.problemDetails and copied into the 5.3.4
    /// SnapshotNotification body — must be generic. The Debug/IO text of
    /// the underlying failure belongs in the server log only.
    #[test]
    fn snapshot_fill_error_detail_is_generic() {
        let inner = ApiError::Bare(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let pd = crate::negotiate::problem_value(&opaque("query execution", &inner));
        let detail = pd["detail"].as_str().unwrap_or_default();
        assert_eq!(detail, "query execution failed");
        assert!(
            !detail.contains("Bare") && !detail.contains("Unsupported"),
            "internal error text leaked into the client-visible detail: {detail}"
        );
    }
}

#[cfg(test)]
mod clause_5_16 {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn state() -> AppState {
        crate::wired_state("antares-snapshots").await
    }

    /// 5.16 with RFC 9110 §9.2.1: reading an expired Snapshot must not
    /// write. The meta row survives the read; the sweep is what frees it,
    /// its reverse-index entry and the synthetic tenant's data.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_expired_snapshot_is_refused_by_a_read_that_writes_nothing() {
        let st = state().await;
        let t = TenantId::default();
        let id = "urn:ngsi-ld:Snapshot:stale";
        let synth = "snap-default-stale";
        st.store
            .create(
                &t,
                Kind::Snapshot,
                id,
                json!({"id": id, "type": "Snapshot", "__tenant": synth,
                       "expiresAt": "2000-01-01T00:00:00Z"}),
            )
            .await
            .expect("seed");
        let idx = TenantId::new_internal("snap-index").expect("index tenant");
        st.store
            .create(&idx, Kind::Snapshot, synth, json!({"id": synth}))
            .await
            .expect("seed index");

        assert!(snap_get(&st, &t, id).await.is_none(), "5.16: not served");
        assert!(
            st.store
                .get(&t, Kind::Snapshot, id)
                .await
                .expect("store")
                .is_some(),
            "the read deleted the meta: a GET must be safe (RFC 9110 9.2.1)"
        );

        assert_eq!(sweep_expired_snapshots(&st, &t).await, 1, "the sweep reaps");
        assert!(
            st.store
                .get(&t, Kind::Snapshot, id)
                .await
                .expect("store")
                .is_none(),
            "the sweep left the meta behind"
        );
        // The teardown runs behind the sweep: the copy is dropped first and
        // the entry that names it last, so a broker that stops in the middle
        // leaves a pointer to data that is still there.
        for _ in 0..100 {
            if st
                .store
                .get(&idx, Kind::Snapshot, synth)
                .await
                .expect("store")
                .is_none()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the sweep left the reverse-index entry behind");
    }

    /// The sweep takes the expiry and nothing else: a live Snapshot and the
    /// reverse-index markers that share its storage both survive.
    #[tokio::test]
    async fn the_sweep_keeps_a_live_snapshot_and_the_index_markers() {
        let st = state().await;
        let t = TenantId::default();
        let live = "urn:ngsi-ld:Snapshot:live";
        st.store
            .create(
                &t,
                Kind::Snapshot,
                live,
                json!({"id": live, "type": "Snapshot",
                       "expiresAt": "2999-01-01T00:00:00Z"}),
            )
            .await
            .expect("seed");
        // a marker doc: same Kind, not a Snapshot, and long past any expiry
        st.store
            .create(
                &t,
                Kind::Snapshot,
                "snap-default-marker",
                json!({"id": "snap-default-marker", "expiresAt": "2000-01-01T00:00:00Z"}),
            )
            .await
            .expect("seed marker");

        assert_eq!(sweep_expired_snapshots(&st, &t).await, 0);
        assert!(snap_get(&st, &t, live).await.is_some(), "live one survives");
        assert!(
            st.store
                .get(&t, Kind::Snapshot, "snap-default-marker")
                .await
                .expect("store")
                .is_some(),
            "the sweep took a reverse-index marker for a Snapshot"
        );
    }

    /// One request through the full router — the 6.3.22 middleware included.
    async fn send(
        st: &AppState,
        method: &str,
        path: &str,
        body: Option<(&str, String)>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Value) {
        let mut b = Request::builder().method(method).uri(path);
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let req = match body {
            Some((ct, payload)) => b
                .header("Content-Type", ct)
                .header("Content-Length", payload.len())
                .body(Body::from(payload)),
            None => b.body(Body::empty()),
        }
        .expect("request");
        let res = crate::router(st.clone())
            .oneshot(req)
            .await
            .expect("response");
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, headers, body)
    }

    async fn post_json(st: &AppState, path: &str, doc: &Value) -> (StatusCode, HeaderMap, Value) {
        send(
            st,
            "POST",
            path,
            Some(("application/json", doc.to_string())),
            &[],
        )
        .await
    }

    fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
        h.get(name).and_then(|v| v.to_str().ok())
    }

    /// Create a snapshot over every Vehicle and wait for the background fill.
    async fn snapshot_over_vehicles(st: &AppState) -> String {
        let doc = json!({"type": "Snapshot",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]});
        let (status, headers, body) = post_json(st, "/ngsi-ld/v1/snapshots", &doc).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let loc = header(&headers, "Location").expect("Location");
        let id = loc.rsplit('/').next().expect("snapshot id").to_owned();
        settled(st, &id).await;
        id
    }

    async fn stored(st: &AppState, id: &str) -> Value {
        st.store
            .get(&TenantId::default(), Kind::Snapshot, id)
            .await
            .ok()
            .flatten()
            .expect("snapshot document")
    }

    /// Hand Snapshot `id`'s fill lease to `owner`, as a fill holds it.
    async fn lease_to(st: &AppState, id: &str, owner: &str) {
        st.store
            .mutate(&TenantId::default(), Kind::Snapshot, id, |d| {
                d[LEASE_MEMBER] = lease(owner);
                Ok::<_, std::convert::Infallible>(())
            })
            .await
            .expect("lease");
    }

    /// S4-22: a Snapshot left `preparing` by a fill that died (a restart, a
    /// killed replica) is claimed by the sweep once its lease ran out, its
    /// partial copy dropped and its fill run again to the end; a held one
    /// and a finished one are left alone.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_fill_whose_holder_died_is_resumed_by_the_sweep() {
        let st = state().await;
        let t = TenantId::default();
        let ent = json!({"id": "urn:ngsi-ld:Vehicle:resume1", "type": "Vehicle",
            "speed": {"type": "Property", "value": 1}});
        let (status, _, b) = post_json(&st, "/ngsi-ld/v1/entities", &ent).await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        let id = "urn:ngsi-ld:Snapshot:resume";
        let synth = TenantId::new_internal("snap-resume-test").expect("tenant");
        let meta = json!({"id": id, "type": "Snapshot", "__tenant": synth.as_str(),
            "snapshotStatus": "preparing", "createdAt": "2000-01-01T00:00:00Z",
            "modifiedAt": "2000-01-01T00:00:00Z", "expiresAt": "2999-01-01T00:00:00Z",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}],
            LEASE_MEMBER: {"owner": "a-dead-pod", "until": "2000-01-01T00:00:10Z"}});
        assert!(fill_orphaned(&meta), "a lease that ran out holds nothing");
        let mut held = meta.clone();
        held[LEASE_MEMBER] = lease("a-live-pod");
        assert!(!fill_orphaned(&held), "a live lease holds");
        let mut unleased = meta.clone();
        unleased.as_object_mut().map(|o| o.remove(LEASE_MEMBER));
        assert!(
            fill_orphaned(&unleased),
            "written before leases, older than one"
        );
        let mut done = meta.clone();
        done["snapshotStatus"] = json!("success");
        assert!(!fill_orphaned(&done), "a finished Snapshot has no fill");
        st.store
            .create(&t, Kind::Snapshot, id, meta)
            .await
            .expect("seed");
        st.store
            .create(
                &synth,
                Kind::Entity,
                "urn:ngsi-ld:Vehicle:partial",
                json!({"id": "urn:ngsi-ld:Vehicle:partial"}),
            )
            .await
            .expect("partial copy");
        assert_eq!(sweep_expired_snapshots(&st, &t).await, 0, "nothing expired");
        let after = settled(&st, id).await;
        assert_eq!(after["snapshotStatus"], "success", "{after}");
        assert!(
            after.get(LEASE_MEMBER).is_none(),
            "the lease is released: {after}"
        );
        let copied: Vec<String> = st
            .store
            .list(&synth, Kind::Entity)
            .await
            .expect("copy")
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();
        assert!(
            copied.contains(&"urn:ngsi-ld:Vehicle:resume1".to_owned()),
            "{copied:?}"
        );
        assert!(
            !copied.contains(&"urn:ngsi-ld:Vehicle:partial".to_owned()),
            "{copied:?}"
        );
    }

    /// S4-22: a fill whose Snapshot another fill claimed stops without
    /// touching the copy, and its outcome does not overwrite the holder's.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_fill_that_lost_its_lease_writes_nothing() {
        let st = state().await;
        let t = TenantId::default();
        let sid = snapshot_over_vehicles(&st).await;
        let synth = synth_tenant(&stored(&st, &sid).await).expect("__tenant");
        lease_to(&st, &sid, "the-new-holder").await;
        assert!(
            fill_cancelled(&st, &t, &sid, &synth, "the-old-fill").await,
            "it stops"
        );
        finish(&st, &t, &sid, "the-old-fill", "failure", None).await;
        let after = stored(&st, &sid).await;
        assert_ne!(after["snapshotStatus"], "failure", "{after}");
        assert!(leased_to(&after, "the-new-holder"), "{after}");
    }

    /// S4-22: a clone renews its own lease while its source prepares, backs
    /// off, and ends in `failure` once it waited past its bound.
    #[tokio::test(start_paused = true)]
    async fn a_clone_stops_waiting_for_a_source_that_never_finishes() {
        let st = AppState::new("antares-snapshots".into());
        let t = TenantId::default();
        let src = "urn:ngsi-ld:Snapshot:stuck";
        let clone = "urn:ngsi-ld:Snapshot:waiting";
        for (id, owner) in [(src, "a-source-fill"), (clone, "the-clone")] {
            st.store
                .create(
                    &t,
                    Kind::Snapshot,
                    id,
                    json!({"id": id, "type": "Snapshot",
                    "__tenant": format!("snap-{}", id.len()), "snapshotStatus": "preparing",
                    "createdAt": now_iso(), "expiresAt": "2999-01-01T00:00:00Z",
                    LEASE_MEMBER: lease(owner)}),
                )
                .await
                .expect("seed");
        }
        clone_fill(&st, &t, src, clone, "the-clone").await;
        let after = st
            .store
            .get(&t, Kind::Snapshot, clone)
            .await
            .expect("get")
            .expect("clone");
        assert_eq!(after["snapshotStatus"], "failure", "{after}");
    }

    /// 6.3.22: the Snapshot API's own resources are never snapshot-scoped —
    /// but that exemption is the /snapshots RESOURCE, not any path in which
    /// the word occurs. An Entity attribute named "snapshots" stays scoped
    /// to the frozen copy, so the live entity is untouched.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_6_3_22_scope_guard_matches_only_the_snapshot_resource() {
        let st = state().await;
        let ent = json!({"id": "urn:ngsi-ld:Vehicle:g1", "type": "Vehicle",
            "snapshots": {"type": "Property", "value": 3}});
        let (status, _, b) = post_json(&st, "/ngsi-ld/v1/entities", &ent).await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        let sid = snapshot_over_vehicles(&st).await;

        let (status, headers, b) = send(
            &st,
            "DELETE",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:g1/attrs/snapshots",
            None,
            &[("NGSILD-Snapshot", sid.as_str())],
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{b}");
        assert_eq!(header(&headers, "NGSILD-Snapshot"), Some(sid.as_str()));

        let (status, _, live) = send(
            &st,
            "GET",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:g1",
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{live}");
        assert!(
            live.get("snapshots").is_some(),
            "a snapshot-scoped request deleted the LIVE attribute: {live}"
        );
        let (_, _, frozen) = send(
            &st,
            "GET",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:g1",
            None,
            &[("NGSILD-Snapshot", sid.as_str())],
        )
        .await;
        assert!(
            frozen.get("snapshots").is_none(),
            "the frozen copy still carries the deleted attribute: {frozen}"
        );
    }

    /// 6.3.22: "If the HTTP header NGSILD-Snapshot is present in the HTTP
    /// request, it shall also be present in HTTP response" — on the
    /// unscoped Snapshot resources and on the error exits as well.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_6_3_22_header_is_echoed_on_every_exit() {
        let st = state().await;
        let sid = snapshot_over_vehicles(&st).await;
        let (status, headers, b) = send(
            &st,
            "GET",
            &format!("/ngsi-ld/v1/snapshots/{sid}"),
            None,
            &[("NGSILD-Snapshot", sid.as_str())],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{b}");
        assert_eq!(header(&headers, "NGSILD-Snapshot"), Some(sid.as_str()));

        let unknown = "urn:ngsi-ld:Snapshot:nope";
        let (status, headers, b) = send(
            &st,
            "GET",
            "/ngsi-ld/v1/entities?type=Vehicle",
            None,
            &[("NGSILD-Snapshot", unknown)],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{b}");
        assert_eq!(header(&headers, "NGSILD-Snapshot"), Some(unknown));
        assert!(
            !b.to_string().contains("snap-"),
            "the synthetic tenant leaked to the client: {b}"
        );
    }

    /// 5.16.1.4: "If the NGSI-LD endpoint already knows about this Snapshot,
    /// as there is an existing Snapshot whose id (URI) is equivalent, an
    /// error of type AlreadyExists shall be raised" — and the rejected
    /// create must not have replaced the existing snapshot.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_1_4_duplicate_id_is_already_exists() {
        let st = state().await;
        let mut doc = json!({"id": "urn:ngsi-ld:Snapshot:dup", "type": "Snapshot",
            "snapshotPriority": 3,
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]});
        let (status, _, b) = post_json(&st, "/ngsi-ld/v1/snapshots", &doc).await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        doc["snapshotPriority"] = json!(9);
        let (status, _, b) = post_json(&st, "/ngsi-ld/v1/snapshots", &doc).await;
        assert_eq!(status, StatusCode::CONFLICT, "{b}");
        assert!(
            b["type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("AlreadyExists"),
            "{b}"
        );
        let (_, _, got) = send(
            &st,
            "GET",
            "/ngsi-ld/v1/snapshots/urn:ngsi-ld:Snapshot:dup",
            None,
            &[],
        )
        .await;
        assert_eq!(
            got["snapshotPriority"], 3,
            "the rejected create overwrote the existing snapshot: {got}"
        );
        assert!(
            got.get("__tenant").is_none(),
            "internal member served to the client: {got}"
        );
    }

    /// 6.3.5 (5.16.4.4 → 5.5.4): snapshot bodies obey the @context rules —
    /// application/json with a body @context is BadRequestData,
    /// application/ld+json without one is BadRequestData, any other media
    /// type is a bare 415, and @context is never stored in the snapshot.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_6_3_5_snapshot_bodies_follow_the_context_rules() {
        let st = state().await;
        let core = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld";
        let plain = json!({"type": "Snapshot",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]});
        let mut with_ctx = plain.clone();
        with_ctx["@context"] = json!(core);

        for (ct, doc) in [
            ("application/json", &with_ctx),
            ("application/ld+json", &plain),
        ] {
            let (status, _, b) = send(
                &st,
                "POST",
                "/ngsi-ld/v1/snapshots",
                Some((ct, doc.to_string())),
                &[],
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{ct}: {b}");
        }
        let (status, _, _) = send(
            &st,
            "POST",
            "/ngsi-ld/v1/snapshots",
            Some(("text/plain", plain.to_string())),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let (status, headers, b) = send(
            &st,
            "POST",
            "/ngsi-ld/v1/snapshots",
            Some(("application/ld+json", with_ctx.to_string())),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        let loc = header(&headers, "Location").expect("Location").to_owned();
        let (_, _, got) = send(&st, "GET", &loc, None, &[]).await;
        assert!(
            got.get("@context").is_none(),
            "@context stored as a snapshot member: {got}"
        );
        let patch = json!({"snapshotPriority": 7, "@context": core});
        let (status, _, b) = send(
            &st,
            "PATCH",
            &loc,
            Some(("application/json", patch.to_string())),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
    }

    /// 5.5.6: "When a query operation is producing so many results that can
    /// potentially exhaust client or server resources … implementations
    /// shall raise an error of type TooManyResults." A fill copies its
    /// results, so the ceiling is enforced before anything is copied.
    /// 5.5.6 and T-3773: a temporal fill page larger than the broker buffers
    /// for one request is TooManyResults, not held whole in memory.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_5_6_fill_stops_at_the_answer_byte_bound() {
        let mut st = state().await;
        let ent = json!({"id": "urn:ngsi-ld:Vehicle:big", "type": "Vehicle",
            "speed": {"type": "Property", "value": 80}});
        let (status, _, b) = post_json(&st, "/ngsi-ld/v1/entities", &ent).await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        st.max_answer_bytes = 16;
        let doc = json!({"type": "Snapshot",
            "snapshotTemporalQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}],
                "temporalQ": {"timerel": "after", "timeAt": "2000-01-01T00:00:00Z",
                    "timeproperty": "createdAt"}}]});
        let (status, headers, body) = post_json(&st, "/ngsi-ld/v1/snapshots", &doc).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let loc = header(&headers, "Location").expect("Location");
        let ready = settled(&st, loc.rsplit('/').next().expect("snapshot id")).await;
        assert_eq!(ready["snapshotStatus"], "failure", "{ready}");
        let detail = ready["snapshotTemporalQueriesDetails"][0]["problemDetails"].clone();
        assert!(
            detail["type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("TooManyResults"),
            "{ready}"
        );
        assert_eq!(detail["detail"], crate::bounds::ANSWER_TOO_LARGE, "{ready}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_5_6_fill_stops_at_the_result_ceiling() {
        let mut st = state().await;
        st.max_limit = 1;
        st.default_limit = 1;
        for i in 0..=fill_cap(&st) {
            let ent = json!({"id": format!("urn:ngsi-ld:Vehicle:c{i}"), "type": "Vehicle"});
            let (status, _, b) = post_json(&st, "/ngsi-ld/v1/entities", &ent).await;
            assert_eq!(status, StatusCode::CREATED, "{b}");
        }
        let sid = snapshot_over_vehicles(&st).await;
        let ready = stored(&st, &sid).await;
        assert_eq!(ready["snapshotStatus"], "failure", "{ready}");
        let detail = &ready["snapshotQueriesDetails"][0];
        assert_eq!(detail["resultStatus"], "failure", "{ready}");
        assert!(
            detail["problemDetails"]["type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("TooManyResults"),
            "{detail}"
        );
        let (status, _, list) = send(
            &st,
            "GET",
            "/ngsi-ld/v1/entities?type=Vehicle&limit=1",
            None,
            &[("NGSILD-Snapshot", sid.as_str())],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert_eq!(
            list,
            json!([]),
            "the oversized result set was copied into the snapshot anyway"
        );
    }

    /// 5.16.1.4 + 5.5.15: the filled copy is reachable only through its
    /// snapshot document, so a fill whose snapshot has been deleted (or
    /// evicted) stops and frees what it already copied.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_1_4_fill_stops_and_reaps_when_its_snapshot_is_gone() {
        let st = state().await;
        let tenant = TenantId::default();
        let orphan = TenantId::new_internal("snap-reap-test").expect("tenant");
        let _ = st
            .store
            .create(
                &orphan,
                Kind::Entity,
                "urn:ngsi-ld:Vehicle:r1",
                json!({"id": "urn:ngsi-ld:Vehicle:r1"}),
            )
            .await;
        assert!(
            fill_cancelled(&st, &tenant, "urn:ngsi-ld:Snapshot:gone", &orphan, "fill").await,
            "a fill kept running after its snapshot was deleted"
        );
        assert!(
            st.store
                .list(&orphan, Kind::Entity)
                .await
                .unwrap_or_default()
                .is_empty(),
            "the orphaned copy outlived its snapshot"
        );

        let sid = snapshot_over_vehicles(&st).await;
        let synth = synth_tenant(&stored(&st, &sid).await).expect("__tenant");
        let _ = st
            .store
            .create(
                &synth,
                Kind::Entity,
                "urn:ngsi-ld:Vehicle:r2",
                json!({"id": "urn:ngsi-ld:Vehicle:r2"}),
            )
            .await;
        lease_to(&st, &sid, "fill").await;
        assert!(
            !fill_cancelled(&st, &tenant, &sid, &synth, "fill").await,
            "a live snapshot must not cancel its own fill"
        );
        assert_eq!(
            st.store
                .list(&synth, Kind::Entity)
                .await
                .unwrap_or_default()
                .len(),
            1,
            "a live snapshot's copy was reaped"
        );
    }

    /// 5.16.5.4 + 5.5.15: deleting a snapshot frees the whole isolated copy,
    /// including the subscriptions created through the 6.3.22 header — no
    /// client can reach them afterwards, so nothing may keep firing.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_5_delete_frees_the_whole_snapshot_copy() {
        let st = state().await;
        let sid = snapshot_over_vehicles(&st).await;
        let synth = synth_tenant(&stored(&st, &sid).await).expect("__tenant");
        let sub = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
            "notification": {"endpoint": {"uri": "http://127.0.0.1:9/none"}}});
        let (status, _, b) = send(
            &st,
            "POST",
            "/ngsi-ld/v1/subscriptions",
            Some(("application/json", sub.to_string())),
            &[("NGSILD-Snapshot", sid.as_str())],
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        assert!(
            !st.store
                .list(&synth, Kind::Subscription)
                .await
                .unwrap_or_default()
                .is_empty(),
            "the scoped subscription did not land in the snapshot copy"
        );

        let (status, _, _) = send(
            &st,
            "DELETE",
            &format!("/ngsi-ld/v1/snapshots/{sid}"),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        for _ in 0..100 {
            let left = st
                .store
                .list(&synth, Kind::Subscription)
                .await
                .unwrap_or_default()
                .len();
            if left == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("a snapshot-scoped subscription outlived its snapshot");
    }

    /// 5.2.41: each malformed member is a BadRequestData naming it.
    #[test]
    fn clause_5_2_41_each_malformed_member_is_named() {
        // every refusal here is BadRequestData, whose Display is its detail
        let refused = |body: Value| {
            let e = validate(&body, Mode::Create).expect_err("refused");
            assert!(matches!(e, NgsiError::BadRequestData(_)), "{e:?}");
            e.to_string()
        };
        let q = json!({"type": "Query"});
        assert_eq!(refused(json!(5)), "snapshot must be a JSON object");
        assert_eq!(
            refused(json!({"type": "Snapshot", "snapshotQueries": []})),
            "snapshotQueries must be a non-empty array of Query (5.2.41)"
        );
        assert_eq!(
            refused(json!({"type": "Snapshot", "snapshotQueries": [5]})),
            "snapshotQueries entries must be Query objects (5.2.23)"
        );
        assert_eq!(
            refused(json!({"type": "Snapshot", "snapshotQueries": [{"type": "Other"}]})),
            "snapshotQueries entries must have type Query (5.2.23)"
        );
        assert_eq!(
            refused(json!({"type": "Snapshot", "snapshotQueries": [q], "endpoint": 5})),
            "endpoint must be a URI string (5.2.41)"
        );
        let mut o = Map::new();
        o.insert("snapshotLifetime".into(), json!("PT1H"));
        let exp = crate::state::instant(&expires_at(&o)).expect("rfc3339");
        let left = exp.duration_since(jiff::Timestamp::now()).as_secs();
        assert!((3_500..=3_600).contains(&left), "{left}");
    }

    /// 5.16.1.4: a client id that is no URI is refused before anything is
    /// stored.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_1_4_an_id_that_is_no_uri_is_refused() {
        let st = state().await;
        let doc = json!({"type": "Snapshot", "id": "not a uri",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]});
        let (status, _, body) = post_json(&st, "/ngsi-ld/v1/snapshots", &doc).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            body["detail"],
            "snapshot id is not a valid URI: \"not a uri\""
        );
        assert!(st
            .store
            .list(&TenantId::default(), Kind::Snapshot)
            .await
            .expect("store")
            .is_empty());
    }

    /// 5.16.4.4 with 5.2.15: a masked receiverInfo value keeps the stored
    /// credential on the same endpoint and is refused once the endpoint
    /// moves; clone and update of an unknown Snapshot are 404.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_4_4_a_masked_credential_does_not_move() {
        let st = state().await;
        let sid = snapshot_over_vehicles(&st).await;
        let path = format!("/ngsi-ld/v1/snapshots/{sid}");
        let patch = |doc: Value| {
            let st = st.clone();
            let path = path.clone();
            async move {
                send(
                    &st,
                    "PATCH",
                    &path,
                    Some(("application/json", doc.to_string())),
                    &[],
                )
                .await
            }
        };
        let (status, _, body) = patch(json!({"endpoint": "http://127.0.0.1:9/a",
            "receiverInfo": [{"key": "X-Key", "value": "s3cret"}]}))
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["receiverInfo"][0]["value"], crate::pairs::MASKED);
        let (status, _, body) =
            patch(json!({"receiverInfo": [{"key": "X-Key", "value": crate::pairs::MASKED}]})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            stored(&st, &sid).await["receiverInfo"][0]["value"],
            "s3cret"
        );
        let (status, _, body) = patch(json!({"endpoint": "http://127.0.0.1:9/b",
            "receiverInfo": [{"key": "X-Key", "value": crate::pairs::MASKED}]}))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(stored(&st, &sid).await["endpoint"], "http://127.0.0.1:9/a");
        let gone = "/ngsi-ld/v1/snapshots/urn:ngsi-ld:Snapshot:none";
        let (status, _, _) = send(
            &st,
            "PATCH",
            gone,
            Some((
                "application/json",
                json!({"snapshotPriority": 2}).to_string(),
            )),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _, body) = send(
            &st,
            "POST",
            &format!("{gone}/clone"),
            Some(("application/json", String::new())),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(
            body["detail"],
            "snapshot urn:ngsi-ld:Snapshot:none not found"
        );
    }

    /// 5.16.6: the SnapshotNotification carries each receiverInfo pair as a
    /// header.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_6_receiver_info_rides_as_headers() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Option<String>, Value)>(1);
        let app = axum::Router::new().route(
            "/n",
            axum::routing::post(move |h: HeaderMap, b: axum::Json<Value>| {
                let tx = tx.clone();
                async move {
                    let key = h
                        .get("x-key")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    let _ = tx.send((key, b.0)).await;
                    StatusCode::NO_CONTENT
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(std::future::IntoFuture::into_future(axum::serve(
            listener, app,
        )));
        let st = state().await;
        let meta = json!({"id": "urn:ngsi-ld:Snapshot:n", "snapshotStatus": "success",
            "endpoint": format!("http://{addr}/n"),
            "receiverInfo": [{"key": "X-Key", "value": "s3cret"}, {"key": "X-None"}]});
        send_notification(&st, &meta).await;
        let (key, body) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("notified in time")
            .expect("one notification");
        assert_eq!(key.as_deref(), Some("s3cret"));
        assert_eq!(body["snapshotId"], "urn:ngsi-ld:Snapshot:n");
        assert_eq!(body["type"], "SnapshotNotification");
    }

    /// S4-22 seams called directly: a fill of a Snapshot that is gone or
    /// that names no copy does nothing; a claim that is not an orphan is
    /// skipped; a resumed clone copies its source again; an insert without
    /// a copy writes no index entry; a removal without one spawns nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn s4_22_fill_seams_stop_on_what_they_cannot_fill() {
        let st = state().await;
        let t = TenantId::default();
        fill_snapshot(&st, &t, "urn:ngsi-ld:Snapshot:none", "o").await;
        assert!(st
            .store
            .list(&t, Kind::Snapshot)
            .await
            .expect("list")
            .is_empty());

        let bare = "urn:ngsi-ld:Snapshot:bare";
        let meta = json!({"id": bare, "type": "Snapshot", "snapshotStatus": "preparing",
            "expiresAt": "2999-01-01T00:00:00Z", LEASE_MEMBER: lease("o")});
        snap_insert(&st, &t, bare, &meta).await.expect("insert");
        assert!(matches!(
            snap_insert(&st, &t, bare, &meta).await,
            Err(NgsiError::AlreadyExists(m)) if m == format!("snapshot {bare} already exists")
        ));
        fill_snapshot(&st, &t, bare, "o").await;
        assert_eq!(
            stored(&st, bare).await["snapshotStatus"],
            "preparing",
            "no copy, no fill"
        );
        snap_remove(&st, &t, bare, &meta).await;
        assert!(snap_get(&st, &t, bare).await.is_none());

        // a live lease: not an orphan, the claim is skipped
        let held = "urn:ngsi-ld:Snapshot:held";
        seeded_snapshot(&st, held, "preparing").await;
        lease_to(&st, held, "alive").await;
        resume_orphans(&st, &t, vec![held.to_owned()]).await;
        assert!(leased_to(&stored(&st, held).await, "alive"));

        // an orphaned clone of a finished source is copied again
        let src = "urn:ngsi-ld:Snapshot:src";
        let from = seeded_snapshot(&st, src, "success").await;
        vehicle_in(&st, &from, 1).await;
        let clone = "urn:ngsi-ld:Snapshot:orphan";
        seeded_snapshot(&st, clone, "preparing").await;
        st.store
            .mutate(&t, Kind::Snapshot, clone, |d| {
                d[CLONE_OF_MEMBER] = json!(src);
                d[LEASE_MEMBER] = json!({"owner": "dead", "until": "2000-01-01T00:00:00Z"});
                Ok::<_, std::convert::Infallible>(())
            })
            .await
            .expect("orphan");
        resume_orphans(&st, &t, vec![clone.to_owned()]).await;
        let after = settled(&st, clone).await;
        assert_eq!(after["snapshotStatus"], "success", "{after}");
        let to = synth_tenant(&after).expect("__tenant");
        assert!(st
            .store
            .get(&to, Kind::Entity, "urn:ngsi-ld:Vehicle:c1")
            .await
            .expect("store")
            .is_some());
    }

    /// 5.16.2.4 seams: a clone whose source is gone, whose own lease is
    /// another's, or which names no copy of its own ends without copying.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_2_4_a_clone_stops_on_what_it_cannot_copy() {
        let st = state().await;
        let t = TenantId::default();
        let clone = "urn:ngsi-ld:Snapshot:c";
        seeded_snapshot(&st, clone, "preparing").await;
        lease_to(&st, clone, "me").await;
        clone_fill(&st, &t, "urn:ngsi-ld:Snapshot:gone", clone, "me").await;
        assert_eq!(stored(&st, clone).await["snapshotStatus"], "failure");

        let src = "urn:ngsi-ld:Snapshot:busy";
        seeded_snapshot(&st, src, "preparing").await;
        let other = "urn:ngsi-ld:Snapshot:c2";
        seeded_snapshot(&st, other, "preparing").await;
        lease_to(&st, other, "someone-else").await;
        clone_fill(&st, &t, src, other, "me").await;
        assert_eq!(stored(&st, other).await["snapshotStatus"], "preparing");

        let done = "urn:ngsi-ld:Snapshot:done";
        seeded_snapshot(&st, done, "success").await;
        clone_fill(&st, &t, done, other, "me").await;
        assert_eq!(stored(&st, other).await["snapshotStatus"], "preparing");

        let bare = "urn:ngsi-ld:Snapshot:c3";
        st.store
            .create(
                &t,
                Kind::Snapshot,
                bare,
                json!({"id": bare, "type": "Snapshot", "snapshotStatus": "preparing",
                       "expiresAt": "2999-01-01T00:00:00Z", LEASE_MEMBER: lease("me")}),
            )
            .await
            .expect("seed");
        clone_fill(&st, &t, done, bare, "me").await;
        assert_eq!(stored(&st, bare).await["snapshotStatus"], "failure");
    }

    /// 5.16.1.4: a query that is no object fails, and one whose results
    /// pass the budget is TooManyResults — counted by the documents when an
    /// idPattern keeps the store from counting.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_5_6_a_query_past_its_budget_copies_nothing() {
        let st = state().await;
        let t = TenantId::default();
        let (status, _, body) = post_json(
            &st,
            "/ngsi-ld/v1/entities",
            &json!({"id": "urn:ngsi-ld:Vehicle:budget", "type": "Vehicle"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let synth = TenantId::new_internal("snap-budget").expect("synth");
        let ctx = st.loader.core();
        assert!(matches!(
            run_query(&st, &t, &synth, &json!(5), &ctx, 10).await,
            Err(NgsiError::BadRequestData(m)) if m == "Query must be an object"
        ));
        let q = json!({"type": "Query",
            "entities": [{"type": "Vehicle", "idPattern": "urn:ngsi-ld:Vehicle:.*"}]});
        let plain = json!({"type": "Query", "entities": [{"type": "Vehicle"}]});
        let r = run_query(&st, &t, &synth, &plain, &ctx, 10).await;
        assert!(matches!(r, Ok(1)), "{r:?}");
        let r = run_query(&st, &t, &synth, &q, &ctx, 0).await;
        assert!(matches!(r, Err(NgsiError::TooManyResults(_))), "{r:?}");
        assert!(matches!(
            run_temporal_query(&st, &t, &synth, "urn:ngsi-ld:Snapshot:x", &json!(5), 10).await,
            Err(NgsiError::BadRequestData(m)) if m == "Query must be an object"
        ));
        assert_eq!(
            st.store
                .list(&synth, Kind::Entity)
                .await
                .expect("list")
                .len(),
            1,
            "only the query within its budget copied"
        );
    }

    /// S4-22 against a store that refuses every call: the sweep reaps
    /// nothing, a lease renewal that fails lets the fill go on, and the
    /// fill then reads no Snapshot and stops.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn s4_22_a_failing_store_never_reads_as_a_deletion() {
        let st = crate::state::refusing_state();
        let t = TenantId::default();
        assert_eq!(sweep_expired_snapshots(&st, &t).await, 0);
        assert_eq!(
            hold(&st, &t, "urn:ngsi-ld:Snapshot:x", "o").await,
            Hold::Held
        );
        fill_snapshot(&st, &t, "urn:ngsi-ld:Snapshot:x", "o").await;
        finish(&st, &t, "urn:ngsi-ld:Snapshot:x", "o", "failure", None).await;
        let synth = TenantId::new_internal("snap-refused").expect("synth");
        let ctx = st.loader.core();
        let any = json!({"type": "Query", "entities": [{"type": "*"}]});
        assert!(matches!(
            run_query(&st, &t, &synth, &any, &ctx, 10).await,
            Err(NgsiError::InternalError(m)) if m == "query execution failed"
        ));
        let tq = json!({"type": "Query", "entities": [{"type": "Vehicle"}],
            "temporalQ": {"timerel": "after", "timeAt": "2000-01-01T00:00:00Z"}});
        assert!(matches!(
            run_temporal_query(&st, &t, &synth, "urn:ngsi-ld:Snapshot:x", &tq, 10).await,
            Err(NgsiError::InternalError(m)) if m == "temporal query execution failed"
        ));
    }

    /// S4-22 and 6.3.22 seams on what is not theirs: an expired Snapshot is
    /// gone to a lease renewal; an orphan that names no copy is claimed and
    /// purges nothing; the reverse-index markers are no Snapshot to a
    /// listing, a sweep or an erasure; a synthetic tenant nobody indexed has
    /// no owner.
    #[tokio::test(flavor = "multi_thread")]
    async fn s4_22_seams_skip_what_is_not_a_live_snapshot() {
        let st = state().await;
        let t = TenantId::default();
        let old = "urn:ngsi-ld:Snapshot:old";
        st.store
            .create(
                &t,
                Kind::Snapshot,
                old,
                json!({"id": old, "type": "Snapshot", "snapshotStatus": "preparing",
                       "expiresAt": "2000-01-01T00:00:00Z", LEASE_MEMBER: lease("o")}),
            )
            .await
            .expect("seed");
        assert_eq!(hold(&st, &t, old, "o").await, Hold::Gone);

        let bare = "urn:ngsi-ld:Snapshot:bare-orphan";
        st.store
            .create(
                &t,
                Kind::Snapshot,
                bare,
                json!({"id": bare, "type": "Snapshot", "snapshotStatus": "preparing",
                       "expiresAt": "2999-01-01T00:00:00Z",
                       LEASE_MEMBER: {"owner": "dead", "until": "2000-01-01T00:00:00Z"}}),
            )
            .await
            .expect("seed");
        resume_orphans(&st, &t, vec![bare.to_owned()]).await;
        let claimed = stored(&st, bare).await;
        assert!(!leased_to(&claimed, "dead"), "claimed anew: {claimed}");
        assert!(!fill_orphaned(&claimed), "{claimed}");

        let sid = snapshot_over_vehicles(&st).await;
        let idx = snap_index_tenant().expect("index tenant");
        assert_eq!(
            st.store
                .list(&idx, Kind::Snapshot)
                .await
                .expect("list")
                .len(),
            1
        );
        assert!(copies_of(&st, &idx).await.expect("walk").is_empty());
        assert_eq!(sweep_expired_snapshots(&st, &idx).await, 0);
        assert_eq!(
            st.store
                .list(&idx, Kind::Snapshot)
                .await
                .expect("list")
                .len(),
            1,
            "the marker of {sid} stays"
        );
        assert_eq!(snapshot_of_synth(&st, "snap-nobody-indexed").await, None);
        assert_eq!(snapshot_of_synth(&st, "mobility").await, None);
    }

    /// clause 5.16.1, item 4 cut of an Evolution: an Attribute whose instances are no
    /// array, and every Attribute without a window, pass the cut untouched.
    #[test]
    fn clause_5_16_1_4_the_cut_keeps_what_has_no_instances_to_drop() {
        let vp: HashMap<String, String> = [
            ("timerel".to_owned(), "after".to_owned()),
            ("timeAt".to_owned(), "2026-01-02T00:00:00Z".to_owned()),
        ]
        .into();
        let tq = crate::temporalq::TemporalQ::from_params(&vp, true)
            .expect("temporalQ parses")
            .expect("temporalQ");
        let doc = json!({"id": "urn:ngsi-ld:V:1", "type": "V", "a": {"value": 1},
            "b": [{"value": 2, "observedAt": "2026-01-01T00:00:00Z"},
                  {"value": 3, "observedAt": "2026-01-03T00:00:00Z"}]});
        let cut = selected(doc.clone(), None, Some(&tq));
        assert_eq!(cut["a"], json!({"value": 1}));
        assert_eq!(cut["b"].as_array().map(Vec::len), Some(1), "{cut}");
        assert_eq!(selected(doc.clone(), None, None), doc);
    }

    /// clause 5.16.1, item 4 query seams: a `type` of "*" is answered locally, a geoQ the
    /// store cannot read is that query's BadRequestData, a temporal query of
    /// a Snapshot already gone copies nothing, one past its budget is
    /// TooManyResults and one whose q does not parse is BadRequestData.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_1_4_query_seams_report_what_each_query_did() {
        let st = state().await;
        let t = TenantId::default();
        let (status, _, b) = post_json(
            &st,
            "/ngsi-ld/v1/temporal/entities",
            &json!({"id": "urn:ngsi-ld:Vehicle:seam", "type": "Vehicle",
                "speed": [{"type": "Property", "value": 1,
                           "observedAt": "2026-01-01T00:00:00Z"}]}),
        )
        .await;
        assert!(status.is_success(), "{status} {b}");
        let (status, _, b) = post_json(
            &st,
            "/ngsi-ld/v1/entities",
            &json!({"id": "urn:ngsi-ld:Vehicle:now", "type": "Vehicle"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        let synth = TenantId::new_internal("snap-seams").expect("synth");
        let ctx = st.loader.core();
        let any = json!({"type": "Query", "entities": [{"type": "*"}]});
        let r = run_query(&st, &t, &synth, &any, &ctx, 10).await;
        assert!(matches!(r, Ok(1)), "{r:?}");
        let geo = json!({"type": "Query", "entities": [{"type": "*"}],
            "geoQ": {"georel": "nowhere", "geometry": "Point", "coordinates": "[1,2]"}});
        let r = run_query(&st, &t, &synth, &geo, &ctx, 10).await;
        assert!(matches!(r, Err(NgsiError::BadRequestData(_))), "{r:?}");

        let tq = json!({"type": "Query", "entities": [{"type": "Vehicle"}],
            "temporalQ": {"timerel": "after", "timeAt": "2000-01-01T00:00:00Z"}});
        let r = run_temporal_query(&st, &t, &synth, "urn:ngsi-ld:Snapshot:gone", &tq, 10).await;
        assert!(matches!(r, Ok(0)), "{r:?}");
        let sid = "urn:ngsi-ld:Snapshot:seams";
        seeded_snapshot(&st, sid, "preparing").await;
        let r = run_temporal_query(&st, &t, &synth, sid, &tq, 0).await;
        assert!(matches!(r, Err(NgsiError::TooManyResults(_))), "{r:?}");
        let mut bad_q = tq.clone();
        bad_q["q"] = json!("speed>>>");
        let r = run_temporal_query(&st, &t, &synth, sid, &bad_q, 10).await;
        assert!(matches!(r, Err(NgsiError::BadRequestData(_))), "{r:?}");
        assert!(st.temporal.list(&synth).await.expect("list").is_empty());
    }

    /// 6.3.22 scoping: a Tenant header that is not one is refused before the
    /// Snapshot is looked up, and a Snapshot naming no copy is an
    /// InternalError, never a request served from live data.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_6_3_22_scoping_refuses_what_it_cannot_scope() {
        let st = state().await;
        let (status, headers, body) = send(
            &st,
            "GET",
            "/ngsi-ld/v1/entities?type=Vehicle",
            None,
            &[
                ("NGSILD-Snapshot", "urn:ngsi-ld:Snapshot:any"),
                ("NGSILD-Tenant", "not a tenant"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            header(&headers, "NGSILD-Snapshot"),
            Some("urn:ngsi-ld:Snapshot:any")
        );
        let bare = "urn:ngsi-ld:Snapshot:copyless";
        st.store
            .create(
                &TenantId::default(),
                Kind::Snapshot,
                bare,
                json!({"id": bare, "type": "Snapshot", "snapshotStatus": "success",
                       "expiresAt": "2999-01-01T00:00:00Z"}),
            )
            .await
            .expect("seed");
        let (status, _, body) = send(
            &st,
            "GET",
            "/ngsi-ld/v1/entities?type=Vehicle",
            None,
            &[("NGSILD-Snapshot", bare)],
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(body["detail"], "snapshot without tenant", "{body}");
    }

    /// 5.16.6: a Snapshot whose endpoint the egress policy refuses is not
    /// notified at all.
    #[tokio::test]
    async fn clause_5_16_6_a_refused_endpoint_is_not_notified() {
        let st = AppState::new("antares-snapshots".into());
        let uri = "mqtt://broker.example/snapshots";
        assert!(st.egress.check_url(uri).await.is_err());
        let meta = json!({"id": "urn:ngsi-ld:Snapshot:n", "type": "Snapshot",
            "snapshotStatus": "success", "endpoint": uri});
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            send_notification(&st, &meta),
        )
        .await
        .expect("returns at once, nothing is sent");
    }

    /// The keeper renews the lease while the fill runs and stops once the
    /// lease is another fill's or once it is dropped.
    #[tokio::test(start_paused = true)]
    async fn the_keeper_renews_until_the_lease_is_lost() {
        let st = AppState::new("antares-snapshots".into());
        let t = TenantId::default();
        let id = "urn:ngsi-ld:Snapshot:kept";
        st.store
            .create(
                &t,
                Kind::Snapshot,
                id,
                json!({"id": id, "type": "Snapshot", "snapshotStatus": "preparing",
                       "expiresAt": "2999-01-01T00:00:00Z",
                       LEASE_MEMBER: {"owner": "me", "until": "2000-01-01T00:00:00Z"}}),
            )
            .await
            .expect("seed");
        let keeper = Keeper::start(&st, &t, id, "me");
        let tick = std::time::Duration::from_millis(u64::from(KEEP_MS) + 10);
        tokio::time::sleep(tick).await;
        let doc = stored(&st, id).await;
        assert!(!fill_orphaned(&doc), "renewed: {doc}");
        lease_to(&st, id, "another").await;
        tokio::time::sleep(tick).await;
        assert!(leased_to(&stored(&st, id).await, "another"));
        drop(keeper);
        let second = Keeper::start(&st, &t, id, "another");
        drop(second);
        tokio::time::sleep(tick).await;
        let doc = stored(&st, id).await;
        assert!(leased_to(&doc, "another"), "{doc}");
    }

    /// 5.2.41 Table 5.2.41-1: the snapshot id "cannot be later modified in
    /// update operations" — whatever JSON type the client sends it as.
    #[test]
    fn clause_5_16_4_4_id_cannot_be_modified() {
        assert!(validate(&json!({"id": "urn:ngsi-ld:Snapshot:1"}), Mode::Update).is_err());
        assert!(
            validate(&json!({"id": 5}), Mode::Update).is_err(),
            "a non-string id escaped the 5.2.41 restriction"
        );
        assert!(validate(&json!({"snapshotPriority": 7}), Mode::Update).is_ok());
    }

    /// 5.16.1.4: expiresAt is set "taking into account the snapshotLifetime
    /// requested, but applying the configured limit of the NGSI-LD system";
    /// a malformed duration is BadRequestData, and a snapshot past its
    /// expiresAt is gone together with its copy.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_1_4_lifetime_is_clamped_and_expiry_removes_the_snapshot() {
        let mut o = Map::new();
        o.insert("snapshotLifetime".into(), json!("P10Y"));
        let exp = crate::state::instant(&expires_at(&o)).expect("rfc3339");
        let limit = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(MAX_LIFETIME_SECS);
        assert!(exp <= limit, "the configured limit was not applied: {exp}");
        assert!(
            exp > jiff::Timestamp::now() + jiff::SignedDuration::from_hours(6 * 24),
            "the suggested lifetime was ignored: {exp}"
        );
        assert!(validate(
            &json!({"type": "Snapshot", "snapshotLifetime": "tomorrow",
                "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]}),
            Mode::Create
        )
        .is_err());

        let st = state().await;
        let sid = snapshot_over_vehicles(&st).await;
        let synth = synth_tenant(&stored(&st, &sid).await).expect("__tenant");
        let _ = st
            .store
            .create(
                &synth,
                Kind::Entity,
                "urn:ngsi-ld:Vehicle:e1",
                json!({"id": "urn:ngsi-ld:Vehicle:e1"}),
            )
            .await;
        let _ = st
            .store
            .mutate(&TenantId::default(), Kind::Snapshot, &sid, |d| {
                d["expiresAt"] = json!("2000-01-01T00:00:00.000Z");
                Ok::<_, std::convert::Infallible>(())
            })
            .await;
        let (status, _, b) = send(
            &st,
            "GET",
            &format!("/ngsi-ld/v1/snapshots/{sid}"),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "an expired snapshot: {b}");
        assert!(
            !st.store
                .list(&synth, Kind::Entity)
                .await
                .unwrap_or_default()
                .is_empty(),
            "the GET freed the copy: reading must not write"
        );
        // The sweep is what frees it, and the purge it starts is background.
        assert_eq!(
            sweep_expired_snapshots(&st, &TenantId::default()).await,
            1,
            "the sweep did not reap the expired snapshot"
        );
        for _ in 0..100 {
            if st
                .store
                .list(&synth, Kind::Entity)
                .await
                .unwrap_or_default()
                .is_empty()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the copy of an expired snapshot was not freed");
    }

    const DEF: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

    /// A Snapshot document held directly, with its synthetic tenant.
    async fn seeded_snapshot(st: &AppState, id: &str, status: &str) -> TenantId {
        let synth = format!("snap-default-{}", id.rsplit(':').next().unwrap_or("x"));
        st.store
            .create(
                &TenantId::default(),
                Kind::Snapshot,
                id,
                json!({"id": id, "type": "Snapshot", "__tenant": synth,
                       "snapshotStatus": status, "expiresAt": "2999-01-01T00:00:00Z"}),
            )
            .await
            .expect("seed snapshot");
        synth_tenant(&stored(st, id).await).expect("__tenant")
    }

    async fn vehicle_in(st: &AppState, t: &TenantId, n: usize) {
        let id = format!("urn:ngsi-ld:Vehicle:c{n}");
        st.store
            .create(
                t,
                Kind::Entity,
                &id,
                json!({"id": id, "type": format!("{DEF}Vehicle")}),
            )
            .await
            .expect("seed entity");
    }

    /// POST .../clone and the clone's id.
    async fn clone_of(st: &AppState, sid: &str) -> String {
        let (status, headers, body) = send(
            st,
            "POST",
            &format!("/ngsi-ld/v1/snapshots/{sid}/clone"),
            Some(("application/json", String::new())),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let loc = header(&headers, "Location").expect("Location");
        loc.rsplit('/').next().expect("clone id").to_owned()
    }

    /// The Snapshot once its fill wrote an outcome, read every 20 ms.
    async fn settled(st: &AppState, id: &str) -> Value {
        let poll = async {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                let meta = stored(st, id).await;
                if meta["snapshotStatus"] != "preparing" {
                    return meta;
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(8), poll)
            .await
            .expect("the snapshot left preparing")
    }

    /// 5.16.2.4 (F-ANT5-005): the clone holds the source's whole Temporal
    /// Evolution, never the one-instance stub its own Entity copy recorded.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_2_4_clone_copies_every_temporal_instance() {
        let st = state().await;
        let sid = "urn:ngsi-ld:Snapshot:tsrc";
        let from = seeded_snapshot(&st, sid, "success").await;
        let eid = "urn:ngsi-ld:Vehicle:c0";
        let insts: Vec<Value> = (1..=5)
            .map(|d| {
                json!({"type": "Property", "value": d,
                            "observedAt": format!("2026-01-0{d}T00:00:00Z")})
            })
            .collect();
        st.temporal
            .upsert(
                &from,
                eid,
                json!({"id": eid, "type": format!("{DEF}Vehicle"),
                                        format!("{DEF}speed"): insts.clone()}),
            )
            .await
            .expect("seed evolution");
        vehicle_in(&st, &from, 0).await;
        let cid = clone_of(&st, sid).await;
        assert_eq!(settled(&st, &cid).await["snapshotStatus"], "success");
        let to = synth_tenant(&stored(&st, &cid).await).expect("__tenant");
        let doc = st
            .temporal
            .get(&to, eid)
            .await
            .expect("store")
            .expect("evolution");
        assert_eq!(
            doc[format!("{DEF}speed")].as_array().map(Vec::len),
            Some(5),
            "the clone kept a stub: {doc}"
        );
        // the stub the 5.6.11 hook leaves when it records the Entity copy
        let stubbed = TenantId::new_internal("snap-default-stubbed").expect("tenant");
        st.temporal
            .upsert(
                &stubbed,
                eid,
                json!({"id": eid, format!("{DEF}speed"): [insts.first().cloned().expect("an instance")]}),
            )
            .await
            .expect("seed stub");
        copy_snapshot_data(&st, &from, &stubbed, fill_cap(&st))
            .await
            .expect("copy");
        let doc = st
            .temporal
            .get(&stubbed, eid)
            .await
            .expect("store")
            .expect("evolution");
        assert_eq!(
            doc[format!("{DEF}speed")].as_array().map(Vec::len),
            Some(5),
            "the copy kept the hook's stub: {doc}"
        );
    }

    /// 5.16.2 step 4: a source whose Temporal Evolutions outgrow the snapshot
    /// ceiling is not copied: TooManyResults, and nothing half-copied is
    /// reported as a success.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_2_4_a_clone_of_too_many_evolutions_is_refused() {
        let mut st = state().await;
        st.max_limit = 1;
        let from = TenantId::new_internal("snap-default-big").expect("tenant");
        let to = TenantId::new_internal("snap-default-bigto").expect("tenant");
        let cap = fill_cap(&st);
        for i in 0..=cap {
            let eid = format!("urn:ngsi-ld:Vehicle:big{i}");
            st.temporal
                .upsert(
                    &from,
                    &eid,
                    json!({"id": eid, "type": format!("{DEF}Vehicle")}),
                )
                .await
                .expect("seed evolution");
        }
        let err = copy_snapshot_data(&st, &from, &to, cap)
            .await
            .expect_err("over the ceiling");
        assert!(matches!(err, NgsiError::TooManyResults(_)), "{err:?}");
    }

    /// 5.16.2.4 (F-ANT5-007): a clone of a source still preparing waits for
    /// the source's fill and copies what it ends with; a source that fails
    /// fails the clone.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_2_4_clone_waits_for_a_preparing_source() {
        let st = state().await;
        let sid = "urn:ngsi-ld:Snapshot:psrc";
        let from = seeded_snapshot(&st, sid, "preparing").await;
        vehicle_in(&st, &from, 1).await;
        let cid = clone_of(&st, sid).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(stored(&st, &cid).await["snapshotStatus"], "preparing");
        vehicle_in(&st, &from, 2).await;
        let _ = st
            .store
            .mutate(&TenantId::default(), Kind::Snapshot, sid, |d| {
                d["snapshotStatus"] = json!("success");
                Ok::<_, std::convert::Infallible>(())
            })
            .await;
        assert_eq!(settled(&st, &cid).await["snapshotStatus"], "success");
        let to = synth_tenant(&stored(&st, &cid).await).expect("__tenant");
        assert_eq!(
            st.store.list(&to, Kind::Entity).await.expect("list").len(),
            2
        );

        let fid = "urn:ngsi-ld:Snapshot:fsrc";
        seeded_snapshot(&st, fid, "failure").await;
        let cid = clone_of(&st, fid).await;
        assert_eq!(settled(&st, &cid).await["snapshotStatus"], "failure");
    }

    /// 5.5.6 (F-ANT5-019): a clone stops at the fill ceiling of its own
    /// broker, whatever the source was filled under.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_5_6_clone_stops_at_the_result_ceiling() {
        let mut st = state().await;
        st.max_limit = 1;
        let sid = "urn:ngsi-ld:Snapshot:big";
        let from = seeded_snapshot(&st, sid, "success").await;
        for n in 0..=fill_cap(&st) {
            vehicle_in(&st, &from, n).await;
        }
        let cid = clone_of(&st, sid).await;
        assert_eq!(settled(&st, &cid).await["snapshotStatus"], "failure");
    }

    /// 5.16.4.4 (F-ANT5-006): a fill finishing with a document it read
    /// before an Update Snapshot keeps the update's expiresAt, and the
    /// update keeps the fill's status.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_4_4_finish_and_update_keep_each_others_members() {
        let st = state().await;
        let t = TenantId::default();
        let sid = "urn:ngsi-ld:Snapshot:race";
        seeded_snapshot(&st, sid, "preparing").await;
        lease_to(&st, sid, "fill").await;
        let (status, _, b) = send(
            &st,
            "PATCH",
            &format!("/ngsi-ld/v1/snapshots/{sid}"),
            Some((
                "application/json",
                json!({"snapshotLifetime": "PT1H"}).to_string(),
            )),
            &[],
        )
        .await;
        assert!(status.is_success(), "{status} {b}");
        let updated = stored(&st, sid).await["expiresAt"].clone();
        assert_ne!(updated, json!("2999-01-01T00:00:00Z"));
        finish(&st, &t, sid, "fill", "success", None).await;
        let meta = stored(&st, sid).await;
        assert_eq!(
            meta["expiresAt"], updated,
            "the fill wrote back a stale expiresAt"
        );
        assert_eq!(meta["snapshotStatus"], "success");
    }

    /// 5.16.1.4 (F-ANT5-008): a temporal snapshot query's attrs and window
    /// bound what the snapshot holds.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_1_4_temporal_copy_keeps_the_query_attrs_and_window() {
        let st = state().await;
        let eid = "urn:ngsi-ld:Vehicle:tq";
        let inst = |v: i64, d: u8| {
            json!({"type": "Property", "value": v,
                                          "observedAt": format!("2026-01-0{d}T00:00:00Z")})
        };
        let ent = json!({"id": eid, "type": "Vehicle",
            "a": [inst(1, 1), inst(2, 3), inst(3, 5)], "b": [inst(9, 3)]});
        let (status, _, b) = post_json(&st, "/ngsi-ld/v1/temporal/entities", &ent).await;
        assert!(status.is_success(), "{status} {b}");
        let doc = json!({"type": "Snapshot", "snapshotTemporalQueries": [{
            "type": "Query", "entities": [{"type": "Vehicle"}], "attrs": ["a"],
            "temporalQ": {"timerel": "between", "timeAt": "2026-01-02T00:00:00Z",
                          "endTimeAt": "2026-01-04T00:00:00Z"}}]});
        let (status, headers, b) = post_json(&st, "/ngsi-ld/v1/snapshots", &doc).await;
        assert_eq!(status, StatusCode::CREATED, "{b}");
        let sid = header(&headers, "Location")
            .and_then(|l| l.rsplit('/').next())
            .expect("id")
            .to_owned();
        assert_eq!(settled(&st, &sid).await["snapshotStatus"], "success");
        let synth = synth_tenant(&stored(&st, &sid).await).expect("__tenant");
        let held = st
            .temporal
            .get(&synth, eid)
            .await
            .expect("store")
            .expect("copy");
        assert!(
            held.get(format!("{DEF}b")).is_none(),
            "b is outside attrs: {held}"
        );
        assert_eq!(
            held[format!("{DEF}a")].as_array().map(Vec::len),
            Some(1),
            "instances outside the window: {held}"
        );
    }
}
