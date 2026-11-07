// SPDX-License-Identifier: EUPL-1.2
//! The LinkML surface of a view (`odata-linkml-mapping.md` section 8,
//! T-3511): `GET /x/views/{view}/linkml/…` serves the view's schema, the
//! LinkML stored for the view's Tenant (`/q/schemas/{tenant}`, T-3628),
//! narrowed to what the caller may see
//! through the view and the policy engine (`effective = view ∧ engine`,
//! ADR-0023 rule 5) and rendered from that narrowed model alone (LK1).
//!
//! Each path is one discovery operation the view must allow, asked through
//! [`antares_ops::ViewOps::visible`] with its own clause: the documents and
//! the index are Retrieve Entity Type Details (5.7.6), `classes/{type}` is
//! Retrieve Entity Type Information (5.7.7), `slots/{attr}` is Retrieve
//! Attribute Information (5.7.10). The surface is read-only: any other
//! method is 405 with `Allow: GET, HEAD` (LK5). Every answer carries a
//! strong `ETag` of its bytes and `Cache-Control: private, no-cache`, and a
//! matching `If-None-Match` is 304 (LK2, D15). Rendered documents are kept
//! per view, schema and visible set, at most [`CACHE_ENTRIES`] of them.
//!
//! ```
//! use antares_ops::Surface;
//! assert_eq!(antares_linkml::Linkml.prefix(), "/x/views");
//! assert_eq!(antares_ops::mount(antares_linkml::Linkml).name(), "linkml");
//! ```
#![forbid(unsafe_code)]

mod narrow;
mod render;

use antares_ops::{Caller, OperationId, Ops, Refusal, Surface, ViewOps};
use axum::extract::{Path, Request};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use std::sync::Arc;

pub use render::Artifact;

/// Where the surface mounts: the view root it shares (ADR-0027 section 1).
pub const PREFIX: &str = "/x/views";

/// How many rendered schemas one pod keeps (`odata-linkml-mapping.md` 7.3).
pub const CACHE_ENTRIES: usize = 256;

/// Sets the bytes of rendered schemas one pod keeps (T-5895), which the
/// next rendering evicts down to, and of the LinkML schema source behind
/// them, which is sized at its first read.
pub fn set_cache_bytes(renders: usize, linkml_models: u64) {
    render::CACHE_BYTES.store(renders, std::sync::atomic::Ordering::Relaxed);
    linkml_model::cache::set_max_bytes(linkml_models);
}

/// The LinkML surface: `/x/views/{view}/linkml`.
#[derive(Debug, Clone, Copy)]
pub struct Linkml;

impl Surface for Linkml {
    fn name(&self) -> &'static str {
        "linkml"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> serde_json::Value {
        serde_json::json!({
            "endpoint": format!("{PREFIX}/{{view}}/linkml"),
            "artifacts": Artifact::ALL.iter().map(|a| a.file_name()).collect::<Vec<_>>(),
        })
    }

    fn router(&self, ops: Ops) -> Router {
        // T-5912: every NGSI-LD write of a Tenant with a schema keeps to it.
        ops.check_writes(linkml_model::check::entity_text);
        let cache = Arc::new(render::Cache::default());
        let serve =
            move |Path((id, rest)): Path<(String, String)>, caller: Caller, req: Request| {
                let (ops, cache) = (ops.clone(), cache.clone());
                async move {
                    serve(
                        &ops,
                        &cache,
                        &id,
                        &rest,
                        &caller,
                        req.method(),
                        req.headers(),
                    )
                    .await
                }
            };
        Router::new()
            .route("/{view}/linkml/{*rest}", any(serve))
            .route(
                "/{view}/linkml",
                any(|| async { Refusal::NotFound.ngsi_ld() }),
            )
    }
}

/// What a path under `linkml/` names (rows L1 to L9, L13).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wanted {
    Index,
    Document { major: String, artifact: Artifact },
    Class(String),
    Slot(String),
}

