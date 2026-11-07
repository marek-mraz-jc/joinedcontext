// SPDX-License-Identifier: EUPL-1.2
//! The load: unknown input refused, not ignored (threat LK4), limits
//! checked before and after the parse, references resolved.

use linkml_model::{Error, Limits, Model};
use serde_json::{json, Value};

const LIMITS: Limits = Limits {
    max_bytes: 1 << 20,
    max_depth: 32,
    max_elements: 10_000,
};

fn base() -> Value {
    json!({
        "id": "https://example.org/s",
        "name": "s",
        "classes": {"Room": {"slots": ["size"]}},
        "slots": {"size": {"range": "integer"}},
        "enums": {"E": {"permissible_values": {"a": {}}}}
    })
}

fn load(doc: &Value) -> Result<Model, Error> {
    Model::read(doc.to_string().as_bytes(), &LIMITS)
}

fn with(pointer: &str, value: Value) -> Value {
    let mut doc = base();
    if let Some((parent, key)) = pointer.rsplit_once('/') {
        if let Some(o) = doc.pointer_mut(parent).and_then(Value::as_object_mut) {
            o.insert(key.to_owned(), value);
        }
    }
    doc
}

#[test]
fn the_base_document_loads() {
    load(&base()).unwrap();
}

#[test]
fn an_unknown_metaslot_fails_the_load() {
    // A metaslot of LinkML outside the subset, at every level.
    for pointer in [
        "/imports",
        "/classes/Room/mixins",
        "/classes/Room/attributes",
        "/slots/size/pattern",
        "/enums/E/enum_uri",
        "/enums/E/permissible_values/a/is_a",
        "/classes/Room/slot_usage/size",
    ] {
        let doc = if pointer.ends_with("slot_usage/size") {
            with(
                "/classes/Room/slot_usage",
                json!({"size": {"minimum_value": 0}}),
            )
        } else {
            with(pointer, json!("x"))
        };
        match load(&doc) {
            Err(Error::NotImplemented(why)) => {
                assert!(why.contains("unknown field"), "{pointer}: {why}");
            }
            other => panic!("{pointer}: {other:?}"),
        }
    }
}

/// T-3780: the two declarations the writer adds load; any other import or
/// default range is outside the subset (LK4).
#[test]
fn the_implied_declarations_load_and_no_other() {
    let mut doc = with("/imports", json!(["linkml:types"]));
    if let Some(o) = doc.as_object_mut() {
        o.insert("default_range".into(), json!("string"));
    }
    let declared = load(&doc).unwrap();
    assert_eq!(declared.schema(), load(&base()).unwrap().schema());
    for (key, value) in [
        ("imports", json!([])),
        ("imports", json!(["linkml:types", "other"])),
        ("imports", json!(["other"])),
        ("default_range", json!("integer")),
        ("default_range", json!(["string"])),
    ] {
        match load(&with(&format!("/{key}"), value.clone())) {
            Err(Error::NotImplemented(why)) => {
                assert!(why.contains(key), "{key} {value}: {why}");
            }
            other => panic!("{key} {value}: {other:?}"),
        }
    }
}

#[test]
fn a_range_outside_the_subset_is_not_implemented() {
    for range in ["ncname", "MyType", "curie"] {
        let doc = with("/slots/size/range", json!(range));
        assert!(
            matches!(load(&doc), Err(Error::NotImplemented(_))),
            "{range}"
        );
    }
}

#[test]
fn a_class_and_a_slot_with_one_name_is_not_implemented() {
    let doc = with("/slots/Room", json!({}));
    assert!(matches!(load(&doc), Err(Error::NotImplemented(_))));
}

