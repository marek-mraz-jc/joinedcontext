// SPDX-License-Identifier: EUPL-1.2
//! OData 4.01 `$batch` (ADR-0024 decision 1 and 5, row `odata-batch`): the request bodies of
//! a `multipart/mixed` batch (Protocol 4.01 section 11.7.7, [`parse_multipart`]) and a JSON
//! batch (JSON Format 4.01 section 19, [`parse_json`]) read into one [`Batch`] under the caller's
//! [`Limits`], and the responses written back in the same format ([`write_multipart`],
//! [`write_json`]).
//!
//! The members run in array order: a JSON `dependsOn` may name only an earlier member (section
//! 19.1), so the order of the batch already satisfies it; [`Request::depends_on`] holds the
//! indices, so a caller under `continue-on-error` answers a member whose dependency failed with
//! 424. Outside the mapped subset: change sets and `atomicityGroup` (atomicity, W24, W25),
//! Content-ID references (`$1/...`, W26) and media bodies answer
//! [`BatchError::NotImplemented`]; a batch inside a batch is [`BatchError::NestedBatch`].
//!
//! The parser returns each member's headers as written (threat model O9): the surface keeps
//! only the OData ones and takes the tenant and the subject from the outer request, never from a
//! member. The crate knows nothing about Antares (`dev/check-module-rules.py`).
//!
//! ```
//! use odata_batch::{parse_json, write_json, Limits, Method, Response};
//!
//! let body = br#"{"requests":[{"id":"1","method":"get","url":"Buildings('b1')"}]}"#;
//! let batch = parse_json(body, &Limits::strict())?;
//! assert_eq!(batch.requests[0].method, Method::Get);
//!
//! let answer = Response { id: Some("1".into()), status: 200, headers: vec![], body: br#"{"id":"b1"}"#.to_vec() };
//! let mut out = Vec::new();
//! write_json(&[answer], &mut out).map_err(|_| odata_batch::BatchError::Malformed)?;
//! assert_eq!(out, br#"{"responses":[{"id":"1","status":200,"body":{"id":"b1"}}]}"#);
//! # Ok::<(), odata_batch::BatchError>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod json;
mod multipart;
mod write;

pub use error::{BatchError, BatchLimit, Feature};
pub use json::parse_json;
pub use multipart::{boundary, parse_multipart};
pub use write::{write_json, write_multipart};

/// The bounds of one `$batch` request (ADR-0024 decision 4, threat model O11). Every parser
/// takes them from the caller; there is no default on a request path.
///
/// ```
/// use odata_batch::Limits;
/// let limits = Limits { max_requests: 10, ..Limits::strict() };
/// assert_eq!(limits.max_requests, 10);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Bytes of the whole batch body.
    pub max_body_bytes: usize,
    /// Members of the batch.
    pub max_requests: usize,
    /// Bytes of one member's header block (MIME part headers and request headers each).
    pub max_part_header_bytes: usize,
    /// Headers of one member (MIME part headers and request headers each).
    pub max_headers_per_part: usize,
    /// `dependsOn` entries of one member.
    pub max_depends_on: usize,
    /// Bytes of a member id (`Content-ID`, `id`).
    pub max_id_len: usize,
    /// Bytes of a member URL.
    pub max_url_len: usize,
    /// Nesting of objects and arrays in one member's JSON body (finding F3: a multipart body
    /// is not depth-scanned by the wall).
    pub max_json_depth: usize,
}

impl Limits {
    /// Bounds for tests and documentation: 1 MiB, 100 members (`MAX_BATCH_ITEMS` of the NGSI-LD
    /// batch operations), 8 KiB and 32 headers per block, 16 `dependsOn`, 64-byte ids, 2 KiB
    /// URLs, JSON depth 16.
    #[must_use]
    pub fn strict() -> Self {
        Self {
            max_body_bytes: 1 << 20,
            max_requests: 100,
            max_part_header_bytes: 8 << 10,
            max_headers_per_part: 32,
            max_depends_on: 16,
            max_id_len: 64,
            max_url_len: 2048,
            max_json_depth: 16,
        }
    }
}

/// The method of a member.
///
/// ```
/// use odata_batch::Method;
/// assert_eq!(Method::parse("patch"), Some(Method::Patch));
/// assert_eq!(Method::Delete.as_str(), "DELETE");
/// assert_eq!(Method::parse("HEAD"), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `PATCH`.
    Patch,
    /// `DELETE`.
    Delete,
}

