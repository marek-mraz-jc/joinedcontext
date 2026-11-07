//! The endpoint's schema as a form needs it (T-0594, AP-61): for every entity type an
//! application reads, the attributes the space's LinkML DataModel declares, each as a JSON
//! Schema property the kit's `<EntityForm>` can turn into an input. The subset is deliberate:
//! a type, an enum's values, a number's bounds, a string's pattern, and which slots are
//! required. It is inlined into the preview document and packed for the pass, so the model
//! and the form see the same attributes.

use serde_json::{json, Map, Value};

use crate::api::assistant::ref_name;
use crate::state::AppState;
use crate::store::ListOptions;

/// The JSON Schema type a LinkML range maps to, with a format for the temporal ones.
fn property_of(range: &str, enums: &Map<String, Value>) -> Value {
    if let Some(values) = enums
        .get(range)
        .and_then(|e| e.get("permissible_values"))
        .and_then(Value::as_object)
    {
        let names: Vec<&str> = values.keys().map(String::as_str).collect();
        return json!({ "enum": names });
    }
    match range {
        "integer" => json!({ "type": "integer" }),
        "float" | "double" | "decimal" => json!({ "type": "number" }),
        "boolean" => json!({ "type": "boolean" }),
        "date" => json!({ "type": "string", "format": "date" }),
        "datetime" => json!({ "type": "string", "format": "date-time" }),
        "uri" | "uriorcurie" => json!({ "type": "string", "format": "uri" }),
        _ => json!({ "type": "string" }),
    }
}

/// `{ "<Type>": { "properties": { "<attr>": {…} }, "required": [...] } }` for `types`, from an
/// inline LinkML schema; a type the model does not declare is absent, so the form falls back
/// to what the rows show.
pub fn field_schema(linkml: &str, types: &[String]) -> Value {
    let root: Value = serde_norway::from_str(linkml).unwrap_or(Value::Null);
    let empty = Map::new();
    let classes = root["classes"].as_object().unwrap_or(&empty);
    let slots = root["slots"].as_object().unwrap_or(&empty);
    let enums = root["enums"].as_object().unwrap_or(&empty);
    let mut out = Map::new();
    for entity_type in types {
        let Some(class) = classes.get(entity_type).and_then(Value::as_object) else {
            continue;
        };
        let mut properties = Map::new();
        let mut required = Vec::new();
        // The class's named slots, defined at the top level, then its inline attributes.
        let named = class
            .get("slots")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|name| slots.get(name).map(|def| (name, def)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let inline = class
            .get("attributes")
            .and_then(Value::as_object)
            .map(|attrs| {
                attrs
                    .iter()
                    .map(|(n, d)| (n.as_str(), d))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (name, def) in named.into_iter().chain(inline) {
            if name == "id" || name == "type" {
                continue;
            }
            let range = def["range"].as_str().unwrap_or("string");
            let mut property = property_of(range, enums);
            for (facet, key) in [
                ("minimum_value", "minimum"),
                ("maximum_value", "maximum"),
                ("pattern", "pattern"),
            ] {
                if let Some(value) = def.get(facet).filter(|v| !v.is_null()) {
                    property[key] = value.clone();
                }
            }
            if def["required"] == Value::Bool(true) {
                required.push(Value::String(name.to_owned()));
            }
            properties.insert(name.to_owned(), property);
        }
        out.insert(
            entity_type.clone(),
            json!({ "properties": properties, "required": required }),
        );
    }
    Value::Object(out)
}

/// The field schema of the space behind `slug` in `project`, for `types`; `None` when the
/// endpoint, its space or its DataModel cannot be found in the mirror, or the model's LinkML
/// source cannot be read from the forge (DM-56).
pub async fn for_endpoint(
    state: &AppState,
    project: &str,
    slug: &str,
    types: &[String],
) -> Option<Value> {
    let endpoint = state
        .mirror
        .list(project, "Endpoint", &ListOptions::default())
        .items
        .into_iter()
        .find(|env| env.spec["slug"].as_str() == Some(slug))?;
    of_endpoint(state, project, &endpoint.spec, types).await
}

/// The field schema of the space an endpoint's `spec` publishes, for `types`; `None` as for
/// [`for_endpoint`].
pub async fn of_endpoint(
    state: &AppState,
    project: &str,
    endpoint: &Value,
    types: &[String],
) -> Option<Value> {
    let space = ref_name(&endpoint["contextSpaceRef"])?;
    let space = state.mirror.get(project, "ContextSpace", &space)?;
    let (namespace, model) =
        crate::resource::model_reference(&space.spec["dataModelRef"], project)?;
    state.mirror.get(namespace, "DataModel", model)?;
    let linkml = crate::api::datamodels::read_source(state, namespace, model)
        .await
        .ok()?;
    Some(field_schema(&linkml, types))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = r#"
id: https://hel.fi/models/bikes
name: bikes
enums:
  StationStatus:
    permissible_values:
      working: {}
      closed: {}
classes:
  BikeHireDockingStation:
    slots: [id, name, availableBikeNumber, status, location]
    attributes:
      stewardNote:
        range: string
        pattern: "^[^<>]*$"
  Unused:
    slots: [name]
slots:
  id: {}
  name: { range: string, required: true }
  availableBikeNumber: { range: integer, minimum_value: 0, maximum_value: 500 }
  status: { range: StationStatus }
  location: { range: string }
"#;

    #[test]
    fn a_type_becomes_its_properties_with_bounds_enums_patterns_and_required() {
        let schema = field_schema(MODEL, &["BikeHireDockingStation".to_owned()]);
        let station = &schema["BikeHireDockingStation"];
        assert_eq!(
            station["properties"]["availableBikeNumber"]["type"],
            "integer"
        );
        assert_eq!(station["properties"]["availableBikeNumber"]["minimum"], 0);
        assert_eq!(station["properties"]["availableBikeNumber"]["maximum"], 500);
        // Alphabetical: the parsed map orders its keys, and a form lists the values as given.
        assert_eq!(
            station["properties"]["status"]["enum"],
            json!(["closed", "working"])
        );
        assert_eq!(station["properties"]["stewardNote"]["pattern"], "^[^<>]*$");
        assert_eq!(station["required"], json!(["name"]));
        assert!(
            station["properties"].get("id").is_none(),
            "id is never a field"
        );
        assert!(schema.get("Unused").is_none(), "only the types asked for");
    }

    #[test]
    fn what_the_model_does_not_declare_is_absent_and_bad_yaml_is_empty() {
        let schema = field_schema(MODEL, &["Ghost".to_owned()]);
        assert_eq!(schema, json!({}));
        assert_eq!(field_schema(": not yaml [", &["X".to_owned()]), json!({}));
    }
}