#[test]
fn malformed_documents_are_malformed() {
    let cases = [
        ("not json", None),
        ("id missing", Some(json!({"name": "s"}))),
        (
            "wrong type",
            Some(with("/slots/size/required", json!("yes"))),
        ),
        ("relative id", Some(with("/id", json!("s")))),
        (
            "dangling is_a",
            Some(with("/classes/Room/is_a", json!("Nope"))),
        ),
        (
            "dangling slot",
            Some(with("/classes/Room/slots", json!(["size", "nope"]))),
        ),
        (
            "dangling inverse",
            Some(with("/slots/size/inverse", json!("nope"))),
        ),
        (
            "usage of a foreign slot",
            Some(with("/slots/other", json!({}))),
        ),
        (
            "name differs from key",
            Some(with("/slots/size/name", json!("other"))),
        ),
        (
            "prefix differs from key",
            Some(with(
                "/prefixes",
                json!({"p": {"prefix_prefix": "q", "prefix_reference": "https://e.org/"}}),
            )),
        ),
        (
            "prefix with a colon",
            Some(with(
                "/prefixes",
                json!({"p:q": {"prefix_prefix": "p:q", "prefix_reference": "https://e.org/"}}),
            )),
        ),
        (
            "prefix not an IRI",
            Some(with(
                "/prefixes",
                json!({"p": {"prefix_prefix": "p", "prefix_reference": "e.org"}}),
            )),
        ),
        (
            "undeclared default_prefix",
            Some(with("/default_prefix", json!("p"))),
        ),
        (
            "class and enum share a name",
            Some(with("/enums/Room", json!({}))),
        ),
        (
            "class named like a type",
            Some(with("/classes/string", json!({}))),
        ),
        (
            "NGSI-LD kind that is no kind name",
            Some(with(
                "/slots/size/annotations",
                json!({"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "some thing"}}),
            )),
        ),
        (
            "annotation tag differs",
            Some(with(
                "/slots/size/annotations",
                json!({"a": {"tag": "b", "value": "c"}}),
            )),
        ),
        (
            "bare name not IRI-safe",
            Some(with("/classes/Two words", json!({}))),
        ),
        (
            "meaning not an IRI",
            Some(with("/enums/E/permissible_values/a/meaning", json!("a b"))),
        ),
    ];
    for (what, doc) in cases {
        let got = match doc {
            None => Model::read(b"{not json", &LIMITS),
            Some(d) if what == "usage of a foreign slot" => {
                let mut d = d;
                d["classes"]["Room"]["slot_usage"] = json!({"other": {}});
                load(&d)
            }
            Some(d) => load(&d),
        };
        assert!(matches!(got, Err(Error::Malformed(_))), "{what}: {got:?}");
    }
}

/// DM-60: a cube role is `dimension` or `measure`, `qb_dsd` a flag; the
/// roles and the flag load.
#[test]
fn a_cube_annotation_outside_its_values_is_malformed() {
    let tagged = |tag: &str, value: &str| json!({ tag: {"tag": tag, "value": value} });
    for (pointer, tag, value) in [
        ("/slots/size/annotations", "qb_component", "slice"),
        ("/classes/Room/annotations", "qb_dsd", "yes"),
    ] {
        let got = load(&with(pointer, tagged(tag, value)));
        assert!(matches!(got, Err(Error::Malformed(_))), "{tag}: {got:?}");
    }
    for (pointer, tag, value) in [
        ("/slots/size/annotations", "qb_component", "dimension"),
        ("/slots/size/annotations", "qb_component", "measure"),
        ("/classes/Room/annotations", "qb_dsd", "false"),
    ] {
        assert!(
            load(&with(pointer, tagged(tag, value))).is_ok(),
            "{tag} {value}"
        );
    }
}

#[test]
fn an_is_a_cycle_is_malformed_and_does_not_loop() {
    let doc = json!({
        "id": "https://example.org/s", "name": "s",
        "classes": {"A": {"is_a": "B"}, "B": {"is_a": "C"}, "C": {"is_a": "A"}}
    });
    assert!(matches!(load(&doc), Err(Error::Malformed(why)) if why.contains("cycle")));
    let selfish =
        json!({"id": "https://example.org/s", "name": "s", "classes": {"A": {"is_a": "A"}}});
    assert!(matches!(load(&selfish), Err(Error::Malformed(_))));
}

#[test]
fn a_name_with_spaces_loads_when_it_has_its_own_uri() {
    let doc = with(
        "/classes/Two words",
        json!({"class_uri": "https://example.org/TwoWords"}),
    );
    let m = load(&doc).unwrap();
    assert_eq!(m.class_iri("Two words"), "https://example.org/TwoWords");
}

#[test]
fn the_size_bound_is_checked_before_the_parse() {
    let doc = base().to_string();
    let exact = Limits {
        max_bytes: doc.len(),
        ..LIMITS
    };
    Model::read(doc.as_bytes(), &exact).unwrap();
    let under = Limits {
        max_bytes: doc.len() - 1,
        ..LIMITS
    };
    assert_eq!(
        Model::read(doc.as_bytes(), &under),
        Err(Error::LimitExceeded {
            limit: "max_bytes",
            max: doc.len() - 1
        })
    );
    // Not even JSON: the bound answers first.
    let junk = vec![b'x'; 64];
    let tiny = Limits {
        max_bytes: 63,
        ..LIMITS
    };
    assert!(matches!(
        Model::read(&junk, &tiny),
        Err(Error::LimitExceeded {
            limit: "max_bytes",
            ..
        })
    ));
}

#[test]
fn the_depth_bound_is_checked_before_the_parse() {
    // Brackets inside strings do not count; real nesting does.
    let doc = with("/slots/size/description", json!("[[[[[[[[[[[[[[[[[[[[")).to_string();
    // the base document nests five deep: {enums {E {permissible_values {a {
    let depth = Limits {
        max_depth: 5,
        ..LIMITS
    };
    Model::read(doc.as_bytes(), &depth).unwrap();
    let deep = format!("{}{}", "[".repeat(6), "]".repeat(6));
    assert_eq!(
        Model::read(deep.as_bytes(), &depth),
        Err(Error::LimitExceeded {
            limit: "max_depth",
            max: 5
        })
    );
    // A depth serde_json itself would refuse never reaches it.
    let abyss = "[".repeat(100_000);
    assert!(matches!(
        Model::read(abyss.as_bytes(), &LIMITS),
        Err(Error::LimitExceeded {
            limit: "max_depth",
            ..
        })
    ));
}

#[test]
fn the_element_bound_counts_every_kind_of_element() {
    // 1 class + 1 slot + 1 enum + 1 slot reference + 1 permissible value
    let five = Limits {
        max_elements: 5,
        ..LIMITS
    };
    Model::read(base().to_string().as_bytes(), &five).unwrap();
    let four = Limits {
        max_elements: 4,
        ..LIMITS
    };
    assert_eq!(
        Model::read(base().to_string().as_bytes(), &four),
        Err(Error::LimitExceeded {
            limit: "max_elements",
            max: 4
        })
    );
    let annotated = with(
        "/slots/size/annotations",
        json!({"t": {"tag": "t", "value": "v"}}),
    );
    assert!(matches!(
        Model::read(annotated.to_string().as_bytes(), &five),
        Err(Error::LimitExceeded {
            limit: "max_elements",
            ..
        })
    ));
}

#[test]
fn errors_print_their_kind() {
    assert!(Error::Malformed("x".into())
        .to_string()
        .starts_with("malformed"));
    assert!(Error::NotImplemented("x".into())
        .to_string()
        .starts_with("outside"));
    assert_eq!(
        Error::LimitExceeded {
            limit: "max_depth",
            max: 3
        }
        .to_string(),
        "LinkML schema exceeds max_depth (3)"
    );
}

#[test]
fn iris_need_a_scheme_and_something_after_it() {
    for bad in [
        "ht_tp://x",
        "https:",
        "1x:y",
        "-x:y",
        "https://e.org/\u{7}",
        "https://e.org/\u{a0}",
    ] {
        let doc = with("/slots/size/slot_uri", json!(bad));
        assert!(matches!(load(&doc), Err(Error::Malformed(_))), "{bad}");
    }
    for good in ["urn:x:y", "https://e.org/a#b", "tag+x.y-z:1"] {
        load(&with("/slots/size/slot_uri", json!(good))).unwrap();
    }
}

#[test]
fn the_namespace_follows_the_id_or_the_default_prefix() {
    for (id, want) in [
        ("https://e.org/s", "https://e.org/s/size"),
        ("https://e.org/s/", "https://e.org/s/size"),
        ("https://e.org/s#", "https://e.org/s#size"),
    ] {
        assert_eq!(
            load(&with("/id", json!(id))).unwrap().slot_iri("size"),
            want
        );
    }
    let mut doc = with(
        "/prefixes",
        json!({"p": {"prefix_prefix": "p", "prefix_reference": "https://p.org/v#"}}),
    );
    doc["default_prefix"] = json!("p");
    let m = load(&doc).unwrap();
    assert_eq!(m.slot_iri("size"), "https://p.org/v#size");
    assert_eq!(m.expand("p:x"), "https://p.org/v#x");
    assert_eq!(
        m.expand("two words"),
        "two words",
        "no IRI: the text as it is"
    );
}

#[test]
fn class_slots_folds_ancestors_and_usage_and_is_empty_for_an_unknown_class() {
    let doc = json!({
        "id": "https://example.org/s", "name": "s",
        "classes": {
            "Thing": {"slots": ["name"], "slot_usage": {"name": {"annotations": {"a": {"tag": "a", "value": "1"}}}}},
            "Room": {"is_a": "Thing", "slots": ["name", "size"],
                     "slot_usage": {"name": {"slot_uri": "https://e.org/label", "range": "uri", "multivalued": true,
                                             "inlined": false, "inverse": "size", "unit": {"symbol": "m"},
                                             "annotations": {"b": {"tag": "b", "value": "2"}}}}}
        },
        "slots": {"name": {"description": "base"}, "size": {"range": "integer"}}
    });
    let m = load(&doc).unwrap();
    let slots = m.class_slots("Room");
    let names: Vec<&str> = slots.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        ["name", "size"],
        "the ancestor's slot first, no duplicate"
    );
    let name = &slots[0].1;
    assert_eq!(name.description.as_deref(), Some("base"));
    assert_eq!(name.range.as_deref(), Some("uri"));
    assert_eq!(name.inverse.as_deref(), Some("size"));
    assert_eq!(
        name.unit.as_ref().and_then(|u| u.symbol.as_deref()),
        Some("m")
    );
    assert_eq!(
        name.annotations.keys().collect::<Vec<_>>(),
        ["a", "b"],
        "usage annotations accumulate"
    );
    assert_eq!(m.class_slots("Nope"), Vec::new());
}

#[test]
fn every_builtin_type_round_trips_its_name() {
    for name in [
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
    ] {
        let b = linkml_model::Builtin::from_name(name).unwrap();
        assert_eq!(b.name(), name);
        assert!(b.xsd().starts_with("xsd:"));
    }
}

#[test]
fn one_plain_identifier_per_class_loads() {
    // T-3720: the slot that keys a class, and the Edm type of a string.
    let guid = json!({"edm_type": {"tag": "edm_type", "value": "Edm.Guid"}});
    let mut doc = base();
    doc["classes"]["Room"] = json!({"slots": ["code", "size"]});
    doc["slots"]["code"] = json!({"identifier": true, "annotations": guid});
    let m = load(&doc).unwrap();
    let slots = m.class_slots("Room");
    assert_eq!(slots[0].1.identifier, Some(true));
    assert_eq!(slots[1].1.identifier, None);
    // A child inherits the key; slot_usage may name it on a class without one.
    doc["classes"]["Hall"] = json!({"is_a": "Room"});
    doc["classes"]["Shed"] =
        json!({"slots": ["size"], "slot_usage": {"size": {"identifier": true}}});
    let m = load(&doc).unwrap();
    assert_eq!(m.class_slots("Hall")[0].1.identifier, Some(true));
    assert_eq!(m.class_slots("Shed")[0].1.identifier, Some(true));
}

#[test]
fn an_identifier_that_is_not_one_plain_value_is_malformed() {
    let room = |slots: Value, code: Value| {
        let mut doc = base();
        doc["classes"]["Room"] = json!({"slots": slots});
        doc["slots"]["code"] = code;
        doc
    };
    let cases = [
        (
            "two",
            room(json!(["code", "size"]), json!({"identifier": true})),
            Some("size"),
        ),
        (
            "inherited and own",
            room(json!(["code"]), json!({"identifier": true})),
            Some("child"),
        ),
        (
            "multivalued",
            room(
                json!(["code"]),
                json!({"identifier": true, "multivalued": true}),
            ),
            None,
        ),
        (
            "a class range",
            room(
                json!(["code"]),
                json!({"identifier": true, "range": "Room"}),
            ),
            None,
        ),
        (
            "an enum range",
            room(json!(["code"]), json!({"identifier": true, "range": "E"})),
            None,
        ),
        (
            "a float",
            room(
                json!(["code"]),
                json!({"identifier": true, "range": "float"}),
            ),
            None,
        ),
        (
            "another kind",
            room(
                json!(["code"]),
                json!({"identifier": true, "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}}),
            ),
            None,
        ),
        (
            "a Guid on an integer",
            room(
                json!(["code"]),
                json!({"range": "integer", "annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Guid"}}}),
            ),
            None,
        ),
        (
            "a duration key",
            room(
                json!(["code"]),
                json!({"identifier": true, "annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Duration"}}}),
            ),
            None,
        ),
        (
            "an Edm type outside the list",
            room(
                json!(["code"]),
                json!({"annotations": {"edm_type": {"tag": "edm_type", "value": "Edm.Byte"}}}),
            ),
            None,
        ),
    ];
    for (what, mut doc, extra) in cases {
        match extra {
            Some("size") => doc["slots"]["size"]["identifier"] = json!(true),
            Some("child") => {
                doc["classes"]["Hall"] = json!({"is_a": "Room", "slots": ["size"]});
                doc["slots"]["size"]["identifier"] = json!(true);
            }
            _ => {}
        }
        let got = load(&doc);
        assert!(matches!(got, Err(Error::Malformed(_))), "{what}: {got:?}");
    }
}

