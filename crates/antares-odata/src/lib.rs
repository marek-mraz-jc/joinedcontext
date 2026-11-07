// SPDX-License-Identifier: EUPL-1.2
//! The OData surface of a view (`docs/analysis/odata-linkml-mapping.md`,
//! T-3508): `/x/views/{view}/odata/v4` serves the view's Entities as OData
//! 4.01 EntitySets, its model the Tenant's `LinkML` schema intersected with
//! what the view shows ([`edm`]). Every request runs as one operation
//! through [`antares_ops`], so the operation's own gate fires and the view
//! narrows the answer as it does on the NGSI-LD view root.
//!
//! A view whose Tenant has no schema, or one this surface cannot read, has
//! no OData endpoint: every path answers `404` as an unknown view does
//! (surfaces-threat-model D12).
//!
//! ```
//! use antares_ops::Surface;
//! let odata = antares_odata::OData;
//! assert_eq!(odata.prefix(), "/x/views");
//! assert_eq!(antares_ops::mount(odata).name(), "odata");
//! ```

#![forbid(unsafe_code)]

mod apply;
mod batch;
mod below;
pub mod edm;
mod filter;
mod functions;
mod history;
mod names;
mod paths;
mod read;
mod route;
mod write;

use antares_ops::{Caller, OperationId, Ops, Surface, ViewOps};
use axum::body::Bytes;
use axum::extract::{FromRequestParts as _, MatchedPath, Request};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use odata_axum::{ODataErrorResponse, ODataRequest, ODataState};
use odata_csdl::CsdlVersion;
use std::sync::{Arc, LazyLock};
use tower::ServiceExt as _;

/// Where the surface is mounted; the view id and `odata/v4` follow.
pub const PREFIX: &str = "/x/views";

/// The bounds a Tenant's schema is read under (threat LK7). The bytes are
/// already bounded by `ANTARES_SCHEMA_MAX_BYTES` when they are written.
pub const SCHEMA_LIMITS: linkml_model::Limits = linkml_model::Limits {
    max_bytes: 16 << 20,
    max_depth: 64,
    max_elements: 100_000,
};

/// The longest request header the surface reads (`Accept`, `OData-*`).
pub const MAX_HEADER_LEN: usize = 4096;

/// The bytes of built models a pod keeps by default (T-3441 7.3, T-4354),
/// each weighed by its CSDL document's XML: 64 views of one ordinary model
/// fit. 16 MiB is this cache's share of the broker's default 200 MiB memory
/// budget (T-5895); [`set_model_cache_bytes`] sets another share.
pub const DEFAULT_MODEL_CACHE_BYTES: u64 = 16 << 20;

static MODEL_CACHE_BYTES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(DEFAULT_MODEL_CACHE_BYTES);

/// Sets the bytes of built models the pod keeps, and of the LinkML schema
/// source the models are read from. Both caches are sized when they are
/// first used, so the broker calls this before it serves.
pub fn set_model_cache_bytes(models: u64, linkml_models: u64) {
    MODEL_CACHE_BYTES.store(models, std::sync::atomic::Ordering::Relaxed);
    linkml_model::cache::set_max_bytes(linkml_models);
}

/// The OData surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct OData;

impl Surface for OData {
    fn name(&self) -> &'static str {
        "odata"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> serde_json::Value {
        serde_json::json!({
            "endpoint": format!("{PREFIX}/{{view}}/odata/v4"),
            "odataVersions": ["4.0", "4.01"],
        })
    }

    fn router(&self, ops: Ops) -> Router {
        // T-5912: every NGSI-LD write of a Tenant with a schema keeps to it.
        ops.check_writes(linkml_model::check::entity_text);
        let handler = any(move |req: Request| {
            let ops = ops.clone();
            async move { serve(&ops, req).await }
        });
        Router::new()
            .route("/{view}/odata/v4", handler.clone())
            .route("/{view}/odata/v4/", handler.clone())
            .route("/{view}/odata/v4/{*rest}", handler)
    }
}

