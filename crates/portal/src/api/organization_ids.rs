//! The operator's door to Organization ids (EP-186, API/15 §2, T-7012): an id is a verified
//! domain with its labels reversed and joined by `_` (`bbsk.sk` -> `sk_bbsk`), minted only by an
//! operator (the bootstrap group, `JC_PORTAL_BOOTSTRAP_ADMINS`) after the domain proves itself.
//!
//! Two steps, the same record the Organization's domain check reads (PF-41):
//!
//! 1. `POST /organization-ids/challenge {domain}` keeps a challenge for the domain and answers the
//!    TXT record to publish, `_joinedcontext.{domain} TXT "jc-verify={challenge}"`;
//! 2. `POST /organization-ids {domain}` asks DNS for that record and, when one of its strings is
//!    the challenge, mints the id.
//!
//! A minted id is never renamed, released or reused (PF-78): the database refuses every change
//! and delete of a minted row (migration 0057), so a deleted Organization's id stays reserved and
//! a second mint answers 409 naming `organizationId`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use jc_core::names::OrganizationId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// The operators' door check (T-7129 §4): a person of the operators' group with a second
// factor, never a service account or a client's token; minting is a person's decision.
use super::operator_changes::operator;
use crate::auth::session::CurrentUser;
use crate::domain_verification::{mint_challenge, record, txt_name, TxtRecords, Unanswered};
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// The longest one TXT lookup of a mint may take, retries and every name server included.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
/// The most TXT strings a mint reads, and the longest one it compares: a `jc-verify=` value is
/// 54 characters, so anything longer proves nothing and is not looked at.
const MAX_RECORDS: usize = 64;
const MAX_RECORD_LEN: usize = 512;
/// Name suffixes the cluster resolves for itself: an id for one would prove nothing public.
const INTERNAL_SUFFIXES: &[&str] = &[
    ".cluster.local",
    ".svc",
    ".local",
    ".internal",
    ".localhost",
];

/// The domain an operator mints an Organization id from.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({ "domain": "bbsk.sk" }))]
pub struct MintRequest {
    /// The Organization's own domain, lowercase, e.g. `bbsk.sk`; its id is `sk_bbsk`.
    pub domain: String,
}

/// The record that proves the domain, to publish before the mint.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    /// The domain, e.g. `bbsk.sk`.
    pub domain: String,
    /// The id the mint gives it, e.g. `sk_bbsk`.
    pub organization_id: String,
    /// Where the TXT record goes, e.g. `_joinedcontext.bbsk.sk`.
    pub name: String,
    /// The TXT string, e.g. `jc-verify=3q2-...`. Not a secret: it is published in DNS.
    pub value: String,
    /// The whole record as a DNS zone writes it.
    pub record: String,
}

/// A minted Organization id.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Minted {
    /// The id, e.g. `sk_bbsk`: the shared-host path segment and the owner segment of its URNs.
    pub organization_id: String,
    /// The domain that proved it, e.g. `bbsk.sk`.
    pub domain: String,
    #[schema(value_type = String, format = DateTime)]
    pub minted_at: DateTime<Utc>,
}

/// The domain of the body and its id: 400 for a body that does not parse, 422 naming `domain`
/// for a name that is not a public DNS name or gives no id.
fn parsed(
    body: Result<Json<MintRequest>, JsonRejection>,
) -> Result<(String, OrganizationId), ApiError> {
    let Json(body) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let domain = body.domain.trim().trim_end_matches('.').to_owned();
    let refuse = |message: String| ApiError::Unprocessable {
        detail: "the domain gives no Organization id".into(),
        errors: vec![format!("domain: {message}")],
    };
    if INTERNAL_SUFFIXES.iter().any(|s| domain.ends_with(s)) {
        return Err(refuse(format!(
            "'{domain}' is a name the cluster resolves for itself; give the Organization's public domain, e.g. bbsk.sk"
        )));
    }
    let id = OrganizationId::from_domain(&domain).map_err(|e| refuse(e.to_string()))?;
    Ok((domain, id))
}

fn database(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "Organization ids are kept in the database, and this Portal has none".into(),
        )
    })
}

fn minted_already(id: &OrganizationId) -> ApiError {
    ApiError::Conflict(format!(
        "organizationId: {id} is minted already and stays reserved, even after its Organization is deleted (PF-78)"
    ))
}

/// The challenge and, once minted, when (epoch seconds: sqlx here speaks `time`, the answer chrono).
type Row = (String, Option<i64>);

async fn stored(pool: &sqlx::PgPool, id: &OrganizationId) -> Result<Option<Row>, ApiError> {
    sqlx::query_as::<_, Row>(
        "SELECT challenge, extract(epoch FROM minted_at)::bigint FROM organization_ids WHERE id = $1",
    )
        .bind(id.as_str())
        .fetch_optional(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("organization ids: {e}")))
}

