// SPDX-License-Identifier: EUPL-1.2
//! Why a batch was refused: fixed text per kind and a byte offset at most, never a value from
//! the request.

use std::fmt::{self, Display};

/// A bound of [`crate::Limits`].
///
/// ```
/// use odata_batch::BatchLimit;
/// assert_eq!(BatchLimit::Requests.as_str(), "members");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchLimit {
    /// `max_body_bytes`.
    BodyBytes,
    /// `max_requests`.
    Requests,
    /// `max_part_header_bytes`.
    HeaderBytes,
    /// `max_headers_per_part`.
    Headers,
    /// `max_depends_on`.
    DependsOn,
    /// `max_id_len`.
    IdLength,
    /// `max_url_len`.
    UrlLength,
    /// `max_json_depth`.
    JsonDepth,
}

impl BatchLimit {
    /// The bound's name in a message.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BodyBytes => "body size",
            Self::Requests => "members",
            Self::HeaderBytes => "header block size",
            Self::Headers => "headers per member",
            Self::DependsOn => "dependsOn entries",
            Self::IdLength => "id length",
            Self::UrlLength => "URL length",
            Self::JsonDepth => "JSON nesting depth",
        }
    }
}

/// A batch form outside the mapped subset (ADR-0024 decision 5, row `odata-batch`).
///
/// ```
/// use odata_batch::Feature;
/// assert_eq!(Feature::ChangeSet.as_str(), "change sets");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// A multipart change set (Protocol 11.7.7.1): atomic, which NGSI-LD has no transaction for.
    ChangeSet,
    /// A JSON `atomicityGroup` (JSON Format 19.1), for the same reason.
    AtomicityGroup,
    /// A URL that references an earlier member's result (`$1/...`, Protocol 11.7.7.2).
    ContentIdReference,
    /// A method other than GET, POST, PUT, PATCH and DELETE.
    Method,
    /// A JSON batch member body that is neither JSON nor text (base64url, JSON Format 19.1).
    MediaBody,
}

impl Feature {
    /// The feature's name in a message.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChangeSet => "change sets",
            Self::AtomicityGroup => "atomicity groups",
            Self::ContentIdReference => "references to another member's result",
            Self::Method => "this method in a batch",
            Self::MediaBody => "binary member bodies",
        }
    }
}

/// Why a batch was refused.
///
/// ```
/// use odata_batch::{BatchError, BatchLimit, Feature};
/// assert_eq!(BatchError::limit(BatchLimit::BodyBytes, 9).status(), 413);
/// assert_eq!(BatchError::NotImplemented(Feature::ChangeSet).status(), 501);
/// assert_eq!(BatchError::Framing { offset: 4 }.to_string(), "the multipart body is not framed as RFC 2046 asks (byte 4)");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchError {
    /// A bound of [`crate::Limits`].
    LimitExceeded {
        /// Which bound.
        limit: BatchLimit,
        /// Its value.
        max: usize,
    },
    /// The `Content-Type` is not `multipart/mixed` with a valid boundary.
    ContentType,
    /// The multipart body breaks the framing: no CRLF, a stray delimiter, text outside the
    /// parts, a header that is not `name: value` (threat model O10).
    Framing {
        /// Byte offset in the body.
        offset: usize,
    },
    /// A part that is not `application/http` with binary transfer encoding.
    PartType {
        /// Byte offset of the part.
        offset: usize,
    },
    /// A member's request line is not `METHOD url [HTTP/1.1]`.
    RequestLine {
        /// Byte offset of the line.
        offset: usize,
    },
    /// A member's `Content-Length` disagrees with its bytes.
    ContentLength {
        /// Byte offset of the part.
        offset: usize,
    },
    /// A JSON batch that is not JSON.
    Json {
        /// 1-based line.
        line: usize,
        /// 1-based column.
        column: usize,
    },
    /// A JSON batch member or response member that is not the section 19 shape.
    Malformed,
    /// Two members with the same id.
    DuplicateId,
    /// A `dependsOn` naming no earlier member.
    UnknownDependency,
    /// A member that is itself a `$batch` request.
    NestedBatch,
    /// A form outside the mapped subset.
    NotImplemented(Feature),
}

impl BatchError {
    /// [`BatchError::LimitExceeded`] for `limit` at `max`.
    #[must_use]
    pub fn limit(limit: BatchLimit, max: usize) -> Self {
        Self::LimitExceeded { limit, max }
    }

    /// The HTTP status the surface answers the whole batch with.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::LimitExceeded {
                limit: BatchLimit::BodyBytes,
                ..
            } => 413,
            Self::NotImplemented(_) => 501,
            _ => 400,
        }
    }
}

impl Display for BatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded { limit, max } => {
                write!(f, "the batch exceeds the {} limit of {max}", limit.as_str())
            }
            Self::ContentType => {
                f.write_str("the batch is not multipart/mixed with a valid boundary")
            }
            Self::Framing { offset } => {
                write!(
                    f,
                    "the multipart body is not framed as RFC 2046 asks (byte {offset})"
                )
            }
            Self::PartType { offset } => {
                write!(
                    f,
                    "a part is not a binary application/http request (byte {offset})"
                )
            }
            Self::RequestLine { offset } => {
                write!(
                    f,
                    "a member's request line is not METHOD url HTTP/1.1 (byte {offset})"
                )
            }
            Self::ContentLength { offset } => {
                write!(
                    f,
                    "a member's Content-Length disagrees with its body (byte {offset})"
                )
            }
            Self::Json { line, column } => {
                write!(f, "the batch is not JSON (line {line}, column {column})")
            }
            Self::Malformed => {
                f.write_str("the batch is not shaped as JSON Format section 19 asks")
            }
            Self::DuplicateId => f.write_str("two members have the same id"),
            Self::UnknownDependency => f.write_str("dependsOn names no earlier member"),
            Self::NestedBatch => f.write_str("a batch member is itself a batch"),
            Self::NotImplemented(feature) => write!(f, "not implemented: {}", feature.as_str()),
        }
    }
}

impl std::error::Error for BatchError {}
