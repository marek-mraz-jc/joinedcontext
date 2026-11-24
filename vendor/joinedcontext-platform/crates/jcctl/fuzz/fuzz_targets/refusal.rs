//! Input: the runner's refusal text, and a mapping's output read as JSON.
//! Every non-blank refusal line becomes one lint error; a line number is only ever read as digits.
#![no_main]

use jcctl::pipeline_test::{line_of, lint_errors, problems_of};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let errors = lint_errors(&text);
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(errors.len(), lines.max(1));
    let _ = line_of(&text);
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let problems = problems_of(&value);
        if !value.is_object() {
            assert_eq!(problems.len(), 1);
        }
    }
});
