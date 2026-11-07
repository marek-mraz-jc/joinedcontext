// SPDX-License-Identifier: EUPL-1.2
//! OData 4.01 over axum: HTTP to the library types and back (ADR-0024 decision 1, row
//! `odata-axum`).
//!
//! [`metadata_router`] serves the service document (`GET /`) and the metadata document
//! (`GET /$metadata`, CSDL XML by default, CSDL JSON on 4.01 when asked) of one
//! [`odata_csdl::Model`]. [`ODataRequest`] parses a request's URL against that model and
//! negotiates its version, for the routes a surface adds; [`ODataErrorResponse`] writes the
//! OData error body. Every limit is a field of [`ODataState`] the caller fills (decision 4): the
//! crate has no default on a request path. It knows nothing of the service behind the model.
//!
//! ```
//! use std::sync::Arc;
//! use odata_axum::{metadata_router, ODataState};
//! use odata_csdl::{CsdlVersion, Model};
//!
//! let model = Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![] };
//! let state = ODataState {
//!     model: Arc::new(model),
//!     url_model: None,
//!     service_root: "https://example.org/odata/".into(),
//!     url_limits: odata_url::Limits::strict(),
//!     csdl_limits: odata_csdl::Limits::strict(),
//!     max_header_len: 1024,
//!     metadata_etag: None,
//!     custom_options: vec![],
//! };
//! // `nest_service`, so both `/odata` and the service root URL `/odata/` reach `GET /`.
//! let app: axum::Router = axum::Router::new().nest_service("/odata", metadata_router(Arc::new(state)));
//! ```

#![forbid(unsafe_code)]

use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use odata_json::{negotiate_version, write_error, Ctx, MetadataLevel, ODataError, ODataVersion};
use odata_url::{EdmModel, Format, LimitKind, ODataUri, QueryOptions, UrlError, UrlErrorKind};

/// One OData service: its model, where it is served, and every limit a request is held to.
///
/// ```
/// # use std::sync::Arc;
/// # use odata_csdl::{CsdlVersion, Model};
/// let state = odata_axum::ODataState {
///     model: Arc::new(Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![] }),
///     url_model: None,
///     service_root: "https://example.org/odata/".into(),
///     url_limits: odata_url::Limits::strict(),
///     csdl_limits: odata_csdl::Limits::strict(),
///     max_header_len: 1024,
///     metadata_etag: None,
///     custom_options: vec![],
/// };
/// assert!(state.service_root.ends_with('/'));
/// ```
#[derive(Debug, Clone)]
pub struct ODataState {
    /// The model `$metadata` describes, and the URLs resolve against unless `url_model` is set.
    pub model: Arc<odata_csdl::Model>,
    /// The model the URLs resolve against when it names more than `model`: a 4.0 document
    /// serves an `Edm.Untyped` property as a dynamic property of an open type (CSDL XML 3.4),
    /// and only the 4.01 document names it, so an undeclared name stays unknown.
    pub url_model: Option<Arc<odata_csdl::Model>>,
    /// The service root URL, ending in `/`: context URLs start with it.
    pub service_root: String,
    /// The URL parser's limits.
    pub url_limits: odata_url::Limits,
    /// The CSDL writers' limits.
    pub csdl_limits: odata_csdl::Limits,
    /// Bytes of one `OData-Version`, `OData-MaxVersion` or `Accept` header this service reads.
    pub max_header_len: usize,
    /// The XML metadata document's `ETag` ([`metadata_etag`]), written as
    /// `@odata.metadataEtag` in every JSON answer; `None` writes none.
    pub metadata_etag: Option<String>,
    /// The custom query options (URL Conventions 5.2: no `$`, no `@`) the service reads on a
    /// resource path, by their exact name; a request naming any other one, or any one on the
    /// service document, `$metadata`, `$batch` or `$entity`, is 400.
    pub custom_options: Vec<String>,
}

impl ODataState {
    /// The model the URLs resolve against: `url_model`, else `model`.
    #[must_use]
    pub fn url_model(&self) -> &odata_csdl::Model {
        self.url_model.as_deref().unwrap_or(&self.model)
    }
}

