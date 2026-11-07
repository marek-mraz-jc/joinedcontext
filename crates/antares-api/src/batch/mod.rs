// SPDX-License-Identifier: EUPL-1.2
//! Batch operations /entityOperations/* (5.6.7–5.6.10, 5.6.20, 5.7.2-POST;
//! resources 6.14–6.17, 6.23, 6.31).

mod delete;
mod forward;
mod query;
#[cfg(test)]
mod tests;
mod write;

pub use delete::batch_delete;
use forward::*;
pub use query::batch_query;
pub use write::{batch_create, batch_merge, batch_update, batch_upsert};

use crate::entities::merge_into;
use crate::negotiate::*;
use crate::paging::{paginate, query_doc_params};
use crate::repr::{apply, parse_repr};
use crate::stamp::stamp_new;
use crate::state::{now_iso, AppState};
use antares_jsonld::{expand_entity, ExpandOpts};
use antares_model::NgsiError;
use antares_store::CurrentStateDriverExt;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::negotiate::CleanParams;

/// Parse a batch body: JSON array of entity documents; per-document context
/// resolution (ld+json ⇒ each doc's own @context; json ⇒ Link header).
/// Batch bodies are the ingest hot path — sonic-rs (3–4× parse) behind
/// the `sonic` feature, serde_json always compiled as the fallback.
#[cfg(feature = "sonic")]
fn parse_batch_body(body: &[u8]) -> Result<Value, String> {
    sonic_rs::from_slice(body).map_err(|e| e.to_string())
}
#[cfg(not(feature = "sonic"))]
fn parse_batch_body(body: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(body).map_err(|e| e.to_string())
}

/// One entry of a batch array as it arrives: the item and the result of
/// resolving the `@context` that governs it. A failed resolution is carried
/// per item, because 5.6.7 answers the array with one error per entity.
type BatchItem = (Value, ApiResult<std::sync::Arc<antares_jsonld::Context>>);

/// One entry of a batch as a forward sends it: the object and the `@context`
/// that expanded it.
type FwdItem = (
    serde_json::Map<String, Value>,
    std::sync::Arc<antares_jsonld::Context>,
);

