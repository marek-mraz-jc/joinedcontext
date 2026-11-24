//! Decodes JSON keeping json.Number text, then encodes it back both ways: the encoding of a
//! decoded document must decode again to the same value.
#![no_main]

use ekiben_bloblang::go_json::decode_with_numbers;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(value) = decode_with_numbers(data) else {
        return;
    };
    let _ = value.to_go_json_with(false, Some("  "));
    let text = value.to_go_json();
    let again = decode_with_numbers(text.as_bytes()).expect("own encoding decodes");
    assert_eq!(again.to_go_json(), text);
});