/// A request's URL, parsed against the service's model, and the version its response is
/// written in (Protocol 4.01 8.2.6, 8.2.7). A custom query option the state does not name in
/// [`ODataState::custom_options`] is refused (400, T-3441 row Q27), as NGSI-LD refuses an
/// unknown parameter.
///
/// ```
/// # use std::sync::Arc;
/// # use odata_csdl::{CsdlVersion, Model};
/// use axum::{routing::get, Router};
/// use odata_axum::{ODataRequest, ODataState};
///
/// async fn handler(request: ODataRequest) -> String {
///     format!("{} {}", request.version, request.uri)
/// }
/// # let state = ODataState {
/// #     model: Arc::new(Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![] }),
/// #     url_model: None,
/// #     service_root: "https://example.org/odata/".into(),
/// #     url_limits: odata_url::Limits::strict(),
/// #     csdl_limits: odata_csdl::Limits::strict(),
/// #     max_header_len: 1024,
/// #     metadata_etag: None,
/// #     custom_options: vec![],
/// # };
/// let app: Router = Router::new().route("/{*rest}", get(handler)).with_state(Arc::new(state));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ODataRequest {
    /// The parsed URL, relative to the service root.
    pub uri: ODataUri,
    /// The negotiated version.
    pub version: ODataVersion,
}

impl<S> FromRequestParts<S> for ODataRequest
where
    S: Send + Sync,
    Arc<ODataState>: FromRef<S>,
{
    type Rejection = ODataErrorResponse;

    fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        // Nothing to wait for: the URL and the headers are already read.
        std::future::ready(Self::read(parts, &Arc::<ODataState>::from_ref(state)))
    }
}

impl ODataRequest {
    fn read(parts: &Parts, state: &ODataState) -> Result<Self, ODataErrorResponse> {
        let version = version(&parts.headers, state.max_header_len)?;
        // Under `nest` or `nest_service`, the path is the part below the service root.
        let path = parts
            .uri
            .path()
            .strip_prefix('/')
            .unwrap_or(parts.uri.path());
        let relative = match parts.uri.query() {
            Some(query) => format!("{path}?{query}"),
            None => path.to_owned(),
        };
        let uri = odata_url::parse(&relative, &EdmModel(state.url_model()), &state.url_limits)
            .map_err(|error| url_error(&error, path.len()).with_version(version))?;
        let resource = matches!(uri, ODataUri::Resource { .. });
        let unknown = |options: &QueryOptions| {
            options.custom.iter().any(|(name, _)| {
                !resource || !state.custom_options.iter().any(|known| known == name)
            })
        };
        if options(&uri).is_some_and(unknown) {
            return Err(ODataErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "BadRequest",
                "custom query options are not supported",
            )
            .with_version(version));
        }
        Ok(Self { uri, version })
    }
}

/// The query options of any URL form.
fn options(uri: &ODataUri) -> Option<&QueryOptions> {
    match uri {
        ODataUri::ServiceDocument => None,
        ODataUri::Metadata { options }
        | ODataUri::Batch { options }
        | ODataUri::Entity { options }
        | ODataUri::Resource { options, .. } => Some(options),
    }
}

/// The OData error body with its status (Protocol 4.01 section 21.1 shape): fixed messages, no
/// `innererror`, no request value.
///
/// ```
/// use axum::{http::StatusCode, response::IntoResponse};
/// use odata_axum::ODataErrorResponse;
/// let response = ODataErrorResponse::new(StatusCode::NOT_FOUND, "NotFound", "no such resource").into_response();
/// assert_eq!(response.status(), StatusCode::NOT_FOUND);
/// assert_eq!(response.headers()["content-type"], "application/json");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ODataErrorResponse {
    /// The HTTP status.
    pub status: StatusCode,
    /// The body.
    pub error: ODataError,
    /// The `OData-Version` header, once a version was negotiated.
    pub version: Option<ODataVersion>,
}

impl ODataErrorResponse {
    /// An error with no target, no details and no negotiated version.
    #[must_use]
    pub fn new(status: StatusCode, code: &str, message: &str) -> Self {
        Self {
            status,
            error: ODataError::new(code, message),
            version: None,
        }
    }