/// One batch body into (item, its resolved @context) pairs. Each item's
/// `@context` — or the Link header standing in for it — resolves within the
/// requesting Tenant (5.5.10): a Hosted @context belongs to the Tenant that
/// stored it (5.13.1), so for any other Tenant the URL is unresolvable rather
/// than a set of term mappings the Entity would be expanded through.
async fn parse_batch(
    st: &AppState,
    tenant: &antares_model::TenantId,
    headers: &HeaderMap,
    body: &[u8],
) -> ApiResult<Vec<BatchItem>> {
    let ct = content_type(headers)?;
    let ld = match ct.as_str() {
        "application/json" => false,
        "application/ld+json" => true,
        _ => return Err(ApiError::Bare(StatusCode::UNSUPPORTED_MEDIA_TYPE)),
    };
    let value: Value = parse_batch_body(body)
        .map_err(|e| NgsiError::InvalidRequest(format!("body is not valid JSON: {e}")))?;
    let items = value.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
        NgsiError::BadRequestData("batch body must be a non-empty JSON array".into())
    })?;
    // 5.6.7.4 (and siblings): a null value in ANY item fails the whole
    // request with BadRequestData — not a per-item 207 error.
    if items.iter().any(Value::is_null) {
        return Err(
            NgsiError::BadRequestData("batch array must not contain null items".into()).into(),
        );
    }
    // Batch entity count cap
    if items.len() > *crate::bounds::MAX_BATCH_ITEMS {
        return Err(NgsiError::BadRequestData(format!(
            "batch of {} exceeds the {}-entity limit",
            items.len(),
            *crate::bounds::MAX_BATCH_ITEMS
        ))
        .into());
    }
    let link = link_context(headers)?;
    if ld && link.is_some() {
        return Err(NgsiError::BadRequestData(
            "application/ld+json batch must not also carry a Link @context (6.3.5)".into(),
        )
        .into());
    }
    let mut out = Vec::new();
    // The loader caps ONE @context resolution at MAX_CONTEXT_FETCHES fetched
    // documents. A batch resolves once per item, so without a ceiling here
    // the item count multiplies that cap: a body inside the 4 MiB limit can
    // name a thousand different @contexts and buy a thousand crawls of a
    // chosen host from one request. Distinct values are what cost a crawl —
    // repeats hit the loader's merged cache, and an inline object fetches
    // nothing — so only those are counted.
    let mut named_contexts: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut count_context = |c: &Value| -> Result<(), ApiError> {
        // an array names each of its URLs: count them one by one, inline
        // objects fetch nothing
        let named: std::collections::HashSet<&str> = match c {
            Value::String(u) => std::iter::once(u.as_str()).collect(),
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => std::collections::HashSet::new(),
        };
        let fresh: Vec<&str> = named
            .into_iter()
            .filter(|u| !named_contexts.contains(*u))
            .collect();
        // Past the ceiling nothing more is remembered either: the set is
        // keyed by client-supplied text, so it may not grow past the cap it
        // guards.
        if named_contexts.len().saturating_add(fresh.len()) > crate::bounds::MAX_CONTEXT_FETCHES {
            return Err(NgsiError::BadRequestData(format!(
                "batch names more than {} distinct @contexts",
                crate::bounds::MAX_CONTEXT_FETCHES
            ))
            .into());
        }
        named_contexts.extend(fresh.into_iter().map(str::to_owned));
        Ok(())
    };
    for item in items {
        let ctx = if ld {
            match item.get("@context") {
                Some(c) => match count_context(c) {
                    Err(e) => Err(e),
                    Ok(()) => st
                        .loader
                        .resolve_for(tenant, c)
                        .await
                        .map_err(ApiError::from),
                },
                None => Err(NgsiError::BadRequestData(
                    "ld+json batch entity without @context".into(),
                )
                .into()),
            }
        } else if item.get("@context").is_some() {
            Err(NgsiError::BadRequestData("application/json entity carries @context".into()).into())
        } else {
            match &link {
                Some(url) => st
                    .loader
                    .resolve_for(tenant, &Value::String(url.clone()))
                    .await
                    .map_err(ApiError::from),
                None => Ok(st.loader.core()),
            }
        };
        out.push((item.clone(), ctx));
    }
    Ok(out)
}

struct BatchOutcome {
    success: Vec<Value>,
    errors: Vec<Value>,
}

