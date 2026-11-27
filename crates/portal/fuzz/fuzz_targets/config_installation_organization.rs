//! Input: an installation Organization's name, a newline, then `JC_PORTAL_ORGANIZATION_HOSTS`
//! (T-7264). A name the hosts do not list is refused; a seeded Organization carries that name
//! and its working host, lower-case, `host[:port]` only: never a path, a user or a scheme.
#![no_main]

use joinedcontext_portal::config::installation_organization;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (name, hosts) = text.split_once('\n').unwrap_or((&text, ""));
    let seeded = installation_organization(|var: &str| match var {
        "JC_PORTAL_INSTALLATION_ORGANIZATION" => Some(name.to_owned()),
        "JC_PORTAL_ORGANIZATION_HOSTS" => Some(hosts.to_owned()),
        _ => None,
    });
    if let Ok(Some(seed)) = seeded {
        assert_eq!(seed.name, name.trim());
        assert!(!seed.host.is_empty());
        assert_eq!(seed.host, seed.host.to_ascii_lowercase());
        assert!(!seed.host.contains(['/', '@', '?', '#']), "{}", seed.host);
        assert_eq!(seed.organization_id, None);
    }
});
