//! Personal data in a connector's answers (T-5976, AG-158, DM-122): an AI client copies what a
//! connector answers into a third-party model, so the attributes a project's models mark
//! `personalData` leave every answer unless the connector's scope ticks them.
//!
//! The marks are the ones the publish form and the erasure read (Architecture/11 §9): a slot, a
//! class's `slot_usage` or `attributes` entry annotated `personalData: true`, or a whole class.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::api::catalogue_draft::{attributes, Withheld};
use crate::state::AppState;

/// Entity type -> the attributes its model marks personal.
pub type Marks = BTreeMap<String, BTreeSet<String>>;

/// The personal attributes of every class of one LinkML model.
pub fn marks(linkml: &str) -> Marks {
    let model = serde_norway::from_str::<serde_norway::Value>(linkml).unwrap_or_default();
    let classes: Vec<String> = model["classes"]
        .as_mapping()
        .into_iter()
        .flatten()
        .filter_map(|(name, _)| name.as_str().map(str::to_owned))
        .collect();
    let mut out = Marks::new();
    for attr in attributes(linkml, &classes, &[]) {
        if attr.withheld == Some(Withheld::Personal) {
            out.entry(attr.type_name).or_default().insert(attr.name);
        }
    }
    out
}

/// The marks of every DataModel of `project`. A model the Portal cannot read is an `Err`
/// naming it: without its marks nothing personal could be withheld, so the caller refuses.
// ponytail: reads each model from the forge per answer that carries entities; cache by etag
// when query_data makes such answers frequent (T-5978 measures).
pub async fn of_project(state: &AppState, project: &str) -> Result<Marks, String> {
    let mut names: Vec<String> = state
        .mirror
        .matching(|env| {
            env.kind == "DataModel" && env.metadata.namespace.as_deref() == Some(project)
        })
        .into_iter()
        .map(|env| env.metadata.name)
        .collect();
    names.sort();
    let mut out = Marks::new();
    for name in names {
        let linkml = crate::api::datamodels::read_source(state, project, &name)
            .await
            .map_err(|_| format!("the data model {name} of project {project} could not be read, so its personal attributes cannot be withheld; try again in a minute"))?;
        for (class, attrs) in marks(&linkml) {
            out.entry(class).or_default().extend(attrs);
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
fn is_entity(node: &serde_json::Map<String, Value>) -> bool {
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

/// The first attribute `marks` names that `texts` mention, whatever class holds it: a computed
/// value built on one tells it as an answer would, and a `type` or `q` can spell a class many ways.
pub fn mentioned<'a>(marks: &'a Marks, texts: &[&str]) -> Option<&'a str> {
    marks
        .values()
        .flatten()
        .find(|name| texts.iter().any(|text| text.contains(name.as_str())))
        .map(String::as_str)
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

/// The fields the project's DataSource `name` lists in `spec.personalFields` (AG-158). A source
/// the mirror lacks lists none.
pub fn source_fields(state: &AppState, project: &str, name: &str) -> BTreeSet<String> {
    state
        .mirror
        .get(project, "DataSource", name)
        .and_then(|source| source.spec.get("personalFields").cloned())
        .and_then(|fields| serde_json::from_value(fields).ok())
        .unwrap_or_default()
}

/// Removes `fields` of the DataSource `source` from a sample answer of
/// `jc_pipeline_sample_source` or `jc_pipeline_try_mapping`: from each record of `records` and
/// of each `steps[].records` that is no entity (an entity is judged by its model's marks), and
/// from the first input message, which leaves the answer whole when it is text. Returns each
/// field removed as `DataSource/{source}.{field}`; values are never returned.
pub fn withhold_source(answer: &mut Value, source: &str, fields: &BTreeSet<String>) -> Vec<String> {
    if fields.is_empty() {
        return Vec::new();
    }
    let mut removed = BTreeSet::new();
    let strip_record = |record: &mut Value, removed: &mut BTreeSet<String>| {
        if let Value::Object(node) = record {
            if !is_entity(node) {
                for field in fields {
                    if node.remove(field).is_some() {
                        removed.insert(field.clone());
                    }
                }
            }
        }
    };
    let strip_list = |list: Option<&mut Value>, removed: &mut BTreeSet<String>| {
        list.and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .for_each(|record| strip_record(record, removed));
    };
    strip_list(answer.get_mut("records"), &mut removed);
    for step in answer
        .get_mut("steps")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        strip_list(step.get_mut("records"), &mut removed);
    }
    if let Some(input) = answer.get_mut("input").and_then(Value::as_object_mut) {
        match input.get_mut("sample") {
            Some(Value::String(text)) => {
                // A text message (a CSV body) cannot lose one column: it leaves whole.
                removed.extend(fields.iter().filter(|f| text.contains(f.as_str())).cloned());
                input.remove("sample");
            }
            Some(sample) => strip_record(sample, &mut removed),
            None => {}
        }
    }
    let named: Vec<String> = removed
        .into_iter()
        .map(|field| format!("DataSource/{source}.{field}"))
        .collect();
    if !named.is_empty() {
        let mut all: BTreeSet<String> = answer["withheld"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        all.extend(named.iter().cloned());
        answer["withheld"] = serde_json::json!(all);
    }
    named
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mention_finds_a_marked_attribute_in_any_spelling_of_any_class() {
        let marks = Marks::from([("Report".to_owned(), BTreeSet::from(["email".to_owned()]))]);
        assert_eq!(mentioned(&marks, &["email"]), Some("email"));
        assert_eq!(mentioned(&marks, &["https://x.org/email"]), Some("email"));
        assert_eq!(mentioned(&marks, &["", "n>1;email~=\"a\""]), Some("email"));
        assert_eq!(mentioned(&marks, &["count", ""]), None);
    }
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

    fn data_model(name: &str, project: &str) -> crate::resource::ResourceEnvelope {
        crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "DataModel".to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, project),
            spec: json!({ "linkml": "model.linkml.yaml" }),
            status: None,
        }
    }

    #[tokio::test]
    async fn a_project_marks_only_its_own_models_and_refuses_a_model_it_cannot_read() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(data_model("residents", "other"));
        assert_eq!(of_project(&state, "helsinki").await, Ok(Marks::new()));

        // No forge to read the model from: without its marks nothing could be withheld.
        state.mirror.upsert(data_model("reports", "helsinki"));
        let err = of_project(&state, "helsinki")
            .await
            .expect_err("unreadable");
        assert!(
            err.contains("data model reports of project helsinki"),
            "{err}"
        );
    }

    #[test]
    fn marks_name_the_slot_the_usage_and_the_whole_class() {
        let marks = marks(MODEL);
        assert_eq!(
            marks["PotholeReport"],
            BTreeSet::from(["contactEmail".to_owned(), "reporterName".to_owned()])
        );
        assert_eq!(marks["Citizen"], BTreeSet::from(["phone".to_owned()]));
        assert!(super::marks("not: [yaml").is_empty());
    }

    #[test]
    fn personal_attributes_leave_every_entity_in_any_format_and_the_rest_stays() {
        let marks = marks(MODEL);
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
        let marks = marks(MODEL);
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

    #[test]
    fn listed_source_fields_leave_every_sample_record_and_are_named() {
        let fields = BTreeSet::from(["rider".to_owned(), "phone".to_owned()]);
        let mut answer = json!({
            "count": 2,
            "fields": ["rider", "station"],
            "records": [
                { "rider": "Jana Nováková", "station": "Námestie SNP" },
                { "station": "Hlavná stanica" },
                { "id": "urn:ngsi-ld:Trip:1", "type": "Trip", "rider": { "type": "Property", "value": "kept for the model" } }
            ],
            "steps": [{ "step": 0, "name": "mapping", "records": [{ "rider": "Peter", "station": "SNP" }] }],
            "input": { "events": 1, "bytes": 40, "sample": "rider,station\nJana,SNP" }
        });
        let named = withhold_source(&mut answer, "trips", &fields);
        assert_eq!(named, ["DataSource/trips.rider"]);
        assert_eq!(answer["withheld"], json!(["DataSource/trips.rider"]));
        let text = answer.to_string();
        for value in ["Jana", "Peter", "Nováková"] {
            assert!(!text.contains(value), "{value} left in {text}");
        }
        assert_eq!(answer["records"][0], json!({ "station": "Námestie SNP" }));
        assert!(answer["input"].get("sample").is_none());
        assert_eq!(answer["input"]["events"], 1);
        // An entity keeps it: the model's marks judge an entity.
        assert_eq!(answer["records"][2]["rider"]["value"], "kept for the model");

        let mut object_sample = json!({ "input": { "sample": { "rider": "Jana", "a": 1 } } });
        withhold_source(&mut object_sample, "trips", &fields);
        assert_eq!(object_sample["input"]["sample"], json!({ "a": 1 }));

        let plain = json!({ "records": [{ "station": "SNP" }] });
        let mut kept = plain.clone();
        assert!(withhold_source(&mut kept, "trips", &fields).is_empty());
        assert!(withhold_source(&mut kept, "trips", &BTreeSet::new()).is_empty());
        assert_eq!(kept, plain);
    }
}
