// SPDX-License-Identifier: EUPL-1.2
//! The NGSI-LD view root (ADR-0023): `/x/views/{view}/ngsi-ld/v1` is an
//! NGSI-LD API root (TS 104 176 clause 5, `/x/views/{view}` its optional
//! prefix) whose every operation is the broker's own, run through
//! [`antares_ops`] composed with the view's narrowing, in the view's one
//! Tenant, and only when the view allows it.
//!
//! This crate serves the reads: Retrieve and Query Entities (10.4.2, 10.4.3,
//! the batch query), Retrieve Attribute Value (10.4.4), discovery (10.6.3 to
//! 10.6.8), the temporal reads (11.3.2, 11.3.3) and List @contexts (13.3,
//! the core one alone, rule 5), and the writes (10.2.2 to 10.2.11 but
//! Purge; 10.3.2, 10.3.4 to 10.3.6 member by member, with the
//! `BatchOperationResult` of 207 when mixed): the Entity is in the view
//! before the write and after
//! it, every Attribute the write names is one the view shows, and every
//! Relationship it writes points at an Entity the view shows (rule 7), or
//! the write is refused and the store keeps what it held. Any other
//! operation answers 405 with the
//! `Allow` the view serves on that resource, 422 when the view serves the
//! method there but not this operation of it (rule 4), and 404 where it
//! serves nothing at all. Entity maps (14.x) are not served through a
//! view: a request that names one is 400.
//!
//! A view of another model (one with a `mapping`, gap G18) serves Retrieve
//! Entity, Query Entities and Retrieve Attribute Value in its target model:
//! the request is inverted into the source model before it runs through the
//! view, and the answer rebuilt after (`docs/analysis/view-mapping.md`).
//!
//! ```
//! use antares_ops::Surface;
//! let root = antares_views::Views;
//! assert_eq!(root.prefix(), "/x/views");
//! let mounted = antares_ops::mount(root);
//! assert_eq!(mounted.name(), "views");
//! ```
#![forbid(unsafe_code)]

use antares_ops::{Caller, OperationId, Ops, OpsRequest, Refusal, Surface, ViewOps};
use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue, Method};
use axum::response::Response;
use axum::routing::any;
use axum::Router;

pub mod union;

/// The variables this crate reads, for the broker's unknown-key check.
pub const ENV_KEYS: &[&str] = &[union::MAX_VIEWS_ENV];

/// Where the view root mounts.
pub const PREFIX: &str = "/x/views";

/// The operations this crate serves through a view: the reads (ADR-0023
/// rules 1 to 3, 5, 6 and 8) and the writes the broker checks on the stored
/// Entity (rule 7), batches member by member, and the Subscriptions made
/// through it (rule 9). A view that allows any other
/// operation (Purge, the temporal writes) answers it with 405 or 422.
pub const SERVED: [OperationId; 34] = [
    OperationId::CreateSubscription,
    OperationId::UpdateSubscription,
    OperationId::RetrieveSubscription,
    OperationId::QuerySubscriptions,
    OperationId::DeleteSubscription,
    OperationId::BatchCreate,
    OperationId::BatchUpsert,
    OperationId::BatchUpdate,
    OperationId::BatchMerge,
    OperationId::BatchDelete,
    OperationId::CreateEntity,
    OperationId::UpdateAttributes,
    OperationId::AppendAttributes,
    OperationId::PartialAttributeUpdate,
    OperationId::SetAttributeValue,
    OperationId::DeleteAttribute,
    OperationId::DeleteEntity,
    OperationId::MergeEntity,
    OperationId::ReplaceEntity,
    OperationId::ReplaceAttribute,
    OperationId::RetrieveEntity,
    OperationId::QueryEntities,
    OperationId::QueryEntitiesPost,
    OperationId::RetrieveAttributeValue,
    OperationId::RetrieveEntityTypes,
    OperationId::RetrieveEntityTypeDetails,
    OperationId::RetrieveEntityTypeInfo,
    OperationId::RetrieveAttributes,
    OperationId::RetrieveAttributeDetails,
    OperationId::RetrieveAttributeInfo,
    OperationId::TemporalRetrieve,
    OperationId::TemporalQuery,
    OperationId::TemporalQueryPost,
    OperationId::ListContexts,
];

