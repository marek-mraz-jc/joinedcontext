// SPDX-License-Identifier: EUPL-1.2
//! Reads below one Entity (odata-linkml-mapping 5.1 rows Q31 to Q33): a
//! property and its raw `$value` through Retrieve Attribute Value, the
//! Entities a navigation names through Retrieve Entity or Query Entities,
//! and their references; each one gated operation of the view.

use crate::edm::{Member, Set, Shape};
use crate::filter::Translated;
use crate::read::{
    admitted, default_instance, encode, entity_value, key_segment, member_value, not_found,
    not_implemented, objects, query, resource, run, selected, set_of, Reads, MAX_PAGE,
};
use antares_ops::{core_name, Caller, OperationId};
use axum::http::{header::CONTENT_TYPE, StatusCode};
use axum::response::{IntoResponse, Response};
use odata_axum::{json_response, ODataErrorResponse};
use odata_json::{
    write_collection, write_entity, write_entity_reference, write_entity_references, Ctx,
    EntityValue, MetadataLevel, ODataVersion, Page,
};
use odata_url::{Key, QueryOptions};
use serde_json::{Map, Value};

/// `{Set}('{id}')/{prop}` and `.../{prop}/$value` (Q31): the Attribute
/// alone, `GET /entities/{id}/attrs/{iri}`. An absent or hidden Attribute
/// is 404; a base member (`id`, `_types`, `scope`, the system members)
/// is a later round's.
pub(crate) async fn property(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    at: (&str, &Key, &str),
    raw: bool,
    options: &QueryOptions,
) -> Result<Response, ODataErrorResponse> {
    let (set_name, key, name) = at;
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let member = set
        .members
        .get(name)
        .ok_or_else(|| not_implemented("a path to a system member", version))?;
    let id = admitted(reads, caller, set, key, options, version).await?;
    let target = format!(
        "/ngsi-ld/v1/entities/{}/attrs/{}",
        encode(&id),
        encode(&member.iri)
    );
    let none = Translated::default();
    let (instances, _) = run(
        reads,
        caller,
        OperationId::RetrieveAttributeValue,
        target,
        &none,
        version,
    )
    .await?;
    let instances = Value::Array(instances);
    let value = member_value(&member.shape, default_instance(Some(&instances)));
    if raw {
        let text = raw_text(value).ok_or_else(|| not_implemented(NON_PRIMITIVE, version))?;
        return Ok((StatusCode::OK, [(CONTENT_TYPE, "text/plain")], text).into_response());
    }
    let mut body = Map::new();
    if ctx.level != MetadataLevel::None {
        body.insert(
            control(version, "context"),
            Value::from(format!(
                "{}$metadata#{set_name}{}/{name}",
                ctx.service_root,
                key_segment(set, &id)
            )),
        );
        body.extend(
            ctx.metadata_etag
                .map(|tag| (control(version, "metadataEtag"), Value::from(tag))),
        );
    }
    body.insert("value".into(), value);
    Ok(json_response(
        version,
        ctx.level,
        Value::Object(body).to_string().into_bytes(),
    ))
}

/// `{Set}('{id}')/$ref` (Q33b): the Entity by Retrieve Entity, gated as
/// its read (Q30), and its reference; hidden or absent is 404.
pub(crate) async fn reference(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    (set_name, key): (&str, &Key),
    options: &QueryOptions,
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let id = admitted(reads, caller, set, key, options, version).await?;
    let target = format!(
        "/ngsi-ld/v1/entities/{}?type={}",
        encode(&id),
        encode(&set.type_iri)
    );
    let none = Translated::default();
    // Retrieve Entity answers 200 only with the Entity it was asked for.
    run(
        reads,
        caller,
        OperationId::RetrieveEntity,
        target,
        &none,
        version,
    )
    .await?;
    let mut body = Vec::new();
    // A Vec takes every byte: the write cannot fail.
    let _ = write_entity_reference(
        ctx,
        &format!("{set_name}{}", key_segment(set, &id)),
        &mut body,
    );
    Ok(json_response(version, ctx.level, body))
}

