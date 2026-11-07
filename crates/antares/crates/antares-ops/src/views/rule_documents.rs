// SPDX-License-Identifier: EUPL-1.2
//! The policy engine's rule documents (ADR-0020 amendment): one opaque JSON
//! document per Tenant, written to `/q/policy/{tenant}`, stored under the
//! reserved tenant `antares-policy` with a revision, and handed to the
//! engine through `PolicyEngine::load` on every reload of the view table.
//!
//! The engine owns the document's schema, so the broker bounds it and checks
//! nothing else. A document the engine refuses is not stored when it comes
//! through a PUT, and keeps the previous revision deciding when it comes
//! from the store; `/q/health` names it by tenant and revision. Neither the
//! document nor the engine's reason is logged.

use super::{problem, serve, Reloader};
use antares_api::AppState;
use antares_model::TenantId;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Weak};

/// The largest rule document `/q/policy/{tenant}` accepts, in bytes. Every
/// Pipeline that writes a space adds a derived grant of about 230 bytes to
/// its tenant's document (platform PL-20): 64 KiB refused a space of 302
/// pipelines and left every newer one without a grant (T-5717). 1 MiB holds
/// about 4,000 and stays under axum's 2 MiB default body limit.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// The deepest nesting of arrays and objects in a rule document; the
/// document itself is level 1.
pub const MAX_DOCUMENT_DEPTH: usize = 32;

/// The most JSON values in a rule document, every array, object and scalar
/// counted once: the bound on what one reload hands the engine.
/// The same density as the byte bound: 8,192 per 64 KiB.
pub const MAX_DOCUMENT_VALUES: usize = 131_072;

/// Each Tenant's stored document as read, and whether the engine took it.
pub(super) type Cache = HashMap<String, (Value, bool)>;

/// The engine, when it reads rule documents: without one `/q/policy`
/// answers 404 and nothing is loaded.
fn engine(state: &AppState) -> Option<&Arc<dyn antares_api::policy::PolicyEngine>> {
    state.policy.as_ref().filter(|e| e.loads_documents())
}

/// Within the depth and value bounds, walked without recursion.
fn bounded(doc: &Value) -> Result<(), String> {
    let mut values = 0usize;
    let mut stack = vec![(doc, 1usize)];
    while let Some((v, depth)) = stack.pop() {
        values = values.saturating_add(1);
        if values > MAX_DOCUMENT_VALUES {
            return Err(format!(
                "a rule document holds at most {MAX_DOCUMENT_VALUES} values"
            ));
        }
        if depth > MAX_DOCUMENT_DEPTH {
            return Err(format!(
                "a rule document nests at most {MAX_DOCUMENT_DEPTH} levels"
            ));
        }
        let next = depth.saturating_add(1);
        match v {
            Value::Array(a) => stack.extend(a.iter().map(|c| (c, next))),
            Value::Object(o) => stack.extend(o.values().map(|c| (c, next))),
            _ => {}
        }
    }
    Ok(())
}

fn revision(stored: &Value) -> u64 {
    stored.get("revision").and_then(Value::as_u64).unwrap_or(0)
}

/// Hand the engine every stored document that changed since the last
/// reload, drop the Tenants whose document was deleted, and set the
/// `/q/health` report. A no-op without an engine that reads documents.
pub(super) async fn reload(state: &AppState, cache: &mut Cache) -> Result<(), String> {
    let Some(engine) = engine(state) else {
        return Ok(());
    };
    let docs = state.policy_docs().await?;
    let mut next = Cache::with_capacity(docs.len());
    for doc in docs {
        let Some(tenant) = doc.get("tenant").and_then(Value::as_str).map(str::to_owned) else {
            tracing::warn!("a stored policy document has no tenant; skipped");
            continue;
        };
        let took = match cache.remove(&tenant) {
            Some((old, took)) if old == doc => took,
            _ => {
                let took = match doc.get("document") {
                    Some(d) if !d.is_null() => engine.load(&tenant, d).is_ok(),
                    _ => false,
                };
                if !took {
                    // The previous revision keeps deciding.
                    let (t, rev) = (tenant.as_str(), revision(&doc));
                    tracing::warn!(tenant = t, revision = rev, "policy document refused");
                }
                took
            }
        };
        next.insert(tenant, (doc, took));
    }
    for tenant in cache.keys() {
        if engine.load(tenant, &Value::Null).is_err() {
            tracing::warn!(tenant = tenant.as_str(), "policy engine refused a drop");
        }
    }
    state.views.set_policy_report(report(&next));
    *cache = next;
    Ok(())
}

/// Tenant and revision of each document, loaded or refused.
fn report(cache: &Cache) -> Value {
    let sorted: BTreeMap<_, _> = cache.iter().collect();
    let (mut loaded, mut refused) = (Vec::new(), Vec::new());
    for (tenant, (doc, took)) in sorted {
        let row = json!({"tenant": tenant, "revision": revision(doc)});
        if *took {
            loaded.push(row);
        } else {
            refused.push(row);
        }
    }
    json!({"loaded": loaded, "refused": refused})
}

