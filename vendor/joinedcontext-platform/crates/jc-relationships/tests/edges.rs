//! Edge cases of the relationship rules through the public API (T-7805): every refusal's
//! problem text, schemas that state no end, every value form a write may use, payloads that hold
//! no entity, unknown types and an empty model.

use jc_core::ProblemDetails;
use jc_relationships::{
    claimed_twice, claims_of, deleted_attribute, first_missing, objects, targets_of, unlink, Claim,
    Extent, OnDelete, RelationshipRules, Shape, StoredEnd, Targets, Unlink, Violation,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const SCHOOL: &str = "urn:ngsi-ld:School:hel.fi:schools:s1";
const OTHER_SCHOOL: &str = "urn:ngsi-ld:School:hel.fi:schools:s2";
const DESK: &str = "urn:ngsi-ld:Desk:hel.fi:schools:d1";
const USER: &str = "urn:ngsi-ld:User:hel.fi:schools:u1";

fn end(target: &str, unique: bool, on_delete: &str) -> Value {
    json!({ "target": target, "unique": unique, "onDelete": on_delete })
}

/// User: `school` required single, `desk` one-to-one optional, `courses` many optional.
fn rules() -> RelationshipRules {
    RelationshipRules::from_schema(&json!({ "$defs": {
        "User": {
            "required": ["school"],
            "properties": {
                "school": { "type": "string", "x-ngsi-ld-relationship": end("School", false, "restrict") },
                "desk": { "type": "string", "x-ngsi-ld-relationship": end("Desk", true, "set-null") },
                "courses": { "type": "array", "x-ngsi-ld-relationship": end("Course", false, "cascade") }
            }
        }
    } }))
}

fn whole() -> Shape<'static> {
    Shape {
        addressed: None,
        targeted: None,
        extent: Extent::Whole,
    }
}

fn violation(rule: &'static str, object: Option<&str>) -> Violation {
    Violation {
        rule,
        class: "User".into(),
        slot: "school".into(),
        object: object.map(str::to_owned),
        target: "School".into(),
    }
}

fn problem(v: Violation) -> ProblemDetails {
    ProblemDetails::from(v)
}

#[test]
fn every_rule_has_its_own_problem_text_and_extensions() {
    let cases = [
        ("target-wrong-type", Some("urn:ngsi-ld:Desk:x:1"), "points at a School, and `urn:ngsi-ld:Desk:x:1` is not one"),
        ("single-end-many-targets", Some(OTHER_SCHOOL), "holds one School, and the write gives it more than one"),
        ("required-end-missing", None, "is required: every `User` points at a School"),
        ("target-taken", Some(SCHOOL), "another `User` already points at `urn:ngsi-ld:School:hel.fi:schools:s1`; unlink it there first"),
        ("target-missing", Some(SCHOOL), "which is no School of this space you"),
    ];
    for (rule, object, words) in cases {
        let p = problem(violation(rule, object));
        let detail = p.detail.clone().expect("a detail");
        assert!(detail.contains(words), "{rule}: {detail}");
        assert!(detail.contains("`school` of `User`"), "{rule}: {detail}");
        assert!(detail.ends_with("(DM-70)"), "{rule}: {detail}");
        assert_eq!(p.status, 400);
        assert_eq!(
            p.type_uri,
            "https://uri.etsi.org/ngsi-ld/errors/BadRequestData"
        );
        assert_eq!(p.extensions["slot"], json!("school"));
        assert_eq!(p.extensions["rule"], json!(rule));
        match object {
            Some(o) => assert_eq!(p.extensions["object"], json!(o)),
            None => assert!(
                !p.extensions.contains_key("object"),
                "{rule}: no object member"
            ),
        }
    }
}

#[test]
fn a_schema_without_ends_relates_nothing() {
    for schema in [
        json!(null),
        json!({ "definitions": [] }),
        json!({ "definitions": { "User": { "type": "object" } } }),
        json!({ "definitions": { "User": { "properties": [] } } }),
        // An empty target, a target that is no string and a plain property are no stored end.
        json!({ "definitions": { "User": { "properties": {
            "a": { "x-ngsi-ld-relationship": { "target": "" } },
            "b": { "x-ngsi-ld-relationship": { "target": 7 } },
            "c": { "x-ngsi-ld-relationship": {} },
            "d": { "type": "string" }
        } } } }),
    ] {
        let rules = RelationshipRules::from_schema(&schema);
        assert!(rules.is_empty(), "{schema}");
        assert_eq!(rules, RelationshipRules::default());
    }
    // `definitions` wins over `$defs`; a class with no end is left out beside one that has one.
    let rules = RelationshipRules::from_schema(&json!({
        "definitions": {
            "Plain": { "properties": { "name": { "type": "string" } } },
            "User": { "properties": { "school": { "x-ngsi-ld-relationship": { "target": "School" } } } }
        },
        "$defs": { "Desk": { "properties": { "room": { "x-ngsi-ld-relationship": { "target": "Room" } } } } }
    }));
    assert_eq!(rules.targets("User"), BTreeSet::from(["School".to_owned()]));
    assert!(rules.targets("Plain").is_empty());
    assert!(
        rules.targets("Desk").is_empty(),
        "$defs is read only without definitions"
    );
}

