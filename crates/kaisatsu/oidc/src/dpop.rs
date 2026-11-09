#![forbid(unsafe_code)]
//! DPoP (RFC 9449) proof verification and JWK thumbprint (`cnf.jkt`) calculation (W1, W12).

use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use jsonwebtoken::decode_header;
use kaisatsu_core::{KaisatsuError, Result};
use kaisatsu_crypto::{sha256_base64url, verify_jwt_with_jwk};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DpopClaims {
    pub jti: String,
    pub htm: String,
    pub htu: String,
    pub iat: i64,
    #[serde(default)]
    pub ath: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpopProof {
    pub jti: String,
    pub jkt: String,
    pub nonce: Option<String>,
}

/// Server-managed DPoP nonce window: 60 seconds per slot, accepting the current and previous slot.
pub const DPOP_NONCE_SLOT_SECS: i64 = 60;

/// Mints a stateless HMAC-SHA256 DPoP nonce for `(realm_id, purpose)` at `now_unix` (RFC 9449 §8 & §9, K6).
#[must_use]
pub fn issue_dpop_nonce(
    kek: &kaisatsu_crypto::Kek,
    realm_id: &str,
    purpose: &str,
    now_unix: i64,
) -> String {
    let slot = now_unix / DPOP_NONCE_SLOT_SECS;
    compute_dpop_nonce_for_slot(kek, realm_id, purpose, slot)
}

fn compute_dpop_nonce_for_slot(
    kek: &kaisatsu_crypto::Kek,
    realm_id: &str,
    purpose: &str,
    slot: i64,
) -> String {
    let tag_bytes = kek.mac(
        "kaisatsu:dpop-nonce:v1",
        format!("{purpose}:{realm_id}:{slot}").as_bytes(),
    );
    let tag = URL_SAFE_NO_PAD.encode(tag_bytes);
    format!("{slot}.{tag}")
}

/// Verifies a client-supplied DPoP `nonce` against the current and previous time slot in constant time.
#[must_use]
pub fn verify_dpop_nonce(
    kek: &kaisatsu_crypto::Kek,
    realm_id: &str,
    purpose: &str,
    presented: &str,
    now_unix: i64,
) -> bool {
    let cur_slot = now_unix / DPOP_NONCE_SLOT_SECS;
    let cur = compute_dpop_nonce_for_slot(kek, realm_id, purpose, cur_slot);
    let prev = compute_dpop_nonce_for_slot(kek, realm_id, purpose, cur_slot - 1);
    kaisatsu_crypto::constant_time_str_eq(presented, &cur)
        || kaisatsu_crypto::constant_time_str_eq(presented, &prev)
}

/// Whether the realm requires server-issued DPoP nonces (`attributes["dpop.require.nonce"] == "true"`).
#[must_use]
pub fn realm_requires_dpop_nonce(realm: &kaisatsu_core::RealmConfig) -> bool {
    realm
        .attributes
        .get("dpop.require.nonce")
        .is_some_and(|v| v == "true")
}

/// Computes the RFC 7638 JWK SHA-256 thumbprint (`jkt`) of a public JWK.
pub fn compute_jwk_thumbprint(jwk: &Value) -> Result<String> {
    let kty = jwk
        .get("kty")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::invalid_dpop_proof("DPoP JWK missing kty"))?;
    let canonical = match kty {
        "EC" => {
            let crv = jwk
                .get("crv")
                .and_then(Value::as_str)
                .ok_or_else(|| KaisatsuError::invalid_dpop_proof("EC JWK missing crv"))?;
            let x = jwk
                .get("x")
                .and_then(Value::as_str)
                .ok_or_else(|| KaisatsuError::invalid_dpop_proof("EC JWK missing x"))?;
            let y = jwk
                .get("y")
                .and_then(Value::as_str)
                .ok_or_else(|| KaisatsuError::invalid_dpop_proof("EC JWK missing y"))?;
            format!(r#"{{"crv":"{crv}","kty":"EC","x":"{x}","y":"{y}"}}"#)
        }
        "RSA" => {
            let e = jwk
                .get("e")
                .and_then(Value::as_str)
                .ok_or_else(|| KaisatsuError::invalid_dpop_proof("RSA JWK missing e"))?;
            let n = jwk
                .get("n")
                .and_then(Value::as_str)
                .ok_or_else(|| KaisatsuError::invalid_dpop_proof("RSA JWK missing n"))?;
            format!(r#"{{"e":"{e}","kty":"RSA","n":"{n}"}}"#)
        }
        "OKP" => {
            let crv = jwk
                .get("crv")
                .and_then(Value::as_str)
                .ok_or_else(|| KaisatsuError::invalid_dpop_proof("OKP JWK missing crv"))?;
            let x = jwk
                .get("x")
                .and_then(Value::as_str)
                .ok_or_else(|| KaisatsuError::invalid_dpop_proof("OKP JWK missing x"))?;
            format!(r#"{{"crv":"{crv}","kty":"OKP","x":"{x}"}}"#)
        }
        _ => return Err(KaisatsuError::invalid_dpop_proof("unsupported JWK kty")),
    };
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(digest))
}

/// Verifies an RFC 9449 `DPoP` header proof JWT, checking `typ == dpop+jwt`, embedded public JWK,
/// signature, `htm`, `htu`, `iat` window, optional `ath` (access token hash), and optional server
/// `nonce` (GHSA-cprq-r4fp-jx46 regression protection, W1, W12).
pub fn verify_dpop_proof(
    proof_jwt: &str,
    expected_method: &str,
    expected_htu: &str,
    expected_access_token: Option<&str>,
    expected_nonce: Option<&str>,
) -> Result<DpopProof> {
    let header_part = proof_jwt
        .split('.')
        .next()
        .ok_or_else(|| KaisatsuError::invalid_dpop_proof("malformed DPoP JWT"))?;
    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_part)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("malformed DPoP header base64"))?;
    let raw_header: Value = serde_json::from_slice(&header_bytes)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("malformed DPoP header JSON"))?;

    let typ = raw_header
        .get("typ")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !typ.eq_ignore_ascii_case("dpop+jwt") {
        return Err(KaisatsuError::invalid_dpop_proof(
            "DPoP header typ must be dpop+jwt",
        ));
    }
    let jwk = raw_header
        .get("jwk")
        .ok_or_else(|| KaisatsuError::invalid_dpop_proof("DPoP header missing jwk"))?;
    if jwk.get("d").is_some() {
        return Err(KaisatsuError::invalid_dpop_proof(
            "DPoP header jwk must not contain private key material",
        ));
    }

    let jkt = compute_jwk_thumbprint(jwk)?;
    let _ = decode_header(proof_jwt)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("invalid DPoP header"))?;

    // Build a synthetic `exp` in claims validation by decoding signature against `jwk`
    let payload_part = proof_jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| KaisatsuError::invalid_dpop_proof("malformed DPoP payload"))?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_part)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("malformed DPoP payload base64"))?;
    let claims: DpopClaims = serde_json::from_slice(&payload_bytes)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("invalid DPoP claims"))?;

    let now = Utc::now().timestamp();
    if (now - claims.iat).abs() > 300 {
        return Err(KaisatsuError::invalid_dpop_proof(
            "DPoP proof iat is outside the 300s freshness window",
        ));
    }
    if !claims.htm.eq_ignore_ascii_case(expected_method) {
        return Err(KaisatsuError::invalid_dpop_proof("DPoP htm mismatch"));
    }
    let clean_htu = expected_htu.split('?').next().unwrap_or(expected_htu);
    let proof_htu = claims.htu.split('?').next().unwrap_or(&claims.htu);
    if proof_htu != clean_htu {
        return Err(KaisatsuError::invalid_dpop_proof("DPoP htu mismatch"));
    }
    if let Some(at) = expected_access_token {
        let expected_ath = sha256_base64url(at);
        if claims.ath.as_deref() != Some(expected_ath.as_str()) {
            return Err(KaisatsuError::invalid_dpop_proof("DPoP ath mismatch"));
        }
    }
    if let Some(req_nonce) = expected_nonce
        && claims.nonce.as_deref() != Some(req_nonce)
    {
        return Err(KaisatsuError::use_dpop_nonce(
            "DPoP nonce required or mismatched",
        ));
    }

    // Verify signature using `jsonwebtoken`
    let alg = decode_header(proof_jwt)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("bad header"))?
        .alg;
    // RFC 9449 §4.2: asymmetric algorithms only; a MAC (`HS*`) proof is refused outright.
    if !matches!(
        alg,
        jsonwebtoken::Algorithm::ES256
            | jsonwebtoken::Algorithm::ES384
            | jsonwebtoken::Algorithm::RS256
            | jsonwebtoken::Algorithm::PS256
            | jsonwebtoken::Algorithm::EdDSA
    ) {
        return Err(KaisatsuError::invalid_dpop_proof(
            "DPoP proof algorithm must be asymmetric",
        ));
    }
    let mut validation = jsonwebtoken::Validation::new(alg);
    validation.validate_exp = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let jwk_parsed: jsonwebtoken::jwk::Jwk = serde_json::from_value(jwk.clone())
        .map_err(|_| KaisatsuError::invalid_dpop_proof("invalid DPoP JWK"))?;
    let decoding_key = jsonwebtoken::DecodingKey::from_jwk(&jwk_parsed)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("cannot decode DPoP JWK"))?;
    jsonwebtoken::decode::<Value>(proof_jwt, &decoding_key, &validation)
        .map_err(|_| KaisatsuError::invalid_dpop_proof("DPoP signature verification failed"))?;

    let _ = json!({});
    let _ = verify_jwt_with_jwk::<Value>;

    Ok(DpopProof {
        jti: claims.jti,
        jkt,
        nonce: claims.nonce,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use kaisatsu_crypto::{Kek, KeyManager};

    const HTU: &str = "https://idm.example.test/realms/dev/protocol/openid-connect/token";

    struct Holder {
        jwk: Value,
        key: EncodingKey,
    }

    fn holder() -> Holder {
        let kek = Kek::generate_for_tests();
        let rec = KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").unwrap();
        let der = kek
            .decrypt(
                format!("dev:{}", rec.kid).as_bytes(),
                &rec.encrypted_private_key,
            )
            .unwrap();
        Holder {
            jwk: serde_json::from_str(&rec.public_jwk_json).unwrap(),
            key: EncodingKey::from_ec_der(&der),
        }
    }

    fn proof_with(h: &Holder, typ: &str, jwk: &Value, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(typ.to_owned());
        header.jwk = Some(serde_json::from_value(jwk.clone()).unwrap());
        encode(&header, claims, &h.key).unwrap()
    }

    fn claims(extra: Value) -> Value {
        let mut c =
            json!({ "jti": "j-1", "htm": "POST", "htu": HTU, "iat": Utc::now().timestamp() });
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    }

    fn oauth_error(e: KaisatsuError) -> String {
        match e {
            KaisatsuError::OAuth { error, .. } => error,
            other => panic!("not an OAuth error: {other:?}"),
        }
    }

    #[test]
    fn rfc7638_thumbprint_vector_and_key_types() {
        let rsa = json!({ "kty": "RSA", "e": "AQAB", "alg": "RS256", "kid": "2011-04-29",
            "n": "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw" });
        assert_eq!(
            compute_jwk_thumbprint(&rsa).unwrap(),
            "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs"
        );
        // Members outside the RFC 7638 set never change the thumbprint.
        let ec = json!({ "kty": "EC", "crv": "P-256", "x": "a", "y": "b" });
        let ec_noise =
            json!({ "kty": "EC", "crv": "P-256", "x": "a", "y": "b", "kid": "k", "use": "sig" });
        assert_eq!(
            compute_jwk_thumbprint(&ec).unwrap(),
            compute_jwk_thumbprint(&ec_noise).unwrap()
        );
        assert!(
            compute_jwk_thumbprint(&json!({ "kty": "OKP", "crv": "Ed25519", "x": "q" })).is_ok()
        );
        for bad in [
            json!({}),
            json!({ "kty": "oct", "k": "c2VjcmV0" }),
            json!({ "kty": "EC", "x": "a", "y": "b" }),
            json!({ "kty": "EC", "crv": "P-256", "y": "b" }),
            json!({ "kty": "EC", "crv": "P-256", "x": "a" }),
            json!({ "kty": "RSA", "n": "n" }),
            json!({ "kty": "RSA", "e": "AQAB" }),
            json!({ "kty": "OKP", "x": "q" }),
            json!({ "kty": "OKP", "crv": "Ed25519" }),
        ] {
            assert_eq!(
                oauth_error(compute_jwk_thumbprint(&bad).unwrap_err()),
                "invalid_dpop_proof"
            );
        }
    }

    #[test]
    fn a_fresh_proof_binds_the_key_thumbprint() {
        let h = holder();
        let p = proof_with(&h, "dpop+jwt", &h.jwk, &claims(json!({})));
        let out = verify_dpop_proof(&p, "post", &format!("{HTU}?x=1"), None, None).unwrap();
        assert_eq!(out.jti, "j-1");
        assert_eq!(out.jkt, compute_jwk_thumbprint(&h.jwk).unwrap());
        assert_eq!(out.nonce, None);
    }

    #[test]
    fn wrong_type_private_key_time_method_or_target_is_refused() {
        let h = holder();
        // A header carrying private key material, assembled by hand (the typed Jwk drops `d`).
        let mut with_d = h.jwk.clone();
        with_d["d"] = json!("private");
        let valid = proof_with(&h, "dpop+jwt", &h.jwk, &claims(json!({})));
        let mut parts = valid.splitn(2, '.');
        let _ = parts.next();
        let header_d = URL_SAFE_NO_PAD
            .encode(json!({ "alg": "ES256", "typ": "dpop+jwt", "jwk": with_d }).to_string());
        let leaked = format!("{header_d}.{}", parts.next().unwrap());
        // The two iat cases sit 10 s past the 300 s window: proofs are built before the loop
        // verifies them, and on a loaded runner a second passes in between (now + 301 was then read
        // 300 s ahead, inside the window, and passed).
        let cases = [
            proof_with(&h, "JWT", &h.jwk, &claims(json!({}))),
            leaked,
            proof_with(
                &h,
                "dpop+jwt",
                &h.jwk,
                &claims(json!({ "iat": Utc::now().timestamp() - 310 })),
            ),
            proof_with(
                &h,
                "dpop+jwt",
                &h.jwk,
                &claims(json!({ "iat": Utc::now().timestamp() + 310 })),
            ),
            proof_with(&h, "dpop+jwt", &h.jwk, &claims(json!({ "htm": "GET" }))),
            proof_with(
                &h,
                "dpop+jwt",
                &h.jwk,
                &claims(json!({ "htu": "https://evil.test/token" })),
            ),
            proof_with(
                &h,
                "dpop+jwt",
                &h.jwk,
                &json!({ "htm": "POST", "htu": HTU }),
            ),
        ];
        for p in cases {
            let e = verify_dpop_proof(&p, "POST", HTU, None, None).unwrap_err();
            assert_eq!(oauth_error(e), "invalid_dpop_proof");
        }
        for garbage in ["", "@@@.x.y", "e30.@@@.x", "bm90IGpzb24.e30.x"] {
            assert!(verify_dpop_proof(garbage, "POST", HTU, None, None).is_err());
        }
    }

    #[test]
    fn a_proof_signed_by_another_key_or_with_a_mac_is_refused() {
        let h = holder();
        let other = holder();
        // Header names h's public key, signature made with the other private key.
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".into());
        header.jwk = Some(serde_json::from_value(h.jwk.clone()).unwrap());
        let forged = encode(&header, &claims(json!({})), &other.key).unwrap();
        let e = verify_dpop_proof(&forged, "POST", HTU, None, None).unwrap_err();
        assert_eq!(oauth_error(e), "invalid_dpop_proof");

        let mut header = Header::new(Algorithm::HS256);
        header.typ = Some("dpop+jwt".into());
        header.jwk = Some(serde_json::from_value(h.jwk.clone()).unwrap());
        let mac = encode(&header, &claims(json!({})), &EncodingKey::from_secret(b"k")).unwrap();
        let e = verify_dpop_proof(&mac, "POST", HTU, None, None).unwrap_err();
        assert_eq!(oauth_error(e), "invalid_dpop_proof");
    }

    #[test]
    fn access_token_hash_and_server_nonce_must_match() {
        let h = holder();
        let ath = sha256_base64url("the-access-token");
        let good = proof_with(
            &h,
            "dpop+jwt",
            &h.jwk,
            &claims(json!({ "ath": ath, "nonce": "n-1" })),
        );
        assert!(
            verify_dpop_proof(&good, "POST", HTU, Some("the-access-token"), Some("n-1")).is_ok()
        );
        let e = verify_dpop_proof(&good, "POST", HTU, Some("another-token"), None).unwrap_err();
        assert_eq!(oauth_error(e), "invalid_dpop_proof");
        let no_ath = proof_with(&h, "dpop+jwt", &h.jwk, &claims(json!({})));
        assert!(verify_dpop_proof(&no_ath, "POST", HTU, Some("the-access-token"), None).is_err());
        let e = verify_dpop_proof(&good, "POST", HTU, None, Some("n-2")).unwrap_err();
        assert_eq!(oauth_error(e), "use_dpop_nonce");
        let e = verify_dpop_proof(&no_ath, "POST", HTU, None, Some("n-1")).unwrap_err();
        assert_eq!(oauth_error(e), "use_dpop_nonce");
    }
}
