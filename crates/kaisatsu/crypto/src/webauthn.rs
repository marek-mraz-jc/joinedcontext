#![forbid(unsafe_code)]
//! W3C WebAuthn Level 3 (§6.1, §6.5.1, §7.1, §7.2, §8.7) ceremony verification, bounded CBOR
//! decoder, COSE_Key parser, and signature verification via `aws-lc-rs` (W7, W10, K11).
//!
//! No external CBOR crate is used: [`decode_cbor`] parses RFC 8949 major types 0–5 and 7 with
//! definite lengths only, bounded at [`MAX_CBOR_BYTES`] (64 KiB), [`MAX_CBOR_DEPTH`] (8), and
//! [`MAX_CBOR_ITEMS`] (1024), using checked slicing and arithmetic so malformed or oversized
//! inputs never panic.

use crate::{constant_time_eq, constant_time_str_eq};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents,
    UnparsedPublicKey,
};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use kaisatsu_core::{KaisatsuError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Maximum CBOR payload size (64 KiB, rule 3).
pub const MAX_CBOR_BYTES: usize = 64 * 1024;
/// Maximum CBOR nesting depth (8, rule 3).
pub const MAX_CBOR_DEPTH: usize = 8;
/// Maximum CBOR items in one decoded structure.
pub const MAX_CBOR_ITEMS: usize = 1024;
/// Maximum WebAuthn credentialId byte length (W3C WebAuthn L3 §6.5.1 & §7.1 step 25).
pub const MAX_CREDENTIAL_ID_LEN: usize = 1023;

/// COSE algorithm identifiers offered and verified by kaisatsu (W3C WebAuthn L3 §6.5.1, K11).
pub const COSE_ALG_ES256: i64 = -7;
pub const COSE_ALG_EDDSA: i64 = -8;
pub const COSE_ALG_RS256: i64 = -257;

/// Authenticator data flags (W3C WebAuthn L3 §6.1 Table 1).
pub const AUTH_DATA_FLAG_UP: u8 = 1 << 0;
pub const AUTH_DATA_FLAG_UV: u8 = 1 << 2;
pub const AUTH_DATA_FLAG_BE: u8 = 1 << 3;
pub const AUTH_DATA_FLAG_BS: u8 = 1 << 4;
pub const AUTH_DATA_FLAG_AT: u8 = 1 << 6;
pub const AUTH_DATA_FLAG_ED: u8 = 1 << 7;

/// Decoded CBOR value within the WebAuthn subset (RFC 8949 major types 0–5 and 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborValue {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<CborValue>),
    Map(Vec<(CborValue, CborValue)>),
    Bool(bool),
    Null,
}

impl CborValue {
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn map_get_str(&self, key: &str) -> Option<&CborValue> {
        let Self::Map(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(_, v)| v)
    }

    #[must_use]
    pub fn map_get_int(&self, key: i64) -> Option<&CborValue> {
        let Self::Map(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find(|(k, _)| k.as_i64() == Some(key))
            .map(|(_, v)| v)
    }
}

struct CborReader<'a> {
    buf: &'a [u8],
    pos: usize,
    items: usize,
}