#[test]
fn edm_types_and_flags_load_only_where_they_fit() {
    // T-3722: each `edm_type` on the range it narrows, flags as "true"/"false".
    let tagged = |range: &str, tag: &str, value: &str| {
        let mut doc = base();
        doc["slots"]["size"] =
            json!({"range": range, "annotations": {tag: {"tag": tag, "value": value}}});
        doc
    };
    for (range, edm) in [
        ("integer", "Edm.Byte"),
        ("integer", "Edm.SByte"),
        ("integer", "Edm.Int16"),
        ("integer", "Edm.Int32"),
        ("float", "Edm.Single"),
        ("double", "Edm.Single"),
        ("string", "Edm.Guid"),
        ("string", "Edm.Duration"),
        ("string", "Edm.Binary"),
    ] {
        load(&tagged(range, "edm_type", edm)).unwrap();
    }
    for flag in ["odata_computed", "odata_immutable"] {
        load(&tagged("integer", flag, "true")).unwrap();
        load(&tagged("integer", flag, "false")).unwrap();
    }
    let mut doc = base();
    doc["classes"]["Room"]["annotations"] = json!({"odata_insertable": {"tag": "odata_insertable", "value": "false"}, "other": {"tag": "other", "value": "x"}});
    load(&doc).unwrap();

    let mut bad = vec![
        (
            "Single on integer",
            tagged("integer", "edm_type", "Edm.Single"),
        ),
        ("Int16 on float", tagged("float", "edm_type", "Edm.Int16")),
        ("Int64 named", tagged("integer", "edm_type", "Edm.Int64")),
        (
            "Guid on a boolean",
            tagged("boolean", "edm_type", "Edm.Guid"),
        ),
        ("Guid on an enum", tagged("E", "edm_type", "Edm.Guid")),
        ("computed yes", tagged("integer", "odata_computed", "yes")),
        ("immutable empty", tagged("integer", "odata_immutable", "")),
    ];
    let mut class = base();
    class["classes"]["Room"]["annotations"] =
        json!({"odata_deletable": {"tag": "odata_deletable", "value": "no"}});
    bad.push(("class flag no", class));
    let mut differs = base();
    differs["classes"]["Room"]["annotations"] = json!({"a": {"tag": "b", "value": "c"}});
    bad.push(("class tag differs", differs));
    for (what, doc) in bad {
        let got = load(&doc);
        assert!(matches!(got, Err(Error::Malformed(_))), "{what}: {got:?}");
    }
}

