//! A broker's publish filters and a topic a device published to: the filters the subscriber asks
//! for are sorted, distinct and widened, and the device a topic names is one of its own levels.
#![no_main]

use joinedcontext_portal::mqtt_capture::{client_of, filters};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut lines = text.lines();
    let topic = lines.next().unwrap_or_default();
    let publish: Vec<String> = lines.map(str::to_owned).collect();
    let widened = filters(&publish);
    assert!(
        widened.windows(2).all(|pair| pair[0] < pair[1]),
        "{widened:?}"
    );
    assert!(
        widened.iter().all(|filter| !filter.contains("${")),
        "{widened:?}"
    );
    if let Some(client) = client_of(&publish, topic) {
        assert!(!client.is_empty(), "{topic:?}");
        assert!(topic.split('/').any(|level| level == client), "{topic:?}");
    }
});
