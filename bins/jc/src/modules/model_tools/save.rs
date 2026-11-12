//! The artifacts the Portal's save commits beside a model's source (DM-02) besides the
//! `@context`: the JSON Schema, the Markdown page, one example entity (DM-21) and the
//! `jc-types.ts` of a generated application (SDK-10). Each is rendered from the broker schema as
//! Antares' `linkml-model` reads it, so the JSON Schema and the page are the ones the broker
//! itself publishes for the model (L3, L7); the example and the row types follow
//! gen_example.py and gen_typescript.py on that same model.

use super::schema::{View, camelcase, truthy};
use linkml_model::{Builtin, Model, Range, Slot};
use serde_json::{Map, Value, json};

/// How deep inline objects nest in an example before the model is called non-terminating.
const MAX_DEPTH: usize = 8;
/// Names `jc-types.ts` declares itself.
const GEOMETRY: &str = "Geometry";
const RESERVED: [&str; 2] = [GEOMETRY, "EntityTypeName"];

/// The JSON Schema (draft-07) with the generator version and each slot's unit (DM-06), which
/// the gateway's unit rules and the exports read as `x-unit`.
pub(super) fn json_schema(model: &Model, version: &str) -> Result<Value, String> {
    let mut schema: Value =
        serde_json::from_str(&linkml_model::json_schema(model)).map_err(|e| e.to_string())?;
    schema["x-generator-version"] = json!(version);
    for name in model.schema().classes.keys() {
        let Some(Value::Object(props)) =
            schema.pointer_mut(&format!("/definitions/{name}/properties"))
        else {
            continue;
        };
        for (slot_name, slot) in model.class_slots(name) {
            let (Some(unit), Some(Value::Object(prop))) = (&slot.unit, props.get_mut(slot_name))
            else {
                continue;
            };
            let mut x = Map::new();
            if let Some(code) = &unit.ucum_code {
                x.insert("ucumCode".to_owned(), json!(code));
            }
            if let Some(symbol) = &unit.symbol {
                x.insert("symbol".to_owned(), json!(symbol));
            }
            x.insert("exactMappings".to_owned(), json!(unit.exact_mappings));
            prop.insert("x-unit".to_owned(), Value::Object(x));
        }
    }
    Ok(schema)
}

/// The Markdown page of the whole model.
pub(super) fn docs(model: &Model) -> String {
    linkml_model::markdown(model)
}

/// Whether `class` specialises `Entity`, through `is_a`.
fn is_entity(model: &Model, class: &str) -> bool {
    let classes = &model.schema().classes;
    let mut at = classes.get(class).and_then(|c| c.is_a.as_deref());
    for _ in 0..classes.len() {
        match at {
            Some("Entity") => return true,
            Some(parent) => at = classes.get(parent).and_then(|c| c.is_a.as_deref()),
            None => return false,
        }
    }
    false
}

/// The classes an endpoint serves entities of: they specialise `Entity` and can have instances.
fn entity_classes<'m>(model: &'m Model, view: &View) -> Vec<&'m str> {
    model
        .schema()
        .classes
        .keys()
        .map(String::as_str)
        .filter(|name| *name != "Entity" && is_entity(model, name))
        .filter(|name| {
            view.class(name)
                .is_none_or(|c| !truthy(c.get("abstract")) && !truthy(c.get("mixin")))
        })
        .collect()
}

