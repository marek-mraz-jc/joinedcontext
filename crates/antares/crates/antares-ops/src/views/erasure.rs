// SPDX-License-Identifier: EUPL-1.2
//! `/q/erasures/{tenant}`: erasure of personal data (T-4665, platform
//! ADR-N-054 2.6, GDPR Art. 17).
//!
//! `POST` names Entities, and optionally the Attributes of theirs that hold
//! the person's values; without `attrs` the whole Entity goes. The broker
//! removes them from the current state, from the history (every version)
//! and from the dead letters that carry them, then keeps one content-free
//! record per erasure beside the retention documents: the Entities' ids, the
//! Attribute names, the subject's keyed hash as the Portal sends it, who,
//! why, when and the counts, never a value. That record is the tombstone,
//! and the erasure log: each pod re-applies every record when it starts, so
//! a restored database serves none of what was erased before the restore.
//!
//! The admin surface's credential is the dedicated scope: the Portal holds
//! it and sends an erasure only once the space's Policy granted the caller
//! `personalData`. The Portal finds the Entities naming a data subject with
//! the subject's `q` over current state and history, and erases them here.

use super::{bad_tenant, problem, serve, Reloader};
use antares_api::erasure::erase as apply;
use antares_model::{EntityId, TenantId};
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Weak};

/// The id prefix an erasure record is stored under, beside the view
/// documents: `erasure:{tenant}:{id}`.
pub const ERASURE_ID_PREFIX: &str = "erasure:";
/// The most Entities one erasure names.
pub const MAX_ENTITIES: usize = 1000;
/// The largest erasure request, in bytes.
const MAX_BYTES: usize = 256 * 1024;
const MAX_REASON: usize = 500;
const MAX_ACTOR: usize = 200;
const MAX_SUBJECT: usize = 128;

/// One erasure as the Portal asks for it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    entities: Vec<String>,
    #[serde(default)]
    attrs: Option<Vec<String>>,
    /// The data subject's keyed hash, for the log only.
    #[serde(default)]
    subject: Option<String>,
    actor: String,
    reason: String,
}

pub(super) fn routes(r: &Weak<Reloader>) -> Router {
    let (write, read) = (r.clone(), r.clone());
    Router::new().route(
        "/erasures/{tenant}",
        post(move |Path(t): Path<String>, body: Bytes| serve(write.clone(), |r| erase(r, t, body)))
            .get(move |Path(t): Path<String>| serve(read.clone(), |r| list(r, t))),
    )
}

fn refused(detail: &str) -> Response {
    problem(StatusCode::BAD_REQUEST, "BadRequestData", detail)
}

/// Non-empty, at most `max` characters, no control character.
fn plain(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control)
}

/// Why `req` cannot run, if it cannot.
fn invalid(req: &Request) -> Option<String> {
    if req.entities.is_empty() || req.entities.len() > MAX_ENTITIES {
        return Some(format!("entities names 1 to {MAX_ENTITIES} Entity ids"));
    }
    if let Some(bad) = req.entities.iter().find(|id| EntityId::new(id).is_err()) {
        return Some(format!("{bad:?} is no Entity id"));
    }
    match &req.attrs {
        Some(a) if a.is_empty() => {
            return Some("attrs, when given, names at least one Attribute".into())
        }
        Some(a) => {
            if let Some(bad) = a
                .iter()
                .find(|n| !antares_jsonld::context::is_absolute_iri(n) || !plain(n, 2048))
            {
                return Some(format!("{bad:?} is no expanded Attribute name"));
            }
        }
        None => {}
    }
    if !plain(&req.reason, MAX_REASON) {
        return Some(format!(
            "reason is required, at most {MAX_REASON} characters"
        ));
    }
    if !plain(&req.actor, MAX_ACTOR) {
        return Some(format!("actor is required, at most {MAX_ACTOR} characters"));
    }
    if let Some(s) = &req.subject {
        if !(1..=MAX_SUBJECT).contains(&s.len())
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Some("subject is the keyed hash of the data subject, never the value".into());
        }
    }
    None
}