impl<'a> CborReader<'a> {
    fn new(buf: &'a [u8]) -> Result<Self> {
        if buf.len() > MAX_CBOR_BYTES {
            return Err(KaisatsuError::BadRequest(
                "CBOR input exceeds 64 KiB limit".into(),
            ));
        }
        Ok(Self {
            buf,
            pos: 0,
            items: 0,
        })
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| KaisatsuError::BadRequest("CBOR length overflow".into()))?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| KaisatsuError::BadRequest("unexpected EOF in CBOR input".into()))?;
        self.pos = end;
        Ok(slice)
    }

    fn read_uint(&mut self, ai: u8) -> Result<u64> {
        match ai {
            0..=23 => Ok(u64::from(ai)),
            24 => {
                let b = self.take(1)?;
                Ok(u64::from(b[0]))
            }
            25 => {
                let b = self.take(2)?;
                Ok(u64::from(u16::from_be_bytes([b[0], b[1]])))
            }
            26 => {
                let b = self.take(4)?;
                Ok(u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
            }
            27 => {
                let b = self.take(8)?;
                Ok(u64::from_be_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ]))
            }
            31 => Err(KaisatsuError::BadRequest(
                "indefinite-length CBOR items are refused".into(),
            )),
            _ => Err(KaisatsuError::BadRequest(
                "invalid CBOR additional information".into(),
            )),
        }
    }

    fn decode_item(&mut self, depth: usize) -> Result<CborValue> {
        if depth > MAX_CBOR_DEPTH {
            return Err(KaisatsuError::BadRequest(
                "CBOR nesting depth exceeds maximum of 8".into(),
            ));
        }
        self.items = self.items.saturating_add(1);
        if self.items > MAX_CBOR_ITEMS {
            return Err(KaisatsuError::BadRequest(
                "CBOR item count exceeds maximum".into(),
            ));
        }
        let head = self.take(1)?[0];
        let major = head >> 5;
        let ai = head & 0x1f;
        match major {
            0 => {
                let v = self.read_uint(ai)?;
                let i = i64::try_from(v).map_err(|_| {
                    KaisatsuError::BadRequest("CBOR unsigned int out of i64 range".into())
                })?;
                Ok(CborValue::Int(i))
            }
            1 => {
                let v = self.read_uint(ai)?;
                let i = i64::try_from(v).map_err(|_| {
                    KaisatsuError::BadRequest("CBOR negative int out of i64 range".into())
                })?;
                let neg = (-1i64).checked_sub(i).ok_or_else(|| {
                    KaisatsuError::BadRequest("CBOR negative int overflow".into())
                })?;
                Ok(CborValue::Int(neg))
            }
            2 => {
                let len_u64 = self.read_uint(ai)?;
                let len = usize::try_from(len_u64)
                    .map_err(|_| KaisatsuError::BadRequest("CBOR byte string too large".into()))?;
                if len > MAX_CBOR_BYTES {
                    return Err(KaisatsuError::BadRequest(
                        "CBOR byte string exceeds 64 KiB".into(),
                    ));
                }
                let bytes = self.take(len)?;
                Ok(CborValue::Bytes(bytes.to_vec()))
            }
            3 => {
                let len_u64 = self.read_uint(ai)?;
                let len = usize::try_from(len_u64)
                    .map_err(|_| KaisatsuError::BadRequest("CBOR text string too large".into()))?;
                if len > MAX_CBOR_BYTES {
                    return Err(KaisatsuError::BadRequest(
                        "CBOR text string exceeds 64 KiB".into(),
                    ));
                }
                let bytes = self.take(len)?;
                let s = std::str::from_utf8(bytes)
                    .map_err(|_| KaisatsuError::BadRequest("CBOR text is not valid UTF-8".into()))?
                    .to_owned();
                Ok(CborValue::Text(s))
            }
            4 => {
                let len_u64 = self.read_uint(ai)?;
                let len = usize::try_from(len_u64)
                    .map_err(|_| KaisatsuError::BadRequest("CBOR array too large".into()))?;
                if len > MAX_CBOR_ITEMS {
                    return Err(KaisatsuError::BadRequest(
                        "CBOR array length exceeds maximum".into(),
                    ));
                }
                let mut arr = Vec::with_capacity(len.min(64));
                for _ in 0..len {
                    arr.push(self.decode_item(depth + 1)?);
                }
                Ok(CborValue::Array(arr))
            }
            5 => {
                let len_u64 = self.read_uint(ai)?;
                let len = usize::try_from(len_u64)
                    .map_err(|_| KaisatsuError::BadRequest("CBOR map too large".into()))?;
                if len > MAX_CBOR_ITEMS {
                    return Err(KaisatsuError::BadRequest(
                        "CBOR map length exceeds maximum".into(),
                    ));
                }
                let mut entries = Vec::with_capacity(len.min(64));
                for _ in 0..len {
                    let k = self.decode_item(depth + 1)?;
                    let v = self.decode_item(depth + 1)?;
                    entries.push((k, v));
                }
                Ok(CborValue::Map(entries))
            }
            7 => match ai {
                20 => Ok(CborValue::Bool(false)),
                21 => Ok(CborValue::Bool(true)),
                22 | 23 => Ok(CborValue::Null),
                _ => Err(KaisatsuError::BadRequest(
                    "unsupported CBOR simple/float value".into(),
                )),
            },
            _ => Err(KaisatsuError::BadRequest(
                "unsupported CBOR major type".into(),
            )),
        }
    }
}

/// Decodes one complete CBOR item from `bytes`, refusing trailing bytes.
pub fn decode_cbor(bytes: &[u8]) -> Result<CborValue> {
    let (val, consumed) = decode_cbor_prefix(bytes)?;
    if consumed != bytes.len() {
        return Err(KaisatsuError::BadRequest(
            "unexpected trailing bytes after CBOR item".into(),
        ));
    }
    Ok(val)
}