impl BatchOutcome {
    const fn new() -> Self {
        Self {
            success: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// `id` joins S once, however many members or forwards succeeded for it.
    fn succeed(&mut self, id: &str) {
        if !self.success.iter().any(|v| v.as_str() == Some(id)) {
            self.success.push(Value::String(id.to_owned()));
        }
    }

    /// Batch output data (5.6.7.5 / 5.6.8.5 / 5.6.9.5 / 5.6.10.5): if every
    /// entity succeeded, the operation's all-ok status (201 + id array for
    /// create, else 204 with no body); otherwise 207 with the S array
    /// ("success") and the E array of BatchEntityError 5.2.17 ("errors").
    fn respond(
        self,
        tenant: &antares_model::TenantId,
        all_ok_status: StatusCode,
        body_on_ok: bool,
    ) -> Response {
        if self.errors.is_empty() {
            if body_on_ok {
                let mut resp = (
                    all_ok_status,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    axum::Json(Value::Array(self.success)),
                )
                    .into_response();
                echo_tenant(tenant, &mut resp);
                resp
            } else {
                let mut resp = all_ok_status.into_response();
                echo_tenant(tenant, &mut resp);
                resp
            }
        } else {
            multi_status(
                json!({"success": self.success, "errors": self.errors}),
                tenant,
            )
        }
    }
}

/// 5.2.17 BatchEntityError for a member the policy engine refused: the
/// AccessDenied problem a refused single-Entity write answers with (ADR-0020).
fn denied_entry(id: Option<&str>, why: &str) -> Value {
    json!({
        "entityId": id.unwrap_or("unknown"),
        "error": {
            "type": crate::policy::ACCESS_DENIED_TYPE,
            "title": crate::policy::ACCESS_DENIED_TITLE,
            "status": StatusCode::FORBIDDEN.as_u16(),
            "detail": why,
        },
    })
}

/// The reason a batch is refused whole: every member was refused, so the
/// answer is the 403 a single refusal gives, carrying the first reason.
fn whole_refusal(refused: &[Result<crate::policy::Filter, String>]) -> Option<String> {
    if refused.iter().all(Result::is_err) {
        refused.first().and_then(|r| r.clone().err())
    } else {
        None
    }
}

/// The conditions the engine set on the allowed members (gap G12), by id.
/// One id named by several members holds to every condition any of them
/// carries: a round writes each id once, so the strictest reading is the
/// one that never applies a member's write past its own condition.
#[derive(Default)]
struct Conditions(HashMap<String, Vec<crate::policy::Condition>>);

impl Conditions {
    fn add(&mut self, id: Option<&str>, granted: &crate::policy::Filter) {
        if let (Some(id), Some(c)) = (id, crate::policy::Condition::of(granted)) {
            self.0.entry(id.to_owned()).or_default().push(c);
        }
    }

    fn holds(&self, id: &str, doc: &Value) -> bool {
        self.0
            .get(id)
            .is_none_or(|cs| cs.iter().all(|c| c.holds(doc)))
    }

    /// Inside the store closure: a member whose stored Entity a condition
    /// does not select is not found, as an absent one is.
    fn pre(&self, id: &str, doc: &Value) -> Result<(), NgsiError> {
        if self.holds(id, doc) {
            Ok(())
        } else {
            Err(Self::not_found(id))
        }
    }

    /// Whether a condition holds `id`: an upsert of it may update an Entity
    /// the condition selects and never creates one (5.6.8, T-3668).
    fn has(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    /// The one answer for a conditioned member that is absent or not
    /// selected, so the two cannot be told apart.
    fn not_found(id: &str) -> NgsiError {
        NgsiError::ResourceNotFound(format!("entity {id} not found"))
    }

    fn unforwarded(&self, forwards: bool) -> Result<(), crate::policy::Denied> {
        if forwards && !self.0.is_empty() {
            Err(crate::policy::Denied(
                crate::policy::CONDITION_NOT_FORWARDED.to_owned(),
            ))
        } else {
            Ok(())
        }
    }
}

fn err_entry(id: Option<&str>, e: &NgsiError) -> Value {
    json!({
        "entityId": id.unwrap_or("unknown"),
        "error": problem_value(e),
    })
}

#[derive(Clone, Copy, PartialEq)]
enum BatchMode {
    Create,
    Upsert,
    Update,
    Merge,
}

impl BatchMode {
    /// The clause the policy seam is asked about: one function serves the
    /// four batch write operations, and they are four clauses.
    const fn clause(self) -> &'static str {
        match self {
            Self::Create => "5.6.7",
            Self::Upsert => "5.6.8",
            Self::Update => "5.6.9",
            Self::Merge => "5.6.20",
        }
    }

    /// The operation a registration names to take the whole array (4.3.6.1).
    const fn batch_op(self) -> &'static str {
        match self {
            Self::Create => "createBatch",
            Self::Upsert => "upsertBatch",
            Self::Update => "updateBatch",
            Self::Merge => "mergeBatch",
        }
    }

    /// The `/entityOperations/{path}` resource of the operation.
    const fn batch_path(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Upsert => "upsert",
            Self::Update => "update",
            Self::Merge => "merge",
        }
    }
}

/// The write a batch asks for: the operation and the 6.3.7 `options` that
/// select its mode, 6.15.3.1 update (else replace) for upsert and 6.16.3.1
/// noOverwrite for update.
#[derive(Clone, Copy)]
struct WriteOpts {
    mode: BatchMode,
    update: bool,
    no_overwrite: bool,
}
