#![forbid(unsafe_code)]
//! Error types with RFC 9457 Problem Details slugs, OData 4.01 error envelopes,
//! and Gitea compatibility `{message, url}` payloads (S7, S8, S9).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub type ShakoResult<T> = Result<T, ShakoError>;

/// RFC 9457 Problem Details response body (S8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, utoipa::ToSchema)]
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

/// Gitea 1.27.3 error body `{message, url}` expected by `GiteaClient::forge_message` (S7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GiteaErrorBody {
    pub message: String,
    pub url: String,
}

/// Canonical error across all `shako` crates.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShakoError {
    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    Forbidden(String),

    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    Unprocessable(String),

    #[error("{0}")]
    Conflict(String),

    #[error("{0}")]
    BehindBaseBranch(String),

    #[error("{0}")]
    MethodNotAllowed(String),

    #[error("quota exceeded for {resource}: limit is {limit}")]
    QuotaExceeded { resource: String, limit: u64 },

    #[error("rate limit exceeded: {0}")]
    RateLimited(String),

    #[error("outbound request blocked by SSRF guard: {0}")]
    SsrfBlocked(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("service busy (memory budget exhausted): {0}")]
    Overloaded(String),

    #[error("rls enforcement check failed: {0}")]
    RlsNotEnforced(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl ShakoError {
    #[must_use]
    pub const fn slug(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "shako.not_found",
            Self::Unauthorized(_) => "shako.unauthorized",
            Self::Forbidden(_) => "shako.forbidden",
            Self::BadRequest(_) => "shako.bad_request",
            Self::Unprocessable(_) => "shako.unprocessable_entity",
            Self::Conflict(_) => "shako.conflict",
            Self::BehindBaseBranch(_) => "shako.behind_base_branch",
            Self::MethodNotAllowed(_) => "shako.method_not_allowed",
            Self::QuotaExceeded { .. } => "shako.quota_exceeded",
            Self::RateLimited(_) => "shako.rate_limited",
            Self::SsrfBlocked(_) => "shako.ssrf_blocked",
            Self::NotImplemented(_) => "shako.not_implemented",
            Self::Overloaded(_) => "shako.overloaded",
            Self::RlsNotEnforced(_) => "shako.rls_not_enforced",
            Self::Git(_) => "shako.git_error",
            Self::Storage(_) => "shako.storage_error",
            Self::Internal(_) => "shako.internal_error",
        }
    }

    #[must_use]
    pub const fn status_code(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) | Self::QuotaExceeded { .. } | Self::SsrfBlocked(_) => 403,
            Self::BadRequest(_) => 400,
            Self::Unprocessable(_) => 422,
            Self::Conflict(_) => 409,
            Self::BehindBaseBranch(_) | Self::MethodNotAllowed(_) => 405,
            Self::RateLimited(_) => 429,
            Self::NotImplemented(_) => 501,
            Self::Overloaded(_) => 503,
            Self::RlsNotEnforced(_) | Self::Git(_) | Self::Storage(_) | Self::Internal(_) => 500,
        }
    }

    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "Not Found",
            Self::Unauthorized(_) => "Unauthorized",
            Self::Forbidden(_) => "Forbidden",
            Self::BadRequest(_) => "Bad Request",
            Self::Unprocessable(_) => "Unprocessable Entity",
            Self::Conflict(_) => "Conflict",
            Self::BehindBaseBranch(_) | Self::MethodNotAllowed(_) => "Method Not Allowed",
            Self::QuotaExceeded { .. } => "Quota Exceeded",
            Self::RateLimited(_) => "Too Many Requests",
            Self::SsrfBlocked(_) => "Outbound Request Refused",
            Self::NotImplemented(_) => "Not Implemented",
            Self::Overloaded(_) => "Service Unavailable",
            Self::RlsNotEnforced(_) => "Tenant Isolation Check Failed",
            Self::Git(_) | Self::Storage(_) | Self::Internal(_) => "Internal Server Error",
        }
    }

    /// Public detail string that never leaks internal storage/git paths on 500.
    #[must_use]
    pub fn public_detail(&self) -> String {
        match self {
            Self::RlsNotEnforced(_) | Self::Storage(_) | Self::Internal(_) | Self::Git(_) => {
                "an internal server error occurred".to_owned()
            }
            Self::NotFound(m)
            | Self::Unauthorized(m)
            | Self::Forbidden(m)
            | Self::BadRequest(m)
            | Self::Unprocessable(m)
            | Self::Conflict(m)
            | Self::BehindBaseBranch(m)
            | Self::MethodNotAllowed(m)
            | Self::RateLimited(m)
            | Self::SsrfBlocked(m)
            | Self::NotImplemented(m)
            | Self::Overloaded(m) => m.clone(),
            Self::QuotaExceeded { .. } => self.to_string(),
        }
    }

    #[must_use]
    pub fn to_problem(&self, instance: Option<String>) -> ProblemDetails {
        ProblemDetails {
            type_uri: format!("urn:joinedcontext:problem:{}", self.slug()),
            slug: self.slug().to_owned(),
            title: self.title().to_owned(),
            status: self.status_code(),
            detail: self.public_detail(),
            instance,
            errors: Vec::new(),
        }
    }

    #[must_use]
    pub fn to_odata_error(&self) -> Value {
        let problem = self.to_problem(None);
        json!({
            "error": {
                "code": problem.slug,
                "message": problem.detail,
                "target": problem.type_uri,
                "details": [],
                "innererror": {
                    "type": problem.type_uri,
                    "status": problem.status,
                }
            }
        })
    }

    #[must_use]
    pub fn to_gitea_error(&self, docs_url: &str) -> GiteaErrorBody {
        GiteaErrorBody {
            message: self.public_detail(),
            url: docs_url.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mappings_preserve_quirks_and_hide_internal_details() {
        let behind = ShakoError::BehindBaseBranch(
            "Pull request is behind the base branch".into(),
        );
        assert_eq!(behind.status_code(), 405);
        assert!(
            behind
                .to_gitea_error("https://docs.example")
                .message
                .contains("behind the base branch")
        );

        let internal = ShakoError::Storage("secret db host error".into());
        assert_eq!(internal.status_code(), 500);
        assert!(!internal.public_detail().contains("secret"));
    }
}