impl Wanted {
    /// The path's document, `None` for any other path (L13).
    fn of(rest: &str, accept: &str) -> Option<Self> {
        let parts: Vec<&str> = rest.split('/').collect();
        match parts.as_slice() {
            ["schema", "index.json"] => Some(Self::Index),
            ["schema", version, name] => Some(Self::Document {
                major: version.strip_prefix('v')?.to_owned(),
                artifact: Artifact::of(name, accept)?,
            }),
            // An empty name is a class or slot no schema holds: 404 below.
            ["classes", name] => Some(Self::Class((*name).to_owned())),
            ["slots", name] => Some(Self::Slot((*name).to_owned())),
            _ => None,
        }
    }

    /// The operation the path is: its clause asks the engine, and the view
    /// must allow it.
    fn operation(&self) -> OperationId {
        match self {
            Self::Index | Self::Document { .. } => OperationId::RetrieveEntityTypeDetails,
            Self::Class(_) => OperationId::RetrieveEntityTypeInfo,
            Self::Slot(_) => OperationId::RetrieveAttributeInfo,
        }
    }
}

async fn serve(
    ops: &Ops,
    cache: &render::Cache,
    id: &str,
    rest: &str,
    caller: &Caller,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    let Some(view) = ops.view(id) else {
        return Refusal::NotFound.ngsi_ld();
    };
    if method != Method::GET && method != Method::HEAD {
        return Refusal::NotAllowed {
            allow: vec![Method::GET, Method::HEAD],
        }
        .ngsi_ld();
    }
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some(wanted) = Wanted::of(rest, accept) else {
        return Refusal::NotFound.ngsi_ld();
    };
    match answer(&view, cache, id, &wanted, caller).await {
        Ok((media_type, body)) => respond(method, headers, media_type, &body),
        Err(refusal) => refusal.ngsi_ld(),
    }
}

/// The bytes `wanted` names and their media type.
async fn answer(
    view: &ViewOps,
    cache: &render::Cache,
    id: &str,
    wanted: &Wanted,
    caller: &Caller,
) -> Result<(&'static str, Arc<[u8]>), Refusal> {
    // A Tenant with no schema has its views' LinkML surface closed: the
    // same answer as a path that is not there. An oversized one never got
    // in (`ANTARES_SCHEMA_MAX_BYTES` refuses the write, LK7).
    let (revision, schema) = view.schema().ok_or(Refusal::NotFound)?;
    let schema = &schema;
    let source = render::Cache::source(id, revision, schema).ok_or(Refusal::NotFound)?;
    // The operation names what it asks about (operation-matrix.yaml
    // `operation_fields`): a class or slot the stored schema lacks is the
    // same 404 as a hidden one.
    let (types, attrs) = match wanted {
        Wanted::Class(name) if source.schema().classes.contains_key(name) => {
            let shown = view.shown_by_type();
            (
                vec![as_viewed(source.class_iri(name), name, shown.keys())],
                Vec::new(),
            )
        }
        Wanted::Slot(name) if source.schema().slots.contains_key(name) => {
            let shown = view.shown_by_type();
            (
                Vec::new(),
                vec![as_viewed(
                    source.slot_iri(name),
                    name,
                    shown.values().flatten(),
                )],
            )
        }
        Wanted::Class(_) | Wanted::Slot(_) => return Err(Refusal::NotFound),
        Wanted::Index | Wanted::Document { .. } => (Vec::new(), Vec::new()),
    };
    // An engine refusing a named class or slot answers as an absent one
    // (LK1): a 403 would tell the caller the name is there.
    let visible = match view
        .visible(caller, wanted.operation(), &types, &attrs)
        .await
    {
        Err(Refusal::Denied(_)) if matches!(wanted, Wanted::Class(_) | Wanted::Slot(_)) => {
            return Err(Refusal::NotFound)
        }
        other => other?,
    };
    let rendered = cache
        .get(id, revision, schema, &visible)
        .ok_or(Refusal::NotFound)?;
    let found = match wanted {
        Wanted::Index => Some((render::JSON, rendered.index.clone())),
        Wanted::Document { major, artifact } => (*major == rendered.major)
            .then(|| rendered.document(*artifact))
            .flatten()
            .map(|bytes| (artifact.media_type(), bytes)),
        Wanted::Class(name) => rendered.class(name).map(|b| (render::JSON, b)),
        Wanted::Slot(name) => rendered.slot(name).map(|b| (render::JSON, b)),
    };
    found.ok_or(Refusal::NotFound)
}

