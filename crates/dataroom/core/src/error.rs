#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Result alias for dataroom operations.
pub type Result<T> = std::result::Result<T, DataroomError>;

/// Errors produced across dataroom crates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum DataroomError {
    /// Resource was not found or caller is not allowed to know it exists.
    #[error("not found: {0}")]
    NotFound(String),

    /// Caller is authenticated/identified, the resource is visible, and the action is denied.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// Caller is not authenticated when authentication is required.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// Input validation failed on one or more fields.
    #[error("validation error: {message}")]
    Validation {
        message: String,
        fields: BTreeMap<String, Vec<String>>,
    },

    /// SHACL validation refused public visibility or profile activation.
    #[error("SHACL validation failed for profile {profile}: {violations} violation(s)")]
    ShaclViolation {
        profile: String,
        violations: usize,
        details: Vec<String>,
    },

    /// Conflict (e.g. duplicate dataset slug or host name).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Feature or action is intentionally not supported (e.g. `datastore_*` actions, C3).
    #[error("not supported: {0}")]
    NotSupported(String),

    /// Outbound fetch or SSRF guard refusal.
    #[error("outbound fetch refused: {0}")]
    OutboundRefused(String),

    /// RDF or SHACL parsing limit exceeded or syntax error.
    #[error("RDF error: {0}")]
    Rdf(String),

    /// Underlying storage or tenant setting error.
    #[error("storage error: {0}")]
    Storage(String),
}

impl DataroomError {
    /// Constructs a single-field validation error.
    pub fn field_error(field: impl Into<String>, problem: impl Into<String>) -> Self {
        let field = field.into();
        let problem = problem.into();
        let message = format!("{field}: {problem}");
        let mut fields = BTreeMap::new();
        fields.insert(field, vec![problem]);
        Self::Validation { message, fields }
    }

    /// HTTP status code corresponding to this error.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::Forbidden(_) => 403,
            Self::Unauthorized(_) => 401,
            Self::Validation { .. } | Self::ShaclViolation { .. } | Self::Rdf(_) => 422,
            Self::Conflict(_) => 409,
            Self::NotSupported(_) => 501,
            Self::OutboundRefused(_) => 400,
            Self::Storage(_) => 500,
        }
    }

    /// CKAN Action API `__type` string.
    pub fn ckan_error_type(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "Not Found Error",
            Self::Forbidden(_) | Self::Unauthorized(_) => "Authorization Error",
            Self::Validation { .. } | Self::ShaclViolation { .. } | Self::Conflict(_) => {
                "Validation Error"
            }
            Self::NotSupported(_) => "Not Supported Error",
            Self::OutboundRefused(_) | Self::Rdf(_) => "Validation Error",
            Self::Storage(_) => "Internal Server Error",
        }
    }
}

impl From<odata_table::TableError> for DataroomError {
    fn from(err: odata_table::TableError) -> Self {
        match err {
            odata_table::TableError::NotImplemented(w) => Self::NotSupported(w),
            odata_table::TableError::UnknownProperty(p) => {
                Self::field_error(p, "unknown property")
            }
            odata_table::TableError::BadRequest(msg) => Self::Validation {
                message: msg,
                fields: BTreeMap::new(),
            },
            odata_table::TableError::Source(msg) => Self::Storage(msg),
        }
    }
}
