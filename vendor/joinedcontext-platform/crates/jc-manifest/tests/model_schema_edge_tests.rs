//! The compiled model schema at its edges (T-7794): ingest modes and personal data per class, a
//! class schema that does not compile, every form a relationship target arrives in, the shape
//! rules a record breaks and what a record that is no entity gets back.

use jc_manifest::model_schema::{ModelSchema, Problem};
use serde_json::{json, Value};

fn schema() -> Value {
    json!({ "$defs": {
        "Person": { "type": "object", "required": ["employer"], "properties": {
            "id": {"type": "string"}, "type": {"type": "string"},
            "name": {"type": "string", "x-personal-data": true, "minLength": 2, "maxLength": 5},
            "email": {"type": "string", "x-personal-data": true, "x-ingest": "hash", "pattern": "^[a-z]+@[a-z]+$"},
            "nick": {"type": "string", "x-ingest": "keep"},
            "age": {"type": ["integer", "null"], "minimum": 0, "maximum": 150},
            "role": {"type": "string", "enum": ["staff", "guest"]},
            "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 2},
            "kind": {"const": "human"},
            "height": {"type": "number", "x-unit": {"exactMappings": ["unece:MTR"]}},
            "weight": {"type": "number", "x-unit": {"exactMappings": ["ucefact:"]}},
            "employer": {"type": "string", "x-ngsi-ld-relationship": {"target": "https://example.org/Organization"}},
            "friends": {"type": "array", "items": {"type": "string"}, "x-ngsi-ld-relationship": {"target": "Person"}},
            "homepage": {"type": "string", "x-ngsi-ld-relationship": {}}
        }},
        "Broken": { "type": "object", "properties": {
            "id": {"type": "string"}, "type": {"type": "string"},
            "code": {"type": "string", "pattern": "("}
        }}
    }})
}

fn model() -> ModelSchema {
    ModelSchema::compile(
        "people",
        "1.0.0",
        &schema(),
        &["Person".to_owned(), "Broken".to_owned()],
        false,
    )
}

fn person(extra: Value) -> Value {
    let mut record = json!({
        "id": "urn:ngsi-ld:Person:p1",
        "type": "Person",
        "employer": {"type": "Relationship", "object": "urn:ngsi-ld:Organization:o1"}
    });
    for (key, value) in extra.as_object().expect("an object") {
        record[key] = value.clone();
    }
    record
}

fn rules(problems: &[Problem]) -> Vec<(&str, &str)> {
    problems
        .iter()
        .map(|p| (p.rule.as_str(), p.path.as_str()))
        .collect()
}

#[test]
fn ingest_lists_every_mode_but_keep_and_personal_data_by_name() {
    let model = model();
    assert_eq!(
        model.ingest("Person"),
        vec![("email".to_owned(), "hash".to_owned())]
    );
    assert_eq!(model.personal_attributes("Person"), vec!["email", "name"]);
    assert!(model.ingest("Nobody").is_empty());
    assert!(model.personal_attributes("Nobody").is_empty());
}

#[test]
fn two_compilations_of_one_model_are_equal_and_of_another_are_not() {
    assert_eq!(model(), model());
    let other = ModelSchema::compile("people", "2.0.0", &schema(), &["Person".to_owned()], false);
    assert_ne!(model(), other);
}

#[test]
fn a_class_whose_schema_does_not_compile_says_so_for_each_record() {
    let problems = model().check(&json!({
        "id": "urn:ngsi-ld:Broken:b1",
        "type": "Broken",
        "code": {"type": "Property", "value": "x"}
    }));
    assert!(
        problems.iter().any(|p| p.rule == "sh:node"
            && p.message == "the schema of Broken in model people does not compile"),
        "{problems:?}"
    );
}

#[test]
fn a_record_that_is_no_entity_gets_one_problem_naming_what_is_missing() {
    let model = model();
    assert_eq!(rules(&model.check(&json!([1]))), vec![("sh:node", "")]);
    assert_eq!(
        rules(&model.check(&json!({"id": "x"}))),
        vec![("sh:minCount", "type")]
    );
    let unknown = model.check(&json!({"id": "urn:ngsi-ld:Car:1", "type": "Car"}));
    assert_eq!(
        unknown[0].message,
        "Car is not a class of the space's data model people (DM-61)"
    );
    let no_id = model.check(&json!({"type": "Person", "employer": "urn:ngsi-ld:Organization:o1"}));
    assert!(rules(&no_id).contains(&("sh:minCount", "id")), "{no_id:?}");
    let bare_prefix = model.check(&person(json!({"id": "urn:ngsi-ld:Person:"})));
    assert!(
        bare_prefix.iter().any(|p| p.path == "id"),
        "{bare_prefix:?}"
    );
}

#[test]
fn normalize_leaves_a_record_that_is_no_object_as_it_is() {
    assert_eq!(model().normalize("Person", &json!("text")), json!("text"));
    assert_eq!(model().normalize("Person", &json!([1, 2])), json!([1, 2]));
}

