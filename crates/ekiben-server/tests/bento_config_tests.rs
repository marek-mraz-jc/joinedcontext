//! Runs `ekiben test` over every suite in tests/bento-config-tests and compares its report with what
//! Bento 1.22.0 printed for the same suite (tests/bento-config-tests/expected, written by
//! scripts/bento-config-tests.sh). Suites needing a component ekiben does not have are listed in
//! `NOT_APPLICABLE`: ekiben must refuse them and name the component.

use std::path::{Path, PathBuf};

use ekiben_server::unit_test::run_all;

/// Suite (relative to tests/bento-config-tests) and the component ekiben lacks for it.
const NOT_APPLICABLE: &[(&str, &str)] = &[
    ("bento/awk.yaml", "awk"),
    ("bento/bloblang/cache_resource.yaml", "cache_get"),
    ("bento/protobuf/house.yaml", "protobuf"),
    ("bento/protobuf/people.yaml", "protobuf"),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn expected_files(dir: &Path, acc: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("expected dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            expected_files(&p, acc);
        } else if p.extension().is_some_and(|x| x == "out") {
            acc.push(p);
        }
    }
}

async fn ekiben_test(suite: &Path) -> (String, String, bool) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let ok = run_all(
        ekiben_testkit::full_registry(),
        &[suite.to_path_buf()],
        &[],
        &mut out,
        &mut err,
    )
    .await;
    (
        String::from_utf8(out).expect("utf-8 report"),
        String::from_utf8(err).expect("utf-8 errors"),
        ok,
    )
}

#[tokio::test]
async fn ekiben_test_reports_what_bento_reports() {
    let root = repo_root();
    let base = root.join("tests/bento-config-tests");
    let mut files = Vec::new();
    expected_files(&base.join("expected"), &mut files);
    files.sort();
    assert!(
        files.len() >= 35,
        "expected outputs missing: {}",
        files.len()
    );

    let prefix = format!("{}/", root.display());
    let mut mismatches = Vec::new();
    let (mut same_pass, mut same_fail, mut not_applicable) = (0, 0, 0);
    for file in &files {
        let rel = file
            .strip_prefix(base.join("expected"))
            .expect("under expected")
            .to_string_lossy()
            .trim_end_matches(".out")
            .to_string();
        let suite = base.join(&rel);
        let (out, err, ok) = ekiben_test(&suite).await;

        if let Some((_, component)) = NOT_APPLICABLE.iter().find(|(s, _)| *s == rel) {
            not_applicable += 1;
            if ok || !(out.contains(component) || err.contains(component)) {
                mismatches.push(format!(
                    "{rel}: not applicable suite must fail naming '{component}'\n{out}{err}"
                ));
            }
            continue;
        }

        let expected = std::fs::read_to_string(file).expect("expected output");
        let got = format!(
            "{}exit {}\n",
            out.replace(&prefix, ""),
            if ok { 0 } else { 1 }
        );
        if got != expected {
            mismatches.push(format!(
                "{rel}:\n--- bento\n{expected}--- ekiben\n{got}{}",
                err.replace(&prefix, "")
            ));
        } else if ok {
            same_pass += 1;
        } else {
            same_fail += 1;
        }
    }
    println!("pass both {same_pass}, fail both {same_fail}, not applicable {not_applicable}");
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[tokio::test]
async fn a_directory_without_tests_reports_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("plain.yaml"),
        "pipeline:\n  processors: []\n",
    )
    .expect("write");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let ok = run_all(
        ekiben_testkit::full_registry(),
        &[dir.path().to_path_buf()],
        &[],
        &mut out,
        &mut err,
    )
    .await;
    assert!(!ok);
    assert_eq!(String::from_utf8_lossy(&out), "No tests were found\n");
}

#[tokio::test]
async fn a_missing_target_config_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let def = dir.path().join("gone_bento_test.yaml");
    std::fs::write(&def, "tests: []\n").expect("write");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let ok = run_all(
        ekiben_testkit::full_registry(),
        &[def],
        &[],
        &mut out,
        &mut err,
    )
    .await;
    assert!(!ok);
    let err = String::from_utf8_lossy(&err);
    assert!(
        err.starts_with("Failed to obtain test targets: unable to access target config file"),
        "{err}"
    );
}
