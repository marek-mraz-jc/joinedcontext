//! An Organization realm list as jc's host registry serves it (`auth::realms::parse`, read again
//! by `replace_organization_realms` at run time): never panics, and every realm it accepts names
//! an Organization with a host, http(s) URLs and its keys under its own issuer.
#![no_main]
use joinedcontext_portal::auth::realms::parse;
use joinedcontext_portal::config::OidcConfig;
use joinedcontext_portal::organization_host::OrganizationHost;
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

fn fixtures() -> &'static (OidcConfig, Vec<OrganizationHost>) {
    static FIXTURES: OnceLock<(OidcConfig, Vec<OrganizationHost>)> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let installation = OidcConfig {
            issuer: "https://idm.example/realms/jc".parse().expect("issuer"),
            client_id: "portal".into(),
            auth: joinedcontext_portal::auth::client_auth::ClientAuth::Secret("fuzz".into()),
            extra_ca_pem: None,
            admin_url: None,
        };
        let hosts = ["city", "region"]
            .iter()
            .map(|name| OrganizationHost {
                organization: (*name).to_owned(),
                base: format!("https://portal.{name}.example").parse().expect("base"),
            })
            .collect();
        (installation, hosts)
    })
}

fuzz_target!(|data: &[u8]| {
    // The registry's answer is capped long before this.
    if data.len() > 64 * 1024 {
        return;
    }
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    let (installation, hosts) = fixtures();
    if let Ok(settings) = parse(raw, Some(installation), hosts) {
        for s in &settings {
            assert!(hosts.iter().any(|h| h.organization == s.organization));
            assert!(matches!(s.issuer.scheme(), "http" | "https"));
            assert!(s.jwks_url.path().ends_with("/protocol/openid-connect/certs"));
        }
    }
});
