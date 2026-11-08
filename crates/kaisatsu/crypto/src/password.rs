#![forbid(unsafe_code)]
//! Password hashing, policy enforcement, and Keycloak credential migration without resets (W1, W7):
//! verifies PHC strings plus Keycloak `secretData`/`credentialData` for `argon2` (argon2id v1.3),
//! `pbkdf2-sha512`, `pbkdf2-sha256`, and `pbkdf2` (SHA-1), and rehashes legacy hashes to current
//! argon2id parameters on successful login.

use crate::constant_time_eq;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::{Engine, general_purpose::STANDARD as B64_STD};
use hmac::Hmac;
use kaisatsu_core::{CredentialRecord, KaisatsuError, Result};
use pbkdf2::pbkdf2;
use serde::Deserialize;
use sha1::Sha1;
use sha2::{Sha256, Sha512};

/// Minimum Argon2id parameters matching Keycloak 26's `Argon2PasswordHashProviderFactory`:
/// argon2id v1.3, m = 7168 KiB, t = 5, p = 1, 32-byte output (W7).
pub const DEFAULT_MEMORY_KIB: u32 = 7168;
pub const DEFAULT_ITERATIONS: u32 = 5;
pub const DEFAULT_PARALLELISM: u32 = 1;
pub const DEFAULT_OUTPUT_LEN: usize = 32;

/// Outcome of verifying a password against a stored credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordVerifyOutcome {
    pub valid: bool,
    /// Present when the credential verified against a legacy Keycloak format and should be
    /// replaced with this freshly hashed PHC string (W1, W7).
    pub rehashed_phc: Option<String>,
}

