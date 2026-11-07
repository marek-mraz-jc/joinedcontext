//! T-3223: every data model the dev seed holds, checked the way a pipeline's output is. The
//! example entity Model Tools wrote for each model passes the runner's validation stage
//! (`pipeline_validation::ModelSchema::check`, PL-59), and every attribute it carries is one the
//! pipeline editor offers for its type (`ModelSchema::attributes`). A model the checker or the
//! attribute list misreads fails here, named. The models are copied from the deployment's seed
//! (`tests/fixtures/seed-models/README.md`).

use std::path::Path;

use joinedcontext_portal::pipeline_validation::ModelSchema;
use serde_json::Value;

fn models() -> Vec<(String, Value, Vec<Value>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/seed-models");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("the seed models") {
        let path = entry.expect("an entry").path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".schema.json"))
        else {
            continue;
        };
        let read = |file: &Path| -> Value {
            serde_json::from_str(&std::fs::read_to_string(file).expect("readable")).expect("JSON")
        };
        let schema = read(&path);
        let example = read(&dir.join(format!("{name}.example.jsonld")));
        let entities = match example {
            Value::Array(many) => many,
            one => vec![one],
        };
        found.push((name.to_owned(), schema, entities));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        found.len() >= 10,
        "the seed holds at least ten models, found {}",
        found.len()
    );
    found
}

/// The KPI model's JSON Schema states `calculationPeriod` and `currentValue` as plain strings, while
/// an indicator carries a `{start, end}` window and a number (jc-core `kpi`; `updatedAt` passes since
/// the typed DateTime literal of T-6593): a divergence of the model, recorded in chyby.md on
/// 2026-10-07. Listed here exactly, so the day the model is fixed this list must shrink with it.
const KNOWN: [(&str, &str); 4] = [
    ("helsinki-kpi.v1", "calculationPeriod"),
    ("helsinki-kpi.v1", "currentValue"),
    ("key-performance-indicator.v1", "calculationPeriod"),
    ("key-performance-indicator.v1", "currentValue"),
];

/// A conforming entity from Model Tools' example: it writes each slot's name as the example value,
/// so a `format: uri` slot holding `"source"` gets a URL instead.
fn conforming(schema: &Value, class: &str, mut entity: Value) -> Value {
    let properties = schema
        .pointer(&format!("/definitions/{class}/properties"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(object) = entity.as_object_mut() {
        for (name, value) in object.iter_mut() {
            let uri = properties
                .get(name)
                .and_then(|p| p.get("format"))
                .and_then(Value::as_str)
                == Some("uri");
            if uri && value.as_str().is_some_and(|v| !v.contains("://")) {
                *value = Value::String(format!("https://example.org/{name}"));
            }
        }
        object.remove("@context");
    }
    entity
}

#[test]
fn every_seeded_models_example_passes_the_check_a_pipeline_output_gets() {
    let mut broken = Vec::new();
    for (name, schema, entities) in models() {
        for entity in entities {
            let class = entity["type"].as_str().expect("a type").to_owned();
            let model =
                ModelSchema::compile(&name, "1", &schema, std::slice::from_ref(&class), false);
            // Model Tools writes most examples in key-value form; a write is normalized, as the
            // editor's preview shows it.
            let entity = model.normalize(&class, &conforming(&schema, &class, entity));
            for problem in model.check(&entity) {
                broken.push((
                    name.clone(),
                    problem.path.clone(),
                    format!(
                        "{name} {class}: {} {} {}",
                        problem.rule, problem.path, problem.message
                    ),
                ));
            }
        }
    }
    let unexpected: Vec<&String> = broken
        .iter()
        .filter(|(name, path, _)| !KNOWN.contains(&(name.as_str(), path.as_str())))
        .map(|(_, _, said)| said)
        .collect();
    assert!(unexpected.is_empty(), "{unexpected:#?}");
    for (name, path) in KNOWN {
        assert!(
            broken.iter().any(|(n, p, _)| n == name && p == path),
            "{name} {path} passes now: remove it from KNOWN"
        );
    }
}

#[test]
fn every_attribute_a_seeded_example_carries_is_offered_for_its_type() {
    let mut missing = Vec::new();
    for (name, schema, entities) in models() {
        for entity in entities {
            let class = entity["type"].as_str().expect("a type").to_owned();
            let model =
                ModelSchema::compile(&name, "1", &schema, std::slice::from_ref(&class), false);
            let listed = model.attributes(&class).expect("the class is the model's");
            assert!(
                !listed.attributes.is_empty(),
                "{name} {class} lists no attribute"
            );
            let offered: Vec<&str> = listed.attributes.iter().map(|a| a.name.as_str()).collect();
            for attribute in entity.as_object().expect("an entity").keys() {
                if matches!(attribute.as_str(), "id" | "type" | "@context") {
                    continue;
                }
                if !offered.contains(&attribute.as_str()) {
                    missing.push(format!("{name} {class}: {attribute}"));
                }
            }
            // Required ones come first, so the editor shows what must be mapped before the rest.
            let first_optional = listed.attributes.iter().position(|a| !a.required);
            if let Some(at) = first_optional {
                assert!(
                    listed.attributes[at..].iter().all(|a| !a.required),
                    "{name} {class}: a required attribute after an optional one"
                );
            }
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}
