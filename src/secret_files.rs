//! Writing the repository's encrypted secrets files (CC-06, T-5971).
//!
//! A person enters a credential in the Portal's secret form, never in a chat or a manifest. The
//! Portal encrypts it here with SOPS + age, as `jcctl::secrets::sops` reads it back: one file
//! `projects/{project}/secrets/{name}.enc.yaml` per secret name, its keys encrypted with
//! AES-256-GCM under a fresh data key, the data key wrapped for every age recipient. The Portal's
//! own recipient (the public half of the identity it resolves `secretRef`s with) is always one of
//! them, and a replaced file keeps the recipients it had, so a person who could open it still can.
//!
//! Names and keys stay readable in the file, as SOPS writes them: [`names`] lists them without a
//! key, so a plan can say which reference has no value yet.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::str::FromStr;

use aes_gcm::aead::{consts::U32, Aead, Payload};
use aes_gcm::KeyInit;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use sha2::{Digest, Sha512};

/// AES-256-GCM with the 32-byte nonce SOPS writes.
type SopsCipher = aes_gcm::AesGcm<aes_gcm::aes::Aes256, U32>;

/// The secrets folder of a project, under its render path (MF-06).
pub fn path_of(project: &str, name: &str) -> String {
    format!("projects/{project}/secrets/{name}.enc.yaml")
}

/// The project and secret name of a secrets file, the inverse of [`path_of`]; `None` for any
/// other path.
pub fn parse_path(path: &str) -> Option<(&str, &str)> {
    let (project, file) = path.strip_prefix("projects/")?.split_once("/secrets/")?;
    let name = file.strip_suffix(".enc.yaml")?;
    (!project.is_empty() && !project.contains('/') && valid_name(name)).then_some((project, name))
}

/// Whether `name` may be a secret's name or key: a DNS label's letters, as a `secretRef` names it.
pub fn valid_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
}

