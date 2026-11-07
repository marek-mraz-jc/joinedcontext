// SPDX-License-Identifier: EUPL-1.2
//! A crafted client message on the WebSocket surface (W3, W11): reading
//! answers a message, a 400 or a close, never panics, and nothing past the
//! size, depth or `x-id` bounds is ever read as a message.
#![no_main]
use antares_ws::envelope::{read, Unread, MAX_ID_BYTES, MAX_MESSAGE_BYTES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    match read(text) {
        Ok(m) => {
            assert!(text.len() <= MAX_MESSAGE_BYTES);
            assert!(m.id.as_ref().map_or(0, String::len) <= MAX_ID_BYTES);
        }
        Err(Unread::Close(_)) => {
            assert!(text.len() > MAX_MESSAGE_BYTES || text.matches(['[', '{']).count() > 64);
        }
        Err(Unread::Bad(_)) => {}
    }
});