#[test]
fn referencing_an_id_reads_the_type_of_its_urn() {
    let rules = rules();
    let by_id = rules.referencing_id(SCHOOL);
    assert_eq!(by_id.len(), 1);
    assert_eq!((by_id[0].class, by_id[0].slot), ("User", "school"));
    assert_eq!(by_id[0].end.on_delete, OnDelete::Restrict);
    assert!(by_id[0].end.required && !by_id[0].end.many && !by_id[0].end.unique);
    assert_eq!(
        rules.referencing_id(DESK)[0].end.on_delete,
        OnDelete::SetNull
    );
    // Not an NGSI-LD URN, an empty string, a type nothing points at: nothing referenced.
    for id in [
        "",
        "School",
        "urn:ngsi-ld:School",
        "urn:ngsi-ld::x",
        "urn:other:School:x",
        USER,
    ] {
        assert!(rules.referencing_id(id).is_empty(), "{id}");
    }
}

#[test]
fn objects_reads_every_form_and_nothing_else() {
    let s = |v: &[&str]| Some(v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>());
    assert_eq!(objects(&json!(null)), None);
    assert_eq!(objects(&json!("urn:ngsi-ld:null")), None);
    assert_eq!(objects(&json!({ "object": "urn:ngsi-ld:null" })), None);
    assert_eq!(objects(&json!({ "object": SCHOOL })), s(&[SCHOOL]));
    // An object list holding strings, nested objects by `object` or `@id`, and junk.
    assert_eq!(
        objects(
            &json!({ "object": [SCHOOL, { "object": OTHER_SCHOOL }, { "@id": DESK }, { "x": 1 }, 7, null] })
        ),
        s(&[SCHOOL, OTHER_SCHOOL, DESK])
    );
    // `object` wins over `@id` inside a list member; a non-string `object` names nothing.
    assert_eq!(
        objects(
            &json!({ "object": [{ "object": SCHOOL, "@id": DESK }, { "object": 1, "@id": DESK }] })
        ),
        s(&[SCHOOL])
    );
    // A member with no object, or an object that is neither string nor list: no target.
    for v in [
        json!({}),
        json!({ "value": SCHOOL }),
        json!({ "object": 3 }),
        json!({ "object": {} }),
    ] {
        assert_eq!(objects(&v), s(&[]), "{v}");
    }
    // A bare value in the concise form is a Property's value.
    for v in [json!(SCHOOL), json!(1), json!(true)] {
        assert_eq!(objects(&v), s(&[]), "{v}");
    }
    // Instances: nulls inside an array name nothing, the rest is gathered in order.
    assert_eq!(
        objects(&json!([{ "object": SCHOOL }, null, { "object": [OTHER_SCHOOL] }])),
        s(&[SCHOOL, OTHER_SCHOOL])
    );
    assert_eq!(objects(&json!([])), s(&[]));
}

#[test]
fn a_payload_without_entities_or_with_unknown_types_names_no_target() {
    let rules = rules();
    for payload in [
        json!(null),
        json!(7),
        json!("x"),
        json!([1, "two", null, []]),
        json!([]),
        json!({ "id": "urn:ngsi-ld:Room:x:1", "type": "Room", "school": { "object": DESK } }),
        json!({ "type": ["Room", "Hall"], "school": { "object": DESK } }),
        json!({ "school": { "object": SCHOOL } }),
    ] {
        assert_eq!(
            targets_of(&payload, whole(), &rules),
            Ok(Targets::new()),
            "{payload}"
        );
        assert!(claims_of(&payload, whole(), &rules).is_empty(), "{payload}");
    }
}

#[test]
fn an_empty_model_checks_nothing() {
    let empty = RelationshipRules::default();
    let write =
        json!({ "id": USER, "type": "User", "school": { "object": "urn:ngsi-ld:Desk:x:1" } });
    assert_eq!(targets_of(&write, whole(), &empty), Ok(Targets::new()));
    assert!(claims_of(&write, whole(), &empty).is_empty());
    assert_eq!(deleted_attribute(USER, "school", &empty), Ok(()));
    assert!(empty.referencing("School").is_empty());
    assert!(empty.targets("User").is_empty());
}

#[test]
fn a_null_on_an_optional_end_unlinks_and_on_a_required_end_is_refused() {
    let rules = rules();
    let partial = Shape {
        addressed: Some(USER),
        targeted: None,
        extent: Extent::Partial,
    };
    // Optional single end set to null: accepted, no target named.
    for gone in [
        json!(null),
        json!("urn:ngsi-ld:null"),
        json!({ "object": "urn:ngsi-ld:null" }),
    ] {
        let fragment = json!({ "desk": gone.clone() });
        assert_eq!(
            targets_of(&fragment, partial, &rules),
            Ok(Targets::new()),
            "{gone}"
        );
        let fragment = json!({ "school": gone.clone() });
        assert_eq!(
            targets_of(&fragment, partial, &rules).map_err(|v| v.rule),
            Err("required-end-missing"),
            "{gone}"
        );
    }
    // An empty list on the required end empties it.
    assert_eq!(
        targets_of(&json!({ "school": { "object": [] } }), partial, &rules).map_err(|v| v.rule),
        Err("required-end-missing")
    );
}