/// The names and keys an encrypted file declares, read without decrypting: a name holding one
/// value has no keys. `None` when the text is no SOPS file.
pub fn names(text: &str) -> Option<BTreeMap<String, Vec<String>>> {
    let document: serde_norway::Mapping = serde_norway::from_str(text).ok()?;
    document.get("sops")?;
    Some(
        document
            .iter()
            .filter_map(|(name, node)| {
                let name = name.as_str()?;
                (name != "sops").then(|| {
                    let keys = node
                        .as_mapping()
                        .map(|keys| {
                            keys.keys()
                                .filter_map(|k| k.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    (name.to_owned(), keys)
                })
            })
            .collect(),
    )
}

/// Whether `text` is a SOPS file whose every value is encrypted: a `sops` block, and each leaf
/// outside it an `ENC[…]` literal. A file carrying a `sops` key beside a plain value (a bundle
/// import, a direct push) is not, so it never lands as a sealed secret (CC-06, T-6359).
pub fn sealed(text: &str) -> bool {
    fn encrypted(node: &serde_norway::Value) -> bool {
        match node {
            serde_norway::Value::Mapping(map) => map.values().all(encrypted),
            serde_norway::Value::Sequence(items) => items.iter().all(encrypted),
            serde_norway::Value::String(value) => {
                value.starts_with("ENC[AES256_GCM,") && value.ends_with(']')
            }
            _ => false,
        }
    }
    let Ok(document) = serde_norway::from_str::<serde_norway::Mapping>(text) else {
        return false;
    };
    document
        .get("sops")
        .is_some_and(serde_norway::Value::is_mapping)
        && document
            .iter()
            .filter(|(name, _)| name.as_str() != Some("sops"))
            .all(|(_, node)| encrypted(node))
}

/// The age recipients a SOPS file names, as their `age1…` strings.
pub fn recipients_of(text: &str) -> Vec<String> {
    let document: serde_norway::Value = serde_norway::from_str(text).unwrap_or_default();
    document["sops"]["age"]
        .as_sequence()
        .map(|stanzas| {
            stanzas
                .iter()
                .filter_map(|s| s["recipient"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The public recipient of the age identity file the Portal resolves references with.
pub fn own_recipient(identity_file: &std::path::Path) -> Result<String, String> {
    let identities = jcctl::secrets::identities_from_file(identity_file)
        .map_err(|_| "the Portal's age identity file cannot be read".to_owned())?;
    identities
        .first()
        .map(|identity| identity.to_public().to_string())
        .ok_or_else(|| "the Portal's age identity file holds no key".to_owned())
}

/// One SOPS file holding the secret `name` with these `keys`, encrypted for `recipients`.
/// `now` is the file's `lastmodified`, RFC 3339. The values are never part of an error.
pub fn encrypt(
    name: &str,
    keys: &BTreeMap<String, zeroize::Zeroizing<String>>,
    recipients: &[String],
    now: &str,
) -> Result<String, String> {
    let mut data_key = zeroize::Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(data_key.as_mut());

    let mut mac = Sha512::new();
    let mut leaves = serde_norway::Mapping::new();
    for (key, value) in keys {
        mac.update(value.as_bytes());
        leaves.insert(
            key.as_str().into(),
            seal(value.as_bytes(), &format!("{name}:{key}:"), &data_key[..])?.into(),
        );
    }
    let mac_hex: String = mac.finalize().iter().map(|b| format!("{b:02X}")).collect();

    let mut age = Vec::new();
    for recipient in recipients {
        let parsed = age::x25519::Recipient::from_str(recipient)
            .map_err(|_| format!("'{recipient}' is no age recipient"))?;
        age.push(serde_json::json!({
            "recipient": recipient,
            "enc": wrap(&data_key[..], &parsed)?,
        }));
    }
    if age.is_empty() {
        return Err("a secret needs at least one age recipient".into());
    }

    let mut document = serde_norway::Mapping::new();
    document.insert(name.into(), serde_norway::Value::Mapping(leaves));
    let metadata = serde_json::json!({
        "age": age,
        "lastmodified": now,
        "mac": seal(mac_hex.as_bytes(), now, &data_key[..])?,
        "unencrypted_suffix": "_unencrypted",
        "version": "3.9.0",
    });
    document.insert(
        "sops".into(),
        serde_norway::to_value(metadata).map_err(|e| e.to_string())?,
    );
    serde_norway::to_string(&document).map_err(|e| e.to_string())
}

/// One `ENC[AES256_GCM,…]` literal, authenticated with `aad` as SOPS does.
fn seal(plaintext: &[u8], aad: &str, data_key: &[u8]) -> Result<String, String> {
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let cipher = SopsCipher::new_from_slice(data_key)
        .map_err(|_| "the data key is not 32 bytes".to_owned())?;
    let mut sealed = cipher
        .encrypt(
            &aes_gcm::Nonce::<U32>::from(nonce),
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "the value could not be encrypted".to_owned())?;
    let tag = sealed.split_off(sealed.len() - 16);
    Ok(format!(
        "ENC[AES256_GCM,data:{},iv:{},tag:{},type:str]",
        BASE64.encode(&sealed),
        BASE64.encode(nonce),
        BASE64.encode(tag)
    ))
}

/// The data key wrapped for one recipient, ASCII-armored.
fn wrap(data_key: &[u8], recipient: &age::x25519::Recipient) -> Result<String, String> {
    let failed = |_| "the data key could not be wrapped".to_owned();
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
            .map_err(|_| "the data key could not be wrapped".to_owned())?;
    let mut out = Vec::new();
    let armor = age::armor::ArmoredWriter::wrap_output(&mut out, age::armor::Format::AsciiArmor)
        .map_err(failed)?;
    let mut writer = encryptor.wrap_output(armor).map_err(failed)?;
    writer.write_all(data_key).map_err(failed)?;
    writer
        .finish()
        .and_then(|armor| armor.finish())
        .map_err(failed)?;
    String::from_utf8(out).map_err(|_| "the armor is not text".to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_is_sealed_only_when_every_value_outside_sops_is_encrypted() {
        let enc = "ENC[AES256_GCM,data:x,iv:y,tag:z,type:str]";
        let sops = "sops:\n  version: 3.9.0\n";
        for (text, want) in [
            (format!("t:\n  token: {enc}\n{sops}"), true),
            (format!("t: {enc}\nu:\n  - {enc}\n{sops}"), true),
            (format!("t:\n  token: hunter2\n{sops}"), false),
            (format!("t:\n  token: {enc}\n  other: plain\n{sops}"), false),
            (format!("t:\n  port: 5432\n{sops}"), false),
            ("x:\n  token: plain\nsops: {}\n".to_owned(), false),
            (format!("t:\n  token: {enc}\nsops: 3\n"), false),
            (format!("t:\n  token: {enc}\n"), false),
            (
                "t:\n  token: ENC[plain]\nsops:\n  version: 3\n".to_owned(),
                false,
            ),
            ("not: [yaml".to_owned(), false),
        ] {
            assert_eq!(super::sealed(&text), want, "{text}");
        }
    }

    #[test]
    fn a_secrets_path_names_its_project_and_secret_and_nothing_else_does() {
        assert_eq!(
            parse_path(&path_of("hel-mobility", "github-token")),
            Some(("hel-mobility", "github-token"))
        );
        for other in [
            "projects/p/secrets/x.yaml",
            "projects/p/secrets/.enc.yaml",
            "projects/p/secrets/a/b.enc.yaml",
            "projects/p/spaces/air/secrets/x.enc.yaml",
            "projects//secrets/x.enc.yaml",
            "users/secrets/x.enc.yaml",
        ] {
            assert_eq!(parse_path(other), None, "{other}");
        }
    }

    use super::*;
    use jc_core::envelope::SecretRef;

    fn reference(name: &str, key: &str) -> SecretRef {
        serde_json::from_value(serde_json::json!({ "name": name, "key": key })).unwrap()
    }

    #[test]
    fn a_written_secret_reads_back_through_jcctl_and_names_no_value() {
        let identity = age::x25519::Identity::generate();
        let dir = std::env::temp_dir().join(format!("t5971-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("feed.enc.yaml");
        let keys = BTreeMap::from([
            (
                "password".to_owned(),
                zeroize::Zeroizing::new("s3cr3t-value".to_owned()),
            ),
            (
                "username".to_owned(),
                zeroize::Zeroizing::new("importer".to_owned()),
            ),
        ]);
        let text = encrypt(
            "feed",
            &keys,
            &[identity.to_public().to_string()],
            "2026-10-28T22:00:00Z",
        )
        .unwrap();
        assert!(!text.contains("s3cr3t-value"));
        std::fs::write(&file, &text).unwrap();
        let store =
            jcctl::secrets::SecretStore::load_file(&file, std::slice::from_ref(&identity)).unwrap();
        assert_eq!(
            store
                .resolve(&reference("feed", "password"))
                .unwrap()
                .expose(),
            "s3cr3t-value"
        );
        assert_eq!(
            store
                .resolve(&reference("feed", "username"))
                .unwrap()
                .expose(),
            "importer"
        );
        assert_eq!(
            names(&text).unwrap(),
            BTreeMap::from([(
                "feed".to_owned(),
                vec!["password".to_owned(), "username".to_owned()]
            )])
        );
        assert_eq!(recipients_of(&text), vec![identity.to_public().to_string()]);
        let other = age::x25519::Identity::generate();
        assert!(jcctl::secrets::SecretStore::load_file(&file, &[other]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_is_a_label_and_a_plain_file_has_no_names() {
        assert!(valid_name("feed-api"));
        assert!(!valid_name("../feed"));
        assert!(!valid_name(""));
        assert!(!valid_name("-feed"));
        assert_eq!(names("feed: plain\n"), None);
        assert_eq!(path_of("p", "feed"), "projects/p/secrets/feed.enc.yaml");
        assert!(encrypt("feed", &BTreeMap::new(), &["nope".into()], "now").is_err());
    }
}