/// `HEAD` is the `GET` of the same resource, through the same gate, with
/// its status and headers and no body (RFC 9110 9.3.2, T-3723); the
/// `Content-Length` is the one the `GET` would send.
async fn serve(ops: &Ops, mut req: Request) -> Response {
    if req.method() != Method::HEAD {
        return respond(ops, req).await;
    }
    *req.method_mut() = Method::GET;
    // The GET's headers already carry its Content-Length (T-3737).
    let (parts, _) = respond(ops, req).await.into_parts();
    Response::from_parts(parts, axum::body::Body::empty())
}

/// One request to a view's endpoint. The path is read as sent, below the
/// prefix (`/{view}/odata/v4...`): a view id is `[a-z0-9-]`, so its raw
/// segment is its id, and `ca%72s` never reaches the view `cars`.
async fn respond(ops: &Ops, req: Request) -> Response {
    let id = req
        .uri()
        .path()
        .split('/')
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let Some(view) = ops.view(&id) else {
        return not_found();
    };
    let version = csdl_version(&req);
    // The documents that name the model's types and properties show only
    // what the engine lets this caller read (F-ANT4-003), as the LinkML
    // surface narrows the same schema. A query naming a hidden property
    // reads it as absent, so the model it runs over stays the view's.
    let (mut parts, body) = req.into_parts();
    let Ok(caller) = Caller::from_request_parts(&mut parts, &()).await;
    let mut req = Request::from_parts(parts, body);
    let names_the_model = req.method() == Method::GET
        && req
            .uri()
            .path()
            .strip_prefix(&format!("/{id}/odata/v4"))
            .is_some_and(|rest| matches!(rest, "" | "/" | "/$metadata"));
    let visible = if names_the_model {
        Some(model_visible(&view, &caller).await)
    } else {
        None
    };
    let visible = visible.as_ref();
    let Some((edm, csdl, metadata_etag)) = model(&id, &view, visible, version) else {
        return not_found();
    };
    // A 4.0 document serves an Untyped member as a dynamic property of an open type (CSDL XML
    // 3.4); its URLs resolve against the 4.01 model, which names each, so a name no model holds
    // stays unknown (T-5206).
    let open = csdl
        .schemas
        .iter()
        .flat_map(|s| &s.entity_types)
        .any(|t| t.open);
    let url_model = (version == CsdlVersion::V4_0 && open)
        .then(|| model(&id, &view, visible, CsdlVersion::V4_01))
        .flatten()
        .map(|(_, csdl, _)| csdl);
    let state = Arc::new(ODataState {
        model: csdl,
        url_model,
        // Absolute: a path-only context URL is read as a file path by
        // Microsoft.OData.Core on Linux (T-3688).
        service_root: format!(
            "{}{PREFIX}/{id}/odata/v4/",
            antares_ops::public_base(req.headers())
        ),
        url_limits: odata_url::Limits::strict(),
        csdl_limits: odata_csdl::Limits::strict(),
        max_header_len: MAX_HEADER_LEN,
        metadata_etag,
        custom_options: read::NGSILD_OPTIONS
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
    });
    // The inner router matches its own path; the outer match goes.
    req.extensions_mut().remove::<MatchedPath>();
    let req = match query_in_body(req, state.url_limits.max_query_len).await {
        Ok(req) => req,
        Err(refused) => return refused.into_response(),
    };
    let reads = Arc::new(read::Reads {
        view,
        edm,
        state: state.clone(),
    });
    let api = odata_axum::metadata_router(state.clone()).merge(
        Router::new()
            .fallback(
                move |method: Method,
                      caller: Caller,
                      request: ODataRequest,
                      headers: HeaderMap,
                      body: Bytes| async move {
                    route::serve(&reads, &method, &caller, &request, &headers, &body).await
                },
            )
            .with_state(state.clone()),
    );
    let inner_root = format!("/{id}/odata/v4/");
    let routed = Router::new().nest_service(&format!("/{id}/odata/v4"), api);
    if req.method() == Method::POST && req.uri().path() == format!("{inner_root}$batch") {
        let path_root = format!("{PREFIX}{inner_root}");
        let roots = (path_root.as_str(), inner_root.as_str());
        return batch::serve(&routed, ops, roots, req, state.url_limits.max_query_len).await;
    }
    let Ok(resp) = routed.oneshot(req).await;
    resp
}

