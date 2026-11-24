//! Input: the schema text, a NUL byte, then the JSON document to check. Without a NUL the
//! document is `{"a":[1,"x",null]}`.
#![no_main]

use ekiben_bloblang::go_json::decode_with_numbers;
use ekiben_bloblang::json_schema::check;
use ekiben_bloblang::Value;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (schema, doc) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], decode_with_numbers(&data[at + 1..])),
        None => (data, decode_with_numbers(br#"{"a":[1,"x",null]}"#)),
    };
    let doc = doc.unwrap_or(Value::Null);
    let _ = check(&doc, &String::from_utf8_lossy(schema));
});
