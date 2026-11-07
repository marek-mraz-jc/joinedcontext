// SPDX-License-Identifier: EUPL-1.2
//! Writes of one Entity (odata-linkml-mapping.md section 6): `POST {Set}`
//! as Create Entity with single-valued binds (W1, W2), `PATCH` and `PUT`
//! of an Entity as Merge and Replace Entity (W4, W6), `DELETE` as Delete
//! Entity (W9). Each is one operation of the view through `ViewOps::run`,
//! so the policy gate fires and the view's rule 7 holds in the handler; a
//! bind reads its target first with Retrieve Entity under the same view,
//! and a target the view hides answers like an absent one (D7, O8). The
//! body is written in the normalized form, so no kind is guessed from a
//! value. The other write forms answer `501` or `405` before any operation.

use crate::edm::{Member, Set, Shape};
use crate::filter::Translated;
use crate::read::{
    body_id, canonical_key, encode, entity, key_id, not_found, resource, run, run_request, set_of,
    Reads,
};
use antares_ops::{Caller, OperationId, OpsRequest};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use odata_axum::ODataErrorResponse;
use odata_json::{
    read_entity_body, Bind, BodyError, BodyLimits, Ctx, ODataError, ODataVersion, Target, WriteKind,
};
use odata_url::{Key, QueryOptions, Segment};
use serde_json::{json, Map, Value};

/// The NGSI-LD Null (8.4.2, 8.4.3): an update or a merge that names it
/// deletes the Attribute.
pub(crate) const NGSI_LD_NULL: &str = "urn:ngsi-ld:null";

/// One write request below the service root.
/// `POST {Set}` (W1, W2): Create Entity of the set's type with the body's
/// id, or a minted one when the body leaves out the key; `201` with the Entity as the view reads it and its `Location`, or
/// `204` under `Prefer: return=minimal`.
pub(crate) async fn create(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    set_name: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let resource = resource(reads, set_name, version)?;
    let payload = read_entity_body(
        ctx,
        &resource,
        WriteKind::Create,
        body,
        &BodyLimits::strict(),
    )
    .map_err(|e| body_error(&e, version))?;
    // The reader checks a key against its Edm type, and leaves one out
    // only where `$metadata` says the service computes it (W1).
    let (id, own) = match payload.key.first() {
        Some((_, value)) => (
            body_id(set, value).ok_or_else(|| body_error(&invalid_key(), version))?,
            value.clone(),
        ),
        None => minted(set, set_name),
    };
    let mut doc = attributes(set, &payload.properties, WriteKind::Create);
    // E2a: the identifier is an Attribute of its own too.
    let identifier = set
        .identifier
        .as_ref()
        .and_then(|i| set.members.get(&i.member));
    doc.extend(identifier.and_then(|m| Some((m.iri.clone(), attribute(m, &own)?))));
    referenced(reads, caller, set, &payload.properties, version).await?;
    bind(reads, caller, set, &payload.binds, &mut doc, version).await?;
    doc.insert("id".into(), Value::from(id.as_str()));
    doc.insert("type".into(), Value::from(set.type_iri.as_str()));
    send(
        reads,
        caller,
        OperationId::CreateEntity,
        Method::POST,
        "/ngsi-ld/v1/entities".into(),
        Some(doc),
        version,
    )
    .await?;
    let key = canonical_key(set, &id);
    let location = format!(
        "{}{set_name}{}",
        ctx.service_root,
        Segment::Key(key.clone())
    );
    let preferred = returned(headers);
    let minimal = preferred == Some(Return::Minimal);
    let mut answer = if minimal {
        no_content(version)
    } else {
        // The view reads it as any other Entity: what it hides stays out.
        let mut read = entity(
            reads,
            caller,
            ctx,
            set_name,
            &key,
            &QueryOptions::default(),
            headers,
        )
        .await?;
        *read.status_mut() = StatusCode::CREATED;
        read
    };
    // A key is a URL segment: always a header value.
    let location = HeaderValue::from_str(&location).unwrap_or(HeaderValue::from_static("/"));
    if minimal {
        answer
            .headers_mut()
            .insert("OData-EntityId", location.clone());
    }
    answer.headers_mut().insert(LOCATION, location);
    Ok(applied(answer, preferred))
}

/// The `return` preference of a write (Protocol 4.01 8.2.8.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Return {
    Minimal,
    Representation,
}

/// The `return` preference `Prefer` names, the first one of several.
fn returned(headers: &HeaderMap) -> Option<Return> {
    headers
        .get_all("Prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(|p| match p.trim().to_ascii_lowercase().as_str() {
            "return=minimal" => Some(Return::Minimal),
            "return=representation" => Some(Return::Representation),
            _ => None,
        })
}

