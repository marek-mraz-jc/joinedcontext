#![forbid(unsafe_code)]
//! Error paths of realm file loading, the `--import-dir` digest (K17) and the CLI: unreadable
//! files and directories, dangling symlinks, a tampered saved plan (stale, exit 3) and a serve
//! whose import directory cannot be applied.

use kaisatsu_server::{
    compute_import_dir_digest, group_files_by_realm, load_realm_files_from_path,
};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_kaisatsu");

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("kaisatsu-srv-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn file(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).expect("write temp file");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Give back permissions taken away by a test so the directory can be removed.
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for e in entries.flatten() {
                let _ = std::fs::set_permissions(e.path(), std::fs::Permissions::from_mode(0o700));
            }
        }
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn chmod(p: &Path, mode: u32) {
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn kaisatsu(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env_remove("KAISATSU_SRV_TEST_MISSING")
        .output()
        .expect("run kaisatsu")
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 path")
}

/// The sandbox may run as root, where file modes do not refuse reads.
fn modes_refuse() -> bool {
    let probe = TempDir::new();
    let f = probe.file("probe.json", "{}");
    chmod(&f, 0o000);
    std::fs::read(&f).is_err()
}

#[test]
fn unreadable_files_and_directories_are_refused_by_name() {
    if !modes_refuse() {
        println!("SKIP: running as root, file modes do not refuse reads");
        return;
    }
    let dir = TempDir::new();
    let f = dir.file("realm.json", r#"{"realm":"x"}"#);
    chmod(&f, 0o000);
    let err = load_realm_files_from_path(&f).expect_err("unreadable file");
    assert!(err.to_string().contains("realm.json"), "{err}");
    let err = compute_import_dir_digest(&dir.0).expect_err("unreadable file in the digest");
    assert!(err.to_string().contains("realm.json"), "{err}");

    let locked = TempDir::new();
    locked.file("a.json", r#"{"realm":"x"}"#);
    chmod(&locked.0, 0o000);
    assert!(load_realm_files_from_path(&locked.0).is_err());
    assert!(compute_import_dir_digest(&locked.0).is_err());
    chmod(&locked.0, 0o700);
}

#[test]
fn dangling_symlinks_and_missing_directories_are_refused() {
    let dir = TempDir::new();
    symlink(dir.0.join("gone.json"), dir.0.join("dangling.json")).expect("symlink");
    let err = load_realm_files_from_path(&dir.0).expect_err("dangling symlink");
    assert!(err.to_string().contains("dangling.json"), "{err}");
    let err = compute_import_dir_digest(&dir.0).expect_err("dangling symlink in the digest");
    assert!(err.to_string().contains("dangling.json"), "{err}");
    assert!(compute_import_dir_digest(&dir.0.join("missing")).is_err());

    // Files of other kinds are ignored; the digest depends on name and content.
    let clean = TempDir::new();
    clean.file("notes.txt", "ignored");
    clean.file("README", "ignored");
    let empty = compute_import_dir_digest(&clean.0).unwrap();
    clean.file("a.json", r#"{"realm":"x"}"#);
    let one = compute_import_dir_digest(&clean.0).unwrap();
    assert_ne!(empty, one);
    assert_eq!(one, compute_import_dir_digest(&clean.0).unwrap());
}

#[test]
fn files_are_grouped_by_realm_in_file_order() {
    let files = vec![
        ("01-a.json".to_owned(), serde_json::json!({"realm": "a"})),
        ("02-b.json".to_owned(), serde_json::json!({"realm": "b"})),
        ("03-a.yaml".to_owned(), serde_json::json!({"realm": "a"})),
    ];
    let groups = group_files_by_realm(files);
    assert_eq!(groups.len(), 2);
    let names: Vec<Vec<&str>> = groups
        .iter()
        .map(|g| g.iter().map(|(n, _)| n.as_str()).collect())
        .collect();
    assert_eq!(
        names,
        vec![vec!["01-a.json", "03-a.yaml"], vec!["02-b.json"]]
    );
}

#[test]
fn a_tampered_saved_plan_is_stale() {
    let dir = TempDir::new();
    let realm = dir.file("realm.json", r#"{"realm":"plan","displayName":"Plan"}"#);
    let plan = dir.0.join("plan.json");
    let out = kaisatsu(&["plan", "-f", s(&realm), "-o", s(&plan)]);
    assert!(
        out.status.success() || out.status.code() == Some(2),
        "{out:?}"
    );
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&plan).unwrap()).unwrap();
    doc["baseStateChecksum"] = serde_json::json!("0000");
    std::fs::write(&plan, serde_json::to_vec(&doc).unwrap()).unwrap();
    let out = kaisatsu(&["apply", "-f", s(&plan)]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("changed since plan"));
}

#[test]
fn serve_refuses_an_import_dir_it_cannot_apply() {
    let kek_dir = TempDir::new();
    let kek = kek_dir.0.join("kek");
    std::fs::write(&kek, [9u8; 32]).unwrap();

    // A broken file: refused before listening.
    let broken = TempDir::new();
    broken.file("01.json", "{ not json");
    let out = kaisatsu(&["serve", "--kek-file", s(&kek), "--import-dir", s(&broken.0)]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("not json"));

    // Two realms; the second names a secret that is not there: refused, nothing served.
    let two = TempDir::new();
    two.file("01-one.json", r#"{"realm":"one"}"#);
    two.file(
        "02-two.json",
        r#"{"realm":"two","users":[{"username":"u","credentials":[{"type":"password","value":"$(env:KAISATSU_SRV_TEST_MISSING)"}]}]}"#,
    );
    let out = kaisatsu(&[
        "serve",
        "--kek-file",
        s(&kek),
        "--listen",
        "127.0.0.1:0",
        "--import-dir",
        s(&two.0),
    ]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}