    /// The same error, answered in `version`.
    ///
    /// ```
    /// # use axum::{http::StatusCode, response::IntoResponse};
    /// # use odata_axum::ODataErrorResponse;
    /// use odata_json::ODataVersion;
    /// let response = ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", "no")
    ///     .with_version(ODataVersion::V4_01)
    ///     .into_response();
    /// assert_eq!(response.headers()["odata-version"], "4.01");
    /// ```
    #[must_use]
    pub fn with_version(mut self, version: ODataVersion) -> Self {
        self.version = Some(version);
        self
    }
}

impl IntoResponse for ODataErrorResponse {
    fn into_response(self) -> Response {
        let mut body = Vec::new();
        // A Vec takes every byte: the write cannot fail.
        let _ = write_error(&self.error, &mut body);
        let mut response = (
            self.status,
            [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            body,
        )
            .into_response();
        if let Some(version) = self.version {
            response
                .headers_mut()
                .insert("odata-version", HeaderValue::from_static(version.as_str()));
        }
        response
    }
}

/// The response to a refused URL: 400 for syntax, a duplicate option or a limit, 414 for a
/// path or query longer than the service reads, 404 for a name the model lacks in the resource
/// path (the first `path_len` bytes) and 400 in a query option, 501 outside the subset. The
/// message names the kind and never a byte of the request.
///
/// ```
/// use axum::http::StatusCode;
/// use odata_axum::url_error;
/// use odata_url::{Feature, UrlError, UrlErrorKind};
/// let refused = UrlError { kind: UrlErrorKind::NotImplemented(Feature::Index), offset: 10 };
/// assert_eq!(url_error(&refused, 9).status, StatusCode::NOT_IMPLEMENTED);
/// assert!(url_error(&refused, 9).error.message.starts_with("$index: no NGSI-LD equivalent"));
/// let unknown = UrlError { kind: UrlErrorKind::Unresolved, offset: 0 };
/// assert_eq!(url_error(&unknown, 6).status, StatusCode::NOT_FOUND);
/// ```
#[must_use]
pub fn url_error(error: &UrlError, path_len: usize) -> ODataErrorResponse {
    let (status, code, message) = match error.kind {
        UrlErrorKind::Syntax { .. } => (
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "the URL is not valid OData",
        ),
        UrlErrorKind::LimitExceeded {
            limit: LimitKind::PathLength | LimitKind::QueryLength,
            ..
        } => (
            StatusCode::URI_TOO_LONG,
            "URITooLong",
            "the URL is longer than this service reads",
        ),
        UrlErrorKind::LimitExceeded { .. } => (
            StatusCode::BAD_REQUEST,
            "LimitExceeded",
            "the request exceeds a limit of this service",
        ),
        UrlErrorKind::DuplicateOption(_) => (
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "a system query option is given twice",
        ),
        UrlErrorKind::NotImplemented(feature) => return not_implemented(feature.as_str()),
        UrlErrorKind::Unresolved if error.offset < path_len => (
            StatusCode::NOT_FOUND,
            "NotFound",
            "the resource does not exist",
        ),
        UrlErrorKind::Unresolved => (
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "a query option names something this service does not have",
        ),
    };
    ODataErrorResponse::new(status, code, message)
}

/// `501 NotImplemented` naming the `construct` outside the subset ADR-0024 decision 5 maps, and
/// why. `construct` is a fixed name, never a byte of the request.
///
/// ```
/// let refused = odata_axum::not_implemented("$index");
/// assert_eq!(refused.status, axum::http::StatusCode::NOT_IMPLEMENTED);
/// assert_eq!(refused.error.message, "$index: no NGSI-LD equivalent (ADR-0024 decision 5)");
/// ```
#[must_use]
pub fn not_implemented(construct: &str) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::NOT_IMPLEMENTED,
        "NotImplemented",
        &format!("{construct}: no NGSI-LD equivalent (ADR-0024 decision 5)"),
    )
}