/// Decodes one CBOR item from the start of `bytes` and returns `(value, bytes_consumed)`
/// (used when parsing `credentialPublicKey` followed by optional `extensions` in `authData`,
/// W3C WebAuthn L3 §6.1 & §6.5.1).
pub fn decode_cbor_prefix(bytes: &[u8]) -> Result<(CborValue, usize)> {
    let mut reader = CborReader::new(bytes)?;
    let val = reader.decode_item(0)?;
    Ok((val, reader.pos))
}

/// Parsed COSE_Key credential public key (RFC 9052 §7, RFC 9053 §2 & §7, W3C WebAuthn L3 §6.5.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kty", rename_all = "UPPERCASE")]
pub enum CosePublicKey {
    /// kty=2 (EC2), crv=1 (P-256), alg=-7 (ES256).
    Ec2 {
        alg: i64,
        crv: i64,
        x: String,
        y: String,
    },
    /// kty=1 (OKP), crv=6 (Ed25519), alg=-8 (EdDSA).
    Okp { alg: i64, crv: i64, x: String },
    /// kty=3 (RSA), alg=-257 (RS256).
    Rsa { alg: i64, n: String, e: String },
}

impl CosePublicKey {
    #[must_use]
    pub fn alg(&self) -> i64 {
        match self {
            Self::Ec2 { alg, .. } | Self::Okp { alg, .. } | Self::Rsa { alg, .. } => *alg,
        }
    }

    /// Parses a `COSE_Key` CBOR map (W3C WebAuthn L3 §6.5.1).
    pub fn from_cbor(map: &CborValue) -> Result<Self> {
        let kty = map
            .map_get_int(1)
            .and_then(CborValue::as_i64)
            .ok_or_else(|| KaisatsuError::BadRequest("COSE_Key missing kty (1)".into()))?;
        let alg = map
            .map_get_int(3)
            .and_then(CborValue::as_i64)
            .ok_or_else(|| KaisatsuError::BadRequest("COSE_Key missing alg (3)".into()))?;

        match (kty, alg) {
            (2, COSE_ALG_ES256) => {
                let crv = map
                    .map_get_int(-1)
                    .and_then(CborValue::as_i64)
                    .ok_or_else(|| {
                        KaisatsuError::BadRequest("EC2 COSE_Key missing crv (-1)".into())
                    })?;
                if crv != 1 {
                    return Err(KaisatsuError::BadRequest(
                        "EC2 COSE_Key must use P-256 curve (crv=1)".into(),
                    ));
                }
                let x = map
                    .map_get_int(-2)
                    .and_then(CborValue::as_bytes)
                    .filter(|b| b.len() == 32)
                    .ok_or_else(|| {
                        KaisatsuError::BadRequest("EC2 COSE_Key x (-2) must be 32 bytes".into())
                    })?;
                let y = map
                    .map_get_int(-3)
                    .and_then(CborValue::as_bytes)
                    .filter(|b| b.len() == 32)
                    .ok_or_else(|| {
                        KaisatsuError::BadRequest("EC2 COSE_Key y (-3) must be 32 bytes".into())
                    })?;
                Ok(Self::Ec2 {
                    alg,
                    crv,
                    x: URL_SAFE_NO_PAD.encode(x),
                    y: URL_SAFE_NO_PAD.encode(y),
                })
            }
            (1, COSE_ALG_EDDSA) => {
                let crv = map
                    .map_get_int(-1)
                    .and_then(CborValue::as_i64)
                    .ok_or_else(|| {
                        KaisatsuError::BadRequest("OKP COSE_Key missing crv (-1)".into())
                    })?;
                if crv != 6 {
                    return Err(KaisatsuError::BadRequest(
                        "OKP COSE_Key must use Ed25519 curve (crv=6)".into(),
                    ));
                }
                let x = map
                    .map_get_int(-2)
                    .and_then(CborValue::as_bytes)
                    .filter(|b| b.len() == 32)
                    .ok_or_else(|| {
                        KaisatsuError::BadRequest("OKP COSE_Key x (-2) must be 32 bytes".into())
                    })?;
                Ok(Self::Okp {
                    alg,
                    crv,
                    x: URL_SAFE_NO_PAD.encode(x),
                })
            }
            (3, COSE_ALG_RS256) => {
                let n = map
                    .map_get_int(-1)
                    .and_then(CborValue::as_bytes)
                    .filter(|b| b.len() >= 256 && b.len() <= 1024)
                    .ok_or_else(|| {
                        KaisatsuError::BadRequest(
                            "RSA COSE_Key modulus n (-1) must be 2048..8192 bits".into(),
                        )
                    })?;
                let e = map
                    .map_get_int(-2)
                    .and_then(CborValue::as_bytes)
                    .filter(|b| !b.is_empty() && b.len() <= 8)
                    .ok_or_else(|| {
                        KaisatsuError::BadRequest("RSA COSE_Key exponent e (-2) invalid".into())
                    })?;
                Ok(Self::Rsa {
                    alg,
                    n: URL_SAFE_NO_PAD.encode(n),
                    e: URL_SAFE_NO_PAD.encode(e),
                })
            }
            _ => Err(KaisatsuError::BadRequest(format!(
                "unsupported COSE_Key kty={kty} alg={alg}"
            ))),
        }
    }

