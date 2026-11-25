// SPDX-License-Identifier: EUPL-1.2
//! The sanitizer stretch of outbound deadlines: its own test binary, because
//! `ANTARES_TEST_SANITIZER` is process-wide and every other loader test reads
//! the production factor of 1.
#![cfg(feature = "fetch")]

use ngsild_jsonld::slow_factor;

#[test]
fn a_sanitizer_run_stretches_deadlines_tenfold() {
    assert!(
        std::env::var_os("ANTARES_TEST_SANITIZER").is_none(),
        "this test reads the factor of an unset variable first"
    );
    assert_eq!(slow_factor(), 1, "production: no stretch");
    std::env::set_var("ANTARES_TEST_SANITIZER", "1");
    assert_eq!(slow_factor(), 10, "under a sanitizer: ten times");
    std::env::remove_var("ANTARES_TEST_SANITIZER");
    assert_eq!(slow_factor(), 1, "read per call, not latched");
}
