//! `write_entities` (AG-78): the conversation prepares a change to entities and the person
//! applies it. The Portal reads the person's grants on the endpoint and the entities as they are,
//! and builds the preview the card shows; it writes nothing. Apply sends each update from the
//! browser with the person's own session, so the gateway's policy decides it (EP-55).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::agents::change::call_of;

/// Entities one preview may change.
pub const MAX_ENTITIES: usize = 50;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WriteEntities {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub entities: Vec<EntityChange>,
}

/// One entity and the attributes that change, with their new values.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EntityChange {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub attrs: Map<String, Value>,
}

/// The `write_entities` call in a model answer, when the answer is one.
pub fn tool_call(answer: &str) -> Option<Result<WriteEntities, String>> {
    call_of(answer, "write_entities")
}

/// The entity type an NGSI-LD URN names: `urn:ngsi-ld:{Type}:…`.
pub fn type_of(id: &str) -> Option<&str> {
    id.strip_prefix("urn:ngsi-ld:")?
        .split(':')
        .next()
        .filter(|entity_type| !entity_type.is_empty())
}

/// What is wrong with the call before anything is read.
pub fn checked(call: &WriteEntities) -> Result<(), String> {
    if call.entities.is_empty() {
        return Err("name at least one entity and the attributes that change".to_owned());
    }
    if call.entities.len() > MAX_ENTITIES {
        return Err(format!(
            "one change reaches at most {MAX_ENTITIES} entities; this one names {}",
            call.entities.len()
        ));
    }
    for change in &call.entities {
        if type_of(&change.id).is_none() {
            return Err(format!(
                "'{}' is not an entity id; an id is urn:ngsi-ld:{{Type}}:…",
                change.id
            ));
        }
        if change.attrs.is_empty() {
            return Err(format!("{} changes no attribute", change.id));
        }
        if let Some(identity) = change
            .attrs
            .keys()
            .find(|name| matches!(name.as_str(), "id" | "type" | "@context"))
        {
            return Err(format!(
                "{identity} is what the entity is, not an attribute that changes"
            ));
        }
    }
    Ok(())
}

/// Why the person's grants on the endpoint (the `describe_access` permissions document) do not
/// let them update `attributes` of `entity_type`; `None` when they do. A prohibition ends it,
/// whatever a permission says, as the gateway decides.
pub fn refusal(access: &Value, entity_type: &str, attributes: &[&str]) -> Option<String> {
    let entries = |key: &str| {
        access
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|entry| {
                matches!(entry.pointer("/resource/type").and_then(Value::as_str), Some(t) if t == entity_type || t == "*")
            })
            .filter(|entry| {
                entry["actions"]
                    .as_array()
                    .is_some_and(|actions| actions.iter().any(|a| a == "updateAttrs" || a == "*"))
            })
            .collect::<Vec<_>>()
    };
    let reaches = |entry: &Value, attribute: &str| match &entry["attributes"] {
        Value::Array(names) => names.iter().any(|name| name == attribute),
        _ => true,
    };
    for prohibition in entries("prohibitions") {
        if let Some(attribute) = attributes.iter().find(|a| reaches(prohibition, a)) {
            return Some(format!(
                "the endpoint's policy forbids the person to change {attribute} of {entity_type}"
            ));
        }
    }
    let grants = entries("permissions");
    if grants.is_empty() {
        return Some(format!(
            "the person's grants on this endpoint do not let them update {entity_type}"
        ));
    }
    let outside: Vec<&str> = attributes
        .iter()
        .copied()
        .filter(|attribute| !grants.iter().any(|grant| reaches(grant, attribute)))
        .collect();
    (!outside.is_empty()).then(|| {
        format!(
            "the person's grants on this endpoint do not let them update {} of {entity_type}",
            outside.join(", ")
        )
    })
}

/// An attribute's value as the entity holds it: a property's value, a relationship's object, or
/// the plain value of a simplified entity; `null` when the entity has no such attribute.
fn value_of(entity: &Value, attribute: &str) -> Value {
    match entity.get(attribute) {
        Some(Value::Object(held)) => held
            .get("value")
            .or_else(|| held.get("object"))
            .cloned()
            .unwrap_or_else(|| Value::Object(held.clone())),
        Some(value) => value.clone(),
        None => Value::Null,
    }
}