/// The router of the two documents every OData service answers: `GET /` (the service document,
/// JSON) and `GET /$metadata` (CSDL XML; CSDL JSON when the response is 4.01 and `$format` or
/// `Accept` asks for JSON). Mount it at the service root with `nest_service`, which hands the
/// service root URL itself (`/odata/`, ending in `/`) to `GET /`; `nest` would not.
///
/// ```
/// # use std::sync::Arc;
/// # use odata_csdl::{CsdlVersion, Model};
/// # let state = odata_axum::ODataState {
/// #     model: Arc::new(Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![] }),
/// #     url_model: None,
/// #     service_root: "https://example.org/odata/".into(),
/// #     url_limits: odata_url::Limits::strict(),
/// #     csdl_limits: odata_csdl::Limits::strict(),
/// #     max_header_len: 1024,
/// #     metadata_etag: None,
/// #     custom_options: vec![],
/// # };
/// let app: axum::Router = axum::Router::new().nest_service("/odata", odata_axum::metadata_router(Arc::new(state)));
/// ```
pub fn metadata_router(state: Arc<ODataState>) -> Router {
    Router::new()
        .route("/", get(service_document).fallback(written))
        .route("/$metadata", get(metadata).fallback(written))
        .with_state(state)
}

/// What a response body is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    Xml,
    Json(MetadataLevel),
}

async fn service_document(
    State(state): State<Arc<ODataState>>,
    request: ODataRequest,
    headers: HeaderMap,
) -> Result<Response, ODataErrorResponse> {
    let version = request.version;
    // The service document has a JSON representation only (JSON Format 4.01 section 5), and the
    // ABNF gives the service root no query, so `Accept` alone chooses its metadata level.
    let Body::Json(level) =
        body(None, &headers, state.max_header_len, false).map_err(|e| e.with_version(version))?
    else {
        return Err(not_acceptable().with_version(version));
    };
    let ctx = Ctx {
        service_root: &state.service_root,
        level,
        version,
        metadata_etag: state.metadata_etag.as_deref(),
    };
    let mut out = Vec::new();
    // A Vec takes every byte: the write cannot fail.
    let _ = odata_json::write_service_document(&ctx, &state.model, &mut out);
    Ok(respond(version, json_type(version, level), out))
}

async fn metadata(
    State(state): State<Arc<ODataState>>,
    request: ODataRequest,
    headers: HeaderMap,
) -> Result<Response, ODataErrorResponse> {
    let version = request.version;
    let format = options(&request.uri).and_then(|o| o.format.as_ref());
    let chosen =
        body(format, &headers, state.max_header_len, true).map_err(|e| e.with_version(version))?;
    let mut out = Vec::new();
    let content_type = match chosen {
        Body::Xml => {
            odata_csdl::write_xml(&state.model, &state.csdl_limits, &mut out)
                .map_err(|_| unwritable().with_version(version))?;
            "application/xml"
        }
        // CSDL JSON is an OData 4.01 format (CSDL JSON 4.01 section 1): a 4.0 client asking
        // for it gets 406, not a document it cannot read.
        Body::Json(_) if version == ODataVersion::V4_0 => {
            return Err(ODataErrorResponse::new(
                StatusCode::NOT_ACCEPTABLE,
                "NotAcceptable",
                "the JSON metadata document is served for OData 4.01",
            )
            .with_version(version));
        }
        Body::Json(_) => {
            odata_csdl::write_json(&state.model, &state.csdl_limits, &mut out)
                .map_err(|_| unwritable().with_version(version))?;
            "application/json"
        }
    };
    Ok(revalidated(&headers, version, content_type, out))
}

/// The metadata document with its strong `ETag` and `Cache-Control:
/// private, no-cache` (T-3441 7.4: the bytes may differ per subject, so no
/// shared cache keeps them), or `304` when `If-None-Match` names it.
fn revalidated(
    headers: &HeaderMap,
    version: ODataVersion,
    content_type: &'static str,
    body: Vec<u8>,
) -> Response {
    let etag = etag_of(&body);
    // If-None-Match compares weakly (RFC 9110 13.1.2).
    let matched = headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|tag| tag == "*" || tag.trim_start_matches("W/") == etag);
    let mut response = if matched {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response
            .headers_mut()
            .insert("odata-version", HeaderValue::from_static(version.as_str()));
        response
    } else {
        respond(version, content_type, body)
    };
    let headers = response.headers_mut();
    // Hex digits between quotes: always a header value.
    headers.insert(
        ETAG,
        HeaderValue::from_str(&etag).unwrap_or(HeaderValue::from_static("\"\"")),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    response
}

