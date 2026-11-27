// SPDX-License-Identifier: EUPL-1.2
//! The CONSUMER half of distributed subscriptions (5.8.1.4 / 5.8.2.4 /
//! 5.8.5.4): an entity Subscription with localOnly != true creates an
//! internal Context Source Registration Subscription (5.11.2) whose
//! CSource notifications drive per-registration remote subscription
//! create/update/delete (triggerReason newlyMatching / updated /
//! noLongerMatching), with subscriptionId mappings stored so inbound
//! remote notifications forward to the original subscriber.

use crate::negotiate::{ApiError, ApiResult};
use crate::state::{now_iso, AppState};
use antares_model::{NgsiError, TenantId};
use antares_store::CurrentStateDriverExt;
use antares_store::Kind;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};

fn ds_index_tenant() -> Option<TenantId> {
    TenantId::new_internal("distsub-index").ok()
}

/// The mapping of `own_id` (`{}` when it has none yet); `None` when the
/// store read failed. An unread mapping is not an empty one: a caller that
/// took it for one would create a second Registration Subscription, write
/// the remotes away or leave the remote copies behind, so it changes nothing.
async fn ds_get(st: &AppState, tenant: &TenantId, own_id: &str) -> Option<Value> {
    match st.store.get(tenant, Kind::DistSub, own_id).await {
        Ok(doc) => Some(doc.unwrap_or_else(|| json!({}))),
        Err(e) => {
            tracing::warn!("subscription {own_id}: distributed mapping not read: {e}");
            None
        }
    }
}

async fn ds_put(st: &AppState, tenant: &TenantId, own_id: &str, doc: Value) {
    let updated = st
        .store
        .mutate(tenant, Kind::DistSub, own_id, |d| {
            *d = doc.clone();
            Ok::<_, std::convert::Infallible>(())
        })
        .await
        .ok()
        .flatten()
        .is_some();
    if !updated {
        if let Err(e) = st.store.create(tenant, Kind::DistSub, own_id, doc).await {
            tracing::warn!("subscription {own_id}: distributed mapping not stored: {e}");
        }
    }
}

/// remotes of one own Subscription: reg id → (endpoint, remote sub id)
fn ds_remotes(doc: &Value) -> Vec<(String, (String, String))> {
    doc.get("remotes")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(reg, v)| {
                    Some((
                        reg.clone(),
                        (
                            v.get(0)?.as_str()?.to_owned(),
                            v.get(1)?.as_str()?.to_owned(),
                        ),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Insert or remove ONE `remotes` entry under the store's own lock, and
/// only if the document still exists. A targeted mutate, never a
/// read-modify-write of the whole document: the per-registration branches of
/// 5.8.1.4 interleave at their forward `await`s, so a full-document write
/// would drop a sibling registration's mapping, and a Delete Subscription
/// (5.8.5.4) that lands mid-forward would be undone by a write that
/// resurrects the deleted document. `false` = the Subscription's mapping
/// document is gone, or the registration already holds a remote subscription
/// — 5.8.1.4 stores ONE subscriptionId per Context Source Registration, and
/// two notifications racing for the same pair would otherwise each create a
/// remote copy and orphan the loser's at the source. That check runs inside
/// the closure the store executes under its write lock, so the same lock
/// decides and acts.
async fn ds_set_remote(
    st: &AppState,
    tenant: &TenantId,
    own_id: &str,
    reg_id: &str,
    entry: Option<Value>,
) -> bool {
    st.store
        .mutate(tenant, Kind::DistSub, own_id, |d| {
            // a mapping document is an object; one that is not takes no remote
            let o = d.as_object_mut().ok_or(())?;
            match &entry {
                Some(v) => {
                    // a refusal commits nothing, so the map is put back only
                    // once it took the entry
                    let mut m = match o.remove("remotes") {
                        Some(Value::Object(m)) => m,
                        _ => serde_json::Map::new(),
                    };
                    if m.contains_key(reg_id) {
                        return Err(());
                    }
                    m.insert(reg_id.to_owned(), v.clone());
                    o.insert("remotes".into(), Value::Object(m));
                }
                None => {
                    if let Some(m) = o.get_mut("remotes").and_then(Value::as_object_mut) {
                        m.remove(reg_id);
                    }
                }
            }
            Ok(())
        })
        .await
        .ok()
        .flatten()
        .is_some_and(|r: Result<(), ()>| r.is_ok())
}

/// The `receiverInfo` header a forwarded copy's notifications carry back to
/// `POST /ex/v1/remote-notify` (T-7421). The remote subscription id names a mapping, it
/// proves nothing: anyone who learns it at the Context Source could post a
/// notification under it.
pub const NOTIFY_PROOF_HEADER: &str = "Antares-Notify-Proof";

/// How far a notification's `notifiedAt` may lie from this broker's clock,
/// and how long its id is remembered against a replay.
const NOTIFY_WINDOW_SECS: i64 = 300;

/// Remembered notification ids at most. ponytail: per process, so a replay
/// to another replica inside the window passes its proof check; a shared
/// store row per id is the upgrade path.
const NOTIFY_SEEN_CAP: usize = 1 << 20;

/// A fresh proof for one remote copy: 244 random bits from two v4 UUIDs.
fn notify_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// What the inbound index stores in place of a proof: its SHA-256, so a
/// store read never yields a token that notifies.
fn proof_digest(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A new proof for the mapping of `remote_id`, the current one kept as
/// `prev` so notifications sent before the update lands still pass;
/// `None` puts the previous proof back after a failed update.
async fn inbound_rotate(st: &AppState, remote_id: &str, digest: Option<&str>) {
    let Some(idx) = ds_index_tenant() else { return };
    let _ = st
        .store
        .mutate(&idx, Kind::DistSub, remote_id, |d| {
            // an index entry is an object: `inbound_put` is its only writer,
            // and it always writes a proof
            let o = d.as_object_mut().ok_or(())?;
            match digest {
                Some(new) => {
                    let cur = o.insert("proof".into(), Value::String(new.to_owned()));
                    o.extend(cur.map(|c| ("prev".to_owned(), c)));
                }
                None => {
                    // a second failed update finds the previous proof back
                    // in place already
                    if let Some(prev) = o.remove("prev") {
                        o.insert("proof".into(), prev);
                    }
                }
            }
            Ok::<_, ()>(())
        })
        .await;
}

/// A PATCH of the remote copy `remote_id`, built by [`reduced_copy`] around
/// the new proof `token`; a refused update puts the previous proof back.
#[allow(clippy::too_many_arguments)] // forward_sub's parts plus the copy id
async fn patch_copy(
    st: &AppState,
    tenant: &TenantId,
    endpoint: &str,
    remote_id: &str,
    reg_id: &str,
    reg: &Value,
    ctx_url: &str,
    via: &HeaderMap,
    token: &str,
    mut copy: Value,
) {
    copy.as_object_mut().map(|o| o.remove("id"));
    inbound_rotate(st, remote_id, Some(&proof_digest(token))).await;
    let (status, _) = forward_sub(
        st,
        tenant,
        reqwest::Method::PATCH,
        format!("{endpoint}/ngsi-ld/v1/subscriptions/{remote_id}"),
        reg_id,
        reg,
        ctx_url,
        via,
        Some(copy),
    )
    .await;
    if !(200..300).contains(&status) {
        inbound_rotate(st, remote_id, None).await;
    }
}

/// Whether `headers` carry the proof one of `digests` stands for.
fn proof_matches(headers: &HeaderMap, digests: &[String]) -> bool {
    let Some(presented) = headers
        .get(NOTIFY_PROOF_HEADER)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let got = proof_digest(presented);
    digests.iter().fold(false, |ok, d| {
        crate::admin_auth::constant_time_eq(d.as_bytes(), got.as_bytes()) | ok
    })
}

/// `false` when the notification is stale, from the future, without an id,
/// or one this broker already took under `sid` inside the window.
fn notification_fresh(v: &Value, sid: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashMap<[u8; 16], i64>>> = OnceLock::new();
    let now = jiff::Timestamp::now().as_second();
    let Some(at) = v
        .get("notifiedAt")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .map(jiff::Timestamp::as_second)
    else {
        return false;
    };
    if now.abs_diff(at) > NOTIFY_WINDOW_SECS.unsigned_abs() {
        return false;
    }
    let Some(id) = v.get("id").and_then(Value::as_str) else {
        return false;
    };
    let key: [u8; 16] = {
        use sha2::{Digest, Sha256};
        let h = Sha256::digest(format!("{sid}\0{id}").as_bytes());
        let mut k = [0u8; 16];
        k.copy_from_slice(h.get(..16).unwrap_or(&[0u8; 16]));
        k
    };
    let mut seen = SEEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    admit(&mut seen, key, now, NOTIFY_SEEN_CAP)
}

/// Remember `key` as taken at `now` in `seen`, holding at most `cap` keys:
/// a full memory first forgets what left the window, and refuses while it
/// stays full. `false` for a key already taken.
fn admit(
    seen: &mut std::collections::HashMap<[u8; 16], i64>,
    key: [u8; 16],
    now: i64,
    cap: usize,
) -> bool {
    if seen.len() >= cap {
        seen.retain(|_, t| now.saturating_sub(*t) <= NOTIFY_WINDOW_SECS);
    }
    if seen.len() >= cap || seen.contains_key(&key) {
        return false;
    }
    seen.insert(key, now);
    true
}

/// The one answer to a notification without a valid proof, for an unknown
/// subscription id alike, so a caller learns nothing about which ids exist.
fn notify_refused() -> Response {
    (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({
                "type": crate::admin_auth::UNAUTHORIZED_TYPE,
                "title": "Unauthorized",
                "status": 401,
                "detail": format!("the notification is not accepted: it needs a fresh notifiedAt, an id not sent before and the {NOTIFY_PROOF_HEADER} header of its subscription"),
            })),
    )
        .into_response()
}

async fn inbound_put(st: &AppState, remote_id: &str, tenant: &TenantId, own_id: &str, proof: &str) {
    let Some(idx) = ds_index_tenant() else { return };
    // Without this index every notification the source sends is answered
    // 404 and the subscription silently never notifies — a store failure
    // here has to be visible to an operator.
    if let Err(e) = st
        .store
        .create(
            &idx,
            Kind::DistSub,
            remote_id,
            json!({"tenant": tenant.as_str(), "own": own_id, "proof": proof}),
        )
        .await
    {
        // Not the remote id: it is the key /ex/v1/remote-notify accepts
        // a notification by, and a log line is no place for it (T-3915).
        tracing::warn!("subscription {own_id}: inbound mapping not stored: {e}");
    }
}

/// Tenant, own Subscription id and the proof digests (current, previous) of
/// the mapping of `remote_id`. An entry whose Tenant does not read is no
/// mapping: `inbound_put` only ever writes a valid one.
async fn inbound_get(st: &AppState, remote_id: &str) -> Option<(TenantId, String, Vec<String>)> {
    let idx = ds_index_tenant()?;
    let doc = st
        .store
        .get(&idx, Kind::DistSub, remote_id)
        .await
        .ok()
        .flatten()?;
    let proofs = ["proof", "prev"]
        .iter()
        .filter_map(|k| doc.get(*k).and_then(Value::as_str).map(str::to_owned))
        .collect();
    Some((
        TenantId::new_internal(doc.get("tenant")?.as_str()?).ok()?,
        doc.get("own")?.as_str()?.to_owned(),
        proofs,
    ))
}

impl AppState {
    /// The Tenant whose distributed Subscription receives the notifications
    /// of the copy `subscription_id` (5.8.1.4), read from this broker's own
    /// inbound index, never from the notification.
    pub async fn notification_receiver(&self, subscription_id: &str) -> Option<String> {
        inbound_get(self, subscription_id)
            .await
            .map(|(tenant, _, _)| tenant.into())
    }
}

async fn inbound_delete(st: &AppState, remote_id: &str) {
    let Some(idx) = ds_index_tenant() else { return };
    let _ = st.store.delete(&idx, Kind::DistSub, remote_id).await;
}

fn distributed(sub: &Value) -> bool {
    sub.get("localOnly").and_then(Value::as_bool) != Some(true)
}

/// 6.3.17/6.3.18: the Via chain the Subscription arrived with, rebuilt as
/// the header the outbound forward extends (`federation::forward` appends
/// this broker's alias). Stored on the Subscription as the broker-internal
/// `__via` member (`__context` is the precedent) by create.
fn sub_via_headers(sub: &Value) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(v) = sub
        .get("__via")
        .and_then(Value::as_str)
        .and_then(|v| axum::http::HeaderValue::from_str(v).ok())
    {
        h.insert("via", v);
    }
    h
}

/// The Subscription members 5.11.2.4 matches a Context Source Registration
/// on, plus the @context the internal Registration Subscription is read
/// under. Kept in one place: create and update must offer the very same
/// document, or an update silently widens which sources are matched.
const CSR_MATCH_MEMBERS: [&str; 6] = [
    "entities",
    "watchedAttributes",
    "csf",
    "geoQ",
    "scopeQ",
    "temporalQ",
];

/// 5.8.1.4: "Based on the content of the Subscription, a Context Source
/// Registration Subscription shall be created (clause 5.11.2)" — internal,
/// with the urn:antares:distsub endpoint handled in-process by notify.
pub(crate) async fn on_subscription_created(st: &AppState, tenant: &TenantId, sub: &Value) {
    if !distributed(sub) {
        return;
    }
    // 6.3.18 ("to avoid infinite loops"): a forwarded copy whose Via chain
    // already names this broker has come full circle — it serves locally,
    // and the distributed half is not created, so mutually registered
    // brokers cannot re-forward copies of copies without bound. via_loop
    // also enforces the MAX_VIA_HOPS ceiling on a forged chain.
    if crate::federation::via_loop(
        &sub_via_headers(sub),
        &crate::federation::alias_for(&st.host_alias, tenant),
    ) {
        return;
    }
    let Some(own_id) = sub.get("id").and_then(Value::as_str) else {
        return;
    };
    let csr_id = format!(
        "{}{}",
        crate::registry::INTERNAL_CSR_PREFIX,
        uuid::Uuid::new_v4()
    );
    let ts = now_iso();
    // 5.11.2.4 matches a registration on all of these, so the Registration
    // Subscription carries every member that decides the match — a copy
    // built from the entity selectors alone offers the Subscription to
    // sources the subscriber's csf, geoQ, scopeQ or temporalQ excluded.
    // `q` is deliberately absent: on a Context Source Registration
    // Subscription it would filter registration properties, not Entity
    // Attributes.
    let mut doc: Map<String, Value> = CSR_MATCH_MEMBERS
        .iter()
        .chain(["__context"].iter())
        .filter_map(|k| Some(((*k).to_owned(), sub.get(*k)?.clone())))
        .collect();
    doc.extend([
        ("id".to_owned(), json!(csr_id)),
        ("type".to_owned(), json!("Subscription")),
        ("isActive".to_owned(), json!(true)),
        ("status".to_owned(), json!("active")),
        ("createdAt".to_owned(), json!(ts)),
        ("modifiedAt".to_owned(), json!(ts)),
        (
            "notification".to_owned(),
            json!({"endpoint": {"uri":
                format!("urn:antares:distsub:{}\n{own_id}", tenant.as_str())}}),
        ),
    ]);
    if let (Some(a), Some(n)) = (
        sub.get("notification").and_then(|n| n.get("attributes")),
        doc.get_mut("notification").and_then(Value::as_object_mut),
    ) {
        n.insert("attributes".into(), a.clone());
    }
    let created = st
        .store
        .create(tenant, Kind::DistSub, &csr_id, Value::Object(doc))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("subscription {own_id}: Registration Subscription not created: {e}");
            false
        });
    if created {
        let Some(mut doc) = ds_get(st, tenant, own_id).await else {
            return;
        };
        if let Some(o) = doc.as_object_mut() {
            o.insert("csr_sub".into(), Value::String(csr_id.clone()));
        }
        ds_put(st, tenant, own_id, doc).await;
        // 5.11.2.4 initial notification with all matching registrations —
        // this is what turns already-known registrations into newlyMatching
        let (st2, t2) = (st.clone(), tenant.clone());
        crate::spawn(async move {
            crate::notify::csource_initial(&st2, &t2, &csr_id).await;
        });
    }
}

/// 5.8.2.4: keep the internal CSR subscription in step and forward reduced
/// updates to every mapped remote supporting updateSubscription (5.11.3).
pub(crate) async fn on_subscription_updated(st: &AppState, tenant: &TenantId, own_id: &str) {
    let Some(sub) = st
        .store
        .get(tenant, Kind::Subscription, own_id)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    // 5.8.1.4 gates the distributed half on "If localOnly=false": a
    // Subscription updated to localOnly=true is torn down like a deleted
    // one — the internal Registration Subscription goes, and every remote
    // copy already created is deleted at its source.
    if !distributed(&sub) {
        on_subscription_deleted(st, tenant, own_id).await;
        return;
    }
    let Some(mapping) = ds_get(st, tenant, own_id).await else {
        return;
    };
    let csr_id = mapping
        .get("csr_sub")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match csr_id {
        None => {
            // became distributed only now (e.g. localOnly flipped off)
            on_subscription_created(st, tenant, &sub).await;
        }
        Some(csr_id) => {
            let _ = st
                .store
                .mutate(tenant, Kind::DistSub, &csr_id, |doc| {
                    // The mapping comes out of the store, and `Value`'s index
                    // panics on anything that is not an object — inside a
                    // closure `mutate` runs holding the write lock, so the shape
                    // is decided once, here, and a mapping that is not one is
                    // left exactly as it was found.
                    let Some(map) = doc.as_object_mut() else {
                        return Ok(());
                    };
                    for k in CSR_MATCH_MEMBERS {
                        match sub.get(k) {
                            Some(v) => {
                                map.insert(k.to_owned(), v.clone());
                            }
                            None => {
                                map.remove(k);
                            }
                        }
                    }
                    let attrs = sub.get("notification").and_then(|n| n.get("attributes"));
                    match map.get_mut("notification").and_then(Value::as_object_mut) {
                        Some(n) => match attrs {
                            Some(a) => {
                                n.insert("attributes".to_owned(), a.clone());
                            }
                            None => {
                                n.remove("attributes");
                            }
                        },
                        // no notification object to carry them: the copy this
                        // mapping stands for is the one create built, so a
                        // mapping without it is not one an update can complete
                        None => return Ok(()),
                    }
                    map.insert("modifiedAt".to_owned(), Value::String(now_iso()));
                    Ok::<(), NgsiError>(())
                })
                .await;
        }
    }
    let Some(mapping) = ds_get(st, tenant, own_id).await else {
        return;
    };
    let remotes: Vec<(String, (String, String))> = ds_remotes(&mapping);
    for (reg_id, (endpoint, remote_id)) in remotes {
        let Some(reg) = st
            .store
            .get(tenant, Kind::Registration, &reg_id)
            .await
            .ok()
            .flatten()
        else {
            continue;
        };
        if !crate::federation::doc_supports(&reg, "updateSubscription") {
            continue;
        }
        let (st2, t2, sub2) = (st.clone(), tenant.clone(), sub.clone());
        let ctx_url = sub_ctx_url(st, &sub);
        crate::spawn(async move {
            let ctx = crate::notify::sub_context(&st2, &t2, &sub2).await;
            let token = notify_token();
            let Some(copy) = reduced_copy(&st2, &sub2, &reg, &remote_id, &ctx, &token) else {
                return;
            };
            patch_copy(
                &st2,
                &t2,
                &endpoint,
                &remote_id,
                &reg_id,
                &reg,
                &ctx_url,
                &sub_via_headers(&sub2),
                &token,
                copy,
            )
            .await;
        });
    }
}

/// 5.8.5.4: delete the internal CSR subscription (5.11.6) and forward the
/// delete to every mapped remote supporting deleteSubscription.
pub(crate) async fn on_subscription_deleted(st: &AppState, tenant: &TenantId, own_id: &str) {
    let Some(doc) = ds_get(st, tenant, own_id).await else {
        return;
    };
    let csr_id = doc
        .get("csr_sub")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let remotes = ds_remotes(&doc);
    for (_, (_, remote_id)) in &remotes {
        inbound_delete(st, remote_id).await;
    }
    let _ = st.store.delete(tenant, Kind::DistSub, own_id).await;
    if let Some(csr_id) = csr_id {
        let _ = st.store.delete(tenant, Kind::DistSub, &csr_id).await;
    }
    for (reg_id, (endpoint, remote_id)) in remotes {
        let stored = st
            .store
            .get(tenant, Kind::Registration, &reg_id)
            .await
            .ok()
            .flatten();
        if stored
            .as_ref()
            .is_some_and(|reg| !crate::federation::doc_supports(reg, "deleteSubscription"))
        {
            continue;
        }
        let (st2, t2) = (st.clone(), tenant.clone());
        let ctx_url = sub_ctx_url(st, &Value::Null);
        crate::spawn(async move {
            let reg = forward_reg(stored, &reg_id, &endpoint);
            // the Subscription is already deleted, so its stored chain is
            // gone — a delete-forward cannot create a copy, so a fresh
            // one-hop chain is loop-safe
            forward_sub(
                &st2,
                &t2,
                reqwest::Method::DELETE,
                format!("{endpoint}/ngsi-ld/v1/subscriptions/{remote_id}"),
                &reg_id,
                &reg,
                &ctx_url,
                &HeaderMap::new(),
                None,
            )
            .await;
        });
    }
}

/// 4.14 / 5.8.5.4: the document a forwarded delete travels with. The stored
/// Context Source Registration carries the tenant, the contextSourceInfo and
/// the timeout/cooldown the forward needs; the id/endpoint pair is the
/// fallback for a registration that is already gone.
fn forward_reg(stored: Option<Value>, reg_id: &str, endpoint: &str) -> Value {
    stored.unwrap_or_else(|| json!({"id": reg_id, "endpoint": endpoint}))
}

/// 5.8.1.4: "The @context to be used for sending Notifications related to
/// this Subscription shall be the one specified in the jsonldContext field."
/// The forwarded copy carries the subscriber's own terms, so it is shipped
/// under the Subscription's @context, falling back to the core context.
fn sub_ctx_url(st: &AppState, sub: &Value) -> String {
    let source = sub
        .get("jsonldContext")
        .or_else(|| sub.get("__context"))
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| st.loader.core().source.clone());
    crate::federation::ctx_link_url(&HeaderMap::new(), &source)
}