#[test]
fn a_fragment_for_one_attribute_answers_its_targets() {
    let rules = rules();
    let shape = Shape {
        addressed: Some(USER),
        targeted: Some("school"),
        extent: Extent::Partial,
    };
    let targets = targets_of(
        &json!({ "type": "Relationship", "object": SCHOOL }),
        shape,
        &rules,
    )
    .expect("one school");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[SCHOOL].slot, "school");
    assert_eq!(targets[SCHOOL].class, "User");
    // An attribute the class does not store: nothing to check.
    let other = Shape {
        targeted: Some("name"),
        ..shape
    };
    assert_eq!(targets_of(&json!("Ann"), other, &rules), Ok(Targets::new()));
    // A fragment addressed to an id of no known type.
    let unknown = Shape {
        addressed: Some("not-a-urn"),
        ..shape
    };
    assert_eq!(
        targets_of(&json!({ "object": DESK }), unknown, &rules),
        Ok(Targets::new())
    );
}

#[test]
fn first_missing_names_the_first_unread_target_or_none() {
    let rules = rules();
    let write = json!({ "id": USER, "type": "User", "school": { "object": SCHOOL },
        "courses": { "object": ["urn:ngsi-ld:Course:x:1", "urn:ngsi-ld:Course:x:2"] } });
    let targets = targets_of(&write, whole(), &rules).expect("valid");
    assert_eq!(targets.len(), 3);
    assert!(first_missing(&Targets::new(), &BTreeSet::new()).is_none());
    let all: BTreeSet<String> = targets.keys().cloned().collect();
    assert!(first_missing(&targets, &all).is_none());
    let missing = first_missing(&targets, &BTreeSet::new()).expect("all missing");
    assert_eq!(missing.rule, "target-missing");
    assert_eq!(
        missing.object.as_deref(),
        Some("urn:ngsi-ld:Course:x:1"),
        "BTreeMap order"
    );
}

#[test]
fn claims_skip_non_objects_unknown_types_and_take_the_at_id_holder() {
    let rules = rules();
    let batch = json!([
        7,
        null,
        { "id": "urn:ngsi-ld:Room:x:1", "type": "Room", "desk": { "object": DESK } },
        { "@id": USER, "@type": "User", "desk": { "object": DESK } },
        { "type": "User", "desk": { "object": DESK } }
    ]);
    let claims = claims_of(&batch, whole(), &rules);
    assert_eq!(claims.len(), 2, "{claims:?}");
    assert_eq!(claims[0].holder.as_deref(), Some(USER));
    assert_eq!(claims[1].holder, None);
    // A holder-less claim after another of the same target is taken twice.
    assert_eq!(claimed_twice(&claims), Some(&claims[1]));
    assert_eq!(claimed_twice(&[]), None);
    let lone: Vec<Claim> = claims.into_iter().take(1).collect();
    assert_eq!(claimed_twice(&lone), None);
}

#[test]
fn deleting_an_attribute_is_refused_only_for_a_required_end() {
    let rules = rules();
    let refused = deleted_attribute(USER, "school", &rules).expect_err("required");
    assert_eq!(refused.rule, "required-end-missing");
    assert_eq!(refused.object, None);
    assert_eq!(deleted_attribute(USER, "desk", &rules), Ok(()));
    assert_eq!(deleted_attribute(USER, "name", &rules), Ok(()));
    assert_eq!(
        deleted_attribute("urn:ngsi-ld:Room:x:1", "school", &rules),
        Ok(())
    );
    assert_eq!(deleted_attribute("", "school", &rules), Ok(()));
}

#[test]
fn unlinking_a_many_end_the_entity_does_not_hold_changes_nothing() {
    let many = |required: bool| StoredEnd {
        target: "Course".into(),
        many: true,
        required,
        unique: false,
        on_delete: OnDelete::Cascade,
    };
    let empty = Unlink::Instances {
        removed: vec![],
        rewritten: vec![],
    };
    let course = "urn:ngsi-ld:Course:x:1";
    assert_eq!(
        unlink(&json!({ "id": USER }), "courses", &many(false), course),
        empty
    );
    // Even a required end: nothing was linked, so nothing empties.
    assert_eq!(
        unlink(&json!({ "id": USER }), "courses", &many(true), course),
        empty
    );
    // A single instance (no array) naming another course only.
    assert_eq!(
        unlink(
            &json!({ "courses": { "object": "urn:ngsi-ld:Course:x:2" } }),
            "courses",
            &many(true),
            course
        ),
        empty
    );
    // A null instance names nothing.
    assert_eq!(
        unlink(&json!({ "courses": null }), "courses", &many(false), course),
        empty
    );
}
