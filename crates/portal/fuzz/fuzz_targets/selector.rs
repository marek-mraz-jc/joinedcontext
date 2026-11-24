//! `labelSelector` and `fieldSelector` of a list request: any text parses or is refused, and a
//! parsed selector runs over labels without a panic.
#![no_main]

use joinedcontext_portal::resource::selector::{FieldSelector, LabelSelector};
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(selector) = LabelSelector::parse(text) {
        let mut labels = BTreeMap::new();
        labels.insert("app".to_owned(), "x".to_owned());
        labels.insert(text.to_owned(), text.to_owned());
        let _ = selector.matches(&labels);
        let _ = selector.matches(&BTreeMap::new());
    }
    let _ = FieldSelector::parse(text);
});
