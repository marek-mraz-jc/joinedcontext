#![forbid(unsafe_code)]
//! RFC 9457 Problem Details and RFC 6749 OAuth error responses (W12).

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, KaisatsuError>;

/// Errors across kaisatsu crates. Never contains secrets, passwords, tokens, or hashes (W12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum KaisatsuError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("second factor required")]
    SecondFactorRequired,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("validation error: {message}")]
    Validation {
        message: String,
        fields: BTreeMap<String, Vec<String>>,
    },

    #[error("oauth error ({error}): {description}")]
    OAuth {
        status: u16,
        error: String,
        description: String,
    },

    #[error("rate limited: {message}")]
    RateLimited {
        message: String,
        retry_after_secs: u64,
    },

    #[error("outbound fetch refused: {0}")]
    OutboundRefused(String),

    #[error("not supported: {0}")]
    NotSupported(String),

    #[error("stale plan: {0}")]
    StalePlan(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("crypto error: {0}")]
    Crypto(String),
}

impl KaisatsuError {
    pub fn oauth(status: u16, error: impl Into<String>, description: impl Into<String>) -> Self {
        Self::OAuth {
            status,
            error: error.into(),
            description: description.into(),
        }
    }

    pub fn invalid_request(description: impl Into<String>) -> Self {
        Self::oauth(400, "invalid_request", description)
    }

    pub fn invalid_client(description: impl Into<String>) -> Self {
        Self::oauth(401, "invalid_client", description)
    }

    pub fn invalid_grant(description: impl Into<String>) -> Self {
        Self::oauth(400, "invalid_grant", description)
    }

    pub fn unauthorized_client(description: impl Into<String>) -> Self {
        Self::oauth(400, "unauthorized_client", description)
    }

    pub fn unsupported_grant_type(description: impl Into<String>) -> Self {
        Self::oauth(400, "unsupported_grant_type", description)
    }

    pub fn unsupported_response_type(description: impl Into<String>) -> Self {
        Self::oauth(400, "unsupported_response_type", description)
    }

    pub fn invalid_scope(description: impl Into<String>) -> Self {
        Self::oauth(400, "invalid_scope", description)
    }

    pub fn invalid_token(description: impl Into<String>) -> Self {
        Self::oauth(400, "invalid_token", description)
    }

    pub fn use_dpop_nonce(description: impl Into<String>) -> Self {
        Self::oauth(400, "use_dpop_nonce", description)
    }

    pub fn invalid_dpop_proof(description: impl Into<String>) -> Self {
        Self::oauth(400, "invalid_dpop_proof", description)
    }

    pub fn field_error(field: impl Into<String>, problem: impl Into<String>) -> Self {
        let field = field.into();
        let problem = problem.into();
        let message = format!("{field}: {problem}");
        let mut fields = BTreeMap::new();
        fields.insert(field, vec![problem]);
        Self::Validation { message, fields }
    }

    /// Stable machine-readable problem slug (W12).
    pub fn slug(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not-found",
            Self::Forbidden(_) => "forbidden",
            Self::Unauthorized(_) => "unauthorized",
            Self::SecondFactorRequired => "second-factor-required",
            Self::Conflict(_) => "conflict",
            Self::BadRequest(_) => "invalid-request",
            Self::Validation { .. } => "validation-error",
            Self::OAuth { .. } => "oauth-error",
            Self::RateLimited { .. } => "rate-limited",
            Self::OutboundRefused(_) => "outbound-refused",
            Self::NotSupported(_) => "not-supported",
            Self::StalePlan(_) => "stale-plan",
            Self::Storage(_) => "storage-error",
            Self::Crypto(_) => "crypto-error",
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::Forbidden(_) | Self::SecondFactorRequired => 403,
            Self::Unauthorized(_) => 401,
            Self::Conflict(_) => 409,
            Self::BadRequest(_) | Self::OutboundRefused(_) => 400,
            Self::Validation { .. } => 422,
            Self::OAuth { status, .. } => *status,
            Self::RateLimited { .. } => 429,
            Self::NotSupported(_) => 501,
            Self::StalePlan(_) => 412,
            Self::Storage(_) | Self::Crypto(_) => 500,
        }
    }

    pub fn to_problem(&self) -> ProblemDetails {
        let status = self.status_code();
        let slug = self.slug();
        let detail = self.public_detail();
        ProblemDetails {
            r#type: format!("urn:joinedcontext:problem:{slug}"),
            title: slug.replace('-', " "),
            status,
            slug: slug.to_owned(),
            detail: detail.clone(),
            error_message: Some(detail),
        }
    }

    /// The text a caller may see. Storage and crypto failures carry internals (SQL, key ids, file
    /// paths) and outbound refusals carry resolved addresses of the internal network, so those
    /// answer a fixed sentence; the full error stays with the server.
    #[must_use]
    pub fn public_detail(&self) -> String {
        match self {
            Self::Storage(_) | Self::Crypto(_) => "internal server error".to_owned(),
            Self::OutboundRefused(_) => "outbound fetch refused".to_owned(),
            other => other.to_string(),
        }
    }
}

