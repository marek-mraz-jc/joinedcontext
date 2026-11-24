//! Input: a field expression with `${! }` interpolations. It compiles or is refused; a compiled
//! field renders against a one-part batch.
#![no_main]

use ekiben_bloblang::{Environment, SimplePartView};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(field) = Environment::new().compile_field(text) else {
        return;
    };
    let parts = [SimplePartView::from_bytes(br#"{"topic":"events","n":[1,2]}"#.to_vec())];
    let text = field.resolve_string(0, &parts);
    let bytes = field.resolve_bytes(0, &parts);
    assert_eq!(text.is_ok(), bytes.is_ok());
});