    /// Verifies `signature` over `signed_bytes` (`authData || sha256(clientDataJSON)`) using
    /// `aws-lc-rs` (W3C WebAuthn L3 §7.2 step 21).
    pub fn verify_signature(&self, signed_bytes: &[u8], signature: &[u8]) -> Result<()> {
        match self {
            Self::Ec2 { x, y, .. } => {
                let xb = URL_SAFE_NO_PAD
                    .decode(x)
                    .map_err(|_| KaisatsuError::Crypto("invalid stored EC2 x".into()))?;
                let yb = URL_SAFE_NO_PAD
                    .decode(y)
                    .map_err(|_| KaisatsuError::Crypto("invalid stored EC2 y".into()))?;
                if xb.len() != 32 || yb.len() != 32 {
                    return Err(KaisatsuError::Crypto(
                        "invalid stored EC2 key length".into(),
                    ));
                }
                let mut uncompressed = Vec::with_capacity(65);
                uncompressed.push(0x04);
                uncompressed.extend_from_slice(&xb);
                uncompressed.extend_from_slice(&yb);
                let pub_key = UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &uncompressed);
                pub_key.verify(signed_bytes, signature).map_err(|_| {
                    KaisatsuError::Unauthorized("WebAuthn ECDSA signature invalid".into())
                })
            }
            Self::Okp { x, .. } => {
                let xb = URL_SAFE_NO_PAD
                    .decode(x)
                    .map_err(|_| KaisatsuError::Crypto("invalid stored OKP x".into()))?;
                let pub_key = UnparsedPublicKey::new(&ED25519, &xb);
                pub_key.verify(signed_bytes, signature).map_err(|_| {
                    KaisatsuError::Unauthorized("WebAuthn Ed25519 signature invalid".into())
                })
            }
            Self::Rsa { n, e, .. } => {
                let nb = URL_SAFE_NO_PAD
                    .decode(n)
                    .map_err(|_| KaisatsuError::Crypto("invalid stored RSA n".into()))?;
                let eb = URL_SAFE_NO_PAD
                    .decode(e)
                    .map_err(|_| KaisatsuError::Crypto("invalid stored RSA e".into()))?;
                let components = RsaPublicKeyComponents { n: &nb, e: &eb };
                components
                    .verify(&RSA_PKCS1_2048_8192_SHA256, signed_bytes, signature)
                    .map_err(|_| {
                        KaisatsuError::Unauthorized("WebAuthn RSA signature invalid".into())
                    })
            }
        }
    }
}

/// Parsed authenticator data structure (W3C WebAuthn L3 §6.1 & §6.5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAuthData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    pub attested: Option<AttestedCredentialData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestedCredentialData {
    pub aaguid: [u8; 16],
    pub credential_id: Vec<u8>,
    pub public_key: CosePublicKey,
}

impl ParsedAuthData {
    #[must_use]
    pub fn user_present(&self) -> bool {
        (self.flags & AUTH_DATA_FLAG_UP) != 0
    }

    #[must_use]
    pub fn user_verified(&self) -> bool {
        (self.flags & AUTH_DATA_FLAG_UV) != 0
    }

    #[must_use]
    pub fn backup_eligible(&self) -> bool {
        (self.flags & AUTH_DATA_FLAG_BE) != 0
    }

    #[must_use]
    pub fn backup_state(&self) -> bool {
        (self.flags & AUTH_DATA_FLAG_BS) != 0
    }

