// SPDX-License-Identifier: EUPL-1.2
//! The OGC API - Features surface of a view (`docs/analysis/ogc-features-mapping.md`,
//! ADR-0029, T-3744): `/x/views/{view}/ogc/features` serves the view's Entities as Features
//! of OGC API - Features Part 1 Core with `GeoJSON` and `OpenAPI` 3.0, Part 2 `CRS84` (`crs.rs`),
//! the Part 4 draft's writes (`write.rs`, T-3746) and the Part 5 draft's schemas, sortables
//! and feature references (`schema.rs`, T-3754). A collection is a class
//! of the Tenant's `LinkML` schema whose type the view shows (`model.rs`); its Features are the
//! Entities a Query Entities through the view returns for the caller, and a write one Create,
//! Replace, Merge or Delete Entity through it, so the operation's own gate fires and the view narrows the answer as on the NGSI-LD view root.
//!
//! A view whose Tenant has no schema, or one this surface cannot read, has no OGC endpoint:
//! every path answers 404 as an unknown view does, as the `OData` surface does.
//!
//! ```
//! use antares_ops::Surface;
//! let ogc = antares_ogc::Ogc;
//! assert_eq!(ogc.prefix(), "/x/views");
//! assert_eq!(antares_ops::mount(ogc).name(), "ogc");
//! ```

#![forbid(unsafe_code)]

mod api;
mod crs;
mod export;
mod extent;
mod features;
mod filter;
mod history;
mod items;
mod model;
mod schema;
mod terms;
mod tiles;
mod write;

use antares_ops::{Caller, OperationId, Ops, Surface, ViewOps};
use axum::body::Body;
use axum::extract::{Query, Request};
use axum::http::header::{ACCEPT, ACCEPT_LANGUAGE, ALLOW, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use model::Collection;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Where the surface is mounted; the view id and `ogc/features` follow.
pub const PREFIX: &str = "/x/views";

/// How many files (`f=parquet`, `csv`, `zip`) the surface writes at once, 1 to 64,
/// default 2; past it a file is `503` with `Retry-After` (T-4465 review).
pub const EXPORT_FILES_ENV: &str = "ANTARES_EXPORT_FILES";

/// The variables this surface reads.
pub const ENV_KEYS: &[&str] = &[EXPORT_FILES_ENV];

/// The bounds a Tenant's schema is read under (threat LK7), as the `OData` surface reads it.
pub const SCHEMA_LIMITS: linkml_model::Limits = linkml_model::Limits {
    max_bytes: 16 << 20,
    max_depth: 64,
    max_elements: 100_000,
};

/// The conformance classes the surface serves (mapping rows C3, R1 to R5, W1 to W5, S1 to S3,
/// S5, S7, F1 to F16, T1 to T6; Parts 4 and 5 are the drafts OGC 20-002r1 and 23-058r1, Part 3
/// is OGC 19-079r2 with CQL2 21-065r2, Tiles is OGC 20-057 with the matrix sets of 17-083r4). A view's `/conformance` declares the Part 4 ones only as
/// far as it offers their writes (the `conformance` handler).
pub const CONFORMANCE: &[&str] = &[
    "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
    "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/geojson",
    "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/oas30",
    "http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/schemas",
    "http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/core-roles-features",
    "http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/returnables-and-receivables",
    "http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/sortables",
    "http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/feature-references",
    "http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/profile-parameter",
    "http://www.opengis.net/spec/ogcapi-features-5/1.0/conf/profile-references",
    "http://www.opengis.net/spec/ogcapi-features-2/1.0/conf/crs",
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/create-replace-delete",
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/update",
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/features",
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables",
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables-query-parameters",
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/filter",
    "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/features-filter",
    "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
    "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
    "http://www.opengis.net/spec/cql2/1.0/conf/advanced-comparison-operators",
    "http://www.opengis.net/spec/cql2/1.0/conf/case-insensitive-comparison",
    "http://www.opengis.net/spec/cql2/1.0/conf/accent-insensitive-comparison",
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-spatial-functions",
    "http://www.opengis.net/spec/cql2/1.0/conf/basic-spatial-functions-plus",
    "http://www.opengis.net/spec/cql2/1.0/conf/spatial-functions",
    "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions",
    "http://www.opengis.net/spec/cql2/1.0/conf/array-functions",
    "http://www.opengis.net/spec/cql2/1.0/conf/property-property",
    "http://www.opengis.net/spec/cql2/1.0/conf/arithmetic",
    "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/core",
    "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/tileset",
    "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/tilesets-list",
    "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/geodata-tilesets",
    "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/mvt",
    "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/oas30",
    "http://www.opengis.net/spec/tms/2.0/conf/tilematrixset",
    "http://www.opengis.net/spec/tms/2.0/conf/json-tilematrixset",
];

/// The Part 4 classes and the writes each needs (W1 to W3, W5). `features` names no write of its
/// own: it is declared with either of the other two.
const PART_4: [(&str, &[OperationId]); 3] = [
    (
        "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/create-replace-delete",
        &[
            OperationId::CreateEntity,
            OperationId::ReplaceEntity,
            OperationId::DeleteEntity,
        ],
    ),
    (
        "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/update",
        &[OperationId::MergeEntity],
    ),
    (
        "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/features",
        &[],
    ),
];

/// C3, W4: the classes a view's `/conformance` declares. A Part 4 class is a promise of every
/// write it names, so a view declares it only when it `offers` all of them; a read-only view
/// declares none and answers each write 405.
fn conformance(offers: &dyn Fn(OperationId) -> bool) -> Vec<&'static str> {
    let declared = |ops: &[OperationId]| !ops.is_empty() && ops.iter().all(|op| offers(*op));
    let writes = PART_4.iter().any(|(_, ops)| declared(ops));
    CONFORMANCE
        .iter()
        .copied()
        .filter(|class| match PART_4.iter().find(|(c, _)| c == class) {
            Some((_, [])) => writes,
            Some((_, ops)) => declared(ops),
            None => true,
        })
        .collect()
}

