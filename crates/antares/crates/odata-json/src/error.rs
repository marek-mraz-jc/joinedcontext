// SPDX-License-Identifier: EUPL-1.2
//! The OData error body (JSON Format 4.01 section 21.1) and the errors of the body reader.
//!
//! Neither carries a value from the request (threat model O20): a message is fixed text per
//! kind, and a target names only a property the model declares, never a name the client sent
//! that the model does not know (it may be a hidden attribute's).

use std::fmt::{self, Display};
use std::io::Write;

use serde_json::{json, Map, Value};

/// An OData error (JSON Format 4.01 section 21.1). There is no `innererror`: it is where servers
/// leak stack traces (ADR-0024 decision 5).
///
/// ```
/// use odata_json::{write_error, ODataError};
/// let error = ODataError::new("NotFound", "the resource does not exist");
/// let mut body = Vec::new();
/// write_error(&error, &mut body)?;
/// assert_eq!(body, br#"{"error":{"code":"NotFound","message":"the resource does not exist"}}"#);
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ODataError {
    /// A service-defined code.
    pub code: String,
    /// Fixed text for people.
    pub message: String,
    /// The part of the request in error: a declared property, for example.
    pub target: Option<String>,
    /// Further errors.
    pub details: Vec<ErrorDetail>,
}

impl ODataError {
    /// An error with no target and no details.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            target: None,
            details: Vec::new(),
        }
    }
}

/// One entry of `details`.
///
/// ```
/// use odata_json::{write_error, ErrorDetail, ODataError};
/// let mut error = ODataError::new("BadRequest", "the request body is not valid");
/// error.target = Some("height".into());
/// error.details.push(ErrorDetail { code: "InvalidValue".into(), message: "wrong type".into(), target: None });
/// let mut body = Vec::new();
/// write_error(&error, &mut body)?;
/// assert!(String::from_utf8_lossy(&body).contains(r#""details":[{"code":"InvalidValue","message":"wrong type"}]"#));
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDetail {
    /// A service-defined code.
    pub code: String,
    /// Fixed text for people.
    pub message: String,
    /// The part of the request in error.
    pub target: Option<String>,
}

fn error_object(code: &str, message: &str, target: Option<&str>) -> Map<String, Value> {
    let mut object = Map::new();
    object.insert("code".into(), code.into());
    object.insert("message".into(), message.into());
    if let Some(target) = target {
        object.insert("target".into(), target.into());
    }
    object
}

/// Writes `{"error": {...}}`; `target` and `details` only when present.
///
/// # Errors
///
/// What `out` answers.
pub fn write_error<W: Write>(error: &ODataError, out: W) -> std::io::Result<()> {
    let mut object = error_object(&error.code, &error.message, error.target.as_deref());
    if !error.details.is_empty() {
        let details = error
            .details
            .iter()
            .map(|d| Value::Object(error_object(&d.code, &d.message, d.target.as_deref())))
            .collect();
        object.insert("details".into(), Value::Array(details));
    }
    serde_json::to_writer(out, &json!({ "error": object })).map_err(std::io::Error::from)
}

/// A bound of [`crate::BodyLimits`].
///
/// ```
/// use odata_json::BodyLimit;
/// assert_eq!(BodyLimit::BindLinks.as_str(), "bind links");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyLimit {
    /// `max_bytes`.
    Bytes,
    /// `max_depth`.
    Depth,
    /// `max_properties`.
    Properties,
    /// `max_bind_links`.
    BindLinks,
    /// `max_collection_len`.
    CollectionLength,
}

impl BodyLimit {
    /// The limit's name in a message.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bytes => "body size",
            Self::Depth => "nesting depth",
            Self::Properties => "properties",
            Self::BindLinks => "bind links",
            Self::CollectionLength => "collection length",
        }
    }
}

/// A body form outside the mapped subset (ADR-0024 decision 5, row `odata-json`).
///
/// ```
/// use odata_json::Feature;
/// assert_eq!(Feature::DeepInsert.as_str(), "deep insert");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Nested entities in a create (W3).
    DeepInsert,
    /// Nested entities or a nested delta in an update (W8).
    DeepUpdate,
    /// A bind on a collection-valued navigation property (W5).
    CollectionBind,
    /// A bind to `null`, which removes a relationship: the surface offers `DELETE .../$ref`
    /// (W17) for that.
    Unbind,
}

impl Feature {
    /// The feature's name in a message.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeepInsert => "deep insert",
            Self::DeepUpdate => "deep update",
            Self::CollectionBind => "binding a collection-valued navigation property",
            Self::Unbind => "binding a navigation property to null",
        }
    }
}

