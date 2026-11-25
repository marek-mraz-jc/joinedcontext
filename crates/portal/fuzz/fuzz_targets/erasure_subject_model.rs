//! The LinkML model a space holds and the subject an owner types for an erasure: any model text
//! gives the classes that name a subject or none, each with its subject slot among the Attributes
//! to erase, and any subject is kept as one string or refused.
#![no_main]

use joinedcontext_portal::erasure::{subject_classes, subject_query, subject_refusal};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    for class in subject_classes(text) {
        if let Some(attrs) = &class.attrs {
            assert!(
                attrs.contains(&class.slot),
                "{class:?} leaves its subject slot out"
            );
            assert!(
                attrs.windows(2).all(|pair| pair[0] < pair[1]),
                "{class:?} lists attributes out of order or twice"
            );
        }
    }
    if subject_refusal(text).is_none() {
        let query = subject_query("rider", text);
        assert_eq!(
            query.matches('"').count(),
            2,
            "{query:?} holds a stray quote"
        );
    }
});
