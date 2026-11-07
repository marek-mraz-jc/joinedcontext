// SPDX-License-Identifier: EUPL-1.2
//! The writers against golden files, and names that try to break out of
//! the format they are printed in (threat LK3).

use linkml_model::{Limits, Model};

const LIMITS: Limits = Limits {
    max_bytes: 1 << 20,
    max_depth: 32,
    max_elements: 10_000,
};

type Writer = fn(&Model) -> String;

const WRITERS: [(&str, Writer); 7] = [
    ("linkml.yaml", linkml_model::yaml),
    ("schema.json", linkml_model::json_schema),
    ("context.jsonld", linkml_model::context),
    ("shacl.ttl", linkml_model::shacl),
    ("owl.ttl", linkml_model::owl),
    ("rdf.ttl", linkml_model::rdfs),
    ("md", linkml_model::markdown),
];

fn golden_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// `LINKML_GOLDEN_UPDATE=1 cargo test -p linkml-model --test emit` rewrites
/// the files; the diff is then reviewed like code.
#[test]
fn every_writer_matches_its_golden_file() {
    let dir = golden_dir();
    let model = Model::read(&std::fs::read(dir.join("building.json")).unwrap(), &LIMITS).unwrap();
    let update = std::env::var_os("LINKML_GOLDEN_UPDATE").is_some();
    for (ext, write) in WRITERS {
        let path = dir.join(format!("building.{ext}"));
        let got = write(&model);
        assert_eq!(got, write(&model), "{ext}: same model, same bytes");
        if update {
            std::fs::write(&path, &got).unwrap();
        }
        let want = std::fs::read_to_string(&path).unwrap();
        assert_eq!(got, want, "{ext} differs from {}", path.display());
    }
}

#[test]
fn the_json_writers_emit_json() {
    let model = Model::read(
        &std::fs::read(golden_dir().join("building.json")).unwrap(),
        &LIMITS,
    )
    .unwrap();
    let schema: serde_json::Value =
        serde_json::from_str(&linkml_model::json_schema(&model)).unwrap();
    let room = &schema["definitions"]["Room"];
    // inherited from Asset, refined by Room's slot_usage
    assert_eq!(
        room["properties"]["name"]["description"],
        "The room's \"label\"."
    );
    assert_eq!(room["required"], serde_json::json!(["name", "floor"]));
    assert_eq!(room["properties"]["sensors"]["type"], "array");
    assert_eq!(
        room["properties"]["sensors"]["x-ngsi-ld-kind"],
        "Relationship"
    );
    assert_eq!(
        room["properties"]["status"]["enum"],
        serde_json::json!(["active", "retired"])
    );
    let ctx: serde_json::Value = serde_json::from_str(&linkml_model::context(&model)).unwrap();
    assert_eq!(ctx["@context"]["Room"], "https://schema.org/Room");
    assert_eq!(
        ctx["@context"]["floor"],
        serde_json::json!({"@id": "https://example.org/building#floor",
                           "@type": "http://www.w3.org/2001/XMLSchema#integer"})
    );
    assert_eq!(
        ctx["@context"]["name"], "https://schema.org/name",
        "a string: no coercion"
    );
    assert!(
        ctx["@context"].get("schema").is_none(),
        "no prefix terms (LK6)"
    );
}

/// T-3780: an Entity in keyValues, read with L4, is what L3 and L5 describe:
/// a Relationship holds the target's `@id`, a float its datatype, and a
/// `GeoProperty` a `GeoJSON` geometry, an object and never a string.
#[test]
fn a_view_s_data_reads_as_its_documents_describe_it() {
    let doc = serde_json::json!({
        "id": "https://e.org/s", "name": "s",
        "classes": {"Station": {"slots": ["temperature", "location", "nearest"]}},
        "slots": {
            "temperature": {"range": "float"},
            "location": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
            "nearest": {"range": "Station"}
        }
    });
    let model = Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap();
    let ctx: serde_json::Value = serde_json::from_str(&linkml_model::context(&model)).unwrap();
    let terms = &ctx["@context"];
    assert_eq!(
        terms["nearest"],
        serde_json::json!({"@id": "https://e.org/s/nearest", "@type": "@id"})
    );
    assert_eq!(
        terms["temperature"]["@type"],
        "http://www.w3.org/2001/XMLSchema#float"
    );
    assert_eq!(terms["location"], "https://e.org/s/location", "no coercion");
    let schema: serde_json::Value =
        serde_json::from_str(&linkml_model::json_schema(&model)).unwrap();
    let location = &schema["definitions"]["Station"]["properties"]["location"];
    assert_eq!(location["type"], "object", "{location}");
    let shapes = linkml_model::shacl(&model);
    let geo = shapes
        .split("sh:property [")
        .find(|p| p.contains("sh:name \"location\""))
        .unwrap();
    assert!(!geo.contains("sh:datatype"), "{geo}");
    assert!(shapes.contains("sh:datatype xsd:float"), "{shapes}");
}