const JSON: &str = "application/json";
const GEOJSON: &str = "application/geo+json";
const OPENAPI: &str = "application/vnd.oai.openapi+json;version=3.0";
const SCHEMA: &str = "application/schema+json";

/// The OGC API - Features surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ogc;

impl Surface for Ogc {
    fn name(&self) -> &'static str {
        "ogc"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> Value {
        // T-4465, T-4477: the files `items` and `/collections` write in this build.
        let encodings: Vec<&str> = export::ITEMS.iter().map(|e| e.0).collect();
        let bundles: Vec<&str> = export::BUNDLES.iter().map(|e| e.0).collect();
        json!({
            "endpoint": format!("{PREFIX}/{{view}}/ogc/features"),
            "conformsTo": CONFORMANCE,
            "encodings": encodings,
            "bundles": bundles,
            "limits": {"exportFiles": export::files_of(std::env::var(EXPORT_FILES_ENV).ok().as_deref())},
        })
    }

    fn router(&self, ops: Ops) -> Router {
        // T-5912: every NGSI-LD write of a Tenant with a schema keeps to it.
        ops.check_writes(linkml_model::check::entity_text);
        let handler = any(move |caller: Caller, req: Request| {
            let ops = ops.clone();
            async move { serve(&ops, &caller, req).await }
        });
        let router = Router::new()
            .route("/{view}/ogc/features", handler.clone())
            .route("/{view}/ogc/features/", handler.clone())
            .route("/{view}/ogc/features/{*rest}", handler);
        // T-4465 review: one cap on the files this router writes at once.
        #[cfg(any(feature = "parquet", feature = "tabular"))]
        let router = router.layer(axum::Extension(export::Gate::default()));
        router
    }
}

/// A refusal: its status and a fixed text that never names what the view hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Problem(pub StatusCode, pub &'static str);

impl Problem {
    fn reply(self) -> Reply {
        problem(self.0, self.1)
    }
}

/// An answer: built before it is sent, so every refusal is one too.
type Reply = Response;

/// An RFC 7807 problem (Part 1 7.5.1); `detail` never names what the view hides.
fn problem(status: StatusCode, detail: &str) -> Reply {
    let body = json!({
        "type": "about:blank",
        "title": status.canonical_reason().unwrap_or_default(),
        "status": status.as_u16(),
        "detail": detail,
    });
    (
        status,
        [(CONTENT_TYPE, "application/problem+json")],
        body.to_string(),
    )
        .into_response()
}

fn document(media: &'static str, body: &Value) -> Reply {
    (StatusCode::OK, [(CONTENT_TYPE, media)], body.to_string()).into_response()
}

/// `HEAD` is the `GET` without its body: status, headers and the one `Content-Length` the
/// `GET` sends (Part 1 `/req/core/http`, RFC 9110 9.3.2).
async fn serve(ops: &Ops, caller: &Caller, req: Request) -> Response {
    let head = req.method() == Method::HEAD;
    let (mut parts, body) = req.into_parts();
    if head {
        parts.method = Method::GET;
    }
    let answer = respond(ops, caller, &parts, body).await;
    if !head {
        return answer;
    }
    let (mut parts, body) = answer.into_parts();
    // A streamed page (T-6036) sends no length: its HEAD states none either, and an empty body
    // of no stated size keeps axum from writing `Content-Length: 0`.
    if axum::body::HttpBody::size_hint(&body).exact().is_none() {
        let none = futures_util::stream::empty::<Result<String, std::io::Error>>();
        return Response::from_parts(parts, Body::from_stream(none));
    }
    let Ok(read) = ops.read_answer(body, "ogc").await else {
        let (parts, _) = problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            antares_ops::bounds::ANSWER_TOO_LARGE,
        )
        .into_parts();
        return Response::from_parts(parts, axum::body::Body::empty());
    };
    parts.headers.insert(CONTENT_LENGTH, read.len().into());
    Response::from_parts(parts, axum::body::Body::empty())
}