    /// Parses `authData` bytes according to W3C WebAuthn L3 §6.1 & §6.5.1.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let head = bytes.get(..37).ok_or_else(|| {
            KaisatsuError::BadRequest("authenticatorData shorter than 37 bytes".into())
        })?;
        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&head[..32]);
        let flags = head[32];
        let sign_count = u32::from_be_bytes([head[33], head[34], head[35], head[36]]);

        // W3C WebAuthn L3 §7.1 step 17 & §7.2 step 18: If BE is not set, verify that BS is not set.
        if (flags & AUTH_DATA_FLAG_BE) == 0 && (flags & AUTH_DATA_FLAG_BS) != 0 {
            return Err(KaisatsuError::BadRequest(
                "authenticatorData has BS=1 while BE=0".into(),
            ));
        }

        let mut offset = 37usize;
        let attested = if (flags & AUTH_DATA_FLAG_AT) != 0 {
            let at_head = bytes
                .get(offset..offset.saturating_add(18))
                .ok_or_else(|| {
                    KaisatsuError::BadRequest("truncated attestedCredentialData header".into())
                })?;
            let mut aaguid = [0u8; 16];
            aaguid.copy_from_slice(&at_head[..16]);
            let cred_len = usize::from(u16::from_be_bytes([at_head[16], at_head[17]]));
            // W3C WebAuthn L3 §6.5.1 & §7.1 step 25: credentialIdLength MUST be <= 1023.
            if cred_len == 0 || cred_len > MAX_CREDENTIAL_ID_LEN {
                return Err(KaisatsuError::BadRequest(
                    "credentialId length must be 1..=1023 bytes".into(),
                ));
            }
            offset = offset.saturating_add(18);
            let cred_end = offset
                .checked_add(cred_len)
                .ok_or_else(|| KaisatsuError::BadRequest("credentialId offset overflow".into()))?;
            let credential_id = bytes
                .get(offset..cred_end)
                .ok_or_else(|| KaisatsuError::BadRequest("truncated credentialId".into()))?
                .to_vec();
            offset = cred_end;

            let rest = bytes
                .get(offset..)
                .ok_or_else(|| KaisatsuError::BadRequest("missing credentialPublicKey".into()))?;
            let (cose_cbor, consumed) = decode_cbor_prefix(rest)?;
            let public_key = CosePublicKey::from_cbor(&cose_cbor)?;
            offset = offset.saturating_add(consumed);
            Some(AttestedCredentialData {
                aaguid,
                credential_id,
                public_key,
            })
        } else {
            None
        };

        if (flags & AUTH_DATA_FLAG_ED) != 0 {
            let ext_rest = bytes.get(offset..).ok_or_else(|| {
                KaisatsuError::BadRequest("missing authenticator extensions".into())
            })?;
            let (ext_cbor, consumed) = decode_cbor_prefix(ext_rest)?;
            if !matches!(ext_cbor, CborValue::Map(_)) {
                return Err(KaisatsuError::BadRequest(
                    "authenticator extensions must be a CBOR map".into(),
                ));
            }
            offset = offset.saturating_add(consumed);
        }

        if offset != bytes.len() {
            return Err(KaisatsuError::BadRequest(
                "trailing bytes in authenticatorData".into(),
            ));
        }

        Ok(Self {
            rp_id_hash,
            flags,
            sign_count,
            attested,
        })
    }
}

/// ClientDataJSON structure (W3C WebAuthn L3 §5.8.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectedClientData {
    #[serde(rename = "type")]
    pub client_type: String,
    pub challenge: String,
    pub origin: String,
    #[serde(default)]
    pub cross_origin: Option<bool>,
    #[serde(default)]
    pub top_origin: Option<String>,
    #[serde(default)]
    pub token_binding: Option<serde_json::Value>,
}

/// Registration response payload from `navigator.credentials.create()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrationClientResponse {
    pub id: String,
    #[serde(default)]
    pub raw_id: Option<String>,
    #[serde(rename = "type")]
    pub cred_type: String,
    pub response: AuthenticatorAttestationResponsePayload,
    #[serde(default)]
    pub authenticator_attachment: Option<String>,
    #[serde(default)]
    pub client_extension_results: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthenticatorAttestationResponsePayload {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub attestation_object: String,
    #[serde(default)]
    pub transports: Vec<String>,
}

/// Authentication assertion payload from `navigator.credentials.get()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthenticationClientResponse {
    pub id: String,
    #[serde(default)]
    pub raw_id: Option<String>,
    #[serde(rename = "type")]
    pub cred_type: String,
    pub response: AuthenticatorAssertionResponsePayload,
    #[serde(default)]
    pub authenticator_attachment: Option<String>,
    #[serde(default)]
    pub client_extension_results: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthenticatorAssertionResponsePayload {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
    #[serde(default)]
    pub user_handle: Option<String>,
}

/// Verified registration result ready to store in `WebAuthnCredentialRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRegistration {
    pub credential_id_b64: String,
    pub public_key: CosePublicKey,
    pub sign_count: u32,
    pub uv_initialized: bool,
    pub backup_eligible: bool,
    pub backup_state: bool,
    pub attestation_type: String,
    pub transports: Vec<String>,
}

