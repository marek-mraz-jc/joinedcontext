#![forbid(unsafe_code)]
//! Apply refusals the offline check lets through: a member of the wrong JSON type in each section
//! fails the typed parse with BadRequest, a secret file that cannot be read is refused by name,
//! a secrets directory that does not exist confines nothing, and a document that is no object is
//! refused before any reference check.

use kaisatsu_core::{KaisatsuError, Store};
use kaisatsu_import::*;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn opts() -> ApplyOptions {
    ApplyOptions {
        mode: ApplyMode::NoDelete,
        dry_run: false,
        allow_literal_secrets: true,
        secrets_dir: None,
        env_vars: BTreeMap::new(),
    }
}

fn store() -> Arc<dyn Store> {
    Arc::new(MemoryStore::default())
}

#[test]
fn a_document_that_is_no_object_is_refused() {
    let e = check_realm_value(&json!(["realm"]), &opts()).unwrap_err();
    assert!(e.to_string().contains("must be an object"), "{e}");
}

#[tokio::test]
async fn a_member_of_the_wrong_type_is_refused_per_section() {
    let cases: [(&str, Value); 6] = [
        (
            "clientScopes",
            json!({"clientScopes": [{"name": "s", "protocol": 5}]}),
        ),
        (
            "roles",
            json!({"roles": {"realm": [{"name": "r", "composite": "yes"}]}}),
        ),
        (
            "groups",
            json!({"groups": [{"name": "g", "attributes": "flat"}]}),
        ),
        (
            "identityProviders",
            json!({"identityProviders": [{"alias": "i", "providerId": "oidc", "enabled": "yes"}]}),
        ),
        (
            "clients",
            json!({"clients": [{"clientId": "c", "enabled": "yes"}]}),
        ),
        (
            "users",
            json!({"users": [{"username": "u", "enabled": "yes"}]}),
        ),
    ];
    for (section, mut doc) in cases {
        doc["realm"] = json!("typed");
        let s = store();
        let e = apply_realm_files(&s, &[("r.json".into(), doc)], &opts())
            .await
            .expect_err(section);
        assert!(
            matches!(e, KaisatsuError::BadRequest(_)),
            "{section}: {e:?}"
        );
    }
}

#[test]
fn secret_files_that_cannot_be_read_or_confined_are_refused() {
    let base = std::env::temp_dir().join(format!("kaisatsu-imp-{}", uuid::Uuid::now_v7()));
    let secrets = base.join("secrets");
    std::fs::create_dir_all(secrets.join("a-directory")).unwrap();
    let with_file = |path: String| {
        json!({"realm": "files", "clients": [{
            "clientId": "c", "publicClient": false, "secret": format!("$(file:{path})")
        }]})
    };
    // A directory inside the secrets directory: confined, but no file to read.
    let in_dir = ApplyOptions {
        secrets_dir: Some(secrets.clone()),
        ..opts()
    };
    let doc = with_file(secrets.join("a-directory").display().to_string());
    let e = check_realm_value(&doc, &in_dir).unwrap_err().to_string();
    assert!(e.contains("cannot read secret file"), "{e}");

    // A secrets directory that does not exist confines to its literal path: /etc is outside.
    let gone = ApplyOptions {
        secrets_dir: Some(base.join("never-made")),
        ..opts()
    };
    let e = check_realm_value(&with_file("/etc/hosts".into()), &gone)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("escapes the configured secrets directory"),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&base);
}
