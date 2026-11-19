// SPDX-License-Identifier: EUPL-1.2
//! The host's default for an unset `ANTARES_EGRESS_ALLOW_PRIVATE`: its own
//! test binary, because the default is process-wide and the loader's unit
//! tests read the standalone default.
#![cfg(feature = "fetch")]

use ngsild_jsonld::{private_egress_default, EgressPolicy};

#[test]
fn a_host_default_arms_the_private_range_wall() {
    assert!(
        std::env::var_os("ANTARES_EGRESS_ALLOW_PRIVATE").is_none(),
        "this test reads the default of an unset variable"
    );
    assert!(
        EgressPolicy::from_env().allow_private,
        "standalone: allowed"
    );
    private_egress_default("false");
    assert!(
        !EgressPolicy::from_env().allow_private,
        "the host's false denies"
    );
    private_egress_default(" TRUE ");
    assert!(
        EgressPolicy::from_env().allow_private,
        "and its true allows"
    );
}