/// The 5.8.1.4 localOnly=false block: one CSource notification for the
/// internal CSR subscription, dispatched per registration and triggerReason.
pub(crate) async fn on_csource_notification(
    st: &AppState,
    tenant: &TenantId,
    own_id: &str,
    reason: Option<&str>,
    regs: &[Value],
) {
    let Some(sub) = st
        .store
        .get(tenant, Kind::Subscription, own_id)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    // 5.8.1.4: "If localOnly=false, each time a Context Source Notification
    // … is received" — a Subscription that has since been flipped to
    // localOnly creates no further remote copy.
    if !distributed(&sub) {
        return;
    }
    let ctx = crate::notify::sub_context(st, tenant, &sub).await;
    let ctx_url = sub_ctx_url(st, &sub);
    let via = sub_via_headers(&sub);
    let seen = crate::federation::via_tokens(&via);
    let reason = reason.unwrap_or("updated");
    for reg in regs {
        let Some(reg_id) = reg.get("id").and_then(Value::as_str) else {
            continue;
        };
        // auxiliary registrations take no part (5.8.1.4 lists exclusive,
        // redirect and inclusive only)
        if reg.get("mode").and_then(Value::as_str) == Some("auxiliary") {
            continue;
        }
        // Table 6.3.18-2: the Via listing "is used when determining matching
        // registrations" — a Context Source the Subscription already
        // travelled through must not receive a copy of it back.
        if reg
            .get("contextSourceAlias")
            .and_then(Value::as_str)
            .is_some_and(|a| seen.iter().any(|t| t == a))
        {
            continue;
        }
        let Some(endpoint) = reg
            .get("endpoint")
            .and_then(Value::as_str)
            .map(crate::federation::endpoint_base)
        else {
            continue;
        };
        let Some(mapping) = ds_get(st, tenant, own_id).await else {
            continue;
        };
        let mapped = ds_remotes(&mapping)
            .into_iter()
            .find(|(r, _)| r == reg_id)
            .map(|(_, v)| v);
        match (reason, mapped) {
            // an unmapped registration that starts (or keeps) matching gets
            // the reduced copy — newlyMatching, or "updated" reported by an
            // initial notification when no mapping exists yet
            ("newlyMatching" | "updated", None)
                if crate::federation::doc_supports(reg, "createSubscription") =>
            {
                let remote_id =
                    format!("urn:ngsi-ld:Subscription:distsub:{}", uuid::Uuid::new_v4());
                let token = notify_token();
                let Some(copy) = reduced_copy(st, &sub, reg, &remote_id, &ctx, &token) else {
                    // nothing this registration covers is watched — there is
                    // no reduced copy to forward to it
                    continue;
                };
                // 5.8.1.4/5.8.5.4: the mapping is stored BEFORE the forward —
                // the remote id is broker-generated, and a delete Subscription
                // arriving while the create-forward's response is still in
                // flight must find the mapping or the delete-forward is lost
                // (the ETSI 5814_01_01 pg race). A failed forward rolls the
                // mapping back below.
                inbound_put(st, &remote_id, tenant, own_id, &proof_digest(&token)).await;
                if !ds_set_remote(
                    st,
                    tenant,
                    own_id,
                    reg_id,
                    Some(json!([endpoint.clone(), remote_id])),
                )
                .await
                {
                    // the Subscription was deleted while this notification
                    // was in flight — no remote copy is created for it
                    inbound_delete(st, &remote_id).await;
                    continue;
                }
                let (status, _) = forward_sub(
                    st,
                    tenant,
                    reqwest::Method::POST,
                    format!("{endpoint}/ngsi-ld/v1/subscriptions"),
                    reg_id,
                    reg,
                    &ctx_url,
                    &via,
                    Some(copy),
                )
                .await;
                if !(200..300).contains(&status) {
                    inbound_delete(st, &remote_id).await;
                    ds_set_remote(st, tenant, own_id, reg_id, None).await;
                }
            }
            ("updated", Some((_, remote_id)))
                if crate::federation::doc_supports(reg, "updateSubscription") =>
            {
                let token = notify_token();
                let Some(copy) = reduced_copy(st, &sub, reg, &remote_id, &ctx, &token) else {
                    continue;
                };
                patch_copy(
                    st, tenant, &endpoint, &remote_id, reg_id, reg, &ctx_url, &via, &token, copy,
                )
                .await;
            }
            ("noLongerMatching", Some((_, remote_id)))
                if crate::federation::doc_supports(reg, "deleteSubscription") =>
            {
                forward_sub(
                    st,
                    tenant,
                    reqwest::Method::DELETE,
                    format!("{endpoint}/ngsi-ld/v1/subscriptions/{remote_id}"),
                    reg_id,
                    reg,
                    &ctx_url,
                    &via,
                    None,
                )
                .await;
                ds_set_remote(st, tenant, own_id, reg_id, None).await;
                inbound_delete(st, &remote_id).await;
            }
            _ => {}
        }
    }
}

