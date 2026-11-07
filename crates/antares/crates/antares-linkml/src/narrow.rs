// SPDX-License-Identifier: EUPL-1.2
//! The view's schema as one caller may see it (odata-linkml-mapping.md
//! section 8 row L2, threats LK1 and LK6): the classes whose type the
//! caller may see, each with the slots its type may show, `is_a` kept only
//! between visible classes, a range to a hidden class turned into
//! `uriorcurie`, `inverse` only when both ends are visible, and only the
//! enums and prefixes a kept element uses. Every document is rendered from
//! this narrowed model, never from the source (LK1).

use antares_ops::Visible;
use linkml_model::{
    Annotation, Builtin, Class, Error, Limits, Model, Prefix, Range, Schema, Slot, KIND_TAG,
};
use std::collections::{BTreeMap, BTreeSet};

/// The bounds the Tenant's schema is read under: 1 MiB whatever
/// `ANTARES_SCHEMA_MAX_BYTES` lets the schema table store (a larger schema
/// closes the surface, LK7), a nesting deep enough for LinkML's JSON form,
/// and as many elements as a large model holds.
pub(crate) const LIMITS: Limits = linkml_model::SURFACE_LIMITS;

/// A class's slots a caller may see, each as the class refines it.
type Shown = Vec<(String, Slot)>;

/// The name `visible` knows class `class` by, `None` when it shows it under neither: the
/// class's own IRI, or its name read under the core @context, which a view's rules, the engine
/// and the stored Entities use (ADR-0020). A schema whose `class_uri` is another vocabulary's
/// (`sdm:`) still shows the classes of a view written with short names (T-3645), as the OGC and
/// `OData` surfaces read it (T-3752). A slot is matched the same way, under the type found here.
fn as_viewed(source: &Model, visible: &Visible, class: &str) -> Option<String> {
    let own = source.class_iri(class);
    if visible.shows_type(&own) {
        Some(own)
    } else {
        visible.shows_type(class).then(|| class.to_owned())
    }
}

