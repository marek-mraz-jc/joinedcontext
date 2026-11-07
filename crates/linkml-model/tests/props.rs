// SPDX-License-Identifier: EUPL-1.2
//! The YAML emitter round trip (threat LK3): whatever names and texts a
//! loaded model holds, the emitted YAML parses back, with a real YAML
//! reader, to the same schema.

use std::collections::BTreeMap;

use linkml_model::{
    Annotation, Class, Enum, Limits, Model, PermissibleValue, Prefix, Schema, Slot, Unit,
};
use proptest::prelude::*;

const LIMITS: Limits = Limits {
    max_bytes: 1 << 20,
    max_depth: 32,
    max_elements: 10_000,
};

/// A name suffix with YAML, Turtle and Markdown punctuation and non-ASCII
/// letters; no control character (refused at load).
fn suffix() -> impl Strategy<Value = String> {
    "[a-z0-9 :#&*!|>%@`'\"\\\\{}\\[\\],?-]{0,6}|[äπ漢😀]{0,3}"
}

/// Any text at all, and text dense in what YAML treats specially: line
/// breaks (`\n`, `\r`, NEL, LS, PS), tabs, quotes, backslashes, BOM.
fn text() -> impl Strategy<Value = Option<String>> {
    proptest::option::of(prop_oneof![
        any::<String>(),
        "[a-z \n\r\t\"'\\\\#:&*!\u{0}\u{1b}\u{85}\u{2028}\u{2029}\u{feff}]{0,12}",
    ])
}

const RANGES: [&str; 5] = ["string", "integer", "datetime", "boolean", "uriorcurie"];
const KINDS: [&str; 3] = ["Property", "GeoProperty", "LanguageProperty"];

#[derive(Debug, Clone)]
struct ClassShape {
    suffix: String,
    description: Option<String>,
    parent: Option<usize>,
}

#[derive(Debug, Clone)]
struct SlotShape {
    suffix: String,
    description: Option<String>,
    range: usize,
    required: Option<bool>,
    multivalued: Option<bool>,
    inverse: Option<usize>,
    annotated: bool,
}

#[derive(Debug, Clone)]
struct EnumShape {
    suffix: String,
    description: Option<String>,
    values: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone)]
struct Shape {
    name: String,
    version: Option<String>,
    classes: Vec<ClassShape>,
    slots: Vec<SlotShape>,
    enums: Vec<EnumShape>,
}

fn shape() -> impl Strategy<Value = Shape> {
    let class = (suffix(), text(), proptest::option::of(0usize..4)).prop_map(
        |(suffix, description, parent)| ClassShape {
            suffix,
            description,
            parent,
        },
    );
    let slot = (
        (suffix(), text(), 0usize..12),
        (
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(0usize..3),
            any::<bool>(),
        ),
    )
        .prop_map(
            |((suffix, description, range), (required, multivalued, inverse, annotated))| {
                SlotShape {
                    suffix,
                    description,
                    range,
                    required,
                    multivalued,
                    inverse,
                    annotated,
                }
            },
        );
    let enumeration = (
        suffix(),
        text(),
        proptest::collection::vec((suffix(), text()), 0..3),
    )
        .prop_map(|(suffix, description, values)| EnumShape {
            suffix,
            description,
            values,
        });
    (
        "[a-z][a-z0-9 :#&*!-]{0,8}",
        text(),
        proptest::collection::vec(class, 0..4),
        proptest::collection::vec(slot, 0..5),
        proptest::collection::vec(enumeration, 0..3),
    )
        .prop_map(|(name, version, classes, slots, enums)| Shape {
            name,
            version,
            classes,
            slots,
            enums,
        })
}

/// The `n`-th of a list, wrapping; `None` for an empty list.
fn pick(list: &[String], n: usize) -> Option<String> {
    n.checked_rem(list.len()).and_then(|i| list.get(i)).cloned()
}

fn even(n: usize) -> bool {
    n.checked_rem(2) == Some(0)
}

fn slots_of(s: &Shape, names: &[String], ranges: &[String]) -> BTreeMap<String, Slot> {
    s.slots
        .iter()
        .zip(names)
        .enumerate()
        .map(|(i, (shape, name))| {
            let annotations = if shape.annotated {
                let kind = KINDS
                    .iter()
                    .cycle()
                    .nth(i)
                    .map_or("Property", |k| *k)
                    .to_owned();
                BTreeMap::from([(
                    "ngsi_ld_kind".into(),
                    Annotation {
                        tag: "ngsi_ld_kind".into(),
                        value: kind,
                    },
                )])
            } else {
                BTreeMap::new()
            };
            let slot = Slot {
                slot_uri: Some(format!("https://example.org/p/{i}")),
                range: pick(ranges, shape.range),
                required: shape.required,
                multivalued: shape.multivalued,
                inverse: shape.inverse.and_then(|j| pick(names, j)),
                unit: shape.annotated.then(|| Unit {
                    ucum_code: Some("Cel".into()),
                    symbol: shape.description.clone(),
                    exact_mappings: vec!["ex:CEL".into()],
                }),
                annotations,
                description: shape.description.clone(),
                ..Slot::default()
            };
            (name.clone(), slot)
        })
        .collect()
}