/// One request below `/{view}/ogc/features`, read as sent: a view id is `[a-z0-9-]`, so its
/// raw segment is its id.
async fn respond(ops: &Ops, caller: &Caller, parts: &Parts, body: Body) -> Reply {
    let (method, uri, headers) = (&parts.method, &parts.uri, &parts.headers);
    let mut segments = uri.path().split('/').skip(1);
    let id = segments.next().unwrap_or_default();
    // `ogc`, `features`: the route matched them.
    let rest: Vec<&str> = segments.skip(2).collect();
    let not_found = || problem(StatusCode::NOT_FOUND, "no such resource");
    let Some(view) = ops.view(id) else {
        return not_found();
    };
    let Some(collections) = collections(&view) else {
        return not_found();
    };
    // W4: a resource's methods are the reads and the writes the view offers on it.
    let item = match rest.as_slice() {
        ["collections", c, "items"] => Some((*c, None)),
        ["collections", c, "items", feature] => Some((*c, Some(*feature))),
        _ => None,
    };
    if let Some(reply) = offered(&view, method, item, &collections) {
        return reply;
    }
    let base = endpoint(headers, id);
    if let Some(item) = item.filter(|_| method != Method::GET) {
        return write_to(&view, caller, &collections, &base, parts, item, body).await;
    }
    if let Some(reply) = tiling(&view, caller, &collections, (&base, id), parts, &rest).await {
        return reply;
    }
    let asked = match negotiate(&rest, headers, uri) {
        Ok(asked) => asked,
        Err(refused) => return refused.reply(),
    };
    let pairs = asked.pairs;
    let at = Here::new(&view, (id, &base), caller, &collections, (ops, parts));
    match (rest.as_slice(), pairs.is_empty()) {
        ([] | [""], true) => document(JSON, &at.landing()),
        (["conformance"], true) => document(
            JSON,
            &json!({"conformsTo": conformance(&|op| view.allows(op))}),
        ),
        (["api"], true) => document(
            OPENAPI,
            &api::document(&base, &collections, &|op| view.allows(op)),
        ),
        #[cfg(feature = "tabular")]
        (["collections"], _) if asked.file.is_some() => {
            at.bundle(&pairs, asked.file.unwrap_or_default()).await
        }
        (["collections"], true) => document(JSON, &at.collections().await),
        (["collections", c], true) => match collections.get(*c) {
            Some(c) => document(JSON, &at.collection(c).await),
            None => not_found(),
        },
        (["functions"], true) => document(JSON, &json!({"functions": []})),
        (["collections", c, which @ ("schema" | "sortables" | "queryables")], true) => {
            match collections.get(*c) {
                Some(c) => {
                    let id = format!("{base}/collections/{}/{which}", c.id);
                    let doc = if *which == "queryables" {
                        schema::queryables(c, &id)
                    } else {
                        schema::document(c, &id, *which == "sortables")
                    };
                    document(SCHEMA, &doc)
                }
                None => not_found(),
            }
        }
        (["collections", c, "items"], _) => match collections.get(*c) {
            #[cfg(any(feature = "parquet", feature = "tabular"))]
            Some(c) if asked.file.is_some() => {
                at.file(c, &pairs, asked.file.unwrap_or_default()).await
            }
            Some(c) => at.items(c, &pairs, uri.query()).await,
            None => not_found(),
        },
        (["collections", c, "items", feature], _) => match (collections.get(*c), decode(feature)) {
            (Some(c), Some(feature)) => at.item(c, &feature, &pairs).await,
            _ => not_found(),
        },
        (
            []
            | ["" | "conformance" | "api" | "collections" | "functions"]
            | ["collections", _]
            | ["collections", _, "schema" | "sortables" | "queryables"],
            false,
        ) => problem(
            StatusCode::BAD_REQUEST,
            "the request names a query parameter this resource does not define",
        ),
        _ => not_found(),
    }
}

/// The request's query pairs, and the file it asks for by its `f` value (T-4465, T-4477).
struct Asked {
    pairs: Vec<(String, String)>,
    file: Option<&'static str>,
}

/// The request's [`Asked`], the `400` for a CSV or Parquet of several collections, or the `406`
/// for an `Accept` this surface writes nothing for.
fn negotiate(rest: &[&str], headers: &HeaderMap, uri: &axum::http::Uri) -> Result<Asked, Problem> {
    // Form decoding is lossy (an undecodable escape stays as written), so this never fails.
    let mut pairs = Query::<Vec<(String, String)>>::try_from_uri(uri)
        .map(|Query(pairs)| pairs)
        .unwrap_or_default();
    // `f=json` / `f=geojson` ask for the JSON this surface writes anyway, whatever `Accept`
    // says, as pygeoapi and OGC API Common links send them.
    let before = pairs.len();
    pairs.retain(|(n, v)| !(n == "f" && matches!(v.as_str(), "json" | "geojson")));
    let json = pairs.len() < before;
    let one_type = |(n, v): &(String, String)| n == "f" && export::ONE_TYPE.contains(&v.as_str());
    if matches!(rest, ["collections"]) && pairs.iter().any(one_type) {
        return Err(Problem(
            StatusCode::BAD_REQUEST,
            "a CSV or Parquet file holds one collection: ask its items, or f=zip here for several",
        ));
    }
    let items: Vec<(&str, &str)> = export::ITEMS.iter().map(|e| (e.0, e.1)).collect();
    let asked = match rest {
        ["collections", _, "items"] => export::asked(headers, pairs, &items),
        ["collections"] => export::asked(headers, pairs, export::BUNDLES),
        _ => Asked { pairs, file: None },
    };
    if asked.file.is_none() && !json && !acceptable(headers) {
        return Err(Problem(
            StatusCode::NOT_ACCEPTABLE,
            "this surface writes JSON, GeoJSON, JSON Schema, OpenAPI JSON and the files of /api",
        ));
    }
    Ok(asked)
}

