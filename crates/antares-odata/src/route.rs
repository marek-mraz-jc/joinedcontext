// SPDX-License-Identifier: EUPL-1.2
//! Which handler a request below the service root goes to: reads by path,
//! writes by method and path. Only this module knows every handler module;
//! the handlers share helpers through `read` and `write` and never call
//! back up here, so the modules form no cycle (T-4020).

use crate::below;
use crate::paths;
use crate::read::{
    collection, count, dereference, entity, later, no_properties, not_implemented, only_select,
    refused, shaped, written, Reads,
};
use crate::write::{change, create, delete};
use antares_ops::{Caller, OperationId};
use axum::http::header::{IF_MATCH, IF_NONE_MATCH};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use odata_axum::{json_level, ODataErrorResponse, ODataRequest};
use odata_json::{Ctx, WriteKind};
use odata_url::{Key, ODataUri, QueryOptions, Segment};

/// One request below the service root that is not the service or metadata
/// document.
pub(crate) async fn serve(
    reads: &Reads,
    method: &Method,
    caller: &Caller,
    request: &ODataRequest,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let version = request.version;
    if let ODataUri::Resource { path, options } = &request.uri {
        let collection = matches!(
            path.as_slice(),
            [Segment::EntitySet(_)] | [Segment::EntitySet(_), Segment::Ref | Segment::Count]
        );
        if !(options.custom.is_empty() || collection && method == Method::GET) {
            return ODataErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "BadRequest",
                "the ngsild options apply to reading a collection",
            )
            .with_version(version)
            .into_response();
        }
    }
    if method != Method::GET {
        return write(reads, method, caller, request, headers, body).await;
    }
    let level = match json_level(request, headers, reads.state.max_header_len) {
        Ok(level) => level,
        Err(refused) => return refused.into_response(),
    };
    let ctx = Ctx {
        service_root: &reads.state.service_root,
        level,
        version,
        metadata_etag: reads.state.metadata_etag.as_deref(),
    };
    if let ODataUri::Entity { options } = &request.uri {
        return dereference(reads, caller, &ctx, options)
            .await
            .unwrap_or_else(IntoResponse::into_response);
    }
    let refused = later(&request.uri, &reads.edm.schema_version);
    let (ODataUri::Resource { path, options }, None) = (&request.uri, refused) else {
        return not_implemented(refused.unwrap_or("this resource"), version).into_response();
    };
    if let Some(set) = history_set(reads, path) {
        return history(reads, caller, &ctx, set, path, options, headers)
            .await
            .unwrap_or_else(IntoResponse::into_response);
    }
    let answer = match path.as_slice() {
        [_, Segment::Ref] | [_, _, Segment::Ref] if shaped(options) => Err(no_properties(version)),
        [Segment::EntitySet(set)] | [Segment::EntitySet(set), Segment::Ref]
            if options.apply.is_some() =>
        {
            crate::apply::collection(reads, caller, &ctx, (set, path), options, headers).await
        }
        [Segment::EntitySet(set)] | [Segment::EntitySet(set), Segment::Ref] => {
            collection(reads, caller, &ctx, set, path, options, headers).await
        }
        [Segment::EntitySet(set), Segment::Count] => {
            count(reads, caller, set, options, version).await
        }
        [Segment::EntitySet(set), Segment::Key(key)] => {
            entity(reads, caller, &ctx, set, key, options, headers).await
        }
        [Segment::EntitySet(set), Segment::Key(key), Segment::Ref] if only_select(options) => {
            paths::reference(reads, caller, &ctx, (set, key), options).await
        }
        // Below one Entity only `$select` applies (Q31 to Q33).
        [_, _, ..] if !only_select(options) => Err(not_implemented(
            "a query option other than $select below one Entity",
            version,
        )),
        [Segment::EntitySet(set), Segment::Key(key), Segment::Property(name)] => {
            paths::property(reads, caller, &ctx, (set, key, name), false, options).await
        }
        [Segment::EntitySet(set), Segment::Key(key), Segment::Property(name), Segment::Value] => {
            paths::property(reads, caller, &ctx, (set, key, name), true, options).await
        }
        [Segment::EntitySet(set), Segment::Key(key), Segment::Navigation(nav)] => {
            paths::related(reads, caller, &ctx, (set, key, nav), false, options).await
        }
        [Segment::EntitySet(set), Segment::Key(key), Segment::Navigation(nav), Segment::Ref] => {
            paths::related(reads, caller, &ctx, (set, key, nav), true, options).await
        }
        _ => Err(not_implemented("this resource path", version)),
    };
    answer.unwrap_or_else(IntoResponse::into_response)
}

/// The history set the path starts at (T-5908): a path starts at an EntitySet, written as its
/// name.
fn history_set<'r>(reads: &'r Reads, path: &[Segment]) -> Option<&'r crate::edm::Set> {
    reads.edm.histories.get(&path.first()?.to_string())
}

/// A read of a history set (T-5908): the set, its `$count`, one row by key.
async fn history(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    set: &crate::edm::Set,
    path: &[Segment],
    options: &QueryOptions,
    headers: &HeaderMap,
) -> Result<Response, ODataErrorResponse> {
    match path {
        [Segment::EntitySet(name)] | [Segment::EntitySet(name), Segment::Ref] => {
            crate::history::collection(reads, caller, ctx, (name, set), path, options, headers)
                .await
        }
        [_, Segment::Count] => {
            crate::history::count(reads, caller, set, options, ctx.version).await
        }
        [Segment::EntitySet(name), Segment::Key(key)] => {
            crate::history::entity(reads, caller, ctx, (name, set), key, options).await
        }
        _ => Err(not_implemented(
            "this path below a history set",
            ctx.version,
        )),
    }
}

