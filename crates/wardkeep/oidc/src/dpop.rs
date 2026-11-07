#![forbid(unsafe_code)]
//! DPoP (RFC 9449) proof verification and JWK thumbprint (`cnf.jkt`) calculation (W1, W12).

use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use jsonwebtoken::decode_header;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wardkeep_core::{Result, WardkeepError};
use wardkeep_crypto::{sha256_base64url, verify_jwt_with_jwk};

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

/// Computes the RFC 7638 JWK SHA-256 thumbprint (`jkt`) of a public JWK.
pub fn compute_jwk_thumbprint(jwk: &Value) -> Result<String> {
    let kty = jwk
        .get("kty")
        .and_then(Value::as_str)
        .ok_or_else(|| WardkeepError::invalid_dpop_proof("DPoP JWK missing kty"))?;
    let canonical = match kty {
        "EC" => {
            let crv = jwk
                .get("crv")
                .and_then(Value::as_str)
                .ok_or_else(|| WardkeepError::invalid_dpop_proof("EC JWK missing crv"))?;
            let x = jwk
                .get("x")
                .and_then(Value::as_str)
                .ok_or_else(|| WardkeepError::invalid_dpop_proof("EC JWK missing x"))?;
            let y = jwk
                .get("y")
                .and_then(Value::as_str)
                .ok_or_else(|| WardkeepError::invalid_dpop_proof("EC JWK missing y"))?;
            format!(r#"{{"crv":"{crv}","kty":"EC","x":"{x}","y":"{y}"}}"#)
        }
        "RSA" => {
            let e = jwk
                .get("e")
                .and_then(Value::as_str)
                .ok_or_else(|| WardkeepError::invalid_dpop_proof("RSA JWK missing e"))?;
            let n = jwk
                .get("n")
                .and_then(Value::as_str)
                .ok_or_else(|| WardkeepError::invalid_dpop_proof("RSA JWK missing n"))?;
            format!(r#"{{"e":"{e}","kty":"RSA","n":"{n}"}}"#)
        }
        "OKP" => {
            let crv = jwk
                .get("crv")
                .and_then(Value::as_str)
                .ok_or_else(|| WardkeepError::invalid_dpop_proof("OKP JWK missing crv"))?;
            let x = jwk
                .get("x")
                .and_then(Value::as_str)
                .ok_or_else(|| WardkeepError::invalid_dpop_proof("OKP JWK missing x"))?;
            format!(r#"{{"crv":"{crv}","kty":"OKP","x":"{x}"}}"#)
        }
        _ => return Err(WardkeepError::invalid_dpop_proof("unsupported JWK kty")),
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
        .ok_or_else(|| WardkeepError::invalid_dpop_proof("malformed DPoP JWT"))?;
    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_part)
        .map_err(|_| WardkeepError::invalid_dpop_proof("malformed DPoP header base64"))?;
    let raw_header: Value = serde_json::from_slice(&header_bytes)
        .map_err(|_| WardkeepError::invalid_dpop_proof("malformed DPoP header JSON"))?;

    let typ = raw_header
        .get("typ")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !typ.eq_ignore_ascii_case("dpop+jwt") {
        return Err(WardkeepError::invalid_dpop_proof(
            "DPoP header typ must be dpop+jwt",
        ));
    }
    let jwk = raw_header
        .get("jwk")
        .ok_or_else(|| WardkeepError::invalid_dpop_proof("DPoP header missing jwk"))?;
    if jwk.get("d").is_some() {
        return Err(WardkeepError::invalid_dpop_proof(
            "DPoP header jwk must not contain private key material",
        ));
    }

    let jkt = compute_jwk_thumbprint(jwk)?;
    let _ = decode_header(proof_jwt)
        .map_err(|_| WardkeepError::invalid_dpop_proof("invalid DPoP header"))?;

    // Build a synthetic `exp` in claims validation by decoding signature against `jwk`
    let payload_part = proof_jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| WardkeepError::invalid_dpop_proof("malformed DPoP payload"))?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_part)
        .map_err(|_| WardkeepError::invalid_dpop_proof("malformed DPoP payload base64"))?;
    let claims: DpopClaims = serde_json::from_slice(&payload_bytes)
        .map_err(|_| WardkeepError::invalid_dpop_proof("invalid DPoP claims"))?;

    let now = Utc::now().timestamp();
    if (now - claims.iat).abs() > 300 {
        return Err(WardkeepError::invalid_dpop_proof(
            "DPoP proof iat is outside the 300s freshness window",
        ));
    }
    if !claims.htm.eq_ignore_ascii_case(expected_method) {
        return Err(WardkeepError::invalid_dpop_proof("DPoP htm mismatch"));
    }
    let clean_htu = expected_htu.split('?').next().unwrap_or(expected_htu);
    let proof_htu = claims.htu.split('?').next().unwrap_or(&claims.htu);
    if proof_htu != clean_htu {
        return Err(WardkeepError::invalid_dpop_proof("DPoP htu mismatch"));
    }
    if let Some(at) = expected_access_token {
        let expected_ath = sha256_base64url(at);
        if claims.ath.as_deref() != Some(expected_ath.as_str()) {
            return Err(WardkeepError::invalid_dpop_proof("DPoP ath mismatch"));
        }
    }
    if let Some(req_nonce) = expected_nonce
        && claims.nonce.as_deref() != Some(req_nonce)
    {
        return Err(WardkeepError::use_dpop_nonce(
            "DPoP nonce required or mismatched",
        ));
    }

    // Verify signature using `jsonwebtoken`
    let alg = decode_header(proof_jwt)
        .map_err(|_| WardkeepError::invalid_dpop_proof("bad header"))?
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
        return Err(WardkeepError::invalid_dpop_proof(
            "DPoP proof algorithm must be asymmetric",
        ));
    }
    let mut validation = jsonwebtoken::Validation::new(alg);
    validation.validate_exp = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let jwk_parsed: jsonwebtoken::jwk::Jwk = serde_json::from_value(jwk.clone())
        .map_err(|_| WardkeepError::invalid_dpop_proof("invalid DPoP JWK"))?;
    let decoding_key = jsonwebtoken::DecodingKey::from_jwk(&jwk_parsed)
        .map_err(|_| WardkeepError::invalid_dpop_proof("cannot decode DPoP JWK"))?;
    jsonwebtoken::decode::<Value>(proof_jwt, &decoding_key, &validation)
        .map_err(|_| WardkeepError::invalid_dpop_proof("DPoP signature verification failed"))?;

    let _ = json!({});
    let _ = verify_jwt_with_jwk::<Value>;

    Ok(DpopProof {
        jti: claims.jti,
        jkt,
        nonce: claims.nonce,
    })
}