/// The methods an NGSI-LD resource can bind, `HEAD` following `GET`.
const METHODS: [Method; 5] = [
    Method::GET,
    Method::POST,
    Method::PATCH,
    Method::PUT,
    Method::DELETE,
];

/// The view root, as the broker's `views` feature mounts it.
pub struct Views;

impl Surface for Views {
    fn name(&self) -> &'static str {
        "views"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> serde_json::Value {
        serde_json::json!({"apiRoot": format!("{PREFIX}/{{view}}/ngsi-ld/v1")})
    }

    fn router(&self, ops: Ops) -> Router {
        let union_ops = ops.clone();
        Router::new()
            .route(
                union::ROUTE,
                any(move |caller: Caller, req: Request| {
                    let ops = union_ops.clone();
                    async move { union::serve(&ops, &caller, req).await }
                }),
            )
            .route(
                "/{view}/ngsi-ld/v1/{*rest}",
                any(move |caller: Caller, req: Request| {
                    let ops = ops.clone();
                    async move { serve(&ops, &caller, req).await }
                }),
            )
    }
}

async fn serve(ops: &Ops, caller: &Caller, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    // The path as sent, below the prefix: `/{view}/ngsi-ld/v1/...` (the
    // route guarantees both parts). A view id is `[a-z0-9-]`, so its raw
    // segment is its id.
    let mut parts_of = parts.uri.path().splitn(3, '/').skip(1);
    let id = parts_of.next().unwrap_or_default();
    let tail = parts_of.next().unwrap_or_default();
    let Some(view) = ops.mapped_view(id) else {
        return Refusal::NotFound.ngsi_ld();
    };
    if names_an_entity_map(&parts) {
        return Refusal::Malformed(NO_MAPS).ngsi_ld();
    }
    let path = format!("/{tail}");
    let query = parts.uri.query();
    let op = match OperationId::of(parts.method.as_str(), &path, query) {
        Some(op) if served(&view, op) => op,
        _ => return refusal(&view, &parts.method, &path, query).ngsi_ld(),
    };
    let target = match query {
        Some(q) => format!("{path}?{q}"),
        None => path,
    };
    let mut inner = OpsRequest::new(parts.method, target).body(body);
    for (name, value) in &parts.headers {
        inner = inner.header(name.clone(), value.clone());
    }
    match view.run(op, caller, inner).await {
        Ok(answer) => rebase(answer, id),
        Err(refusal) => refusal.ngsi_ld(),
    }
}

/// Why a request naming an entity map is refused.
pub const NO_MAPS: &str = "entity maps are not served through a view";

/// Whether the request creates or reads an Entity Map (TS 104 175 14.x):
/// the `entityMap` parameter, however its name is encoded, or the
/// `NGSILD-EntityMap` header. A map holds the ids one query selected and
/// pages through them later, so a map built through one view and read
/// through another, or at the root, would carry the first one's id set
/// (threat V22); through a view none is built or read. A query that is no
/// form encoding is refused alike, as the NGSI-LD root refuses it.
fn names_an_entity_map(parts: &axum::http::request::Parts) -> bool {
    parts.headers.contains_key("ngsild-entitymap")
        || axum::extract::Query::<Vec<(String, String)>>::try_from_uri(&parts.uri)
            .map_or(true, |q| q.0.iter().any(|(k, _)| k == "entityMap"))
}

fn served(view: &ViewOps, op: OperationId) -> bool {
    SERVED.contains(&op) && view.allows(op)
}

/// ADR-0023 rule 4 for a request the view does not serve: 405 with the
/// methods it serves on the resource, 422 when the request's method is one
/// of them (another operation of the same method and path, such as
/// `?details=true`), 404 when it serves the resource not at all.
fn refusal(view: &ViewOps, method: &Method, path: &str, query: Option<&str>) -> Refusal {
    let mut allow: Vec<Method> = METHODS
        .into_iter()
        .filter(|m| {
            [query, None]
                .into_iter()
                .filter_map(|q| OperationId::of(m.as_str(), path, q))
                .any(|op| served(view, op))
        })
        .collect();
    if allow.contains(&Method::GET) {
        allow.insert(1, Method::HEAD);
    }
    let method = if method == Method::HEAD {
        &Method::GET
    } else {
        method
    };
    if allow.is_empty() {
        Refusal::NotFound
    } else if allow.contains(method) {
        Refusal::NotSupported
    } else {
        Refusal::NotAllowed { allow }
    }
}