/// Names and texts that would end a YAML scalar, open a mapping, an anchor,
/// a tag or a comment, a Turtle literal or IRI, or a Markdown link.
const HOSTILE: [&str; 8] = [
    "a: b",
    "x # comment",
    "&anchor *alias",
    "!!python/object",
    "\"quoted\" 'single'",
    "back\\slash",
    "- [list]",
    "{flow: map}",
];

#[test]
fn names_are_quoted_and_cannot_inject_yaml() {
    for (i, name) in HOSTILE.iter().enumerate() {
        let doc = serde_json::json!({
            "id": "https://example.org/s",
            "name": name,
            "version": name,
            "classes": {"C": {"description": name, "slots": [name], "slot_usage": {*name: {"description": name}}}},
            "slots": {*name: {"slot_uri": "https://example.org/p", "description": format!("{name}\nline two\u{2028}")}},
            "enums": {"E": {"permissible_values": {*name: {"description": name}}}}
        });
        let model =
            Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap_or_else(|e| panic!("{i}: {e}"));
        let yaml = linkml_model::yaml(&model);
        // Every line is a quoted key, a quoted list item or a closing marker:
        // nothing the schema supplied stands outside quotes.
        for line in yaml.lines() {
            let t = line.trim_start();
            assert!(
                t.starts_with('"') || t.starts_with("- \""),
                "{i}: unquoted line {line:?}"
            );
        }
        assert_eq!(
            yaml.lines().count(),
            yaml.matches('\n').count(),
            "{i}: no raw line break inside a scalar"
        );
        let ttl = linkml_model::owl(&model);
        assert!(ttl.contains(&format!(
            "rdfs:label \"{}\"",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        )));
        let md = linkml_model::markdown(&model);
        assert!(
            !md.replace("\\[", "").contains('['),
            "{i}: an unescaped [ in {md}"
        );
    }
}

#[test]
fn a_control_character_in_a_name_is_refused_at_load() {
    for bad in ["new\nline", "tab\there", "nul\u{0}", "esc\u{1b}[31m", ""] {
        let doc =
            serde_json::json!({"id": "https://example.org/s", "name": "s", "slots": {bad: {}}});
        let err = Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap_err();
        assert!(
            matches!(err, linkml_model::Error::Malformed(_)),
            "{bad:?}: {err}"
        );
    }
}

#[test]
fn an_iri_that_would_break_turtle_is_refused_at_load() {
    for bad in [
        "https://e.org/a> . <x",
        "https://e.org/a b",
        "1bad:scheme",
        ":empty",
        "https://e.org/\"q\"",
    ] {
        let doc = serde_json::json!({"id": "https://example.org/s", "name": "s", "classes": {"C": {"class_uri": bad}}});
        let err = Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap_err();
        assert!(
            matches!(err, linkml_model::Error::Malformed(_)),
            "{bad:?}: {err}"
        );
    }
}

#[test]
fn every_builtin_type_maps_in_json_schema_and_turtle() {
    let cases = [
        ("string", "string", None, "xsd:string"),
        ("integer", "integer", None, "xsd:integer"),
        ("float", "number", None, "xsd:float"),
        ("double", "number", None, "xsd:double"),
        ("decimal", "number", None, "xsd:decimal"),
        ("boolean", "boolean", None, "xsd:boolean"),
        ("date", "string", Some("date"), "xsd:date"),
        ("datetime", "string", Some("date-time"), "xsd:dateTime"),
        ("time", "string", Some("time"), "xsd:time"),
        ("uri", "string", Some("uri"), "xsd:anyURI"),
        ("uriorcurie", "string", None, "xsd:anyURI"),
    ];
    for (range, kind, format, xsd) in cases {
        let doc = serde_json::json!({
            "id": "https://example.org/s", "name": "s",
            "classes": {"C": {"slots": ["v"]}},
            "slots": {"v": {"range": range, "unit": {"ucum_code": "m"},
                            "annotations": {"note": {"tag": "note", "value": "kept"}}}}
        });
        let model = Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap();
        let schema: serde_json::Value =
            serde_json::from_str(&linkml_model::json_schema(&model)).unwrap();
        let v = &schema["definitions"]["C"]["properties"]["v"];
        assert_eq!(v["type"], kind, "{range}");
        assert_eq!(v["format"].as_str(), format, "{range}");
        assert_eq!(
            v["x-ngsi-ld-kind"], "Property",
            "{range}: a tag other than ngsi_ld_kind is no kind"
        );
        assert!(
            linkml_model::shacl(&model).contains(&format!("sh:datatype {xsd}")),
            "{range}"
        );
        assert!(
            linkml_model::rdfs(&model).contains(&format!("rdfs:range {xsd}")),
            "{range}"
        );
        assert!(
            linkml_model::yaml(&model).contains("\"unit\":\n      \"ucum_code\": \"m\"\n"),
            "{range}"
        );
    }
}