/// W4: whether the resource offers `method`: its reads, and the writes the view offers on it
/// (`item`: the collection and the Feature of an `items` path). A method it does not offer is
/// 405 before the body is read (W1); `OPTIONS` answers the list. `None`: serve it.
/// A history collection's `items` offers no write (T-5100).
fn offered(
    view: &ViewOps,
    method: &Method,
    item: Option<(&str, Option<&str>)>,
    collections: &BTreeMap<String, Collection>,
) -> Option<Reply> {
    let item = item.filter(|(c, _)| {
        !collections
            .get(*c)
            .is_some_and(|c| c.history || c.output.is_some())
    });
    let writes = [
        (Method::POST, "POST"),
        (Method::PUT, "PUT"),
        (Method::PATCH, "PATCH"),
        (Method::DELETE, "DELETE"),
    ];
    let allow: Vec<&str> = ["GET", "HEAD", "OPTIONS"]
        .into_iter()
        .chain(
            writes
                .iter()
                .filter(|(m, _)| {
                    item.and_then(|(_, feature)| write::Write::of(m, feature))
                        .is_some_and(|w| view.allows(w.operation().0))
                })
                .map(|(_, name)| *name),
        )
        .collect();
    // Method names joined by commas are a header value.
    let allow_header =
        HeaderValue::from_str(&allow.join(", ")).unwrap_or(HeaderValue::from_static("GET"));
    if !allow.contains(&method.as_str()) {
        let mut refused = problem(
            StatusCode::METHOD_NOT_ALLOWED,
            "this view does not offer the method on this resource",
        );
        refused.headers_mut().insert(ALLOW, allow_header);
        return Some(refused);
    }
    (method == Method::OPTIONS).then(|| (StatusCode::OK, [(ALLOW, allow_header)]).into_response())
}

/// A write on an `items` path the view offers (W1 to W7).
async fn write_to(
    view: &ViewOps,
    caller: &Caller,
    collections: &BTreeMap<String, Collection>,
    base: &str,
    parts: &Parts,
    (c, feature): (&str, Option<&str>),
    body: Body,
) -> Reply {
    let decoded = feature.map(decode);
    let write = match &decoded {
        None => write::Write::of(&parts.method, None),
        Some(f) => f
            .as_deref()
            .and_then(|f| write::Write::of(&parts.method, Some(f))),
    };
    let (Some(c), Some(write)) = (collections.get(c), write) else {
        return problem(StatusCode::NOT_FOUND, "no such resource");
    };
    write::run(view, caller, (c, base), write, &parts.headers, body)
        .await
        .unwrap_or_else(Problem::reply)
}

/// The schema of `bytes` under this surface's bounds, parsed once per revision on this pod
/// (F-ANT5-017: every request, each tile, parsed it again).
fn parsed(
    revision: u64,
    bytes: &std::sync::Arc<[u8]>,
) -> Option<std::sync::Arc<linkml_model::Model>> {
    linkml_model::cache::read(revision, bytes, &SCHEMA_LIMITS)
        .0
        .ok()
}

/// The collections of the view, if its Tenant has a schema this surface reads.
fn collections(view: &ViewOps) -> Option<BTreeMap<String, Collection>> {
    let (revision, bytes) = view.schema()?;
    let schema = parsed(revision, &bytes)?;
    // A view of `*` names no type; it shows the whole schema (ADR-0023 rule 12).
    let mut all = if view.shows_every_name() {
        model::build(&schema, &schema.slot_iris_by_class())
    } else {
        let shown = view.shown_by_type();
        model::build(&schema.as_shown(&shown, antares_ops::core_iri), &shown)
    };
    // T-5100: beside each collection its history, where the view offers the temporal query.
    if view.allows(OperationId::TemporalQuery) {
        let past: Vec<Collection> = all.values().map(history::of).collect();
        for c in past {
            all.entry(c.id.clone()).or_insert(c);
        }
    }
    // T-5216: the view's named outputs, each over its one source's collection; a class keeps
    // its own id.
    let named: Vec<Collection> = antares_ops::outputs::listed(view.representation("ogc"), "ogc")
        .into_iter()
        .filter_map(|(_, o)| {
            let o = o.ok()?;
            model::output(all.get(&o.sources.first()?.entity_type)?, &o)
        })
        .collect();
    for c in named {
        all.entry(c.id.clone()).or_insert(c);
    }
    Some(all)
}

/// No `Accept`, or one that takes some JSON or anything.
/// The `Accept-Language` ranges as NGSI-LD `lang` reads them (RFC 9110 12.5.4: tags, `*`,
/// `;q=`), `*` when the header is absent or holds anything else.
fn language(headers: &HeaderMap) -> String {
    headers
        .get(ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 200
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-*,;=. ".contains(c))
        })
        .unwrap_or("*")
        .to_owned()
}

fn acceptable(headers: &HeaderMap) -> bool {
    headers.get_all(ACCEPT).iter().next().is_none()
        || headers.get_all(ACCEPT).iter().any(|v| {
            v.to_str().is_ok_and(|v| {
                v.split(',')
                    .map(|range| range.split(';').next().unwrap_or_default().trim())
                    .any(|m| m.contains("json") || m == "*/*" || m == "application/*")
            })
        })
}

/// The endpoint's URI as the client reached it.
fn endpoint(headers: &HeaderMap, id: &str) -> String {
    format!(
        "{}{PREFIX}/{id}/ogc/features",
        antares_ops::public_base(headers)
    )
}

