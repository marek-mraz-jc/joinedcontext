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
    if let Some(range) = usage
        .and_then(|u| u.get("range"))
        .filter(|r| truthy(Some(r)))
    {
        return Some(py_str(range));
    }
    if let Some((_, attr)) = attributes(cls).find(|(n, _)| *n == name) {
        return attr.get("range").filter(|r| !r.is_null()).map(py_str);
    }
    let slot = view
        .schema
        .slots
        .get(name)
        .and_then(Value::as_object)
        .or_else(|| {
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
            if class_range
                && kind != "JsonProperty"
                && slot.get("inlined") != Some(&Value::Bool(true))
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
    [
        "ucum_code",
        "symbol",
        "descriptive_name",
        "exact_mappings",
        "has_quantity_kind",
    ]
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

#[cfg(test)]
mod tests {
    use super::super::generate;
    use serde_json::{Map, Value};

    const HEAD: &str = "\
id: https://city.example/models/t
name: t
default_prefix: city
default_range: string
prefixes:
  city: https://city.example/models/t/
  linkml: https://w3id.org/linkml/
imports:
  - linkml:types
  - ngsi-ld-core
";

    /// The `errors` a model of `HEAD` plus `body` answers.
    fn errors(body: &str) -> Vec<String> {
        compiled(&format!("{HEAD}{body}"))
    }

    fn compiled(source: &str) -> Vec<String> {
        let mut m = Map::new();
        m.insert("source".to_owned(), Value::String(source.to_owned()));
        let (_, out) = generate(&m);
        let errors = out["errors"].as_array().cloned().unwrap_or_default();
        assert!(out["errors"].is_array(), "no errors in {out}");
        errors
            .iter()
            .map(|e| e.as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn has(errors: &[String], needle: &str) -> bool {
        errors.iter().any(|e| e.contains(needle))
    }

    /// Two classes joined by `here` (on A, to B) and `there` (on B, to A); `a` and `b` add
    /// lines to each slot.
    fn pair(a: &str, b: &str) -> String {
        format!(
            "\
classes:
  A:
    is_a: Entity
    slots: [here]
  B:
    is_a: Entity
    slots: [there]
slots:
  here:
    range: B
    inverse: there
    annotations:
      ngsi_ld_kind: Relationship
{a}  there:
    range: A
    inverse: here
    annotations:
      ngsi_ld_kind: Relationship
{b}"
        )
    }

    /// A class that lists a slot its parent declares as an attribute reads that attribute's
    /// range; a class that lists another class's attribute without inheriting it is refused.
    #[test]
    fn a_slot_listed_from_another_classes_attributes_reads_its_range() {
        let body = "\
classes:
  A:
    is_a: Entity
    attributes:
      shared:
        range: integer
        annotations:
          ngsi_ld_kind: Property
  B:
    is_a: A
    slots: [shared]
";
        assert_eq!(errors(body), Vec::<String>::new());
        let e = errors(&body.replace("    is_a: A\n", "    is_a: Entity\n"));
        assert!(
            e.iter()
                .any(|m| m.contains("No such slot shared as an attribute of B ancestors")),
            "{e:?}"
        );
    }

    #[test]
    fn a_reciprocal_pair_compiles_and_an_unknown_kind_is_named() {
        assert_eq!(errors(&pair("", "")), Vec::<String>::new());
        let bad = errors(
            "classes:\n  A:\n    is_a: Entity\n    slots: [s]\nslots:\n  s:\n    annotations:\n      ngsi_ld_kind: Thing\n",
        );
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(
            bad[0].starts_with(
                "slot 's' declares ngsi_ld_kind 'Thing', which is not one of Property, GeoProperty"
            ),
            "{bad:?}"
        );
    }

    #[test]
    fn a_slot_whose_range_is_a_class_must_be_a_relationship() {
        let body = "\
classes:
  A:
    is_a: Entity
    slots: [b, blob, nested]
  B:
    is_a: Entity
slots:
  b:
    range: B
  blob:
    range: B
    annotations:
      ngsi_ld_kind: JsonProperty
  nested:
    range: B
    inlined: true
";
        let e = errors(body);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].starts_with("slots.b: class-range-not-relationship: b on A points at the class B but is a Property"), "{e:?}");
    }

    #[test]
    fn a_relationship_to_an_outside_entity_needs_no_inverse_and_names_none() {
        let body = "\
classes:
  A:
    is_a: Entity
    slots: [outside, wrong]
slots:
  outside:
    range: uriorcurie
    annotations:
      ngsi_ld_kind: Relationship
  wrong:
    range: uriorcurie
    inverse: back
    annotations:
      ngsi_ld_kind: Relationship
";
        let e = errors(body);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].starts_with("slots.wrong: range-not-a-class: wrong on A names the inverse back but its range is no class"), "{e:?}");
    }

    #[test]
    fn a_relationship_to_a_type_or_an_enum_is_refused() {
        let body = "\
classes:
  A:
    is_a: Entity
    slots: [count, state]
slots:
  count:
    range: integer
    annotations:
      ngsi_ld_kind: Relationship
  state:
    range: State
    annotations:
      ngsi_ld_kind: Relationship
enums:
  State:
    permissible_values:
      on: {}
";
        let e = errors(body);
        assert!(
            has(
                &e,
                "slots.count: primitive-range: count on A is a Relationship with the range integer"
            ),
            "{e:?}"
        );
        assert!(
            has(
                &e,
                "slots.state: primitive-range: state on A is a Relationship with the range State"
            ),
            "{e:?}"
        );
        assert_eq!(e.len(), 2, "{e:?}");
    }

    #[test]
    fn on_delete_takes_one_of_three_rules_and_only_on_one_end() {
        let e = errors(&pair("      on_delete: explode\n", ""));
        assert!(
            has(
                &e,
                "slots.here: on-delete-unknown: here on A has on_delete 'explode'; it is one of restrict, cascade, set-null"
            ),
            "{e:?}"
        );
        let both = pair("      on_delete: cascade\n", "      on_delete: restrict\n");
        let e = errors(&both);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(
            e[0].contains("on-delete-on-both-ends: both here and there carry on_delete"),
            "{e:?}"
        );
        assert_eq!(
            errors(&pair("      on_delete: set-null\n", "")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_slot_shared_by_two_classes_cannot_be_an_end_of_two_relationships() {
        let body = "\
classes:
  A:
    is_a: Entity
    slots: [link]
  C:
    is_a: Entity
    slots: [link]
  B:
    is_a: Entity
slots:
  link:
    range: B
    annotations:
      ngsi_ld_kind: Relationship
";
        let e = errors(body);
        assert!(
            has(
                &e,
                "slots.link: slot-in-two-relationships: link is used by A and C"
            ),
            "{e:?}"
        );
        assert_eq!(e.len(), 1, "one entry for both ends: {e:?}");
    }

    #[test]
    fn a_missing_or_one_sided_inverse_is_named() {
        let none = "\
classes:
  A:
    is_a: Entity
    slots: [b]
  B:
    is_a: Entity
slots:
  b:
    range: B
    annotations:
      ngsi_ld_kind: Relationship
";
        let e = errors(none);
        assert_eq!(e, vec!["slots.b: inverse-missing: b (A → B) names no inverse; add the slot on B that points back".to_owned()]);
        let absent = none.replace("    range: B\n", "    range: B\n    inverse: gone\n");
        let e = errors(&absent);
        assert_eq!(
            e,
            vec![
                "slots.b: inverse-missing: b names the inverse gone, which B does not have"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn an_inverse_that_does_not_point_back_is_not_reciprocal() {
        let own = "\
classes:
  A:
    is_a: Entity
    slots: [me]
slots:
  me:
    range: A
    inverse: me
    annotations:
      ngsi_ld_kind: Relationship
";
        let e = errors(own);
        assert!(
            has(
                &e,
                "slots.me: inverse-not-reciprocal: me names itself as its inverse"
            ),
            "{e:?}"
        );
        // `there` is a Property, points elsewhere, or names another inverse: each is one-sided.
        for (b, why) in [
            ("    range: A\n    inverse: here\n", "a Property"),
            (
                "    range: B\n    inverse: here\n    annotations:\n      ngsi_ld_kind: Relationship\n",
                "another range",
            ),
            (
                "    range: A\n    inverse: other\n    annotations:\n      ngsi_ld_kind: Relationship\n",
                "another inverse",
            ),
        ] {
            let body = pair("", "")
                .split("  there:\n")
                .next()
                .unwrap_or_default()
                .to_owned()
                + "  there:\n"
                + b;
            let e = errors(&body);
            assert!(
                has(
                    &e,
                    "slots.here: inverse-not-reciprocal: here (A → B) names B.there as its inverse, which does not point back at A.here"
                ),
                "{why}: {e:?}"
            );
        }
    }

    #[test]
    fn the_computed_end_of_a_relationship_cannot_be_required() {
        // One-to-one: `here` comes first, so it is the source and `there` is computed.
        let e = errors(&pair("", "    required: true\n"));
        assert_eq!(e, vec!["slots.there: required-on-computed-end: there on B is computed on read and cannot be required; make the stored end required instead".to_owned()]);
        // The stored end may be required.
        assert_eq!(
            errors(&pair("    required: true\n", "")),
            Vec::<String>::new()
        );
        // One-to-many: the source holds many, the target one, so the source end is computed.
        let e = errors(&pair("    multivalued: true\n    required: true\n", ""));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(
            e[0].starts_with("slots.here: required-on-computed-end: here on A"),
            "{e:?}"
        );
        // on_delete names the source even when it comes second.
        let e = errors(&pair("    required: true\n", "      on_delete: cascade\n"));
        assert!(
            e[0].starts_with("slots.here: required-on-computed-end"),
            "{e:?}"
        );
    }

    #[test]
    fn attributes_and_slot_usage_ranges_take_part_in_relationships() {
        let body = "\
classes:
  A:
    is_a: Entity
    attributes:
      owner:
        range: B
        inverse: owned
        annotations:
          ngsi_ld_kind: Relationship
  B:
    is_a: Entity
    slots: [owned]
    slot_usage:
      owned:
        range: A
slots:
  owned:
    range: string
    inverse: owner
    annotations:
      ngsi_ld_kind: Relationship
";
        assert_eq!(errors(body), Vec::<String>::new());
        let broken = body.replace("        range: B\n", "        range: string\n");
        let e = errors(&broken);
        assert!(
            has(&e, "classes.A.attributes.owner: primitive-range"),
            "{e:?}"
        );
    }

    #[test]
    fn a_unit_mapping_needs_its_prefix_declared() {
        let body = "\
classes:
  A:
    is_a: Entity
    slots: [t]
slots:
  t:
    range: float
    unit:
      ucum_code: Cel
      exact_mappings: [qudt:DEG_C, https://qudt.org/vocab/unit/DEG_C, plain, city:degree]
      has_quantity_kind: quantitykind:Temperature
";
        let e = errors(body);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(
            e[0].starts_with("these unit mappings use a prefix the model does not declare"),
            "{e:?}"
        );
        assert!(
            e[0].ends_with("'t' → quantitykind:Temperature; 't' → qudt:DEG_C"),
            "sorted: {e:?}"
        );
        let declared = HEAD.replace(
            "prefixes:\n",
            "prefixes:\n  qudt: https://qudt.org/vocab/unit/\n  quantitykind: https://qudt.org/vocab/quantitykind/\n",
        ) + body;
        assert_eq!(compiled(&declared), Vec::<String>::new());
    }

    /// A cube: one VocabProperty dimension, one time dimension, one measure with a unit.
    const CUBE: &str = "\
classes:
  Reading:
    is_a: Entity
    slots: [station, day, level]
    annotations:
      qb_dsd: true
slots:
  station:
    required: true
    range: string
    annotations:
      ngsi_ld_kind: VocabProperty
      qb_component: dimension
  day:
    required: true
    range: date
    annotations:
      qb_component: dimension
  level:
    required: true
    range: float
    unit:
      ucum_code: m
    annotations:
      qb_component: measure
";

    #[test]
    fn a_data_structure_definition_with_its_dimensions_and_a_measure_compiles() {
        assert_eq!(errors(CUBE), Vec::<String>::new());
        // Without a default range either, a dimension without a range is a string term.
        assert_eq!(
            compiled(&format!(
                "{}{}",
                HEAD.replace("default_range: string\n", ""),
                CUBE.replace("    range: string\n", "")
            )),
            Vec::<String>::new()
        );
        // A dimension without a range is a string, as LinkML's default range is.
        assert_eq!(
            errors(&CUBE.replace("    range: string\n", "")),
            Vec::<String>::new()
        );
        assert_eq!(
            errors(
                &CUBE
                    .replace("qb_dsd: true", "qb_dsd: false")
                    .replace("      qb_component: dimension\n", "")
                    .replace("      qb_component: measure\n", "")
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_dsd_flag_is_true_or_false() {
        let e = errors(&CUBE.replace("qb_dsd: true", "qb_dsd: maybe"));
        assert!(
            has(
                &e,
                "class 'Reading' has qb_dsd 'maybe'; it is true or false"
            ),
            "{e:?}"
        );
        // The class is no cube, so its components belong to none.
        assert!(
            has(
                &e,
                "slot 'level' declares qb_component but no class using it is annotated qb_dsd: true"
            ),
            "{e:?}"
        );
    }

    #[test]
    fn each_component_rule_of_a_cube_is_named() {
        let place = |s: &str| format!("slot '{s}' of the DSD 'Reading'");
        let e = errors(&CUBE.replace("qb_component: measure", "qb_component: total"));
        assert!(
            has(
                &e,
                &format!(
                    "{} has qb_component 'total'; it is dimension or measure",
                    place("level")
                )
            ),
            "{e:?}"
        );
        assert!(
            has(
                &e,
                "the DSD 'Reading' needs at least one dimension and one measure"
            ),
            "{e:?}"
        );

        let e = errors(&CUBE.replacen("    required: true\n", "    multivalued: true\n", 1));
        assert!(
            has(
                &e,
                &format!("{} is a dimension and must be required", place("station"))
            ),
            "{e:?}"
        );
        assert!(
            has(
                &e,
                &format!(
                    "{} is a dimension and holds one value per observation, so it cannot be multivalued",
                    place("station")
                )
            ),
            "{e:?}"
        );

        let e = errors(&CUBE.replace("      ngsi_ld_kind: VocabProperty\n", ""));
        assert!(
            has(
                &e,
                &format!(
                    "{} is a dimension of kind Property with range string",
                    place("station")
                )
            ),
            "{e:?}"
        );

        let e = errors(
            &CUBE
                .replace("    range: string\n", "    range: date\n")
                .replace("      ngsi_ld_kind: VocabProperty\n", ""),
        );
        assert!(has(&e, "the DSD 'Reading' has 2 time dimensions"), "{e:?}");

        let e = errors(&CUBE.replace("    unit:\n      ucum_code: m\n", ""));
        assert!(
            has(
                &e,
                &format!(
                    "{} is a measure without a unit; give it one (DM-59)",
                    place("level")
                )
            ),
            "{e:?}"
        );

        let e = errors(&CUBE.replace(
            "      qb_component: measure\n",
            "      qb_component: measure\n      ngsi_ld_kind: JsonProperty\n",
        ));
        assert!(
            has(
                &e,
                &format!(
                    "{} is a measure of kind JsonProperty; a measure is a Property",
                    place("level")
                )
            ),
            "{e:?}"
        );
        assert!(
            e.iter()
                .all(|m| m.starts_with("the Data Structure Definition is not a cube: ")),
            "{e:?}"
        );
    }

    #[test]
    fn a_unit_with_no_field_set_is_no_unit() {
        let e = errors(&CUBE.replace("      ucum_code: m\n", "      symbol: \"\"\n"));
        assert!(has(&e, "is a measure without a unit"), "{e:?}");
        assert_eq!(
            errors(&CUBE.replace("      ucum_code: m\n", "      descriptive_name: metre\n")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_relationship_to_a_name_the_model_lacks_is_named() {
        let e = errors(
            "classes:\n  A:\n    is_a: Entity\n    slots: [to]\nslots:\n  to:\n    range: Nowhere\n    annotations:\n      ngsi_ld_kind: Relationship\n",
        );
        assert!(e.iter().any(|m| m.contains("Nowhere")), "{e:?}");
    }

    #[test]
    fn attribute_ends_find_their_own_attribute_and_may_share_a_name() {
        let body = "\
classes:
  A:
    is_a: Entity
    attributes:
      label:
        range: string
      link:
        range: B
        inverse: link
        annotations:
          ngsi_ld_kind: Relationship
  B:
    is_a: Entity
    attributes:
      link:
        range: A
        inverse: link
        annotations:
          ngsi_ld_kind: Relationship
";
        assert_eq!(errors(body), Vec::<String>::new());
    }

    #[test]
    fn of_two_attribute_ends_the_first_declared_is_stored_and_the_second_computed() {
        let body = "\
classes:
  A:
    is_a: Entity
    slots: [note]
    attributes:
      to:
        range: B
        inverse: back
        annotations:
          ngsi_ld_kind: Relationship
  B:
    is_a: Entity
    attributes:
      back:
        range: A
        inverse: to
        required: true
        annotations:
          ngsi_ld_kind: Relationship
slots:
  note:
    range: string
";
        let e = errors(body);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(
            e[0].starts_with("classes.B.attributes.back: required-on-computed-end: back on B"),
            "{e:?}"
        );
    }
}
