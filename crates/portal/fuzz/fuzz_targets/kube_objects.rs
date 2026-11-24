//! The API server's objects the host waves copy and read: an edge Ingress becomes an App's, a
//! hostname's or a registry host's Ingress, a Certificate its state. Any JSON object and host
//! yields an object or none, never a panic.
#![no_main]

use joinedcontext_portal::reconciler::{app_hosts, org_hosts};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let (host, object) = match data.iter().position(|b| *b == b'\n') {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (&data[..0], data),
    };
    let (Ok(host), Ok(object)) = (
        std::str::from_utf8(host),
        serde_json::from_slice::<Value>(object),
    ) else {
        return;
    };
    let _ = app_hosts::ingress(&object, "apisix", "app", host);
    let _ = app_hosts::hostname_ingress(&object, "apisix", "app", host);
    let _ = app_hosts::state_of(&object);
    let _ = app_hosts::wildcard_serves(&object, &object, host);
    if let Some(acme) = org_hosts::acme_ingress(&object, "apisix", host) {
        assert!(
            org_hosts::acme_only(&acme),
            "an ACME Ingress carries no TLS"
        );
    }
    let _ = org_hosts::registered_domain(host);
    if host.is_ascii() {
        assert!(
            org_hosts::object_name(host).len() <= 63,
            "a name stays a label value"
        );
    }
});
