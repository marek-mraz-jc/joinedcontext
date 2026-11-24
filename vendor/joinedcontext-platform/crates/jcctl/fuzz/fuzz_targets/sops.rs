//! Input: a secrets file from the repository (sops-encrypted or not), read with a key that did
//! not seal it. The seed is plain YAML: an encrypted fixture is never committed (public-check).
//! Reading ends in a store or an error, never a panic; no value decrypts under a foreign key.
#![no_main]

use jcctl::SecretStore;
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

fn scratch() -> &'static std::path::PathBuf {
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("jcctl-fuzz-sops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    })
}

fuzz_target!(|data: &[u8]| {
    let path = scratch().join("secrets.enc.yaml");
    std::fs::write(&path, data).expect("scratch file");
    let identity = age::x25519::Identity::generate();
    if let Ok(store) = SecretStore::load_file(&path, &[identity]) {
        assert_eq!(store.names().count(), 0, "a foreign key opened a value");
    }
});