/// T1 to T6: the tile matrix sets, a collection's tilesets and tiles (Tiles Part 1); `None` for
/// any other path. A collection without a geometry has no tiles.
async fn tiling(
    view: &ViewOps,
    caller: &Caller,
    collections: &BTreeMap<String, Collection>,
    (base, id): (&str, &str),
    parts: &Parts,
    rest: &[&str],
) -> Option<Reply> {
    let mapped = |c: &str| collections.get(c).filter(|c| c.geometry.is_some());
    if let ["collections", c, "tiles", tiles::TMS, z, row, col] = rest {
        return Some(match mapped(c) {
            Some(c) => tile(view, caller, c, (base, id), parts, (z, row, col)).await,
            None => problem(StatusCode::NOT_FOUND, "no such resource"),
        });
    }
    let doc = match rest {
        ["tileMatrixSets"] => Some(tiles::schemes(base)),
        ["tileMatrixSets", tiles::TMS] => Some(tiles::scheme()),
        ["collections", c, "tiles"] | ["collections", c, "tiles", tiles::TMS] => {
            let at = mapped(c).map(|c| format!("{base}/collections/{}", c.id));
            match (at, rest.len()) {
                (Some(at), 3) => Some(tiles::tilesets(base, &at, c)),
                (Some(at), _) => Some(tiles::tileset(base, &at, c)),
                (None, _) => return Some(problem(StatusCode::NOT_FOUND, "no such resource")),
            }
        }
        _ => None,
    }?;
    Some(if parts.uri.query().is_some_and(|q| !q.is_empty()) {
        problem(
            StatusCode::BAD_REQUEST,
            "the request names a query parameter this resource does not define",
        )
    } else if !acceptable(&parts.headers) {
        problem(StatusCode::NOT_ACCEPTABLE, "this resource is JSON")
    } else {
        document(JSON, &doc)
    })
}

/// T1 to T6: one vector tile of `c` (Tiles Part 1 `/req/core`, `/req/mvt`), the `items` page of
/// the tile's box through the view, so its rules and the Policy narrow it ([`tiles::reply`]).
async fn tile(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    (base, id): (&str, &str),
    parts: &Parts,
    (z, row, col): (&str, &str, &str),
) -> Reply {
    let Some(at) = tiles::Tile::of(z, row, col) else {
        return problem(
            StatusCode::NOT_FOUND,
            "no such tile in WebMercatorQuad 0 to 22",
        );
    };
    if parts.uri.query().is_some_and(|q| !q.is_empty()) {
        return problem(StatusCode::BAD_REQUEST, "a tile takes no query parameter");
    }
    if !takes(&parts.headers, tiles::MVT) {
        return problem(
            StatusCode::NOT_ACCEPTABLE,
            "a tile is application/vnd.mapbox-vector-tile",
        );
    }
    let p = items::Params {
        limit: tiles::MAX_FEATURES,
        bbox: Some(at.bbox()),
        crs: crs::CRS84,
        ..items::Params::default()
    };
    let refs = items::Refs {
        profile: items::Profile::Key,
        base,
        alternate: false,
    };
    let digest = items::digest(id, &c.id, &[]);
    let page = match features::page(view, caller, c, &p, digest, refs).await {
        Ok(page) => page,
        Err(refused) => return refused.reply(),
    };
    let held: Vec<&str> = parts
        .headers
        .get_all(axum::http::header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    let truncated = page.next.is_some();
    tiles::reply(
        &c.id,
        at,
        &page.features,
        truncated,
        &held,
        tiles::MAX_BYTES,
    )
}

/// Whether the request's `Accept` takes `media` (none, `*/*` and `application/*` do).
fn takes(headers: &HeaderMap, media: &str) -> bool {
    headers.get_all(ACCEPT).iter().next().is_none()
        || headers.get_all(ACCEPT).iter().any(|v| {
            v.to_str().is_ok_and(|v| {
                v.split(',')
                    .map(|range| range.split(';').next().unwrap_or_default().trim())
                    .any(|m| m == media || m == "*/*" || m == "application/*")
            })
        })
}

use antares_ops::percent::decoded as decode;

/// The link object of Part 1 (`rel`, `type`, `href`, Req 29).
fn link(rel: &str, media: &str, href: &str) -> Value {
    json!({"rel": rel, "type": media, "href": href})
}

/// The link relation of a collection's schema (Part 5 `/req/returnables-and-receivables`).
const SCHEMA_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/schema";
/// The link relation of a collection's sortables (Part 5 `/req/sortables`).
const SORTABLES_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/sortables";
/// The link relation of a collection's queryables (Part 3 `/req/queryables/get-queryables-uri`).
const QUERYABLES_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/queryables";
/// The link relation of the functions a filter may call (CQL2 `functions`; Part 3 F14).
const FUNCTIONS_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/functions";

/// The `profile` link of a response whose collection holds references (Part 5
/// `/req/profile-references`): the profile its references are written in.
fn profile_link(c: &Collection, profile: items::Profile) -> Option<Value> {
    c.properties
        .iter()
        .any(|p| p.reference)
        .then(|| json!({"rel": "profile", "href": items::profile_uri(profile)}))
}

/// A Feature answer with the `Content-Crs` of its coordinates (R4).
fn in_crs(crs: &str, mut reply: Reply) -> Reply {
    // A CRS URI is ASCII: always a header value.
    let value = HeaderValue::from_str(&format!("<{crs}>")).unwrap_or(HeaderValue::from_static(""));
    reply.headers_mut().insert("content-crs", value);
    reply
}

/// What one request reads under.
struct Here<'a> {
    view: &'a ViewOps,
    id: &'a str,
    base: &'a str,
    /// The caller's `Accept-Language`, `*` without one: a `LanguageProperty` is one text in a
    /// Feature (API/02 §1, EP-37), as NGSI-LD `lang` picks it (TS 104 175 4.15).
    lang: String,
    caller: &'a Caller,
    collections: &'a BTreeMap<String, Collection>,
    /// The ceiling a file this surface assembles is written under and the files the router is
    /// writing (T-4465).
    #[cfg(any(feature = "parquet", feature = "tabular"))]
    budget: export::Budget,
}

