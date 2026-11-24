//! Input: a YAML sequence of LinkML documents, each merged in as another model's source.
//! A merged document is a mapping named as asked.
#![no_main]

use jcctl::model::merge::{merge_sources, Source};
use libfuzzer_sys::fuzz_target;
use serde_norway::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(Value::Sequence(docs)) = serde_norway::from_slice::<Value>(data) else {
        return;
    };
    let sources: Vec<Source> = docs
        .into_iter()
        .enumerate()
        .map(|(i, schema)| Source {
            model: format!("m{}", i % 3),
            schema,
        })
        .collect();
    if let Ok(merged) = merge_sources("m0", &sources) {
        assert_eq!(merged.get("name").and_then(Value::as_str), Some("m0"));
    }
    let _ = merge_sources("other", &sources);
});