/// 5.8.1.4: "a copy of the original Subscription shall be reduced to what
/// is matched by the registration information"; with splitEntities the
/// q/geoQ/scopeQ members are removed; the notification attributes/pick/omit
/// members are removed; the endpoint is set to the local broker, carrying
/// `token` as a receiverInfo pair (5.2.22): the Context Source sends it with
/// every notification, masks it on a read and seals it at rest. A stored
/// Subscription is an object with a notification object (5.2.12); anything
/// else has no copy.
///
/// 5.5.7 Term to URI expansion: the Subscription comes out of the store with
/// its names and types EXPANDED, while a registration delivered by a Context
/// Source Notification carries them COMPACTED. Every registration-derived
/// name is expanded against `ctx` before it is compared or inserted, so the
/// reduction happens in one representation — `expand_key` is idempotent on
/// an absolute IRI, which is what the stored registration already holds.
fn reduced_copy(
    st: &AppState,
    sub: &Value,
    reg: &Value,
    remote_id: &str,
    ctx: &antares_jsonld::Context,
    token: &str,
) -> Option<Value> {
    let mut o = sub.as_object()?.clone();
    // __via travels as the Via HTTP header the forward extends (6.3.17),
    // never as a body member
    for k in [
        "status",
        "timesSent",
        "lastNotification",
        "lastSuccess",
        "lastFailure",
        "createdAt",
        "modifiedAt",
        "localOnly",
    ] {
        o.remove(k);
    }
    // every broker-internal member, not a list of the ones known today:
    // __context, __via and the ADR-0020 __subject are this broker's own
    // record and none of them may cross to a Context Source
    o.retain(|k, _| !k.starts_with("__"));
    o.insert("id".into(), Value::String(remote_id.to_owned()));
    // reduce the entity selectors to the registration information
    let reg_entities: Vec<Value> = reg
        .get("information")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| i.get("entities").and_then(Value::as_array))
        .flatten()
        .filter_map(|e| {
            // a registered EntityInfo is an object with a type (5.2.8);
            // notification presentation arrayifies the type, and an
            // EntitySelector wants the plain form back
            let mut o = e.as_object()?.clone();
            let t = o.get("type")?;
            let t = t.as_array().and_then(|a| a.first()).unwrap_or(t).as_str()?;
            let t = Value::String(ctx.expand_key(t));
            o.insert("type".into(), t);
            Some(Value::Object(o))
        })
        .collect();
    if !reg_entities.is_empty() {
        o.insert("entities".into(), Value::Array(reg_entities));
    }
    // watchedAttributes ∩ the registration's attribute scope
    let reg_attrs: Vec<String> = reg
        .get("information")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|i| {
            antares_model::registration_attribute_names(i)
                .unwrap_or_default()
                .into_iter()
                .map(|n| ctx.expand_key(n))
        })
        .collect();
    // a periodic Subscription watches nothing (5.2.12 forbids the pair):
    // its copy gets no watchedAttributes either, or the source refuses it
    let periodic = o.contains_key("timeInterval");
    if !reg_attrs.is_empty() && !periodic {
        let watch: Vec<Value> = match o.get("watchedAttributes").and_then(Value::as_array) {
            Some(w) => w
                .iter()
                .filter(|a| a.as_str().is_some_and(|a| reg_attrs.iter().any(|r| r == a)))
                .cloned()
                .collect(),
            // 5.8.1.4 "reduced to what is matched by the registration
            // information": a watch-everything Subscription still only
            // watches the registered names at the source — otherwise an
            // unregistered attribute change notifies through the chain.
            None => reg_attrs.iter().map(|a| Value::String(a.clone())).collect(),
        };
        // Nothing the subscriber watches is matched by this registration
        // information: there is no reduced copy to forward — and an empty
        // watchedAttributes is a payload 5.2.12 forbids.
        if watch.is_empty() {
            return None;
        }
        o.insert("watchedAttributes".into(), Value::Array(watch));
    }
    // 5.8.1.4: with splitEntities the remote sees only fragments — the
    // q/geoQ/scopeQ conditions are evaluated LOCALLY after the 5.8.6 merge
    // (splitEntities is a Subscription member, 5.2.12)
    if sub.get("splitEntities").and_then(Value::as_bool) == Some(true) {
        for k in ["q", "geoQ", "scopeQ"] {
            o.remove(k);
        }
    }
    let n = o.get_mut("notification").and_then(Value::as_object_mut)?;
    for k in ["attributes", "pick", "omit"] {
        n.remove(k);
    }
    // Table clause 5.2.14, item 2-1: the delivery bookkeeping is this broker's own
    // record of notifying ITS subscriber. It lives inside `notification`,
    // it is output-only on a create ("implementations shall ignore
    // them"), and it says nothing the Context Source may act on — so it
    // stays here, like the top-level members stripped above.
    for k in [
        "status",
        "timesSent",
        "timesFailed",
        "lastNotification",
        "lastSuccess",
        "lastFailure",
    ] {
        n.remove(k);
    }
    n.insert(
        "endpoint".into(),
        json!({"uri": format!("{}/ex/v1/remote-notify", st.public_url),
               "receiverInfo": [{"key": NOTIFY_PROOF_HEADER, "value": token}]}),
    );
    Some(Value::Object(o))
}

/// One forwarded subscription operation, through the shared federation
/// forward (egress policy, Via, contextSourceInfo, tenant mapping).
/// `via` is the chain the Subscription arrived with ([`sub_via_headers`]);
/// the forward appends this broker's alias to it (6.3.17), so downstream
/// brokers see the full path and can cut a loop.
#[allow(clippy::too_many_arguments)] // mirrors the wire: one param per forwarded request part
async fn forward_sub(
    st: &AppState,
    tenant: &TenantId,
    method: reqwest::Method,
    url: String,
    reg_id: &str,
    reg: &Value,
    ctx_url: &str,
    via: &HeaderMap,
    body: Option<Value>,
) -> (u16, Value) {
    let fed = crate::federation::fed_reg_of(reg_id, reg);
    let (status, body, _) =
        crate::federation::forward(st, method, url, &[], via, tenant, &fed, ctx_url, body).await;
    (status, body)
}

/// 5.8.6 splitEntities=true inbound merge: each notified Entity "shall be
/// retrieved locally and from all Context Sources that have information
/// about these Entities, except for the one from which the Notification has
/// been received", merged with the notified fragment, and "all Entities
/// that do not match the query, geoquery and Scope query conditions of the
/// local Subscription shall be removed".
async fn split_merge(
    st: &AppState,
    tenant: &TenantId,
    sub: &Value,
    origin_reg: Option<&str>,
    data: Vec<Value>,
) -> Result<Vec<Value>, NgsiError> {
    let ctx = crate::notify::sub_context(st, tenant, sub).await;
    let headers = HeaderMap::new();
    let mut out = Vec::new();
    for ent in data {
        let Some(obj) = ent.as_object() else { continue };
        // inbound notification presentation → the expanded storage form.
        // 5.5.4 bans "urn:ngsi-ld:null" as a first level member value "with
        // the exception of NGSI-LD Fragments … or to represent deleted
        // Properties in concise representation as part of notifications", so
        // a notified deletion (4.5.7) is a valid payload here and a
        // validating expansion would drop the whole Entity. `sys` keeps the
        // timestamps a sysAttrs subscriber asked for; this is a translation
        // of a document the Context Source already produced, not an
        // admission check on client input.
        // an expanded Entity always carries the id it was read with
        let e = antares_jsonld::expand_entity(
            obj,
            &ctx,
            antares_jsonld::ExpandOpts {
                allow_null: true,
                sys: true,
                ..Default::default()
            },
        )
        .ok()
        .and_then(|m| Some((m.get("id")?.as_str()?.to_owned(), m)));
        let Some((id, mut merged)) = e else { continue };
        if let Ok(Some(local)) = st.store.get(tenant, Kind::Entity, &id).await {
            crate::federation::merge_docs(&mut merged, &local, false);
        }
        let mut warnings = Vec::new();
        let fed = crate::federation::fed_retrieve(
            st,
            tenant,
            &headers,
            &ctx,
            &id,
            None,
            origin_reg,
            &mut warnings,
            &mut None,
        )
        .await?;
        for aux_pass in [false, true] {
            for (aux, doc) in &fed {
                if *aux == aux_pass {
                    crate::federation::merge_docs(&mut merged, doc, *aux);
                }
            }
        }
        if crate::notify::linked_eval(st, tenant, None, |l| {
            crate::notify::conditions_match(sub, &merged, &ctx, l)
        })
        .await
        {
            // 5.3.1/5.8.6: notification data carries Entities in their
            // API representation — shape and compact the merged storage
            // form exactly like the local notify path.
            let shape = crate::notify::notif_shape(sub, &ctx);
            let shaped = crate::repr::apply(&merged, &shape.repr);
            out.push(crate::repr::compact_for(&shape.repr, &shaped, &ctx));
        }
    }
    Ok(out)
}

