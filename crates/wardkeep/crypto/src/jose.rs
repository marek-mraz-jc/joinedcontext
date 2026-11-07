#![forbid(unsafe_code)]
//! JWS signing and verification via `jsonwebtoken` 11 (`aws_lc_rs`), pinning the algorithm from the
//! key and never from the untrusted token header (W10, W12).

use crate::keys::{Kek, PublicJwk, SigningKeyRecord};
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use wardkeep_core::{Result, WardkeepError};

pub type JwtHeader = Header;
pub type DecodedHeader = Header;

fn parse_alg(alg: &str) -> Result<Algorithm> {
    match alg {
        "ES256" => Ok(Algorithm::ES256),
        "RS256" => Ok(Algorithm::RS256),
        "EdDSA" => Ok(Algorithm::EdDSA),
        _ => Err(WardkeepError::Crypto(format!(
            "unsupported signing algorithm: {alg} (HS* is forbidden by W10)"
        ))),
    }
}

/// Signs `claims` with `key_record` after decrypting its PKCS#8 DER under `kek`.
pub fn sign_jwt<T: Serialize>(
    key_record: &SigningKeyRecord,
    kek: &Kek,
    typ: Option<&str>,
    claims: &T,
) -> Result<String> {
    let alg = parse_alg(&key_record.algorithm)?;
    let aad = format!("{}:{}", key_record.realm_id, key_record.kid);
    let pkcs8_der = kek.decrypt(aad.as_bytes(), &key_record.encrypted_private_key)?;
    let encoding_key = match alg {
        Algorithm::ES256 => EncodingKey::from_ec_der(&pkcs8_der),
        Algorithm::RS256 => EncodingKey::from_rsa_der(&pkcs8_der),
        Algorithm::EdDSA => EncodingKey::from_ed_der(&pkcs8_der),
        _ => return Err(WardkeepError::Crypto("forbidden algorithm".into())),
    };
    let mut header = Header::new(alg);
    header.kid = Some(key_record.kid.clone());
    header.typ = typ.map(str::to_owned).or_else(|| Some("JWT".into()));
    encode(&header, claims, &encoding_key)
        .map_err(|e| WardkeepError::Crypto(format!("JWT sign failed: {e}")))
}

/// Verifies a JWT against the realm's `SigningKeyRecord`s. Algorithm is pinned by the key found by
/// `kid`, never by the token header alone (W10, W12).
pub fn verify_jwt<T: DeserializeOwned>(
    token: &str,
    keys: &[SigningKeyRecord],
    expected_issuer: Option<&str>,
    expected_audience: Option<&str>,
) -> Result<(Header, T)> {
    let header = decode_header(token)
        .map_err(|_| WardkeepError::Unauthorized("malformed JWT header".into()))?;
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| WardkeepError::Unauthorized("JWT missing kid".into()))?;
    let rec = keys
        .iter()
        .find(|k| k.kid == kid && k.status != "RETIRED")
        .ok_or_else(|| WardkeepError::Unauthorized("unknown or retired kid".into()))?;
    let key_alg = parse_alg(&rec.algorithm)?;
    if header.alg != key_alg {
        return Err(WardkeepError::Unauthorized("algorithm mismatch with key".into()));
    }
    let pub_jwk: PublicJwk = serde_json::from_str(&rec.public_jwk_json)
        .map_err(|_| WardkeepError::Crypto("invalid stored public JWK".into()))?;
    verify_jwt_with_public_jwk(token, &pub_jwk, expected_issuer, expected_audience)
}

pub fn verify_jwt_with_public_jwk<T: DeserializeOwned>(
    token: &str,
    pub_jwk: &PublicJwk,
    expected_issuer: Option<&str>,
    expected_audience: Option<&str>,
) -> Result<(Header, T)> {
    let jwk_val = serde_json::to_value(pub_jwk)
        .map_err(|e| WardkeepError::Crypto(e.to_string()))?;
    verify_jwt_with_jwk(token, &jwk_val, expected_issuer, expected_audience)
}

/// Verifies a JWT against an arbitrary JWK JSON value (used for DPoP proofs, RFC 7523 assertions,
/// and Kubernetes federated-jwt client assertions).
pub fn verify_jwt_with_jwk<T: DeserializeOwned>(
    token: &str,
    jwk_json: &Value,
    expected_issuer: Option<&str>,
    expected_audience: Option<&str>,
) -> Result<(Header, T)> {
    let header = decode_header(token)
        .map_err(|_| WardkeepError::Unauthorized("malformed JWT header".into()))?;
    if !matches!(header.alg, Algorithm::ES256 | Algorithm::RS256 | Algorithm::EdDSA) {
        return Err(WardkeepError::Unauthorized(
            "unsupported JWT signature algorithm".into(),
        ));
    }
    let jwk: Jwk = serde_json::from_value(jwk_json.clone())
        .map_err(|_| WardkeepError::Unauthorized("invalid JWK".into()))?;
    let decoding_key = DecodingKey::from_jwk(&jwk)
        .map_err(|_| WardkeepError::Unauthorized("cannot build decoding key from JWK".into()))?;
    let mut validation = Validation::new(header.alg);
    if let Some(iss) = expected_issuer {
        validation.set_issuer(&[iss]);
    }
    if let Some(aud) = expected_audience {
        validation.set_audience(&[aud]);
    } else {
        validation.validate_aud = false;
    }
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.leeway = 30;
    let data = decode::<T>(token, &decoding_key, &validation)
        .map_err(|_| WardkeepError::Unauthorized("JWT signature or claim validation failed".into()))?;
    Ok((data.header, data.claims))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn signs_and_verifies_es256_rs256_and_eddsa_and_rejects_alg_confusion() {
        let kek = Kek::generate_for_tests();
        let es_key = KeyManager::generate_es256("dev", &kek, 100, "ACTIVE").expect("es256");
        let rs_key = KeyManager::generate_rs256("dev", &kek, 50, "ACTIVE").expect("rs256");
        let ed_key = KeyManager::generate_eddsa("dev", &kek, 80, "ACTIVE").expect("eddsa");
        let keys = vec![es_key.clone(), rs_key.clone(), ed_key.clone()];

        let claims = json!({
            "iss": "https://idm.example.test/realms/dev",
            "sub": "u1",
            "aud": "portal-api",
            "exp": chrono::Utc::now().timestamp() + 300,
        });

        for k in &keys {
            let jwt = sign_jwt(k, &kek, Some("JWT"), &claims).expect("sign");
            let (_, verified): (_, Value) = verify_jwt(
                &jwt,
                &keys,
                Some("https://idm.example.test/realms/dev"),
                Some("portal-api"),
            )
            .expect("verify");
            assert_eq!(verified["sub"], "u1");
        }
    }
}