/// `{Set}('{id}')/{nav}` (Q32) and `.../{nav}/$ref` (Q33): the source's
/// Relationship by Retrieve Entity, then the targets, each read its own
/// gated operation, so a hidden target is absent from a collection and a
/// single one is 404; references list the Relationship's objects.
pub(crate) async fn related(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    at: (&str, &Key, &str),
    refs: bool,
    options: &QueryOptions,
) -> Result<Response, ODataErrorResponse> {
    let (set_name, key, nav) = at;
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let (iri, target, collection) = nav_of(set, nav);
    let target_set = set_of(reads, target, version)?;
    let id = admitted(reads, caller, set, key, options, version).await?;
    let none = Translated::default();
    // The whole source: with `attrs` naming only the Relationship, an
    // Entity without it would answer 404 like an absent one.
    let source = format!(
        "/ngsi-ld/v1/entities/{}?type={}",
        encode(&id),
        encode(&set.type_iri)
    );
    let (docs, _) = run(
        reads,
        caller,
        OperationId::RetrieveEntity,
        source,
        &none,
        version,
    )
    .await?;
    let doc = docs.first().ok_or_else(|| not_found(version))?;
    let mut urns = objects(default_instance(doc.get(core_name(iri))));
    if !collection {
        urns.truncate(1);
    }
    let mut body = Vec::new();
    if refs {
        let ids: Vec<String> = urns
            .iter()
            .map(|urn| format!("{target}{}", key_segment(target_set, urn)))
            .collect();
        // A Vec takes every byte: the writes cannot fail.
        if collection {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            let _ = write_entity_references(ctx, &ids, &Page::default(), &mut body);
        } else {
            let one = ids.first().ok_or_else(|| not_found(version))?;
            let _ = write_entity_reference(ctx, one, &mut body);
        }
        return Ok(json_response(version, ctx.level, body));
    }
    let select = options.select.as_ref();
    let picked = selected(target_set, select).map_err(|()| not_implemented(SELECT, version))?;
    let found = targets(
        reads,
        caller,
        (target, target_set),
        urns,
        collection,
        picked.as_deref(),
        version,
    )
    .await?;
    let names = picked.clone().unwrap_or_default();
    let resource = resource(reads, target, version)?;
    let resource = if names.is_empty() {
        resource
    } else {
        resource.select(&names)
    };
    if collection {
        let _ = write_collection(ctx, &resource, &found, &Page::default(), &mut body);
    } else {
        let one = found.first().ok_or_else(|| not_found(version))?;
        let _ = write_entity(ctx, &resource, one, &mut body);
    }
    Ok(json_response(version, ctx.level, body))
}

/// The targets of a navigation, read through one gated operation: a
/// single one by Retrieve Entity (hidden or absent: 404), many by Query
/// Entities with their `id` list (a hidden one is left out).
async fn targets(
    reads: &Reads,
    caller: &Caller,
    (name, set): (&str, &Set),
    urns: Vec<String>,
    collection: bool,
    picked: Option<&[&str]>,
    version: ODataVersion,
) -> Result<Vec<EntityValue>, ODataErrorResponse> {
    let none = Translated::default();
    let (op, read) = match urns.as_slice() {
        [] if collection => return Ok(Vec::new()),
        [] => return Err(not_found(version)),
        [urn] if !collection => (
            OperationId::RetrieveEntity,
            format!(
                "/ngsi-ld/v1/entities/{}?{}",
                encode(urn),
                query(set, picked, &[], &none)
            ),
        ),
        many if u64::try_from(many.len()).unwrap_or(u64::MAX) > MAX_PAGE => {
            return Err(ODataErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "BadRequest",
                "the navigation names more Entities than one answer holds: query the set with $filter",
            )
            .with_version(version))
        }
        _ => {
            let ids = Translated {
                ids: Some(urns),
                ..Translated::default()
            };
            (
                OperationId::QueryEntities,
                format!(
                    "/ngsi-ld/v1/entities?{}&limit={MAX_PAGE}",
                    query(set, picked, &[], &ids)
                ),
            )
        }
    };
    let (docs, _) = run(reads, caller, op, read, &none, version).await?;
    Ok(docs
        .iter()
        .map(|doc| entity_value(reads, name, set, doc, picked, &[], &none))
        .collect())
}

/// The Relationship IRI, target set and multiplicity of navigation `name`
/// of `set`. The parser resolves a navigation segment only to one, so
/// anything else names the empty target, which no set has (404).
pub(crate) fn nav_of<'s>(set: &'s Set, name: &str) -> (&'s str, &'s str, bool) {
    match set.members.get(name) {
        Some(Member {
            iri,
            shape: Shape::Nav { target, collection },
            ..
        }) => (iri, target, *collection),
        _ => ("", "", false),
    }
}

/// The raw `$value` of a primitive (URL 4.7): a string as it is, a number
/// or a Boolean as JSON writes it; `None` for anything else.
/// The constructs this module answers `501`.
const NON_PRIMITIVE: &str = "/$value of a non-primitive value";
const SELECT: &str = "$select of a navigation, a path or an annotation";

fn raw_text(value: Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

/// A control member, `@odata.{name}` in 4.0 and `@{name}` in 4.01.
fn control(version: ODataVersion, name: &str) -> String {
    match version {
        ODataVersion::V4_0 => format!("@odata.{name}"),
        ODataVersion::V4_01 => format!("@{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_navigation_names_a_target() {
        let member = |shape| Member {
            iri: "https://e.org/m".into(),
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
            references: std::collections::BTreeMap::new(),
            bound: std::collections::BTreeMap::new(),
            writable: crate::edm::Writable::default(),
            members: std::collections::BTreeMap::from([
                (
                    "nav".into(),
                    member(Shape::Nav {
                        target: "T".into(),
                        collection: true,
                    }),
                ),
                ("geo".into(), member(Shape::Geo)),
            ]),
        };
        assert_eq!(nav_of(&set, "nav"), ("https://e.org/m", "T", true));
        assert_eq!(nav_of(&set, "geo"), ("", "", false));
        assert_eq!(nav_of(&set, "none"), ("", "", false));
    }

    #[test]
    fn a_raw_value_is_a_primitive_as_text() {
        assert_eq!(raw_text(json!("a b")), Some("a b".into()));
        assert_eq!(raw_text(json!(1.5)), Some("1.5".into()));
        assert_eq!(raw_text(json!(false)), Some("false".into()));
        assert_eq!(raw_text(Value::Null), None);
        assert_eq!(raw_text(json!([1])), None);
    }
}