/// `/q/policy`: the report (GET), and one Tenant's document (GET, PUT,
/// DELETE).
pub(super) fn routes(r: &Weak<Reloader>) -> Router {
    let (list, one, put, del) = (r.clone(), r.clone(), r.clone(), r.clone());
    Router::new()
        .route(
            "/policy",
            get(move || {
                serve(list.clone(), |r| async move {
                    Ok(match engine(&r.state) {
                        Some(_) => axum::Json(r.state.views.policy_report()).into_response(),
                        None => no_engine(),
                    })
                })
            }),
        )
        .route(
            "/policy/{tenant}",
            get(move |Path(t): Path<String>| serve(one.clone(), |r| get_one(r, t)))
                .put(move |Path(t): Path<String>, body: Bytes| {
                    serve(put.clone(), |r| put_one(r, t, body))
                })
                .delete(move |Path(t): Path<String>| serve(del.clone(), |r| delete_one(r, t))),
        )
}

/// The engine and a valid client Tenant name, or the answer to give.
fn admit<'a>(
    r: &'a Reloader,
    tenant: &str,
) -> Result<&'a Arc<dyn antares_api::policy::PolicyEngine>, Box<Response>> {
    let engine = engine(&r.state).ok_or_else(|| Box::new(no_engine()))?;
    TenantId::new(tenant).map_err(|e| {
        Box::new(problem(
            StatusCode::BAD_REQUEST,
            "BadRequestData",
            &e.to_string(),
        ))
    })?;
    Ok(engine)
}

async fn get_one(r: Arc<Reloader>, tenant: String) -> Result<Response, String> {
    if let Err(answer) = admit(&r, &tenant) {
        return Ok(*answer);
    }
    super::count_doc_reads("admin", 1);
    Ok(match r.state.policy_doc(&tenant).await? {
        Some(doc) => axum::Json(doc).into_response(),
        None => not_found(),
    })
}

/// Bound, hand to the engine, store, and answer once this pod's engine
/// holds the document. Under the cache lock, so a reload cannot hand the
/// engine an older revision between the load and the store.
async fn put_one(r: Arc<Reloader>, tenant: String, body: Bytes) -> Result<Response, String> {
    let engine = match admit(&r, &tenant) {
        Ok(e) => e,
        Err(answer) => return Ok(*answer),
    };
    if body.len() > MAX_DOCUMENT_BYTES {
        return Ok(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "BadRequestData",
            &format!("a rule document is at most {MAX_DOCUMENT_BYTES} bytes"),
        ));
    }
    let checked = serde_json::from_slice::<Value>(&body)
        .map_err(|e| format!("not JSON: {e}"))
        .and_then(|doc| {
            if doc.is_null() {
                return Err("a rule document is not null; DELETE drops one".into());
            }
            bounded(&doc).map(|()| doc)
        });
    let doc = match checked {
        Ok(doc) => doc,
        Err(why) => return Ok(problem(StatusCode::BAD_REQUEST, "BadRequestData", &why)),
    };
    let replaced = {
        let mut cache = r.policy.lock().await;
        if let Err(why) = engine.load(&tenant, &doc) {
            return Ok(problem(StatusCode::BAD_REQUEST, "BadRequestData", &why));
        }
        super::count_doc_reads("admin", 1);
        let previous = r.state.policy_doc(&tenant).await?;
        let stored = json!({
            "tenant": tenant,
            "revision": previous.as_ref().map_or(0, revision).saturating_add(1),
            "document": doc,
        });
        // Out of the cache first: should the store fail, the next reload
        // hands the engine the stored revision again.
        cache.remove(&tenant);
        let replaced = r.state.put_policy_doc(&tenant, stored.clone()).await?;
        cache.insert(tenant, (stored, true));
        replaced
    };
    r.reload().await?;
    Ok(if replaced {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    }
    .into_response())
}

async fn delete_one(r: Arc<Reloader>, tenant: String) -> Result<Response, String> {
    if let Err(answer) = admit(&r, &tenant) {
        return Ok(*answer);
    }
    {
        // Held across the delete so no reload runs between it and the one
        // below; the Tenant stays in the cache, and that reload drops it
        // from the engine the way it drops a document another pod deleted.
        let _held = r.policy.lock().await;
        if !r.state.delete_policy_doc(&tenant).await? {
            return Ok(not_found());
        }
    }
    r.reload().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn not_found() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "ResourceNotFound",
        "no rule document for this tenant",
    )
}

fn no_engine() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "ResourceNotFound",
        "the policy engine reads no rule documents",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(depth: usize) -> Value {
        (1..depth).fold(json!(0), |v, _| json!([v]))
    }

    #[test]
    fn a_document_is_bounded_in_depth_and_values() {
        assert!(bounded(&nested(MAX_DOCUMENT_DEPTH)).is_ok());
        assert!(bounded(&nested(MAX_DOCUMENT_DEPTH + 1)).is_err());
        let wide = |n: usize| Value::Array(vec![json!(0); n]);
        assert!(bounded(&wide(MAX_DOCUMENT_VALUES - 1)).is_ok());
        assert!(bounded(&wide(MAX_DOCUMENT_VALUES)).is_err());
        assert!(bounded(&json!({"a": {"b": "c"}})).is_ok());
    }

    #[test]
    fn the_report_names_tenant_and_revision_only() {
        let mut cache = Cache::new();
        let doc = |t: &str, n: u64| json!({"tenant": t, "revision": n, "document": {"secret": 1}});
        cache.insert("b".into(), (doc("b", 2), false));
        cache.insert("a".into(), (doc("a", 1), true));
        assert_eq!(
            report(&cache),
            json!({
                "loaded": [{"tenant": "a", "revision": 1}],
                "refused": [{"tenant": "b", "revision": 2}],
            })
        );
    }
}