/// `POST {resource}/$query` (Protocol 11.2.5.2, T-3441 Q22): the options
/// of the body, joined to the URL's, served as the `GET` they stand for,
/// so the parser, the refusals and the engine see one request (O24). The
/// body is bounded by the query length the parser takes.
async fn query_in_body(req: Request, max: usize) -> Result<Request, ODataErrorResponse> {
    let path = req.uri().path();
    let Some(resource) = path
        .strip_suffix("/$query")
        .filter(|_| req.method() == Method::POST)
    else {
        return Ok(req);
    };
    let resource = resource.to_owned();
    let kind = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase());
    let form = match kind.as_deref() {
        Some("text/plain") => false,
        Some("application/x-www-form-urlencoded") => true,
        _ => {
            return Err(ODataErrorResponse::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UnsupportedMediaType",
                "a query body is text/plain or application/x-www-form-urlencoded",
            ))
        }
    };
    let (mut parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, max).await.map_err(|_| {
        ODataErrorResponse::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "PayloadTooLarge",
            "the query body is longer than this service reads",
        )
    })?;
    let bad = || {
        ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "the query body is not a URL query",
        )
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| bad())?.trim();
    // A form encodes a space as `+`; a URL query reads `+` as itself.
    let text = if form {
        text.replace('+', "%20")
    } else {
        text.to_owned()
    };
    let query: Vec<&str> = parts
        .uri
        .query()
        .into_iter()
        .chain(Some(text.as_str()))
        .filter(|q| !q.is_empty())
        .collect();
    let uri = match query.join("&") {
        q if q.is_empty() => resource,
        q => format!("{resource}?{q}"),
    };
    parts.uri = uri.parse().map_err(|_| bad())?;
    parts.method = Method::GET;
    parts.headers.remove(CONTENT_TYPE);
    parts.headers.remove(CONTENT_LENGTH);
    Ok(Request::from_parts(parts, axum::body::Body::empty()))
}

/// What a model is a function of (T-3441 7.3): the view, the types and
/// Attributes it shows, the writes it offers, its Tenant's schema
/// revision, the OData version.
/// The key is compared whole, so two views never share a model by a
/// collision. ponytail: no engine probe bits yet; section 4's Capabilities
/// add them to the key when the model reads the engine.
/// The view, what it shows, its writes, the schema revision, CSDL 4.01, its
/// `representations.odata` as written, and whether it offers the Temporal Query (T-5908).
type ModelKey = (String, edm::Shown, edm::Writable, u64, bool, String, bool);

/// A built model, its CSDL document and that document's `ETag`.
type Built = (Arc<edm::Edm>, Arc<odata_csdl::Model>, Option<String>);

/// What the caller may read through `view`: asked as the type details the
/// LinkML surface asks for its schema documents, else as the reads the model
/// describes. A view offering none of them, or an engine refusing, leaves
/// the caller a model that names nothing (fail closed).
async fn model_visible(view: &ViewOps, caller: &Caller) -> antares_ops::Visible {
    // a view offering none is refused by `visible` and gets the empty model
    let op = [
        OperationId::RetrieveEntityTypeDetails,
        OperationId::QueryEntities,
        OperationId::RetrieveEntity,
    ]
    .into_iter()
    .find(|op| view.allows(*op))
    .unwrap_or(OperationId::RetrieveEntityTypeDetails);
    view.visible(caller, op, &[], &[]).await.unwrap_or_default()
}