/// A request that is not a `GET`: the write its method and path stand for.
async fn write(
    reads: &Reads,
    method: &Method,
    caller: &Caller,
    request: &ODataRequest,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let version = request.version;
    let ODataUri::Resource { path, options } = &request.uri else {
        return written(method, &request.uri, version);
    };
    // A history is read only, whatever the view offers (T-5908).
    if history_set(reads, path).is_some() {
        return refused(StatusCode::METHOD_NOT_ALLOWED, version).into_response();
    }
    let form = match (method, path.as_slice()) {
        (&Method::POST, [Segment::EntitySet(set)]) => Form::Create(set),
        (&Method::PATCH, [Segment::EntitySet(set), Segment::Key(key)]) => {
            Form::Change(set, key, WriteKind::Update)
        }
        (&Method::PUT, [Segment::EntitySet(set), Segment::Key(key)]) => {
            Form::Change(set, key, WriteKind::Replace)
        }
        (&Method::DELETE, [Segment::EntitySet(set), Segment::Key(key)]) => Form::Delete(set, key),
        (
            &Method::PUT | &Method::PATCH | &Method::DELETE,
            [Segment::EntitySet(set), Segment::Key(key), Segment::Property(name)],
        )
        | (
            &Method::DELETE,
            [Segment::EntitySet(set), Segment::Key(key), Segment::Property(name), Segment::Value],
        ) => Form::Attribute(set, key, name, method.clone()),
        (
            &Method::PUT | &Method::DELETE,
            [Segment::EntitySet(set), Segment::Key(key), Segment::Navigation(nav), Segment::Ref],
        ) => Form::Reference(set, key, nav, method == Method::DELETE),
        _ => return written(method, &request.uri, version),
    };
    let Some(op) = form.op(reads) else {
        // W19: no operation removes one object of a Relationship.
        return not_implemented("removing one object of a Relationship", version).into_response();
    };
    // Rule 4: a write the view does not offer is refused before its body
    // is read, whatever the body holds.
    if !reads.view.allows(op) || !form.taken(reads) {
        return refused(StatusCode::METHOD_NOT_ALLOWED, version).into_response();
    }
    // W32: the broker has no conditional write; options shape a read's
    // answer, and a write answers no representation but a create's.
    if headers.contains_key(IF_MATCH) || headers.contains_key(IF_NONE_MATCH) {
        return not_implemented("a conditional write (If-Match, If-None-Match)", version)
            .into_response();
    }
    if *options != QueryOptions::default() {
        return not_implemented("a query option on a write", version).into_response();
    }
    let level = match json_level(request, headers, reads.state.max_header_len) {
        Ok(level) => level,
        Err(refused) => return refused.into_response(),
    };
    let ctx = Ctx {
        service_root: &reads.state.service_root,
        level,
        version,
        metadata_etag: reads.state.metadata_etag.as_deref(),
    };
    let answer = match form {
        Form::Create(set) => create(reads, caller, &ctx, set, headers, body).await,
        Form::Change(set, key, kind) => {
            change(reads, caller, &ctx, (set, key), kind, headers, body).await
        }
        Form::Delete(set, key) => delete(reads, caller, set, key, version).await,
        Form::Attribute(set, key, name, method) => {
            below::property(reads, caller, &ctx, (set, key, name), &method, body).await
        }
        Form::Reference(set, key, nav, _) => {
            below::reference(reads, caller, &ctx, (set, key, nav), op, body).await
        }
    };
    answer.unwrap_or_else(IntoResponse::into_response)
}

/// The write a method and a path stand for.
enum Form<'u> {
    Create(&'u str),
    Change(&'u str, &'u Key, WriteKind),
    Delete(&'u str, &'u Key),
    /// A structural property: `PUT`, `PATCH` or `DELETE` (W10, W11, W13).
    Attribute(&'u str, &'u Key, &'u str, Method),
    /// A navigation's `$ref`: set (`false`) or removed (W16, W17, W20).
    Reference(&'u str, &'u Key, &'u str, bool),
}

impl Form<'_> {
    /// Whether the set's class takes this write (C10a): a create, a delete,
    /// or an update of the Entity, a property or a reference.
    fn taken(&self, reads: &Reads) -> bool {
        let (Self::Create(set)
        | Self::Change(set, ..)
        | Self::Delete(set, _)
        | Self::Attribute(set, ..)
        | Self::Reference(set, ..)) = self;
        // An unknown set is the write's own 404 (the path resolves against
        // the same model first, so it does not get here).
        reads.edm.sets.get(*set).is_none_or(|s| match self {
            Self::Create(_) => s.writable.insert,
            Self::Delete(..) => s.writable.delete,
            _ => s.writable.update,
        })
    }

    /// The operation the write is; `None` for a form none is (W19).
    fn op(&self, reads: &Reads) -> Option<OperationId> {
        Some(match self {
            Self::Create(_) => OperationId::CreateEntity,
            Self::Change(.., WriteKind::Replace) => OperationId::ReplaceEntity,
            Self::Change(..) => OperationId::MergeEntity,
            Self::Delete(..) => OperationId::DeleteEntity,
            Self::Attribute(.., method) if method == Method::PUT => OperationId::ReplaceAttribute,
            Self::Attribute(.., method) if method == Method::PATCH => {
                OperationId::PartialAttributeUpdate
            }
            Self::Attribute(..) => OperationId::DeleteAttribute,
            Self::Reference(set, _, nav, remove) => {
                let collection = reads
                    .edm
                    .sets
                    .get(*set)
                    .is_some_and(|s| crate::paths::nav_of(s, nav).2);
                match (collection, remove) {
                    (true, true) => return None,
                    (_, true) => OperationId::DeleteAttribute,
                    (_, false) => OperationId::AppendAttributes,
                }
            }
        })
    }
}