/// Hashes a password with kaisatsu's default Argon2id parameters into a PHC string.
pub fn hash_password(password: &str) -> Result<String> {
    let mut salt_bytes = [0u8; 16];
    aws_lc_rs::rand::fill(&mut salt_bytes)
        .map_err(|_| KaisatsuError::Crypto("CSPRNG failure".into()))?;
    let params = Params::new(
        DEFAULT_MEMORY_KIB,
        DEFAULT_ITERATIONS,
        DEFAULT_PARALLELISM,
        Some(DEFAULT_OUTPUT_LEN),
    )
    .map_err(|e| KaisatsuError::Crypto(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let hash: PasswordHash = argon2
        .hash_password_with_salt(password.as_bytes(), &salt_bytes)
        .map_err(|e| KaisatsuError::Crypto(e.to_string()))?;
    Ok(hash.to_string())
}

/// Performs a dummy Argon2id hash when a username does not exist so unknown-user and wrong-password
/// timings stay in the same class (W3, W7, W12).
pub fn dummy_verify_password(password: &str) {
    let _ = hash_password(password);
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KcSecretData {
    value: String,
    #[serde(default)]
    salt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KcCredentialData {
    #[serde(default)]
    hash_iterations: Option<i32>,
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default)]
    additional_parameters: serde_json::Map<String, serde_json::Value>,
}

fn param_u32(map: &serde_json::Map<String, serde_json::Value>, key: &str, default: u32) -> u32 {
    let Some(val) = map.get(key) else {
        return default;
    };
    if let Some(n) = val.as_u64() {
        return n as u32;
    }
    if let Some(s) = val.as_str() {
        return s.parse().unwrap_or(default);
    }
    if let Some(arr) = val.as_array()
        && let Some(first) = arr.first()
    {
        if let Some(s) = first.as_str() {
            return s.parse().unwrap_or(default);
        }
        if let Some(n) = first.as_u64() {
            return n as u32;
        }
    }
    default
}

/// Verifies a password against a `CredentialRecord` (either PHC string in `secret_data` or
/// Keycloak's JSON `secretData` + `credentialData`).
pub fn verify_and_maybe_rehash(
    password: &str,
    cred: &CredentialRecord,
) -> Result<PasswordVerifyOutcome> {
    if cred.secret_data.starts_with("$argon2") {
        let parsed = PasswordHash::new(&cred.secret_data)
            .map_err(|_| KaisatsuError::Crypto("invalid PHC password hash".into()))?;
        let valid = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        return Ok(PasswordVerifyOutcome {
            valid,
            rehashed_phc: None,
        });
    }
    let valid = verify_keycloak_credential(password, &cred.secret_data, &cred.credential_data)?;
    let rehashed_phc = if valid {
        Some(hash_password(password)?)
    } else {
        None
    };
    Ok(PasswordVerifyOutcome {
        valid,
        rehashed_phc,
    })
}

/// Verifies Keycloak's `PasswordSecretData` and `PasswordCredentialData` JSON (research-keycloak §3.2, §5.5).
pub fn verify_keycloak_credential(
    password: &str,
    secret_data_json: &str,
    credential_data_json: &str,
) -> Result<bool> {
    let secret: KcSecretData = serde_json::from_str(secret_data_json)
        .map_err(|_| KaisatsuError::Crypto("invalid Keycloak secretData JSON".into()))?;
    let meta: KcCredentialData = if credential_data_json.trim().is_empty() {
        KcCredentialData {
            hash_iterations: None,
            algorithm: Some("argon2".into()),
            additional_parameters: serde_json::Map::new(),
        }
    } else {
        serde_json::from_str(credential_data_json)
            .map_err(|_| KaisatsuError::Crypto("invalid Keycloak credentialData JSON".into()))?
    };

    let expected_hash = B64_STD
        .decode(secret.value.trim())
        .map_err(|_| KaisatsuError::Crypto("invalid base64 hash in secretData".into()))?;
    let salt_b64 = secret.salt.as_deref().unwrap_or_default();
    let salt_bytes = B64_STD
        .decode(salt_b64.trim())
        .map_err(|_| KaisatsuError::Crypto("invalid base64 salt in secretData".into()))?;

    let alg = meta
        .algorithm
        .as_deref()
        .unwrap_or("argon2")
        .to_ascii_lowercase();

    match alg.as_str() {
        "argon2" | "argon2id" | "argon2i" | "argon2d" => {
            let memory = param_u32(&meta.additional_parameters, "memory", DEFAULT_MEMORY_KIB);
            let iterations = meta
                .hash_iterations
                .filter(|&i| i > 0)
                .map(|i| i as u32)
                .unwrap_or_else(|| {
                    param_u32(
                        &meta.additional_parameters,
                        "iterations",
                        DEFAULT_ITERATIONS,
                    )
                });
            let parallelism = param_u32(
                &meta.additional_parameters,
                "parallelism",
                DEFAULT_PARALLELISM,
            );
            let hash_len = expected_hash.len();
            let params = Params::new(memory, iterations, parallelism, Some(hash_len))
                .map_err(|e| KaisatsuError::Crypto(e.to_string()))?;
            let argon_alg = match meta
                .additional_parameters
                .get("type")
                .and_then(|v| v.as_str().or_else(|| v.as_array()?.first()?.as_str()))
                .unwrap_or("id")
            {
                "i" => Algorithm::Argon2i,
                "d" => Algorithm::Argon2d,
                _ => Algorithm::Argon2id,
            };
            let version = match meta
                .additional_parameters
                .get("version")
                .and_then(|v| v.as_str().or_else(|| v.as_array()?.first()?.as_str()))
                .unwrap_or("1.3")
            {
                "1.0" | "16" => Version::V0x10,
                _ => Version::V0x13,
            };
            let argon2 = Argon2::new(argon_alg, version, params);
            let mut computed = vec![0u8; hash_len];
            argon2
                .hash_password_into(password.as_bytes(), &salt_bytes, &mut computed)
                .map_err(|e| KaisatsuError::Crypto(e.to_string()))?;
            Ok(constant_time_eq(&computed, &expected_hash))
        }
        "pbkdf2-sha512" => {
            let rounds = meta.hash_iterations.unwrap_or(210_000).max(1) as u32;
            let mut computed = vec![0u8; expected_hash.len()];
            let _ = pbkdf2::<Hmac<Sha512>>(password.as_bytes(), &salt_bytes, rounds, &mut computed);
            Ok(constant_time_eq(&computed, &expected_hash))
        }
        "pbkdf2-sha256" => {
            let rounds = meta.hash_iterations.unwrap_or(27_500).max(1) as u32;
            let mut computed = vec![0u8; expected_hash.len()];
            let _ = pbkdf2::<Hmac<Sha256>>(password.as_bytes(), &salt_bytes, rounds, &mut computed);
            Ok(constant_time_eq(&computed, &expected_hash))
        }
        "pbkdf2" | "pbkdf2-sha1" => {
            let rounds = meta.hash_iterations.unwrap_or(1_300_000).max(1) as u32;
            let mut computed = vec![0u8; expected_hash.len()];
            let _ = pbkdf2::<Hmac<Sha1>>(password.as_bytes(), &salt_bytes, rounds, &mut computed);
            Ok(constant_time_eq(&computed, &expected_hash))
        }
        other => Err(KaisatsuError::Crypto(format!(
            "unsupported Keycloak password hash algorithm: {other}"
        ))),
    }
}

/// Enforces RI's password policy (`length(N)`, `upperCase(N)`, `lowerCase(N)`, `digits(N)`,
/// `specialChars(N)`, `notUsername`, `notEmail`, `passwordHistory(N)`), W7.
pub fn check_password_policy(
    policy: &str,
    password: &str,
    username: &str,
    email: Option<&str>,
    history_records: &[CredentialRecord],
) -> Result<()> {
    for clause in policy
        .split(" and ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(arg) = clause
            .strip_prefix("length(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let min: usize = arg.parse().unwrap_or(12);
            if password.chars().count() < min {
                return Err(KaisatsuError::field_error(
                    "password",
                    format!("must be at least {min} characters"),
                ));
            }
        } else if let Some(arg) = clause
            .strip_prefix("upperCase(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let min: usize = arg.parse().unwrap_or(1);
            if password.chars().filter(|c| c.is_uppercase()).count() < min {
                return Err(KaisatsuError::field_error(
                    "password",
                    format!("must contain at least {min} uppercase character(s)"),
                ));
            }
        } else if let Some(arg) = clause
            .strip_prefix("lowerCase(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let min: usize = arg.parse().unwrap_or(1);
            if password.chars().filter(|c| c.is_lowercase()).count() < min {
                return Err(KaisatsuError::field_error(
                    "password",
                    format!("must contain at least {min} lowercase character(s)"),
                ));
            }
        } else if let Some(arg) = clause
            .strip_prefix("digits(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let min: usize = arg.parse().unwrap_or(1);
            if password.chars().filter(|c| c.is_ascii_digit()).count() < min {
                return Err(KaisatsuError::field_error(
                    "password",
                    format!("must contain at least {min} digit(s)"),
                ));
            }
        } else if let Some(arg) = clause
            .strip_prefix("specialChars(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let min: usize = arg.parse().unwrap_or(1);
            if password.chars().filter(|c| !c.is_alphanumeric()).count() < min {
                return Err(KaisatsuError::field_error(
                    "password",
                    format!("must contain at least {min} special character(s)"),
                ));
            }
        } else if clause.starts_with("notUsername") {
            if !username.is_empty()
                && password
                    .to_ascii_lowercase()
                    .contains(&username.to_ascii_lowercase())
            {
                return Err(KaisatsuError::field_error(
                    "password",
                    "must not contain the username",
                ));
            }
        } else if clause.starts_with("notEmail") {
            if let Some(em) = email.filter(|e| !e.is_empty())
                && password
                    .to_ascii_lowercase()
                    .contains(&em.to_ascii_lowercase())
            {
                return Err(KaisatsuError::field_error(
                    "password",
                    "must not contain the e-mail address",
                ));
            }
        } else if let Some(arg) = clause
            .strip_prefix("passwordHistory(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let count: usize = arg.parse().unwrap_or(5);
            for old in history_records.iter().take(count) {
                if let Ok(outcome) = verify_and_maybe_rehash(password, old)
                    && outcome.valid
                {
                    return Err(KaisatsuError::field_error(
                        "password",
                        format!("must not match any of the last {count} passwords"),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_keycloak_argon2_and_pbkdf2_variants_and_rehashes() {
        let salt = b"0123456789abcdef";
        let password = "CorrectHorse99!Battery";

        // 1. Keycloak Argon2id
        let params = Params::new(7168, 5, 1, Some(32)).unwrap();
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        argon
            .hash_password_into(password.as_bytes(), salt, &mut out)
            .unwrap();
        let secret_json = serde_json::json!({
            "value": B64_STD.encode(out),
            "salt": B64_STD.encode(salt),
        })
        .to_string();
        let cred_json = serde_json::json!({
            "hashIterations": 5,
            "algorithm": "argon2",
            "additionalParameters": {
                "memory": ["7168"],
                "parallelism": ["1"],
                "type": ["id"],
                "version": ["1.3"],
                "hashLength": ["32"]
            }
        })
        .to_string();

        let rec = CredentialRecord {
            secret_data: secret_json,
            credential_data: cred_json,
            ..Default::default()
        };
        let outcome = verify_and_maybe_rehash(password, &rec).expect("verify");
        assert!(outcome.valid);
        assert!(
            outcome
                .rehashed_phc
                .as_deref()
                .unwrap()
                .starts_with("$argon2id$")
        );

        // 2. Keycloak pbkdf2-sha256
        let mut pbkdf2_out = [0u8; 32];
        let _ = pbkdf2::<Hmac<Sha256>>(password.as_bytes(), salt, 1000, &mut pbkdf2_out);
        let pbkdf2_secret = serde_json::json!({
            "value": B64_STD.encode(pbkdf2_out),
            "salt": B64_STD.encode(salt),
        })
        .to_string();
        let pbkdf2_cred = serde_json::json!({
            "hashIterations": 1000,
            "algorithm": "pbkdf2-sha256"
        })
        .to_string();
        assert!(verify_keycloak_credential(password, &pbkdf2_secret, &pbkdf2_cred).unwrap());
        assert!(!verify_keycloak_credential("wrong", &pbkdf2_secret, &pbkdf2_cred).unwrap());
    }

    #[test]
    fn enforces_ri_password_policy_and_history() {
        let policy = "length(12) and upperCase(1) and lowerCase(1) and digits(1) and notUsername(undefined) and notEmail(undefined) and passwordHistory(5)";
        let old_hash = hash_password("OldValidPass123").unwrap();
        let history = vec![CredentialRecord {
            secret_data: old_hash,
            ..Default::default()
        }];

        assert!(
            check_password_policy(policy, "Short1a", "alice", Some("alice@hel.fi"), &history)
                .is_err()
        );
        assert!(
            check_password_policy(
                policy,
                "aliceValidPass123",
                "alice",
                Some("alice@hel.fi"),
                &history
            )
            .is_err()
        );
        assert!(
            check_password_policy(
                policy,
                "OldValidPass123",
                "alice",
                Some("alice@hel.fi"),
                &history
            )
            .is_err()
        );
        assert!(
            check_password_policy(
                policy,
                "NewValidPass456",
                "alice",
                Some("alice@hel.fi"),
                &history
            )
            .is_ok()
        );
    }
}
