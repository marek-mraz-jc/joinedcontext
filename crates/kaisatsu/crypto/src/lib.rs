#![forbid(unsafe_code)]
//! `kaisatsu-crypto` (W10):
//! - Key generation (ES256, RS256, EdDSA) and AES-256-GCM encryption at rest via `aws-lc-rs`.
//! - JWS signing and verification via `jsonwebtoken` 11 (`aws_lc_rs` feature).
//! - Password hashing and Keycloak credential migration (`argon2` v1.3, `pbkdf2-sha512`,
//!   `pbkdf2-sha256`, `pbkdf2-sha1`).
//! - TOTP (`HmacSHA256`, `HmacSHA1`, `HmacSHA512`) and hashed recovery codes.
//! - Constant-time comparisons (`subtle`) and SHA-256 token handle hashing.

pub mod jose;
pub mod keys;
pub mod password;
pub mod totp;
pub mod webauthn;

pub use jose::{DecodedHeader, JwtHeader, sign_jwt, verify_jwt, verify_jwt_with_jwk};
pub use keys::{Kek, KeyManager, PublicJwk, PublicJwkSet, SIGNING_ALGORITHMS};
pub use password::{
    PasswordVerifyOutcome, check_password_policy, hash_password, verify_and_maybe_rehash,
    verify_keycloak_credential,
};
pub use totp::{
    generate_recovery_codes, generate_totp_code, generate_totp_code_for_credential,
    generate_totp_secret, verify_recovery_code, verify_totp_code,
};
pub use webauthn::{
    AUTH_DATA_FLAG_AT, AUTH_DATA_FLAG_BE, AUTH_DATA_FLAG_BS, AUTH_DATA_FLAG_ED, AUTH_DATA_FLAG_UP,
    AUTH_DATA_FLAG_UV, AuthenticationClientResponse, AuthenticatorAssertionResponsePayload,
    AuthenticatorAttestationResponsePayload, COSE_ALG_EDDSA, COSE_ALG_ES256, COSE_ALG_RS256,
    CborValue, CosePublicKey, ParsedAuthData, RegistrationClientResponse, VerifiedAssertion,
    VerifiedRegistration, decode_cbor, decode_cbor_prefix, issue_registration_challenge,
    verify_authentication_response, verify_registration_challenge, verify_registration_response,
};

use rand::Rng as _;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Constant-time comparison of two byte slices or strings (W10, W12).
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// Constant-time string comparison.
#[must_use]
pub fn constant_time_str_eq(a: &str, b: &str) -> bool {
    constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// SHA-256 hex digest of an opaque handle, code, or token (W2).
#[must_use]
pub fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Base64url-no-pad SHA-256 digest (for PKCE S256 and DPoP `ath`).
#[must_use]
pub fn sha256_base64url(input: &str) -> String {
    use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
    let digest = Sha256::digest(input.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// The letters a device user code is made of (RFC 8628 §6.1): no vowels, so no words, and none
/// of the shapes people confuse (0/O, 1/I).
pub const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

/// A CSPRNG device user code `XXXX-XXXX` over [`USER_CODE_ALPHABET`] (20^8, about 2^34.6).
#[must_use]
pub fn random_user_code() -> String {
    let n = USER_CODE_ALPHABET.len();
    // Bytes at or above the largest multiple of 20 are drawn again, so every letter is as likely.
    let limit = (256 / n * n) as u8;
    let mut rng = rand::rng();
    let mut out = String::with_capacity(9);
    while out.len() < 9 {
        if out.len() == 4 {
            out.push('-');
        }
        let b = rng.next_u32() as u8;
        if b < limit {
            out.push(USER_CODE_ALPHABET[usize::from(b) % n] as char);
        }
    }
    out
}

/// The form a user code is compared in: letters upper-cased, everything that is not a letter of
/// [`USER_CODE_ALPHABET`] (hyphens, spaces) dropped.
#[must_use]
pub fn normalize_user_code(raw: &str) -> String {
    raw.chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(|c| c.is_ascii() && USER_CODE_ALPHABET.contains(&(*c as u8)))
        .collect()
}

/// Generates a 256-bit (32-byte) CSPRNG opaque token/session handle encoded as base64url (W2, W10).
#[must_use]
pub fn random_handle_256() -> String {
    use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
