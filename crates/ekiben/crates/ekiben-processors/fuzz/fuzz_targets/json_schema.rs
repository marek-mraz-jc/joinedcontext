//! Input: a JSON schema, a NUL byte, then the document. Without a NUL the whole input is the
//! schema and the document is `{"a":1}`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (schema, document) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &br#"{"a":1}"#[..]),
    };
    let ctx = ekiben_processors_fuzz::ctx();
    let config = serde_json::json!([{"json_schema": {"schema": String::from_utf8_lossy(schema)}}]);
    if let Some(procs) = ekiben_processors_fuzz::build(&ctx, &config) {
        // json_schema flags a failing document and passes it on: one in, one out.
        assert_eq!(ekiben_processors_fuzz::run(&ctx, &procs, document), 1);
    }
});
