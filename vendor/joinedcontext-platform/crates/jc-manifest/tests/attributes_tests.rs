//! G-15 (T-7794): `spec.attributes` checked against the space's model, its `unitCode: model`
//! resolved, and compiled into one Bloblang step whose text holds only checked parts.

use jc_core::kinds::{Pipeline, PipelineSpec};
use jc_manifest::attributes::{check, resolve_units, step};
use jc_manifest::model_schema::ModelSchema;
use serde_json::json;

fn model() -> ModelSchema {
    let celsius = json!({"exactMappings": ["ucefact:CEL"], "ucumCode": "Cel"});
    let schema = json!({ "$defs": {
        "Sensor": { "type": "object", "properties": {
            "id": {"type": "string"}, "type": {"type": "string"},
            "temperature": {"type": "number", "x-unit": celsius},
            "dewPoint": {"type": "number", "x-unit": celsius},
            "pressure": {"type": "number", "x-unit": {"exactMappings": ["ucefact:HPA"]}},
            "count": {"type": "number"},
            "measuredAt": {"type": "string", "format": "date-time"},
            "dateCreated": {"type": "string", "format": "date-time"},
            "dateModified": {"type": "string", "format": "date-time"},
            "station": {"type": "string"}
        }},
        "Bare": { "type": "object", "properties": {
            "id": {"type": "string"}, "type": {"type": "string"}, "count": {"type": "number"}
        }}
    }});
    ModelSchema::compile(
        "sensors",
        "1.0.0",
        &schema,
        &["Sensor".to_owned(), "Bare".to_owned()],
        false,
    )
}

/// The Pipeline's spec with `attributes` as written, parsed but not admitted, so a test can
/// hand the step what admission would refuse.
fn spec(attributes: &str) -> PipelineSpec {
    let yaml = format!(
        r#"apiVersion: joinedcontext.com/v1alpha2
kind: Pipeline
metadata: {{ name: sensors, namespace: city }}
spec:
  class: resident
  sources:
    - dataSourceRef: {{ kind: DataSource, name: feed }}
  outputs:
    - targetEndpoint: urn:ngsi-ld:Endpoint:city.example:city:ep-write
  attributes:
{attributes}"#
    );
    Pipeline::from_yaml(&yaml).expect("parses").spec
}

fn refusal(attributes: &str) -> String {
    check(&spec(attributes), &model())
        .expect_err("refused")
        .to_string()
}

#[test]
fn a_plan_of_the_model_s_slots_passes_the_check() {
    let plan = "    - type: Sensor
      sourceDates: { created: created_at, modified: updated_at }
      rules:
        - { names: [temperature, dewPoint], unitCode: model, observedAt: measuredAt }
        - { names: [pressure], unitCode: HPA, observedAt: now }
        - { names: [count], observedAt: received }
";
    check(&spec(plan), &model()).expect("every name a slot, every unit the model's");
}

#[test]
fn an_unknown_type_is_refused_by_its_field() {
    let message = refusal("    - type: Nothing\n      rules: [{ names: [count] }]\n");
    assert!(message.contains("spec.attributes[0].type"), "{message}");
    assert!(message.contains("no class `Nothing`"), "{message}");
}

#[test]
fn a_source_date_without_its_slot_is_refused() {
    let message = refusal(
        "    - type: Bare\n      sourceDates: { modified: updated_at }\n      rules: [{ names: [count] }]\n",
    );
    assert!(
        message.contains("spec.attributes[0].sourceDates.modified"),
        "{message}"
    );
    assert!(message.contains("no slot `dateModified`"), "{message}");
    let message = refusal(
        "    - type: Bare\n      sourceDates: { created: created_at }\n      rules: [{ names: [count] }]\n",
    );
    assert!(message.contains("sourceDates.created"), "{message}");
}

#[test]
fn a_name_that_is_no_slot_is_refused_with_the_model_and_version() {
    let message = refusal("    - type: Sensor\n      rules: [{ names: [humidity] }]\n");
    assert!(
        message.contains("spec.attributes[0].rules[0].names"),
        "{message}"
    );
    assert!(
        message.contains("`humidity` is not a slot of `Sensor` in the space's model sensors 1.0.0"),
        "{message}"
    );
}

#[test]
fn unit_code_model_needs_a_slot_with_a_unit() {
    let message =
        refusal("    - type: Sensor\n      rules: [{ names: [count], unitCode: model }]\n");
    assert!(message.contains("rules[0].unitCode"), "{message}");
    assert!(
        message.contains("the model gives `count` no unit"),
        "{message}"
    );
}

#[test]
fn a_fixed_unit_code_other_than_the_model_s_is_refused() {
    let message =
        refusal("    - type: Sensor\n      rules: [{ names: [temperature], unitCode: FAH }]\n");
    assert!(
        message.contains("writes `temperature` in `CEL`, not `FAH`"),
        "{message}"
    );
    // A fixed code on a slot the model gives no unit passes: nothing to contradict.
    check(
        &spec("    - type: Sensor\n      rules: [{ names: [count], unitCode: C62 }]\n"),
        &model(),
    )
    .expect("no model unit to contradict");
}