/// The source schema narrowed to `visible`, under the id
/// `<source id>/views/<view>`, so a narrowed document never claims to be the
/// full model (section 8 decision). The second value is whether anything of
/// the source was left out.
pub(crate) fn narrow(
    source: &Model,
    visible: &Visible,
    view: &str,
) -> Result<(Model, bool), Error> {
    let s = source.schema();
    // Each visible class with the name the view knows its type by.
    let typed: BTreeMap<&str, (&Class, String)> = s
        .classes
        .iter()
        .filter_map(|(c, class)| Some((c.as_str(), (class, as_viewed(source, visible, c)?))))
        .collect();
    let kept: BTreeSet<&str> = typed.keys().copied().collect();
    let shown = |class: &str, ty: &str| -> Shown {
        source
            .class_slots(class)
            .into_iter()
            .filter(|(name, _)| {
                visible.shows_attribute(ty, &source.slot_iri(name))
                    || visible.shows_attribute(ty, name)
            })
            .map(|(name, slot)| (name.to_owned(), slot))
            .collect()
    };
    let per_class: BTreeMap<&str, (&Class, Shown)> = typed
        .iter()
        .map(|(c, (class, ty))| (*c, (*class, shown(c, ty))))
        .collect();
    let slot_names: BTreeSet<&str> = per_class
        .values()
        .flat_map(|(_, slots)| slots)
        .map(|(n, _)| n.as_str())
        .collect();

    // An inlined value whose class is hidden is an open object (LinkML's `linkml:Any`), never
    // a URI: the stored value is an object (T-5206 finding 2).
    let any = if kept.contains(ANY) { "LinkmlAny" } else { ANY };
    let rewrite = |slot: &Slot| rewrite(source, &kept, &slot_names, any, slot);

    let mut classes = BTreeMap::new();
    for (name, (class, slots)) in &per_class {
        // `is_a` holds only when the parent is visible and shows nothing the
        // child does not: LinkML would otherwise hand the child the
        // parent's slots back.
        let own: BTreeSet<&str> = slots.iter().map(|(n, _)| n.as_str()).collect();
        let is_a = class.is_a.clone().filter(|p| {
            per_class
                .get(p.as_str())
                .is_some_and(|(_, ps)| ps.iter().all(|(n, _)| own.contains(n.as_str())))
        });
        let slot_usage = slots
            .iter()
            .filter_map(|(n, refined)| {
                let base = s.slots.get(n)?;
                let refined = rewrite(refined);
                (refined != rewrite(base)).then(|| (n.clone(), refined))
            })
            .collect();
        // The IRI the view reads the type by, as the OData surface states it (T-5206 finding 1):
        // a class shown only by its short name is that name under the core @context.
        let own = source.class_iri(name);
        let class_uri = if typed.get(name).is_some_and(|(_, ty)| *ty == own) {
            class.class_uri.clone()
        } else {
            Some(antares_ops::core_iri(name))
        };
        classes.insert(
            (*name).to_owned(),
            Class {
                name: class.name.clone(),
                class_uri,
                is_a,
                slots: slots.iter().map(|(n, _)| n.clone()).collect(),
                slot_usage,
                description: class.description.clone(),
                annotations: class.annotations.clone(),
            },
        );
    }
    let slots = kept_slots(source, visible, &typed, &per_class, &slot_names, rewrite);

    let enums = used_enums(source, &slots, &classes);

    // Left out: a hidden class, or a slot a kept class does not show. A slot
    // no class uses goes too; it names nothing a caller could see.
    let left_out = classes.len() != s.classes.len()
        || per_class
            .iter()
            .any(|(c, (_, slots))| slots.len() != source.class_slots(c).len());
    let mut narrowed = Schema {
        id: format!("{}/views/{view}", s.id.trim_end_matches(['/', '#'])),
        name: s.name.clone(),
        version: s.version.clone(),
        prefixes: BTreeMap::new(),
        default_prefix: s.default_prefix.clone(),
        classes,
        slots,
        enums,
    };
    let used = used_prefixes(&narrowed);
    narrowed.prefixes = s
        .prefixes
        .iter()
        .filter(|(k, _)| used.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    with_any(&mut narrowed, any);
    Ok((Model::new(narrowed, &LIMITS)?, left_out))
}

/// The kept slots, rewritten. A slot no kept class shows by its own IRI is shown by its short
/// name: its IRI is that name under the core @context, the one the Entities and the `OData`
/// surface use (T-5206 finding 1).
fn kept_slots(
    source: &Model,
    visible: &Visible,
    typed: &BTreeMap<&str, (&Class, String)>,
    per_class: &BTreeMap<&str, (&Class, Shown)>,
    slot_names: &BTreeSet<&str>,
    rewrite: impl Fn(&Slot) -> Slot,
) -> BTreeMap<String, Slot> {
    let s = source.schema();
    let by_own_iri = |n: &str| {
        typed.iter().any(|(c, (_, ty))| {
            visible.shows_attribute(ty, &source.slot_iri(n))
                && per_class
                    .get(c)
                    .is_some_and(|(_, shown)| shown.iter().any(|(k, _)| k == n))
        })
    };
    slot_names
        .iter()
        .filter_map(|n| {
            let mut slot = rewrite(s.slots.get(*n)?);
            if !by_own_iri(n) {
                slot.slot_uri = Some(antares_ops::core_iri(n));
            }
            Some(((*n).to_owned(), slot))
        })
        .collect()
}

/// `schema` with class `any` (`linkml:Any`) and the `linkml` prefix when a kept slot ranges over
/// it: an inlined object whose class the view hides (T-5206 finding 2).
fn with_any(schema: &mut Schema, any: &str) {
    let used = schema
        .slots
        .values()
        .chain(schema.classes.values().flat_map(|c| c.slot_usage.values()))
        .any(|slot| slot.range.as_deref() == Some(any));
    if !used {
        return;
    }
    schema.classes.insert(
        any.to_owned(),
        Class {
            class_uri: Some("linkml:Any".to_owned()),
            description: Some(
                "Any value: an inlined object whose class this view does not show.".to_owned(),
            ),
            ..Class::default()
        },
    );
    schema
        .prefixes
        .entry("linkml".to_owned())
        .or_insert_with(|| Prefix {
            prefix_prefix: "linkml".to_owned(),
            prefix_reference: "https://w3id.org/linkml/".to_owned(),
        });
}

/// The class an inlined slot whose class is hidden ranges over.
const ANY: &str = "Any";

/// The annotation naming the slot that holds a navigation's foreign key
/// (antares-odata `edm::REFERENCE_TAG`): the one annotation whose value names
/// another slot.
const REFERENCE_TAG: &str = "odata_referential_constraint";

/// `slot` as the narrowed schema states it: a range to a hidden class is
/// `uriorcurie` with the kind it implied stated (LK1), or `any` (`linkml:Any`)
/// when the slot holds the object inlined; an `inverse` to a hidden slot is dropped,
/// and so is a referential constraint naming one, so no annotation names a slot
/// the view hides.
fn rewrite(
    source: &Model,
    kept: &BTreeSet<&str>,
    slot_names: &BTreeSet<&str>,
    any: &str,
    slot: &Slot,
) -> Slot {
    let mut out = slot.clone();
    out.annotations
        .retain(|tag, a| tag != REFERENCE_TAG || slot_names.contains(a.value.as_str()));
    if let Range::Class(target) = source.range(slot) {
        let link = matches!(source.kind(slot), "Relationship" | "ListRelationship");
        if !kept.contains(target) && (slot.inlined == Some(true) || !link) {
            out.range = Some(any.to_owned());
            out.inlined = Some(true);
            out.inverse = None;
        } else if !kept.contains(target) {
            // Still a Relationship (DM-69, an external reference): the
            // kind the class range implied is stated, the class is not.
            let kind = source.kind(slot).to_owned();
            out.annotations
                .entry(KIND_TAG.to_owned())
                .or_insert(Annotation {
                    tag: KIND_TAG.to_owned(),
                    value: kind,
                });
            out.range = Some(Builtin::Uriorcurie.name().to_owned());
            out.inverse = None;
        }
    }
    if out
        .inverse
        .as_deref()
        .is_some_and(|inv| !slot_names.contains(inv))
    {
        out.inverse = None;
    }
    out
}

/// The enums a kept slot or slot usage ranges over.
fn used_enums(
    source: &Model,
    slots: &BTreeMap<String, Slot>,
    classes: &BTreeMap<String, Class>,
) -> BTreeMap<String, linkml_model::Enum> {
    let used: BTreeSet<&str> = slots
        .values()
        .chain(classes.values().flat_map(|c| c.slot_usage.values()))
        .filter_map(|slot| match source.range(slot) {
            Range::Enum(e) => Some(e),
            _ => None,
        })
        .collect();
    source
        .schema()
        .enums
        .iter()
        .filter(|(k, _)| used.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The prefixes a kept element names: the default prefix and the prefix of
/// every CURIE in a URI, a meaning or a unit mapping (LK6: a prefix no kept
/// element uses may name a hidden namespace).
fn used_prefixes(s: &Schema) -> BTreeSet<String> {
    let mut curies: Vec<&str> = s.default_prefix.iter().map(String::as_str).collect();
    let slots = s
        .slots
        .values()
        .chain(s.classes.values().flat_map(|c| c.slot_usage.values()));
    for slot in slots {
        curies.extend(slot.slot_uri.as_deref());
        if let Some(unit) = &slot.unit {
            curies.extend(unit.exact_mappings.iter().map(String::as_str));
        }
    }
    curies.extend(s.classes.values().filter_map(|c| c.class_uri.as_deref()));
    curies.extend(
        s.enums
            .values()
            .flat_map(|e| e.permissible_values.values())
            .filter_map(|v| v.meaning.as_deref()),
    );
    curies
        .into_iter()
        .map(|c| c.split_once(':').map_or(c, |(p, _)| p).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use antares_ops::policy::{Filter, Rule};

    fn model(json: &str) -> Model {
        Model::read(json.as_bytes(), &LIMITS).expect("a model")
    }

    const SOURCE: &str = r#"{
      "id": "https://e.org/city", "name": "city", "version": "2.1.0",
      "prefixes": {
        "city": {"prefix_prefix": "city", "prefix_reference": "https://e.org/city/"},
        "secret": {"prefix_prefix": "secret", "prefix_reference": "https://hidden.e.org/"},
        "unece": {"prefix_prefix": "unece", "prefix_reference": "http://unece.org/rec20/"}
      },
      "default_prefix": "city",
      "classes": {
        "Thing": {"slots": ["name"]},
        "Car": {"is_a": "Thing", "slots": ["speed", "owner", "fuel", "vin"],
                "slot_usage": {"speed": {"required": true}}},
        "Person": {"class_uri": "secret:Person", "slots": ["name", "car"]}
      },
      "slots": {
        "name": {},
        "speed": {"range": "float", "unit": {"ucum_code": "km/h", "exact_mappings": ["unece:KMH"]}},
        "owner": {"range": "Person", "inverse": "car"},
        "car": {"range": "Car", "inverse": "owner"},
        "fuel": {"range": "Fuel"}, "vin": {}
      },
      "enums": {"Fuel": {"permissible_values": {"diesel": {}}},
                "Secret": {"permissible_values": {"x": {}}}}
    }"#;

    fn visible(types: &[&str], attrs: &[&str]) -> Visible {
        let iri = |n: &&str| format!("https://e.org/city/{n}");
        antares_ops::Visible::from_filter(&Filter {
            rules: vec![Rule {
                types: types.iter().map(iri).collect(),
                attributes: attrs.iter().map(iri).collect(),
                ..Rule::default()
            }],
            ..Filter::default()
        })
    }

    #[test]
    fn a_hidden_class_goes_with_its_prefix_ranges_and_inverses() {
        let source = model(SOURCE);
        let (m, left_out) = narrow(
            &source,
            &visible(&["Car", "Thing"], &["name", "speed", "owner", "fuel"]),
            "cars",
        )
        .expect("narrowed");
        let s = m.schema();
        assert!(left_out);
        assert_eq!(s.id, "https://e.org/city/views/cars");
        assert_eq!(s.version.as_deref(), Some("2.1.0"));
        assert_eq!(s.classes.keys().collect::<Vec<_>>(), ["Car", "Thing"]);
        assert_eq!(
            s.prefixes.keys().collect::<Vec<_>>(),
            ["city", "unece"],
            "LK6: the unit's prefix stays, the hidden class's goes"
        );
        let owner = &s.slots["owner"];
        assert_eq!(owner.range.as_deref(), Some("uriorcurie"), "LK1");
        assert_eq!(owner.inverse, None);
        assert_eq!(m.kind(owner), "Relationship", "the kind survives the range");
        assert!(!s.slots.contains_key("vin"));
        assert!(!s.slots.contains_key("car"));
        assert_eq!(s.enums.keys().collect::<Vec<_>>(), ["Fuel"]);
        let car = &s.classes["Car"];
        assert_eq!(car.is_a.as_deref(), Some("Thing"));
        assert_eq!(car.slot_usage["speed"].required, Some(true));
        assert!(!linkml_model::yaml(&m).contains("Person"));
    }

    #[test]
    fn a_referential_constraint_naming_a_hidden_slot_goes() {
        let source = model(&SOURCE.replace(
            r#""owner": {"range": "Person", "inverse": "car"}"#,
            r#""owner": {"range": "Person", "inverse": "car", "annotations": {"odata_referential_constraint": {"tag": "odata_referential_constraint", "value": "vin"}}}"#,
        ));
        let hidden = visible(&["Car", "Thing"], &["name", "speed", "owner"]);
        let (m, _) = narrow(&source, &hidden, "cars").expect("narrowed");
        assert!(!m.schema().slots["owner"]
            .annotations
            .contains_key(REFERENCE_TAG));
        assert!(!linkml_model::yaml(&m).contains("vin"));
        let shown = visible(&["Car", "Thing"], &["name", "speed", "owner", "vin"]);
        let (m, _) = narrow(&source, &shown, "cars").expect("narrowed");
        assert_eq!(
            m.schema().slots["owner"].annotations[REFERENCE_TAG].value,
            "vin"
        );
    }

    #[test]
    fn is_a_is_cut_when_the_parent_shows_what_the_child_hides() {
        let source = model(SOURCE);
        let iri = |n: &str| format!("https://e.org/city/{n}");
        let v = antares_ops::Visible::from_filter(&Filter {
            rules: vec![
                Rule {
                    types: vec![iri("Thing")],
                    attributes: vec![iri("name")],
                    ..Rule::default()
                },
                Rule {
                    types: vec![iri("Car")],
                    attributes: vec![iri("speed")],
                    ..Rule::default()
                },
            ],
            ..Filter::default()
        });
        let (m, _) = narrow(&source, &v, "cars").expect("narrowed");
        let car = &m.schema().classes["Car"];
        assert_eq!(car.is_a, None);
        assert_eq!(car.slots, ["speed"]);
    }

    #[test]
    fn both_ends_visible_keep_the_relationship_whole() {
        let source = model(SOURCE);
        let v = antares_ops::Visible::from_filter(&Filter {
            rules: vec![Rule {
                types: vec![
                    "https://e.org/city/Car".into(),
                    "https://hidden.e.org/Person".into(),
                ],
                attributes: ["owner", "car", "name"]
                    .iter()
                    .map(|n| format!("https://e.org/city/{n}"))
                    .collect(),
                ..Rule::default()
            }],
            ..Filter::default()
        });
        let (m, _) = narrow(&source, &v, "all").expect("narrowed");
        let s = m.schema();
        assert_eq!(s.slots["owner"].range.as_deref(), Some("Person"));
        assert_eq!(s.slots["owner"].inverse.as_deref(), Some("car"));
        assert!(s.prefixes.contains_key("secret"), "Person's own prefix");
    }

    #[test]
    fn nothing_visible_is_an_empty_schema_not_an_error() {
        let (m, left_out) = narrow(&model(SOURCE), &Visible::default(), "none").expect("ok");
        assert!(left_out);
        assert!(m.schema().classes.is_empty());
        assert!(m.schema().slots.is_empty());
    }

    /// Every class and every slot, Person's under its own namespace.
    fn everything(attrs: &[&str]) -> Visible {
        antares_ops::Visible::from_filter(&Filter {
            rules: vec![Rule {
                types: [
                    "https://e.org/city/Thing",
                    "https://e.org/city/Car",
                    "https://hidden.e.org/Person",
                ]
                .map(str::to_owned)
                .to_vec(),
                attributes: attrs
                    .iter()
                    .map(|a| format!("https://e.org/city/{a}"))
                    .collect(),
                ..Rule::default()
            }],
            ..Filter::default()
        })
    }

    #[test]
    fn everything_visible_leaves_nothing_out() {
        let all = ["name", "speed", "owner", "car", "fuel", "vin"];
        let (m, left_out) = narrow(&model(SOURCE), &everything(&all), "all").expect("narrowed");
        assert!(!left_out);
        assert_eq!(m.schema().classes.len(), 3);
        let (_, left_out) =
            narrow(&model(SOURCE), &everything(&all[..5]), "all").expect("narrowed");
        assert!(left_out, "every class kept, one slot hidden");
    }

    #[test]
    fn an_inverse_to_a_hidden_slot_goes_and_the_range_stays() {
        let (m, _) =
            narrow(&model(SOURCE), &everything(&["owner", "name"]), "v").expect("narrowed");
        let owner = &m.schema().slots["owner"];
        assert_eq!(owner.range.as_deref(), Some("Person"));
        assert_eq!(owner.inverse, None);
    }

    /// T-3645: a schema in another vocabulary (`sdm:`) and a view written with short names, as
    /// the Portal writes them: the classes and slots the view names are shown, read under the
    /// core @context, the rest stays hidden, and the document keeps the model's own IRIs.
    #[test]
    fn a_schema_of_another_vocabulary_shows_what_the_view_names() {
        let source = model(
            r#"{"id": "https://e.org/m", "name": "m",
              "prefixes": {"sdm": {"prefix_prefix": "sdm",
                                   "prefix_reference": "https://smartdatamodels.org/"}},
              "default_prefix": "sdm",
              "classes": {"BikeHireDockingStation": {"slots": ["status", "secretNote"]},
                          "Depot": {"slots": ["status"]}},
              "slots": {"status": {}, "secretNote": {}}}"#,
        );
        let core = |n: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{n}");
        let v = antares_ops::Visible::from_filter(&Filter {
            rules: vec![Rule {
                types: vec![core("BikeHireDockingStation")],
                attributes: vec![core("status")],
                ..Rule::default()
            }],
            ..Filter::default()
        });
        assert!(!v.shows_type(&source.class_iri("BikeHireDockingStation")));
        let (m, left_out) = narrow(&source, &v, "bikes").expect("narrowed");
        assert!(left_out);
        let s = m.schema();
        assert_eq!(
            s.classes.keys().collect::<Vec<_>>(),
            ["BikeHireDockingStation"]
        );
        assert_eq!(s.slots.keys().collect::<Vec<_>>(), ["status"]);
        // T-5206 finding 1: the IRIs the view reads, the ones the OData surface states.
        assert_eq!(
            m.class_iri("BikeHireDockingStation"),
            core("BikeHireDockingStation")
        );
        assert_eq!(m.slot_iri("status"), core("status"));
        let shown = BTreeMap::from([(
            core("BikeHireDockingStation"),
            BTreeSet::from([core("status")]),
        )]);
        let odata = source.as_shown(&shown, antares_ops::core_iri);
        assert_eq!(m.slot_iri("status"), odata.slot_iri("status"));
        assert_eq!(
            m.class_iri("BikeHireDockingStation"),
            odata.class_iri("BikeHireDockingStation")
        );
        let context: serde_json::Value =
            serde_json::from_str(&linkml_model::context(&m)).expect("json");
        assert_eq!(context["@context"]["status"], core("status"));
    }

    /// T-5206 finding 2: an inlined object whose class the view hides stays an object of
    /// `linkml:Any` with its kind, never a `uriorcurie`; a hidden Relationship target still is one.
    #[test]
    fn an_inlined_value_of_a_hidden_class_stays_an_object() {
        let source = model(
            r#"{"id": "https://e.org/city/", "name": "city",
              "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://e.org/city/"}},
              "default_prefix": "city",
              "classes": {"Event": {"slots": ["address", "organizer", "venue"]},
                          "PostalAddress": {"slots": ["street"]}, "Place": {}},
              "slots": {"address": {"range": "PostalAddress", "inlined": true},
                        "organizer": {"range": "PostalAddress",
                                      "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "Property"}}},
                        "venue": {"range": "Place"}, "street": {}}}"#,
        );
        let (m, _) = narrow(
            &source,
            &visible(&["Event"], &["address", "organizer", "venue"]),
            "events",
        )
        .expect("narrowed");
        let s = m.schema();
        for slot in ["address", "organizer"] {
            assert_eq!(s.slots[slot].range.as_deref(), Some("Any"), "{slot}");
            assert_eq!(m.kind(&s.slots[slot]), "Property", "{slot}");
        }
        assert_eq!(s.slots["venue"].range.as_deref(), Some("uriorcurie"));
        assert_eq!(m.class_iri("Any"), "https://w3id.org/linkml/Any");
        assert!(!s.classes.contains_key("PostalAddress"), "LK1");
        let j: serde_json::Value =
            serde_json::from_str(&linkml_model::json_schema(&m)).expect("json");
        assert_eq!(
            j["definitions"]["Event"]["properties"]["address"]["type"],
            "object"
        );
        let c: serde_json::Value = serde_json::from_str(&linkml_model::context(&m)).expect("json");
        assert_eq!(c["@context"]["address"], "https://e.org/city/address");
        assert_eq!(c["@context"]["venue"]["@type"], "@id");
    }

    /// A visible class the schema itself names `Any` keeps its name: the stub is `LinkmlAny`.
    #[test]
    fn a_visible_class_named_any_keeps_its_name() {
        let source = model(
            r#"{"id": "https://e.org/city/", "name": "city",
              "classes": {"Event": {"slots": ["address"]}, "Any": {}, "PostalAddress": {}},
              "slots": {"address": {"range": "PostalAddress", "inlined": true}}}"#,
        );
        let (m, _) =
            narrow(&source, &visible(&["Event", "Any"], &["address"]), "events").expect("narrowed");
        let s = m.schema();
        assert_eq!(s.slots["address"].range.as_deref(), Some("LinkmlAny"));
        assert_eq!(m.class_iri("LinkmlAny"), "https://w3id.org/linkml/Any");
        assert!(
            s.classes["Any"].class_uri.is_none(),
            "the schema's own class"
        );
    }
}
