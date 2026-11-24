//! An entity a person moves to the trash: a kept one is an NGSI-LD URN of its own type.
#![no_main]

use joinedcontext_portal::api::entity_trash::urn_and_type;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(entity) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    if let Ok((urn, kind)) = urn_and_type(&entity) {
        assert!(urn.starts_with(&format!("urn:ngsi-ld:{kind}:")), "{urn:?} {kind:?}");
        assert!(urn.len() <= 1024 && !urn.chars().any(char::is_whitespace));
        assert!(kind.chars().next().is_some_and(|c| c.is_ascii_alphabetic()));
    }
});
