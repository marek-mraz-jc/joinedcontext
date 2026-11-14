//! The attributes a LinkML model marks personal data (AG-158, DM-122, Architecture/11 §9): a
//! slot, a class's `slot_usage` or `attributes` entry annotated `personalData: true`, or a whole
//! class. The Portal's agent runs and the assistant's chat withhold them from every answer they
//! hand a model provider, read by the same rules here.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use serde_norway::Value as Yaml;

/// Entity type -> the attributes its model marks personal.
pub type Marks = BTreeMap<String, BTreeSet<String>>;

/// Whether a LinkML node is annotated `personalData: true`, as a bare value or an annotation's
/// `value`, a boolean or the string `true`.
pub fn marked(node: &Yaml) -> bool {
    let mark = &node["annotations"]["personalData"];
    let mark = if mark.get("value").is_some() {
        &mark["value"]
    } else {
        mark
    };
    mark.as_bool() == Some(true)
        || mark
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("true"))
}

/// The personal attributes of every class of one LinkML model. A model that is no YAML is an
/// `Err`: without its marks nothing personal could be withheld, so the caller refuses.
pub fn marks(linkml: &str) -> Result<Marks, String> {
    let model = serde_norway::from_str::<Yaml>(linkml)
        .map_err(|_| "the data model is no LinkML YAML".to_owned())?;
    let mut out = Marks::new();
    for (class, node) in model["classes"].as_mapping().into_iter().flatten() {
        let Some(class) = class.as_str() else {
            continue;
        };
        let listed = node["slots"]
            .as_sequence()
            .into_iter()
            .flatten()
            .filter_map(Yaml::as_str);
        let own = node["attributes"]
            .as_mapping()
            .into_iter()
            .flatten()
            .filter_map(|(k, _)| k.as_str());
        for slot in listed.chain(own) {
            // Identity and type are on every entity and served whatever a projection says.
            if matches!(slot, "id" | "type") {
                continue;
            }
            if marked(node)
                || marked(&model["slots"][slot])
                || marked(&node["slot_usage"][slot])
                || marked(&node["attributes"][slot])
            {
                out.entry(class.to_owned())
                    .or_default()
                    .insert(slot.to_owned());
            }
        }
    }
    Ok(out)
}

/// `Type`, `ex:Type` or `https://…/Type` as the model's class name.
fn short(term: &str) -> &str {
    let tail = term.rsplit(['/', '#']).next().unwrap_or(term);
    tail.rsplit(':').next().unwrap_or(tail)
}

fn types_of(entity: &serde_json::Map<String, Value>) -> Vec<&str> {
    match entity.get("type") {
        Some(Value::String(t)) => vec![short(t)],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).map(short).collect(),
        _ => Vec::new(),
    }
}

/// An NGSI-LD entity in any format: a string `id` and a `type`.
pub fn is_entity(node: &serde_json::Map<String, Value>) -> bool {
    node.get("id").is_some_and(Value::is_string) && !types_of(node).is_empty()
}

/// A GeoJSON Feature's `properties` (NGSI-LD GeoJSON, OGC Features): the entity's type and
/// attributes sit there, under a Feature whose own `type` is `Feature`.
fn feature_properties(
    node: &mut serde_json::Map<String, Value>,
) -> Option<&mut serde_json::Map<String, Value>> {
    if node.get("type").and_then(Value::as_str) != Some("Feature") {
        return None;
    }
    node.get_mut("properties").and_then(Value::as_object_mut)
}

/// Whether `value` holds an entity anywhere: only such an answer needs the models read.
pub fn carries_entities(value: &Value) -> bool {
    match value {
        Value::Object(node) => {
            is_entity(node)
                || node.get("type").and_then(Value::as_str) == Some("Feature")
                || node.values().any(carries_entities)
        }
        Value::Array(items) => items.iter().any(carries_entities),
        _ => false,
    }
}

/// Removes from every entity in `value` the attributes `marks` names for its types, and
/// returns each one removed as `Type.attribute`, sorted, once each. Values are never returned.
pub fn withhold(value: &mut Value, marks: &Marks) -> Vec<String> {
    let mut removed = BTreeSet::new();
    walk(value, marks, &mut removed);
    removed.into_iter().collect()
}

fn walk(value: &mut Value, marks: &Marks, removed: &mut BTreeSet<String>) {
    match value {
        Value::Object(node) => {
            if let Some(properties) = feature_properties(node) {
                strip(properties, marks, removed);
            }
            if is_entity(node) {
                strip(node, marks, removed);
            }
            node.values_mut().for_each(|v| walk(v, marks, removed));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| walk(v, marks, removed)),
        _ => {}
    }
}