/// `shown` without the types and Attributes `visible` hides; all of it
/// without one.
fn narrowed(shown: edm::Shown, visible: Option<&antares_ops::Visible>) -> edm::Shown {
    let Some(visible) = visible else {
        return shown;
    };
    shown
        .into_iter()
        .filter(|(ty, _)| visible.shows_type(ty))
        .map(|(ty, attrs)| {
            let attrs = attrs
                .into_iter()
                .filter(|a| visible.shows_attribute(&ty, a))
                .collect();
            (ty, attrs)
        })
        .collect()
}

/// The model of `view` at `version` as `visible` narrows it, built once per
/// key on this pod (O21), `None` when its Tenant has no schema this surface
/// can read.
fn model(
    id: &str,
    view: &ViewOps,
    visible: Option<&antares_ops::Visible>,
    version: CsdlVersion,
) -> Option<Built> {
    static CACHE: LazyLock<moka::sync::Cache<ModelKey, (Built, u32)>> = LazyLock::new(|| {
        moka::sync::Cache::builder()
            .max_capacity(MODEL_CACHE_BYTES.load(std::sync::atomic::Ordering::Relaxed))
            .weigher(|_key: &ModelKey, value: &(Built, u32)| value.1)
            .build()
    });
    let (revision, bytes) = view.schema()?;
    let schema = parsed(revision, &bytes)?;
    // A view of `*` names no type; it shows the whole schema (ADR-0023 rule 12).
    let every = view.shows_every_name();
    let shown = narrowed(
        if every {
            schema.slot_iris_by_class()
        } else {
            view.shown_by_type()
        },
        visible,
    );
    let writable = edm::Writable {
        insert: view.allows(OperationId::CreateEntity),
        update: view.allows(OperationId::MergeEntity) || view.allows(OperationId::ReplaceEntity),
        delete: view.allows(OperationId::DeleteEntity),
    };
    let key = (
        id.to_owned(),
        shown,
        writable,
        revision,
        version == CsdlVersion::V4_01,
        view.representation("odata")
            .map(ToString::to_string)
            .unwrap_or_default(),
        view.allows(OperationId::TemporalQuery),
    );
    if let Some((hit, _)) = CACHE.get(&key) {
        return Some(hit);
    }
    // A view's names are the Entities' IRIs; the schema's may be another vocabulary's.
    let viewed;
    let schema = if every {
        &*schema
    } else {
        viewed = schema.as_shown(&key.1, antares_ops::core_iri);
        &viewed
    };
    let edm = edm::build(id, schema, &key.1, key.2, version).ok()?;
    let listed: Vec<_> = antares_ops::outputs::listed(view.representation("odata"), "odata")
        .into_iter()
        .filter_map(|(_, o)| o.ok())
        .collect();
    let edm = edm::outputs(edm, &listed);
    let edm = if key.6 { edm::histories(edm) } else { edm };
    tracing::debug!(
        view = id,
        schema_revision = revision,
        "the view's OData model was built"
    );
    let etag = odata_axum::metadata_etag(&edm.csdl, &odata_csdl::Limits::strict());
    let mut xml = Vec::new();
    // ponytail: the document is written a second time for its weight, once per built key.
    let weight = odata_csdl::write_xml(&edm.csdl, &odata_csdl::Limits::strict(), &mut xml)
        .map_or(u32::MAX, |()| u32::try_from(xml.len()).unwrap_or(u32::MAX))
        .max(4096);
    let built = (Arc::new(edm.clone()), Arc::new(edm.csdl), etag);
    CACHE.insert(key, (built.clone(), weight));
    Some(built)
}

/// The schema of `bytes`, parsed once per revision on this pod and shared
/// with the LinkML surface (T-4354): read under the LinkML surface's bounds
/// first, and under this surface's own only when those refuse it, so OData
/// takes what it took before and a schema inside both is parsed once.
fn parsed(revision: u64, bytes: &Arc<[u8]>) -> Option<Arc<linkml_model::Model>> {
    use linkml_model::cache;
    let (read, fresh) = match cache::read(revision, bytes, &linkml_model::SURFACE_LIMITS) {
        (Ok(model), fresh) => (Ok(model), fresh),
        (Err(_), _) => cache::read(revision, bytes, &SCHEMA_LIMITS),
    };
    read.inspect_err(|e| {
        if fresh {
            unreadable(revision, e);
        }
    })
    .ok()
}

