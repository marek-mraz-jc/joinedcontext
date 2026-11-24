//! A visitor's request bodies and the Portal's usage range (T-7768): the question and the
//! feedback parse or are refused, a parsed feedback is checked, a range names what is wrong.
#![no_main]

use assistant::chat::usage::Range;
use assistant::chat::{check_feedback, Feedback, Question};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Question>(data);
    if let Ok(feedback) = serde_json::from_slice::<Feedback>(data) {
        let _ = check_feedback(&feedback);
    }
    if let Ok(range) = serde_json::from_slice::<Range>(data) {
        let _ = range.refusal();
    }
});
