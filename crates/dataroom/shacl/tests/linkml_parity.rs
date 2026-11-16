//! linkml-model's check against this validator over the model's own SHACL rendering (G-20,
//! PL-59): for entities drawn over one model, `linkml_model::check` refuses exactly those the
//! shapes `linkml_model::shacl` writes refuse, and names a component among the findings.
//! The entity reaches the validator as JSON-LD would expand it under the model's `@context`
//! (`linkml_model::context`): a slot whose range the context coerces carries its value as a
//! literal of that datatype, a string or enum slot a native JSON literal.

use dataroom_core::{CatalogueId, OrgId};
use dataroom_rdf::turtle::parse_turtle_checked;
use dataroom_rdf::RdfLimits;
use dataroom_shacl::{parse_shapes_graph, validate_graph_against_shapes};
use linkml_model::{check, Model};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

const MODEL: &str = r#"{"id": "https://e.org/m/", "name": "m",
  "classes": {"Station": {"slots": ["count", "level", "code", "kind", "name", "day"]}},
  "enums": {"Kind": {"permissible_values": {"a": {}, "b": {}}}},
  "slots": {
    "count": {"range": "integer", "minimum_value": 0, "maximum_value": 10},
    "level": {"range": "float", "maximum_value": 100},
    "code": {"range": "string", "pattern": "^[A-Z]{2}[0-9]$"},
    "kind": {"range": "Kind"},
    "name": {"range": "string", "required": true},
    "day": {"range": "date"}}}"#;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The values a slot is drawn from: in range first, then each way out of it.
fn values(slot: &str) -> Vec<Value> {
    match slot {
        "count" => vec![
            json!(0),
            json!(10),
            json!(-1),
            json!(11),
            json!(2.5),
            json!("x"),
            json!(true),
        ],
        "level" => vec![json!(1), json!(99.5), json!(100.5), json!("high")],
        "code" => vec![json!("AB1"), json!("ab1"), json!("ABC"), json!(7)],
        "kind" => vec![json!("a"), json!("b"), json!("c"), json!(1)],
        "name" => vec![json!("n"), json!(""), json!(3), json!(false)],
        _ => vec![json!("2026-11-15"), json!("15.11.2026"), json!(20261115)],
    }
}

/// The datatype the model's context coerces a slot's value to, `None` for a native literal.
fn coerced(slot: &str) -> Option<&'static str> {
    match slot {
        "count" => Some("integer"),
        "level" => Some("float"),
        "day" => Some("date"),
        _ => None,
    }
}

/// One literal in Turtle, as JSON-LD expansion types it.
fn literal(slot: &str, value: &Value) -> String {
    let lexical = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let quoted = serde_json::to_string(&lexical).unwrap_or_default();
    let datatype = coerced(slot).unwrap_or(match value {
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "double",
        Value::Number(_) => "integer",
        _ => "string",
    });
    format!("{quoted}^^<{XSD}{datatype}>")
}

/// A small deterministic generator: the draw is the same on every run.
struct Draw(u64);
impl Draw {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap_or(0) % n
    }
}

#[test]
fn linkml_check_and_its_shacl_shapes_refuse_the_same_entities() {
    let model = Model::read(MODEL.as_bytes(), &linkml_model::SURFACE_LIMITS).expect("the model");
    let rules = check::Rules::of(&model);
    let shapes = parse_shapes_graph(&linkml_model::shacl(&model), &RdfLimits::default())
        .expect("the shapes parse");
    let slots = ["count", "level", "code", "kind", "name", "day"];
    let mut draw = Draw(0x5eed);
    let (mut refused, mut cases) = (0, 0);
    for case in 0..600 {
        let id = format!("urn:ngsi-ld:Station:{case}");
        let mut doc = json!({"id": id, "type": [model.class_iri("Station")]});
        let mut turtle = format!("<{id}> a <{}>", model.class_iri("Station"));
        for slot in slots {
            let choices = values(slot);
            // a quarter of the draws leave the slot out, half take its first (valid) value
            let value = match draw.below(4) {
                0 if slot != "name" || draw.below(4) == 0 => continue,
                0..=2 => &choices[0],
                _ => &choices[draw.below(choices.len())],
            };
            doc[model.slot_iri(slot)] = json!([{"type": "Property", "value": value}]);
            turtle.push_str(&format!(
                " ;\n  <{}> {}",
                model.slot_iri(slot),
                literal(slot, value)
            ));
        }
        turtle.push_str(" .\n");
        let data = parse_turtle_checked(&turtle, &RdfLimits::default()).expect("the data parse");
        let report = validate_graph_against_shapes(
            &data,
            &shapes,
            OrgId::from_uuid(Uuid::from_u128(1)),
            CatalogueId::from_uuid(Uuid::from_u128(2)),
            "d",
            "linkml",
            "1",
        );
        let components: BTreeSet<String> = report
            .findings
            .iter()
            .map(|f| {
                f.source_constraint_component
                    .rsplit('#')
                    .next()
                    .unwrap_or("")
                    .to_owned()
            })
            .collect();
        let verdict = check::entity(&rules, &doc, true);
        assert_eq!(
            verdict.is_ok(),
            report.conforms,
            "{turtle}\n{verdict:?}\n{components:?}"
        );
        if let Err(v) = verdict {
            let component = v.component.trim_start_matches("sh:");
            assert!(
                components.contains(component),
                "{turtle}\n{component} not in {components:?}"
            );
            refused += 1;
        }
        cases += 1;
    }
    assert!(
        refused > 100 && cases - refused > 50,
        "both verdicts drawn: {refused} of {cases}"
    );
}