impl Method {
    const ALL: [Self; 5] = [Self::Get, Self::Post, Self::Put, Self::Patch, Self::Delete];

    /// The method a token names, ASCII case-insensitive (JSON batches write it in lower case);
    /// `None` for one outside the five.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(token))
    }

    /// The method token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// One member of a batch.
///
/// ```
/// use odata_batch::{Method, Request};
/// let get = Request { id: None, method: Method::Get, url: "Buildings".into(), headers: vec![], body: vec![], depends_on: vec![] };
/// assert!(get.body.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `Content-ID` (multipart) or `id` (JSON).
    pub id: Option<String>,
    /// The method.
    pub method: Method,
    /// The URL as written: relative, absolute path or absolute URI (Protocol 11.7.7 lets the
    /// client choose); the caller checks that it names this service.
    pub url: String,
    /// Request headers as written, names as the client spelled them.
    pub headers: Vec<(String, String)>,
    /// The body bytes; a JSON body as compact JSON.
    pub body: Vec<u8>,
    /// Indices of earlier members this one depends on (JSON `dependsOn`).
    pub depends_on: Vec<usize>,
}

/// A parsed batch: its members in order.
///
/// ```
/// use odata_batch::Batch;
/// assert!(Batch::default().requests.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Batch {
    /// The members.
    pub requests: Vec<Request>,
}

/// The answer to one member, as the caller ran it.
///
/// ```
/// use odata_batch::Response;
/// let created = Response { id: Some("1".into()), status: 201, headers: vec![("Location".into(), "Buildings('b1')".into())], body: vec![] };
/// assert_eq!(created.status, 201);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The member's id, echoed.
    pub id: Option<String>,
    /// The HTTP status.
    pub status: u16,
    /// Response headers; a value with a line break is refused by the writers.
    pub headers: Vec<(String, String)>,
    /// The body bytes.
    pub body: Vec<u8>,
}

/// The value of header `name` (ASCII case-insensitive), the first if repeated.
pub(crate) fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Whether a media type is JSON (`application/json`, `...+json`, with parameters).
pub(crate) fn is_json(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence == "application/json" || essence.ends_with("+json")
}

/// The checks every member gets in both formats: id and URL length, no Content-ID reference,
/// no nested batch, a JSON body within the depth bound.
pub(crate) fn check_member(
    request: &Request,
    earlier: &[Request],
    limits: &Limits,
) -> Result<(), BatchError> {
    if request
        .id
        .as_ref()
        .is_some_and(|id| id.len() > limits.max_id_len)
    {
        return Err(BatchError::limit(BatchLimit::IdLength, limits.max_id_len));
    }
    if request.url.len() > limits.max_url_len {
        return Err(BatchError::limit(BatchLimit::UrlLength, limits.max_url_len));
    }
    if let Some(id) = &request.id {
        if earlier.iter().any(|e| e.id.as_ref() == Some(id)) {
            return Err(BatchError::DuplicateId);
        }
    }
    // `$1/...`: a reference to an earlier member's result (Protocol 11.7.7.2).
    if let Some(rest) = request.url.strip_prefix('$') {
        let name = rest.split(['/', '?']).next().unwrap_or_default();
        if earlier.iter().any(|e| e.id.as_deref() == Some(name)) {
            return Err(BatchError::NotImplemented(Feature::ContentIdReference));
        }
    }
    let path = request.url.split('?').next().unwrap_or_default();
    if path.rsplit('/').next() == Some("$batch") {
        return Err(BatchError::NestedBatch);
    }
    let json = header(&request.headers, "Content-Type").is_none_or(is_json);
    if json && json_depth(&request.body) > limits.max_json_depth {
        return Err(BatchError::limit(
            BatchLimit::JsonDepth,
            limits.max_json_depth,
        ));
    }
    Ok(())
}

/// The deepest nesting of objects and arrays in `bytes`, strings skipped; the bytes need not
/// be valid JSON (the member's own parser judges that later).
pub(crate) fn json_depth(bytes: &[u8]) -> usize {
    let (mut depth, mut deepest, mut in_string, mut escaped) = (0usize, 0usize, false, false);
    for &byte in bytes {
        match byte {
            _ if escaped => escaped = false,
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            _ if in_string => {}
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                deepest = deepest.max(depth);
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}