/// `answer` with the `return` preference it honoured in `Preference-Applied` (Protocol 4.01 8.3.4).
fn applied(mut answer: Response, preferred: Option<Return>) -> Response {
    let value = match preferred {
        Some(Return::Minimal) => "return=minimal",
        Some(Return::Representation) => "return=representation",
        None => return answer,
    };
    answer
        .headers_mut()
        .insert("Preference-Applied", HeaderValue::from_static(value));
    answer
}

/// W1: the URN of a create that names no key and the key value it is
/// under: `urn:ngsi-ld:{Set}:{uuid}`, or the E2a identifier's prefix and
/// the uuid as its value.
fn minted(set: &Set, set_name: &str) -> (String, Value) {
    let uuid = uuid::Uuid::new_v4().to_string();
    let prefix = set
        .identifier
        .as_ref()
        .map_or_else(|| format!("urn:ngsi-ld:{set_name}:"), |i| i.prefix.clone());
    (format!("{prefix}{uuid}"), Value::from(uuid))
}

/// `PATCH` (W4) and `PUT` (W6) of an Entity: Merge or Replace Entity of
/// the set's type, `204`, or `200` with the Entity as the view reads it under
/// `Prefer: return=representation`. A key in the body must be the URL's; an Entity
/// that is absent, of another type or hidden is `404`, never created
/// (W7).
pub(crate) async fn change(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    (set_name, key): (&str, &Key),
    kind: WriteKind,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let id = id_of(set, key, version)?;
    let resource = resource(reads, set_name, version)?;
    let payload = read_entity_body(ctx, &resource, kind, body, &BodyLimits::strict())
        .map_err(|e| body_error(&e, version))?;
    if payload
        .key
        .iter()
        .any(|(_, value)| body_id(set, value).as_deref() != Some(id.as_str()))
    {
        return Err(body_error(&invalid_key(), version));
    }
    let mut doc = attributes(set, &payload.properties, kind);
    referenced(reads, caller, set, &payload.properties, version).await?;
    bind(reads, caller, set, &payload.binds, &mut doc, version).await?;
    if kind == WriteKind::Replace {
        named_instances_kept(reads, caller, set, &id, version).await?;
    }
    let (op, method) = if kind == WriteKind::Replace {
        doc.insert("id".into(), Value::from(id.as_str()));
        doc.insert("type".into(), Value::from(set.type_iri.as_str()));
        (OperationId::ReplaceEntity, Method::PUT)
    } else {
        (OperationId::MergeEntity, Method::PATCH)
    };
    send(
        reads,
        caller,
        op,
        method,
        typed(&format!("/ngsi-ld/v1/entities/{}", encode(&id)), set),
        Some(doc),
        version,
    )
    .await?;
    let preferred = returned(headers);
    let answer = if preferred == Some(Return::Representation) {
        // The view reads it as any other Entity: what it hides stays out.
        let key = canonical_key(set, &id);
        entity(
            reads,
            caller,
            ctx,
            set_name,
            &key,
            &QueryOptions::default(),
            headers,
        )
        .await?
    } else {
        no_content(version)
    };
    Ok(applied(answer, preferred))
}

/// A `PUT` replaces the whole Entity, and the surface serves only the default instance of an
/// Attribute (S3): an Entity that holds an instance with a `datasetId` is `409`, never replaced,
/// so no instance the caller cannot read here is deleted (T-5206 finding 8). The Entity is read
/// under the view first; an absent or hidden one is `404`, as the replace would answer.
async fn named_instances_kept(
    reads: &Reads,
    caller: &Caller,
    set: &Set,
    id: &str,
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    let target = typed(&format!("/ngsi-ld/v1/entities/{}", encode(id)), set);
    // an absent or hidden Entity is the `404` the replace would answer
    let (docs, _) = run(
        reads,
        caller,
        OperationId::RetrieveEntity,
        target,
        &Translated::default(),
        version,
    )
    .await?;
    let named = |v: &Value| v.get("datasetId").is_some();
    let held = docs
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|o| o.values())
        .any(|v| match v {
            Value::Array(instances) => instances.iter().any(named),
            one => named(one),
        });
    if held {
        return Err(ODataErrorResponse::new(
            StatusCode::CONFLICT,
            "Conflict",
            "the Entity holds Attribute instances with a datasetId, which a PUT would delete: change it with PATCH",
        )
        .with_version(version));
    }
    Ok(())
}

