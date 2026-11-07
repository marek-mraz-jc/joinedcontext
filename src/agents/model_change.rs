//! `change_resource` on a DataModel (AG-77, DM-13): the model editor's operations applied to the
//! model's LinkML source as data, so the platform checks what the editor will do before its page
//! opens. The page applies the same operations to the source's text, which keeps its comments and
//! order; the rules here are the editor's (`ui/src/pages/models/operations.ts`).

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// One operation of the model editor the conversation may send.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Operation {
    AddClass {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// The parent class; `Entity` makes it an NGSI-LD entity type (DM-09).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_a: Option<String>,
    },
    RemoveClass {
        name: String,
    },
    RenameClass {
        name: String,
        to: String,
    },
    AddSlot {
        name: String,
        /// The class the slot is attached to; a slot without one is declared and left loose.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#class: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
    RemoveSlot {
        name: String,
    },
    RenameSlot {
        name: String,
        to: String,
    },
    AttachSlot {
        r#class: String,
        slot: String,
    },
    DetachSlot {
        r#class: String,
        slot: String,
    },
    SetSlot {
        name: String,
        field: String,
        value: Value,
    },
    /// The class a class specialises, as LinkML `is_a`; an empty value removes it (DM-13).
    SetClassParent {
        name: String,
        parent: String,
    },
    /// An enum with no values yet, as `operations.ts` declares it; values follow one by one.
    AddEnum {
        name: String,
    },
    /// One permissible value of an enum the model declares.
    AddEnumValue {
        r#enum: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// The classes a class mixes in; an empty list removes the key.
    SetClassMixins {
        name: String,
        mixins: Vec<String>,
    },
    /// The profiles a slot belongs to; an empty list removes the key.
    SetSlotSubsets {
        name: String,
        subsets: Vec<String>,
    },
    /// A relationship, both ends in one step (DM-64): `name` on `from` points at `to`, `inverse`
    /// on `to` points back. The cardinality reads from `from` to `to`; `required` is allowed on
    /// the stored end only (DM-65), and `onDelete` is written on the source, `restrict` by default.
    AddRelationship {
        from: String,
        to: String,
        name: String,
        inverse: String,
        cardinality: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        required: Option<bool>,
        #[serde(
            default,
            rename = "inverseRequired",
            skip_serializing_if = "Option::is_none"
        )]
        inverse_required: Option<bool>,
        #[serde(default, rename = "onDelete", skip_serializing_if = "Option::is_none")]
        on_delete: Option<String>,
    },
    /// Both ends' `multivalued` together; `name` is either end.
    SetCardinality {
        name: String,
        cardinality: String,
    },
    /// The delete rule, on the source end; `name` is either end.
    SetOnDelete {
        name: String,
        #[serde(rename = "onDelete")]
        on_delete: String,
    },
    /// Both ends from every class and from the model; `name` is either end.
    RemoveRelationship {
        name: String,
    },
    /// The inverse a relationship saved before inverses were required lacks (DM-73): multivalued,
    /// on the target class, so the stored end stays the slot that holds the data today.
    AddInverse {
        name: String,
        inverse: String,
    },
}

/// LinkML element names: what the generators accept as a class or slot name.
static NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new("^[A-Za-z_][A-Za-z0-9_]*$").expect("a literal pattern"));

/// The ranges a slot takes besides the model's own classes and enums (`RANGES` in `linkml.ts`).
const RANGES: [&str; 10] = [
    "string",
    "integer",
    "float",
    "double",
    "decimal",
    "boolean",
    "date",
    "datetime",
    "uri",
    "uriorcurie",
];

/// The NGSI-LD kinds a slot declares (DM-05); a Property stays unwritten.
const KINDS: [&str; 7] = [
    "Property",
    "GeoProperty",
    "Relationship",
    "LanguageProperty",
    "ListProperty",
    "JsonProperty",
    "VocabProperty",
];

/// The cardinalities of a relationship, read from its source to its target (DM-64).
const CARDINALITIES: [&str; 4] = ["one-to-one", "one-to-many", "many-to-one", "many-to-many"];

/// What deleting a target does to the entities pointing at it (DM-66).
const ON_DELETE_RULES: [&str; 3] = ["restrict", "cascade", "set-null"];

/// The slot fields only the relationship operations change, so both ends change together.
const RELATIONSHIP_FIELDS: [&str; 2] = ["range", "multivalued"];

/// The slot fields a conversation sets; units, IRIs and bounds are set in the editor.
const SET_FIELDS: [&str; 4] = ["range", "required", "multivalued", "description"];

/// A new model as the editor starts one (`blankSource` in `ui/src/pages/models/linkml.ts`): the
/// organization's prefix, LinkML's types and `ngsi-ld-core`, so a class added `is_a: Entity` is an
/// NGSI-LD entity type (DM-09, DM-13).
pub fn blank(domain: &str, name: &str) -> Value {
    let prefix = domain
        .split('.')
        .next()
        .filter(|p| !p.is_empty())
        .unwrap_or("org");
    json!({
        "id": format!("https://{domain}/models/{name}"),
        "name": name,
        "prefixes": { (prefix): format!("https://{domain}/terms/"), "linkml": "https://w3id.org/linkml/" },
        "default_prefix": prefix,
        "imports": ["linkml:types", "ngsi-ld-core"],
        "classes": {},
        "slots": {},
        "enums": {},
    })
}

/// The model with the operations applied in order, all or nothing. A refusal names the operation
/// by its index, so the model corrects that one.
pub fn apply(model: &Value, operations: &[Operation]) -> Result<Value, String> {
    let mut changed = model.clone();
    for (index, operation) in operations.iter().enumerate() {
        mutate(&mut changed, operation).map_err(|reason| format!("operation {index}: {reason}"))?;
    }
    Ok(changed)
}

/// The model's classes with their slots, one line each, for the model to name real ones.
pub fn outline(model: &Value) -> String {
    let lines: Vec<String> = section(model, "classes")
        .map(|classes| {
            classes
                .iter()
                .map(|(name, class)| format!("{name}: {}", slots_of(class).join(", ")))
                .collect()
        })
        .unwrap_or_default();
    if lines.is_empty() {
        "the model has no class".to_owned()
    } else {
        lines.join("; ")
    }
}

fn section<'a>(model: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    model.get(key).and_then(Value::as_object)
}

/// The model's own prefix for a term added without an IRI (DM-04, T-0893): the prefix named
/// after the model, `{name}: {id}/`, declared on first use; a model without a name or an id
/// has none, and the term is left for the check to name.
fn own_prefix(model: &mut Value) -> Option<String> {
    let name = model.get("name")?.as_str()?.trim().to_owned();
    let id = model
        .get("id")?
        .as_str()?
        .trim()
        .trim_end_matches('/')
        .to_owned();
    if name.is_empty() || id.is_empty() {
        return None;
    }
    let prefixes = section_mut(model, "prefixes");
    if !prefixes.contains_key(&name) {
        prefixes.insert(name.clone(), json!(format!("{id}/")));
    }
    Some(name)
}

