//! The checks Model Tools' loader runs on every model before any generator sees it
//! (common.py `load`): the NGSI-LD kind of a slot, unit prefixes (DM-59), relationships
//! (relationships.py, DM-64..DM-69) and Data Structure Definitions (DM-60). Messages are the
//! Python ones, word for word: the Portal shows them and refuses a save on them.

use super::schema::{Obj, View, annotation_value, attributes, py_str, texts, truthy};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// What the loader refuses a model with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoadError {
    /// One message, the same for every generator.
    Model(String),
    /// The broken relationship rules, one entry each (`{path}: {rule}: {message}`).
    Relationships(Vec<String>),
}

impl From<String> for LoadError {
    fn from(message: String) -> Self {
        Self::Model(message)
    }
}

/// The NGSI-LD kinds a slot may declare (DM-05).
pub(super) const NGSI_LD_KINDS: [&str; 7] = [
    "Property",
    "GeoProperty",
    "Relationship",
    "LanguageProperty",
    "ListProperty",
    "JsonProperty",
    "VocabProperty",
];

/// `ngsi_ld_kind`: the declared kind, `Property` when none is declared.
pub(super) fn ngsi_ld_kind(name: &str, slot: &Obj) -> Result<String, String> {
    match annotation_value(slot, "ngsi_ld_kind") {
        None => Ok("Property".to_owned()),
        Some(kind) if NGSI_LD_KINDS.contains(&kind.as_str()) => Ok(kind),
        Some(kind) => Err(format!(
            "slot '{name}' declares ngsi_ld_kind '{kind}', which is not one of {}; write it as \
             `annotations: {{ngsi_ld_kind: Relationship}}` (or another of them)",
            NGSI_LD_KINDS.join(", ")
        )),
    }
}

/// Every check of `load`, in its order.
pub(super) fn check(view: &View) -> Result<(), LoadError> {
    unit_prefixes(view)?;
    let problems = relationships(view)?;
    if !problems.is_empty() {
        return Err(LoadError::Relationships(problems));
    }
    data_structures(view)?;
    Ok(())
}

fn unit_prefixes(view: &View) -> Result<(), String> {
    let declared: HashSet<&str> = view
        .schema
        .prefixes
        .iter()
        .map(|(p, _)| p.as_str())
        .collect();
    let mut dangling = Vec::new();
    for (name, slot) in view.all_slots() {
        let Some(unit) = slot.get("unit").and_then(Value::as_object) else {
            continue;
        };
        let mut curies = texts(unit.get("exact_mappings"));
        if let Some(kind) = unit.get("has_quantity_kind").filter(|v| truthy(Some(v))) {
            curies.push(py_str(kind));
        }
        for curie in curies {
            if curie.contains("://") || !curie.contains(':') {
                continue;
            }
            let prefix = curie.split(':').next().unwrap_or_default();
            if !declared.contains(prefix) {
                dangling.push(format!("'{name}' → {curie}"));
            }
        }
    }
    if dangling.is_empty() {
        return Ok(());
    }
    dangling.sort();
    Err(format!(
        "these unit mappings use a prefix the model does not declare, so the CURIE resolves to \
         nothing wherever the artifacts carry it; add it to `prefixes`: {}",
        dangling.join("; ")
    ))
}

const ON_DELETE_RULES: [&str; 3] = ["restrict", "cascade", "set-null"];
const EXTERNAL_RANGE: &str = "uriorcurie";

/// One slot where a class declares it.
struct Placed {
    cls: String,
    name: String,
    slot: Obj,
    path: String,
    order: usize,
    range: Option<String>,
}

fn declared_range(view: &View, cls: &Obj, name: &str) -> Option<String> {
    let usage = cls
        .get("slot_usage")
        .and_then(Value::as_object)
        .and_then(|u| u.get(name))
        .and_then(Value::as_object);
    if let Some(range) = usage.and_then(|u| u.get("range")).filter(|r| truthy(Some(r))) {
        return Some(py_str(range));
    }
    if let Some((_, attr)) = attributes(cls).find(|(n, _)| *n == name) {
        return attr.get("range").filter(|r| !r.is_null()).map(py_str);
    }
    let slot = view.schema.slots.get(name).and_then(Value::as_object).or_else(|| {
        view.classes()
            .filter_map(|(_, c)| attributes(c).find(|(n, _)| *n == name).map(|(_, a)| a))
            .last()
    })?;
    slot.get("range").filter(|r| !r.is_null()).map(py_str)
}