/// Logged once per revision on this pod: the result is cached.
fn unreadable(revision: u64, e: &linkml_model::Error) {
    tracing::warn!(
        revision,
        error = %e,
        "the Tenant's schema cannot be read; OData is closed for its views"
    );
}

/// The version the response is written in (Protocol 8.2.6, 8.2.7), as
/// `odata-axum` negotiates it, so the model and the document agree.
fn csdl_version(req: &Request) -> CsdlVersion {
    let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok());
    match odata_json::negotiate_version(header("OData-Version"), header("OData-MaxVersion")) {
        Ok(odata_json::ODataVersion::V4_0) => CsdlVersion::V4_0,
        // A refused header is answered 400 by the router; the model is not read.
        _ => CsdlVersion::V4_01,
    }
}

fn not_found() -> Response {
    ODataErrorResponse::new(StatusCode::NOT_FOUND, "NotFound", "no such resource").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-5895: the shares the broker sets are what the model caches take.
    #[test]
    fn the_set_shares_are_kept() {
        set_model_cache_bytes(DEFAULT_MODEL_CACHE_BYTES, linkml_model::cache::MAX_BYTES);
        assert_eq!(
            MODEL_CACHE_BYTES.load(std::sync::atomic::Ordering::Relaxed),
            DEFAULT_MODEL_CACHE_BYTES
        );
    }

    #[test]
    fn the_surface_names_its_endpoint() {
        assert_eq!((OData.name(), OData.prefix()), ("odata", "/x/views"));
        assert_eq!(OData.version_info()["endpoint"], "/x/views/{view}/odata/v4");
    }

    #[test]
    fn a_revision_is_parsed_once_and_shared_with_the_linkml_bounds() {
        let schema: &Arc<[u8]> = &br#"{"id": "https://e.org/odata-once", "name": "s"}"#[..].into();
        let first = parsed(9_001, schema).expect("reads");
        let again = parsed(9_001, schema).expect("reads");
        assert!(Arc::ptr_eq(&first, &again));
        // The LinkML surface asks the same revision under its bounds: no new parse.
        let (linkml, fresh) =
            linkml_model::cache::read(9_001, schema, &linkml_model::SURFACE_LIMITS);
        assert!(!fresh);
        assert!(Arc::ptr_eq(&first, &linkml.expect("reads")));
    }

    #[test]
    fn a_schema_past_the_linkml_bounds_still_reads_under_odatas_own() {
        // 40 levels deep: past LinkML's 32, inside OData's 64, as before T-4354.
        let deep = format!(
            r#"{{"id": "https://e.org/deep", "name": "s", "classes": {{"Room": {{"annotations": {{"a": {{"tag": "a", "value": {}1{}}}}}}}}}}}"#,
            "[".repeat(36),
            "]".repeat(36)
        );
        let deep: Arc<[u8]> = deep.into_bytes().into();
        let (strict, _) = linkml_model::cache::read(9_002, &deep, &linkml_model::SURFACE_LIMITS);
        assert!(strict.is_err());
        let loose = linkml_model::Model::read(&deep, &SCHEMA_LIMITS).is_ok();
        assert_eq!(parsed(9_002, &deep).is_some(), loose);
    }

    #[test]
    fn an_unreadable_schema_stays_closed_and_is_read_once() {
        let bad: Arc<[u8]> = b"{not json"[..].into();
        assert!(parsed(9_003, &bad).is_none());
        // Answered from the cache, the reason logged the first time only.
        assert!(parsed(9_003, &bad).is_none());
        let (read, fresh) = linkml_model::cache::read(9_003, &bad, &SCHEMA_LIMITS);
        assert_eq!((read.is_err(), fresh), (true, false));
    }
}
