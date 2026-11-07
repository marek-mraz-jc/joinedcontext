//! Secret-looking values in what a caller sends or is answered (T-5971, MF-24).
//!
//! A credential belongs in the Portal's secret form, which encrypts it into the repository, and a
//! manifest names it by `secretRef`. One pasted into a chat travels through the model, the tool
//! call, the plan and the audit row, so the operations door refuses any input holding one, by its
//! path and never by its value, and every answer to a connector or an agent run is masked.
//!
//! Three shapes count: a value under a name that says it is a credential (`password`, `apiKey`,
//! `X-Api-Key`, `authToken`), a string shaped like one wherever it sits (the gitleaks-style
//! prefixes and forms of [`credential_shaped`]), and a URL carrying one in its query
//! (`?api_key=…`). A whole `${VAR}` names a secret the runner injects and holds none.

use serde_json::Value;

use crate::api::mutate::{is_variable_reference, SECRET_KEYS};
use crate::pipeline_outcomes::credential_shaped;

/// The endings of a member name that holds a credential, lower case without `-` and `_`.
const SECRET_ENDINGS: &[&str] = &[
    "apikey",
    "apitoken",
    "accesstoken",
    "authtoken",
    "token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "privatekey",
    "accesskey",
];

/// Query parameters that carry a credential, lower case without `-` and `_`.
const SECRET_PARAMS: &[&str] = &[
    "apikey",
    "key",
    "token",
    "accesstoken",
    "apitoken",
    "authtoken",
    "secret",
    "clientsecret",
    "password",
    "sig",
    "signature",
];

fn folded(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether a member called `name` holds a credential.
pub fn secret_name(name: &str) -> bool {
    let name = folded(name);
    SECRET_KEYS.iter().any(|k| folded(k) == name)
        || SECRET_ENDINGS.iter().any(|ending| name.ends_with(ending))
}

/// Whether a text reads like a credential: [`credential_shaped`] less a certificate or public
/// key (which an MQTT broker's manifest carries in the open), or a URL with one in its query.
pub fn secret_text(text: &str) -> bool {
    let public_pem = text.contains("-----BEGIN CERTIFICATE")
        || text.contains("-----BEGIN PUBLIC KEY")
        || text.contains("-----BEGIN X509 CRL");
    (credential_shaped(text) && !public_pem) || secret_in_query(text)
}

/// Whether a query parameter is named like a credential (`apiKey`, `sig`, `client_secret`, …):
/// the one list every address check reads (JC1-54).
pub fn secret_param(name: &str) -> bool {
    let name = folded(name);
    SECRET_PARAMS.contains(&name.as_str())
        || ["auth", "credential"].contains(&name.as_str())
        || SECRET_KEYS.iter().any(|key| folded(key) == name)
}

fn secret_in_query(text: &str) -> bool {
    let Ok(url) = url::Url::parse(text.trim()) else {
        return false;
    };
    url.query_pairs().any(|(name, value)| {
        secret_param(&name) && !value.is_empty() && !is_variable_reference(&value)
    })
}

/// The JSON pointer of every value in `input` that holds a credential, in document order.
pub fn find(input: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(input, "", false, &mut found);
    found
}

fn walk(value: &Value, at: &str, named: bool, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let pointer = format!("{at}/{}", key.replace('~', "~0").replace('/', "~1"));
                walk(child, &pointer, secret_name(key), found);
            }
        }
        Value::Array(items) => {
            for (n, item) in items.iter().enumerate() {
                walk(item, &format!("{at}/{n}"), named, found);
            }
        }
        Value::String(text) if is_variable_reference(text) || text.is_empty() => {}
        Value::String(text) if named || secret_text(text) => found.push(at.to_owned()),
        Value::Number(_) | Value::Bool(_) if named => found.push(at.to_owned()),
        _ => {}
    }
}

/// An answer as a model may read it: a value under a credential's name, a credential-shaped
/// string and each credential-shaped word of free text (a log line quoting a header) masked.
pub fn masked(answer: &Value) -> Value {
    fn words(value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(crate::pipeline_outcomes::mask_text(&text)),
            Value::Array(items) => Value::Array(items.into_iter().map(words).collect()),
            Value::Object(map) => {
                Value::Object(map.into_iter().map(|(k, v)| (k, words(v))).collect())
            }
            other => other,
        }
    }
    words(crate::pipeline_outcomes::mask(answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// JC1-54: one parameter list for the operations door, the inference address and the
    /// drafted source address, each former list's names included.
    #[test]
    fn a_credential_parameter_is_one_list_for_every_address_check() {
        for name in [
            "key",
            "apiKey",
            "api_key",
            "sig",
            "signature",
            "auth",
            "credential",
            "token",
            "client_secret",
            "clientSecret",
            "password",
            "access-token",
        ] {
            assert!(secret_param(name), "{name}");
        }
        for name in ["format", "limit", "type", "keyword", "q"] {
            assert!(!secret_param(name), "{name}");
        }
        assert!(secret_text("https://feed.example.com/v1?auth=0123456789"));
        assert!(!secret_text(
            "https://feed.example.com/v1?auth=${FEED_AUTH}"
        ));
    }

    #[test]
    fn a_pasted_key_is_found_by_its_name_its_shape_or_its_url() {
        let input = json!({
            "changes": [{ "manifest": { "spec": { "http": {
                "url": "https://api.example.com/v1/counters?format=csv&api_key=abc123def456",
                "headers": { "X-Api-Key": "abc123def456", "Accept": "text/csv" },
                "basic": "Bearer abc.def.ghi",
                "pin": 123456,
                "password": "${FEED_PASSWORD}",
                "secretRef": { "name": "feed", "key": "password" },
                "tokenUrl": "https://login.example.com/token",
                "ca": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
                "pem": "-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----",
                "note": "ghp_0123456789abcdef0123"
            } } } }]
        });
        let at = "/changes/0/manifest/spec/http";
        assert_eq!(
            find(&input),
            vec![
                format!("{at}/basic"),
                format!("{at}/headers/X-Api-Key"),
                format!("{at}/note"),
                format!("{at}/pem"),
                format!("{at}/url"),
            ]
        );
        let mut named = input.clone();
        named["changes"][0]["manifest"]["spec"]["http"]["password"] = json!(123456);
        assert!(find(&named).contains(&format!("{at}/password")));
    }

    #[test]
    fn plain_arguments_pass() {
        let input = json!({
            "q": "count>10", "limit": 100, "maxTokens": 4000, "name": "token",
            "url": "https://api.example.com/v1?key=${API_KEY}&page=2",
            "urls": ["nats://demo:${DEMO_FEED_PASSWORD}@demo-nats.dev.svc.cluster.local:4222"],
            "patch": { "spec": { "description": "rotate the token weekly" } }
        });
        assert_eq!(find(&input), Vec::<String>::new());
        assert!(!secret_name("tokenUrl"));
        assert!(secret_name("x-api-key"));
    }
}