/// Why a request body was refused. Property and navigation names are ones the model declares.
///
/// ```
/// use odata_json::{BodyError, BodyLimit, ODataError};
/// let error = BodyError::LimitExceeded { limit: BodyLimit::Bytes, max: 1024 };
/// assert_eq!(error.status(), 413);
/// assert_eq!(ODataError::from(&error).code, "LimitExceeded");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyError {
    /// A bound of [`crate::BodyLimits`].
    LimitExceeded {
        /// Which bound.
        limit: BodyLimit,
        /// Its value.
        max: usize,
    },
    /// Not JSON, at a line and column of the body.
    Syntax {
        /// 1-based line.
        line: usize,
        /// 1-based column.
        column: usize,
    },
    /// An object with the same member twice (I-JSON, RFC 7493 section 2.3).
    DuplicateMember,
    /// The body is not a JSON object.
    NotAnObject,
    /// `@odata.type` names another type than the resource's.
    TypeMismatch,
    /// A property the model does not declare (the name is not repeated: it may be hidden).
    UnknownProperty,
    /// A value that does not have the declared type.
    InvalidValue {
        /// The property path, `/`-separated.
        property: String,
    },
    /// `null` for a property that is not nullable.
    NullNotAllowed {
        /// The property path, `/`-separated.
        property: String,
    },
    /// A create without a key property.
    MissingKey {
        /// The key property.
        property: String,
    },
    /// A create that leaves out, or names `null`, a property the set's
    /// `InsertRestrictions/RequiredProperties` lists.
    MissingProperty {
        /// The property.
        property: String,
    },
    /// A key, computed, non-insertable or non-updatable property in a write.
    ReadOnly {
        /// The property path, `/`-separated.
        property: String,
    },
    /// A bind that does not address an entity of the navigation's target type in this service.
    InvalidBind {
        /// The navigation property.
        navigation: String,
    },
    /// A form outside the mapped subset.
    NotImplemented(Feature),
}

impl BodyError {
    /// The HTTP status the surface answers with.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::LimitExceeded {
                limit: BodyLimit::Bytes,
                ..
            } => 413,
            Self::NotImplemented(_) => 501,
            _ => 400,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::LimitExceeded { .. } => "LimitExceeded",
            Self::Syntax { .. } | Self::DuplicateMember | Self::NotAnObject => "MalformedBody",
            Self::TypeMismatch => "TypeMismatch",
            Self::UnknownProperty => "UnknownProperty",
            Self::InvalidValue { .. } | Self::NullNotAllowed { .. } => "InvalidValue",
            Self::MissingKey { .. } => "MissingKey",
            Self::MissingProperty { .. } => "MissingProperty",
            Self::ReadOnly { .. } => "ReadOnly",
            Self::InvalidBind { .. } => "InvalidBind",
            Self::NotImplemented(_) => "NotImplemented",
        }
    }

    fn target(&self) -> Option<&str> {
        match self {
            Self::InvalidValue { property }
            | Self::NullNotAllowed { property }
            | Self::MissingKey { property }
            | Self::MissingProperty { property }
            | Self::ReadOnly { property } => Some(property),
            Self::InvalidBind { navigation } => Some(navigation),
            _ => None,
        }
    }
}

/// Fixed text per kind; the target is not repeated.
impl Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded { limit, max } => {
                write!(
                    f,
                    "the request body exceeds the {} limit of {max}",
                    limit.as_str()
                )
            }
            Self::Syntax { line, column } => {
                write!(
                    f,
                    "the request body is not JSON (line {line}, column {column})"
                )
            }
            Self::DuplicateMember => f.write_str("the request body names a member twice"),
            Self::NotAnObject => f.write_str("the request body is not a JSON object"),
            Self::TypeMismatch => f.write_str("the request body names another type"),
            Self::UnknownProperty => f.write_str("the request body names an unknown property"),
            Self::InvalidValue { .. } => f.write_str("a value does not have the declared type"),
            Self::NullNotAllowed { .. } => f.write_str("a property that is not nullable is null"),
            Self::MissingKey { .. } => f.write_str("the key property is missing"),
            Self::MissingProperty { .. } => f.write_str("a required property is missing"),
            Self::ReadOnly { .. } => f.write_str("the property cannot be written"),
            Self::InvalidBind { .. } => {
                f.write_str("the bind does not address an entity this navigation property reaches")
            }
            Self::NotImplemented(feature) => write!(f, "{} is not implemented", feature.as_str()),
        }
    }
}

impl std::error::Error for BodyError {}

impl From<&BodyError> for ODataError {
    fn from(error: &BodyError) -> Self {
        Self {
            code: error.code().into(),
            message: error.to_string(),
            target: error.target().map(str::to_owned),
            details: Vec::new(),
        }
    }
}
