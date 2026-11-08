//! LinkML → the schema a broker Tenant's views read (gen_broker_schema.py, T-3703): the merged
//! model on the subset Antares' `linkml-model` reads, every class listing all its slots and a
//! class's changes to a slot as its `slot_usage`.

use super::schema::{Obj, View, py_json, py_str};
use serde_json::{Map, Value, json};

/// The built-in types of the broker's subset (Antares `linkml-model`, `Builtin`).
const BUILTINS: [&str; 11] = [
    "string",
    "integer",
    "float",
    "double",
    "decimal",
    "boolean",
    "date",
    "datetime",
    "time",
    "uri",
    "uriorcurie",
];

/// The slot metaslots that are plain values, in the order they are written.
const SLOT_VALUES: [&str; 10] = [
    "slot_uri",
    "range",
    "required",
    "multivalued",
    "identifier",
    "inlined",
    "inverse",
    "description",
    "rank",
    "title",
];

/// An annotation value as the broker reads it: text, a boolean as `true` or `false`.
fn text(value: &Value) -> String {
    match value {
        Value::Bool(b) => if *b { "true" } else { "false" }.to_owned(),
        Value::String(s) => s.clone(),
        other => py_json(other),
    }
}

fn annotations(element: &Obj) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(entries) = element.get("annotations").and_then(Value::as_object) else {
        return out;
    };
    for (tag, entry) in entries {
        let value = entry.get("value").unwrap_or(&Value::Null);
        out.insert(tag.clone(), json!({"tag": tag, "value": text(value)}));
    }
    out
}

/// A class, an enum or the built-in a LinkML type specialises.
fn range(view: &View, name: &str, owner: &str) -> Result<String, String> {
    if view.is_class(name) || view.is_enum(name) {
        return Ok(name.to_owned());
    }
    let mut seen = std::collections::HashSet::new();
    let mut current = Some(name.to_owned());
    while let Some(c) = current.clone().filter(|c| !BUILTINS.contains(&c.as_str())) {
        let Some(declared) = view.type_def(&c) else {
            break;
        };
        if !seen.insert(c) {
            break;
        }
        current = declared
            .get("typeof")
            .filter(|v| !v.is_null())
            .map(py_str);
    }
    match current {
        Some(c) if BUILTINS.contains(&c.as_str()) => Ok(c),
        _ => Err(format!(
            "{owner}: range '{name}' is not a class, an enum or a type based on one of {}, which \
             is what a broker Tenant's schema can hold",
            BUILTINS.join(", ")
        )),
    }
}

fn slot(view: &View, name: &str, slot: &Obj, owner: &str) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    out.insert("name".to_owned(), Value::String(name.to_owned()));
    for field in SLOT_VALUES {
        let Some(value) = slot.get(field).filter(|v| !v.is_null()) else {
            continue;
        };
        let value = if field == "range" {
            Value::String(range(view, &py_str(value), owner)?)
        } else {
            match value {
                Value::Bool(_) => value.clone(),
                Value::Number(n) if n.is_i64() || n.is_u64() => value.clone(),
                other => Value::String(py_str(other)),
            }
        };
        out.insert(field.to_owned(), value);
    }
    if let Some(unit) = slot.get("unit").and_then(Value::as_object) {
        let mut present = Map::new();
        for key in ["ucum_code", "symbol"] {
            if let Some(v) = unit.get(key).filter(|v| super::schema::truthy(Some(v))) {
                present.insert(key.to_owned(), Value::String(py_str(v)));
            }
        }
        let mappings = super::schema::texts(unit.get("exact_mappings"));
        if !mappings.is_empty() {
            present.insert(
                "exact_mappings".to_owned(),
                Value::Array(mappings.into_iter().map(Value::String).collect()),
            );
        }
        if !present.is_empty() {
            out.insert("unit".to_owned(), Value::Object(present));
        }
    }
    let annotations = annotations(slot);
    if !annotations.is_empty() {
        out.insert("annotations".to_owned(), Value::Object(annotations));
    }
    Ok(out)
}