#[test]
fn slot_iris_by_class_include_inherited_slots_and_a_class_without_any() {
    let doc = json!({
        "id": "https://example.org/s", "name": "s",
        "classes": {
            "Place": {"slots": ["size"]},
            "Room": {"is_a": "Place", "slots": ["floor"]},
            "Marker": {}
        },
        "slots": {"size": {"range": "integer"}, "floor": {"range": "integer"}}
    });
    let m = load(&doc).unwrap();
    let by_class = m.slot_iris_by_class();
    let iris = |names: &[&str]| names.iter().map(|n| m.slot_iri(n)).collect();
    assert_eq!(by_class.len(), 3);
    assert_eq!(by_class[&m.class_iri("Place")], iris(&["size"]));
    assert_eq!(by_class[&m.class_iri("Room")], iris(&["size", "floor"]));
    assert!(by_class[&m.class_iri("Marker")].is_empty());
}

/// `renamed` takes an IRI by name and own IRI, leaves a name answered `None` as it was, and
/// leaves the source model untouched; `class_slots` sees the renamed slot.
#[test]
fn renamed_gives_the_named_iris_and_keeps_the_rest() {
    let doc = with("/classes/Hall", json!({"slots": ["size"]}));
    let m = load(&doc).expect("loads");
    let r = m.renamed(
        |name, own| (name == "Room").then(|| format!("{own}-x")),
        |name, _| (name == "size").then(|| "https://o.org/size".to_owned()),
    );
    assert_eq!(r.class_iri("Room"), format!("{}-x", m.class_iri("Room")));
    assert_eq!(r.class_iri("Hall"), m.class_iri("Hall"));
    assert_eq!(r.slot_iri("size"), "https://o.org/size");
    assert_ne!(
        m.slot_iri("size"),
        "https://o.org/size",
        "the source is unchanged"
    );
    let names: Vec<&str> = r.class_slots("Room").into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["size"]);
}