/// RFC 9457 Problem Details, carrying `errorMessage` as well for Keycloak Admin REST compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemDetails {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub slug: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// RFC 6749 OAuth 2.0 error body returned on protocol endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
}

impl IntoResponse for KaisatsuError {
    fn into_response(self) -> Response {
        match &self {
            Self::OAuth {
                status,
                error,
                description,
            } => {
                let code = StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_REQUEST);
                let body = OAuthErrorResponse {
                    error: error.clone(),
                    error_description: if description.is_empty() {
                        None
                    } else {
                        Some(description.clone())
                    },
                };
                let json = serde_json::to_string(&body)
                    .unwrap_or_else(|_| r#"{"error":"server_error"}"#.to_owned());
                (
                    code,
                    [
                        (header::CONTENT_TYPE, "application/json"),
                        (header::CACHE_CONTROL, "no-store"),
                    ],
                    json,
                )
                    .into_response()
            }
            Self::RateLimited {
                retry_after_secs, ..
            } => {
                let problem = self.to_problem();
                let json = serde_json::to_string(&problem).unwrap_or_default();
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [
                        (header::CONTENT_TYPE, "application/problem+json"),
                        (header::RETRY_AFTER, &retry_after_secs.to_string()),
                    ],
                    json,
                )
                    .into_response()
            }
            Self::Unauthorized(msg) if msg.starts_with("use_dpop_nonce:") => {
                let nonce = msg.strip_prefix("use_dpop_nonce:").unwrap_or_default();
                let problem = self.to_problem();
                let json = serde_json::to_string(&problem).unwrap_or_default();
                (
                    StatusCode::UNAUTHORIZED,
                    [
                        (header::CONTENT_TYPE, "application/problem+json"),
                        (header::CACHE_CONTROL, "no-store"),
                        (
                            header::WWW_AUTHENTICATE,
                            r#"DPoP error="use_dpop_nonce", error_description="Resource server requires nonce in DPoP proof", algs="ES256 RS256 EdDSA""#,
                        ),
                        (header::HeaderName::from_static("dpop-nonce"), nonce),
                    ],
                    json,
                )
                    .into_response()
            }
            _ => {
                let problem = self.to_problem();
                let code = StatusCode::from_u16(problem.status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let json = serde_json::to_string(&problem).unwrap_or_default();
                (
                    code,
                    [
                        (header::CONTENT_TYPE, "application/problem+json"),
                        (header::CACHE_CONTROL, "no-store"),
                    ],
                    json,
                )
                    .into_response()
            }
        }
    }
}

impl From<odata_table::TableError> for KaisatsuError {
    fn from(err: odata_table::TableError) -> Self {
        match err {
            odata_table::TableError::NotImplemented(w) => Self::NotSupported(w),
            odata_table::TableError::UnknownProperty(p) => {
                Self::BadRequest(format!("unknown property: {p}"))
            }
            odata_table::TableError::BadRequest(msg) => Self::BadRequest(msg),
            odata_table::TableError::Source(msg) => Self::Storage(msg),
        }
    }
}

#[cfg(test)]
mod problem_tests {
    use super::*;

    #[test]
    fn internal_errors_never_reach_the_caller() {
        for err in [
            KaisatsuError::Storage("relation \"wk_user\" does not exist; host=10.0.0.7".into()),
            KaisatsuError::Crypto("cannot read /run/secrets/kek".into()),
            KaisatsuError::OutboundRefused("resolved address 10.43.0.10 is private".into()),
        ] {
            let p = err.to_problem();
            assert!(!p.detail.contains("10."), "{}", p.detail);
            assert!(!p.detail.contains("/run/secrets"));
            assert!(!p.detail.contains("wk_user"));
            assert_eq!(p.error_message.as_deref(), Some(p.detail.as_str()));
        }
        let nf = KaisatsuError::NotFound("user not found".into()).to_problem();
        assert_eq!(nf.status, 404);
        assert!(nf.detail.contains("user not found"));
    }
}