impl<'a> Here<'a> {
    /// A request's place: its view, its id and endpoint, its caller and what it accepts.
    fn new(
        view: &'a ViewOps,
        (id, base): (&'a str, &'a str),
        caller: &'a Caller,
        collections: &'a BTreeMap<String, Collection>,
        (ops, parts): (&Ops, &Parts),
    ) -> Self {
        // read only by the export budget
        let _ = ops;
        Self {
            view,
            id,
            base,
            lang: language(&parts.headers),
            caller,
            collections,
            #[cfg(any(feature = "parquet", feature = "tabular"))]
            budget: export::Budget::of(ops, parts),
        }
    }
}

impl Here<'_> {
    /// How this request's Features write references and their EP-38 links.
    fn refs(&self, profile: items::Profile) -> items::Refs<'_> {
        items::Refs {
            profile,
            base: self.base,
            alternate: self.view.allows(OperationId::RetrieveEntity),
        }
    }

    /// C1: the landing page.
    fn landing(&self) -> Value {
        let b = self.base;
        json!({
            "title": format!("OGC API - Features of view {}", self.id),
            "links": [
                link("self", JSON, b),
                link("service-desc", OPENAPI, &format!("{b}/api")),
                link("conformance", JSON, &format!("{b}/conformance")),
                link("data", JSON, &format!("{b}/collections")),
                link(FUNCTIONS_REL, JSON, &format!("{b}/functions")),
                link(tiles::SCHEMES_REL, JSON, &format!("{b}/tileMatrixSets")),
            ],
        })
    }

    /// C5: every collection, with its links; their extents are read side by side.
    async fn collections(&self) -> Value {
        let all: Vec<Value> =
            futures_util::future::join_all(self.collections.values().map(|c| self.collection(c)))
                .await;
        json!({
            "links": [link("self", JSON, &format!("{}/collections", self.base))],
            "crs": [crs::CRS84, crs::CRS84H],
            "collections": all,
        })
    }

    /// C7: one collection, with the `extent` of what the caller reads (C6, M8).
    async fn collection(&self, c: &Collection) -> Value {
        let at = format!("{}/collections/{}", self.base, c.id);
        let mut links = vec![
            link("self", JSON, &at),
            link("items", GEOJSON, &format!("{at}/items")),
            link(SCHEMA_REL, SCHEMA, &format!("{at}/schema")),
            link(SORTABLES_REL, SCHEMA, &format!("{at}/sortables")),
            link(QUERYABLES_REL, SCHEMA, &format!("{at}/queryables")),
        ];
        if c.geometry.is_some() {
            links.push(link(tiles::TILESETS_REL, JSON, &format!("{at}/tiles")));
        }
        let fields = [
            ("id", Some(Value::from(c.id.as_str()))),
            (
                "title",
                Some(Value::from(c.output.as_ref().map_or(c.id.as_str(), |o| {
                    o.title_in(self.lang.split([',', ';', '-']).next().unwrap_or_default())
                }))),
            ),
            ("description", c.description.as_deref().map(Value::from)),
            (
                "extent",
                extent::of(self.view, self.id, self.caller, c).await,
            ),
            ("itemType", Some(Value::from("feature"))),
            ("crs", Some(json!([crs::CRS84, crs::CRS84H]))),
            ("storageCrs", Some(Value::from(crs::CRS84))),
            ("links", Some(Value::from(links))),
        ];
        Value::Object(
            fields
                .into_iter()
                .filter_map(|(k, v)| Some((k.to_owned(), v?)))
                .collect(),
        )
    }

    /// C8 to C18: one page of Features.
    async fn items(
        &self,
        c: &Collection,
        pairs: &[(String, String)],
        query: Option<&str>,
    ) -> Reply {
        let p = match items::params(c, pairs) {
            Ok(p) => items::Params {
                lang: Some(self.lang.clone()),
                ..p
            },
            Err(refused) => return refused.reply(),
        };
        let digest = items::digest(self.id, &c.id, pairs);
        let close = self.close(c, pairs, query, p.profile);
        if c.history {
            return match features::page(self.view, self.caller, c, &p, digest, self.refs(p.profile))
                .await
            {
                Ok(page) => in_crs(
                    p.crs,
                    close.document(page.features, page.next.as_deref(), page.matched),
                ),
                Err(refused) => refused.reply(),
            };
        }
        let mut cursor = match items::Cursor::new(c, &p, digest) {
            Ok(cursor) => cursor,
            Err(refused) => return refused.reply(),
        };
        let mut held = Vec::new();
        // A page that fits one store read is answered whole, with its length (T-6036).
        while let Some(more) = match cursor
            .chunk(self.view, self.caller, c, self.refs(p.profile))
            .await
        {
            Ok(more) => more,
            Err(refused) => return refused.reply(),
        } {
            held.extend(more);
            if !cursor.done() && held.len() >= self.view.page_size() {
                let rest = Streamed {
                    view: self.view.clone(),
                    caller: self.caller.clone(),
                    c: c.clone(),
                    base: self.base.to_owned(),
                    profile: p.profile,
                    cursor,
                    close,
                    returned: held.len(),
                };
                return in_crs(p.crs, rest.reply(&held));
            }
        }
        in_crs(
            p.crs,
            close.document(held, cursor.next.as_deref(), cursor.matched),
        )
    }

    /// The members of an `items` answer beside its Features.
    fn close(
        &self,
        c: &Collection,
        pairs: &[(String, String)],
        query: Option<&str>,
        profile: items::Profile,
    ) -> Close {
        let at = format!("{}/collections/{}/items", self.base, c.id);
        let this = query.map_or_else(|| at.clone(), |q| format!("{at}?{q}"));
        let links = vec![
            link("self", GEOJSON, &this),
            link(
                "collection",
                JSON,
                &format!("{}/collections/{}", self.base, c.id),
            ),
        ];
        let kept: Vec<String> = pairs
            .iter()
            .filter(|(n, _)| n != "next")
            .map(|(n, v)| format!("{}={}", items::encode(n), items::encode(v)))
            .chain([String::from("next=")])
            .collect();
        let separator = if this.contains('?') { '&' } else { '?' };
        let after = profile_link(c, profile)
            .into_iter()
            .chain(
                export::ITEMS
                    .iter()
                    .map(|e| link("alternate", e.1, &format!("{this}{separator}f={}", e.0))),
            )
            .collect();
        Close {
            // C17: when the answer was made, a response time and no Entity time; whole
            // seconds, so a HEAD states the length of the GET.
            stamp: items::now()
                .and_then(|t| {
                    let seconds = t.unix_micros().checked_div(1_000_000)?;
                    antares_ops::residual::DateTime::from_unix_micros(
                        seconds.checked_mul(1_000_000)?,
                    )
                })
                .as_ref()
                .map(ToString::to_string),
            links,
            next_at: format!("{at}?{}", kept.join("&")),
            after,
        }
    }

    /// C19: one Feature; `properties` and `profile` are its only parameters (S6, S7).
    async fn item(&self, c: &Collection, id: &str, pairs: &[(String, String)]) -> Reply {
        let p = match items::item_params(c, pairs) {
            Ok(p) => items::Params {
                lang: Some(self.lang.clone()),
                ..p
            },
            Err(refused) => return refused.reply(),
        };
        let refs = self.refs(p.profile);
        match features::one(self.view, self.caller, c, id, &p, refs).await {
            Ok(mut feature) => {
                let at = format!("{}/collections/{}", self.base, c.id);
                let own = format!("{at}/items/{}", items::encode(id));
                let links: Vec<Value> =
                    [link("self", GEOJSON, &own), link("collection", JSON, &at)]
                        .into_iter()
                        .chain(profile_link(c, p.profile))
                        .chain(refs.links(&c.id, id))
                        .collect();
                feature.insert("links".into(), Value::from(links));
                in_crs(p.crs, document(GEOJSON, &Value::Object(feature)))
            }
            Err(refused) => refused.reply(),
        }
    }
}