/// T-3752: `as_shown` gives a class or slot the IRI its short name expands to when `shown`
/// holds that IRI and not its own; one shown by its own IRI, or not shown at all, keeps its own.
#[test]
fn as_shown_rebases_only_what_is_shown_by_its_short_name() {
    let m = load(&json!({"id": "https://e.org/s/", "name": "s",
        "prefixes": {"sdm": {"prefix_prefix": "sdm", "prefix_reference": "https://sdm.org/"}},
        "classes": {"Car": {"class_uri": "sdm:Car", "slots": ["speed", "colour"]},
                    "Bus": {"class_uri": "sdm:Bus"}, "Tram": {"class_uri": "sdm:Tram"}},
        "slots": {"speed": {"slot_uri": "sdm:speed"}, "colour": {"slot_uri": "sdm:colour"}}}))
    .expect("loads");
    let expand = |name: &str| format!("https://v.org/{name}");
    let shown = std::collections::BTreeMap::from([
        (
            expand("Car"),
            std::collections::BTreeSet::from([
                expand("speed"),
                "https://sdm.org/colour".to_owned(),
            ]),
        ),
        (
            "https://sdm.org/Bus".to_owned(),
            std::collections::BTreeSet::new(),
        ),
    ]);
    let v = m.as_shown(&shown, expand);
    assert_eq!(v.class_iri("Car"), "https://v.org/Car");
    assert_eq!(
        v.class_iri("Bus"),
        "https://sdm.org/Bus",
        "shown by its own IRI"
    );
    assert_eq!(
        v.class_iri("Tram"),
        "https://sdm.org/Tram",
        "not shown at all"
    );
    assert_eq!(v.slot_iri("speed"), "https://v.org/speed");
    assert_eq!(v.slot_iri("colour"), "https://sdm.org/colour");
    let none = m.as_shown(&std::collections::BTreeMap::new(), expand);
    assert_eq!(none.class_iri("Car"), "https://sdm.org/Car");
}