/// One example entity of the first entity class (DM-21), every member a term of `context`.
pub(super) fn example(model: &Model, view: &View, context: &Value) -> Result<Value, String> {
    let Some(class) = entity_classes(model, view).first().copied() else {
        return Err(
            "no class of this model specialises `Entity`, so there is no entity to make \
                    an example of; a model whose classes are all inline objects declares no \
                    NGSI-LD type (DM-09)"
                .to_owned(),
        );
    };
    let domain = domain(&model.schema().id);
    let mut out = Map::new();
    out.insert("id".to_owned(), json!(urn(class, &domain)));
    out.insert("type".to_owned(), json!(class));
    out.extend(object(model, class, &domain, 0)?);
    let terms = context.get("@context").and_then(Value::as_object);
    let unmapped: Vec<&str> = out
        .keys()
        .map(String::as_str)
        .filter(|k| !matches!(*k, "id" | "type") && terms.is_none_or(|t| !t.contains_key(*k)))
        .collect();
    if !unmapped.is_empty() {
        return Err(format!(
            "the generated example carries members the generated @context does not define, so \
             it would expand as opaque data (DM-21): {}",
            unmapped.join(", ")
        ));
    }
    Ok(Value::Object(out))
}

/// The organisation domain of a model: the host of its namespace IRI.
fn domain(id: &str) -> String {
    id.split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .filter(|host| !host.is_empty())
        .unwrap_or("example.invalid")
        .to_owned()
}

fn urn(entity_type: &str, domain: &str) -> String {
    format!("urn:ngsi-ld:{entity_type}:{domain}:example:1")
}

fn object(
    model: &Model,
    class: &str,
    domain: &str,
    depth: usize,
) -> Result<Map<String, Value>, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "`{class}` nests inline objects more than {MAX_DEPTH} deep, which a model that \
             terminates does not do"
        ));
    }
    let mut body = Map::new();
    for (name, slot) in model.class_slots(class) {
        if matches!(name, "id" | "type") {
            continue;
        }
        body.insert(name.to_owned(), value(model, &slot, domain, depth)?);
    }
    Ok(body)
}

fn value(model: &Model, slot: &Slot, domain: &str, depth: usize) -> Result<Value, String> {
    let kind = model.kind(slot);
    let one = match model.range(slot) {
        _ if kind == "GeoProperty" => json!({"type": "Point", "coordinates": [0.0, 0.0]}),
        Range::Class(target) if matches!(kind, "Relationship" | "ListRelationship") => {
            json!(urn(target, domain))
        }
        Range::Class(target) => Value::Object(object(model, target, domain, depth + 1)?),
        range => {
            let scalar = scalar(model, range, slot);
            if kind == "LanguageProperty" {
                json!({ "en": scalar })
            } else {
                scalar
            }
        }
    };
    let listed = matches!(kind, "ListProperty" | "ListRelationship");
    Ok(if slot.multivalued == Some(true) || listed {
        json!([one])
    } else {
        one
    })
}

fn scalar(model: &Model, range: Range<'_>, slot: &Slot) -> Value {
    match range {
        Range::Enum(name) => model
            .schema()
            .enums
            .get(name)
            .and_then(|e| e.permissible_values.keys().next())
            .map_or(Value::Null, |v| json!(v)),
        Range::Type(Builtin::Integer) => json!(0),
        Range::Type(Builtin::Float | Builtin::Double | Builtin::Decimal) => json!(0.0),
        Range::Type(Builtin::Boolean) => json!(true),
        Range::Type(Builtin::Date) => json!("2026-01-01"),
        Range::Type(Builtin::Datetime) => json!("2026-01-01T00:00:00Z"),
        Range::Type(Builtin::Time) => json!("00:00:00"),
        Range::Type(Builtin::Uri | Builtin::Uriorcurie) => json!("https://example.invalid/"),
        _ => json!(slot.name.as_deref().unwrap_or("text")),
    }
}