/// The IRI the view knows schema element `name` by, the one the engine is asked about: its own
/// when the view shows it (or names nothing, a `*` view), else the shown IRI the core @context
/// reads `name` as, as `narrow` matches it (T-3645).
fn as_viewed<'a>(own: String, name: &str, shown: impl Iterator<Item = &'a String>) -> String {
    let shown: Vec<&String> = shown.collect();
    if shown.contains(&&own) {
        return own;
    }
    shown
        .into_iter()
        .find(|iri| antares_ops::core_name(iri) == name)
        .cloned()
        .unwrap_or(own)
}

/// `body` as `GET` or `HEAD` answers it: a strong `ETag` of the bytes,
/// `Cache-Control: private, no-cache`, 304 when `If-None-Match` names it.
fn respond(
    method: &Method,
    headers: &HeaderMap,
    media_type: &'static str,
    body: &[u8],
) -> Response {
    let etag = format!("\"{}\"", render::sha256(body));
    let matched = headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == etag);
    let status = if matched {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let mut response = if matched || method == Method::HEAD {
        status.into_response()
    } else {
        (status, axum::body::Body::from(body.to_vec())).into_response()
    };
    let h = response.headers_mut();
    // A quoted hex digest is always a header value; the fallback is never
    // sent.
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or(HeaderValue::from_static("\"\"")),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    if !matched {
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_names_one_document_or_none() {
        assert_eq!(Wanted::of("schema/index.json", ""), Some(Wanted::Index));
        assert_eq!(
            Wanted::of("schema/v2/model.linkml.yaml", ""),
            Some(Wanted::Document {
                major: "2".into(),
                artifact: Artifact::LinkMl
            })
        );
        assert_eq!(
            Wanted::of("schema/v2/model", "text/turtle; profile=\"owl\""),
            Some(Wanted::Document {
                major: "2".into(),
                artifact: Artifact::Owl
            })
        );
        assert_eq!(
            Wanted::of("classes/Car", ""),
            Some(Wanted::Class("Car".into()))
        );
        assert_eq!(Wanted::of("slots/", ""), Some(Wanted::Slot(String::new())));
        for other in [
            "schema",
            "schema/2/model.md",
            "schema/v2/model.xml",
            "classes/Car/x",
            "slots",
            "",
        ] {
            assert_eq!(Wanted::of(other, ""), None, "{other}");
        }
    }

    #[test]
    fn an_etag_named_in_if_none_match_is_304_without_a_body() {
        let body: Arc<[u8]> = Arc::from(&b"abc"[..]);
        let etag = format!("\"{}\"", render::sha256(&body));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&format!("\"x\", W/{etag}")).expect("a value"),
        );
        let r = respond(&Method::GET, &headers, "text/yaml", &body);
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        let mut any = HeaderMap::new();
        any.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        let r = respond(&Method::GET, &any, "text/yaml", &body);
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(r.headers()[header::ETAG], etag.as_str());
        let r = respond(&Method::HEAD, &HeaderMap::new(), "text/yaml", &body);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()[header::CONTENT_LENGTH], "3");
        assert_eq!(r.headers()[header::CACHE_CONTROL], "private, no-cache");
    }
}