/// What a `FeatureCollection` holds beside `features` (C11, C16, C17): known before its page is
/// read, but for `next`, `numberReturned` and `numberMatched`.
struct Close {
    stamp: Option<String>,
    /// `self` and `collection`, before a `next` link.
    links: Vec<Value>,
    /// The `next` link's URI without its token.
    next_at: String,
    /// The `profile` and `alternate` links, after a `next` link.
    after: Vec<Value>,
}

impl Close {
    /// The members but `type` and `features`.
    fn members(
        &self,
        next: Option<&str>,
        matched: Option<u64>,
        returned: usize,
    ) -> serde_json::Map<String, Value> {
        let links: Vec<Value> = self
            .links
            .iter()
            .cloned()
            .chain(next.map(|t| link("next", GEOJSON, &format!("{}{t}", self.next_at))))
            .chain(self.after.iter().cloned())
            .collect();
        let mut members = serde_json::Map::new();
        members.insert("timeStamp".into(), json!(self.stamp));
        members.insert("numberReturned".into(), json!(returned));
        members.insert("links".into(), Value::from(links));
        // Optional in Part 1: omitted when the count would need more than the budget reads.
        if let Some(matched) = matched {
            members.insert("numberMatched".into(), json!(matched));
        }
        members
    }

    /// A page read whole.
    fn document(&self, features: Vec<Value>, next: Option<&str>, matched: Option<u64>) -> Reply {
        let mut body = self.members(next, matched, features.len());
        body.insert("type".into(), json!("FeatureCollection"));
        body.insert("features".into(), Value::from(features));
        document(GEOJSON, &Value::Object(body))
    }
}

/// A page past one store read, written as it is read (T-6036): each further store read is
/// one body frame, so the request holds one upstream page of Features, and the members known
/// only at the end follow `features`. A refusal after the first byte ends the body with an
/// error: the client sees a broken transfer, never a complete `FeatureCollection`.
struct Streamed {
    view: ViewOps,
    caller: Caller,
    c: Collection,
    base: String,
    profile: items::Profile,
    cursor: items::Cursor,
    close: Close,
    returned: usize,
}