fn classes_of(s: &Shape, names: &[String], slot_names: &[String]) -> BTreeMap<String, Class> {
    s.classes
        .iter()
        .zip(names)
        .enumerate()
        .map(|(i, (shape, name))| {
            // a parent only among the earlier classes: no cycle
            let is_a = shape
                .parent
                .filter(|p| *p < i)
                .and_then(|p| names.get(p))
                .cloned();
            let own: Vec<String> = slot_names.iter().skip(i).step_by(2).cloned().collect();
            let usage = Slot {
                required: Some(true),
                description: shape.description.clone(),
                ..Slot::default()
            };
            let slot_usage = own
                .first()
                .map(|n| (n.clone(), usage))
                .into_iter()
                .collect();
            let class = Class {
                name: even(i).then(|| name.clone()),
                class_uri: Some(format!("ex:c{i}")),
                is_a,
                slots: own,
                slot_usage,
                // the class annotations round-trip as the slot ones do
                annotations: even(i)
                    .then(|| {
                        let tag = "odata_deletable".to_owned();
                        let value = "false".to_owned();
                        (tag.clone(), Annotation { tag, value })
                    })
                    .into_iter()
                    .collect(),
                description: shape.description.clone(),
            };
            (name.clone(), class)
        })
        .collect()
}

fn enums_of(s: &Shape, names: &[String]) -> BTreeMap<String, Enum> {
    s.enums
        .iter()
        .zip(names)
        .map(|(shape, name)| {
            let permissible_values = shape
                .values
                .iter()
                .enumerate()
                .map(|(j, (t, d))| {
                    let key = format!("v{j}{t}");
                    let v = PermissibleValue {
                        text: even(j).then(|| key.clone()),
                        description: d.clone(),
                        meaning: Some(format!("ex:v{j}")),
                        annotations: BTreeMap::default(),
                    };
                    (key, v)
                })
                .collect();
            let e = Enum {
                name: None,
                description: shape.description.clone(),
                permissible_values,
            };
            (name.clone(), e)
        })
        .collect()
}

/// A schema every load check accepts: indices pick parents, ranges and
/// inverses only among what exists, names are unique by their prefix.
fn schema(s: &Shape) -> Schema {
    let class_names: Vec<String> = s
        .classes
        .iter()
        .enumerate()
        .map(|(i, c)| format!("C{i}{}", c.suffix))
        .collect();
    let slot_names: Vec<String> = s
        .slots
        .iter()
        .enumerate()
        .map(|(i, c)| format!("s{i}{}", c.suffix))
        .collect();
    let enum_names: Vec<String> = s
        .enums
        .iter()
        .enumerate()
        .map(|(i, e)| format!("E{i}{}", e.suffix))
        .collect();
    let ranges: Vec<String> = RANGES
        .iter()
        .map(|r| (*r).to_owned())
        .chain(class_names.iter().cloned())
        .chain(enum_names.iter().cloned())
        .collect();
    Schema {
        id: "https://example.org/s".into(),
        name: s.name.clone(),
        version: s.version.clone(),
        prefixes: BTreeMap::from([(
            "ex".into(),
            Prefix {
                prefix_prefix: "ex".into(),
                prefix_reference: "https://example.org/ex/".into(),
            },
        )]),
        default_prefix: Some("ex".into()),
        classes: classes_of(s, &class_names, &slot_names),
        slots: slots_of(s, &slot_names, &ranges),
        enums: enums_of(s, &enum_names),
    }
}

proptest! {
    // Miri interprets every case: 8 there keep the weekly job inside its
    // budget, 256 everywhere else.
    #![proptest_config(ProptestConfig {
        cases: if cfg!(miri) { 8 } else { 256 },
        ..ProptestConfig::default()
    })]

    #[test]
    fn emitted_yaml_parses_back_to_the_same_model(s in shape()) {
        let model = Model::new(schema(&s), &LIMITS).unwrap();
        let yaml = linkml_model::yaml(&model);
        let back: Schema = serde_saphyr::from_str(&yaml)
            .unwrap_or_else(|e| panic!("emitted YAML does not parse: {e}\n{yaml}"));
        prop_assert_eq!(&back, model.schema());
        // and the parsed copy is a model that emits the same bytes
        let again = Model::new(back, &LIMITS).unwrap();
        prop_assert_eq!(linkml_model::yaml(&again), yaml);
    }

    #[test]
    fn every_writer_is_deterministic_and_total(s in shape()) {
        let model = Model::new(schema(&s), &LIMITS).unwrap();
        for write in [linkml_model::yaml, linkml_model::json_schema, linkml_model::context,
                      linkml_model::shacl, linkml_model::owl, linkml_model::rdfs, linkml_model::markdown] {
            prop_assert_eq!(write(&model), write(&model));
        }
        let _: serde_json::Value = serde_json::from_str(&linkml_model::json_schema(&model)).unwrap();
        let _: serde_json::Value = serde_json::from_str(&linkml_model::context(&model)).unwrap();
    }
}