/// `DELETE {Set}('{id}')` (W9): Delete Entity of the set's type, `204`.
pub(crate) async fn delete(
    reads: &Reads,
    caller: &Caller,
    set_name: &str,
    key: &Key,
    version: ODataVersion,
) -> Result<Response, ODataErrorResponse> {
    let set = set_of(reads, set_name, version)?;
    let id = id_of(set, key, version)?;
    send(
        reads,
        caller,
        OperationId::DeleteEntity,
        Method::DELETE,
        typed(&format!("/ngsi-ld/v1/entities/{}", encode(&id)), set),
        None,
        version,
    )
    .await?;
    Ok(no_content(version))
}

/// The body's structural members as normalized NGSI-LD Attributes keyed by
/// IRI, `scope` as the Entity's. A `null` in a merge is the NGSI-LD Null
/// as the member's value (8.4.3: it deletes the Attribute, whatever its
/// kind); in a create or a replace it is an absent member. A member no
/// write may name ([`Shape::writable`]) was refused by the body reader
/// through the Capabilities that list it.
fn attributes(set: &Set, properties: &[(String, Value)], kind: WriteKind) -> Map<String, Value> {
    let mut doc = Map::new();
    for (name, value) in properties {
        let member = set.members.get(name);
        let iri = match (member, name.as_str()) {
            (Some(member), _) => member.iri.as_str(),
            (None, "scope") => "scope",
            (None, _) => continue,
        };
        let written = match (value, kind, member) {
            (Value::Null, WriteKind::Update, _) => Some(Value::from(NGSI_LD_NULL)),
            (Value::Null, ..) => None,
            (value, _, None) => Some(value.clone()),
            (value, _, Some(member)) => attribute(member, value),
        };
        doc.extend(written.map(|v| (iri.to_owned(), v)));
    }
    doc
}

/// One member's value as its normalized NGSI-LD Attribute, of the kind
/// the schema names; `None` for a member no write may name.
pub(crate) fn attribute(member: &Member, value: &Value) -> Option<Value> {
    Some(match (&member.shape, member.list) {
        (Shape::Value { .. }, true) => json!({"type": "ListProperty", "valueList": value}),
        (Shape::Geo, _) => json!({"type": "GeoProperty", "value": value}),
        (Shape::Json, _) if member.json => json!({"type": "JsonProperty", "json": value}),
        // An inlined object or an untyped value keeps the schema's kind (P2, T-5206).
        (Shape::Value { .. } | Shape::Json, _) => json!({"type": "Property", "value": value}),
        (Shape::Vocab, _) => json!({"type": "VocabProperty", "vocab": value}),
        (Shape::Language | Shape::Url { .. } | Shape::Nav { .. }, _) => return None,
    })
}

/// Each bind as a Relationship, after its target is read under the view
/// (W2): a target the view or the engine hides, an absent one and one of
/// another type are the same `400` naming the navigation, because the bind
/// is the request body's (Protocol 11.4.2.1, T-3760), and nothing is
/// written.
async fn bind(
    reads: &Reads,
    caller: &Caller,
    set: &Set,
    binds: &[Bind],
    doc: &mut Map<String, Value>,
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    for bind in binds {
        // The body reader resolved both against the same model.
        let member = set
            .members
            .get(&bind.navigation)
            .ok_or(not_found(version))?;
        let urn = visible(reads, caller, &bind.target, version)
            .await
            .map_err(|e| {
                unbound(
                    e,
                    &bind.navigation,
                    "the bound Entity does not exist",
                    version,
                )
            })?;
        doc.insert(
            member.iri.clone(),
            json!({"type": "Relationship", "object": urn}),
        );
        // R6: the Property the navigation's constraint names takes the target's key.
        let dependent = set
            .bound
            .get(&bind.navigation)
            .and_then(|p| set.members.get(p));
        let key = set_of(reads, &bind.target.set, version)?
            .identifier
            .as_ref()
            .and_then(|i| urn.strip_prefix(i.prefix.as_str()));
        if let Some((m, a)) = dependent
            .zip(key)
            .and_then(|(m, k)| Some((m, attribute(m, &Value::from(k))?)))
        {
            doc.insert(m.iri.clone(), a);
        }
    }
    Ok(())
}

/// Each R6 Property the body gives a value names an Entity of the
/// target's set, read under the view first (T-3761): absent, hidden and of
/// another type are the same `400` naming the Property, as for a bind, and
/// nothing is written. `null` names none.
pub(crate) async fn referenced(
    reads: &Reads,
    caller: &Caller,
    set: &Set,
    properties: &[(String, Value)],
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    for (name, value) in properties {
        let Some(target) = set.references.get(name).filter(|_| !value.is_null()) else {
            continue;
        };
        let target = set_of(reads, target, version)?;
        // The body reader checked the value against the key's Edm type.
        let urn = body_id(target, value).ok_or_else(|| body_error(&invalid_key(), version))?;
        run(
            reads,
            caller,
            OperationId::RetrieveEntity,
            typed(&format!("/ngsi-ld/v1/entities/{}", encode(&urn)), target),
            &Translated::default(),
            version,
        )
        .await
        .map_err(|e| unbound(e, name, "the referenced Entity does not exist", version))?;
    }
    Ok(())
}