fn relationships(view: &View) -> Result<Vec<String>, String> {
    let order: HashMap<&str, usize> = view
        .schema
        .slots
        .keys()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut inline_order = order.len();
    let mut placed: Vec<Placed> = Vec::new();
    let mut listed_by: HashMap<String, Vec<String>> = HashMap::new();
    for (cname, cls) in view.classes() {
        for name in texts(cls.get("slots")) {
            listed_by
                .entry(name.clone())
                .or_default()
                .push(cname.clone());
            placed.push(Placed {
                cls: cname.clone(),
                slot: view.induced_slot(&name, Some(cname))?,
                path: format!("slots.{name}"),
                order: order.get(name.as_str()).copied().unwrap_or(0),
                range: declared_range(view, cls, &name),
                name,
            });
        }
        for (name, _) in attributes(cls) {
            placed.push(Placed {
                cls: cname.clone(),
                slot: view.induced_slot(name, Some(cname))?,
                path: format!("classes.{cname}.attributes.{name}"),
                order: inline_order,
                range: declared_range(view, cls, name),
                name: name.clone(),
            });
            inline_order += 1;
        }
    }
    let mut by_end: HashMap<(String, String), usize> = HashMap::new();
    for (i, one) in placed.iter().enumerate() {
        by_end.insert((one.cls.clone(), one.name.clone()), i);
    }
    let text = |slot: &Obj, k: &str| slot.get(k).filter(|v| !v.is_null()).map(py_str);

    let mut problems: Vec<String> = Vec::new();
    let mut problem = |path: &str, rule: &str, message: String| {
        let entry = format!("{path}: {rule}: {message}");
        if !problems.contains(&entry) {
            problems.push(entry);
        }
    };
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for end in &placed {
        let (slot, owner, path, name) = (&end.slot, &end.cls, &end.path, &end.name);
        let kind = ngsi_ld_kind(name, slot)?;
        let target = end.range.as_deref();
        let class_range = target.is_some_and(|t| view.is_class(t));
        if kind != "Relationship" {
            if class_range && kind != "JsonProperty" && slot.get("inlined") != Some(&Value::Bool(true))
            {
                let target = target.unwrap_or_default();
                problem(
                    path,
                    "class-range-not-relationship",
                    format!(
                        "{name} on {owner} points at the class {target} but is a {kind}; a slot \
                         whose range is a class is a Relationship: declare it with `annotations: \
                         {{ngsi_ld_kind: Relationship}}` and an inverse slot on {target} that \
                         points back"
                    ),
                );
            }
            continue;
        }
        let inverse = text(slot, "inverse").filter(|i| !i.is_empty());
        let Some(target) = target.filter(|t| *t != EXTERNAL_RANGE) else {
            if let Some(inverse) = &inverse {
                problem(
                    path,
                    "range-not-a-class",
                    format!(
                        "{name} on {owner} names the inverse {inverse} but its range is no \
                         class; name the class it points at"
                    ),
                );
            }
            continue;
        };
        if view.is_type(target) || view.is_enum(target) {
            problem(
                path,
                "primitive-range",
                format!(
                    "{name} on {owner} is a Relationship with the range {target}; a Relationship \
                     points at a class, or at uriorcurie for an entity outside the model"
                ),
            );
            continue;
        }
        if !class_range {
            problem(
                path,
                "range-not-a-class",
                format!(
                    "{name} on {owner} points at {target}, which is no class of this model or of \
                     its imports"
                ),
            );
            continue;
        }
        let on_delete = annotation_value(slot, "on_delete");
        if let Some(rule) = on_delete
            .as_deref()
            .filter(|r| !ON_DELETE_RULES.contains(r))
        {
            problem(
                path,
                "on-delete-unknown",
                format!(
                    "{name} on {owner} has on_delete '{rule}'; it is one of {}",
                    ON_DELETE_RULES.join(", ")
                ),
            );
        }
        let owners = listed_by.get(name.as_str()).cloned().unwrap_or_default();
        if path.starts_with("slots.") && owners.len() > 1 {
            problem(
                path,
                "slot-in-two-relationships",
                format!(
                    "{name} is used by {}, so it would be an end of two relationships; give each \
                     class a slot of its own",
                    owners.join(" and ")
                ),
            );
            continue;
        }
        let Some(inverse) = inverse else {
            problem(
                path,
                "inverse-missing",
                format!(
                    "{name} ({owner} → {target}) names no inverse; add the slot on {target} that \
                     points back"
                ),
            );
            continue;
        };
        let Some(other) = by_end
            .get(&(target.to_owned(), inverse.clone()))
            .map(|i| &placed[*i])
        else {
            problem(
                path,
                "inverse-missing",
                format!("{name} names the inverse {inverse}, which {target} does not have"),
            );
            continue;
        };
        if other.cls == *owner && other.name == *name {
            problem(
                path,
                "inverse-not-reciprocal",
                format!("{name} names itself as its inverse; the other end is a slot of its own"),
            );
            continue;
        }
        if ngsi_ld_kind(&other.name, &other.slot)? != "Relationship"
            || other.range.as_deref() != Some(owner.as_str())
            || text(&other.slot, "inverse").as_deref() != Some(name.as_str())
        {
            problem(
                path,
                "inverse-not-reciprocal",
                format!(
                    "{name} ({owner} → {target}) names {target}.{inverse} as its inverse, which \
                     does not point back at {owner}.{name}"
                ),
            );
            continue;
        }
        let here = format!("{owner}.{name}");
        let there = format!("{target}.{}", other.name);
        let key = if here <= there {
            (here, there)
        } else {
            (there, here)
        };
        if !seen.insert(key) {
            continue;
        }
        let other_delete = annotation_value(&other.slot, "on_delete");
        if on_delete.is_some() && other_delete.is_some() {
            problem(
                path,
                "on-delete-on-both-ends",
                format!(
                    "both {name} and {} carry on_delete; it goes on the source end only",
                    other.name
                ),
            );
            continue;
        }
        let this_is_source =
            on_delete.is_some() || (other_delete.is_none() && end.order <= other.order);
        let (source, tgt) = if this_is_source {
            (end, other)
        } else {
            (other, end)
        };
        let source_many = truthy(source.slot.get("multivalued"));
        let target_many = truthy(tgt.slot.get("multivalued"));
        // The stored end is the target only for one-to-many (DM-67).
        let computed = if source_many && !target_many {
            source
        } else {
            tgt
        };
        if truthy(computed.slot.get("required")) {
            problem(
                &computed.path,
                "required-on-computed-end",
                format!(
                    "{} on {} is computed on read and cannot be required; make the stored end \
                     required instead",
                    computed.name, computed.cls
                ),
            );
        }
    }
    Ok(problems)
}