#[test]
fn observed_at_must_name_a_date_time_slot() {
    let message =
        refusal("    - type: Sensor\n      rules: [{ names: [count], observedAt: station }]\n");
    assert!(message.contains("rules[0].observedAt"), "{message}");
    assert!(
        message.contains("`station` is not a date-time slot of `Sensor`"),
        "{message}"
    );
    let message =
        refusal("    - type: Sensor\n      rules: [{ names: [count], observedAt: missing }]\n");
    assert!(
        message.contains("`missing` is not a date-time slot"),
        "{message}"
    );
}

#[test]
fn resolve_units_writes_the_model_s_code_only_when_every_name_shares_it() {
    let mut one = spec("    - type: Sensor\n      rules:\n        - { names: [temperature, dewPoint], unitCode: model }\n        - { names: [temperature, pressure], unitCode: model }\n        - { names: [count], unitCode: model }\n        - { names: [pressure], unitCode: HPA }\n    - type: Nothing\n      rules: [{ names: [x], unitCode: model }]\n");
    resolve_units(&mut one, &model());
    let codes: Vec<Option<&str>> = one.attributes[0]
        .rules
        .iter()
        .map(|rule| rule.unit_code.as_deref())
        .collect();
    assert_eq!(
        codes,
        vec![Some("CEL"), Some("model"), Some("model"), Some("HPA")]
    );
    assert_eq!(
        one.attributes[1].rules[0].unit_code.as_deref(),
        Some("model")
    );
}

#[test]
fn no_attributes_is_no_step() {
    let mut plain = spec("    []\n");
    plain.attributes.clear();
    assert_eq!(step(&plain, "sensors").unwrap(), None);
}

#[test]
fn a_rule_that_sets_nothing_written_is_no_step() {
    let quiet = spec("    - type: Sensor\n      rules: [{ names: [count], unitCode: model }]\n");
    assert_eq!(step(&quiet, "sensors").unwrap(), None);
}

#[test]
fn the_step_writes_each_member_as_a_literal_by_type_and_name() {
    let plan = spec(
        "    - type: Sensor
      systemMembers: strip
      sourceDates: { created: meta.created, modified: updated_at }
      rules:
        - { names: [temperature], observedAt: measuredAt, unitCode: CEL, datasetId: pipeline }
        - { names: [pressure], observedAt: now, datasetId: 'urn:jc:station:{station}' }
        - { names: [count], observedAt: received }
    - type: Bare
      rules: [{ names: [count], unitCode: C62 }]
",
    );
    let text = step(&plan, "sensors").unwrap().expect("a step");
    assert!(text.starts_with("# spec.attributes (G-15)"), "{text}");
    assert!(
        text.find("if this.type == \"Bare\"").unwrap()
            < text.find("if this.type == \"Sensor\"").unwrap()
    );
    for member in ["createdAt", "modifiedAt", "deletedAt"] {
        assert!(
            text.contains(&format!("root.{member} = deleted()")),
            "{member}"
        );
    }
    assert!(text.contains("let dateCreated = this.meta.created."));
    assert!(text.contains("let dateModified = this.updated_at."));
    assert!(text.contains("\"unitCode\": \"CEL\""));
    assert!(text.contains("\"datasetId\": \"urn:jc:pipeline:sensors\""));
    assert!(
        text.contains("\"datasetId\": (\"urn:jc:station:\" + this.station."),
        "{text}"
    );
    assert!(text.contains("\"observedAt\": now().ts_parse("));
    assert!(text.contains("\"observedAt\": this.measuredAt."));
    assert!(
        text.contains(".without(\"observedAt\")"),
        "received drops the instance's time"
    );
    assert!(text.contains("\"unitCode\": \"C62\""));
    // Names in order inside one type.
    assert!(
        text.find("this.exists(\"count\")").unwrap()
            < text.find("this.exists(\"pressure\")").unwrap()
    );
    assert!(text.ends_with("else { this }\n"));
    assert_eq!(
        step(&plan, "sensors").unwrap().unwrap(),
        text,
        "the same manifest, the same text"
    );
}

#[test]
fn the_step_refuses_what_admission_would_and_writes_no_code_from_it() {
    let refused = |attributes: &str| {
        step(&spec(attributes), "sensors")
            .expect_err("refused")
            .to_string()
    };
    let message =
        refused("    - type: Sensor\n      rules: [{ names: ['a\"b'], unitCode: CEL }]\n");
    assert!(message.contains("is not a slot name"), "{message}");
    assert!(message.contains("spec.attributes"), "{message}");
    let message =
        refused("    - type: Sensor\n      rules: [{ names: [count], observedAt: 'x}y' }]\n");
    assert!(message.contains("`x}y` is not a slot name"), "{message}");
    let message = refused("    - type: Sensor\n      sourceDates: { created: 'a b' }\n      rules: [{ names: [count] }]\n");
    assert!(message.contains("`a b` is no record path"), "{message}");
    let message =
        refused("    - type: Sensor\n      rules: [{ names: [count], datasetId: 'urn:{' }]\n");
    assert!(!message.is_empty());
}