fn section_mut<'a>(model: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !model.is_object() {
        *model = json!({});
    }
    let entry = model
        .as_object_mut()
        .expect("an object")
        .entry(key)
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().expect("an object")
}

fn has(model: &Value, key: &str, name: &str) -> bool {
    section(model, key).is_some_and(|entries| entries.contains_key(name))
}

fn slots_of(class: &Value) -> Vec<String> {
    class
        .get("slots")
        .and_then(Value::as_array)
        .map(|slots| {
            slots
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn set_slots(class: &mut Value, slots: Vec<String>) {
    if let Some(fields) = class.as_object_mut() {
        fields.insert("slots".to_owned(), json!(slots));
    }
}

fn valid_name(name: &str, what: &str) -> Result<(), String> {
    if NAME.is_match(name) {
        Ok(())
    } else {
        Err(format!("'{name}' is not a valid {what} name"))
    }
}

fn existing(model: &Value, key: &str, name: &str, what: &str) -> Result<(), String> {
    if has(model, key, name) {
        Ok(())
    } else {
        Err(format!("unknown {what} '{name}'"))
    }
}

fn new_name(model: &Value, key: &str, name: &str, what: &str) -> Result<(), String> {
    valid_name(name, what)?;
    if has(model, key, name) {
        Err(format!("{what} '{name}' already exists"))
    } else {
        Ok(())
    }
}

fn mutate(model: &mut Value, operation: &Operation) -> Result<(), String> {
    match operation {
        Operation::AddClass {
            name,
            class_uri,
            description,
            is_a,
        } => {
            new_name(model, "classes", name, "class")?;
            let mut class = Map::new();
            let minted = class_uri
                .clone()
                .or_else(|| own_prefix(model).map(|prefix| format!("{prefix}:{name}")));
            for (field, value) in [
                ("class_uri", &minted),
                ("description", description),
                ("is_a", is_a),
            ] {
                if let Some(value) = value {
                    class.insert(field.to_owned(), json!(value));
                }
            }
            class.insert("slots".to_owned(), json!([]));
            section_mut(model, "classes").insert(name.clone(), Value::Object(class));
        }
        Operation::RemoveClass { name } => {
            existing(model, "classes", name, "class")?;
            section_mut(model, "classes").remove(name);
        }
        Operation::RenameClass { .. } => rename_class(model, operation)?,
        Operation::AddSlot { .. } => add_slot(model, operation)?,
        Operation::RemoveSlot { name } => {
            existing(model, "slots", name, "slot")?;
            section_mut(model, "slots").remove(name);
            for class in section_mut(model, "classes").values_mut() {
                let slots = slots_of(class);
                if slots.contains(name) {
                    set_slots(class, slots.into_iter().filter(|s| s != name).collect());
                }
            }
        }
        Operation::RenameSlot { name, to } => {
            existing(model, "slots", name, "slot")?;
            new_name(model, "slots", to, "slot")?;
            let slots = section_mut(model, "slots");
            let slot = slots.remove(name).unwrap_or(Value::Null);
            slots.insert(to.clone(), slot);
            for class in section_mut(model, "classes").values_mut() {
                let slots = slots_of(class);
                if slots.contains(name) {
                    let renamed = slots
                        .into_iter()
                        .map(|s| if &s == name { to.clone() } else { s })
                        .collect();
                    set_slots(class, renamed);
                }
            }
        }
        Operation::AttachSlot { r#class, slot } => {
            existing(model, "classes", r#class, "class")?;
            existing(model, "slots", slot, "slot")?;
            let owner = section_mut(model, "classes")
                .get_mut(r#class)
                .expect("checked above");
            let mut slots = slots_of(owner);
            if !slots.contains(slot) {
                slots.push(slot.clone());
                set_slots(owner, slots);
            }
        }
        Operation::DetachSlot { r#class, slot } => {
            existing(model, "classes", r#class, "class")?;
            let owner = section_mut(model, "classes")
                .get_mut(r#class)
                .expect("checked above");
            let slots = slots_of(owner);
            if !slots.contains(slot) {
                return Err(format!("class '{class}' does not use slot '{slot}'"));
            }
            set_slots(owner, slots.into_iter().filter(|s| s != slot).collect());
        }
        Operation::SetClassParent { name, parent } => {
            existing(model, "classes", name, "class")?;
            let parent = parent.trim();
            if !parent.is_empty() {
                if parent == name {
                    return Err(format!("class '{name}' cannot specialise itself"));
                }
                existing(model, "classes", parent, "class")?;
            }
            set_class_field(
                model,
                name,
                "is_a",
                (!parent.is_empty()).then(|| json!(parent)),
            );
        }
        Operation::SetClassMixins { name, mixins } => {
            existing(model, "classes", name, "class")?;
            let named = named_list(mixins);
            for mixin in &named {
                if mixin == name {
                    return Err(format!("class '{name}' cannot mix itself in"));
                }
                existing(model, "classes", mixin, "class")?;
            }
            set_class_field(
                model,
                name,
                "mixins",
                (!named.is_empty()).then(|| json!(named)),
            );
        }
        Operation::AddEnum { .. } | Operation::AddEnumValue { .. } => add_enum(model, operation)?,
        Operation::SetSlotSubsets { name, subsets } => {
            existing(model, "slots", name, "slot")?;
            let named = named_list(subsets);
            for subset in &named {
                if !NAME.is_match(subset) {
                    return Err(format!("'{subset}' is not a valid subset name"));
                }
            }
            let slot = section_mut(model, "slots")
                .get_mut(name)
                .expect("checked above");
            if !slot.is_object() {
                *slot = json!({});
            }
            let fields = slot.as_object_mut().expect("an object");
            match named.is_empty() {
                true => fields.remove("subsets"),
                false => fields.insert("subsets".to_owned(), json!(named)),
            };
        }
        Operation::SetSlot { name, field, value } => {
            existing(model, "slots", name, "slot")?;
            refuse_one_sided_edit(model, name, field, value)?;
            let value = set_value(model, name, field, value)?;
            let slot = section_mut(model, "slots")
                .get_mut(name)
                .expect("checked above");
            if !slot.is_object() {
                *slot = json!({});
            }
            let fields = slot.as_object_mut().expect("an object");
            match value {
                Some(value) => fields.insert(field.clone(), value),
                None => fields.remove(field),
            };
        }
        Operation::AddRelationship { .. } => add_relationship(model, operation)?,
        Operation::SetCardinality { name, cardinality } => {
            let (source_many, target_many) = flags_of(cardinality)?;
            let pair = relationship_named(model, name)?;
            let computed = if stored_on_source(cardinality) {
                &pair.target
            } else {
                &pair.source
            };
            if slot_flag(model, computed, "required") {
                return Err(format!("as {cardinality}, {computed} would be computed on read, and it is required; clear required on {computed} first (DM-65)"));
            }
            for (slot, many) in [(&pair.source, source_many), (&pair.target, target_many)] {
                set_slot_field(model, slot, "multivalued", many.then_some(json!(true)));
            }
        }
        Operation::SetOnDelete { name, on_delete } => {
            delete_rule(on_delete)?;
            let pair = relationship_named(model, name)?;
            let slot = slot_object(model, &pair.source);
            let annotations = slot.entry("annotations").or_insert_with(|| json!({}));
            if !annotations.is_object() {
                *annotations = json!({});
            }
            annotations["on_delete"] = json!(on_delete);
        }
        Operation::RemoveRelationship { name } => {
            let pair = relationship_named(model, name)?;
            for end in [&pair.source, &pair.target] {
                section_mut(model, "slots").remove(end);
                for class in section_mut(model, "classes").values_mut() {
                    let slots = slots_of(class);
                    if slots.contains(end) {
                        set_slots(class, slots.into_iter().filter(|s| s != end).collect());
                    }
                }
            }
        }
        Operation::AddInverse { .. } => add_inverse(model, operation)?,
    }
    Ok(())
}

/// Adds a slot, minted under the model's prefix unless it names its own URI, to its class if any.
fn add_slot(model: &mut Value, operation: &Operation) -> Result<(), String> {
    let Operation::AddSlot {
        name,
        r#class,
        range,
        slot_uri,
        kind,
    } = operation
    else {
        return Err("not add_slot".to_owned());
    };
    new_name(model, "slots", name, "slot")?;
    if let Some(owner) = r#class {
        existing(model, "classes", owner, "class")?;
    }
    let mut slot = json!({ "range": range.as_deref().unwrap_or("string") });
    if let Some(uri) = slot_uri {
        slot["slot_uri"] = json!(uri.trim());
    } else if let Some(prefix) = own_prefix(model) {
        slot["slot_uri"] = json!(format!("{prefix}:{name}"));
    }
    if let Some(kind) = kind.as_deref().map(str::trim) {
        if !KINDS.contains(&kind) {
            return Err(format!(
                "kind '{kind}' of slot '{name}' is not one of {}",
                KINDS.join(", ")
            ));
        }
        if kind != "Property" {
            slot["annotations"] = json!({ "ngsi_ld_kind": kind });
        }
    }
    section_mut(model, "slots").insert(name.clone(), slot);
    if let Some(owner) = r#class {
        let class = section_mut(model, "classes")
            .get_mut(owner)
            .expect("checked above");
        let mut slots = slots_of(class);
        slots.push(name.clone());
        set_slots(class, slots);
    }
    Ok(())
}

/// Declares an enum, or adds one permissible value to an enum the model declares.
fn add_enum(model: &mut Value, operation: &Operation) -> Result<(), String> {
    let (name, value, description) = match operation {
        Operation::AddEnum { name } => {
            new_name(model, "enums", name, "enum")?;
            section_mut(model, "enums").insert(name.clone(), json!({ "permissible_values": {} }));
            return Ok(());
        }
        Operation::AddEnumValue {
            r#enum,
            value,
            description,
        } => (r#enum, value.trim(), description.as_deref().map(str::trim)),
        _ => return Err("not add_enum".to_owned()),
    };
    if value.is_empty() {
        return Err(format!("a value of enum '{name}' needs a text"));
    }
    let entry = section_mut(model, "enums")
        .get_mut(name)
        .ok_or_else(|| format!("unknown enum '{name}'"))?;
    if !entry.is_object() {
        *entry = json!({});
    }
    if !entry
        .get("permissible_values")
        .is_some_and(Value::is_object)
    {
        entry["permissible_values"] = json!({});
    }
    let values = entry["permissible_values"]
        .as_object_mut()
        .expect("an object");
    if values.contains_key(value) {
        return Err(format!("enum '{name}' already has the value '{value}'"));
    }
    let fields = match description.filter(|d| !d.is_empty()) {
        Some(text) => json!({ "description": text }),
        None => json!({}),
    };
    values.insert(value.to_owned(), fields);
    Ok(())
}

/// Adds both ends of a relationship, each a slot on its owner, after the rules of DM-64 and DM-65.
fn add_relationship(model: &mut Value, operation: &Operation) -> Result<(), String> {
    let Operation::AddRelationship {
        from,
        to,
        name,
        inverse,
        cardinality,
        required,
        inverse_required,
        on_delete,
    } = operation
    else {
        return Err("not add_relationship".to_owned());
    };
    let (source_many, target_many) = flags_of(cardinality)?;
    let on_delete = on_delete.as_deref().unwrap_or("restrict");
    delete_rule(on_delete)?;
    existing(model, "classes", from, "class")?;
    if !has(model, "classes", to) {
        return Err(format!("unknown class '{to}': a relationship's inverse is written on its target, so the target is a class of this model"));
    }
    if inverse.trim().is_empty() {
        return Err(format!(
            "the relationship '{name}' needs an inverse: the slot on {to} that points back (DM-64)"
        ));
    }
    if inverse == name {
        return Err(format!(
            "'{name}' cannot be its own inverse: the two ends are two slots"
        ));
    }
    let stored_on_source = stored_on_source(cardinality);
    if (stored_on_source && *inverse_required == Some(true))
        || (!stored_on_source && *required == Some(true))
    {
        let computed = if stored_on_source { inverse } else { name };
        return Err(format!("as {cardinality}, {computed} is computed on read and cannot be required; require the other end (DM-65)"));
    }
    add_end(
        model,
        End {
            name,
            owner: from,
            range: to,
            inverse,
            multivalued: source_many,
            required: *required == Some(true),
            on_delete: Some(on_delete),
        },
    )?;
    add_end(
        model,
        End {
            name: inverse,
            owner: to,
            range: from,
            inverse: name,
            multivalued: target_many,
            required: *inverse_required == Some(true),
            on_delete: None,
        },
    )?;
    Ok(())
}

/// Gives an existing Relationship slot its inverse on the target class (DM-73).
fn add_inverse(model: &mut Value, operation: &Operation) -> Result<(), String> {
    let Operation::AddInverse { name, inverse } = operation else {
        return Err("not add_inverse".to_owned());
    };
    existing(model, "slots", name, "slot")?;
    let slot = &model["slots"][name];
    let range = slot.get("range").and_then(Value::as_str).unwrap_or("");
    if kind_of(slot) != Some("Relationship") || !has(model, "classes", range) {
        return Err(format!(
            "slot '{name}' is not a Relationship pointing at a class of this model"
        ));
    }
    if let Some(named) = slot.get("inverse").and_then(Value::as_str) {
        return Err(format!("slot '{name}' already names the inverse '{named}'"));
    }
    let range = range.to_owned();
    let owners = owners_of(model, name);
    let [owner] = owners.as_slice() else {
        return Err(format!(
            "slot '{name}' is used by {} classes; a relationship starts on one",
            owners.len()
        ));
    };
    let owner = owner.clone();
    add_end(
        model,
        End {
            name: inverse,
            owner: &range,
            range: &owner,
            inverse: name,
            multivalued: true,
            required: false,
            on_delete: None,
        },
    )?;
    let slot = slot_object(model, name);
    slot.insert("inverse".to_owned(), json!(inverse));
    slot.insert("inlined".to_owned(), json!(false));
    // The slot holding the data today is the source, so its entities do not change (DM-73).
    let annotations = slot.entry("annotations").or_insert_with(|| json!({}));
    if annotations.get("on_delete").is_none() {
        annotations["on_delete"] = json!("restrict");
    }
    Ok(())
}

/// Renames a class and every `is_a` and `range` that named it.
fn rename_class(model: &mut Value, operation: &Operation) -> Result<(), String> {
    let Operation::RenameClass { name, to } = operation else {
        return Err("not rename_class".to_owned());
    };
    existing(model, "classes", name, "class")?;
    new_name(model, "classes", to, "class")?;
    let classes = section_mut(model, "classes");
    let class = classes.remove(name).unwrap_or(Value::Null);
    classes.insert(to.clone(), class);
    for class in classes.values_mut() {
        if class.get("is_a").and_then(Value::as_str) == Some(name) {
            class["is_a"] = json!(to);
        }
    }
    for slot in section_mut(model, "slots").values_mut() {
        if slot.get("range").and_then(Value::as_str) == Some(name) {
            slot["range"] = json!(to);
        }
    }
    Ok(())
}

/// Each end's `multivalued`, source then target, for a cardinality read from source to target.
fn flags_of(cardinality: &str) -> Result<(bool, bool), String> {
    if !CARDINALITIES.contains(&cardinality) {
        return Err(format!(
            "'{cardinality}' is not a cardinality: {}",
            CARDINALITIES.join(", ")
        ));
    }
    Ok((
        matches!(cardinality, "one-to-many" | "many-to-many"),
        matches!(cardinality, "many-to-one" | "many-to-many"),
    ))
}

/// Where the foreign key would be: the "many" side, or the source of 1:1 and N:M (DM-67).
fn stored_on_source(cardinality: &str) -> bool {
    cardinality != "one-to-many"
}

fn delete_rule(rule: &str) -> Result<(), String> {
    if ON_DELETE_RULES.contains(&rule) {
        Ok(())
    } else {
        Err(format!(
            "'{rule}' is not a delete rule: {}",
            ON_DELETE_RULES.join(", ")
        ))
    }
}

fn kind_of(slot: &Value) -> Option<&str> {
    slot.pointer("/annotations/ngsi_ld_kind")
        .and_then(Value::as_str)
}

fn owners_of(model: &Value, slot: &str) -> Vec<String> {
    section(model, "classes")
        .map(|classes| {
            classes
                .iter()
                .filter(|(_, class)| slots_of(class).iter().any(|s| s == slot))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn slot_flag(model: &Value, slot: &str, field: &str) -> bool {
    model["slots"][slot].get(field).and_then(Value::as_bool) == Some(true)
}

/// A declared slot's fields; the caller has checked the slot exists.
fn slot_object<'a>(model: &'a mut Value, name: &str) -> &'a mut Map<String, Value> {
    let slot = section_mut(model, "slots")
        .entry(name)
        .or_insert_with(|| json!({}));
    if !slot.is_object() {
        *slot = json!({});
    }
    slot.as_object_mut().expect("an object")
}

fn set_slot_field(model: &mut Value, name: &str, field: &str, value: Option<Value>) {
    let slot = slot_object(model, name);
    match value {
        Some(value) => slot.insert(field.to_owned(), value),
        None => slot.remove(field),
    };
}

/// One end of a relationship written as a slot of its owner (DM-64).
struct End<'a> {
    name: &'a str,
    owner: &'a str,
    range: &'a str,
    inverse: &'a str,
    multivalued: bool,
    required: bool,
    on_delete: Option<&'a str>,
}

fn add_end(model: &mut Value, end: End<'_>) -> Result<(), String> {
    valid_name(end.name, "slot")?;
    if has(model, "slots", end.name) {
        return Err(format!(
            "slot '{}' already exists; a relationship's ends are slots of their own",
            end.name
        ));
    }
    existing(model, "classes", end.owner, "class")?;
    let mut slot = json!({ "range": end.range, "inverse": end.inverse, "inlined": false });
    if end.multivalued {
        slot["multivalued"] = json!(true);
    }
    if end.required {
        slot["required"] = json!(true);
    }
    if let Some(prefix) = own_prefix(model) {
        slot["slot_uri"] = json!(format!("{prefix}:{}", end.name));
    }
    slot["annotations"] = json!({ "ngsi_ld_kind": "Relationship" });
    if let Some(rule) = end.on_delete {
        slot["annotations"]["on_delete"] = json!(rule);
    }
    section_mut(model, "slots").insert(end.name.to_owned(), slot);
    let class = section_mut(model, "classes")
        .get_mut(end.owner)
        .expect("checked above");
    let mut slots = slots_of(class);
    slots.push(end.name.to_owned());
    set_slots(class, slots);
    Ok(())
}

/// The two ends of a relationship, by slot name: the source is the end that carries the delete
/// rule, the target the one that points back.
struct Pair {
    source: String,
    target: String,
}

/// The relationship `name` is an end of: a Relationship slot whose `inverse` names it back.
fn relationship_of(model: &Value, name: &str) -> Option<Pair> {
    let slot = model.get("slots")?.get(name)?;
    let other = slot.get("inverse")?.as_str()?;
    let back = model.get("slots")?.get(other)?;
    if kind_of(slot) != Some("Relationship")
        || kind_of(back) != Some("Relationship")
        || back.get("inverse").and_then(Value::as_str) != Some(name)
    {
        return None;
    }
    let marks = |one: &Value| one.pointer("/annotations/on_delete").is_some();
    // ponytail: a pair neither end marks with on_delete is sourced by declaration order in the
    // editor; the parsed model keeps no order, so the name that sorts first stands in. Every
    // pair these operations write carries the mark.
    let this_is_source = marks(slot) || (!marks(back) && name <= other);
    let (source, target) = if this_is_source {
        (name, other)
    } else {
        (other, name)
    };
    Some(Pair {
        source: source.to_owned(),
        target: target.to_owned(),
    })
}

fn relationship_named(model: &Value, name: &str) -> Result<Pair, String> {
    existing(model, "slots", name, "slot")?;
    relationship_of(model, name)
        .ok_or_else(|| format!("slot '{name}' is not an end of a relationship"))
}

/// A `setSlot` may not change one end of a relationship alone, nor require the computed end.
fn refuse_one_sided_edit(
    model: &Value,
    name: &str,
    field: &str,
    value: &Value,
) -> Result<(), String> {
    let Some(pair) = relationship_of(model, name) else {
        return Ok(());
    };
    if RELATIONSHIP_FIELDS.contains(&field) {
        return Err(format!(
            "slot '{name}' is an end of the relationship {}.{} ↔ {}.{}; change its cardinality with setCardinality, or remove the relationship with removeRelationship",
            owners_of(model, &pair.source).join("/"),
            pair.source,
            owners_of(model, &pair.target).join("/"),
            pair.target
        ));
    }
    let source_many = slot_flag(model, &pair.source, "multivalued");
    let target_many = slot_flag(model, &pair.target, "multivalued");
    // one-to-many is the one cardinality whose target end holds the data (DM-67).
    let stored_on_target = source_many && !target_many;
    let (computed, stored) = if stored_on_target {
        (&pair.source, &pair.target)
    } else {
        (&pair.target, &pair.source)
    };
    if field == "required" && value == &Value::Bool(true) && computed == name {
        return Err(format!("slot '{name}' is computed on read from {stored} and cannot be required; make the stored end required (DM-65)"));
    }
    Ok(())
}

/// The value a `setSlot` writes, `None` when it clears the field, as the editor's `setOrDelete`.
/// The names of a list as the metamodel writes them: trimmed, and the empty ones dropped.
fn named_list(names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|one| one.trim().to_owned())
        .filter(|one| !one.is_empty())
        .collect()
}

/// Sets one field of a class, or removes it when there is nothing to set.
fn set_class_field(model: &mut Value, name: &str, field: &str, value: Option<Value>) {
    let class = section_mut(model, "classes")
        .get_mut(name)
        .expect("checked by the caller");
    if !class.is_object() {
        *class = json!({});
    }
    let fields = class.as_object_mut().expect("an object");
    match value {
        Some(value) => fields.insert(field.to_owned(), value),
        None => fields.remove(field),
    };
}

fn set_value(
    model: &Value,
    name: &str,
    field: &str,
    value: &Value,
) -> Result<Option<Value>, String> {
    if !SET_FIELDS.contains(&field) {
        return Err(format!(
            "'{field}' of slot '{name}' is set in the model editor; a conversation sets {}",
            SET_FIELDS.join(", ")
        ));
    }
    match (field, value) {
        (_, Value::Null) | ("required" | "multivalued", Value::Bool(false)) => Ok(None),
        ("required" | "multivalued", Value::Bool(_)) => Ok(Some(value.clone())),
        ("required" | "multivalued", _) => {
            Err(format!("'{field}' of slot '{name}' takes true or false"))
        }
        (_, Value::String(text)) if text.trim().is_empty() => Ok(None),
        ("description", Value::String(_)) => Ok(Some(value.clone())),
        ("range", Value::String(range)) => {
            let range = range.trim();
            if RANGES.contains(&range) || has(model, "classes", range) || has(model, "enums", range)
            {
                Ok(Some(json!(range)))
            } else {
                Err(format!(
                    "range '{range}' of slot '{name}' is neither a type, an enum nor a class of this model"
                ))
            }
        }
        _ => Err(format!("'{field}' of slot '{name}' takes text")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bikes() -> Value {
        serde_norway::from_str(
            "classes:\n  Entity:\n    abstract: true\n  BikeHireDockingStation:\n    is_a: Entity\n    slots: [name, availableBikeNumber, status]\n  Vehicle:\n    is_a: Entity\n    slots: [name, station]\nslots:\n  name:\n    range: string\n  availableBikeNumber:\n    range: integer\n  status:\n    range: string\n  station:\n    range: BikeHireDockingStation\n",
        )
        .expect("yaml")
    }

    fn ops(json: Value) -> Vec<Operation> {
        serde_json::from_value(json).expect("operations")
    }

    #[test]
    fn a_blank_model_takes_the_organization_prefix_and_imports_ngsi_ld_core() {
        let model = blank("bratislava.sk", "bikes");
        assert_eq!(model["id"], "https://bratislava.sk/models/bikes");
        assert_eq!(model["default_prefix"], "bratislava");
        assert_eq!(
            model["prefixes"]["bratislava"],
            "https://bratislava.sk/terms/"
        );
        assert_eq!(model["imports"], json!(["linkml:types", "ngsi-ld-core"]));
        assert_eq!(blank(".sk", "bikes")["default_prefix"], "org");
        let added = apply(&model, &ops(json!([{ "op": "addClass", "name": "Bike" }])));
        assert!(added.is_ok(), "{added:?}");
    }

    fn op(json: Value) -> Operation {
        serde_json::from_value(json).expect("operation")
    }

    /// T-4470: the arms `mutate` was cut into, each called alone, and refusing another operation.
    #[test]
    fn rename_class_follows_every_is_a_and_range() {
        let mut model = bikes();
        rename_class(
            &mut model,
            &op(json!({ "op": "renameClass", "name": "BikeHireDockingStation", "to": "Dock" })),
        )
        .expect("renamed");
        assert!(model["classes"].get("BikeHireDockingStation").is_none());
        assert_eq!(model["slots"]["station"]["range"], json!("Dock"));
        let taken = op(json!({ "op": "renameClass", "name": "Dock", "to": "Vehicle" }));
        assert!(rename_class(&mut model, &taken).is_err());
        assert!(rename_class(
            &mut model,
            &op(json!({ "op": "removeClass", "name": "Dock" }))
        )
        .is_err());
    }

    #[test]
    fn add_slot_joins_its_class_and_refuses_an_unknown_kind() {
        let mut model = bikes();
        add_slot(&mut model, &op(json!({ "op": "addSlot", "name": "dock", "class": "Vehicle", "kind": "Relationship" }))).expect("added");
        assert_eq!(
            model["slots"]["dock"]["annotations"]["ngsi_ld_kind"],
            json!("Relationship")
        );
        assert!(slots_of(&model["classes"]["Vehicle"]).contains(&"dock".to_owned()));
        let bad = op(json!({ "op": "addSlot", "name": "x", "kind": "Thing" }));
        assert!(add_slot(&mut model, &bad)
            .unwrap_err()
            .contains("kind 'Thing'"));
        assert!(add_slot(
            &mut model,
            &op(json!({ "op": "addSlot", "name": "y", "class": "Nope" }))
        )
        .is_err());
    }

    #[test]
    fn add_relationship_writes_both_ends_and_refuses_its_own_inverse() {
        let mut model = bikes();
        let pair = json!({ "op": "addRelationship", "from": "Vehicle", "to": "BikeHireDockingStation", "name": "dockedAt", "inverse": "vehicles", "cardinality": "many-to-one" });
        add_relationship(&mut model, &op(pair)).expect("added");
        assert_eq!(model["slots"]["dockedAt"]["inverse"], json!("vehicles"));
        assert_eq!(model["slots"]["vehicles"]["inverse"], json!("dockedAt"));
        let selfish = json!({ "op": "addRelationship", "from": "Vehicle", "to": "Vehicle", "name": "twin", "inverse": "twin", "cardinality": "one-to-one" });
        assert!(add_relationship(&mut model, &op(selfish)).is_err());
    }

    #[test]
    fn add_inverse_needs_a_relationship_with_one_owner() {
        let mut model = bikes();
        // `station` is a string-kinded slot today: no Relationship, no inverse.
        let ask = op(json!({ "op": "addInverse", "name": "station", "inverse": "vehicles" }));
        assert!(add_inverse(&mut model, &ask).is_err());
        model["slots"]["station"]["annotations"] = json!({ "ngsi_ld_kind": "Relationship" });
        add_inverse(&mut model, &ask).expect("added");
        assert_eq!(model["slots"]["station"]["inverse"], json!("vehicles"));
        assert_eq!(
            model["slots"]["station"]["annotations"]["on_delete"],
            json!("restrict")
        );
        assert!(add_inverse(&mut model, &ask)
            .unwrap_err()
            .contains("already names"));
    }

    #[test]
    fn a_term_added_without_an_iri_is_minted_under_the_models_own_prefix() {
        // The helsinki model on dev: its default prefix is an imported vocabulary, so a term
        // left without an IRI would be minted there and refused (DM-04, DM-16, T-0893).
        let mut model = bikes();
        model["name"] = json!("helsinki");
        model["id"] = json!("https://hel.fi/models/helsinki/helsinki");
        model["default_prefix"] = json!("sdm");
        model["prefixes"] = json!({ "sdm": "https://smartdatamodels.org/" });
        let changed = apply(
            &model,
            &ops(json!([
                { "op": "addSlot", "name": "bikeType", "class": "BikeHireDockingStation", "range": "string" },
                { "op": "addSlot", "name": "colour", "slot_uri": "schema:color" },
                { "op": "addClass", "name": "Dock", "is_a": "Entity" }
            ])),
        )
        .expect("applied");
        assert_eq!(
            changed["slots"]["bikeType"]["slot_uri"],
            json!("helsinki:bikeType")
        );
        assert_eq!(
            changed["slots"]["colour"]["slot_uri"],
            json!("schema:color")
        );
        assert_eq!(
            changed["classes"]["Dock"]["class_uri"],
            json!("helsinki:Dock")
        );
        assert_eq!(
            changed["prefixes"]["helsinki"],
            json!("https://hel.fi/models/helsinki/helsinki/")
        );
        assert_eq!(
            changed["prefixes"]["sdm"],
            json!("https://smartdatamodels.org/")
        );
        assert_eq!(changed["default_prefix"], json!("sdm"));

        // A prefix the model already declares under its name is kept as it is.
        model["prefixes"]["helsinki"] = json!("https://hel.fi/ns/");
        let changed =
            apply(&model, &ops(json!([{ "op": "addSlot", "name": "x" }]))).expect("applied");
        assert_eq!(changed["prefixes"]["helsinki"], json!("https://hel.fi/ns/"));
        assert_eq!(changed["slots"]["x"]["slot_uri"], json!("helsinki:x"));
    }

    #[test]
    fn an_attribute_added_to_a_class_is_declared_and_listed_on_it() {
        let changed = apply(
            &bikes(),
            &ops(json!([{ "op": "addSlot", "name": "bikeType", "class": "BikeHireDockingStation", "range": "string" }])),
        )
        .expect("applied");
        assert_eq!(changed["slots"]["bikeType"], json!({ "range": "string" }));
        assert_eq!(
            changed["classes"]["BikeHireDockingStation"]["slots"],
            json!(["name", "availableBikeNumber", "status", "bikeType"])
        );
    }

    #[test]
    fn a_renamed_attribute_keeps_its_place_in_every_class_and_its_definition() {
        let changed = apply(
            &bikes(),
            &ops(json!([{ "op": "renameSlot", "name": "name", "to": "title" }])),
        )
        .expect("applied");
        assert!(changed["slots"].get("name").is_none());
        assert_eq!(changed["slots"]["title"], json!({ "range": "string" }));
        assert_eq!(
            changed["classes"]["BikeHireDockingStation"]["slots"],
            json!(["title", "availableBikeNumber", "status"])
        );
        assert_eq!(
            changed["classes"]["Vehicle"]["slots"],
            json!(["title", "station"])
        );
    }

    #[test]
    fn a_removed_attribute_leaves_every_class_and_a_renamed_class_every_reference() {
        let changed = apply(
            &bikes(),
            &ops(json!([
                { "op": "removeSlot", "name": "status" },
                { "op": "renameClass", "name": "BikeHireDockingStation", "to": "BikeStation" }
            ])),
        )
        .expect("applied");
        assert!(changed["slots"].get("status").is_none());
        assert!(changed["classes"].get("BikeHireDockingStation").is_none());
        assert_eq!(
            changed["classes"]["BikeStation"]["slots"],
            json!(["name", "availableBikeNumber"])
        );
        assert_eq!(changed["slots"]["station"]["range"], "BikeStation");
    }

    #[test]
    fn a_class_added_removed_or_detached_changes_only_what_it_names() {
        let changed = apply(
            &bikes(),
            &ops(json!([
                { "op": "addClass", "name": "Dock", "is_a": "Entity" },
                { "op": "attachSlot", "class": "Dock", "slot": "status" },
                { "op": "detachSlot", "class": "Vehicle", "slot": "station" },
                { "op": "setSlot", "name": "status", "field": "required", "value": true },
                { "op": "removeClass", "name": "Vehicle" }
            ])),
        )
        .expect("applied");
        assert_eq!(
            changed["classes"]["Dock"],
            json!({ "is_a": "Entity", "slots": ["status"] })
        );
        assert!(changed["classes"].get("Vehicle").is_none());
        assert_eq!(changed["slots"]["status"]["required"], true);
        assert_eq!(
            changed["slots"]["station"]["range"],
            "BikeHireDockingStation"
        );
    }

    #[test]
    fn an_unknown_class_or_slot_a_taken_name_or_a_wrong_value_is_refused_by_index() {
        let model = bikes();
        for (operations, reason) in [
            (
                json!([{ "op": "addSlot", "name": "bikeType", "class": "BikeStation" }]),
                "operation 0: unknown class 'BikeStation'",
            ),
            (
                json!([{ "op": "addSlot", "name": "x" }, { "op": "removeSlot", "name": "bikeType" }]),
                "operation 1: unknown slot 'bikeType'",
            ),
            (
                json!([{ "op": "renameSlot", "name": "status", "to": "name" }]),
                "operation 0: slot 'name' already exists",
            ),
            (
                json!([{ "op": "addClass", "name": "bike station" }]),
                "operation 0: 'bike station' is not a valid class name",
            ),
            (
                json!([{ "op": "setSlot", "name": "status", "field": "range", "value": "text" }]),
                "operation 0: range 'text' of slot 'status' is neither a type, an enum nor a class of this model",
            ),
            (
                json!([{ "op": "setSlot", "name": "status", "field": "unit", "value": "GQ" }]),
                "operation 0: 'unit' of slot 'status' is set in the model editor; a conversation sets range, required, multivalued, description",
            ),
            (
                json!([{ "op": "detachSlot", "class": "Vehicle", "slot": "status" }]),
                "operation 0: class 'Vehicle' does not use slot 'status'",
            ),
        ] {
            assert_eq!(apply(&model, &ops(operations)), Err(reason.to_owned()));
        }
    }

    #[test]
    fn an_enum_slot_keeps_its_values_and_a_repeated_value_or_unknown_enum_is_refused() {
        // S-10 (T-6530): "an optional slot direction (enum in, out)" reaches the model as an enum.
        let model = apply(
            &bikes(),
            &ops(json!([
                { "op": "addEnum", "name": "DirectionEnum" },
                { "op": "addEnumValue", "enum": "DirectionEnum", "value": "in" },
                { "op": "addEnumValue", "enum": "DirectionEnum", "value": "out", "description": "leaving" },
                { "op": "addSlot", "name": "direction", "class": "Vehicle", "range": "DirectionEnum" }
            ])),
        )
        .expect("applied");
        assert_eq!(
            model["enums"]["DirectionEnum"],
            json!({ "permissible_values": { "in": {}, "out": { "description": "leaving" } } })
        );
        assert_eq!(model["slots"]["direction"]["range"], "DirectionEnum");
        for (operations, reason) in [
            (
                json!([{ "op": "addEnumValue", "enum": "Nope", "value": "in" }]),
                "operation 0: unknown enum 'Nope'",
            ),
            (
                json!([
                    { "op": "addEnum", "name": "DirectionEnum" },
                    { "op": "addEnumValue", "enum": "DirectionEnum", "value": "in" },
                    { "op": "addEnumValue", "enum": "DirectionEnum", "value": "in" }
                ]),
                "operation 2: enum 'DirectionEnum' already has the value 'in'",
            ),
            (
                json!([{ "op": "addEnum", "name": "Direction Enum" }]),
                "operation 0: 'Direction Enum' is not a valid enum name",
            ),
        ] {
            assert_eq!(apply(&bikes(), &ops(operations)), Err(reason.to_owned()));
        }
    }

    #[test]
    fn an_operation_the_editor_does_not_know_or_a_field_it_does_not_take_does_not_parse() {
        assert!(
            serde_json::from_value::<Operation>(json!({ "op": "dropTable", "name": "x" })).is_err()
        );
        assert!(serde_json::from_value::<Operation>(
            json!({ "op": "removeSlot", "name": "x", "cascade": true })
        )
        .is_err());
    }

    #[test]
    fn the_outline_names_each_class_with_its_slots() {
        assert_eq!(
            outline(&bikes()),
            "BikeHireDockingStation: name, availableBikeNumber, status; Entity: ; Vehicle: name, station"
        );
        assert_eq!(outline(&json!({})), "the model has no class");
    }

    /// T-1085, DM-13: the assistant edits the hierarchy the model declares, not only its slots.
    /// A class that specialises nothing this model has, or itself, is refused rather than
    /// written, so the model never names a parent the generators cannot resolve.
    #[test]
    fn a_class_hierarchy_is_set_and_cleared_through_the_operations() {
        let changed = apply(
            &bikes(),
            &ops(json!([
                { "op": "setClassParent", "name": "Vehicle", "parent": "Entity" },
                { "op": "setClassMixins", "name": "Vehicle", "mixins": ["Entity"] },
                { "op": "setSlotSubsets", "name": "name", "subsets": ["public", "steward"] }
            ])),
        )
        .expect("the hierarchy lands");
        assert_eq!(changed["classes"]["Vehicle"]["is_a"], json!("Entity"));
        assert_eq!(changed["classes"]["Vehicle"]["mixins"], json!(["Entity"]));
        assert_eq!(
            changed["slots"]["name"]["subsets"],
            json!(["public", "steward"])
        );

        // Emptied, the keys go rather than staying behind as null or an empty list.
        let cleared = apply(
            &changed,
            &ops(json!([
                { "op": "setClassParent", "name": "Vehicle", "parent": "  " },
                { "op": "setClassMixins", "name": "Vehicle", "mixins": [] },
                { "op": "setSlotSubsets", "name": "name", "subsets": [] }
            ])),
        )
        .expect("the hierarchy goes");
        assert!(cleared["classes"]["Vehicle"].get("is_a").is_none());
        assert!(cleared["classes"]["Vehicle"].get("mixins").is_none());
        assert!(cleared["slots"]["name"].get("subsets").is_none());
    }

    fn school() -> Value {
        serde_norway::from_str(
            "name: learning\nid: https://example.org/learning\nclasses:\n  School:\n    slots: [name]\n  User:\n    slots: [name]\n  Course:\n    slots: []\n  Profile:\n    slots: []\n  Person:\n    slots: []\nslots:\n  name:\n    range: string\n",
        )
        .expect("a model")
    }

    /// DM-64, T-2742: the four cardinalities the assistant builds, each as two ends that name
    /// each other, the stored end required where the person said so, the delete rule on the source.
    #[test]
    fn every_cardinality_is_two_ends_naming_each_other_with_the_rule_on_the_source() {
        let changed = apply(
            &school(),
            &ops(json!([
                { "op": "addRelationship", "from": "School", "to": "User", "name": "users", "inverse": "school", "cardinality": "one-to-many", "inverseRequired": true },
                { "op": "addRelationship", "from": "User", "to": "Course", "name": "courses", "inverse": "students", "cardinality": "many-to-many" },
                { "op": "addRelationship", "from": "User", "to": "Profile", "name": "profile", "inverse": "owner", "cardinality": "one-to-one", "onDelete": "cascade" },
                { "op": "addRelationship", "from": "Person", "to": "Person", "name": "manager", "inverse": "reports", "cardinality": "many-to-one", "onDelete": "set-null" }
            ])),
        )
        .expect("applied");
        let slot = |name: &str| changed["slots"][name].clone();
        assert_eq!(
            slot("users"),
            json!({ "range": "User", "multivalued": true, "inverse": "school", "inlined": false, "slot_uri": "learning:users", "annotations": { "ngsi_ld_kind": "Relationship", "on_delete": "restrict" } })
        );
        assert_eq!(
            slot("school"),
            json!({ "range": "School", "required": true, "inverse": "users", "inlined": false, "slot_uri": "learning:school", "annotations": { "ngsi_ld_kind": "Relationship" } })
        );
        assert_eq!(slot("courses")["multivalued"], json!(true));
        assert_eq!(slot("students")["multivalued"], json!(true));
        assert!(
            slot("profile").get("multivalued").is_none()
                && slot("owner").get("multivalued").is_none()
        );
        assert_eq!(
            slot("profile")["annotations"]["on_delete"],
            json!("cascade")
        );
        assert_eq!(slot("reports")["multivalued"], json!(true));
        assert!(slot("manager").get("multivalued").is_none());
        assert_eq!(
            changed["classes"]["User"]["slots"],
            json!(["name", "school", "courses", "profile"])
        );
        assert_eq!(
            changed["classes"]["Person"]["slots"],
            json!(["manager", "reports"])
        );
    }

    /// DM-64, DM-65: what the editor refuses, the conversation is refused too, by index.
    #[test]
    fn a_relationship_the_model_cannot_hold_is_refused_by_index() {
        let rel = |extra: Value| {
            let mut op = json!({ "op": "addRelationship", "from": "School", "to": "User", "name": "users", "inverse": "school", "cardinality": "one-to-many" });
            op.as_object_mut()
                .expect("an object")
                .extend(extra.as_object().expect("an object").clone());
            op
        };
        for (op, reason) in [
            (rel(json!({ "inverse": "" })), "operation 0: the relationship 'users' needs an inverse: the slot on User that points back (DM-64)"),
            (rel(json!({ "inverse": "users" })), "operation 0: 'users' cannot be its own inverse: the two ends are two slots"),
            (rel(json!({ "to": "Nowhere" })), "operation 0: unknown class 'Nowhere': a relationship's inverse is written on its target, so the target is a class of this model"),
            (rel(json!({ "cardinality": "several" })), "operation 0: 'several' is not a cardinality: one-to-one, one-to-many, many-to-one, many-to-many"),
            (rel(json!({ "onDelete": "ignore" })), "operation 0: 'ignore' is not a delete rule: restrict, cascade, set-null"),
            (rel(json!({ "required": true })), "operation 0: as one-to-many, users is computed on read and cannot be required; require the other end (DM-65)"),
            (rel(json!({ "name": "name" })), "operation 0: slot 'name' already exists; a relationship's ends are slots of their own"),
        ] {
            assert_eq!(apply(&school(), &ops(json!([op]))), Err(reason.to_owned()));
        }
    }

    #[test]
    fn a_relationship_changes_both_ends_together_and_leaves_as_a_pair() {
        let built = apply(
            &school(),
            &ops(json!([{ "op": "addRelationship", "from": "School", "to": "User", "name": "users", "inverse": "school", "cardinality": "one-to-many", "inverseRequired": true }])),
        )
        .expect("applied");
        // One end alone is refused: the two would stop agreeing.
        assert_eq!(
            apply(&built, &ops(json!([{ "op": "setSlot", "name": "school", "field": "multivalued", "value": true }]))),
            Err("operation 0: slot 'school' is an end of the relationship School.users ↔ User.school; change its cardinality with setCardinality, or remove the relationship with removeRelationship".to_owned())
        );
        assert_eq!(
            apply(&built, &ops(json!([{ "op": "setSlot", "name": "users", "field": "required", "value": true }]))),
            Err("operation 0: slot 'users' is computed on read from school and cannot be required; make the stored end required (DM-65)".to_owned())
        );
        // As many-to-many the stored end moves to users, and school, required, would be computed.
        assert_eq!(
            apply(&built, &ops(json!([{ "op": "setCardinality", "name": "school", "cardinality": "many-to-many" }]))),
            Err("operation 0: as many-to-many, school would be computed on read, and it is required; clear required on school first (DM-65)".to_owned())
        );
        let changed = apply(
            &built,
            &ops(json!([
                { "op": "setSlot", "name": "school", "field": "required", "value": false },
                { "op": "setCardinality", "name": "school", "cardinality": "many-to-many" },
                { "op": "setOnDelete", "name": "school", "onDelete": "cascade" }
            ])),
        )
        .expect("applied");
        assert_eq!(changed["slots"]["users"]["multivalued"], json!(true));
        assert_eq!(changed["slots"]["school"]["multivalued"], json!(true));
        assert_eq!(
            changed["slots"]["users"]["annotations"]["on_delete"],
            json!("cascade")
        );
        assert!(changed["slots"]["school"]["annotations"]
            .get("on_delete")
            .is_none());

        let removed = apply(
            &changed,
            &ops(json!([{ "op": "removeRelationship", "name": "users" }])),
        )
        .expect("applied");
        assert!(
            removed["slots"].get("users").is_none() && removed["slots"].get("school").is_none()
        );
        assert_eq!(removed["classes"]["School"]["slots"], json!(["name"]));
        assert_eq!(removed["classes"]["User"]["slots"], json!(["name"]));
        assert_eq!(
            apply(
                &school(),
                &ops(json!([{ "op": "setOnDelete", "name": "name", "onDelete": "cascade" }]))
            ),
            Err("operation 0: slot 'name' is not an end of a relationship".to_owned())
        );
    }

    /// DM-73: a relationship saved without an inverse gets a multivalued one on its target, and
    /// the slot holding the data stays the stored end.
    #[test]
    fn an_inverse_is_added_to_a_relationship_saved_without_one() {
        let old = apply(
            &school(),
            &ops(json!([{ "op": "addSlot", "name": "school", "class": "User", "range": "School", "kind": "Relationship" }])),
        )
        .expect("applied");
        let fixed = apply(
            &old,
            &ops(json!([{ "op": "addInverse", "name": "school", "inverse": "users" }])),
        )
        .expect("applied");
        assert_eq!(fixed["slots"]["school"]["inverse"], json!("users"));
        assert_eq!(
            fixed["slots"]["school"]["annotations"]["on_delete"],
            json!("restrict")
        );
        assert_eq!(fixed["slots"]["users"]["multivalued"], json!(true));
        assert_eq!(fixed["slots"]["users"]["range"], json!("User"));
        assert_eq!(
            fixed["classes"]["School"]["slots"],
            json!(["name", "users"])
        );
        assert_eq!(
            apply(
                &fixed,
                &ops(json!([{ "op": "addInverse", "name": "school", "inverse": "pupils" }]))
            ),
            Err("operation 0: slot 'school' already names the inverse 'users'".to_owned())
        );
        assert_eq!(
            apply(
                &school(),
                &ops(json!([{ "op": "addInverse", "name": "name", "inverse": "x" }]))
            ),
            Err(
                "operation 0: slot 'name' is not a Relationship pointing at a class of this model"
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_hierarchy_that_names_nothing_or_names_itself_is_refused() {
        for operation in [
            json!({ "op": "setClassParent", "name": "Vehicle", "parent": "Nowhere" }),
            json!({ "op": "setClassParent", "name": "Vehicle", "parent": "Vehicle" }),
            json!({ "op": "setClassMixins", "name": "Vehicle", "mixins": ["Vehicle"] }),
            json!({ "op": "setClassMixins", "name": "Vehicle", "mixins": ["Nowhere"] }),
            json!({ "op": "setClassParent", "name": "Nowhere", "parent": "Entity" }),
            json!({ "op": "setSlotSubsets", "name": "name", "subsets": ["not a name"] }),
        ] {
            let refused = apply(&bikes(), &ops(json!([operation.clone()])));
            assert!(refused.is_err(), "{operation} was accepted");
        }
    }
}