/// The strong `ETag` of `$metadata`'s default (XML) representation, the
/// one `@odata.metadataEtag` names (JSON Format 4.01 section 4.5.2); `None`
/// when the model is past the writer's limits, as `$metadata` then is.
#[must_use]
pub fn metadata_etag(model: &odata_csdl::Model, limits: &odata_csdl::Limits) -> Option<String> {
    let mut out = Vec::new();
    odata_csdl::write_xml(model, limits, &mut out).ok()?;
    Some(etag_of(&out))
}

/// A strong tag of `body`.
/// ponytail: the std `SipHash` of the bytes, the same on every pod of one
/// build; SHA-256 if tags must agree across builds.
fn etag_of(body: &[u8]) -> String {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

/// The JSON metadata level a resource is written at (JSON Format 4.01 section 3.1): `$format`
/// first, then `Accept` by quality. A resource has no XML form, so XML or any other type is
/// 406, as an `Accept` header this service cannot read is 400.
///
/// # Errors
///
/// The refusal, in the request's version.
pub fn json_level(
    request: &ODataRequest,
    headers: &HeaderMap,
    max_header_len: usize,
) -> Result<MetadataLevel, ODataErrorResponse> {
    let format = options(&request.uri).and_then(|o| o.format.as_ref());
    match body(format, headers, max_header_len, false)
        .map_err(|e| e.with_version(request.version))?
    {
        Body::Json(level) => Ok(level),
        Body::Xml => Err(not_acceptable().with_version(request.version)),
    }
}

/// A `200` JSON answer: `body` as written, its content type carrying the metadata level in the
/// spelling of `version`, and `OData-Version`.
#[must_use]
pub fn json_response(version: ODataVersion, level: MetadataLevel, body: Vec<u8>) -> Response {
    respond(version, json_type(version, level), body)
}

fn respond(version: ODataVersion, content_type: &'static str, body: Vec<u8>) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert("odata-version", HeaderValue::from_static(version.as_str()));
    response
}

/// `application/json` with the metadata parameter in the spelling of `version`.
fn json_type(version: ODataVersion, level: MetadataLevel) -> &'static str {
    match (version, level) {
        (ODataVersion::V4_0, MetadataLevel::None) => "application/json;odata.metadata=none",
        (ODataVersion::V4_0, MetadataLevel::Minimal) => "application/json;odata.metadata=minimal",
        (ODataVersion::V4_0, MetadataLevel::Full) => "application/json;odata.metadata=full",
        (ODataVersion::V4_01, MetadataLevel::None) => "application/json;metadata=none",
        (ODataVersion::V4_01, MetadataLevel::Minimal) => "application/json;metadata=minimal",
        (ODataVersion::V4_01, MetadataLevel::Full) => "application/json;metadata=full",
    }
}

async fn written(request: ODataRequest) -> Response {
    read_only(request.version)
}

/// `405 MethodNotAllowed` with `Allow: GET`: a write method on a resource
/// that is only read (the service and metadata documents, Protocol 11.1;
/// `/$count`).
///
/// ```
/// let answer = odata_axum::read_only(odata_json::ODataVersion::V4_01);
/// assert_eq!(answer.status(), 405);
/// assert_eq!(answer.headers()["allow"], "GET");
/// ```
#[must_use]
pub fn read_only(version: ODataVersion) -> Response {
    let mut answer = ODataErrorResponse::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "MethodNotAllowed",
        "this resource is read only",
    )
    .with_version(version)
    .into_response();
    answer
        .headers_mut()
        .insert(axum::http::header::ALLOW, HeaderValue::from_static("GET"));
    answer
}

fn not_acceptable() -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::NOT_ACCEPTABLE,
        "NotAcceptable",
        "this resource is not available in the requested format",
    )
}

fn unwritable() -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "InternalError",
        "the service model could not be written",
    )
}

fn bad_header() -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "BadRequest",
        "a request header is not valid",
    )
}

/// One header as text, within `max` bytes.
fn header<'h>(
    headers: &'h HeaderMap,
    name: &str,
    max: usize,
) -> Result<Option<&'h str>, ODataErrorResponse> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    if value.len() > max {
        return Err(bad_header());
    }
    value.to_str().map(Some).map_err(|_| bad_header())
}

fn version(headers: &HeaderMap, max: usize) -> Result<ODataVersion, ODataErrorResponse> {
    let asked = header(headers, "odata-version", max)?;
    let at_most = header(headers, "odata-maxversion", max)?;
    negotiate_version(asked, at_most).map_err(|_| {
        ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "the requested OData version is not supported",
        )
    })
}

