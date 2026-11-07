use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use ekiben_core::{CoreError, SecretString};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NatsAuthConfig {
    pub nkey_file: Option<String>,
    pub user_credentials_file: Option<String>,
    pub user_jwt: Option<SecretString>,
    pub user_nkey_seed: Option<SecretString>,
}

#[derive(Clone, Debug)]
pub struct NatsConnectionDetails {
    pub urls: Vec<String>,
    pub auth: NatsAuthConfig,
    pub tls_enabled: bool,
}

impl NatsConnectionDetails {
    pub fn from_json(conf: &JsonValue) -> Result<Self, CoreError> {
        let obj = conf
            .as_object()
            .ok_or_else(|| CoreError::config("nats config must be an object"))?;

        let mut urls = Vec::new();
        if let Some(arr) = obj.get("urls").and_then(JsonValue::as_array) {
            for item in arr.iter().filter_map(JsonValue::as_str) {
                for part in item.split(',') {
                    let trimmed = part.trim();
                    if !trimmed.is_empty() {
                        urls.push(trimmed.to_string());
                    }
                }
            }
        } else if let Some(s) = obj.get("urls").and_then(JsonValue::as_str) {
            for part in s.split(',') {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    urls.push(trimmed.to_string());
                }
            }
        }
        if urls.is_empty() {
            return Err(CoreError::config(
                "nats requires at least one URL in 'urls'",
            ));
        }

        let mut auth = NatsAuthConfig::default();
        if let Some(auth_obj) = obj.get("auth").and_then(JsonValue::as_object) {
            auth.nkey_file = auth_obj
                .get("nkey_file")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from);
            auth.user_credentials_file = auth_obj
                .get("user_credentials_file")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from);
            let jwt = auth_obj
                .get("user_jwt")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty());
            let seed = auth_obj
                .get("user_nkey_seed")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty());
            match (jwt, seed) {
                (Some(_), None) => {
                    return Err(CoreError::config(
                        "missing auth.user_nkey_seed config field",
                    ))
                }
                (None, Some(_)) => {
                    return Err(CoreError::config("missing auth.user_jwt config field"))
                }
                (Some(j), Some(s)) => {
                    // Validate NKey seed syntax early
                    nkeys::KeyPair::from_seed(s).map_err(|e| {
                        CoreError::config(format!("invalid NATS auth.user_nkey_seed: {e}"))
                    })?;
                    auth.user_jwt = Some(SecretString::new(j));
                    auth.user_nkey_seed = Some(SecretString::new(s));
                }
                (None, None) => {}
            }
        }

        let tls_enabled = obj
            .get("tls")
            .and_then(|t| t.get("enabled"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        Ok(Self {
            urls,
            auth,
            tls_enabled,
        })
    }

    pub fn is_in_memory(&self) -> bool {
        self.urls
            .first()
            .is_some_and(|u| u.starts_with("nats://memory") || u.starts_with("nats://inproc"))
    }

    pub fn cluster_name(&self) -> String {
        self.urls
            .first()
            .cloned()
            .unwrap_or_else(|| "nats://memory".to_string())
    }

    pub async fn connect_live(&self, label: &str) -> Result<async_nats::Client, CoreError> {
        let mut opts = async_nats::ConnectOptions::new().name(label);
        if let (Some(jwt), Some(seed)) = (&self.auth.user_jwt, &self.auth.user_nkey_seed) {
            let jwt_str = jwt.expose().to_string();
            let kp = Arc::new(
                nkeys::KeyPair::from_seed(seed.expose())
                    .map_err(|e| CoreError::config(format!("nkey error: {e}")))?,
            );
            opts = opts.jwt(jwt_str, move |nonce| {
                let kp = kp.clone();
                async move { kp.sign(&nonce).map_err(async_nats::AuthError::new) }
            });
        } else if let Some(ref creds_file) = self.auth.user_credentials_file {
            opts = opts
                .credentials_file(creds_file)
                .await
                .map_err(|e| CoreError::config(format!("credentials_file error: {e}")))?;
        } else if let Some(ref nkey_file) = self.auth.nkey_file {
            let seed = std::fs::read_to_string(nkey_file)
                .map_err(|e| CoreError::config(format!("failed to read nkey_file: {e}")))?;
            opts = opts.nkey(seed.trim().to_string());
        }

        let server_addrs: Vec<&str> = self.urls.iter().map(String::as_str).collect();
        async_nats::connect_with_options(server_addrs.as_slice(), opts)
            .await
            .map_err(|e| CoreError::other(format!("failed to connect to NATS: {e}")))
    }
}

use std::sync::Arc;

/// Checks if a NATS subject pattern (supporting `*` and `>`) matches a concrete message subject.
pub fn subject_matches(pattern: &str, subject: &str) -> bool {
    if pattern == ">" || pattern == subject {
        return true;
    }
    let pat_tokens: Vec<&str> = pattern.split('.').collect();
    let sub_tokens: Vec<&str> = subject.split('.').collect();
    for (i, &pt) in pat_tokens.iter().enumerate() {
        if pt == ">" {
            return i <= sub_tokens.len();
        }
        let Some(&st) = sub_tokens.get(i) else {
            return false;
        };
        if pt != "*" && pt != st {
            return false;
        }
    }
    pat_tokens.len() == sub_tokens.len()
}