#[test]
fn every_shape_rule_a_value_breaks_is_named_by_its_shacl_rule() {
    let model = model();
    let cases = [
        (
            json!({"name": {"type": "Property", "value": "a"}}),
            "sh:minLength",
            "name",
        ),
        (
            json!({"name": {"type": "Property", "value": "abcdefg"}}),
            "sh:maxLength",
            "name",
        ),
        (
            json!({"email": {"type": "Property", "value": "NOT AN EMAIL"}}),
            "sh:pattern",
            "email",
        ),
        (
            json!({"age": {"type": "Property", "value": -1}}),
            "sh:minInclusive",
            "age",
        ),
        (
            json!({"age": {"type": "Property", "value": 200}}),
            "sh:maxInclusive",
            "age",
        ),
        (
            json!({"age": {"type": "Property", "value": "old"}}),
            "sh:datatype",
            "age",
        ),
        (
            json!({"role": {"type": "Property", "value": "boss"}}),
            "sh:in",
            "role",
        ),
        (
            json!({"tags": {"type": "Property", "value": []}}),
            "sh:minCount",
            "tags",
        ),
        (
            json!({"tags": {"type": "Property", "value": ["a", "b", "c"]}}),
            "sh:maxCount",
            "tags",
        ),
        (
            json!({"kind": {"type": "Property", "value": "robot"}}),
            "sh:hasValue",
            "kind",
        ),
        (
            json!({"height": {"type": "Property", "value": 1.8, "unitCode": "CMT"}}),
            "ngsi-ld:unitCode",
            "height",
        ),
    ];
    for (extra, rule, attribute) in cases {
        let problems = model.check(&person(extra.clone()));
        assert!(
            problems
                .iter()
                .any(|p| p.rule == rule && p.path.starts_with(attribute)),
            "{extra}: {problems:?}"
        );
        assert!(
            problems
                .iter()
                .all(|p| !p.message.contains("NOT AN EMAIL") && !p.message.contains("boss")),
            "a problem never echoes the value: {problems:?}"
        );
    }
}

#[test]
fn a_unit_problem_names_the_model_s_code_and_its_symbol() {
    let problems = model().check(&person(
        json!({"height": {"type": "Property", "value": 1.8, "unitCode": "CMT"}}),
    ));
    let unit = problems
        .iter()
        .find(|p| p.rule == "ngsi-ld:unitCode")
        .expect("a unit problem");
    assert!(
        unit.message.contains("is measured in MTR"),
        "{}",
        unit.message
    );
    assert!(
        unit.message.ends_with("write unitCode MTR (DM-06)"),
        "{}",
        unit.message
    );
    // An empty `ucefact:` mapping is no unit at all.
    let free = model().check(&person(
        json!({"weight": {"type": "Property", "value": 70, "unitCode": "KGM"}}),
    ));
    assert!(
        free.iter()
            .all(|p| p.path != "weight.unitCode" && p.rule != "ngsi-ld:unitCode"),
        "{free:?}"
    );
}

#[test]
fn a_required_end_absent_or_nulled_is_refused() {
    let model = model();
    for employer in [
        json!(null),
        json!("urn:ngsi-ld:null"),
        json!({"type": "Relationship", "object": "urn:ngsi-ld:null"}),
    ] {
        let mut record = person(json!({}));
        record["employer"] = employer.clone();
        let problems = model.check(&record);
        assert!(
            problems
                .iter()
                .any(|p| p.rule == "required-end-missing" && p.path == "employer"),
            "{employer}: {problems:?}"
        );
    }
}

#[test]
fn relationship_targets_are_read_in_every_form_a_write_uses() {
    let model = model();
    let ok = [
        json!([{"type": "Relationship", "object": "urn:ngsi-ld:Person:a", "datasetId": "urn:d:1"},
               {"type": "Relationship", "object": "urn:ngsi-ld:Person:b", "datasetId": "urn:d:2"}]),
        json!({"type": "ListRelationship", "object": [{"object": "urn:ngsi-ld:Person:a"}, {"@id": "urn:ngsi-ld:Person:b"}, "urn:ngsi-ld:Person:c"]}),
    ];
    for friends in ok {
        let problems = model.check(&person(json!({"friends": friends.clone()})));
        assert!(
            problems.iter().all(|p| p.path != "friends"
                || !p.rule.contains("-end-") && p.rule != "target-wrong-type"),
            "{friends}: {problems:?}"
        );
    }
    let wrong = model.check(&person(json!({"friends": {"type": "Relationship", "object": ["urn:ngsi-ld:Person:a", "urn:ngsi-ld:Car:b"]}})));
    let problem = wrong
        .iter()
        .find(|p| p.rule == "target-wrong-type")
        .expect("a wrong target");
    assert_eq!(
        problem.message,
        "friends points at a Person, and target 2 of the record is not one (DM-70)"
    );
    let two = model.check(&person(json!({"employer": [
        {"type": "Relationship", "object": "urn:ngsi-ld:Organization:a", "datasetId": "urn:d:1"},
        {"type": "Relationship", "object": "urn:ngsi-ld:Organization:b", "datasetId": "urn:d:2"}
    ]})));
    assert!(
        two.iter().any(|p| p.rule == "single-end-many-targets"),
        "{two:?}"
    );
    // A member with no object, or a number, names no target: nothing to judge.
    for odd in [json!({"type": "Relationship"}), json!(7)] {
        let problems = model.check(&person(json!({"friends": odd.clone()})));
        assert!(
            problems
                .iter()
                .all(|p| p.path != "friends" || p.rule != "target-wrong-type"),
            "{odd}: {problems:?}"
        );
    }
}
