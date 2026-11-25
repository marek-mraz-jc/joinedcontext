//! An alert subscription's body as a person sends it: a kept one names a known scope, events of
//! that scope's kind (each once, in order), a target and a delivery this Portal sends.
#![no_main]

use joinedcontext_portal::alerts::{checked, room_for, NewSubscription, Scope};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(new) = serde_json::from_slice::<NewSubscription>(data) else {
        return;
    };
    if let Ok((scope, events, _delivery)) = checked(&new) {
        assert!(!events.is_empty(), "{new:?}");
        assert!(
            events.windows(2).all(|pair| pair[0] < pair[1]),
            "events are sorted and appear once: {events:?}"
        );
        assert!(
            events
                .iter()
                .all(|event| event.is_agent() == (scope == Scope::Agent)),
            "{new:?}"
        );
        assert!(!new.target.is_empty(), "{new:?}");
        if scope == Scope::Type {
            assert!(new.target.contains('/'), "{new:?}");
        }
        assert!(room_for(&[], &new, scope).is_ok());
    }
});
