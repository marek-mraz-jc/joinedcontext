//! A gateway's usage post: whatever bytes arrive, a post is taken whole or refused with a
//! sentence, and a taken post holds only lines `check` accepts.
#![no_main]

use joinedcontext_portal::endpoint_usage::{check, parse, MAX_LINES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(post) = parse(data) {
        assert!(post.counts.len() <= MAX_LINES);
        assert_eq!(check(&post), Ok(()), "a taken post checks again");
    }
});
