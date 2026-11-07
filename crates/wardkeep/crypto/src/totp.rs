#![forbid(unsafe_code)]
//! TOTP verification and generation matching RI (`HmacSHA256`, 6 digits, 30 s, look-ahead 1,
//! UTF-8 raw secret or BASE32) and 10 single-use hashed recovery codes (W1, W7).

use crate::{constant_time_str_eq, sha256_hex};
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng as _;
use serde::Deserialize;
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use wardkeep_core::{CredentialRecord, Result, WardkeepError};

#[derive(Debug, Deserialize)]
struct OtpSecretData {
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtpCredentialData {
    #[serde(default = "default_digits")]
    digits: u32,
    #[serde(default = "default_period")]
    period: u64,
    #[serde(default = "default_alg")]
    algorithm: String,
    #[serde(default)]
    secret_encoding: Option<String>,
}

fn default_digits() -> u32 {
    6
}
fn default_period() -> u64 {
    30
}
fn default_alg() -> String {
    "HmacSHA256".into()
}

fn compute_hotp_at(secret_bytes: &[u8], counter: u64, digits: u32, alg: &str) -> Result<String> {
    let msg = counter.to_be_bytes();
    let hash_bytes: Vec<u8> = match alg {
        "HmacSHA1" | "SHA1" => {
            let mut mac = Hmac::<Sha1>::new_from_slice(secret_bytes)
                .map_err(|_| WardkeepError::Crypto("invalid TOTP key".into()))?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        "HmacSHA512" | "SHA512" => {
            let mut mac = Hmac::<Sha512>::new_from_slice(secret_bytes)
                .map_err(|_| WardkeepError::Crypto("invalid TOTP key".into()))?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        _ => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret_bytes)
                .map_err(|_| WardkeepError::Crypto("invalid TOTP key".into()))?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
    };
    let offset = (hash_bytes[hash_bytes.len() - 1] & 0x0f) as usize;
    let binary = ((u32::from(hash_bytes[offset] & 0x7f)) << 24)
        | ((u32::from(hash_bytes[offset + 1])) << 16)
        | ((u32::from(hash_bytes[offset + 2])) << 8)
        | u32::from(hash_bytes[offset + 3]);
    let modulo = 10u32.pow(digits.clamp(6, 8));
    let otp = binary % modulo;
    Ok(format!("{otp:0width$}", width = digits as usize))
}

fn extract_secret_and_meta(cred: &CredentialRecord) -> Result<(Vec<u8>, OtpCredentialData)> {
    let raw_secret = if cred.secret_data.trim_start().starts_with('{') {
        serde_json::from_str::<OtpSecretData>(&cred.secret_data)
            .map(|s| s.value)
            .map_err(|_| WardkeepError::Crypto("invalid OTP secretData JSON".into()))?
    } else {
        cred.secret_data.clone()
    };
    let meta: OtpCredentialData = if cred.credential_data.trim().is_empty() {
        OtpCredentialData {
            digits: 6,
            period: 30,
            algorithm: "HmacSHA256".into(),
            secret_encoding: None,
        }
    } else {
        serde_json::from_str(&cred.credential_data)
            .map_err(|_| WardkeepError::Crypto("invalid OTP credentialData JSON".into()))?
    };
    // RI note: "Keycloak keeps an OTP secret as the text itself and hashes its UTF-8 bytes"
    let bytes = if meta.secret_encoding.as_deref() == Some("BASE32") {
        let secret = totp_rs::Secret::try_from_base32(raw_secret.trim())
            .map_err(|_| WardkeepError::Crypto("invalid BASE32 OTP secret".into()))?;
        AsRef::<[u8]>::as_ref(&secret).to_vec()
    } else {
        raw_secret.into_bytes()
    };
    Ok((bytes, meta))
}

/// Generates a fresh 160-bit TOTP secret, BASE32 without padding (RFC 4226 §4 minimum 128 bits),
/// for an enrolment the server shows once and stores with `secretEncoding: "BASE32"`.
#[must_use]
pub fn generate_totp_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::rng().fill_bytes(&mut bytes);
    totp_rs::Secret::from(bytes)
        .to_base32()
        .trim_end_matches('=')
        .to_owned()
}

/// Generates the current TOTP code for `unix_secs` (for tests and enrolment verification).
pub fn generate_totp_code(
    secret: &str,
    digits: u32,
    period: u64,
    algorithm: &str,
    unix_secs: u64,
) -> Result<String> {
    let step = unix_secs / period.max(1);
    compute_hotp_at(secret.as_bytes(), step, digits, algorithm)
}

/// Generates the code `cred` (raw or BASE32 secret, its digits, period and algorithm) shows at
/// `unix_secs`, as an authenticator app would.
pub fn generate_totp_code_for_credential(
    cred: &CredentialRecord,
    unix_secs: u64,
) -> Result<String> {
    let (secret_bytes, meta) = extract_secret_and_meta(cred)?;
    compute_hotp_at(
        &secret_bytes,
        unix_secs / meta.period.max(1),
        meta.digits,
        &meta.algorithm,
    )
}

/// Verifies `submitted_code` against `cred` within `look_ahead_window` steps around `unix_secs`.
/// Returns `Ok(Some(matched_step))` so the caller can record `matched_step` in `revoked_token`
/// (`single_use`) and refuse replay of a used code (W7).
pub fn verify_totp_code(
    cred: &CredentialRecord,
    submitted_code: &str,
    look_ahead_window: u32,
    unix_secs: u64,
) -> Result<Option<u64>> {
    let trimmed = submitted_code.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let (secret_bytes, meta) = extract_secret_and_meta(cred)?;
    let current_step = unix_secs / meta.period.max(1);
    let win = u64::from(look_ahead_window);
    let start = current_step.saturating_sub(win);
    let end = current_step.saturating_add(win);
    for step in start..=end {
        let candidate = compute_hotp_at(&secret_bytes, step, meta.digits, &meta.algorithm)?;
        if constant_time_str_eq(&candidate, trimmed) {
            return Ok(Some(step));
        }
    }
    Ok(None)
}

/// Generates 10 single-use recovery codes (plaintext to show once, and their SHA-256 hashes for storage, W7).
#[must_use]
pub fn generate_recovery_codes() -> (Vec<String>, Vec<String>) {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    let mut plain = Vec::with_capacity(10);
    let mut hashes = Vec::with_capacity(10);
    for _ in 0..10 {
        let mut bytes = [0u8; 10];
        rng.fill_bytes(&mut bytes);
        let mut code = String::with_capacity(11);
        for (i, b) in bytes.iter().enumerate() {
            if i == 5 {
                code.push('-');
            }
            code.push(ALPHABET[(*b as usize) % ALPHABET.len()] as char);
        }
        hashes.push(sha256_hex(&code));
        plain.push(code);
    }
    (plain, hashes)
}

/// Verifies a recovery code against a slice of stored SHA-256 hashes, returning the index to consume.
#[must_use]
pub fn verify_recovery_code(submitted: &str, stored_hashes: &[String]) -> Option<usize> {
    let candidate = sha256_hex(&submitted.trim().to_ascii_uppercase());
    stored_hashes
        .iter()
        .position(|h| constant_time_str_eq(h, &candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secret_is_160_bit_base32_and_round_trips_through_a_credential() {
        let a = generate_totp_secret();
        let b = generate_totp_secret();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32); // 20 bytes -> 32 BASE32 characters, no padding
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_uppercase() || (b'2'..=b'7').contains(&c))
        );
        let cred = CredentialRecord {
            credential_type: "otp".into(),
            secret_data: serde_json::json!({ "value": a }).to_string(),
            credential_data:
                r#"{"digits":6,"period":30,"algorithm":"HmacSHA1","secretEncoding":"BASE32"}"#
                    .into(),
            ..Default::default()
        };
        let code = generate_totp_code_for_credential(&cred, 1_700_000_000).unwrap();
        assert_eq!(
            verify_totp_code(&cred, &code, 0, 1_700_000_000).unwrap(),
            Some(1_700_000_000 / 30)
        );
        // RFC 6238 test vector: SHA1, secret "12345678901234567890", T=59 -> 94287082 (8 digits)
        let rfc = CredentialRecord {
            credential_type: "otp".into(),
            secret_data: "12345678901234567890".into(),
            credential_data: r#"{"digits":8,"period":30,"algorithm":"HmacSHA1"}"#.into(),
            ..Default::default()
        };
        assert_eq!(
            generate_totp_code_for_credential(&rfc, 59).unwrap(),
            "94287082"
        );
        let bad = CredentialRecord {
            secret_data: "not base32 !!".into(),
            credential_data: r#"{"secretEncoding":"BASE32"}"#.into(),
            ..Default::default()
        };
        assert!(generate_totp_code_for_credential(&bad, 59).is_err());
    }

    #[test]
    fn totp_hmac_sha256_matches_ri_and_recovery_codes_work_once() {
        let cred = CredentialRecord {
            credential_type: "otp".into(),
            secret_data: r#"{"value":"my-test-journey-seed-12345"}"#.into(),
            credential_data:
                r#"{"subType":"totp","digits":6,"counter":0,"period":30,"algorithm":"HmacSHA256"}"#
                    .into(),
            ..Default::default()
        };
        let now = 1_700_000_000u64;
        let code =
            generate_totp_code("my-test-journey-seed-12345", 6, 30, "HmacSHA256", now).unwrap();
        assert_eq!(code.len(), 6);
        let step = verify_totp_code(&cred, &code, 1, now + 15).unwrap();
        assert_eq!(step, Some(now / 30));
        assert_eq!(verify_totp_code(&cred, "000000", 1, now).unwrap(), None);

        let (plain, mut hashes) = generate_recovery_codes();
        assert_eq!(plain.len(), 10);
        let idx = verify_recovery_code(&plain[3], &hashes).expect("valid recovery code");
        assert_eq!(idx, 3);
        hashes.remove(idx);
        assert!(verify_recovery_code(&plain[3], &hashes).is_none());
    }
}
