//! The keys a space's `keyedHash` slots are hashed under (DM-121, ADR-N-054 2.6).
//!
//! Each sync resolves every space's `spec.personalData.keyRef` and keeps the values here, by
//! (project, space), for the data subject lookup, and hands them to the gateway as one Secret,
//! one key per space segment, which the gateway mounts at `JC_GATEWAY_PERSONAL_KEYS` and reads
//! at each write that hashes. Both sides hash the same way: HMAC-SHA-256 over the string's UTF-8
//! bytes, lowercase hex.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use jc_core::Secret;
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::sync::RwLock;

/// The Secret the gateway mounts, in its own namespace.
pub const SECRET_NAME: &str = "personal-data-keys";
/// The shortest key accepted, the gateway's own floor (`ingest::MIN_KEY_BYTES`).
pub const MIN_KEY_BYTES: usize = 32;

/// A resolved value as the gateway reads its file: trailing line breaks are not key, and a
/// value shorter than [`MIN_KEY_BYTES`] is no key at all.
pub fn usable(raw: &str) -> Option<Secret> {
    let key = raw.trim_end_matches(['\n', '\r']);
    (key.len() >= MIN_KEY_BYTES).then(|| Secret::new(key))
}

/// The keyed hash of `value` under `key`, as the gateway writes a `keyedHash` slot.
pub fn hash(key: &Secret, value: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.expose().as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(value.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The keys of the last sync, by (project, space name).
#[derive(Debug, Default)]
pub struct Keys {
    held: RwLock<BTreeMap<(String, String), Secret>>,
}

impl Keys {
    /// Replaces every key with what this sync resolved: a space whose key is gone has none.
    pub fn replace(&self, keys: BTreeMap<(String, String), Secret>) {
        *self.held.write().unwrap_or_else(|p| p.into_inner()) = keys;
    }

    /// The space's key, `None` while it has none.
    pub fn of(&self, project: &str, space: &str) -> Option<Secret> {
        self.held
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&(project.to_owned(), space.to_owned()))
            .cloned()
    }
}

/// The gateway's Secret: one key per space segment, the file name the gateway reads.
///
/// `data` rather than `stringData`: a server-side apply owns each `data` key, so a key a space
/// dropped leaves the Secret on the next apply, which a `stringData` write would not remove.
pub fn secret(namespace: &str, keys: &BTreeMap<String, Secret>) -> Value {
    let data: BTreeMap<&str, String> = keys
        .iter()
        .map(|(segment, key)| {
            let encoded = base64::engine::general_purpose::STANDARD.encode(key.expose());
            (segment.as_str(), encoded)
        })
        .collect();
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": SECRET_NAME,
            "namespace": namespace,
            "labels": { "app.kubernetes.io/managed-by": "joinedcontext-portal" },
        },
        "type": "Opaque",
        "data": data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    /// DM-121: the Portal finds what the gateway stored. The expected value is the gateway's own
    /// (`context-gateway` ingest), recomputed with `openssl dgst -sha256 -hmac`.
    #[test]
    fn the_hash_is_the_gateways_hmac_sha256_in_lowercase_hex() {
        let key = usable(KEY).expect("32 bytes");
        assert_eq!(
            hash(&key, "r-42"),
            "606a62f1ce588d825f88912faeb84707ca3a1f7acdecffad13b0bd1d7ac56396"
        );
    }

    #[test]
    fn a_short_key_is_no_key_and_line_breaks_are_not_key() {
        assert!(usable(&KEY[..31]).is_none());
        assert!(usable(&format!("{}\n", &KEY[..31])).is_none());
        let trimmed = usable(&format!("{KEY}\r\n")).expect("a file's line break");
        assert_eq!(trimmed.expose(), KEY);
    }

    #[test]
    fn the_secret_carries_one_key_per_segment_and_nothing_else() {
        let keys = BTreeMap::from([("rides-bikes".to_owned(), Secret::new(KEY))]);
        let secret = secret("jc-context-gateway", &keys);
        assert_eq!(secret["metadata"]["name"], SECRET_NAME);
        assert_eq!(secret["metadata"]["namespace"], "jc-context-gateway");
        assert_eq!(secret["type"], "Opaque");
        let data = secret["data"].as_object().expect("data");
        assert_eq!(data.len(), 1);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data["rides-bikes"].as_str().expect("base64"))
            .expect("decodes");
        assert_eq!(decoded, KEY.as_bytes());
        assert!(secret.get("stringData").is_none());
    }

    #[test]
    fn a_sync_replaces_every_key() {
        let keys = Keys::default();
        keys.replace(BTreeMap::from([(
            ("p".to_owned(), "s".to_owned()),
            Secret::new(KEY),
        )]));
        assert_eq!(
            keys.of("p", "s").map(|k| k.expose().to_owned()).as_deref(),
            Some(KEY)
        );
        keys.replace(BTreeMap::new());
        assert!(keys.of("p", "s").is_none());
    }
}