fn challenge_of(domain: &str, id: &OrganizationId, challenge: &str) -> Challenge {
    Challenge {
        domain: domain.to_owned(),
        organization_id: id.to_string(),
        name: txt_name(domain),
        value: format!("jc-verify={challenge}"),
        record: record(domain, challenge),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/organization-ids/challenge",
    summary = "Ask For An Organization Id Challenge",
    description = "The TXT record an operator publishes to prove a domain before its Organization id is minted (EP-186, PF-41). The same domain always gets the same challenge. 409 naming organizationId when the id is minted already.",
    tag = "system",
    request_body = MintRequest,
    responses(
        (status = 200, description = "The record to publish", body = Challenge),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator", body = ProblemDetails),
        (status = 409, description = "The id is minted already", body = ProblemDetails),
        (status = 422, description = "The domain gives no id", body = ProblemDetails),
        (status = 503, description = "No database", body = ProblemDetails)
    )
)]
pub async fn request_challenge(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Result<Json<MintRequest>, JsonRejection>,
) -> Result<Json<Challenge>, ApiError> {
    operator(&state, &user.0.identity)?;
    let (domain, id) = parsed(body)?;
    let pool = database(&state)?;
    // A concurrent request for the same domain keeps the first challenge: the insert is a no-op
    // and the read below answers the stored one.
    sqlx::query(
        "INSERT INTO organization_ids (id, domain, challenge, requested_by) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id.as_str())
    .bind(&domain)
    .bind(mint_challenge())
    .bind(&user.0.identity.subject)
    .execute(pool)
    .await
    .map_err(|e| ApiError::Internal(format!("organization ids: {e}")))?;
    match stored(pool, &id).await? {
        Some((_, Some(_))) => Err(minted_already(&id)),
        Some((challenge, None)) => Ok(Json(challenge_of(&domain, &id, &challenge))),
        None => Err(ApiError::Internal(
            "organization ids: the challenge was not kept".into(),
        )),
    }
}

/// The TXT strings at `name`, at most [`MAX_RECORDS`] of at most [`MAX_RECORD_LEN`] each, within
/// [`LOOKUP_TIMEOUT`].
async fn lookup(state: &AppState, name: &str) -> Result<Vec<String>, Unanswered> {
    let records: Arc<dyn TxtRecords> = match &state.txt_records {
        Some(records) => Arc::clone(records),
        None => Arc::new(crate::domain_verification::NetLookup::new().map_err(|_| Unanswered)?),
    };
    let found = tokio::time::timeout(LOOKUP_TIMEOUT, records.txt(name))
        .await
        .map_err(|_| Unanswered)??;
    Ok(found
        .into_iter()
        .filter(|text| text.len() <= MAX_RECORD_LEN)
        .take(MAX_RECORDS)
        .collect())
}

/// Whether one string is exactly `jc-verify={challenge}`, compared in constant time.
fn proves(records: &[String], challenge: &str) -> bool {
    let wanted = format!("jc-verify={challenge}");
    records
        .iter()
        .any(|text| crate::auth::csrf::constant_time_eq(text.trim(), &wanted))
}

#[utoipa::path(
    post,
    path = "/api/v1/organization-ids",
    summary = "Mint An Organization Id",
    description = "Mints the Organization id of a domain (`bbsk.sk` -> `sk_bbsk`) once its `_joinedcontext.{domain}` TXT record carries the challenge (EP-186, PF-41). A minted id is never renamed, released or reused (PF-78). Needs an operator who proved a second factor.",
    tag = "system",
    request_body = MintRequest,
    responses(
        (status = 201, description = "The id is minted", body = Minted),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, or no second factor", body = ProblemDetails),
        (status = 409, description = "The id is minted already", body = ProblemDetails),
        (status = 422, description = "The domain gives no id, has no challenge yet, or its TXT record does not carry it", body = ProblemDetails),
        (status = 503, description = "No database", body = ProblemDetails)
    )
)]
pub async fn mint(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Result<Json<MintRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Minted>), ApiError> {
    operator(&state, &user.0.identity)?;
    crate::auth::session::require_second_factor(&user.0.identity, &state.config)?;
    let (domain, id) = parsed(body)?;
    let pool = database(&state)?;
    let refuse = |message: String| ApiError::Unprocessable {
        detail: format!("{domain} is not proven, so no Organization id is minted"),
        errors: vec![format!("domain: {message}")],
    };
    let challenge = match stored(pool, &id).await? {
        Some((_, Some(_))) => return Err(minted_already(&id)),
        Some((challenge, None)) => challenge,
        None => {
            return Err(refuse(
                "no challenge was asked for this domain: ask for one, publish its TXT record, then mint".into(),
            ))
        }
    };
    let name = txt_name(&domain);
    match lookup(&state, &name).await {
        Ok(records) if proves(&records, &challenge) => {}
        Ok(records) if records.is_empty() => {
            return Err(refuse(format!(
                "{name} has no TXT record; publish {} and mint again once DNS answers it",
                record(&domain, &challenge)
            )))
        }
        Ok(_) => {
            return Err(refuse(format!(
                "no TXT record at {name} is this challenge; publish {} and mint again",
                record(&domain, &challenge)
            )))
        }
        Err(Unanswered) => {
            return Err(refuse(format!(
                "the TXT lookup of {name} did not answer in time; try again in a minute"
            )))
        }
    }
    // Only a pending row turns minted: of two mints at once, one stamps it and the other finds
    // none to stamp and answers 409.
    let minted_at = sqlx::query_scalar::<_, i64>(
        "UPDATE organization_ids SET minted_by = $2, minted_at = now() \
         WHERE id = $1 AND minted_at IS NULL RETURNING extract(epoch FROM minted_at)::bigint",
    )
    .bind(id.as_str())
    .bind(&user.0.identity.subject)
    .fetch_optional(pool)
    .await
    .map_err(|e| ApiError::Internal(format!("organization ids: {e}")))?
    .ok_or_else(|| minted_already(&id))?;
    let minted_at = DateTime::from_timestamp(minted_at, 0).unwrap_or_else(Utc::now);
    tracing::info!(organization_id = %id, subject = %user.0.identity.subject, "organization id minted");
    Ok((
        StatusCode::CREATED,
        Json(Minted {
            organization_id: id.to_string(),
            domain,
            minted_at,
        }),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization-ids", post(mint))
        .route("/organization-ids/challenge", post(request_challenge))
}