/// T-5207: what `gen-linkml` 1.12 writes for `tests/fixtures/gen-linkml-source.yaml`, in its JSON
/// form and its YAML form read as JSON (`dev/gen-linkml-fixtures.sh` regenerates both), loads with
/// the classes, slots, kinds and flags the source states.
#[test]
fn real_gen_linkml_output_loads() {
    for (name, text) in [
        ("json", include_str!("fixtures/gen-linkml.json")),
        ("yaml", include_str!("fixtures/gen-linkml-yaml.json")),
    ] {
        let m = Model::read(text.as_bytes(), &LIMITS).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let s = m.schema();
        assert_eq!(s.classes.len(), 3, "{name}");
        assert_eq!(s.slots.len(), 7, "{name}");
        assert_eq!(m.kind(&s.slots["organizer"]), "Relationship", "{name}");
        assert_eq!(
            s.slots["capacity"].annotations["odata_computed"].value, "true",
            "{name}: a YAML bool is the string the flag reads"
        );
        assert_eq!(
            s.slots["temperature"].title.as_deref(),
            Some("Temperature"),
            "{name}"
        );
        assert_eq!(s.slots["temperature"].rank, Some(3), "{name}");
    }
}

/// T-5207: every listed non-semantic metaslot is dropped at every level; a meaning-changing one
/// beside it is still refused by name.
#[test]
fn non_semantic_metaslots_are_ignored_and_semantic_ones_refused() {
    let mut doc = base();
    for pointer in [
        "",
        "/classes/Room",
        "/slots/size",
        "/enums/E",
        "/enums/E/permissible_values/a",
    ] {
        for key in linkml_model::NON_SEMANTIC {
            if pointer == "/slots/size" && matches!(key, "rank" | "title") {
                continue;
            }
            if let Some(o) = doc.pointer_mut(pointer).and_then(Value::as_object_mut) {
                o.insert(key.to_owned(), json!("x"));
            }
        }
    }
    assert_eq!(
        load(&doc).unwrap().schema(),
        load(&base()).unwrap().schema()
    );
    for (pointer, value) in [
        ("/slots/size/any_of", json!([{"range": "string"}])),
        (
            "/classes/Room/attributes",
            json!({"other": {"range": "string"}}),
        ),
        (
            "/types",
            json!({"string": {"uri": "xsd:string"}, "money": {"uri": "xsd:decimal"}}),
        ),
    ] {
        let mut bad = doc.clone();
        if let Some((parent, key)) = pointer.rsplit_once('/') {
            if let Some(o) = bad.pointer_mut(parent).and_then(Value::as_object_mut) {
                o.insert(key.to_owned(), value);
            }
        }
        match load(&bad) {
            Err(Error::NotImplemented(why)) => {
                assert!(why.contains("unknown field"), "{pointer}: {why}");
            }
            other => panic!("{pointer}: {other:?}"),
        }
    }
}