/// The body format: `$format` first, which overrides `Accept` (Protocol 4.01 8.2.1), then
/// `Accept` by quality, then the resource's default (XML where `xml_default`, else JSON).
fn body(
    format: Option<&Format>,
    headers: &HeaderMap,
    max: usize,
    xml_default: bool,
) -> Result<Body, ODataErrorResponse> {
    if let Some(format) = format {
        return match format {
            Format::Json => Ok(Body::Json(MetadataLevel::Minimal)),
            Format::Xml => Ok(Body::Xml),
            Format::Atom => Err(not_acceptable()),
            // Written percent-encoded by a form encoder, meant decoded (T-3742); the URL
            // parser refused an escape that does not decode.
            Format::MediaType(media) => {
                media_type(&odata_url::percent_decoded(media).unwrap_or_default())?
                    .ok_or_else(not_acceptable)
            }
        };
    }
    let Some(accept) = header(headers, ACCEPT.as_str(), max)? else {
        return Ok(if xml_default {
            Body::Xml
        } else {
            Body::Json(MetadataLevel::Minimal)
        });
    };
    let mut ranges: Vec<(u16, &str)> = Vec::new();
    for range in accept.split(',') {
        let media = range.trim();
        if media.is_empty() {
            continue;
        }
        let quality = quality(media).ok_or_else(bad_header)?;
        if quality > 0 {
            ranges.push((quality, media));
        }
    }
    // Stable: equal qualities keep the client's order.
    ranges.sort_by_key(|range| std::cmp::Reverse(range.0));
    for (_, media) in ranges {
        let essence = media.split(';').next().unwrap_or(media).trim();
        if essence == "*/*" || essence.eq_ignore_ascii_case("application/*") {
            return Ok(if xml_default {
                Body::Xml
            } else {
                Body::Json(MetadataLevel::Minimal)
            });
        }
        if let Some(chosen) = media_type(media)? {
            return Ok(chosen);
        }
    }
    Err(not_acceptable())
}

/// `application/xml` or `application/json` with its metadata level; `None` for any other type.
fn media_type(media: &str) -> Result<Option<Body>, ODataErrorResponse> {
    let mut parts = media.split(';');
    let essence = parts.next().unwrap_or_default().trim();
    if essence.eq_ignore_ascii_case("application/xml") {
        return Ok(Some(Body::Xml));
    }
    if !essence.eq_ignore_ascii_case("application/json") {
        return Ok(None);
    }
    let mut level = MetadataLevel::Minimal;
    for parameter in parts {
        let Some((name, value)) = parameter.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.eq_ignore_ascii_case("metadata") || name.eq_ignore_ascii_case("odata.metadata") {
            level = MetadataLevel::parse(value.trim()).ok_or_else(not_acceptable)?;
        }
    }
    Ok(Some(Body::Json(level)))
}

/// The `q` of one media range in thousandths (RFC 9110 12.4.2), 1000 without one; `None` for a
/// malformed value.
fn quality(media: &str) -> Option<u16> {
    let Some(q) = media.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("q")
            .then_some(value.trim())
    }) else {
        return Some(1000);
    };
    let (whole, fraction) = q.split_once('.').unwrap_or((q, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Up to three ASCII digits, padded to three: always a number below 1000.
    let thousandths: u16 = format!("{fraction:0<3}").parse().unwrap_or_default();
    match whole {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(1000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::quality;

    #[test]
    fn quality_reads_rfc_9110_values_and_refuses_the_rest() {
        assert_eq!(quality("application/json"), Some(1000));
        assert_eq!(quality("application/json;q=0.5"), Some(500));
        assert_eq!(quality("application/json; Q=0.25"), Some(250));
        assert_eq!(quality("application/json;q=1"), Some(1000));
        assert_eq!(quality("application/json;q=1.000"), Some(1000));
        assert_eq!(quality("application/json;q=0"), Some(0));
        for bad in ["q=2", "q=1.5", "q=0.1234", "q=x", "q=-1", "q=0.a"] {
            assert_eq!(quality(&format!("application/json;{bad}")), None, "{bad}");
        }
    }
}
