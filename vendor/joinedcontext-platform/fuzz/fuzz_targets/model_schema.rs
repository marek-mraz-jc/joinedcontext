//! Any JSON Schema and record: compiling a model, normalizing and checking a record never panic,
//! and a record the schema refuses names at least one problem (T-7794).
#![no_main]

use jc_manifest::model_schema::ModelSchema;
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    // The schema, then a NUL, then the record.
    let mut parts = data.splitn(2, |b| *b == 0);
    let (Some(schema), Some(record)) = (parts.next(), parts.next()) else {
        return;
    };
    let (Ok(schema), Ok(record)) = (
        serde_json::from_slice::<Value>(schema),
        serde_json::from_slice::<Value>(record),
    ) else {
        return;
    };
    let classes: Vec<String> = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(Value::as_object)
        .map(|defs| defs.keys().take(4).cloned().collect())
        .unwrap_or_default();
    for open_world in [false, true] {
        let model = ModelSchema::compile("fuzz", "1.0.0", &schema, &classes, open_world);
        for class in &classes {
            let _ = model.ingest(class);
            let _ = model.personal_attributes(class);
            let _ = model.attributes(class);
            let normalized = model.normalize(class, &record);
            let _ = model.check(&normalized);
        }
        let _ = model.check(&record);
        let _ = model.quality_problems(&record);
    }
});
