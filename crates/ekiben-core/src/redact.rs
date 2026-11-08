use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use url::Url;

pub const REDACTED_PLACEHOLDER: &str = "REDACTED";

/// Wrapper for secret configuration strings that always formats as `"REDACTED"` in `Debug` and `Display`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"REDACTED\"")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED_PLACEHOLDER)
    }
}

/// Redacts userinfo password in URLs (`https://user:secret@host/path` -> `https://user:REDACTED@host/path`),
/// matching `net/url.URL.Redacted()` in Go (`internal/httpclient/logger.go`).
pub fn redact_url_str(raw: &str) -> String {
    if let Ok(mut parsed) = Url::parse(raw) {
        if parsed.password().is_some() {
            let _ = parsed.set_password(Some(REDACTED_PLACEHOLDER));
            return parsed.to_string();
        }
    }
    // Also handle URLs containing `${...}` interpolations after `@`
    if let Some(scheme_end) = raw.find("://") {
        let after_scheme = &raw[scheme_end + 3..];
        let authority_end = after_scheme.find('/').unwrap_or(after_scheme.len());
        let authority = &after_scheme[..authority_end];
        if let Some(at_idx) = authority.find('@') {
            let userinfo = &authority[..at_idx];
            if let Some(colon_idx) = userinfo.find(':') {
                let user = &userinfo[..colon_idx];
                let host_and_rest = &after_scheme[at_idx..];
                return format!(
                    "{}{}:{REDACTED_PLACEHOLDER}{}",
                    &raw[..scheme_end + 3],
                    user,
                    host_and_rest
                );
            }
        }
    }
    raw.to_string()
}

const SECRET_FIELD_KEYS: &[&str] = &[
    "password",
    "password_hash",
    "client_secret",
    "access_token",
    "access_token_secret",
    "consumer_secret",
    "secret",
    "bearer",
    "token",
    "bot_token",
    "api_key",
    "api_secret",
    "auth_secret",
    "user_jwt",
    "user_nkey_seed",
    "private_key_pass",
    "account_key",
    "storage_access_key",
    "storage_connection_string",
    "storage_sas_token",
];

/// Recursively scrubs secrets, header values, and URL userinfo from a stream configuration `serde_json::Value`
/// before returning it in `GET /streams/{id}` or logging it.
pub fn redact_config_value(val: &JsonValue) -> JsonValue {
    redact_inner(val, None)
}

fn redact_inner(val: &JsonValue, parent_key: Option<&str>) -> JsonValue {
    match val {
        JsonValue::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            let parent_is_headers = parent_key.is_some_and(|k| k.eq_ignore_ascii_case("headers"));
            for (k, v) in map {
                let k_lower = k.to_ascii_lowercase();
                if parent_is_headers && is_sensitive_header_name(&k_lower) {
                    out.insert(
                        k.clone(),
                        JsonValue::String(REDACTED_PLACEHOLDER.to_string()),
                    );
                } else if SECRET_FIELD_KEYS.contains(&k_lower.as_str()) {
                    if let JsonValue::String(s) = v {
                        if s.is_empty() {
                            out.insert(k.clone(), JsonValue::String(String::new()));
                        } else {
                            out.insert(
                                k.clone(),
                                JsonValue::String(REDACTED_PLACEHOLDER.to_string()),
                            );
                        }
                    } else {
                        out.insert(k.clone(), redact_inner(v, Some(k)));
                    }
                } else {
                    out.insert(k.clone(), redact_inner(v, Some(k)));
                }
            }
            JsonValue::Object(out)
        }
        JsonValue::Array(arr) => JsonValue::Array(
            arr.iter()
                .map(|item| redact_inner(item, parent_key))
                .collect(),
        ),
        JsonValue::String(s) => {
            if parent_key.is_some_and(|k| {
                k.eq_ignore_ascii_case("url")
                    || k.eq_ignore_ascii_case("urls")
                    || k.eq_ignore_ascii_case("token_url")
                    || k.eq_ignore_ascii_case("proxy_url")
                    || k.eq_ignore_ascii_case("dsn")
            }) || s.contains("://")
            {
                JsonValue::String(redact_url_str(s))
            } else {
                JsonValue::String(s.clone())
            }
        }
        other => other.clone(),
    }
}

pub fn is_sensitive_header_name(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-access-token"
            | "x-auth-token"
    ) || name_lower.contains("secret")
        || name_lower.contains("token")
        || name_lower.contains("password")
        || name_lower.contains("key")
}
