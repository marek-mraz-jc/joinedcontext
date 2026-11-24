//! Input: a grok expression, a NUL byte, then the text. The expression compiles against the default
//! patterns or is refused; a compiled one parses any text into an object or an error.
#![no_main]

use std::collections::BTreeMap;

use ekiben_text::processor_grok::{GrokConfig, GrokProcessor};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (expr, text) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &b""[..]),
    };
    let Ok(expr) = std::str::from_utf8(expr) else {
        return;
    };
    let conf = GrokConfig {
        expressions: vec![expr.to_string()],
        pattern_definitions: BTreeMap::new(),
        remove_empty: true,
        named_only: expr.len() % 2 == 0,
        use_defaults: true,
    };
    let Ok(p) = GrokProcessor::new(&conf) else {
        return;
    };
    let mut part = ekiben_core::Part::new(bytes::Bytes::copy_from_slice(text));
    if p.process(&mut part).is_ok() {
        let doc = part.as_structured().expect("grok writes a document");
        assert_eq!(doc.type_name(), "object");
    }
});
