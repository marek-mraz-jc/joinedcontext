//! CPU per record of the two validation stages the Portal renders (G-19): one `json_schema` per
//! class, the shape `jcctl::model_schema` compiles, against `linkml_validate` over the same
//! 30-attribute class. Both parse the message, as the processors do.
//! `cargo bench -p ekiben-linkml --bench validate_bench`.

use criterion::{Criterion, criterion_group, criterion_main};
use ekiben_core::ComponentConfig;
use ekiben_linkml::LinkmlValidateProcessor;
use ekiben_processors::json_schema::Schema;
use serde_json::{Map, Value, json};
use std::hint::black_box;

const SLOTS: usize = 30;

/// Slot `i`: a bounded number, a text with a pattern, an integer or an enum, in turn.
fn slot(i: usize) -> (Value, Value, Value) {
    match i % 4 {
        0 => (
            json!({"range": "float", "minimum_value": 0, "maximum_value": 1000}),
            json!({"type": "number", "minimum": 0, "maximum": 1000}),
            json!(12.5),
        ),
        1 => (
            json!({"range": "string", "pattern": "^[A-Z]{2}-[0-9]+$"}),
            json!({"type": "string", "pattern": "^[A-Z]{2}-[0-9]+$"}),
            json!("AB-123"),
        ),
        2 => (
            json!({"range": "integer"}),
            json!({"type": "integer"}),
            json!(7),
        ),
        _ => (
            json!({"range": "Level"}),
            json!({"enum": ["low", "mid", "high"]}),
            json!("mid"),
        ),
    }
}

fn inputs() -> (String, Value, Vec<u8>) {
    let mut slots = Map::new();
    let mut properties = Map::new();
    let mut entity = Map::new();
    entity.insert("id".into(), json!("urn:ngsi-ld:Station:1"));
    entity.insert("type".into(), json!("Station"));
    properties.insert("id".into(), json!({"type": "string"}));
    properties.insert("type".into(), json!({"const": "Station"}));
    for i in 0..SLOTS {
        let (linkml, value_schema, value) = slot(i);
        let name = format!("a{i}");
        slots.insert(name.clone(), linkml);
        properties.insert(
            name.clone(),
            json!({"type": "object", "required": ["type", "value"],
                "properties": {"type": {"const": "Property"}, "value": value_schema,
                    "observedAt": {"type": "string", "format": "date-time"}}}),
        );
        entity.insert(
            name,
            json!({"type": "Property", "value": value, "observedAt": "2026-11-15T10:00:00Z"}),
        );
    }
    let names: Vec<&String> = slots.keys().collect();
    let model = json!({"id": "https://e.org/", "name": "air",
        "classes": {"Station": {"slots": names}},
        "enums": {"Level": {"permissible_values": {"low": {}, "mid": {}, "high": {}}}},
        "slots": slots});
    let schema = json!({"$schema": "http://json-schema.org/draft-07/schema#", "type": "object",
        "required": ["id", "type"], "properties": properties, "additionalProperties": false});
    let record = serde_json::to_vec(&Value::Object(entity)).unwrap_or_default();
    (model.to_string(), schema, record)
}

fn bench(c: &mut Criterion) {
    let (model, schema, record) = inputs();
    let schema = Schema::compile(schema).expect("the schema compiles");
    let conf = json!({"linkml_validate": {"model": model, "closed": true}});
    let conf = ComponentConfig::from_json_value(&conf, "noop", None).expect("a config");
    let linkml = LinkmlValidateProcessor::from_config(&conf).expect("the model reads");
    let doc: Value = serde_json::from_slice(&record).expect("a record");
    assert!(schema.validate(&doc).is_empty(), "the record passes both");
    assert!(linkml.check(&doc).is_ok(), "the record passes both");
    let mut group = c.benchmark_group("validate_30_attributes");
    // What both stages share: the parse of the message.
    group.bench_function("parse", |b| {
        b.iter(|| {
            black_box(serde_json::from_slice::<Value>(black_box(&record)).unwrap_or_default())
        })
    });
    group.bench_function("json_schema", |b| {
        b.iter(|| {
            let doc: Value = serde_json::from_slice(black_box(&record)).unwrap_or_default();
            black_box(schema.validate(&doc))
        })
    });
    group.bench_function("linkml_validate", |b| {
        b.iter(|| {
            let doc: Value = serde_json::from_slice(black_box(&record)).unwrap_or_default();
            black_box(linkml.check(&doc).is_ok())
        })
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
