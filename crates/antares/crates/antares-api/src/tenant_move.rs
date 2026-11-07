// SPDX-License-Identifier: EUPL-1.2
//! Tenant export, import and digest (ADR-0030): what a platform needs to move
//! one Tenant to another broker with its system attributes and its history,
//! and to prove the copy from both sides. Admin surface only (`/q/`).

use crate::negotiate::ApiError;
use crate::state::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::Kind;
use axum::body::{Body, Bytes};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The longest line an import reads: one stored document.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
/// Documents per store page while exporting.
const PAGE: usize = 500;
/// The format the header line names.
const FORMAT: u64 = 1;

/// The Tenant a path names, or the `400` of a name no request could carry.
fn tenant_of(raw: &str) -> Result<TenantId, Box<Response>> {
    let bad = || {
        Box::new(
            ApiError::from(NgsiError::BadRequestData(format!(
                "invalid tenant: {raw:?}"
            )))
            .into_response(),
        )
    };
    TenantId::new(raw).map_err(|_| bad())
}

/// Adds `b` into `a`, both big-endian 256-bit numbers, modulo 2²⁵⁶.
fn add_into(a: &mut [u8; 32], b: &[u8]) {
    let mut carry = false;
    for (x, y) in a.iter_mut().rev().zip(b.iter().rev()) {
        let (sum, over) = x.overflowing_add(*y);
        let (sum, over_carry) = sum.overflowing_add(u8::from(carry));
        *x = sum;
        carry = over || over_carry;
    }
}

/// The multiset hash of (id, `modifiedAt`) pairs: the order they come in
/// does not change it (ADR-0030 rule 2).
pub fn digest_of<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> (u64, String) {
    let mut sum = [0u8; 32];
    let mut count = 0u64;
    for (id, modified) in pairs {
        let mut h = Sha256::new();
        h.update(id.as_bytes());
        h.update([0u8]);
        h.update(modified.as_bytes());
        add_into(&mut sum, &h.finalize());
        count = count.saturating_add(1);
    }
    (count, sum.iter().map(|b| format!("{b:02x}")).collect())
}

