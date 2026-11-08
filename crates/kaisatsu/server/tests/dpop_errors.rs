#![forbid(unsafe_code)]
//! Malformed DPoP proofs (RFC 9449 §4.3) are refused with `invalid_dpop_proof`, never a panic:
//! a header without `jwk`, an unknown `alg`, a payload that is not base64url, and a `jwk` that
//! has a thumbprint but is no usable public key.

use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use kaisatsu_core::KaisatsuError;
use kaisatsu_oidc::verify_dpop_proof;
use serde_json::{Value, json};

const HTU: &str = "https://idm.example.test/realms/dev/protocol/openid-connect/token";

fn b64(v: &Value) -> String {
    URL_SAFE_NO_PAD.encode(v.to_string())
}

fn claims() -> Value {
    json!({ "jti": "j-err", "htm": "POST", "htu": HTU, "iat": chrono::Utc::now().timestamp() })
}

/// A proof assembled by hand: the signature segment is junk, so every case must stop before or
/// at the signature check.
fn hand_proof(header: Value, payload: &str) -> String {
    format!("{}.{payload}.c2lnbmF0dXJl", b64(&header))
}

fn refusal(p: &str) -> String {
    match verify_dpop_proof(p, "POST", HTU, None, None) {
        Err(KaisatsuError::OAuth { error, .. }) => error,
        other => panic!("expected an OAuth refusal, got {other:?}"),
    }
}

#[test]
fn malformed_proofs_are_refused_without_panic() {
    let ec = json!({ "kty": "EC", "crv": "P-256",
        "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
        "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" });
    let payload = b64(&claims());
    let cases = [
        // No `jwk` in the header.
        hand_proof(json!({ "alg": "ES256", "typ": "dpop+jwt" }), &payload),
        // A `jwk` with a thumbprint but an `alg` the JOSE library does not know.
        hand_proof(
            json!({ "alg": "XX42", "typ": "dpop+jwt", "jwk": ec }),
            &payload,
        ),
        // A payload segment that is not base64url.
        hand_proof(
            json!({ "alg": "ES256", "typ": "dpop+jwt", "jwk": ec }),
            "!!!",
        ),
        // A curve the JWK parser refuses (the thumbprint only needs the members).
        hand_proof(
            json!({ "alg": "ES256", "typ": "dpop+jwt",
                "jwk": { "kty": "EC", "crv": "P-999", "x": "eA", "y": "eQ" } }),
            &payload,
        ),
        // Coordinates that are no point: the key cannot be built.
        hand_proof(
            json!({ "alg": "ES256", "typ": "dpop+jwt",
                "jwk": { "kty": "EC", "crv": "P-256", "x": "a", "y": "b" } }),
            &payload,
        ),
        // A well-formed key whose signature does not match.
        hand_proof(
            json!({ "alg": "ES256", "typ": "dpop+jwt", "jwk": ec }),
            &payload,
        ),
        // Not base64 in the header at all.
        "%%%.e30.c2ln".to_owned(),
    ];
    for p in &cases {
        assert_eq!(refusal(p), "invalid_dpop_proof", "{p}");
    }
}