async fn erase(r: Arc<Reloader>, tenant: String, body: Bytes) -> Result<Response, String> {
    let Ok(t) = TenantId::new(&tenant) else {
        return Ok(bad_tenant());
    };
    if body.len() > MAX_BYTES {
        return Ok(refused(&format!(
            "an erasure request is at most {MAX_BYTES} bytes"
        )));
    }
    let req: Request = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return Ok(refused(&format!("not an erasure request: {e}"))),
    };
    if let Some(why) = invalid(&req) {
        return Ok(refused(&why));
    }
    let id = antares_api::erasure::record_id();
    let key = format!("{ERASURE_ID_PREFIX}{}:{id}", t.as_str());
    let mut record = json!({
        "id": key,
        "operation": "erase",
        "tenant": t.as_str(),
        "entities": req.entities,
        "attrs": req.attrs,
        "subject": req.subject,
        "actor": req.actor,
        "reason": req.reason,
        "at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    // the tombstone first: nothing is deleted without one, and an erasure
    // that fails halfway is finished by the next start's re-apply
    r.state.put_view_doc(&key, record.clone()).await?;
    let counts = apply(&r.state, &t, &req.entities, req.attrs.as_deref(), None).await?;
    // a json! object always; no branch is left that no request could take
    let _ = record
        .as_object_mut()
        .map(|o| o.insert("counts".into(), counts.clone()));
    r.state.put_view_doc(&key, record.clone()).await?;
    r.reload().await?;
    tracing::info!(tenant = t.as_str(), erasure = %id, %counts, "personal data erased");
    Ok((StatusCode::CREATED, axum::Json(record)).into_response())
}

async fn list(r: Arc<Reloader>, tenant: String) -> Result<Response, String> {
    let Ok(t) = TenantId::new(&tenant) else {
        return Ok(bad_tenant());
    };
    Ok(axum::Json(records(&r, Some(&t)).await).into_response())
}

/// The stored erasure records, of one Tenant or of all, oldest first.
async fn records(r: &Reloader, tenant: Option<&TenantId>) -> Vec<Value> {
    let prefix = tenant.map_or_else(
        || ERASURE_ID_PREFIX.to_owned(),
        |t| format!("{ERASURE_ID_PREFIX}{}:", t.as_str()),
    );
    let mut out: Vec<Value> = r
        .cache
        .lock()
        .await
        .iter()
        .filter(|(id, _)| id.starts_with(&prefix))
        .map(|(_, held)| held.doc.clone())
        .collect();
    let at = |r: &Value| r.get("at").and_then(Value::as_str).map(str::to_owned);
    out.sort_by_key(at);
    out
}

/// Every logged erasure applied again (DM-127): after a restore, what was
/// erased before it is gone again before the pod serves. Erasing what is
/// gone erases nothing, so a pod that restarts on a sound database only
/// spends the reads.
pub(super) async fn reapply(r: &Reloader) {
    for record in records(r, None).await {
        let field = |k: &str| record.get(k).cloned().unwrap_or_default();
        let tenant = field("tenant").as_str().and_then(|t| TenantId::new(t).ok());
        let entities: Option<Vec<String>> = serde_json::from_value(field("entities")).ok();
        let attrs: Option<Vec<String>> = serde_json::from_value(field("attrs")).ok();
        let at = field("at");
        let (Some(t), Some(entities), Some(at)) = (tenant, entities, at.as_str()) else {
            tracing::warn!(id = ?record.get("id"), "an erasure record names no tenant, Entities or time; skipped");
            continue;
        };
        match apply(&r.state, &t, &entities, attrs.as_deref(), Some(at)).await {
            Ok(counts)
                if counts
                    .as_object()
                    .is_some_and(|c| c.values().any(|n| n != 0)) =>
            {
                tracing::warn!(id = ?record.get("id"), %counts, "an erasure was applied again: restored data held erased values");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(id = ?record.get("id"), %e, "an erasure could not be applied again");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(body: Value) -> Request {
        serde_json::from_value(body).expect("request")
    }

    #[test]
    fn an_erasure_names_entities_a_reason_and_never_a_value_as_subject() {
        let ok = json!({"entities": ["urn:ngsi-ld:BikeTrip:t1"], "actor": "steward",
                        "reason": "Art. 17 request 2026-10-18", "subject": "9f86d081884c7d65"});
        assert_eq!(invalid(&req(ok.clone())), None);
        let with = |k: &str, v: Value| {
            let mut b = ok.clone();
            b[k] = v;
            invalid(&req(b))
        };
        assert!(with("entities", json!([])).is_some());
        assert!(with("entities", json!(["not an id"])).is_some());
        assert!(with("attrs", json!([])).is_some());
        assert!(with("attrs", json!(["rider"])).is_some(), "compact name");
        assert!(with("reason", json!(" ")).is_some());
        assert!(with("actor", json!("")).is_some());
        assert!(with("subject", json!("jane.doe@example.org")).is_some());
        assert!(with("subject", json!("")).is_some());
        assert!(with("subject", json!("a".repeat(MAX_SUBJECT + 1))).is_some());
        let many: Vec<String> = (0..=MAX_ENTITIES).map(|n| format!("urn:a:{n}")).collect();
        assert!(with("entities", json!(many)).is_some());
        assert!(with("attrs", json!(["https://e.org/a\u{7}"])).is_some());
        assert!(with("attrs", json!(["https://e.org/rider"])).is_none());
        assert!(with("attrs", json!(["urn:x:rider"])).is_none(), "urn IRI");
        assert!(with("actor", json!("a".repeat(MAX_ACTOR + 1))).is_some());
        assert!(with("reason", json!("a".repeat(MAX_REASON + 1))).is_some());
        assert!(with("subject", json!(null)).is_none());
        assert!(serde_json::from_value::<Request>(json!({
            "entities": ["urn:a:b"], "actor": "a", "reason": "r", "value": "x"
        }))
        .is_err());
    }
}
