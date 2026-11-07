// SPDX-License-Identifier: EUPL-1.2
//! A stored view document, whatever its bytes (ADR-0023 rule 10, V23): the
//! loader answers a view or a reason, never panics, and stays bounded.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(view) = antares_ops::views::View::parse(data) {
        let _ = view.load();
    }
});