/// A bind target or a referenced Entity that is not there, as the body
/// error it is, naming `target`; any other refusal stays as the operation
/// answered it.
fn unbound(
    e: ODataErrorResponse,
    target: &str,
    message: &'static str,
    version: ODataVersion,
) -> ODataErrorResponse {
    if e.status != StatusCode::NOT_FOUND {
        return e;
    }
    let mut bad = ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", message)
        .with_version(version);
    bad.error.target = Some(target.to_owned());
    bad
}

/// The id of a bound Entity, after Retrieve Entity of the target's set
/// reads it under the view: hidden, absent and of another type are the
/// same `404` (T-3441 section 6, O15).
pub(crate) async fn visible(
    reads: &Reads,
    caller: &Caller,
    target: &Target,
    version: ODataVersion,
) -> Result<String, ODataErrorResponse> {
    let set = set_of(reads, &target.set, version)?;
    let urn = id_of(set, &target.key, version)?;
    run(
        reads,
        caller,
        OperationId::RetrieveEntity,
        typed(&format!("/ngsi-ld/v1/entities/{}", encode(&urn)), set),
        &Translated::default(),
        version,
    )
    .await?;
    Ok(urn)
}

/// Runs one write operation; its status alone counts.
pub(crate) async fn send(
    reads: &Reads,
    caller: &Caller,
    op: OperationId,
    method: Method,
    target: String,
    doc: Option<Map<String, Value>>,
    version: ODataVersion,
) -> Result<(), ODataErrorResponse> {
    let mut req = OpsRequest::new(method, target);
    if let Some(doc) = doc {
        let body = Value::Object(doc).to_string();
        req = req
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header(CONTENT_LENGTH, HeaderValue::from(body.len()))
            .body(body);
    }
    run_request(reads, caller, op, req, version).await.map(drop)
}

/// `target` narrowed to the set's type: an Entity of another type is not
/// known to the operation (5.6.6.4), tested under the store's row lock.
pub(crate) fn typed(target: &str, set: &Set) -> String {
    format!("{target}?type={}", encode(&set.type_iri))
}

/// The id a key names in `set`; another key form names no Entity.
pub(crate) fn id_of(
    set: &Set,
    key: &Key,
    version: ODataVersion,
) -> Result<String, ODataErrorResponse> {
    key_id(set, key, &QueryOptions::default()).ok_or_else(|| not_found(version))
}

fn invalid_key() -> BodyError {
    BodyError::InvalidValue {
        property: "id".into(),
    }
}

pub(crate) fn body_error(error: &BodyError, version: ODataVersion) -> ODataErrorResponse {
    if let BodyError::NotImplemented(feature) = error {
        return crate::read::not_implemented(feature.as_str(), version);
    }
    ODataErrorResponse {
        status: StatusCode::from_u16(error.status()).unwrap_or(StatusCode::BAD_REQUEST),
        error: ODataError::from(error),
        version: Some(version),
    }
}

pub(crate) fn no_content(version: ODataVersion) -> Response {
    (
        StatusCode::NO_CONTENT,
        [("odata-version", HeaderValue::from_static(version.as_str()))],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edm::Member;
    use std::collections::BTreeMap;

    #[test]
    fn a_member_no_write_may_name_is_never_written() {
        let member = |iri: &str, shape| Member {
            iri: iri.into(),
            shape,
            list: false,
            json: false,
            untyped: false,
            written: crate::edm::Written::Always,
        };
        let set = Set {
            output: None,
            type_iri: "https://e.org/T".into(),
            identifier: None,
            references: BTreeMap::new(),
            bound: BTreeMap::new(),
            writable: crate::edm::Writable::default(),
            members: BTreeMap::from([
                (
                    "url".into(),
                    member("https://e.org/u", Shape::Url { collection: false }),
                ),
                ("lang".into(), member("https://e.org/l", Shape::Language)),
                (
                    "nav".into(),
                    member(
                        "https://e.org/n",
                        Shape::Nav {
                            target: "T".into(),
                            collection: false,
                        },
                    ),
                ),
            ]),
        };
        let named = |n: &str| (n.to_owned(), json!("x"));
        let doc = attributes(
            &set,
            &[named("url"), named("lang"), named("nav"), named("unknown")],
            WriteKind::Create,
        );
        assert!(doc.is_empty(), "{doc:?}");
    }
}
