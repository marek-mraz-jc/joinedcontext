//! Input: a DPoP proof JWT as a client sends it in the `DPoP` header, then NUL, then a JWK as
//! JSON. Unsigned or forged input is always refused; the thumbprint of any JSON ends in a value
//! or an error.
#![no_main]

use kaisatsu_oidc::verify_dpop_proof;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (proof, jwk) = text.split_once('\0').unwrap_or((&text, ""));
    let verified = verify_dpop_proof(
        proof,
        "POST",
        "https://login.example/token",
        Some("access-token"),
        Some("nonce"),
    );
    assert!(verified.is_err(), "fuzzed proof verified");
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(jwk) {
        if let Ok(jkt) = kaisatsu_oidc::dpop::compute_jwk_thumbprint(&value) {
            assert_eq!(jkt.len(), 43, "thumbprint is base64url sha-256");
        }
    }
});
