// SPDX-License-Identifier: EUPL-1.2
//! Each refusal of the entry checks on its own, so every condition is seen false once.
#![allow(clippy::unwrap_used)]
use ngsild_model::{check_attr_name, EntityId};

#[test]
fn an_entity_id_is_refused_for_each_reason_alone() {
    for bad in [
        "urn:x\u{7f}y",   // a character no URI holds
        "urn:x/./y",      // a "." segment
        "urn:x/../y",     // a ".." segment
        "urn:x/%2E%2E/y", // the same, percent-encoded
        ":nss",           // an empty scheme
        "urn:",           // nothing after the scheme
        "u_rn:x",         // a scheme character RFC 3986 3.1 does not allow
        "no-colon",       // no scheme at all
    ] {
        assert_eq!(EntityId::new(bad).unwrap_err().status(), 400, "{bad:?}");
    }
    assert_eq!(
        EntityId::new("urn:ngsi-ld:Vehicle:A1").unwrap().as_str(),
        "urn:ngsi-ld:Vehicle:A1"
    );
}

#[test]
fn an_attribute_name_is_refused_for_each_reason_alone() {
    for bad in ["", "speed(km)", "..", "a/./b", "%2e%2E"] {
        assert_eq!(check_attr_name(bad).unwrap_err().status(), 400, "{bad:?}");
    }
    check_attr_name("https://example.org/speed").unwrap();
}