/// `jc-types.ts`: one row type per entity class in the App SDK's row shape (SDK-03, SDK-10).
pub(super) fn typescript(model: &Model, view: &View, version: &str) -> Result<String, String> {
    let schema = model.schema();
    let entities = entity_classes(model, view);
    for name in schema
        .enums
        .keys()
        .map(String::as_str)
        .chain(entities.iter().copied())
    {
        let declared = camelcase(name);
        if RESERVED.contains(&declared.as_str()) {
            return Err(format!(
                "'{declared}' is a name jc-types.ts declares itself; rename the class or enum"
            ));
        }
    }
    let mut lines = vec![
        format!(
            "// Rendered by Model Tools ({version}) from the endpoint's LinkML model. Do not edit."
        ),
        "// Row shapes of @joinedcontext/sdk (SDK-03): a GeoProperty is a GeoJSON geometry; a DateTime,".to_owned(),
        "// a LanguageProperty, a Relationship, a list and an inline object are strings.".to_owned(),
        String::new(),
        format!("export type {GEOMETRY} = {{ type: string; coordinates: unknown }};"),
    ];
    for (name, e) in &schema.enums {
        let values: Vec<String> = e
            .permissible_values
            .keys()
            .map(|v| json!(v).to_string())
            .collect();
        let values = if values.is_empty() {
            "never".to_owned()
        } else {
            values.join(" | ")
        };
        lines.push(String::new());
        lines.extend(doc(e.description.as_deref()));
        lines.push(format!("export type {} = {values};", camelcase(name)));
    }
    for class in &entities {
        lines.push(String::new());
        lines.extend(row_type(model, class));
    }
    let names: Vec<String> = entities.iter().map(|c| json!(c).to_string()).collect();
    let names = if names.is_empty() {
        "never".to_owned()
    } else {
        names.join(" | ")
    };
    lines.push(String::new());
    lines.push(format!("export type EntityTypeName = {names};"));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn row_type(model: &Model, class: &str) -> Vec<String> {
    let mut body = vec![
        "  id: string;".to_owned(),
        format!("  type: {};", json!(class)),
    ];
    for (name, slot) in model.class_slots(class) {
        if matches!(name, "id" | "type") {
            continue;
        }
        let symbol = slot
            .unit
            .as_ref()
            .and_then(|u| u.symbol.as_deref().or(u.ucum_code.as_deref()));
        let text = [
            slot.description.clone(),
            symbol.map(|s| format!("Unit: {s}.")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        body.extend(doc(Some(&text)).into_iter().map(|l| format!("  {l}")));
        let identifier = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        let prop = if identifier {
            name.to_owned()
        } else {
            json!(name).to_string()
        };
        let cell = cell(model, &slot);
        body.push(if slot.required == Some(true) {
            format!("  {prop}: {cell};")
        } else {
            format!("  {prop}?: {cell} | null;")
        });
    }
    let declared = model.schema().classes.get(class);
    let mut out: Vec<String> = doc(declared.and_then(|c| c.description.as_deref()));
    out.push(format!("export type {} = {{", camelcase(class)));
    out.extend(body);
    let open = declared
        .and_then(|c| c.annotations.get("open_world"))
        .is_some_and(|a| a.value.eq_ignore_ascii_case("true"));
    out.push(if open {
        format!("}} & {{ [attr: string]: string | number | boolean | {GEOMETRY} | null }};")
    } else {
        "};".to_owned()
    });
    out
}

/// The TypeScript type of one attribute's cell.
fn cell(model: &Model, slot: &Slot) -> String {
    let kind = model.kind(slot);
    if kind == "GeoProperty" {
        return GEOMETRY.to_owned();
    }
    if slot.multivalued == Some(true) || kind != "Property" {
        return "string".to_owned();
    }
    match model.range(slot) {
        Range::Enum(name) => camelcase(name),
        Range::Type(Builtin::Integer | Builtin::Float | Builtin::Double | Builtin::Decimal) => {
            "number".to_owned()
        }
        Range::Type(Builtin::Boolean) => "boolean".to_owned(),
        _ => "string".to_owned(),
    }
}

fn doc(text: Option<&str>) -> Vec<String> {
    let flat = text
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.is_empty() {
        return Vec::new();
    }
    vec![format!("/** {} */", flat.replace("*/", "*\\/"))]
}
