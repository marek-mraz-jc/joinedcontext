#![forbid(unsafe_code)]
//! Signing key generation (ES256, RS256, EdDSA), AES-256-GCM encryption at rest under a KEK
//! read from a file in the secrets mount, rotation, and JWKS publication (W10).

use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use aws_lc_rs::encoding::{AsBigEndian, AsDer};
use aws_lc_rs::rand::{fill as lc_fill, SystemRandom};
use aws_lc_rs::rsa::{KeySize, PrivateDecryptingKey};
use aws_lc_rs::signature::{
    EcdsaKeyPair, Ed25519KeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING,
};
use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use wardkeep_core::{Result, SigningKeyRecord, WardkeepError};
use zeroize::Zeroize;

/// Key-Encryption Key (256-bit AES-256-GCM) read from a file in the secrets mount (W10).
#[derive(Clone)]
pub struct Kek {
    bytes: [u8; 32],
}

impl Drop for Kek {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl std::fmt::Debug for Kek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Kek([redacted])")
    }
}

impl Kek {
    /// Reads the KEK from a file path (raw 32 bytes, 64-char hex, or base64). Refuses empty or
    /// short files (W10).
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path).map_err(|e| {
            WardkeepError::Crypto(format!("cannot read KEK file {}: {e}", path.display()))
        })?;
        Self::from_raw_bytes(&raw)
    }

    pub fn from_raw_bytes(raw: &[u8]) -> Result<Self> {
        let trimmed = raw
            .strip_suffix(b"\r\n")
            .or_else(|| raw.strip_suffix(b"\n"))
            .unwrap_or(raw);
        if trimmed.len() == 32 {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(trimmed);
            return Ok(Self { bytes });
        }
        if let Ok(text) = std::str::from_utf8(trimmed) {
            let text = text.trim();
            if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
                let mut bytes = [0u8; 32];
                for i in 0..32 {
                    bytes[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
                        .map_err(|_| WardkeepError::Crypto("invalid hex in KEK".into()))?;
                }
                return Ok(Self { bytes });
            }
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD
                .decode(text)
                .or_else(|_| URL_SAFE_NO_PAD.decode(text))
            {
                if decoded.len() == 32 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&decoded);
                    return Ok(Self { bytes });
                }
            }
        }
        Err(WardkeepError::Crypto(
            "KEK must be 32 raw bytes, 64 hex characters, or 32-byte base64".into(),
        ))
    }

    #[must_use]
    pub fn generate_for_tests() -> Self {
        let mut bytes = [0u8; 32];
        let _ = lc_fill(&mut bytes);
        Self { bytes }
    }

    /// Encrypts `plaintext` with AES-256-GCM using `aad` (e.g. `realm_id:kid`), returning `nonce || ciphertext || tag`.
    pub fn encrypt(&self, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let unbound = UnboundKey::new(&AES_256_GCM, &self.bytes)
            .map_err(|_| WardkeepError::Crypto("failed to init AES-256-GCM key".into()))?;
        let key = LessSafeKey::new(unbound);
        let mut nonce_bytes = [0u8; 12];
        lc_fill(&mut nonce_bytes)
            .map_err(|_| WardkeepError::Crypto("CSPRNG failure".into()))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut buf = Vec::with_capacity(12 + plaintext.len() + 16);
        buf.extend_from_slice(&nonce_bytes);
        let mut in_out = plaintext.to_vec();
        key.seal_in_place_append_tag(nonce, Aad::from(aad), &mut in_out)
            .map_err(|_| WardkeepError::Crypto("AES-256-GCM seal failed".into()))?;
        buf.extend_from_slice(&in_out);
        Ok(buf)
    }

    /// Decrypts `ciphertext` (`nonce || payload || tag`) with AES-256-GCM and `aad`.
    pub fn decrypt(&self, aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() < 12 + 16 {
            return Err(WardkeepError::Crypto("ciphertext too short".into()));
        }
        let unbound = UnboundKey::new(&AES_256_GCM, &self.bytes)
            .map_err(|_| WardkeepError::Crypto("failed to init AES-256-GCM key".into()))?;
        let key = LessSafeKey::new(unbound);
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes.copy_from_slice(&ciphertext[..12]);
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut in_out = ciphertext[12..].to_vec();
        let plain = key
            .open_in_place(nonce, Aad::from(aad), &mut in_out)
            .map_err(|_| WardkeepError::Crypto("AES-256-GCM open failed (wrong KEK or AAD)".into()))?;
        Ok(plain.to_vec())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicJwk {
    pub kid: String,
    pub kty: String,
    pub alg: String,
    #[serde(rename = "use")]
    pub key_use: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crv: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicJwkSet {
    pub keys: Vec<PublicJwk>,
}

pub struct KeyManager;

impl KeyManager {
    /// Generates an ES256 (P-256) signing key encrypted under `kek` (W10).
    pub fn generate_es256(realm_id: &str, kek: &Kek, priority: i32, status: &str) -> Result<SigningKeyRecord> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| WardkeepError::Crypto("ECDSA P-256 generation failed".into()))?;
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref())
            .map_err(|_| WardkeepError::Crypto("ECDSA P-256 parse failed".into()))?;
        let pub_bytes = pair.public_key().as_ref();
        // Uncompressed P-256 point: 0x04 || X (32 bytes) || Y (32 bytes)
        if pub_bytes.len() != 65 || pub_bytes[0] != 0x04 {
            return Err(WardkeepError::Crypto("unexpected P-256 public key format".into()));
        }
        let x = URL_SAFE_NO_PAD.encode(&pub_bytes[1..33]);
        let y = URL_SAFE_NO_PAD.encode(&pub_bytes[33..65]);
        let kid = format!("es256-{}", &crate::sha256_hex(&format!("{x}.{y}"))[..16]);
        let jwk = PublicJwk {
            kid: kid.clone(),
            kty: "EC".into(),
            alg: "ES256".into(),
            key_use: "sig".into(),
            crv: Some("P-256".into()),
            x: Some(x),
            y: Some(y),
            n: None,
            e: None,
        };
        let aad = format!("{realm_id}:{kid}");
        let encrypted = kek.encrypt(aad.as_bytes(), pkcs8.as_ref())?;
        Ok(SigningKeyRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm_id.to_owned(),
            kid,
            algorithm: "ES256".into(),
            status: status.to_owned(),
            priority,
            public_jwk_json: serde_json::to_string(&jwk)
                .map_err(|e| WardkeepError::Crypto(e.to_string()))?,
            encrypted_private_key: encrypted,
            created_at: Utc::now(),
            activate_at: None,
            expires_at: None,
        })
    }

    /// Generates an RS256 signing key via `aws-lc-rs` (W10, for `edge` / lua-resty-openidc).
    pub fn generate_rs256(realm_id: &str, kek: &Kek, priority: i32, status: &str) -> Result<SigningKeyRecord> {
        let priv_key = PrivateDecryptingKey::generate(KeySize::Rsa2048)
            .map_err(|_| WardkeepError::Crypto("RSA key generation failed".into()))?;
        let pkcs8_der = priv_key
            .as_der()
            .map_err(|_| WardkeepError::Crypto("RSA PKCS#8 DER encode failed".into()))?;
        let pub_key = priv_key.public_key();
        let n_bytes = pub_key
            .modulus()
            .as_be_bytes()
            .map_err(|_| WardkeepError::Crypto("RSA modulus encode failed".into()))?;
        let e_bytes = pub_key
            .exponent()
            .as_be_bytes()
            .map_err(|_| WardkeepError::Crypto("RSA exponent encode failed".into()))?;
        let n = URL_SAFE_NO_PAD.encode(n_bytes.as_ref());
        let e = URL_SAFE_NO_PAD.encode(e_bytes.as_ref());
        let kid = format!("rs256-{}", &crate::sha256_hex(&format!("{n}.{e}"))[..16]);
        let jwk = PublicJwk {
            kid: kid.clone(),
            kty: "RSA".into(),
            alg: "RS256".into(),
            key_use: "sig".into(),
            crv: None,
            x: None,
            y: None,
            n: Some(n),
            e: Some(e),
        };
        let aad = format!("{realm_id}:{kid}");
        let encrypted = kek.encrypt(aad.as_bytes(), pkcs8_der.as_ref())?;
        Ok(SigningKeyRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm_id.to_owned(),
            kid,
            algorithm: "RS256".into(),
            status: status.to_owned(),
            priority,
            public_jwk_json: serde_json::to_string(&jwk)
                .map_err(|err| WardkeepError::Crypto(err.to_string()))?,
            encrypted_private_key: encrypted,
            created_at: Utc::now(),
            activate_at: None,
            expires_at: None,
        })
    }

    /// Generates an EdDSA (Ed25519) signing key via `aws-lc-rs` (W10).
    pub fn generate_eddsa(realm_id: &str, kek: &Kek, priority: i32, status: &str) -> Result<SigningKeyRecord> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8()
            .map_err(|_| WardkeepError::Crypto("Ed25519 key generation failed".into()))?;
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| WardkeepError::Crypto("Ed25519 parse failed".into()))?;
        let x = URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());
        let kid = format!("eddsa-{}", &crate::sha256_hex(&x)[..16]);
        let jwk = PublicJwk {
            kid: kid.clone(),
            kty: "OKP".into(),
            alg: "EdDSA".into(),
            key_use: "sig".into(),
            crv: Some("Ed25519".into()),
            x: Some(x),
            y: None,
            n: None,
            e: None,
        };
        let aad = format!("{realm_id}:{kid}");
        let encrypted = kek.encrypt(aad.as_bytes(), pkcs8.as_ref())?;
        Ok(SigningKeyRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm_id.to_owned(),
            kid,
            algorithm: "EdDSA".into(),
            status: status.to_owned(),
            priority,
            public_jwk_json: serde_json::to_string(&jwk)
                .map_err(|err| WardkeepError::Crypto(err.to_string()))?,
            encrypted_private_key: encrypted,
            created_at: Utc::now(),
            activate_at: None,
            expires_at: None,
        })
    }

    /// Builds the public `JwkSet` from the realm's non-expired keys (`ACTIVE` and `PASSIVE`, W10).
    pub fn build_jwks(records: &[SigningKeyRecord]) -> PublicJwkSet {
        let now = Utc::now();
        let mut keys = Vec::new();
        for rec in records {
            if rec.status == "RETIRED" {
                continue;
            }
            if rec.expires_at.is_some_and(|exp| exp <= now) {
                continue;
            }
            if let Ok(jwk) = serde_json::from_str::<PublicJwk>(&rec.public_jwk_json) {
                keys.push(jwk);
            }
        }
        PublicJwkSet { keys }
    }

    /// Rotates keys for `algorithm`: existing `ACTIVE` keys become `PASSIVE` with `expires_at`
    /// set to `now + 2 * token_ttl`, and a new key is generated (W10).
    pub fn rotate(
        realm_id: &str,
        kek: &Kek,
        existing: &mut [SigningKeyRecord],
        algorithm: &str,
        token_ttl_secs: i64,
        immediate: bool,
    ) -> Result<SigningKeyRecord> {
        let now = Utc::now();
        let mut new_key = match algorithm {
            "RS256" => Self::generate_rs256(
                realm_id,
                kek,
                50,
                if immediate { "ACTIVE" } else { "PASSIVE" },
            )?,
            "EdDSA" => Self::generate_eddsa(
                realm_id,
                kek,
                80,
                if immediate { "ACTIVE" } else { "PASSIVE" },
            )?,
            _ => Self::generate_es256(
                realm_id,
                kek,
                100,
                if immediate { "ACTIVE" } else { "PASSIVE" },
            )?,
        };
        if immediate {
            for k in existing.iter_mut().filter(|k| k.algorithm == algorithm && k.status == "ACTIVE") {
                k.status = "PASSIVE".into();
                k.expires_at = Some(now + Duration::seconds(token_ttl_secs.max(300) * 2));
            }
        } else {
            // W10: published in JWKS one token lifetime before it signs
            new_key.activate_at = Some(now + Duration::seconds(token_ttl_secs.max(300)));
        }
        Ok(new_key)
    }
}