/// One entity of the preview: every attribute that changes, before and after.
pub fn previewed(change: &EntityChange, entity_type: &str, current: &Value) -> Value {
    let changes: Vec<Value> = change
        .attrs
        .iter()
        .map(|(attribute, after)| {
            json!({ "attribute": attribute, "before": value_of(current, attribute), "after": after })
        })
        .collect();
    json!({ "id": change.id, "type": entity_type, "changes": changes })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATION: &str = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001";

    fn call(entities: Value) -> WriteEntities {
        serde_json::from_value(json!({ "endpoint": "helsinki-bikes-ops", "entities": entities }))
            .expect("call")
    }

    #[test]
    fn the_call_is_read_from_the_fence_and_checked_before_anything_is_read() {
        let answer = format!("Setting it.\n```json\n{{\"tool\":\"write_entities\",\"endpoint\":\"helsinki-bikes-ops\",\"entities\":[{{\"id\":\"{STATION}\",\"attrs\":{{\"status\":\"outOfService\"}}}}]}}\n```");
        let parsed = tool_call(&answer).expect("a call").expect("parses");
        assert!(checked(&parsed).is_ok());
        assert_eq!(type_of(STATION), Some("BikeHireDockingStation"));

        assert!(checked(&call(json!([]))).is_err_and(|e| e.contains("at least one")));
        let many: Vec<Value> = (0..51)
            .map(|i| json!({ "id": format!("urn:ngsi-ld:T:hel.fi:helsinki:{i}"), "attrs": { "a": 1 } }))
            .collect();
        assert!(checked(&call(json!(many))).is_err_and(|e| e.contains("at most 50")));
        assert!(
            checked(&call(json!([{ "id": "station-001", "attrs": { "a": 1 } }])))
                .is_err_and(|e| e.contains("not an entity id"))
        );
        assert!(checked(&call(json!([{ "id": STATION, "attrs": {} }])))
            .is_err_and(|e| e.contains("changes no attribute")));
        assert!(checked(&call(
            json!([{ "id": STATION, "attrs": { "type": "Other" } }])
        ))
        .is_err_and(|e| e.contains("not an attribute")));
    }

    #[test]
    fn the_grants_decide_by_type_attribute_and_prohibition() {
        let access = json!({
            "permissions": [
                { "resource": { "type": "BikeHireDockingStation" }, "actions": ["queryEntity", "updateAttrs"], "attributes": ["status", "name"] },
                { "resource": { "type": "Road" }, "actions": ["queryEntity"], "attributes": "*" }
            ],
            "prohibitions": [
                { "resource": { "type": "BikeHireDockingStation" }, "actions": ["updateAttrs"], "attributes": ["name"] }
            ]
        });
        assert_eq!(
            refusal(&access, "BikeHireDockingStation", &["status"]),
            None
        );
        assert_eq!(
            refusal(&access, "BikeHireDockingStation", &["status", "name"]).as_deref(),
            Some(
                "the endpoint's policy forbids the person to change name of BikeHireDockingStation"
            )
        );
        assert_eq!(
            refusal(&access, "BikeHireDockingStation", &["capacity"]).as_deref(),
            Some("the person's grants on this endpoint do not let them update capacity of BikeHireDockingStation")
        );
        assert_eq!(
            refusal(&access, "Road", &["status"]).as_deref(),
            Some("the person's grants on this endpoint do not let them update Road")
        );
        assert!(
            refusal(&json!({}), "Road", &["status"]).is_some(),
            "no grant, no write"
        );
    }

    #[test]
    fn the_preview_holds_every_attribute_before_and_after() {
        let change: EntityChange = serde_json::from_value(
            json!({ "id": STATION, "attrs": { "status": "outOfService", "capacity": 20, "operator": "urn:x" } }),
        )
        .expect("change");
        let current = json!({
            "id": STATION,
            "type": "BikeHireDockingStation",
            "status": { "type": "Property", "value": "working" },
            "operator": { "type": "Relationship", "object": "urn:y" }
        });
        let preview = previewed(&change, "BikeHireDockingStation", &current);
        let changed = |name: &str| {
            preview["changes"]
                .as_array()
                .and_then(|changes| changes.iter().find(|c| c["attribute"] == name))
                .map(|c| (c["before"].clone(), c["after"].clone()))
        };
        assert_eq!(
            changed("status"),
            Some((json!("working"), json!("outOfService")))
        );
        assert_eq!(changed("operator"), Some((json!("urn:y"), json!("urn:x"))));
        assert_eq!(changed("capacity"), Some((Value::Null, json!(20))));
        assert_eq!(preview["type"], "BikeHireDockingStation");
    }
}