/// POST /ex/v1/remote-notify — the local broker's endpoint for
/// notifications from forwarded subscription copies. 5.8.1.4: "the mapping
/// of the received subscriptionId with the own Subscription identifier …
/// to enable forwarding received notifications to the original subscriber."
pub async fn remote_notify(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match remote_notify_inner(&st, &headers, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn remote_notify_inner(
    st: &AppState,
    headers: &HeaderMap,
    body: &[u8],
) -> ApiResult<Response> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| NgsiError::InvalidRequest(format!("body is not valid JSON: {e}")))?;
    // Peer-facing entry point: the Entity count is capped before any store
    // touch or per-Entity work, under the same ceiling a client batch gets
    // (ANTARES_MAX_BATCH_ITEMS). One notification drives one local retrieve
    // and one federated fan-out per Entity in the 5.8.6 merge, so an
    // uncapped data array is an amplification lever.
    let cap = *crate::bounds::MAX_BATCH_ITEMS;
    if v.get("data")
        .and_then(Value::as_array)
        .is_some_and(|a| a.len() > cap)
    {
        return Err(NgsiError::BadRequestData(format!(
            "notification data carries more than {cap} Entities"
        ))
        .into());
    }
    let sid = v
        .get("subscriptionId")
        .and_then(Value::as_str)
        .ok_or_else(|| NgsiError::BadRequestData("notification without subscriptionId".into()))?;
    // T-7421: the proof first, and the same answer for an id no mapping
    // holds, so a guessed or leaked subscription id buys nothing.
    let Some((tenant, own_id, proofs)) = inbound_get(st, sid).await else {
        return Ok(notify_refused());
    };
    if !proof_matches(headers, &proofs) || !notification_fresh(&v, sid) {
        return Ok(notify_refused());
    }
    // read back from the broker's own inbound index, not from the request
    // EP-188: the Tenant is the stored one; a refused one answers as a
    // mapping this broker does not hold, so the peer learns nothing more.
    if crate::admin_auth::tenant_door(st, Some(tenant.as_str()), headers, true)
        .await
        .is_some()
    {
        return Ok(notify_refused());
    }
    let Some(sub) = st
        .store
        .get(&tenant, Kind::Subscription, &own_id)
        .await
        .ok()
        .flatten()
    else {
        // the subscriber is gone: prune the mapping on touch so a remote
        // that keeps notifying cannot pin a dead index entry forever, and
        // answer about the peer's own id — the local Subscription id is not
        // the peer's to learn
        inbound_delete(st, sid).await;
        return Err(ApiError::from(NgsiError::ResourceNotFound(format!(
            "no distributed subscription maps {sid}"
        ))));
    };
    // the origin of this notification is the registration its remote
    // subscription was created at
    let origin_reg_id = ds_remotes(&ds_get(st, &tenant, &own_id).await.unwrap_or_default())
        .into_iter()
        .find(|(_, (_, rid))| rid.as_str() == sid)
        .map(|(reg_id, _)| reg_id);
    // 5.8.6: "if a Context Source filter is defined, then only the
    // subscribed Entities whose origin Context Source matches the referred
    // filter shall be included".
    // a stored csf passed the same parse when the Subscription was written
    let csf = sub.get("csf").and_then(Value::as_str);
    if let Some(ast) = csf.and_then(|c| antares_ql::parse_q(c).ok()) {
        let origin_reg = match origin_reg_id.as_ref() {
            Some(reg_id) => st
                .store
                .get(&tenant, Kind::Registration, reg_id)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        // clause 5.8.1, item 4: the csf is written in the Subscription's own @context,
        // and clause 5.11.2, item 4 already read it in that @context when it chose the
        // sources this Subscription was forwarded to. Reading it in the
        // core context here would let the two disagree, and a source the
        // broker deliberately subscribed to would have every notification
        // dropped.
        let ctx = crate::notify::sub_context(st, &tenant, &sub).await;
        let matches = origin_reg.is_some_and(|reg| crate::registry::csf_matches(&ast, &reg, &ctx));
        if !matches {
            // origin gated out — acknowledged, nothing forwarded
            return Ok(StatusCode::OK.into_response());
        }
    }
    let mut data: Vec<Value> = v
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // 5.2.33 / 5.8.1.4: the remote copy carries the REGISTRATION's entity
    // scope, which may be broader than the original Subscription's own
    // entities selector — re-filter inbound entities against the original
    // selector (id over idPattern precedence included) before forwarding.
    let sel_ctx = crate::notify::sub_context(st, &tenant, &sub).await;
    data.retain(|e| {
        let id = e.get("id").and_then(Value::as_str).unwrap_or("");
        let types: Vec<Value> = match e.get("type") {
            Some(Value::String(t)) => vec![Value::String(sel_ctx.expand_key(t))],
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .map(|t| Value::String(sel_ctx.expand_key(t)))
                .collect(),
            _ => Vec::new(),
        };
        let shim = serde_json::json!({"id": id, "type": types});
        crate::notify::selector_match(&sub, &shim, &sel_ctx)
    });
    if data.is_empty() {
        // acknowledged; nothing the original Subscription selected
        return Ok(StatusCode::OK.into_response());
    }
    // 5.8.6 splitEntities=true: the notified Entities are fragments —
    // retrieve them locally and from all other Context Sources (except the
    // origin), merge, and re-filter by the local Subscription's conditions.
    if sub.get("splitEntities").and_then(Value::as_bool) == Some(true) {
        data = split_merge(st, &tenant, &sub, origin_reg_id.as_deref(), data).await?;
        if data.is_empty() {
            // "If there are Entities in the data member of the Notification
            // copy, the Notification copy shall be forwarded" — none left
            return Ok(StatusCode::OK.into_response());
        }
    }
    // 5.8.6: forward to the original subscriber under the OWN subscriptionId
    let split = sub.get("splitEntities").and_then(Value::as_bool) == Some(true);
    let (st2, sub2, t2) = (st.clone(), sub.clone(), tenant.clone());
    crate::spawn(async move {
        let ctx = crate::notify::sub_context(&st2, &t2, &sub2).await;
        // split_merge shaped its Entities already
        let data = if split {
            data
        } else {
            project_forwarded(&sub2, data, &ctx)
        };
        crate::notify::deliver(&st2, &t2, &sub2, data, &ctx).await;
    });
    Ok(StatusCode::OK.into_response())
}

/// The local Subscription's projection (attributes, pick, omit, format)
/// over Entities a Context Source notified (F-ANT3-013): the source answers
/// its own copy's projection, which need not be the subscriber's. Each is
/// read back into the storage form, shaped and compacted as the local
/// notify path does; one that does not read is dropped.
fn project_forwarded(sub: &Value, data: Vec<Value>, ctx: &antares_jsonld::Context) -> Vec<Value> {
    let shape = crate::notify::notif_shape(sub, ctx);
    data.iter()
        .filter_map(Value::as_object)
        .filter_map(|obj| {
            antares_jsonld::expand_entity(
                obj,
                ctx,
                antares_jsonld::ExpandOpts {
                    allow_null: true,
                    sys: true,
                    ..Default::default()
                },
            )
            .ok()
        })
        .map(|doc| {
            let shaped = crate::repr::apply(&doc, &shape.repr);
            crate::repr::compact_for(&shape.repr, &shaped, ctx)
        })
        .collect()
}

/// The mapping bookkeeping of clauses 5.8.1, 5.8.2 and 5.8.5 (step 4) against the inputs
/// that take its other arms: a rewrite of a stored mapping, a Subscription
/// without an id or gone, registrations that are gone or do not offer the
/// operation, and a store that refuses every call.
#[cfg(test)]
mod mapping_edges {
    use super::*;

    const OWN: &str = "urn:ngsi-ld:Subscription:own";

    fn sub(extra: Value) -> Value {
        let mut s = json!({
            "id": OWN,
            "type": "Subscription",
            "entities": [{"type": "Building"}],
            "notification": {"endpoint": {"uri": "http://127.0.0.1:1/notify"}, "attributes": ["height"]}
        });
        for (k, v) in extra.as_object().into_iter().flatten() {
            s[k] = v.clone();
        }
        s
    }

    /// A remote is recorded once per registration: a second one for the
    /// same registration is refused and leaves the first; removing one from
    /// a mapping that holds none changes nothing.
    #[tokio::test]
    async fn a_remote_is_recorded_once_and_removed_quietly() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        let id = "urn:ngsi-ld:Subscription:remotes";
        st.store
            .create(&t, Kind::DistSub, id, json!({"id": id}))
            .await
            .expect("seed");
        assert!(
            ds_set_remote(&st, &t, id, "reg", None).await,
            "nothing to remove"
        );
        assert!(ds_set_remote(&st, &t, id, "reg", Some(json!("r1"))).await);
        assert!(!ds_set_remote(&st, &t, id, "reg", Some(json!("r2"))).await);
        let doc = st
            .store
            .get(&t, Kind::DistSub, id)
            .await
            .expect("store")
            .expect("mapping");
        assert_eq!(doc["remotes"], json!({"reg": "r1"}));
        assert!(ds_set_remote(&st, &t, id, "reg", None).await);
        let doc = st
            .store
            .get(&t, Kind::DistSub, id)
            .await
            .expect("store")
            .expect("mapping");
        assert_eq!(doc["remotes"], json!({}));
    }

    #[tokio::test]
    async fn a_second_put_rewrites_the_stored_mapping() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        assert_eq!(
            ds_get(&st, &t, OWN).await,
            Some(json!({})),
            "no mapping yet reads empty"
        );
        ds_put(&st, &t, OWN, json!({"csr_sub": "a"})).await;
        ds_put(&st, &t, OWN, json!({"csr_sub": "b"})).await;
        assert_eq!(ds_get(&st, &t, OWN).await, Some(json!({"csr_sub": "b"})));
    }

    #[tokio::test]
    async fn a_subscription_without_id_or_gone_builds_no_mapping() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        let mut anonymous = sub(json!({}));
        anonymous.as_object_mut().map(|o| o.remove("id"));
        on_subscription_created(&st, &t, &anonymous).await;
        on_subscription_updated(&st, &t, OWN).await;
        assert!(st
            .store
            .list(&t, Kind::DistSub)
            .await
            .expect("list")
            .is_empty());
    }

    /// Clause 5.8.2, step 4: a Subscription that became distributed only by this update
    /// gets its Registration Subscription now.
    #[tokio::test]
    async fn an_update_of_an_unmapped_subscription_creates_its_half() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        st.store
            .create(&t, Kind::Subscription, OWN, sub(json!({})))
            .await
            .expect("seed");
        on_subscription_updated(&st, &t, OWN).await;
        let mapping = ds_get(&st, &t, OWN).await.expect("read");
        let csr = mapping["csr_sub"]
            .as_str()
            .expect("a Registration Subscription id");
        let doc = st
            .store
            .get(&t, Kind::DistSub, csr)
            .await
            .expect("read")
            .expect("created");
        assert_eq!(doc["entities"], json!([{"type": "Building"}]));
        assert_eq!(doc["notification"]["attributes"], json!(["height"]));
    }

    /// Clause 5.8.2, step 4: the Registration Subscription follows the update, a member
    /// the update dropped goes, and a remote whose registration is gone or
    /// offers no updateSubscription is not contacted.
    #[tokio::test]
    async fn an_update_mirrors_the_match_members_and_skips_unfit_remotes() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        st.store
            .create(
                &t,
                Kind::Subscription,
                OWN,
                sub(json!({"notification": {"endpoint": {"uri": "http://127.0.0.1:1/n"}}})),
            )
            .await
            .expect("seed");
        st.store
            .create(
                &t,
                Kind::DistSub,
                "csr-1",
                json!({
                    "id": "csr-1", "watchedAttributes": ["old"],
                    "notification": {"attributes": ["old"]}
                }),
            )
            .await
            .expect("seed csr");
        st.store
            .create(
                &t,
                Kind::Registration,
                "urn:ngsi-ld:ContextSourceRegistration:ro",
                json!({
                    "id": "urn:ngsi-ld:ContextSourceRegistration:ro",
                    "endpoint": "http://127.0.0.1:1", "operations": ["retrieveOps"]
                }),
            )
            .await
            .expect("seed reg");
        ds_put(
            &st,
            &t,
            OWN,
            json!({"csr_sub": "csr-1", "remotes": {
                "urn:ngsi-ld:ContextSourceRegistration:gone": ["http://127.0.0.1:1", "r1"],
                "urn:ngsi-ld:ContextSourceRegistration:ro": ["http://127.0.0.1:1", "r2"]
            }}),
        )
        .await;
        on_subscription_updated(&st, &t, OWN).await;
        let csr = st
            .store
            .get(&t, Kind::DistSub, "csr-1")
            .await
            .expect("read")
            .expect("kept");
        assert_eq!(csr["entities"], json!([{"type": "Building"}]));
        assert!(
            csr.get("watchedAttributes").is_none(),
            "a member the update dropped goes: {csr}"
        );
        assert!(csr["notification"].get("attributes").is_none(), "{csr}");
        assert!(csr.get("modifiedAt").is_some());
    }

    /// A Registration Subscription without a notification object is not one
    /// create built: an update leaves it as found.
    #[tokio::test]
    async fn an_update_leaves_a_mapping_without_notification_unstamped() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        st.store
            .create(&t, Kind::Subscription, OWN, sub(json!({})))
            .await
            .expect("seed");
        st.store
            .create(&t, Kind::DistSub, "csr-2", json!({"id": "csr-2"}))
            .await
            .expect("seed csr");
        ds_put(&st, &t, OWN, json!({"csr_sub": "csr-2"})).await;
        on_subscription_updated(&st, &t, OWN).await;
        let csr = st
            .store
            .get(&t, Kind::DistSub, "csr-2")
            .await
            .expect("read")
            .expect("kept");
        assert!(csr.get("modifiedAt").is_none(), "{csr}");
    }

    /// Clause 5.8.5, step 4: a delete drops the mapping, its Registration Subscription
    /// and the inbound index; a registration without deleteSubscription is
    /// not asked to delete its copy.
    #[tokio::test]
    async fn a_delete_drops_the_mapping_and_skips_sources_without_the_operation() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        st.store
            .create(&t, Kind::DistSub, "csr-3", json!({"id": "csr-3"}))
            .await
            .expect("seed csr");
        st.store
            .create(
                &t,
                Kind::Registration,
                "urn:ngsi-ld:ContextSourceRegistration:ro",
                json!({
                    "id": "urn:ngsi-ld:ContextSourceRegistration:ro",
                    "endpoint": "http://127.0.0.1:1", "operations": ["retrieveOps"]
                }),
            )
            .await
            .expect("seed reg");
        inbound_put(&st, "r3", &t, OWN, "digest").await;
        ds_put(
            &st,
            &t,
            OWN,
            json!({"csr_sub": "csr-3", "remotes": {
                "urn:ngsi-ld:ContextSourceRegistration:ro": ["http://127.0.0.1:1", "r3"]
            }}),
        )
        .await;
        on_subscription_deleted(&st, &t, OWN).await;
        assert!(st
            .store
            .get(&t, Kind::DistSub, OWN)
            .await
            .expect("read")
            .is_none());
        assert!(st
            .store
            .get(&t, Kind::DistSub, "csr-3")
            .await
            .expect("read")
            .is_none());
        assert!(
            inbound_get(&st, "r3").await.is_none(),
            "the inbound key is gone"
        );
    }

    /// A notification without a parseable notifiedAt, outside the window or
    /// without an id is never fresh.
    #[test]
    fn a_notification_needs_time_and_id_to_be_fresh() {
        let sid = "urn:ngsi-ld:Subscription:fresh";
        assert!(!notification_fresh(&json!({"id": "urn:n:1"}), sid));
        assert!(!notification_fresh(
            &json!({"id": "urn:n:1", "notifiedAt": "yesterday"}),
            sid
        ));
        assert!(!notification_fresh(
            &json!({"id": "urn:n:1", "notifiedAt": "2001-01-01T00:00:00Z"}),
            sid
        ));
        assert!(!notification_fresh(&json!({"notifiedAt": now_iso()}), sid));
        let n = json!({"id": format!("urn:n:{}", uuid::Uuid::new_v4()), "notifiedAt": now_iso()});
        assert!(notification_fresh(&n, sid));
        assert!(!notification_fresh(&n, sid), "a replay inside the window");
    }

    /// The default @context prefix every Term expands to (5.5.7).
    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

    /// A registration `id` at an endpoint nothing listens on, offering
    /// `ops` over the Attribute `attr`.
    fn unreachable_reg(id: &str, ops: &[&str], attr: &str) -> Value {
        json!({"id": id, "endpoint": "http://127.0.0.1:1", "operations": ops,
               "information": [{"propertyNames": [attr]}]})
    }

    async fn index_ids(st: &AppState) -> Vec<String> {
        let idx = ds_index_tenant().expect("index tenant");
        st.store
            .list(&idx, Kind::DistSub)
            .await
            .expect("index")
            .iter()
            .filter_map(|d| d.get("own").and_then(Value::as_str).map(str::to_owned))
            .collect()
    }

    /// clause 5.8.1, item 4 on a Context Source Notification: registrations without an
    /// id, auxiliary ones, ones without an endpoint, ones not offering the
    /// operation and ones covering nothing the Subscription watches take no
    /// copy; a copy whose forward fails is rolled back; a Subscription with
    /// no mapping document takes none; an update whose forward fails puts
    /// the previous proof back, and one covering nothing is not sent.
    #[tokio::test]
    async fn a_csource_notification_forwards_only_what_it_can() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        let watched = sub(json!({"watchedAttributes": [format!("{DC}height")]}));
        st.store
            .create(&t, Kind::Subscription, OWN, watched.clone())
            .await
            .expect("seed subscription");
        st.store
            .create(&t, Kind::DistSub, OWN, json!({"id": OWN}))
            .await
            .expect("seed mapping");
        on_csource_notification(&st, &t, "urn:ngsi-ld:Subscription:none", None, &[json!({})]).await;
        let create = ["createSubscription"];
        let regs = [
            json!({"endpoint": "http://127.0.0.1:1"}),
            json!({"id": "r-aux", "mode": "auxiliary", "endpoint": "http://127.0.0.1:1",
                   "operations": create}),
            json!({"id": "r-noep", "operations": create}),
            unreachable_reg("r-read", &["retrieveEntity"], "height"),
            unreachable_reg("r-other", &create, "other"),
            unreachable_reg("r-down", &create, "height"),
        ];
        on_csource_notification(&st, &t, OWN, Some("newlyMatching"), &regs).await;
        let mapping = ds_get(&st, &t, OWN).await.expect("mapping");
        assert!(ds_remotes(&mapping).is_empty(), "{mapping}");
        assert!(
            index_ids(&st).await.is_empty(),
            "the failed forward rolled back"
        );

        // a Subscription whose mapping document is gone takes no copy
        let lone = "urn:ngsi-ld:Subscription:lone";
        let mut lone_sub = watched.clone();
        lone_sub["id"] = json!(lone);
        st.store
            .create(&t, Kind::Subscription, lone, lone_sub)
            .await
            .expect("seed subscription");
        on_csource_notification(&st, &t, lone, None, &[regs[5].clone()]).await;
        assert!(st
            .store
            .get(&t, Kind::DistSub, lone)
            .await
            .expect("read")
            .is_none());
        assert!(index_ids(&st).await.is_empty());

        // an update: the forward fails and the previous proof is back; a
        // registration covering nothing watched is sent nothing
        let update = ["updateSubscription"];
        assert!(
            ds_set_remote(
                &st,
                &t,
                OWN,
                "r-up",
                Some(json!(["http://127.0.0.1:1", "urn:remote:up"]))
            )
            .await
        );
        assert!(
            ds_set_remote(
                &st,
                &t,
                OWN,
                "r-up2",
                Some(json!(["http://127.0.0.1:1", "urn:remote:up2"]))
            )
            .await
        );
        inbound_put(&st, "urn:remote:up", &t, OWN, &proof_digest("p0")).await;
        inbound_put(&st, "urn:remote:up2", &t, OWN, &proof_digest("q0")).await;
        let ups = [
            unreachable_reg("r-up", &update, "height"),
            unreachable_reg("r-up2", &update, "other"),
        ];
        on_csource_notification(&st, &t, OWN, Some("updated"), &ups).await;
        let (_, _, proofs) = inbound_get(&st, "urn:remote:up").await.expect("index");
        assert_eq!(proofs, vec![proof_digest("p0")], "the refused update");
        let (_, _, proofs) = inbound_get(&st, "urn:remote:up2").await.expect("index");
        assert_eq!(proofs, vec![proof_digest("q0")], "never rotated");
    }

    /// Two updates that both fail put the previous proof back once: the
    /// second finds nothing more to restore.
    #[tokio::test]
    async fn a_second_failed_update_keeps_the_restored_proof() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        inbound_put(&st, "urn:remote:r", &t, OWN, &proof_digest("p0")).await;
        assert_eq!(index_ids(&st).await, vec![OWN.to_owned()]);
        inbound_rotate(&st, "urn:remote:r", Some(&proof_digest("p1"))).await;
        inbound_rotate(&st, "urn:remote:r", Some(&proof_digest("p2"))).await;
        inbound_rotate(&st, "urn:remote:r", None).await;
        inbound_rotate(&st, "urn:remote:r", None).await;
        let (_, _, proofs) = inbound_get(&st, "urn:remote:r").await.expect("index");
        assert_eq!(proofs, vec![proof_digest("p1")]);
        assert_eq!(
            st.notification_receiver("urn:remote:r").await.as_deref(),
            Some("default")
        );
    }

    /// The replay memory at its cap forgets what left the window first, and
    /// refuses while it stays full of keys inside it.
    #[test]
    fn a_full_replay_memory_forgets_only_what_left_the_window() {
        let mut seen = std::collections::HashMap::new();
        let now = 1_000_000;
        assert!(admit(&mut seen, [1; 16], now - NOTIFY_WINDOW_SECS - 1, 1));
        assert!(
            admit(&mut seen, [2; 16], now, 1),
            "the stale key is forgotten"
        );
        assert!(!admit(&mut seen, [3; 16], now, 1), "full inside the window");
        assert!(!admit(&mut seen, [2; 16], now, 2), "a replay");
        assert!(admit(&mut seen, [3; 16], now, 2));
    }

    /// 5.8.6 merge seams: what is no object or no Entity is dropped, and a
    /// notified fragment is merged with the Entity this broker holds before
    /// the Subscription's conditions are read.
    #[tokio::test]
    async fn the_split_merge_drops_what_is_no_entity_and_merges_the_local_one() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        let b1 = "urn:ngsi-ld:Building:b1";
        st.store
            .create(
                &t,
                Kind::Entity,
                b1,
                json!({"id": b1, "type": [format!("{DC}Building")],
                       format!("{DC}height"): [{"type": "Property", "value": 120}]}),
            )
            .await
            .expect("seed entity");
        let watcher = sub(json!({"q": "height>100"}));
        let data = vec![
            json!(5),
            json!({"type": "Building"}),
            json!({"id": b1, "type": "Building", "name": {"type": "Property", "value": "x"}}),
        ];
        let out = split_merge(&st, &t, &watcher, None, data)
            .await
            .expect("merged");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0]["id"], b1);
        let low = sub(json!({"q": "height>500"}));
        let none = split_merge(
            &st,
            &t,
            &low,
            None,
            vec![json!({"id": b1, "type": "Building"})],
        )
        .await
        .expect("merged");
        assert!(none.is_empty(), "{none:?}");
    }

    /// clause 5.8.1, item 4: a mapping document of the wrong shape is neither stamped
    /// with the Registration Subscription nor replaced by the create.
    #[tokio::test]
    async fn a_create_leaves_a_mapping_of_the_wrong_shape_as_it_was() {
        let st = AppState::new("me".into());
        let t = TenantId::default();
        st.store
            .create(&t, Kind::DistSub, OWN, json!(["not", "a", "mapping"]))
            .await
            .expect("seed");
        on_subscription_created(&st, &t, &sub(json!({}))).await;
        assert_eq!(
            ds_get(&st, &t, OWN).await,
            Some(json!(["not", "a", "mapping"]))
        );
    }

    /// An unread mapping is not an empty one: with the store refusing, no
    /// mapping is read, written, created or torn down, and nothing panics.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn a_refusing_store_changes_nothing() {
        let st = crate::state::refusing_state();
        let t = TenantId::new("refused").expect("tenant");
        assert_eq!(ds_get(&st, &t, OWN).await, None);
        ds_put(&st, &t, OWN, json!({"csr_sub": "x"})).await;
        inbound_put(&st, "urn:remote:refused", &t, OWN, "digest").await;
        on_csource_notification(&st, &t, OWN, None, &[json!({"id": "r"})]).await;
        on_subscription_created(&st, &t, &sub(json!({}))).await;
        on_subscription_updated(&st, &t, OWN).await;
        on_subscription_deleted(&st, &t, OWN).await;
        assert_eq!(ds_get(&st, &t, OWN).await, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Headers carrying the proof `t` the unit-test mappings store.
    fn proven(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(NOTIFY_PROOF_HEADER, token.parse().expect("header value"));
        h
    }

    /// A notification under `sid` with a fresh notifiedAt and a new id.
    fn fresh(sid: &str, data: Value) -> String {
        json!({
            "id": format!("urn:ngsi-ld:Notification:{}", uuid::Uuid::new_v4()),
            "type": "Notification",
            "subscriptionId": sid,
            "notifiedAt": now_iso(),
            "data": data,
        })
        .to_string()
    }

    /// F-ANT3-013: a forwarded notification carries what the local
    /// Subscription projects, not what the source sent.
    #[test]
    fn a_forwarded_notification_takes_the_local_projection() {
        let ctx = antares_jsonld::core_context();
        let sub = json!({"notification": {"attributes": [format!("{DC}speed")]}});
        let data = vec![json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle",
            "speed": {"type": "Property", "value": 1},
            "brand": {"type": "Property", "value": "x"}})];
        let out = project_forwarded(&sub, data, &ctx);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["speed"]["value"], 1, "{}", out[0]);
        assert!(out[0].get("brand").is_none(), "{}", out[0]);
    }

    /// The default @context prefix every Term expands to (5.5.7).
    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

    /// A registration exactly as `on_csource_notification` receives it: out
    /// of a Context Source Notification, whose names and types
    /// `present_registration` has COMPACTED.
    fn reg_doc() -> Value {
        json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:r1",
            "endpoint": "http://source.example.org",
            "operations": ["createSubscription", "updateSubscription", "deleteSubscription"],
            "information": [{
                "entities": [{"type": ["Vehicle"], "id": "urn:ngsi-ld:Vehicle:1"}],
                "propertyNames": ["speed"],
            }],
        })
    }

    /// A Subscription exactly as the store holds it: names and types
    /// EXPANDED by `normalize_subscription` (5.5.7).
    fn sub_doc() -> Value {
        json!({
            "id": "urn:ngsi-ld:Subscription:own",
            "type": "Subscription",
            "entities": [{"type": format!("{DC}Vehicle")}, {"type": format!("{DC}Device")}],
            "watchedAttributes": [format!("{DC}speed"), format!("{DC}brand")],
            "q": "speed>10",
            "geoQ": {"georel": "near;maxDistance==1000"},
            "scopeQ": "/A",
            "localOnly": false,
            "status": "active",
            "timesSent": 7,
            "createdAt": "2026-01-01T00:00:00Z",
            "modifiedAt": "2026-01-02T00:00:00Z",
            "__context": "http://example.org/ctx.jsonld",
            "__via": "1.1 upstream-broker",
            // Table 5.2.14.2-1 keeps the delivery bookkeeping HERE, not at
            // the top level: this is the shape `record_delivery` leaves
            // behind, and the shape a forward is built from.
            "notification": {
                "attributes": [format!("{DC}speed")],
                "pick": ["speed"],
                "omit": ["brand"],
                "endpoint": {"uri": "http://subscriber.example.org/cb"},
                "status": "ok",
                "timesSent": 7,
                "timesFailed": 2,
                "lastNotification": "2026-01-03T00:00:00Z",
                "lastSuccess": "2026-01-03T00:00:00Z",
                "lastFailure": "2026-01-02T00:00:00Z",
            },
        })
    }

    /// A local endpoint recording each request body it is sent.
    async fn capture() -> (String, tokio::sync::mpsc::Receiver<Value>) {
        let (tx, rx) = tokio::sync::mpsc::channel::<Value>(8);
        let app = axum::Router::new().fallback(move |b: Bytes| {
            let tx = tx.clone();
            async move {
                let _ = tx
                    .send(serde_json::from_slice(&b).unwrap_or(Value::Null))
                    .await;
                StatusCode::NO_CONTENT
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(std::future::IntoFuture::into_future(axum::serve(
            listener, app,
        )));
        (format!("http://{addr}"), rx)
    }

    /// A Subscription `own` of the default Tenant, mapped from the remote
    /// copy `remote` under the proof "t", notifying `endpoint`.
    async fn mapped(st: &AppState, remote: &str, own: &str, endpoint: &str, extra: Value) {
        let t = TenantId::default();
        let mut sub = json!({"id": own, "type": "Subscription",
            "entities": [{"type": format!("{DC}Building")}],
            "notification": {"endpoint": {"uri": format!("{endpoint}/notify")}}});
        for (k, v) in extra.as_object().into_iter().flatten() {
            sub[k] = v.clone();
        }
        st.store
            .create(&t, Kind::Subscription, own, sub)
            .await
            .expect("seed subscription");
        inbound_put(st, remote, &t, own, &proof_digest("t")).await;
    }

    /// 5.8.6 on the inbound side: a notification without a subscriptionId is
    /// BadRequestData; a csf that no known origin matches acknowledges and
    /// forwards nothing; the original selector keeps what it selects, read
    /// from a type array as well, and drops an Entity without a type; a
    /// split merge that leaves nothing forwards nothing.
    #[tokio::test]
    async fn clause_5_8_6_inbound_notifications_forward_what_the_subscriber_selected() {
        let st = AppState::new("antares-ds-inbound".into());
        let no_sid = remote_notify(
            State(st.clone()),
            proven("t"),
            Bytes::from(json!({"type": "Notification", "data": []}).to_string()),
        )
        .await;
        assert_eq!(no_sid.status(), StatusCode::BAD_REQUEST);

        let (sink, mut got) = capture().await;
        mapped(
            &st,
            "urn:remote:csf",
            "urn:ngsi-ld:Subscription:csf",
            &sink,
            json!({"csf": "mode==\"inclusive\""}),
        )
        .await;
        let b = json!({"id": "urn:ngsi-ld:Building:1", "type": "Building"});
        let gated = remote_notify(
            State(st.clone()),
            proven("t"),
            Bytes::from(fresh("urn:remote:csf", json!([b]))),
        )
        .await;
        assert_eq!(gated.status(), StatusCode::OK);

        mapped(
            &st,
            "urn:remote:sel",
            "urn:ngsi-ld:Subscription:sel",
            &sink,
            json!({}),
        )
        .await;
        let data = json!([
            {"id": "urn:ngsi-ld:Building:2", "type": ["Building"]},
            {"id": "urn:ngsi-ld:Building:3"},
        ]);
        let kept = remote_notify(
            State(st.clone()),
            proven("t"),
            Bytes::from(fresh("urn:remote:sel", data)),
        )
        .await;
        assert_eq!(kept.status(), StatusCode::OK);
        let sent = tokio::time::timeout(std::time::Duration::from_secs(5), got.recv())
            .await
            .expect("delivered in time")
            .expect("one notification");
        assert_eq!(
            sent["subscriptionId"], "urn:ngsi-ld:Subscription:sel",
            "{sent}"
        );
        let ids: Vec<&str> = sent["data"]
            .as_array()
            .expect("data")
            .iter()
            .filter_map(|e| e["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["urn:ngsi-ld:Building:2"], "{sent}");

        mapped(
            &st,
            "urn:remote:split",
            "urn:ngsi-ld:Subscription:split",
            &sink,
            json!({"splitEntities": true, "q": "height>100"}),
        )
        .await;
        let low = json!([{"id": "urn:ngsi-ld:Building:4", "type": "Building",
            "height": {"type": "Property", "value": 3}}]);
        let none = remote_notify(
            State(st.clone()),
            proven("t"),
            Bytes::from(fresh("urn:remote:split", low)),
        )
        .await;
        assert_eq!(none.status(), StatusCode::OK);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(300), got.recv())
                .await
                .is_err(),
            "the csf-gated and the emptied notifications reach nobody"
        );
    }

    /// 5.8.2: an update whose reduced copy covers nothing the registration
    /// registers sends no PATCH to the Context Source.
    #[tokio::test]
    async fn clause_5_8_2_an_update_covering_nothing_is_not_sent() {
        let st = AppState::new("antares-ds-upd".into());
        let t = TenantId::default();
        let (source, mut got) = capture().await;
        let own = "urn:ngsi-ld:Subscription:upd";
        let sub = json!({"id": own, "type": "Subscription",
            "entities": [{"type": format!("{DC}Building")}],
            "watchedAttributes": [format!("{DC}height")],
            "notification": {"endpoint": {"uri": "http://127.0.0.1:1/notify"}}});
        st.store
            .create(&t, Kind::Subscription, own, sub)
            .await
            .expect("seed subscription");
        st.store
            .create(
                &t,
                Kind::DistSub,
                own,
                json!({"csr_sub": "urn:csr:none", "remotes": {"r-x": [source, "urn:remote:x"]}}),
            )
            .await
            .expect("seed mapping");
        st.store
            .create(
                &t,
                Kind::Registration,
                "r-x",
                json!({"id": "r-x", "endpoint": source, "operations": ["updateSubscription"],
                       "information": [{"propertyNames": [format!("{DC}other")]}]}),
            )
            .await
            .expect("seed registration");
        on_subscription_updated(&st, &t, own).await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(300), got.recv())
                .await
                .is_err(),
            "nothing to update at the source"
        );
    }

    /// F-ANT3-014: a periodic Subscription that watches everything gets no
    /// watchedAttributes in its copy: 5.2.12 forbids the pair with
    /// timeInterval, so the source refused every such copy.
    #[test]
    fn a_periodic_copy_watches_nothing() {
        let st = AppState::new("antares-ds-periodic".into());
        let mut sub = sub_doc();
        let o = sub.as_object_mut().expect("a Subscription is an object");
        o.remove("watchedAttributes");
        o.insert("timeInterval".into(), json!(10));
        let copy = reduced_copy(
            &st,
            &sub,
            &reg_doc(),
            "urn:ngsi-ld:Subscription:remote1",
            &st.loader.core(),
            "t",
        )
        .expect("a copy the registration covers");
        assert_eq!(copy.get("watchedAttributes"), None, "{copy}");
        assert_eq!(copy.get("timeInterval"), Some(&json!(10)));
    }

    /// 5.8.1.4: "a copy of the original Subscription shall be reduced to
    /// what is matched by the registration information … Also from the
    /// notification member, the attributes, pick and omit members are to be
    /// removed. The copied Subscription is then forwarded to the Context
    /// Source as a new Subscription where the notification endpoint is set
    /// to that of the local Broker." Nothing outside the registration's
    /// scope, and no local bookkeeping, may travel with the copy.
    #[test]
    fn clause_5_8_1_reduced_copy_carries_only_the_registration_scope() {
        let st = AppState::new("antares-ds-reduce".into());
        let copy = reduced_copy(
            &st,
            &sub_doc(),
            &reg_doc(),
            "urn:ngsi-ld:Subscription:remote1",
            &st.loader.core(),
            "t",
        )
        .expect("a copy the registration covers");
        assert_eq!(copy["id"], json!("urn:ngsi-ld:Subscription:remote1"));
        assert_ne!(
            copy["id"],
            json!("urn:ngsi-ld:Subscription:own"),
            "the remote must be told the broker-generated id, not the local one"
        );
        let ents = copy["entities"].as_array().expect("entities");
        assert_eq!(ents.len(), 1, "{copy}");
        assert_eq!(ents[0]["type"], json!(format!("{DC}Vehicle")));
        assert!(
            !copy.to_string().contains(&format!("{DC}Device")),
            "a selector the registration does not cover must NOT be forwarded: {copy}"
        );
        assert_eq!(
            copy["watchedAttributes"],
            json!([format!("{DC}speed")]),
            "watchedAttributes are intersected with the registered names"
        );
        // local-only bookkeeping never leaves the broker — the Via chain
        // travels as the HTTP header the forward extends, never in the body
        for k in [
            "status",
            "timesSent",
            "lastNotification",
            "lastSuccess",
            "lastFailure",
            "createdAt",
            "modifiedAt",
            "__context",
            "__via",
            "localOnly",
        ] {
            assert!(copy.get(k).is_none(), "{k} must not be forwarded: {copy}");
        }
        // Table 5.2.14.2-1: the same bookkeeping the store keeps under
        // `notification` is this broker's alone — it says how often IT has
        // notified ITS subscriber, is output-only on a create, and has no
        // meaning to the Context Source receiving the copy.
        for k in [
            "status",
            "timesSent",
            "timesFailed",
            "lastNotification",
            "lastSuccess",
            "lastFailure",
        ] {
            assert!(
                copy["notification"].get(k).is_none(),
                "notification.{k} must not be forwarded: {copy}"
            );
        }
        let n = &copy["notification"];
        for k in ["attributes", "pick", "omit"] {
            assert!(
                n.get(k).is_none(),
                "notification.{k} must be removed: {copy}"
            );
        }
        let uri = n["endpoint"]["uri"].as_str().expect("endpoint uri");
        assert!(uri.ends_with("/ex/v1/remote-notify"), "{uri}");
        assert_ne!(
            uri, "http://subscriber.example.org/cb",
            "the source must never learn the original subscriber's endpoint"
        );
        // splitEntities is absent here, so the filters stay on the copy
        assert_eq!(copy["q"], json!("speed>10"));
    }

    /// 5.8.1.4: "If the splitEntities member is explicitly set to true …
    /// the members q, geoQ and scopeQ shall be removed from the created
    /// copy"; and a Subscription with no watchedAttributes is still reduced
    /// to the registered names, so an unregistered Attribute change cannot
    /// notify through the chain.
    #[test]
    fn clause_5_8_1_reduced_copy_split_and_watch_everything() {
        let st = AppState::new("antares-ds-split".into());
        let mut sub = sub_doc();
        sub["splitEntities"] = json!(true);
        let copy = reduced_copy(
            &st,
            &sub,
            &reg_doc(),
            "urn:ngsi-ld:Subscription:remote2",
            &st.loader.core(),
            "t",
        )
        .expect("copy");
        for k in ["q", "geoQ", "scopeQ"] {
            assert!(
                copy.get(k).is_none(),
                "{k} is evaluated locally after the 5.8.6 merge: {copy}"
            );
        }
        let mut watch_all = sub_doc();
        watch_all
            .as_object_mut()
            .expect("object")
            .remove("watchedAttributes");
        let copy = reduced_copy(
            &st,
            &watch_all,
            &reg_doc(),
            "urn:ngsi-ld:Subscription:remote3",
            &st.loader.core(),
            "t",
        )
        .expect("copy");
        assert_eq!(
            copy["watchedAttributes"],
            json!([format!("{DC}speed")]),
            "a watch-everything Subscription still only watches the \
             registered names at the source"
        );
    }

    /// 5.8.1.4 "reduced to what is matched by the registration information",
    /// with 5.5.7 Term to URI expansion: the registration arrives through a
    /// Context Source Notification (names COMPACTED), the Subscription comes
    /// out of the store (names EXPANDED). The intersection must be taken in
    /// ONE representation — an empty `watchedAttributes` is a payload 5.2.12
    /// forbids, and it narrows the forwarded copy to nothing.
    #[test]
    fn clause_5_8_1_reduced_copy_intersects_across_representations() {
        let st = AppState::new("antares-ds-expand".into());
        let copy = reduced_copy(
            &st,
            &sub_doc(),
            &reg_doc(),
            "urn:ngsi-ld:Subscription:remote4",
            &st.loader.core(),
            "t",
        )
        .expect("a copy the registration covers");
        assert_eq!(
            copy["watchedAttributes"],
            json!([format!("{DC}speed")]),
            "the compacted registered name must be expanded before the \
             intersection: {copy}"
        );
        assert_ne!(
            copy["watchedAttributes"],
            json!([]),
            "5.2.12: watchedAttributes, when present, is a non-empty array"
        );
        assert!(
            !copy.to_string().contains(&format!("{DC}brand")),
            "an unregistered watched name must not be forwarded: {copy}"
        );
        assert_eq!(
            copy["entities"],
            json!([{"type": format!("{DC}Vehicle"), "id": "urn:ngsi-ld:Vehicle:1"}]),
            "the registration's selector travels in the Subscription's own \
             representation: {copy}"
        );
    }

    /// 5.8.1.4: "Based on the content of the Subscription, a Context Source
    /// Registration Subscription shall be created (clause 5.11.2)" — 5.11.2.4
    /// matches registrations on csf, geoQ, scopeQ, temporalQ and the
    /// notification attributes as well, so a Registration Subscription built
    /// from entities and watchedAttributes alone offers the Subscription to
    /// sources the subscriber excluded.
    #[tokio::test]
    async fn clause_5_8_1_csr_subscription_carries_every_matching_member() {
        let st = AppState::new("antares-ds-csr".into());
        let t = TenantId::default();
        let mut sub = sub_doc();
        sub["csf"] = json!("name==\"SourceA\"");
        sub["temporalQ"] = json!({"timerel": "before", "timeAt": "2026-01-01T00:00:00Z"});
        let own = sub["id"].as_str().expect("id").to_owned();
        st.store
            .create(&t, Kind::Subscription, &own, sub.clone())
            .await
            .expect("create");
        on_subscription_created(&st, &t, &sub).await;
        let csr_id = ds_get(&st, &t, &own).await.expect("mapping read")["csr_sub"]
            .as_str()
            .expect("csr_sub")
            .to_owned();
        let csr = st
            .store
            .get(&t, Kind::DistSub, &csr_id)
            .await
            .ok()
            .flatten()
            .expect("csr subscription");
        for k in [
            "entities",
            "watchedAttributes",
            "csf",
            "geoQ",
            "scopeQ",
            "temporalQ",
        ] {
            assert!(
                csr.get(k).is_some(),
                "{k} decides which registrations match: {csr}"
            );
        }
        assert_eq!(
            csr["notification"]["attributes"],
            json!([format!("{DC}speed")]),
            "5.11.2.4 unions notification.attributes into the match spec: {csr}"
        );
        assert!(
            csr.get("q").is_none(),
            "q filters Entity Attributes, not registration properties: {csr}"
        );
    }

    /// 6.3.18: the Via header exists "to avoid infinite loops". A forwarded
    /// Subscription copy (5.8.1.4) arrives with the Via chain of the brokers
    /// it has already passed through; a chain that names THIS broker means
    /// the copy has looped back, so the Subscription serves locally and the
    /// distributed half is NOT created — otherwise two mutually registered
    /// brokers re-forward copies of copies without bound.
    #[tokio::test]
    async fn clause_6_3_18_looping_via_chain_suppresses_the_distributed_half() {
        let st = AppState::new("antares-ds-viahost".into());
        let t = TenantId::default();
        // the copy's chain already names this broker's own alias
        let mut looped = sub_doc();
        looped["__via"] = json!("1.1 sourceX, 1.1 antares-ds-viahost");
        let own = looped["id"].as_str().expect("id").to_owned();
        st.store
            .create(&t, Kind::Subscription, &own, looped.clone())
            .await
            .expect("create");
        on_subscription_created(&st, &t, &looped).await;
        assert!(
            ds_get(&st, &t, &own)
                .await
                .expect("mapping read")
                .get("csr_sub")
                .is_none(),
            "a looped copy must not create the internal Registration Subscription"
        );
        // positive control: a chain naming only OTHER brokers is not a loop
        let mut chained = sub_doc();
        chained["id"] = json!("urn:ngsi-ld:Subscription:chained");
        chained["__via"] = json!("1.1 sourceX");
        st.store
            .create(
                &t,
                Kind::Subscription,
                "urn:ngsi-ld:Subscription:chained",
                chained.clone(),
            )
            .await
            .expect("create");
        on_subscription_created(&st, &t, &chained).await;
        assert!(
            ds_get(&st, &t, "urn:ngsi-ld:Subscription:chained")
                .await
                .expect("mapping read")
                .get("csr_sub")
                .is_some(),
            "a pass-through chain (A->B->C) must keep the distributed half"
        );
    }

    /// 5.8.1.4 gates the whole distributed block on "If localOnly=false": a
    /// Subscription updated to localOnly=true forwards no further copy, and
    /// the internal Context Source Registration Subscription plus the
    /// mappings it already holds are torn down.
    #[tokio::test]
    async fn clause_5_8_1_local_only_flip_tears_the_distributed_half_down() {
        let st = AppState::new("antares-ds-local".into());
        let t = TenantId::default();
        let mut sub = sub_doc();
        sub["localOnly"] = json!(true);
        let own = sub["id"].as_str().expect("id").to_owned();
        let csr_id = "urn:ngsi-ld:CSourceSubscription:distsub:x";
        st.store
            .create(&t, Kind::Subscription, &own, sub.clone())
            .await
            .expect("create");
        st.store
            .create(&t, Kind::DistSub, csr_id, json!({"id": csr_id}))
            .await
            .expect("create");
        ds_put(&st, &t, &own, json!({"csr_sub": csr_id})).await;
        // a Context Source Notification arriving after the flip creates
        // nothing at the source
        on_csource_notification(&st, &t, &own, Some("newlyMatching"), &[reg_doc()]).await;
        assert!(
            ds_remotes(&ds_get(&st, &t, &own).await.expect("mapping read")).is_empty(),
            "a local-only Subscription forwards no copy"
        );
        on_subscription_updated(&st, &t, &own).await;
        assert!(
            st.store
                .get(&t, Kind::DistSub, csr_id)
                .await
                .ok()
                .flatten()
                .is_none(),
            "the internal Registration Subscription must not survive the flip"
        );
        assert!(
            st.store
                .get(&t, Kind::DistSub, &own)
                .await
                .ok()
                .flatten()
                .is_none(),
            "the mapping document must not survive the flip"
        );
    }

    /// 5.8.1.4 stores ONE remote subscriptionId per (Subscription,
    /// registration) pair. Two Context Source Notifications for the same pair
    /// interleave at their forward, so the second insert is refused under the
    /// store's own lock instead of overwriting — an overwritten mapping
    /// orphans a live remote subscription at the source and doubles every
    /// notification the subscriber receives.
    #[tokio::test]
    async fn clause_5_8_1_second_mapping_for_a_registration_is_refused() {
        let st = AppState::new("antares-ds-cas".into());
        let t = TenantId::default();
        let own = "urn:ngsi-ld:Subscription:own";
        ds_put(&st, &t, own, json!({"csr_sub": "urn:csr:1"})).await;
        assert!(
            ds_set_remote(
                &st,
                &t,
                own,
                "urn:reg:1",
                Some(json!(["http://s", "urn:remote:1"]))
            )
            .await
        );
        assert!(
            !ds_set_remote(
                &st,
                &t,
                own,
                "urn:reg:1",
                Some(json!(["http://s", "urn:remote:2"]))
            )
            .await,
            "the registration already has a remote subscription"
        );
        let got = ds_remotes(&ds_get(&st, &t, own).await.expect("mapping read"));
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].1 .1, "urn:remote:1",
            "the first mapping survives — the loser rolls its own copy back"
        );
    }

    /// 4.14: "the Tenant information from the Context Source Registration has
    /// to be used" — the 5.8.5.4 delete-forward must travel with the stored
    /// registration, not a synthetic id/endpoint pair, or it lands in the
    /// peer's default tenant and the remote subscription is never deleted.
    /// The synthetic fallback stays for the already-deleted registration.
    #[test]
    fn clause_5_8_5_delete_forward_carries_the_stored_registration() {
        let stored = json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:r1",
            "endpoint": "http://source.example.org",
            "tenant": "cityB",
            "contextSourceInfo": [{"key": "Authorization", "value": "Bearer t"}],
        });
        let reg = forward_reg(
            Some(stored.clone()),
            "urn:ngsi-ld:ContextSourceRegistration:r1",
            "http://source.example.org",
        );
        assert_eq!(reg["tenant"], json!("cityB"), "{reg}");
        assert_eq!(reg["contextSourceInfo"], stored["contextSourceInfo"]);
        let gone = forward_reg(
            None,
            "urn:ngsi-ld:ContextSourceRegistration:r1",
            "http://source.example.org",
        );
        assert_eq!(gone["endpoint"], json!("http://source.example.org"));
        assert!(
            gone.get("tenant").is_none(),
            "a deleted registration carries nothing to forward with: {gone}"
        );
    }

    /// 5.8.1.4: "The @context to be used for sending Notifications related to
    /// this Subscription shall be the one specified in the jsonldContext
    /// field." The forwarded copy carries the subscriber's own terms (`q` is
    /// stored verbatim), so it must be shipped under the Subscription's
    /// @context — under the core context those terms name Attributes that do
    /// not exist at the source.
    #[test]
    fn clause_5_8_1_forwarded_copy_is_shipped_under_the_subscription_context() {
        let st = AppState::new("antares-ds-ctx".into());
        let core = st.loader.core().source.clone();
        let hosted = json!({"jsonldContext": "http://broker.example.org/jsonldContexts/abc",
                            "__context": "http://example.org/ctx.jsonld"});
        assert_eq!(
            sub_ctx_url(&st, &hosted),
            "http://broker.example.org/jsonldContexts/abc",
            "jsonldContext wins — it is the member 5.8.1.4 names"
        );
        let own = json!({"__context": "http://example.org/ctx.jsonld"});
        assert_eq!(sub_ctx_url(&st, &own), "http://example.org/ctx.jsonld");
        assert_ne!(
            sub_ctx_url(&st, &own),
            crate::federation::ctx_link_url(&HeaderMap::new(), &core),
            "a Subscription with its own vocabulary is not forwarded under \
             the core context"
        );
        assert_eq!(
            sub_ctx_url(&st, &json!({})),
            crate::federation::ctx_link_url(&HeaderMap::new(), &core),
            "with no context of its own the core context is the fallback"
        );
    }

    /// 5.8.1.4 stores three mappings; every store touch takes the tenant
    /// first (4.14: "an NGSI-LD system shall behave as if the tenants were
    /// separate systems"). The inbound index is keyed by the
    /// broker-generated remote subscriptionId under its own reserved
    /// tenant, so it never collides with a tenant's own mapping documents.
    #[tokio::test]
    async fn clause_5_8_1_mappings_are_tenant_scoped() {
        let st = AppState::new("antares-ds-tenant".into());
        let a = TenantId::new("alpha").expect("tenant");
        let b = TenantId::new("beta").expect("tenant");
        let own = "urn:ngsi-ld:Subscription:own";
        ds_put(
            &st,
            &a,
            own,
            json!({"csr_sub": "urn:csr:1",
                   "remotes": {"urn:reg:1": ["http://s", "urn:remote:1"]}}),
        )
        .await;
        assert_eq!(
            ds_remotes(&ds_get(&st, &a, own).await.expect("mapping read")).len(),
            1
        );
        assert!(
            ds_remotes(&ds_get(&st, &b, own).await.expect("mapping read")).is_empty(),
            "another tenant must not read the remote mapping"
        );
        assert!(ds_get(&st, &b, own)
            .await
            .expect("mapping read")
            .get("csr_sub")
            .is_none());
        inbound_put(&st, "urn:remote:1", &a, own, &proof_digest("t")).await;
        assert_eq!(
            inbound_get(&st, "urn:remote:1").await,
            Some((a.clone(), own.to_owned(), vec![proof_digest("t")]))
        );
        assert!(
            st.store
                .get(&a, Kind::DistSub, "urn:remote:1")
                .await
                .ok()
                .flatten()
                .is_none(),
            "the index entry must not land in the subscriber's own namespace"
        );
        inbound_delete(&st, "urn:remote:1").await;
        assert!(inbound_get(&st, "urn:remote:1").await.is_none());
    }

    /// 4.14 — "an NGSI-LD system shall behave as if the tenants were
    /// separate systems" — through the one index every tenant's inbound
    /// mappings share. Its key is the remote subscriptionId the Context
    /// Source echoes back on every notification (5.8.1.4), and the tenant
    /// the index names is the tenant the data is delivered to: nothing later
    /// on that path re-derives it. So a mapping claiming a key another
    /// tenant already holds must lose. An index where the newer write won
    /// would let a Context Source hand one tenant's notifications to another
    /// tenant's subscriber by returning a subscriptionId already in use.
    #[tokio::test]
    async fn clause_5_8_1_an_inbound_key_is_never_taken_from_the_tenant_holding_it() {
        let st = AppState::new("antares-ds-claim".into());
        let a = TenantId::new("alpha").expect("tenant");
        let b = TenantId::new("beta").expect("tenant");
        let shared = "urn:ngsi-ld:Subscription:distsub:collide";
        inbound_put(
            &st,
            shared,
            &a,
            "urn:ngsi-ld:Subscription:a",
            &proof_digest("t"),
        )
        .await;
        inbound_put(
            &st,
            shared,
            &b,
            "urn:ngsi-ld:Subscription:b",
            &proof_digest("t"),
        )
        .await;
        assert_eq!(
            inbound_get(&st, shared).await,
            Some((
                a.clone(),
                "urn:ngsi-ld:Subscription:a".to_owned(),
                vec![proof_digest("t")]
            )),
            "the second tenant took the first tenant's inbound route"
        );
    }

    /// 5.8.2.4 keeps the Registration Subscription in step by mutating the
    /// stored mapping document. That document comes out of the store, and
    /// `Value`'s index panics on anything that is not an object or Null, so
    /// the two shapes a driver can hand back — a mapping that is not an
    /// object, and one whose `notification` member is not an object — decide
    /// whether an update of an ordinary Subscription takes the process down.
    /// The panic would land inside the closure `mutate` runs under the
    /// store's write lock, so it is not one request that is lost.
    #[tokio::test]
    async fn clause_5_8_2_a_stored_mapping_of_the_wrong_shape_is_not_indexed() {
        for stored in [
            json!("not an object"),
            json!([1, 2, 3]),
            json!(7),
            Value::Null,
            json!({"notification": "not an object"}),
            json!({"notification": [1]}),
        ] {
            let st = AppState::new("antares-ds-shape".into());
            let tenant = TenantId::new("shape").expect("tenant");
            let own = "urn:ngsi-ld:Subscription:shape";
            st.store
                .create(
                    &tenant,
                    Kind::Subscription,
                    own,
                    json!({"id": own, "type": "Subscription",
                           "entities": [{"type": "Vehicle"}],
                           "notification": {"endpoint": {"uri": "http://127.0.0.1:9/n"},
                                            "attributes": ["speed"]}}),
                )
                .await
                .expect("seed the subscription");
            ds_put(&st, &tenant, own, json!({"csr_sub": "urn:csr:shape"})).await;
            st.store
                .create(&tenant, Kind::DistSub, "urn:csr:shape", stored.clone())
                .await
                .expect("seed the mapping");

            on_subscription_updated(&st, &tenant, own).await;

            // the update completes, and a mapping that could not be brought
            // into step is left as it was rather than half-written
            let after = st
                .store
                .get(&tenant, Kind::DistSub, "urn:csr:shape")
                .await
                .expect("read the mapping");
            assert!(after.is_some(), "{stored}: the mapping was dropped");
        }
    }

    /// The same path on a well-formed mapping still carries the update
    /// through: the guard above refuses a shape, it does not refuse the work.
    #[tokio::test]
    async fn clause_5_8_2_a_well_formed_mapping_is_brought_into_step() {
        let st = AppState::new("antares-ds-step".into());
        let tenant = TenantId::new("step").expect("tenant");
        let own = "urn:ngsi-ld:Subscription:step";
        st.store
            .create(
                &tenant,
                Kind::Subscription,
                own,
                json!({"id": own, "type": "Subscription",
                       "entities": [{"type": "Bus"}],
                       "scopeQ": "/a/b",
                       "notification": {"endpoint": {"uri": "http://127.0.0.1:9/n"},
                                        "attributes": ["speed"]}}),
            )
            .await
            .expect("seed the subscription");
        ds_put(&st, &tenant, own, json!({"csr_sub": "urn:csr:step"})).await;
        st.store
            .create(
                &tenant,
                Kind::DistSub,
                "urn:csr:step",
                json!({"id": "urn:csr:step", "type": "Subscription",
                       "entities": [{"type": "Tram"}],
                       "notification": {"endpoint": {"uri": "urn:antares:distsub:step"}}}),
            )
            .await
            .expect("seed the mapping");

        on_subscription_updated(&st, &tenant, own).await;

        let after = st
            .store
            .get(&tenant, Kind::DistSub, "urn:csr:step")
            .await
            .ok()
            .flatten()
            .expect("the mapping is still there");
        assert_eq!(
            after["entities"],
            json!([{"type": "Bus"}]),
            "the match members follow the Subscription: {after}"
        );
        assert_eq!(after["scopeQ"], "/a/b", "{after}");
        assert_eq!(
            after["notification"]["attributes"],
            json!(["speed"]),
            "5.8.2.4: the notification attributes follow too: {after}"
        );
        assert!(
            after["notification"]["endpoint"]["uri"]
                .as_str()
                .is_some_and(|u| u.starts_with("urn:antares:distsub:")),
            "the endpoint the copy points at is not rewritten: {after}"
        );
    }

    /// The remotes index is read back from storage, so every malformed
    /// shape is dropped rather than indexed or unwrapped.
    #[test]
    fn clause_5_8_1_remotes_index_drops_malformed_entries() {
        let doc = json!({"remotes": {
            "urn:reg:ok": ["http://s", "urn:remote:1"],
            "urn:reg:short": ["http://s"],
            "urn:reg:nonstring": [1, 2],
            "urn:reg:notarray": "http://s",
            "urn:reg:empty": [],
        }});
        let got = ds_remotes(&doc);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "urn:reg:ok");
        assert_eq!(got[0].1 .1, "urn:remote:1");
        assert!(ds_remotes(&json!({"remotes": "nope"})).is_empty());
        assert!(ds_remotes(&json!({})).is_empty());
    }

    /// 5.8.1.4 / 5.8.5.4: the per-registration branches interleave at their
    /// forward, so one registration's mapping write must not drop another's,
    /// and a mapping document deleted by Delete Subscription must stay
    /// deleted — an in-flight branch may not write it back.
    #[tokio::test]
    async fn clause_5_8_5_mapping_writes_never_resurrect_a_deleted_subscription() {
        let st = AppState::new("antares-ds-resurrect".into());
        let t = TenantId::default();
        let own = "urn:ngsi-ld:Subscription:own";
        ds_put(&st, &t, own, json!({"csr_sub": "urn:csr:1"})).await;
        assert!(
            ds_set_remote(
                &st,
                &t,
                own,
                "urn:reg:1",
                Some(json!(["http://s1", "urn:remote:1"]))
            )
            .await
        );
        assert!(
            ds_set_remote(
                &st,
                &t,
                own,
                "urn:reg:2",
                Some(json!(["http://s2", "urn:remote:2"]))
            )
            .await
        );
        assert_eq!(
            ds_remotes(&ds_get(&st, &t, own).await.expect("mapping read")).len(),
            2,
            "a second registration's write must not drop the first"
        );
        assert!(ds_set_remote(&st, &t, own, "urn:reg:1", None).await);
        assert_eq!(
            ds_remotes(&ds_get(&st, &t, own).await.expect("mapping read")).len(),
            1
        );
        let _ = st.store.delete(&t, Kind::DistSub, own).await;
        assert!(
            !ds_set_remote(
                &st,
                &t,
                own,
                "urn:reg:3",
                Some(json!(["http://s3", "urn:remote:3"]))
            )
            .await,
            "there is no mapping document left to write to"
        );
        assert!(
            st.store
                .get(&t, Kind::DistSub, own)
                .await
                .ok()
                .flatten()
                .is_none(),
            "a deleted mapping document must stay deleted"
        );
    }

    /// The inbound notification endpoint is peer-facing: the Entity count is
    /// capped before any store touch, because the 5.8.6 merge runs one local
    /// retrieve and one federated fan-out per notified Entity.
    #[tokio::test]
    async fn clause_5_8_6_inbound_notification_entity_count_is_capped() {
        let st = AppState::new("antares-ds-cap".into());
        let cap = *crate::bounds::MAX_BATCH_ITEMS;
        let body = |n: usize| {
            json!({
                "type": "Notification",
                "subscriptionId": "urn:ngsi-ld:Subscription:remote1",
                "data": vec![json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle"}); n],
            })
            .to_string()
        };
        let over = remote_notify(
            State(st.clone()),
            HeaderMap::new(),
            Bytes::from(body(cap + 1)),
        )
        .await;
        assert_eq!(
            over.status(),
            StatusCode::BAD_REQUEST,
            "an over-cap notification is rejected before the mapping lookup"
        );
        let at_cap =
            remote_notify(State(st.clone()), HeaderMap::new(), Bytes::from(body(cap))).await;
        assert_eq!(
            at_cap.status(),
            StatusCode::UNAUTHORIZED,
            "the ceiling itself is accepted — the unknown mapping is what stops it"
        );
    }

    /// 5.8.1.4 stores the mapping "to enable forwarding received
    /// notifications to the original subscriber": once that subscriber is
    /// gone the mapping is dead weight, so it is pruned on touch, and the
    /// peer is answered about its own id — never told the local one.
    #[tokio::test]
    async fn clause_5_8_1_mapping_to_a_deleted_subscription_is_pruned() {
        let st = AppState::new("antares-ds-prune".into());
        let t = TenantId::new("alpha").expect("tenant");
        let remote = "urn:ngsi-ld:Subscription:remote9";
        inbound_put(
            &st,
            remote,
            &t,
            "urn:ngsi-ld:Subscription:gone",
            &proof_digest("t"),
        )
        .await;
        let body = fresh(remote, json!([]));
        let resp = remote_notify(State(st.clone()), proven("t"), Bytes::from(body)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(
            inbound_get(&st, remote).await.is_none(),
            "the dead mapping must not survive the touch"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("urn:ngsi-ld:Subscription:gone"),
            "the local Subscription id must not leak to the peer: {text}"
        );
    }

    /// 5.8.1.4 consumer half, as a seam: the delivery path hands a
    /// notification on the internal endpoint to the handler `wire` installs
    /// and to nothing else. An unwired state has no handler, so such a
    /// notification is dropped rather than delivered somewhere — the seam
    /// fails closed, and a caller cannot reach `distsub` by bypassing it.
    #[tokio::test]
    async fn the_internal_endpoint_is_reached_only_through_the_installed_handler() {
        let bare = AppState::new("antares".into());
        assert!(
            bare.csource_notification.is_none(),
            "a state nobody wired carries no handler"
        );
        let wired = crate::wired_state("antares");
        assert!(
            wired.await.csource_notification.is_some(),
            "wire installs the consumer half"
        );
    }

    /// 5.8.6 forwards an inbound notification to the original subscriber,
    /// and that delivery is a notification like any other: it passes
    /// `pre_notify` once, under the subject stored with the ORIGINAL
    /// Subscription, so a narrowing the subscriber is under holds for data a
    /// remote source sent (operation matrix section 8 item 13, T-3486).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_remote_notification_is_narrowed_for_the_original_subscriber() {
        use crate::policy::{
            Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
        };
        use std::sync::{Arc, Mutex};

        /// Strips `secret` from every notified Entity and records the
        /// subject it was asked about.
        struct OmitSecret(Mutex<Vec<Vec<(String, String)>>>);
        impl PolicyEngine for OmitSecret {
            fn name(&self) -> &str {
                "omit-secret"
            }
            fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
                Box::pin(std::future::ready(Decision::Allow))
            }
            fn pre_notify(&self, s: &Subject, _sub: &Value, n: &mut Value) -> NotifyDecision {
                self.0.lock().expect("subjects").push(s.headers.clone());
                for e in n
                    .get_mut("data")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    e.as_object_mut().map(|o| o.remove("secret"));
                }
                NotifyDecision::Deliver
            }
        }

        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sink = seen.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/notify",
            axum::routing::post(move |body: String| {
                let sink = sink.clone();
                async move {
                    let v = serde_json::from_str::<Value>(&body).expect("a JSON notification");
                    sink.lock().expect("bodies").push(v);
                    StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

        let engine = Arc::new(OmitSecret(Mutex::default()));
        let st = AppState::new("antares-ds-narrow".into()).with_policy(engine.clone());
        let t = TenantId::new("alpha").expect("tenant");
        let own = "urn:ngsi-ld:Subscription:own-narrowed";
        st.store
            .create(
                &t,
                Kind::Subscription,
                own,
                json!({
                    "id": own,
                    "type": "Subscription",
                    "entities": [{"type": format!("{DC}Vehicle")}],
                    "notification": {"endpoint": {
                        "uri": format!("http://{addr}/notify"),
                        "accept": "application/json",
                    }},
                    crate::policy::SUBJECT_MEMBER: [["x-user", "alice"]],
                }),
            )
            .await
            .expect("subscription row");
        let remote = "urn:ngsi-ld:Subscription:remote-narrowed";
        inbound_put(&st, remote, &t, own, &proof_digest("t")).await;

        let body = fresh(
            remote,
            json!([{
                "id": "urn:ngsi-ld:Vehicle:1",
                "type": "Vehicle",
                "speed": {"type": "Property", "value": 50},
                "secret": {"type": "Property", "value": "hidden"},
            }]),
        );
        let resp = remote_notify(State(st.clone()), proven("t"), Bytes::from(body)).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let mut delivered = None;
        for _ in 0..100 {
            if let Some(b) = seen.lock().expect("bodies").first().cloned() {
                delivered = Some(b);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let delivered = delivered.expect("the original subscriber was notified");
        let entity = &delivered["data"][0];
        assert_eq!(entity["id"], "urn:ngsi-ld:Vehicle:1");
        assert!(
            entity.get("speed").is_some(),
            "visible data travels: {delivered}"
        );
        assert!(
            entity.get("secret").is_none(),
            "the subscriber's narrowing holds for remote data: {delivered}"
        );
        assert_eq!(
            delivered["subscriptionId"], own,
            "delivered under the subscriber's own id"
        );
        assert_eq!(
            *engine.0.lock().expect("subjects"),
            vec![vec![("x-user".to_owned(), "alice".to_owned())]],
            "asked once, about the original subscriber"
        );
        assert_eq!(engine.name(), "omit-secret");
        let probe = Subject {
            tenant: TenantId::default(),
            headers: Vec::new(),
        };
        assert!(matches!(
            engine.decide(&probe, &Operation::new("5.7.1")).await,
            Decision::Allow
        ));
    }

    /// T-7421: a proof names one mapping. The proof of copy A sent under the
    /// subscription id of copy B is refused like no proof at all, and the
    /// previous proof of a rotated copy still passes until the next rotation.
    #[tokio::test]
    async fn remote_notify_proof_belongs_to_its_own_mapping() {
        let st = AppState::new("antares-ds-proof".into());
        let t = TenantId::new("alpha").expect("tenant");
        let (ra, rb) = (
            "urn:ngsi-ld:Subscription:remote-a",
            "urn:ngsi-ld:Subscription:remote-b",
        );
        inbound_put(
            &st,
            ra,
            &t,
            "urn:ngsi-ld:Subscription:gone-a",
            &proof_digest("ta"),
        )
        .await;
        inbound_put(
            &st,
            rb,
            &t,
            "urn:ngsi-ld:Subscription:gone-b",
            &proof_digest("tb"),
        )
        .await;
        let wrong = remote_notify(
            State(st.clone()),
            proven("ta"),
            Bytes::from(fresh(rb, json!([]))),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        inbound_rotate(&st, ra, Some(&proof_digest("ta2"))).await;
        let (_, _, proofs) = inbound_get(&st, ra).await.expect("mapping");
        assert_eq!(proofs, vec![proof_digest("ta2"), proof_digest("ta")]);
        inbound_rotate(&st, ra, None).await;
        let (_, _, proofs) = inbound_get(&st, ra).await.expect("mapping");
        assert_eq!(
            proofs,
            vec![proof_digest("ta")],
            "a failed update restores the old proof"
        );
        // the right proof reaches the mapping: its subscriber is gone, so 404
        let right = remote_notify(
            State(st.clone()),
            proven("ta"),
            Bytes::from(fresh(ra, json!([]))),
        )
        .await;
        assert_eq!(right.status(), StatusCode::NOT_FOUND);
    }

    /// T-7421: no stored row or index entry holds a token that notifies.
    #[tokio::test]
    async fn remote_notify_index_stores_a_digest_never_the_token() {
        let st = AppState::new("antares-ds-digest".into());
        let t = TenantId::new("alpha").expect("tenant");
        let token = notify_token();
        assert_eq!(token.len(), 64);
        inbound_put(&st, "urn:r", &t, "urn:own", &proof_digest(&token)).await;
        let idx = ds_index_tenant().expect("index tenant");
        let row = st
            .store
            .get(&idx, Kind::DistSub, "urn:r")
            .await
            .expect("read")
            .expect("row");
        assert!(!row.to_string().contains(&token), "{row}");
        let sub = json!({"id": "urn:own", "type": "Subscription",
            "notification": {"endpoint": {"uri": "http://subscriber.example.org/cb"}}});
        let reg = json!({"id": "urn:reg", "information": [{"propertyNames": ["speed"]}]});
        let copy = reduced_copy(&st, &sub, &reg, "urn:r", &st.loader.core(), &token)
            .expect("a copy the registration covers");
        assert_eq!(
            copy["notification"]["endpoint"]["receiverInfo"],
            json!([{"key": NOTIFY_PROOF_HEADER, "value": token}])
        );
        assert_eq!(
            copy["entities"],
            json!(null),
            "a registration without entities leaves the selectors as they were"
        );
    }
}