/// The metaslots `induced` states differently from `base`; empty when they agree.
fn usage(base: &Map<String, Value>, induced: &Map<String, Value>) -> Map<String, Value> {
    let mut diff: Map<String, Value> = induced
        .iter()
        .filter(|(k, v)| *k != "name" && *k != "annotations" && base.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let empty = Map::new();
    let base_annotations = base
        .get("annotations")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let changed: Map<String, Value> = induced
        .get("annotations")
        .and_then(Value::as_object)
        .unwrap_or(&empty)
        .iter()
        .filter(|(tag, a)| base_annotations.get(*tag) != Some(*a))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !changed.is_empty() {
        diff.insert("annotations".to_owned(), Value::Object(changed));
    }
    if diff.is_empty() {
        return diff;
    }
    let mut out = Map::new();
    out.insert(
        "name".to_owned(),
        induced.get("name").cloned().unwrap_or(Value::Null),
    );
    out.extend(diff);
    out
}

/// The broker schema of one merged model.
pub(super) fn broker_schema(view: &View) -> Result<Value, String> {
    let schema = &view.schema;
    let default_range = schema.default_range.as_deref().unwrap_or("string");

    let mut all = view.all_slots();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let mut slots = Map::new();
    for (name, declared) in all {
        let owner = format!("slot '{name}'");
        let mut entry = slot(view, &name, declared, &owner)?;
        if !entry.contains_key("range") {
            entry.insert(
                "range".to_owned(),
                Value::String(range(view, default_range, &owner)?),
            );
        }
        slots.insert(name, Value::Object(entry));
    }

    let mut class_names: Vec<&String> = schema.classes.keys().collect();
    class_names.sort();
    let mut classes = Map::new();
    for name in class_names {
        let Some(cls) = view.class(name) else {
            continue;
        };
        let names = view.class_slots(name)?;
        let mut slot_usage = Map::new();
        for slot_name in &names {
            let induced = view.induced_slot(slot_name, Some(name))?;
            let induced = slot(
                view,
                slot_name,
                &induced,
                &format!("class '{name}' slot '{slot_name}'"),
            )?;
            let base = slots
                .get(slot_name)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("'{slot_name}'"))?;
            let differs = usage(base, &induced);
            if !differs.is_empty() {
                slot_usage.insert(slot_name.clone(), Value::Object(differs));
            }
        }
        let mut entry = Map::new();
        entry.insert("name".to_owned(), Value::String(name.clone()));
        let class_uri = cls
            .get("class_uri")
            .filter(|v| super::schema::truthy(Some(v)))
            .map_or_else(|| name.clone(), py_str);
        entry.insert("class_uri".to_owned(), Value::String(class_uri));
        if let Some(is_a) = cls.get("is_a").filter(|v| super::schema::truthy(Some(v))) {
            entry.insert("is_a".to_owned(), Value::String(py_str(is_a)));
        }
        entry.insert(
            "slots".to_owned(),
            Value::Array(names.into_iter().map(Value::String).collect()),
        );
        if !slot_usage.is_empty() {
            entry.insert("slot_usage".to_owned(), Value::Object(slot_usage));
        }
        let annotations = annotations(cls);
        if !annotations.is_empty() {
            entry.insert("annotations".to_owned(), Value::Object(annotations));
        }
        if let Some(d) = cls.get("description").filter(|v| super::schema::truthy(Some(v))) {
            entry.insert("description".to_owned(), Value::String(py_str(d)));
        }
        classes.insert(name.clone(), Value::Object(entry));
    }

    let mut enum_list: Vec<(&String, &Obj)> = view.enums().collect();
    enum_list.sort_by(|a, b| a.0.cmp(b.0));
    let mut enums = Map::new();
    for (name, e) in enum_list {
        let mut values = Map::new();
        if let Some(pvs) = e.get("permissible_values").and_then(Value::as_object) {
            for (text, pv) in pvs {
                let empty = Map::new();
                let pv = pv.as_object().unwrap_or(&empty);
                let mut value = Map::new();
                value.insert("text".to_owned(), Value::String(text.clone()));
                for key in ["description", "meaning"] {
                    if let Some(v) = pv.get(key).filter(|v| super::schema::truthy(Some(v))) {
                        value.insert(key.to_owned(), Value::String(py_str(v)));
                    }
                }
                let annotations = annotations(pv);
                if !annotations.is_empty() {
                    value.insert("annotations".to_owned(), Value::Object(annotations));
                }
                values.insert(text.clone(), Value::Object(value));
            }
        }
        let mut entry = Map::new();
        entry.insert("name".to_owned(), Value::String(name.clone()));
        entry.insert("permissible_values".to_owned(), Value::Object(values));
        if let Some(d) = e.get("description").filter(|v| super::schema::truthy(Some(v))) {
            entry.insert("description".to_owned(), Value::String(py_str(d)));
        }
        enums.insert(name.clone(), Value::Object(entry));
    }

    let mut prefixes: Vec<&(String, String)> = schema.prefixes.iter().collect();
    prefixes.sort_by(|a, b| a.0.cmp(&b.0));
    let prefixes: Map<String, Value> = prefixes
        .into_iter()
        .map(|(k, r)| {
            (
                k.clone(),
                json!({"prefix_prefix": k, "prefix_reference": r}),
            )
        })
        .collect();
    let mut out = Map::new();
    out.insert("id".to_owned(), Value::String(schema.id.clone()));
    out.insert("name".to_owned(), Value::String(schema.name.clone()));
    out.insert("prefixes".to_owned(), Value::Object(prefixes));
    if let Some(v) = schema.version.as_ref().filter(|v| !v.is_empty()) {
        out.insert("version".to_owned(), Value::String(v.clone()));
    }
    if let Some(p) = schema.default_prefix.as_ref().filter(|v| !v.is_empty()) {
        out.insert("default_prefix".to_owned(), Value::String(p.clone()));
    }
    out.insert("classes".to_owned(), Value::Object(classes));
    out.insert("slots".to_owned(), Value::Object(slots));
    out.insert("enums".to_owned(), Value::Object(enums));
    Ok(Value::Object(out))
}
