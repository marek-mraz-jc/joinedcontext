// SPDX-License-Identifier: EUPL-1.2
//! A crafted paging cursor (MC20): decoding answers an offset or a refusal,
//! never panics, and a cursor the surface made decodes to its offset.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (head, rest) = data.split_at(data.len().min(8));
    let mut offset = [0u8; 8];
    offset[..head.len()].copy_from_slice(head);
    if let Ok(raw) = std::str::from_utf8(rest) {
        antares_mcp::fuzzing::cursor(raw, u64::from_le_bytes(offset));
    }
});
