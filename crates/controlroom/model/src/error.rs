#![forbid(unsafe_code)]
//! RFC 9457 Problem Details with slugs and OData 4.01 error envelopes (R10a, R12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub type CrResult<T> = Result<T, CrError>;

/// RFC 9457 Problem Details payload carrying a stable machine-readable `slug` (R12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub type_uri: String,
    pub slug: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Canonical error type across all `controlroom` crates.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CrError {
    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    Forbidden(String),

    #[error("{0}")]
    BadRequest(String),

    #[error("{detail}")]
    Unprocessable { detail: String, errors: Vec<String> },

    #[error("{0}")]
    Conflict(String),

    #[error("precondition failed: {0}")]
    PreconditionFailed(String),

    #[error("stale plan: {0}")]
    StalePlan(String),

    #[error("quota exceeded for {resource}: limit is {limit}")]
    QuotaExceeded { resource: String, limit: u64 },

    #[error("rate limit exceeded: {0}")]
    RateLimited(String),

    #[error("outbound request blocked by guard: {0}")]
    SsrfBlocked(String),

    #[error("outbound datasource error: {0}")]
    OutboundFailed(String),

    #[error("schema migration failed: {0}")]
    MigrationFailed(String),

    #[error("expression evaluation error: {0}")]
    ExpressionError(String),

    #[error("wasm plugin error: {0}")]
    WasmError(String),

    #[error("rls enforcement check failed: {0}")]
    RlsNotEnforced(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl CrError {
    /// Machine-readable RFC 9457 / OData error slug (R10a, R12).
    #[must_use]
    pub const fn slug(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "cr.not_found",
            Self::Unauthorized(_) => "cr.unauthorized",
            Self::Forbidden(_) => "cr.forbidden",
            Self::BadRequest(_) => "cr.bad_request",
            Self::Unprocessable { .. } => "cr.unprocessable_entity",
            Self::Conflict(_) => "cr.conflict",
            Self::PreconditionFailed(_) => "cr.precondition_failed",
            Self::StalePlan(_) => "cr.stale_plan",
            Self::QuotaExceeded { .. } => "cr.quota_exceeded",
            Self::RateLimited(_) => "cr.rate_limited",
            Self::SsrfBlocked(_) => "cr.ssrf_blocked",
            Self::OutboundFailed(_) => "cr.outbound_failed",
            Self::MigrationFailed(_) => "cr.migration_failed",
            Self::ExpressionError(_) => "cr.expression_error",
            Self::WasmError(_) => "cr.wasm_error",
            Self::RlsNotEnforced(_) => "cr.rls_not_enforced",
            Self::NotImplemented(_) => "cr.not_implemented",
            Self::Internal(_) => "cr.internal_error",
        }
    }

    /// HTTP status code for this error.
    #[must_use]
    pub const fn status_code(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::BadRequest(_)
            | Self::MigrationFailed(_)
            | Self::ExpressionError(_)
            | Self::WasmError(_) => 400,
            Self::Unprocessable { .. } => 422,
            Self::Conflict(_) | Self::StalePlan(_) => 409,
            Self::PreconditionFailed(_) => 412,
            Self::QuotaExceeded { .. } | Self::SsrfBlocked(_) => 403,
            Self::RateLimited(_) => 429,
            Self::OutboundFailed(_) => 502,
            Self::NotImplemented(_) => 501,
            Self::RlsNotEnforced(_) | Self::Internal(_) => 500,
        }
    }

    /// Short human-readable title for RFC 9457.
    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "Not Found",
            Self::Unauthorized(_) => "Unauthorized",
            Self::Forbidden(_) => "Forbidden",
            Self::BadRequest(_) => "Bad Request",
            Self::Unprocessable { .. } => "Unprocessable Entity",
            Self::Conflict(_) => "Conflict",
            Self::PreconditionFailed(_) => "Precondition Failed",
            Self::StalePlan(_) => "Stale Plan",
            Self::QuotaExceeded { .. } => "Quota Exceeded",
            Self::RateLimited(_) => "Too Many Requests",
            Self::SsrfBlocked(_) => "Outbound Request Blocked",
            Self::OutboundFailed(_) => "Bad Gateway",
            Self::MigrationFailed(_) => "Schema Migration Failed",
            Self::ExpressionError(_) => "Expression Error",
            Self::WasmError(_) => "Plugin Execution Error",
            Self::RlsNotEnforced(_) => "Tenant Isolation Check Failed",
            Self::NotImplemented(_) => "Not Implemented",
            Self::Internal(_) => "Internal Server Error",
        }
    }

    /// Safe public detail message (never leaks internal error strings on 500).
    #[must_use]
    pub fn public_detail(&self) -> String {
        match self {
            Self::Internal(_) | Self::RlsNotEnforced(_) => {
                "an internal server error occurred".to_owned()
            }
            other => other.to_string(),
        }
    }

    /// Converts into an RFC 9457 [`ProblemDetails`] struct.
    #[must_use]
    pub fn to_problem(&self, instance: Option<String>) -> ProblemDetails {
        let errors = match self {
            Self::Unprocessable { errors, .. } => errors.clone(),
            _ => Vec::new(),
        };
        ProblemDetails {
            type_uri: format!("urn:joinedcontext:problem:{}", self.slug()),
            slug: self.slug().to_owned(),
            title: self.title().to_owned(),
            status: self.status_code(),
            detail: self.public_detail(),
            instance,
            errors,
        }
    }

    /// Converts into an OData 4.01 error JSON envelope carrying the RFC 9457 slug (R10a).
    #[must_use]
    pub fn to_odata_error(&self) -> Value {
        let problem = self.to_problem(None);
        let details: Vec<Value> = problem
            .errors
            .iter()
            .map(|e| json!({"code": problem.slug, "message": e}))
            .collect();
        json!({
            "error": {
                "code": problem.slug,
                "message": problem.detail,
                "target": problem.type_uri,
                "status": problem.status,
                "details": details,
            }
        })
    }
}