fn pair(doc: &Value) -> (String, String) {
    let text = |k: &str| {
        doc.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    (text("id"), text("modifiedAt"))
}

/// `GET /q/tenants/{tenant}/digest`.
pub async fn digest(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> Response {
    let tenant = match tenant_of(&raw) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    match st.store.tenant_exists(&tenant).await {
        Ok(true) => {}
        Ok(false) => return not_found(&raw),
        Err(e) => return ApiError::from(e).into_response(),
    }
    let mut pairs = Vec::new();
    // ponytail: the pairs of one Tenant are held while hashing (two short
    // strings an Entity); fold per page if a Tenant outgrows that.
    if let Err(e) = crate::csource::walk_docs(&st, &tenant, Kind::Entity, |doc| {
        pairs.push(pair(&doc));
        Ok(())
    })
    .await
    {
        return ApiError::from(e).into_response();
    }
    let (entities, sha256) = digest_of(pairs.iter().map(|(i, m)| (i.as_str(), m.as_str())));
    let instances = match st.temporal.attr_instance_count(&tenant).await {
        Ok(n) => n,
        Err(e) => return ApiError::from(e).into_response(),
    };
    let body =
        json!({"tenant": raw, "entities": entities, "attrInstances": instances, "sha256": sha256});
    (StatusCode::OK, axum::Json(body)).into_response()
}

fn not_found(raw: &str) -> Response {
    ApiError::from(NgsiError::ResourceNotFound(format!("tenant {raw}"))).into_response()
}

/// What v1 does not move, by name and count (ADR-0030 rule 5).
fn unmovable(stats: &antares_store::TenantStats) -> Vec<String> {
    [
        ("subscriptions", stats.subscriptions),
        ("registrations", stats.registrations),
        ("csourceSubscriptions", stats.csource_subscriptions),
        ("snapshots", stats.snapshots),
        ("distSubs", stats.dist_subs),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .map(|(name, n)| format!("{n} {name}"))
    .collect()
}

/// `GET /q/tenants/{tenant}/export`: NDJSON, header first, trailer last.
pub async fn export(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> Response {
    let tenant = match tenant_of(&raw) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let stats = match st.store.tenant_stats_one(&tenant).await {
        Ok(Some(s)) => s,
        Ok(None) => return not_found(&raw),
        Err(e) => return ApiError::from(e).into_response(),
    };
    let held = unmovable(&stats);
    if !held.is_empty() {
        return ApiError::from(NgsiError::Conflict(format!(
            "the tenant holds {}, which an export does not carry (ADR-0030 rule 5)",
            held.join(", ")
        )))
        .into_response();
    }
    // The walk below pages history out of the current-state driver, which
    // holds it only when one instance serves both seams; digest and import
    // use `st.temporal`. A second backend would export no history at all.
    if let Some(second) = st.temporal_name.as_deref().filter(|t| *t != st.store_name) {
        return ApiError::from(NgsiError::Conflict(format!(
            "the history lives in a second backend (ANTARES_TEMPORAL={second}), which an \
             export does not carry; export from a broker whose store holds the history"
        )))
        .into_response();
    }
    let header_line = json!({"antaresExport": FORMAT, "tenant": raw});
    // One page per poll: the stream holds a page, never the Tenant.
    struct Walk {
        st: AppState,
        tenant: TenantId,
        kinds: Vec<(Kind, &'static str)>,
        after: Option<String>,
        records: u64,
        done: bool,
    }
    let walk = Walk {
        st,
        tenant,
        kinds: vec![(Kind::Temporal, "temporal"), (Kind::Entity, "entity")],
        after: None,
        records: 0,
        done: false,
    };
    let body =
        futures_util::stream::once(async move { Ok::<Bytes, NgsiError>(line(&header_line)) })
            .chain(futures_util::stream::unfold(walk, |mut w| async move {
                if w.done {
                    return None;
                }
                let Some(&(kind, name)) = w.kinds.last() else {
                    w.done = true;
                    let trailer = line(&json!({"end": true, "records": w.records}));
                    return Some((Ok(trailer), w));
                };
                let page = match w
                    .st
                    .store
                    .list_page(&w.tenant, kind, w.after.as_deref(), PAGE)
                    .await
                {
                    Ok(p) => p,
                    // A failed page ends the stream without a trailer: the
                    // reader sees a cut stream, never a short complete one.
                    Err(e) => {
                        w.done = true;
                        return Some((Err(e), w));
                    }
                };
                let before = w.after.clone();
                let mut out = Vec::new();
                for doc in &page {
                    if let Some(id) = doc.get("id").and_then(Value::as_str) {
                        w.after = Some(id.to_owned());
                    }
                    out.extend_from_slice(&line(&json!({"kind": name, "doc": doc})));
                    w.records = w.records.saturating_add(1);
                }
                if page.len() < PAGE || w.after == before {
                    w.kinds.pop();
                    w.after = None;
                }
                Some((Ok(Bytes::from(out)), w))
            }));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(
            body.map(|r| r.map_err(std::io::Error::other)),
        ))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn line(v: &Value) -> Bytes {
    let mut out = serde_json::to_vec(v).unwrap_or_default();
    out.push(b'\n');
    Bytes::from(out)
}

/// What the Tenant holds in any kind, by name and count: an import takes a
/// Tenant that holds nothing, so a failed one can never touch data it did
/// not write (T-4016).
async fn occupied(st: &AppState, tenant: &TenantId) -> Result<Vec<String>, NgsiError> {
    let mut held = match st.store.tenant_stats_one(tenant).await? {
        Some(stats) => {
            let mut held = unmovable(&stats);
            for (name, n) in [
                ("entities", stats.entities),
                ("entityMaps", stats.entity_maps),
            ] {
                if n > 0 {
                    held.push(format!("{n} {name}"));
                }
            }
            held
        }
        None => Vec::new(),
    };
    let instances = st.temporal.attr_instance_count(tenant).await?;
    if instances > 0 {
        held.push(format!("{instances} history instances"));
    }
    let contexts = st
        .store
        .context_list_meta(Some(tenant))
        .await?
        .iter()
        .filter(|row| row.get("owner").and_then(Value::as_str) == Some(tenant.as_str()))
        .count();
    if contexts > 0 {
        held.push(format!("{contexts} @contexts"));
    }
    Ok(held)
}

/// `POST /q/tenants/{tenant}/import`: an export into a Tenant that holds
/// nothing; all or nothing (ADR-0030 rule 4).
pub async fn import(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
    body: Body,
) -> Response {
    let tenant = match tenant_of(&raw) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let existed = match st.store.tenant_exists(&tenant).await {
        Ok(b) => b,
        Err(e) => return ApiError::from(e).into_response(),
    };
    match occupied(&st, &tenant).await {
        Ok(held) if held.is_empty() => {}
        Ok(held) => {
            return ApiError::from(NgsiError::Conflict(format!(
                "the tenant already holds {}; an import takes an empty tenant",
                held.join(", ")
            )))
            .into_response()
        }
        Err(e) => return ApiError::from(e).into_response(),
    }
    let mut written = Vec::new();
    match take(&st, &tenant, &raw, body, &mut written).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            // Undo by id: only the Entities this import wrote and their
            // history, never what another writer put in meanwhile.
            for id in &written {
                let _ = st.store.delete(&tenant, Kind::Entity, id).await;
                let _ = st.temporal.delete(&tenant, id).await;
            }
            // A Tenant this import created goes again, once nothing is left
            // in it: an emptied one would still list.
            if !existed && matches!(occupied(&st, &tenant).await, Ok(held) if held.is_empty()) {
                let _ = st.store.purge_tenant(&tenant).await;
                st.known_tenants.forget(&tenant);
            }
            ApiError::from(e).into_response()
        }
    }
}

async fn take(
    st: &AppState,
    tenant: &TenantId,
    raw: &str,
    body: Body,
    written: &mut Vec<String>,
) -> Result<(), NgsiError> {
    let bad = |n: usize, why: &str| NgsiError::BadRequestData(format!("line {n}: {why}"));
    let mut stream = body.into_data_stream();
    let mut buf: Vec<u8> = Vec::new();
    // Bytes of `buf` already searched for a line break: a long line that
    // arrives in many chunks is scanned once, not once a chunk.
    let mut scanned = 0usize;
    let mut n = 0usize;
    let mut header_seen = false;
    let mut ended = false;
    let mut records = 0u64;
    let mut in_history = false;
    let mut with_history = std::collections::HashSet::new();
    loop {
        let next = stream.next().await;
        let eof = next.is_none();
        if let Some(chunk) = next {
            let chunk =
                chunk.map_err(|e| NgsiError::BadRequestData(format!("the body was cut: {e}")))?;
            buf.extend_from_slice(&chunk);
        }
        while let Some(end) = buf
            .get(scanned..)
            .and_then(|rest| rest.iter().position(|b| *b == b'\n'))
            .map(|at| at.saturating_add(scanned))
        {
            scanned = 0;
            let mut text: Vec<u8> = buf.drain(..=end).collect();
            text.pop();
            n = n.saturating_add(1);
            if text.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if ended {
                return Err(bad(n, "a record after the trailer"));
            }
            let record: Value =
                serde_json::from_slice(&text).map_err(|e| bad(n, &format!("not JSON: {e}")))?;
            if !header_seen {
                if record.get("antaresExport").and_then(Value::as_u64) != Some(FORMAT) {
                    return Err(bad(n, "the first line is not an export header of format 1"));
                }
                if record.get("tenant").and_then(Value::as_str) != Some(raw) {
                    return Err(bad(n, "the export is of another tenant"));
                }
                header_seen = true;
                continue;
            }
            if record.get("end") == Some(&Value::Bool(true)) {
                if record.get("records").and_then(Value::as_u64) != Some(records) {
                    return Err(bad(n, "the trailer counts a different number of records"));
                }
                ended = true;
                continue;
            }
            let doc = record.get("doc").cloned().unwrap_or(Value::Null);
            let Some(id) = doc.get("id").and_then(Value::as_str).map(str::to_owned) else {
                return Err(bad(n, "a record without a document id"));
            };
            match record.get("kind").and_then(Value::as_str) {
                Some("entity") => {
                    if in_history {
                        return Err(bad(n, "an entity after the history: entities come first"));
                    }
                    if !st.store.create(tenant, Kind::Entity, &id, doc).await? {
                        return Err(bad(n, &format!("entity {id} appears twice")));
                    }
                    written.push(id);
                }
                // After every Entity: writing one records history through
                // the change hook, which the exported history then replaces.
                Some("temporal") => {
                    in_history = true;
                    st.temporal.upsert(tenant, &id, doc).await?;
                    with_history.insert(id.clone());
                    written.push(id);
                }
                _ => return Err(bad(n, "a record of an unknown kind")),
            }
            records = records.saturating_add(1);
        }
        scanned = buf.len();
        if buf.len() > MAX_LINE_BYTES {
            return Err(bad(n.saturating_add(1), "longer than 16 MiB"));
        }
        if eof {
            break;
        }
    }
    if !buf.iter().all(u8::is_ascii_whitespace) {
        return Err(bad(n.saturating_add(1), "the last line has no line break"));
    }
    if !ended {
        return Err(NgsiError::BadRequestData(
            "the export has no trailer: the stream was cut".into(),
        ));
    }
    // An Entity the export carried no history for had none at the source:
    // drop what its create recorded through the change hook, or the target
    // digests more attribute instances than the source.
    for id in written.iter().filter(|id| !with_history.contains(*id)) {
        st.temporal.delete(tenant, id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_does_not_depend_on_order_and_sees_every_pair() {
        let a = [
            ("urn:a", "2026-01-01T00:00:00Z"),
            ("urn:b", "2026-01-02T00:00:00Z"),
        ];
        let b = [a[1], a[0]];
        assert_eq!(digest_of(a), digest_of(b));
        assert_eq!(digest_of(a).0, 2);
        assert_ne!(
            digest_of(a),
            digest_of([a[0], ("urn:b", "2026-01-03T00:00:00Z")])
        );
        // the separator keeps (ab, c) apart from (a, bc)
        assert_ne!(digest_of([("ab", "c")]), digest_of([("a", "bc")]));
        assert_eq!(digest_of([]), (0, "0".repeat(64)));
    }

    #[test]
    fn the_sum_wraps_modulo_two_to_the_256() {
        let mut a = [0xffu8; 32];
        let mut one = [0u8; 32];
        one[31] = 1;
        add_into(&mut a, &one);
        assert_eq!(a, [0u8; 32]);
    }
}