/// T-5207: a kind a later NGSI-LD version adds loads; the slot states it and the model
/// reports it as unmapped, so each surface degrades that slot alone.
#[test]
fn a_new_ngsi_ld_kind_loads_and_is_not_known() {
    let doc = with(
        "/slots/size/annotations",
        json!({"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "TimeSeriesProperty"}}),
    );
    let m = load(&doc).unwrap();
    let kind = m.kind(&m.schema().slots["size"]);
    // the YAML shorthand of an annotation, and a number, read as `{tag, value}` strings
    let short = with(
        "/slots/size/annotations",
        json!({"ngsi_ld_kind": "GeoProperty", "rank_hint": 3}),
    );
    let read = load(&short).unwrap();
    let size = &read.schema().slots["size"];
    assert_eq!(read.kind(size), "GeoProperty");
    assert_eq!(size.annotations["rank_hint"].value, "3");
    assert_eq!(kind, "TimeSeriesProperty");
    assert!(!linkml_model::known_kind(kind));
    let schema: Value = serde_json::from_str(&linkml_model::json_schema(&m)).unwrap();
    let size = &schema["definitions"]["Room"]["properties"]["size"];
    assert_eq!(size.get("type"), None, "any value: {size}");
    assert_eq!(size["x-ngsi-ld-kind"], "TimeSeriesProperty");
    let context: Value = serde_json::from_str(&linkml_model::context(&m)).unwrap();
    assert!(
        context["@context"]["size"].is_string(),
        "no coercion: {context}"
    );
}

/// T-5207: a kind value that is no kind name, and a document that is no object, stay malformed.
#[test]
fn a_kind_that_is_no_name_and_a_non_object_are_malformed() {
    let spaced = with(
        "/slots/size/annotations",
        json!({"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "Time Series"}}),
    );
    for doc in [spaced, json!(["s"])] {
        assert!(matches!(load(&doc), Err(Error::Malformed(_))), "{doc}");
    }
}