/// The broker's links name its own API root; through a view they name the
/// view's, so a next page or a created resource stays inside the view.
fn rebase(mut answer: Response, id: &str) -> Response {
    let root = format!("{PREFIX}/{id}");
    for name in [header::LINK, header::LOCATION, header::CONTENT_LOCATION] {
        rebase_header(answer.headers_mut(), &name, &root);
    }
    answer
}

fn rebase_header(headers: &mut axum::http::HeaderMap, name: &HeaderName, root: &str) {
    let values: Vec<HeaderValue> = headers
        .get_all(name)
        .iter()
        .map(|v| rebase_value(v, root))
        .collect();
    if values.is_empty() {
        return;
    }
    headers.remove(name);
    for v in values {
        headers.append(name.clone(), v);
    }
}

fn rebase_value(value: &HeaderValue, root: &str) -> HeaderValue {
    const API: &str = "/ngsi-ld/v1";
    // A value that is no text names no path, and the root is a path, so a
    // rebased text is a header value again.
    value
        .to_str()
        .ok()
        .map(|text| match text.strip_prefix(API) {
            Some(rest) => format!("{root}{API}{rest}"),
            None => text.replace(&format!("<{API}"), &format!("<{root}{API}")),
        })
        .and_then(|rebased| HeaderValue::from_str(&rebased).ok())
        .unwrap_or_else(|| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_names_itself_and_its_api_root() {
        assert_eq!(Views.name(), "views");
        assert_eq!(Views.prefix(), PREFIX);
        assert_eq!(
            Views.version_info(),
            serde_json::json!({"apiRoot": "/x/views/{view}/ngsi-ld/v1"})
        );
    }

    #[test]
    fn a_link_or_location_under_the_api_root_moves_under_the_view() {
        let root = "/x/views/cars";
        let cases = [
            (
                "</ngsi-ld/v1/entities?limit=1&offset=1>; rel=\"next\";type=\"application/ld+json\"",
                "</x/views/cars/ngsi-ld/v1/entities?limit=1&offset=1>; rel=\"next\";type=\"application/ld+json\"",
            ),
            (
                "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1",
                "/x/views/cars/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1",
            ),
            (
                "<https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context.jsonld>; rel=\"http://www.w3.org/ns/json-ld#context\"",
                "<https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context.jsonld>; rel=\"http://www.w3.org/ns/json-ld#context\"",
            ),
        ];
        for (from, to) in cases {
            assert_eq!(
                rebase_value(&HeaderValue::from_static(from), root),
                HeaderValue::from_static(to),
                "{from}"
            );
        }
        let opaque =
            HeaderValue::from_bytes(b"\xff/ngsi-ld/v1").unwrap_or(HeaderValue::from_static("x"));
        assert_eq!(
            rebase_value(&opaque, root),
            opaque,
            "a value that is no text stays"
        );
    }

    #[test]
    fn every_value_of_a_repeated_header_is_rebased_and_kept() {
        let mut h = axum::http::HeaderMap::new();
        h.append(
            header::LINK,
            HeaderValue::from_static("</ngsi-ld/v1/a>; rel=\"next\""),
        );
        h.append(
            header::LINK,
            HeaderValue::from_static("</ngsi-ld/v1/b>; rel=\"prev\""),
        );
        rebase_header(&mut h, &header::LINK, "/x/views/v");
        let got: Vec<_> = h.get_all(header::LINK).iter().collect();
        assert_eq!(
            got,
            [
                "</x/views/v/ngsi-ld/v1/a>; rel=\"next\"",
                "</x/views/v/ngsi-ld/v1/b>; rel=\"prev\""
            ]
        );
        rebase_header(&mut h, &header::LOCATION, "/x/views/v");
        assert!(!h.contains_key(header::LOCATION));
    }
}
