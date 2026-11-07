// SPDX-License-Identifier: EUPL-1.2
//! Writes below one Entity (odata-linkml-mapping.md section 6), routed by
//! [`crate::write`]: a
//! structural property by `PUT` as Replace Attribute (W10), by `PATCH` as
//! Partial Attribute Update (W11), by `DELETE` as Delete Attribute (W13);
//! a navigation's `$ref` by `PUT` as Append Attributes, one object when
//! single-valued (W16) and the full list when collection-valued (W20), by
//! `DELETE` as Delete Attribute (W17). Each is one operation of
//! the view, narrowed to the set's type; every target a `$ref` names is
//! read under the view first, so a hidden one answers like an absent one
//! and nothing is written.

use crate::edm::Set;
use crate::paths::nav_of;
use crate::read::{encode, resource, set_of, Reads};
use crate::write::{
    attribute, body_error, id_of, no_content, referenced, send, typed, visible, NGSI_LD_NULL,
};
use antares_ops::{Caller, OperationId};
use axum::http::Method;
use axum::response::Response;
use odata_axum::ODataErrorResponse;
use odata_json::{read_property_body, read_reference_body, BodyError, BodyLimits, Ctx};
use odata_url::Key;
use serde_json::{json, Map, Value};

/// `PUT`, `PATCH` or `DELETE` of `{Set}('{id}')/{prop}` (W10, W11, W13),
/// `204`. A base member (`id`, `scope`, the computed ones) and a member no
/// write may name are `400` before any operation.
pub(crate) async fn property(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    (set_name, key, name): (&str, &Key, &str),
    method: &Method,
    body: &[u8],
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let read_only = BodyError::ReadOnly {
        property: name.to_owned(),
    };
    let member = set
        .members
        .get(name)
        .filter(|m| m.shape.writable())
        .ok_or(body_error(&read_only, version))?;
    let id = id_of(set, key, version)?;
    let target = attrs_of(&id, Some(&member.iri), set);
    if method == Method::DELETE {
        send(
            reads,
            caller,
            OperationId::DeleteAttribute,
            Method::DELETE,
            target,
            None,
            version,
        )
        .await?;
        return Ok(no_content(version));
    }
    let resource = resource(reads, set_name, version)?;
    let value = read_property_body(&resource, name, body, &BodyLimits::strict())
        .map_err(|e| body_error(&e, version))?;
    let named = [(name.to_owned(), value)];
    referenced(reads, caller, set, &named, version).await?;
    let [(_, value)] = named;
    // `null` sets the property to null: the NGSI-LD Null in its member.
    let value = if value.is_null() {
        Value::from(NGSI_LD_NULL)
    } else {
        value
    };
    let (op, method) = if method == Method::PUT {
        (OperationId::ReplaceAttribute, Method::PUT)
    } else {
        (OperationId::PartialAttributeUpdate, Method::PATCH)
    };
    // The filter above admits only members a write may name.
    let written = attribute_of(member, &value);
    send(reads, caller, op, method, target, Some(written), version).await?;
    Ok(no_content(version))
}

/// `PUT` or `DELETE` of `{Set}('{id}')/{nav}/$ref` (W16, W17, W20), `204`:
/// `op` is Append Attributes or Delete Attribute.
pub(crate) async fn reference(
    reads: &Reads,
    caller: &Caller,
    ctx: &Ctx<'_>,
    (set_name, key, nav): (&str, &Key, &str),
    op: OperationId,
    body: &[u8],
) -> Result<Response, ODataErrorResponse> {
    let version = ctx.version;
    let set = set_of(reads, set_name, version)?;
    let id = id_of(set, key, version)?;
    let (iri, _, _) = nav_of(set, nav);
    let list = set.members.get(nav).is_some_and(|m| m.list);
    if op == OperationId::DeleteAttribute {
        let target = attrs_of(&id, Some(iri), set);
        send(reads, caller, op, Method::DELETE, target, None, version).await?;
        return Ok(no_content(version));
    }
    let resource = resource(reads, set_name, version)?;
    let targets = read_reference_body(ctx, &resource, nav, body, &BodyLimits::strict())
        .map_err(|e| body_error(&e, version))?;
    let mut urns = Vec::with_capacity(targets.len());
    for target in &targets {
        urns.push(visible(reads, caller, target, version).await?);
    }
    // Append overwrites by default (10.2.4): it sets the Relationship,
    // or the full list, whether or not the Entity holds one.
    let relationship = match (nav_of(set, nav).2, urns.as_slice()) {
        (false, [one]) => json!({"type": "Relationship", "object": one}),
        _ => relationships(&urns, list),
    };
    let mut doc = Map::new();
    doc.insert(iri.to_owned(), relationship);
    let target = attrs_of(&id, None, set);
    send(reads, caller, op, Method::POST, target, Some(doc), version).await?;
    Ok(no_content(version))
}

/// The full list of a collection navigation: a `ListRelationship` keeps
/// its kind and order, any other a Relationship with every object.
fn relationships(urns: &[String], list: bool) -> Value {
    if list {
        let items: Vec<Value> = urns.iter().map(|u| json!({"object": u})).collect();
        json!({"type": "ListRelationship", "objectList": items})
    } else {
        json!({"type": "Relationship", "object": urns})
    }
}

/// One member as the Attribute object a `/attrs/{attr}` body is.
fn attribute_of(member: &crate::edm::Member, value: &Value) -> Map<String, Value> {
    attribute(member, value)
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// `/entities/{id}/attrs[/{iri}]?type=`.
fn attrs_of(id: &str, iri: Option<&str>, set: &Set) -> String {
    let attr = iri
        .map(|iri| format!("/{}", encode(iri)))
        .unwrap_or_default();
    typed(
        &format!("/ngsi-ld/v1/entities/{}/attrs{attr}", encode(id)),
        set,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_navigation_keeps_its_kind_and_order() {
        let urns = ["urn:b".to_owned(), "urn:a".to_owned()];
        assert_eq!(
            relationships(&urns, true),
            json!({"type": "ListRelationship", "objectList": [{"object": "urn:b"}, {"object": "urn:a"}]})
        );
        assert_eq!(
            relationships(&urns, false),
            json!({"type": "Relationship", "object": ["urn:b", "urn:a"]})
        );
    }
}