/// The population cube of the platform's Model Tools fixture
/// (`population-cube.linkml.yaml`, DM-60), resolved: three dimensions, a
/// measure, and `source`, which has no role.
fn population() -> Model {
    let role = |r: &str| serde_json::json!({"qb_component": {"tag": "qb_component", "value": r}});
    let doc = serde_json::json!({
        "id": "https://banskabystrica.sk/ns/population", "name": "population",
        "prefixes": {"bb": {"prefix_prefix": "bb", "prefix_reference": "https://banskabystrica.sk/ns/"}},
        "default_prefix": "bb",
        "classes": {
            "PopulationObservation": {
                "description": "The number of inhabitants of one district in one age band at the end of one year.",
                "slots": ["refDistrict", "ageBand", "year", "population", "source"],
                "annotations": {"qb_dsd": {"tag": "qb_dsd", "value": "true"}}
            },
            "District": {"slots": ["source"]},
            "Bare": {"slots": ["source"], "annotations": {"qb_dsd": {"tag": "qb_dsd", "value": "true"}}}
        },
        "slots": {
            "refDistrict": {"range": "uriorcurie", "required": true, "annotations": role("dimension")},
            "ageBand": {"range": "AgeBand", "required": true, "annotations": role("dimension")},
            "year": {"range": "integer", "required": true, "annotations": role("dimension")},
            "population": {"range": "integer", "required": true, "annotations": role("measure"),
                           "description": "Inhabitants with permanent residence."},
            "source": {"range": "string"}
        },
        "enums": {"AgeBand": {"permissible_values": {"0-4": {}, "65+": {}}}}
    });
    Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap_or_else(|e| unreachable!("{e}"))
}

/// DM-60: the cube of a Data Structure Definition names every component by
/// its role, the OWL types them; a model without one has no cube.
#[test]
fn the_cube_names_each_component_by_its_role() {
    const BB: &str = "https://banskabystrica.sk/ns/";
    let m = population();
    let cube = linkml_model::qb(&m).unwrap();
    assert!(cube.contains("@prefix qb: <http://purl.org/linked-data/cube#> ."));
    assert!(cube.contains(&format!(
        "<{BB}PopulationObservationStructure> a qb:DataStructureDefinition"
    )));
    for dimension in ["refDistrict", "ageBand", "year"] {
        assert!(
            cube.contains(&format!("[ qb:dimension <{BB}{dimension}> ]")),
            "{cube}"
        );
        assert!(cube.contains(&format!(
            "<{BB}{dimension}> a rdf:Property, qb:DimensionProperty"
        )));
    }
    assert!(cube.contains(&format!("[ qb:measure <{BB}population> ]")));
    assert!(cube.contains(&format!(
        "<{BB}population> a rdf:Property, qb:MeasureProperty"
    )));
    assert!(cube.contains(&format!(
        "<{BB}PopulationObservation> rdfs:subClassOf qb:Observation"
    )));
    assert!(
        !cube.contains(&format!("<{BB}source>")) && !cube.contains(&format!("<{BB}District>")),
        "{cube}"
    );

    // a Data Structure Definition without a description or a component
    assert!(
        cube.contains(&format!(
            "<{BB}BareStructure> a qb:DataStructureDefinition ;\n  rdfs:label \"Bare\" .\n"
        )),
        "{cube}"
    );
    let owl = linkml_model::owl(&m);
    assert!(owl.contains("rdfs:subClassOf qb:Observation"), "{owl}");
    assert!(owl.contains(&format!(
        "<{BB}year> a owl:DatatypeProperty, qb:DimensionProperty"
    )));
    assert!(owl.contains(&format!(
        "<{BB}population> a owl:DatatypeProperty, qb:MeasureProperty"
    )));
    assert!(
        owl.contains(&format!("<{BB}source> a owl:DatatypeProperty ;")),
        "{owl}"
    );
    let shacl = linkml_model::shacl(&m);
    for component in ["refDistrict", "ageBand", "year", "population"] {
        let at = shacl.find(&format!("sh:path <{BB}{component}>")).unwrap();
        let shape = &shacl[at..shacl[at..].find(']').map_or(shacl.len(), |end| at + end)];
        assert!(shape.contains("sh:minCount 1"), "{component}: {shape}");
    }

    let building = Model::read(
        &std::fs::read(golden_dir().join("building.json")).unwrap(),
        &LIMITS,
    )
    .unwrap();
    assert_eq!(linkml_model::qb(&building), None);
    assert!(!linkml_model::owl(&building).contains("qb:"));
}

