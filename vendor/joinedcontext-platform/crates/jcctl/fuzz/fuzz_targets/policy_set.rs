//! Input: an ODRL `Set` as `jcctl policy import` reads it from a file. A `Set` that imports is a
//! Policy that validates, renders back to a `Set`, and imports again to the same Policy.
#![no_main]

use jc_core::kinds::policy_odrl::{import, render, Names};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    let names = Names {
        project: Some("p".to_owned()),
        space: Some("s".to_owned()),
        name: Some("n".to_owned()),
    };
    let Ok(policy) = import(&document, &names) else {
        return;
    };
    policy.validate().expect("an imported Policy validates");
    let set = render(&policy).expect("an imported Policy renders");
    let again = import(&set, &names).expect("a rendered Set imports");
    assert_eq!(again, policy, "the round trip keeps the Policy");
});
