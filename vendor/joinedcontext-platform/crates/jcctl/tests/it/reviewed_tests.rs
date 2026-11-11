//! P-14: `jcctl apply --repo-dir` with a gateway and `jcctl publish ckan` write live state only
//! from configuration that validates and is the reviewed `main`.

use crate::common::{demo_repo, write};
use jcctl::commands::reviewed;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.org"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// The demo repository committed and its `origin/main` at `HEAD`: what a fresh clone looks like.
fn reviewed_main(test: &str) -> std::path::PathBuf {
    let dir = demo_repo(test);
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "reviewed"]);
    git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    dir
}

fn jcctl(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_jcctl"))
        .args(args)
        .env_remove("JC_GATEWAY_URL")
        .env_remove("JC_TOKEN_FILE")
        .output()
        .expect("jcctl runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_reviewed_main_passes_the_gate() {
    let dir = reviewed_main("reviewed-ok");
    assert_eq!(reviewed::check(&dir), Ok(()));
}

#[test]
fn a_tree_that_is_not_the_reviewed_main_is_refused() {
    let plain = demo_repo("reviewed-plain");
    let err = reviewed::check(&plain).expect_err("not a checkout");
    assert!(err.contains("not a git checkout"), "{err}");

    let dirty = reviewed_main("reviewed-dirty");
    write(
        &dirty,
        "projects/ovzdusie/README.md",
        "an edit nobody merged\n",
    );
    let err = reviewed::check(&dirty).expect_err("dirty");
    assert!(err.contains("changes nobody merged"), "{err}");

    let ahead = reviewed_main("reviewed-ahead");
    write(
        &ahead,
        "projects/ovzdusie/README.md",
        "committed, never merged\n",
    );
    git(&ahead, &["add", "-A"]);
    git(&ahead, &["commit", "-qm", "local"]);
    let err = reviewed::check(&ahead).expect_err("ahead of main");
    assert!(err.contains("not at the reviewed main"), "{err}");

    let no_remote = demo_repo("reviewed-no-remote");
    git(&no_remote, &["init", "-q", "-b", "main"]);
    git(&no_remote, &["add", "-A"]);
    git(&no_remote, &["commit", "-qm", "local only"]);
    let err = reviewed::check(&no_remote).expect_err("no origin");
    assert!(err.contains("no origin/main"), "{err}");
}

#[test]
fn an_invalid_reviewed_main_is_refused() {
    let dir = reviewed_main("reviewed-invalid");
    write(
        &dir,
        "projects/ovzdusie/spaces/ovzdusie/endpoints/broken.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: broken\nspec: {}\n",
    );
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "broken"]);
    git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let err = reviewed::check(&dir).expect_err("invalid");
    assert!(err.contains("does not validate"), "{err}");
}

/// Both live writers stop at the gate before they reach the network: the gateway and the
/// catalogue named here are closed ports.
#[test]
fn apply_and_publish_refuse_a_working_copy_nobody_merged() {
    let dir = reviewed_main("reviewed-cli");
    write(
        &dir,
        "projects/ovzdusie/README.md",
        "an edit nobody merged\n",
    );
    let token = dir.join("token");
    std::fs::write(&token, "not-a-real-token").unwrap();
    let repo = dir.to_str().unwrap();
    // The token file is untracked, so it alone makes the tree dirty; the README makes sure.
    let (ok, said) = jcctl(&[
        "apply",
        "--repo-dir",
        repo,
        "--gateway-url",
        "http://127.0.0.1:9",
        "--token-file",
        token.to_str().unwrap(),
    ]);
    assert!(!ok && said.contains("changes nobody merged"), "{said}");
    let (ok, said) = jcctl(&[
        "publish",
        "ckan",
        "--repo-dir",
        repo,
        "--project",
        "ovzdusie",
        "--host",
        "127.0.0.1:9",
    ]);
    assert!(!ok && said.contains("changes nobody merged"), "{said}");

    // Without a gateway, `apply` writes nothing live and still reads any working copy.
    let (ok, said) = jcctl(&["apply", "--repo-dir", repo]);
    assert!(ok, "{said}");
}