/// Removes from one entity (or a Feature's `properties`) the attributes its types mark.
fn strip(node: &mut serde_json::Map<String, Value>, marks: &Marks, removed: &mut BTreeSet<String>) {
    let personal: Vec<(String, String)> = types_of(node)
        .into_iter()
        .filter_map(|t| marks.get(t).map(|attrs| (t, attrs)))
        .flat_map(|(t, attrs)| {
            node.keys()
                .filter(|key| attrs.contains(short(key)))
                .map(move |key| (t.to_owned(), key.clone()))
        })
        .collect();
    for (t, key) in personal {
        if node.remove(&key).is_some() {
            removed.insert(format!("{t}.{}", short(&key)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MODEL: &str = r#"
classes:
  PotholeReport:
    slots: [id, type, description, contactEmail, reporterName]
    slot_usage:
      reporterName:
        annotations: { personalData: { tag: personalData, value: "true" } }
  Citizen:
    annotations: { personalData: true }
    attributes:
      phone: {}
slots:
  contactEmail:
    annotations: { personalData: true }
"#;

    #[test]
    fn marks_name_the_slot_the_usage_and_the_whole_class() {
        let marks = marks(MODEL).expect("YAML");
        assert_eq!(
            marks["PotholeReport"],
            BTreeSet::from(["contactEmail".to_owned(), "reporterName".to_owned()])
        );
        assert_eq!(marks["Citizen"], BTreeSet::from(["phone".to_owned()]));
        assert!(super::marks("not: [yaml").is_err());
    }

    #[test]
    fn personal_attributes_leave_every_entity_in_any_format_and_the_rest_stays() {
        let marks = marks(MODEL).expect("YAML");
        let mut answer = json!({
            "entities": [
                {
                    "id": "urn:ngsi-ld:PotholeReport:1",
                    "type": "PotholeReport",
                    "description": { "type": "Property", "value": "Hlboká jama" },
                    "contactEmail": { "type": "Property", "value": "jana@example.org" },
                    "https://smartdatamodels.org/reporterName": "Jana"
                },
                { "id": "urn:ngsi-ld:Citizen:2", "type": ["ex:Citizen"], "phone": "+421900000000" },
                { "id": "urn:ngsi-ld:Other:3", "type": "Other", "contactEmail": "kept@example.org" }
            ],
            "contactEmail": "not an entity, kept"
        });
        let removed = withhold(&mut answer, &marks);
        assert_eq!(
            removed,
            vec![
                "Citizen.phone",
                "PotholeReport.contactEmail",
                "PotholeReport.reporterName"
            ]
        );
        let text = answer.to_string();
        assert!(!text.contains("jana@example.org"), "{text}");
        assert!(!text.contains("+421900000000"), "{text}");
        assert!(!text.contains("\"Jana\""), "{text}");
        assert!(text.contains("Hlboká jama"), "{text}");
        assert!(text.contains("kept@example.org"), "{text}");
        assert!(text.contains("not an entity, kept"), "{text}");
    }

    #[test]
    fn a_geojson_feature_loses_the_personal_attributes_under_its_properties() {
        let marks = marks(MODEL).expect("YAML");
        let mut answer = json!({ "type": "FeatureCollection", "features": [{
            "id": "urn:ngsi-ld:PotholeReport:1",
            "type": "Feature",
            "geometry": { "type": "Point", "coordinates": [19.15, 48.73] },
            "properties": {
                "type": "PotholeReport",
                "description": "Hlboká jama",
                "contactEmail": { "type": "Property", "value": "jana@example.org" }
            }
        }] });
        assert!(carries_entities(&answer));
        assert_eq!(
            withhold(&mut answer, &marks),
            vec!["PotholeReport.contactEmail"]
        );
        let text = answer.to_string();
        assert!(!text.contains("jana@example.org"), "{text}");
        assert!(text.contains("Hlboká jama"), "{text}");
    }

    #[test]
    fn only_an_answer_with_an_entity_needs_the_models() {
        assert!(carries_entities(
            &json!({"entity": {"id": "urn:x", "type": "T"}})
        ));
        assert!(!carries_entities(
            &json!({"items": [{"kind": "Endpoint", "name": "x"}]})
        ));
        assert!(!carries_entities(&json!({"id": "urn:x", "type": 3})));
    }
}