/// T-4672 (DM-98…DM-103): the SHACL carries the display order as
/// `sh:order`, the key attributes and the rest as two `sh:PropertyGroup`s,
/// each label as a language-tagged `sh:name`, and `rank`/`title` survive the
/// LinkML round trip. A tag that is no language tag never reaches Turtle.
#[test]
fn shacl_carries_the_display_order_groups_and_labels() {
    let doc = serde_json::json!({
        "id": "https://e.org/s", "name": "s",
        "classes": {
            "Thing": {"slots": ["located"]},
            "Event": {
                "is_a": "Thing",
                "slots": ["id", "name", "description", "startDate", "located"],
                "slot_usage": {
                    "startDate": {"rank": 1, "annotations": {
                        "summary": {"tag": "summary", "value": "true"},
                        "label": {"tag": "label", "value": "{\"sk\":\"Začiatok\",\"en\":\"Start\",\"x\\\" ; a <y>\":\"bad\",\"\":\"empty\",\"abcdefghij-abcdefghij-abcdefghij-abcd\":\"long\"}"}
                    }},
                    "name": {"annotations": {"summary": {"tag": "summary", "value": "true"}}}
                }
            }
        },
        "slots": {
            "id": {}, "name": {"title": "Name"}, "description": {}, "startDate": {"range": "datetime"},
            "located": {}
        }
    });
    let model = Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap();
    let shapes = linkml_model::shacl(&model);
    let event = shapes.split("<https://e.org/s/EventShape>").nth(1).unwrap();
    let event = event.split(" .\n").next().unwrap();
    let order: Vec<&str> = event
        .split("sh:property [")
        .skip(1)
        .map(|p| {
            p.split("sh:name \"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(order, ["startDate", "name", "description", "located", "id"]);
    let property = |name: &str| {
        event
            .split("sh:property [")
            .find(|p| p.contains(&format!("sh:name \"{name}\"")))
            .unwrap()
    };
    let start = property("startDate");
    assert!(
        start.contains("sh:name \"startDate\", \"Start\"@en, \"Začiatok\"@sk"),
        "{start}"
    );
    assert!(
        start.contains("sh:order 0") && start.contains("SummaryGroup>"),
        "{start}"
    );
    assert!(
        !start.contains("bad") && !start.contains("empty") && !start.contains("long"),
        "a hostile, empty or over-long language tag is skipped: {start}"
    );
    assert!(property("name").contains("SummaryGroup>"));
    assert!(
        property("description").contains("sh:order 2")
            && property("description").contains("DetailGroup>")
    );
    assert!(
        property("id").contains("DetailGroup>"),
        "id stays last and out of the summary"
    );
    assert_eq!(shapes.matches("a sh:PropertyGroup").count(), 2);

    let yaml = linkml_model::yaml(&model);
    assert!(
        yaml.contains("\"rank\": 1") && yaml.contains("\"title\": \"Name\""),
        "{yaml}"
    );
}

/// T-4672: a class that marks no key attribute takes its first four that
/// are not hidden.
#[test]
fn without_summary_annotations_the_first_four_shown_slots_are_the_summary() {
    let doc = serde_json::json!({
        "id": "https://e.org/s", "name": "s",
        "classes": {"Room": {"slots": ["a", "b", "c", "d", "e"],
            "slot_usage": {"b": {"annotations": {"hidden": {"tag": "hidden", "value": "true"}}}}}},
        "slots": {"a": {}, "b": {}, "c": {}, "d": {}, "e": {}}
    });
    let model = Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap();
    let shapes = linkml_model::shacl(&model);
    let summary: Vec<&str> = shapes
        .split("sh:property [")
        .skip(1)
        .filter(|p| p.contains("SummaryGroup>"))
        .map(|p| {
            p.split("sh:name \"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(summary, ["a", "c", "d", "e"]);
}

/// T-4803: a permissible value keeps its annotations, so a reader of the view's LinkML names it
/// in the reader's language ("cesta III. triedy"), not by its English description.
#[test]
fn a_permissible_value_keeps_its_label_annotation() {
    let label = r#"{"sk": "cesta III. triedy", "en": "third-class road"}"#;
    let doc = serde_json::json!({
        "id": "https://example.org/s",
        "name": "s",
        "enums": {"RoadClass": {"permissible_values": {"thirdClass": {
            "text": "thirdClass",
            "description": "A third-class road (cesta III. triedy).",
            "annotations": {"label": {"tag": "label", "value": label}}
        }}}}
    });
    let model = Model::read(doc.to_string().as_bytes(), &LIMITS).unwrap();
    let yaml = linkml_model::yaml(&model);
    let value = yaml
        .split("\"thirdClass\":")
        .nth(1)
        .expect("the value is written");
    assert!(value.contains("\"annotations\":"), "{yaml}");
    assert!(value.contains("\"tag\": \"label\""), "{yaml}");
    assert!(value.contains(r#"\"sk\": \"cesta III. triedy\""#), "{yaml}");
}

/// T-5206 finding 3: the `@context` and the JSON Schema follow each NGSI-LD kind before the range
/// (TS 104 175 5.2.6.4.6 to 5.2.6.4.10), so an Entity in keyValues reads under the context as
/// stored and validates against the schema.
#[test]
fn each_ngsi_ld_kind_shapes_the_context_and_the_json_schema() {
    let kind = |k: &str| serde_json::json!({"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": k}});
    let doc = serde_json::json!({
        "id": "https://e.org/s/", "name": "s",
        "classes": {"Room": {"slots": ["prop", "rel", "geo", "lang", "vocab", "list", "links",
                                       "json", "address", "uri_rel", "place"]},
                    "PostalAddress": {}},
        "slots": {
            "prop": {"range": "integer"},
            "rel": {"range": "Room"},
            "geo": {"annotations": kind("GeoProperty")},
            "lang": {"annotations": kind("LanguageProperty")},
            "vocab": {"annotations": kind("VocabProperty")},
            "list": {"range": "float", "annotations": kind("ListProperty")},
            "links": {"range": "Room", "annotations": kind("ListRelationship")},
            "json": {"annotations": kind("JsonProperty")},
            "address": {"range": "PostalAddress", "inlined": true, "annotations": kind("Property")},
            "uri_rel": {"range": "uriorcurie", "annotations": kind("Relationship")},
            "place": {"range": "PostalAddress", "inlined": true}
        }
    });
    let m = Model::read(doc.to_string().as_bytes(), &LIMITS).expect("loads");
    let c: serde_json::Value = serde_json::from_str(&linkml_model::context(&m)).expect("json");
    let c = &c["@context"];
    assert_eq!(
        c["prop"]["@type"],
        "http://www.w3.org/2001/XMLSchema#integer"
    );
    assert_eq!(c["rel"]["@type"], "@id");
    assert_eq!(c["geo"], "https://e.org/s/geo");
    assert_eq!(c["lang"]["@container"], "@language");
    assert_eq!(c["vocab"]["@type"], "@vocab");
    assert_eq!(c["list"]["@container"], "@list");
    assert_eq!(c["links"]["@container"], "@list");
    assert_eq!(c["links"]["@type"], "@id");
    assert_eq!(c["json"]["@type"], "@json");
    assert_eq!(
        c["address"], "https://e.org/s/address",
        "an inlined object is no IRI"
    );
    assert_eq!(
        c["uri_rel"]["@type"], "@id",
        "a Relationship is an IRI node, not xsd:anyURI"
    );

    let j: serde_json::Value = serde_json::from_str(&linkml_model::json_schema(&m)).expect("json");
    let p = &j["definitions"]["Room"]["properties"];
    assert_eq!(p["prop"]["type"], "integer");
    assert_eq!(p["rel"]["format"], "uri");
    assert_eq!(p["geo"]["type"], "object");
    assert_eq!(p["lang"]["type"], "object");
    assert_eq!(p["lang"]["additionalProperties"]["type"], "string");
    assert_eq!(p["vocab"]["type"], "string");
    assert_eq!(p["list"]["type"], "array");
    assert_eq!(p["list"]["items"]["type"], "number");
    assert_eq!(p["links"]["type"], "array");
    assert_eq!(p["json"].get("type"), None, "a JsonProperty holds any JSON");
    assert_eq!(p["address"]["type"], "object");
    assert_eq!(p["address"]["x-ngsi-ld-kind"], "Property");
    assert_eq!(
        p["place"]["x-ngsi-ld-kind"], "Property",
        "an inlined class is no link"
    );
    assert_eq!(c["place"], "https://e.org/s/place");
}
