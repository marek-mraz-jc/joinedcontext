// SPDX-License-Identifier: EUPL-1.2
//! A surface's test reaches its own loopback listener through the kit: the
//! kit's switch grants private egress even where the deployment's
//! environment denies it. One test in its own binary, because it sets the
//! egress switch in the environment.

use antares_jsonld::EgressPolicy;
use antares_ops::test_kit::allow_private_egress;

#[test]
fn the_kit_grants_private_egress_the_environment_denies() {
    std::env::set_var("ANTARES_EGRESS_ALLOW_PRIVATE", "false");
    assert!(!EgressPolicy::from_env().allow_private);
    allow_private_egress();
    assert!(EgressPolicy::from_env().allow_private);
}