/// Verified authentication result ready to update `WebAuthnCredentialRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAssertion {
    pub credential_id_b64: String,
    pub new_sign_count: u32,
    pub user_verified: bool,
    pub backup_state: bool,
    pub user_handle: Option<String>,
}

fn parse_client_data(
    client_data_b64: &str,
    expected_type: &str,
    expected_challenge_b64: &str,
    allowed_origins: &[String],
) -> Result<(Vec<u8>, [u8; 32])> {
    let raw = URL_SAFE_NO_PAD
        .decode(client_data_b64.trim())
        .map_err(|_| KaisatsuError::BadRequest("invalid clientDataJSON base64url".into()))?;
    // W3C WebAuthn L3 §7.1 step 5 / §7.2 step 8: UTF-8 decode, stripping any leading UTF-8 BOM.
    let text = std::str::from_utf8(&raw)
        .map_err(|_| KaisatsuError::BadRequest("clientDataJSON is not UTF-8".into()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let c: CollectedClientData = serde_json::from_str(text)
        .map_err(|_| KaisatsuError::BadRequest("invalid clientDataJSON structure".into()))?;

    // Step 7 / 10: Verify C.type
    if c.client_type != expected_type {
        return Err(KaisatsuError::BadRequest(format!(
            "clientDataJSON type must be {expected_type}"
        )));
    }
    // Step 8 / 11: Verify C.challenge in constant time
    if expected_challenge_b64.is_empty()
        || !constant_time_str_eq(&c.challenge, expected_challenge_b64)
    {
        return Err(KaisatsuError::BadRequest(
            "clientDataJSON challenge mismatch".into(),
        ));
    }
    // Step 9 / 12: Verify C.origin is in allowed_origins
    if !allowed_origins
        .iter()
        .any(|o| o.eq_ignore_ascii_case(&c.origin))
    {
        return Err(KaisatsuError::BadRequest(
            "clientDataJSON origin is not permitted".into(),
        ));
    }
    // Step 10–11 / 13–14: Cross-origin iframe creation/assertion is refused by policy.
    if c.cross_origin == Some(true) || c.top_origin.is_some() {
        return Err(KaisatsuError::BadRequest(
            "crossOrigin WebAuthn ceremony is refused".into(),
        ));
    }

    let digest = Sha256::digest(&raw);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    Ok((raw, hash))
}

/// W3C WebAuthn Level 3 §7.1: Verifies a registration ceremony response.
pub fn verify_registration_response(
    payload: &RegistrationClientResponse,
    expected_challenge_b64: &str,
    expected_rp_id: &str,
    allowed_origins: &[String],
    require_user_verification: bool,
    allowed_cose_algs: &[i64],
) -> Result<VerifiedRegistration> {
    if payload.cred_type != "public-key" {
        return Err(KaisatsuError::BadRequest(
            "credential.type must be public-key".into(),
        ));
    }
    // Steps 5–12: Parse and verify clientDataJSON (`webauthn.create`)
    let (_cdata_raw, _cdata_hash) = parse_client_data(
        &payload.response.client_data_json,
        "webauthn.create",
        expected_challenge_b64,
        allowed_origins,
    )?;

    // Step 13: Perform CBOR decoding on attestationObject to obtain fmt, authData, attStmt
    let att_bytes = URL_SAFE_NO_PAD
        .decode(payload.response.attestation_object.trim())
        .map_err(|_| KaisatsuError::BadRequest("invalid attestationObject base64url".into()))?;
    let att_cbor = decode_cbor(&att_bytes)?;
    let _fmt = att_cbor
        .map_get_str("fmt")
        .and_then(CborValue::as_str)
        .ok_or_else(|| KaisatsuError::BadRequest("attestationObject missing fmt".into()))?;
    let auth_data_bytes = att_cbor
        .map_get_str("authData")
        .and_then(CborValue::as_bytes)
        .ok_or_else(|| KaisatsuError::BadRequest("attestationObject missing authData".into()))?;
    let _att_stmt = att_cbor
        .map_get_str("attStmt")
        .ok_or_else(|| KaisatsuError::BadRequest("attestationObject missing attStmt".into()))?;

    let auth_data = ParsedAuthData::parse(auth_data_bytes)?;

    // Step 14: Verify rpIdHash == SHA-256(expected_rp_id)
    let expected_rp_hash = Sha256::digest(expected_rp_id.as_bytes());
    if !constant_time_eq(&auth_data.rp_id_hash, &expected_rp_hash) {
        return Err(KaisatsuError::BadRequest(
            "WebAuthn rpIdHash mismatch".into(),
        ));
    }

    // Step 15: Verify UP bit is set
    if !auth_data.user_present() {
        return Err(KaisatsuError::BadRequest(
            "WebAuthn user presence (UP) flag is not set".into(),
        ));
    }

    // Step 16: If user verification is required, verify UV bit is set
    if require_user_verification && !auth_data.user_verified() {
        return Err(KaisatsuError::BadRequest(
            "WebAuthn user verification (UV) flag is required".into(),
        ));
    }

    let sign_count = auth_data.sign_count;
    let uv_initialized = auth_data.user_verified();
    let backup_eligible = auth_data.backup_eligible();
    let backup_state = auth_data.backup_state();
    let attested = auth_data.attested.ok_or_else(|| {
        KaisatsuError::BadRequest(
            "registration authenticatorData missing attestedCredentialData".into(),
        )
    })?;

    // Step 20: Verify alg matches one of the offered pubKeyCredParams
    if !allowed_cose_algs.contains(&attested.public_key.alg()) {
        return Err(KaisatsuError::BadRequest(
            "credential public key alg is not in offered pubKeyCredParams".into(),
        ));
    }

    let cred_id_b64 = URL_SAFE_NO_PAD.encode(&attested.credential_id);
    if payload.id != cred_id_b64 {
        return Err(KaisatsuError::BadRequest(
            "credential.id does not match attested credentialId".into(),
        ));
    }
    if let Some(ref raw_id) = payload.raw_id
        && raw_id != &cred_id_b64
    {
        return Err(KaisatsuError::BadRequest(
            "credential.rawId does not match attested credentialId".into(),
        ));
    }

    // Steps 21–24 (§8.7 None attestation conveyance): Accept any fmt with None attestation type.
    Ok(VerifiedRegistration {
        credential_id_b64: cred_id_b64,
        public_key: attested.public_key,
        sign_count,
        uv_initialized,
        backup_eligible,
        backup_state,
        attestation_type: "None".into(),
        transports: payload.response.transports.clone(),
    })
}

/// W3C WebAuthn Level 3 §7.2: Verifies an authentication assertion response against a stored
/// credential record.
#[allow(clippy::too_many_arguments)]
pub fn verify_authentication_response(
    payload: &AuthenticationClientResponse,
    expected_challenge_b64: &str,
    expected_rp_id: &str,
    allowed_origins: &[String],
    require_user_verification: bool,
    stored_credential_id_b64: &str,
    stored_user_handle: &str,
    stored_public_key: &CosePublicKey,
    stored_sign_count: u32,
    stored_backup_eligible: bool,
) -> Result<VerifiedAssertion> {
    if payload.cred_type != "public-key" {
        return Err(KaisatsuError::BadRequest(
            "credential.type must be public-key".into(),
        ));
    }
    if payload.id != stored_credential_id_b64 {
        return Err(KaisatsuError::Unauthorized(
            "credential.id does not match stored credential".into(),
        ));
    }
    // Step 6: If userHandle is present, verify it matches the user's handle
    if let Some(ref uh) = payload.response.user_handle
        && !uh.is_empty()
        && !constant_time_str_eq(uh, stored_user_handle)
    {
        return Err(KaisatsuError::Unauthorized(
            "WebAuthn userHandle does not match user account".into(),
        ));
    }

    // Steps 8–14: Parse and verify clientDataJSON (`webauthn.get`)
    let (_cdata_raw, cdata_hash) = parse_client_data(
        &payload.response.client_data_json,
        "webauthn.get",
        expected_challenge_b64,
        allowed_origins,
    )?;

    let auth_data_bytes = URL_SAFE_NO_PAD
        .decode(payload.response.authenticator_data.trim())
        .map_err(|_| KaisatsuError::BadRequest("invalid authenticatorData base64url".into()))?;
    let auth_data = ParsedAuthData::parse(&auth_data_bytes)?;

    // §6.1: For assertion signatures, the AT flag MUST NOT be set.
    if auth_data.attested.is_some() {
        return Err(KaisatsuError::BadRequest(
            "assertion authenticatorData must not include attestedCredentialData".into(),
        ));
    }

    // Step 15: Verify rpIdHash
    let expected_rp_hash = Sha256::digest(expected_rp_id.as_bytes());
    if !constant_time_eq(&auth_data.rp_id_hash, &expected_rp_hash) {
        return Err(KaisatsuError::Unauthorized(
            "WebAuthn rpIdHash mismatch".into(),
        ));
    }

    // Step 16: Verify UP bit is set
    if !auth_data.user_present() {
        return Err(KaisatsuError::Unauthorized(
            "WebAuthn user presence (UP) flag is not set".into(),
        ));
    }

    // Step 17: Verify UV bit when required
    if require_user_verification && !auth_data.user_verified() {
        return Err(KaisatsuError::Unauthorized(
            "WebAuthn user verification (UV) is required".into(),
        ));
    }

    // Step 19: Verify backup eligibility consistency with registration record
    if auth_data.backup_eligible() != stored_backup_eligible {
        return Err(KaisatsuError::Unauthorized(
            "WebAuthn backup eligibility (BE) changed since registration".into(),
        ));
    }

    // Step 20–21: Verify signature over authData || sha256(clientDataJSON)
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(payload.response.signature.trim())
        .map_err(|_| KaisatsuError::BadRequest("invalid signature base64url".into()))?;
    let mut signed = Vec::with_capacity(auth_data_bytes.len() + 32);
    signed.extend_from_slice(&auth_data_bytes);
    signed.extend_from_slice(&cdata_hash);
    stored_public_key.verify_signature(&signed, &sig_bytes)?;

    // Step 22: Signature counter regression check
    if (auth_data.sign_count != 0 || stored_sign_count != 0)
        && auth_data.sign_count <= stored_sign_count
    {
        return Err(KaisatsuError::Unauthorized(
            "WebAuthn signature counter regression detected".into(),
        ));
    }

    Ok(VerifiedAssertion {
        credential_id_b64: payload.id.clone(),
        new_sign_count: auth_data.sign_count,
        user_verified: auth_data.user_verified(),
        backup_state: auth_data.backup_state(),
        user_handle: payload.response.user_handle.clone(),
    })
}

/// Lifetime of a stateless registration challenge (WebAuthn L3 §13.4.3 asks for a challenge the
/// Relying Party generated and remembers; the MAC is how it remembers without storage).
pub const REGISTRATION_CHALLENGE_SECS: i64 = 300;
const REG_CHALLENGE_PURPOSE: &str = "kaisatsu:webauthn-reg-challenge:v1";
const REG_CHALLENGE_LEN: usize = 16 + 8 + 32;

fn registration_challenge_mac(
    kek: &crate::Kek,
    realm_id: &str,
    user_id: &str,
    body: &[u8],
) -> [u8; 32] {
    let mut msg = Vec::with_capacity(realm_id.len() + user_id.len() + 2 + body.len());
    msg.extend_from_slice(realm_id.as_bytes());
    msg.push(0);
    msg.extend_from_slice(user_id.as_bytes());
    msg.push(0);
    msg.extend_from_slice(body);
    kek.mac(REG_CHALLENGE_PURPOSE, &msg)
}

/// Issues a registration challenge for `(realm_id, user_id)`: 16 random bytes, the expiry and an
/// HMAC under a KEK subkey, base64url. Only [`verify_registration_challenge`] accepts it, so a
/// caller cannot pick its own challenge (WebAuthn L3 §7.1 step 8).
#[must_use]
pub fn issue_registration_challenge(
    kek: &crate::Kek,
    realm_id: &str,
    user_id: &str,
    now_unix: i64,
) -> String {
    use rand::Rng as _;
    let mut body = [0u8; 24];
    rand::rng().fill_bytes(&mut body[..16]);
    body[16..].copy_from_slice(
        &now_unix
            .saturating_add(REGISTRATION_CHALLENGE_SECS)
            .to_be_bytes(),
    );
    let mac = registration_challenge_mac(kek, realm_id, user_id, &body);
    let mut out = Vec::with_capacity(REG_CHALLENGE_LEN);
    out.extend_from_slice(&body);
    out.extend_from_slice(&mac);
    URL_SAFE_NO_PAD.encode(out)
}

/// True when `challenge` was issued by [`issue_registration_challenge`] for this realm and user
/// and has not expired. Single use is the caller's job (record it as revoked).
#[must_use]
pub fn verify_registration_challenge(
    kek: &crate::Kek,
    realm_id: &str,
    user_id: &str,
    challenge: &str,
    now_unix: i64,
) -> bool {
    let Ok(raw) = URL_SAFE_NO_PAD.decode(challenge.trim()) else {
        return false;
    };
    let (Some(body), Some(mac)) = (raw.get(..24), raw.get(24..)) else {
        return false;
    };
    if raw.len() != REG_CHALLENGE_LEN {
        return false;
    }
    let expected = registration_challenge_mac(kek, realm_id, user_id, body);
    if !constant_time_eq(mac, &expected) {
        return false;
    }
    let mut exp = [0u8; 8];
    exp.copy_from_slice(body.get(16..24).unwrap_or(&[0u8; 8]));
    i64::from_be_bytes(exp) > now_unix
}
