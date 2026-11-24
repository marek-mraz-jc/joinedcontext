//! Any text as a bearer token at a door that holds an issuer's keys: `Verifier::client` refuses
//! it without a panic. Only a token the issuer signed names a client, so a fuzzed one never does
//! (T-7766).
#![no_main]

use jc_door::issuer::LocalIssuer;
use jc_door::Verifier;
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

/// A door with the issuer's keys already fetched, so a token reaches the signature check.
fn door() -> &'static Verifier {
    static DOOR: OnceLock<Verifier> = OnceLock::new();
    DOOR.get_or_init(|| {
        let issuer = LocalIssuer::start("", "k1").expect("issuer");
        let verifier =
            Verifier::new(issuer.issuer(), issuer.jwks_url(), &["svc".to_owned()]).expect("door");
        let token = issuer.token("svc", "aud", 3600).expect("token");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(verifier.prepare(&token));
        assert_eq!(verifier.client(&token, "aud").as_deref(), Some("svc"));
        verifier
    })
}

fuzz_target!(|data: &[u8]| {
    let Ok(token) = std::str::from_utf8(data) else {
        return;
    };
    assert_eq!(door().client(token, "aud"), None, "{token:?}");
});