const QB_DSD: &str = "qb_dsd";
const QB_COMPONENT: &str = "qb_component";
const QB_TIME_RANGES: [&str; 3] = ["date", "datetime", "integer"];

/// `unit_of(slot) is None`: a unit with no field set.
fn has_unit(slot: &Obj) -> bool {
    let Some(unit) = slot.get("unit").and_then(Value::as_object) else {
        return false;
    };
    ["ucum_code", "symbol", "descriptive_name", "exact_mappings", "has_quantity_kind"]
        .iter()
        .any(|k| truthy(unit.get(*k)))
}

fn data_structures(view: &View) -> Result<(), String> {
    let mut problems: Vec<String> = Vec::new();
    let mut dsd_slots: HashSet<String> = HashSet::new();
    for (cname, cls) in view.classes() {
        let flag = annotation_value(cls, QB_DSD);
        if let Some(flag) = flag
            .as_deref()
            .filter(|f| !matches!(f.to_lowercase().as_str(), "true" | "false"))
        {
            problems.push(format!(
                "class '{cname}' has {QB_DSD} '{flag}'; it is true or false"
            ));
            continue;
        }
        if flag.as_deref().map(str::to_lowercase).as_deref() != Some("true") {
            continue;
        }
        let (mut dimensions, mut measures, mut time) = (0usize, 0usize, 0usize);
        for slot_name in view.class_slots(cname)? {
            let slot = view.induced_slot(&slot_name, Some(cname))?;
            let Some(role) = annotation_value(&slot, QB_COMPONENT) else {
                continue;
            };
            dsd_slots.insert(slot_name.clone());
            let place = format!("slot '{slot_name}' of the DSD '{cname}'");
            if role != "dimension" && role != "measure" {
                problems.push(format!(
                    "{place} has {QB_COMPONENT} '{role}'; it is dimension or measure"
                ));
                continue;
            }
            if !truthy(slot.get("required")) {
                problems.push(format!(
                    "{place} is a {role} and must be required: an observation without it is no \
                     cell of the table"
                ));
            }
            if truthy(slot.get("multivalued")) {
                problems.push(format!(
                    "{place} is a {role} and holds one value per observation, so it cannot be \
                     multivalued"
                ));
            }
            let kind = ngsi_ld_kind(&slot_name, &slot)?;
            let range = slot
                .get("range")
                .filter(|r| truthy(Some(r)))
                .map_or_else(|| "string".to_owned(), py_str);
            if role == "dimension" {
                dimensions += 1;
                if kind == "Property" && QB_TIME_RANGES.contains(&range.as_str()) {
                    time += 1;
                } else if kind != "VocabProperty" && kind != "Relationship" {
                    problems.push(format!(
                        "{place} is a dimension of kind {kind} with range {range}; a dimension is \
                         a VocabProperty, a Relationship, or the one time dimension (a Property \
                         of range {})",
                        QB_TIME_RANGES.join(", ")
                    ));
                }
            } else {
                measures += 1;
                if kind != "Property" {
                    problems.push(format!(
                        "{place} is a measure of kind {kind}; a measure is a Property"
                    ));
                } else if !has_unit(&slot) {
                    problems.push(format!(
                        "{place} is a measure without a unit; give it one (DM-59)"
                    ));
                }
            }
        }
        if time > 1 {
            problems.push(format!(
                "the DSD '{cname}' has {time} time dimensions; it has one at most, the rest are \
                 VocabProperty terms"
            ));
        }
        if dimensions == 0 || measures == 0 {
            problems.push(format!(
                "the DSD '{cname}' needs at least one dimension and one measure (qb_component on \
                 its slots)"
            ));
        }
    }
    for (name, slot) in view.all_slots() {
        if annotation_value(slot, QB_COMPONENT).is_some() && !dsd_slots.contains(&name) {
            problems.push(format!(
                "slot '{name}' declares {QB_COMPONENT} but no class using it is annotated \
                 {QB_DSD}: true"
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the Data Structure Definition is not a cube: {}",
            problems.join("; ")
        ))
    }
}