impl Streamed {
    /// The answer whose body starts with the Features `held` and goes on with the rest.
    fn reply(self, held: &[Value]) -> Reply {
        let opening = format!(
            "{{\"type\":\"FeatureCollection\",\"features\":[{}",
            listed(held).get(1..).unwrap_or_default()
        );
        let rest = futures_util::stream::unfold(Some(self), |state| async move {
            let mut s = state?;
            let refs = items::Refs {
                profile: s.profile,
                base: &s.base,
                alternate: s.view.allows(OperationId::RetrieveEntity),
            };
            match s.cursor.chunk(&s.view, &s.caller, &s.c, refs).await {
                Ok(Some(more)) => {
                    s.returned = s.returned.saturating_add(more.len());
                    Some((Ok(listed(&more)), Some(s)))
                }
                Ok(None) => {
                    let members =
                        s.close
                            .members(s.cursor.next.as_deref(), s.cursor.matched, s.returned);
                    let tail = Value::Object(members).to_string();
                    Some((Ok(format!("],{}", tail.get(1..).unwrap_or_default())), None))
                }
                Err(Problem(_, detail)) => Some((Err(std::io::Error::other(detail)), None)),
            }
        });
        let body = futures_util::StreamExt::chain(futures_util::stream::iter([Ok(opening)]), rest);
        (
            StatusCode::OK,
            [(CONTENT_TYPE, GEOJSON)],
            Body::from_stream(body),
        )
            .into_response()
    }
}

/// Each Feature after a comma.
fn listed(features: &[Value]) -> String {
    let mut out = String::new();
    for f in features {
        out.push(',');
        out.push_str(&f.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-ANT5-017: two requests over one revision share one parse.
    #[test]
    fn a_revision_is_parsed_once() {
        let schema: &std::sync::Arc<[u8]> =
            &br#"{"id": "https://e.org/ogc-once", "name": "s"}"#[..].into();
        let first = parsed(9_017, schema);
        let again = parsed(9_017, schema);
        assert_eq!(
            first
                .zip(again)
                .map(|(a, b)| std::sync::Arc::ptr_eq(&a, &b)),
            Some(true)
        );
    }

    #[test]
    fn the_surface_names_its_endpoint() {
        assert_eq!(Ogc.name(), "ogc");
        assert_eq!(
            Ogc.version_info().pointer("/endpoint"),
            Some(&json!("/x/views/{view}/ogc/features"))
        );
        // T-4465, T-4477: the surfaces listing names a file only in a build that writes it.
        let encodings: Vec<&str> = [
            #[cfg(feature = "parquet")]
            "parquet",
            #[cfg(feature = "tabular")]
            "csv",
        ]
        .into();
        let bundles: Vec<&str> = [
            #[cfg(feature = "tabular")]
            "zip",
        ]
        .into();
        let info = Ogc.version_info();
        assert_eq!(info.pointer("/encodings"), Some(&json!(encodings)));
        assert_eq!(info.pointer("/bundles"), Some(&json!(bundles)));
        assert_eq!(info.pointer("/limits/exportFiles"), Some(&json!(2)));
    }

    #[test]
    fn a_part_4_class_is_declared_only_with_every_write_it_names() {
        let part_4 = |offered: &[OperationId]| -> Vec<&str> {
            conformance(&|op| offered.contains(&op))
                .into_iter()
                .filter(|c| c.contains("ogcapi-features-4"))
                .filter_map(|c| c.rsplit('/').next())
                .collect()
        };
        let (create, replace, merge, delete) = (
            OperationId::CreateEntity,
            OperationId::ReplaceEntity,
            OperationId::MergeEntity,
            OperationId::DeleteEntity,
        );
        let none: [&str; 0] = [];
        assert_eq!(part_4(&[OperationId::QueryEntities]), none);
        assert_eq!(part_4(&[create]), none, "create alone is no class");
        assert_eq!(part_4(&[create, replace]), none, "nor without delete");
        assert_eq!(part_4(&[merge]), ["update", "features"]);
        assert_eq!(
            part_4(&[create, replace, delete]),
            ["create-replace-delete", "features"]
        );
        assert_eq!(
            part_4(&[create, replace, merge, delete]),
            ["create-replace-delete", "update", "features"]
        );
        assert_eq!(
            conformance(&|_| true),
            CONFORMANCE,
            "a view offering every write declares every class, in order"
        );
    }

    #[test]
    fn segments_decode_or_are_refused() {
        assert_eq!(
            decode("urn%3Angsi-ld%3ACar%3A1").as_deref(),
            Some("urn:ngsi-ld:Car:1")
        );
        assert_eq!(
            decode("urn:ngsi-ld:Car:1").as_deref(),
            Some("urn:ngsi-ld:Car:1")
        );
        assert_eq!(decode("a%2"), None);
        assert_eq!(decode("a%zz"), None);
        assert_eq!(decode("%ff"), None, "not UTF-8");
    }

    #[test]
    fn accept_takes_json_or_anything() {
        let with = |v: &'static str| {
            let mut h = HeaderMap::new();
            h.insert(ACCEPT, HeaderValue::from_static(v));
            acceptable(&h)
        };
        assert!(acceptable(&HeaderMap::new()));
        assert!(with("application/geo+json"));
        assert!(with("text/html;q=1, */*;q=0.1"));
        assert!(with("application/*"));
        assert!(!with("text/html"));
        assert!(!with("application/gml+xml; profile=x"));
    }
}
