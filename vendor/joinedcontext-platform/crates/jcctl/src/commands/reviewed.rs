//! The gate in front of every `--repo-dir` command that writes live state (`apply` with a
//! gateway, `publish ckan`, P-14, CHG-12): live state is made only from configuration that
//! validates and that is the reviewed `main`, a clean checkout whose `HEAD` is the remote's
//! default branch. An edit nobody merged reaches the platform as a Change, through the Portal
//! (`jcctl apply -f`), never from a working copy.

use std::path::Path;
use std::process::Command;

/// `Ok` when `dir` validates and is a clean checkout of its remote's default branch; otherwise
/// what is wrong and what to do instead.
pub fn check(dir: &Path) -> Result<(), String> {
    let report = super::validate::run(dir);
    if !report.is_valid() {
        let first = report
            .findings
            .first()
            .map(ToString::to_string)
            .unwrap_or_default();
        return Err(format!(
            "{} does not validate ({} findings, first: {first}); run `jcctl validate --repo-dir {}` \
             and fix it through a Change",
            dir.display(),
            report.findings.len(),
            dir.display()
        ));
    }
    let dirty =
        git(dir, &["status", "--porcelain", "--untracked-files=normal"]).ok_or_else(|| {
            format!(
                "{} is not a git checkout: live state is written only from the reviewed main",
                dir.display()
            )
        })?;
    if !dirty.is_empty() {
        return Err(format!(
            "{} has changes nobody merged ({}); commit them and merge a Change, or apply them \
             through the Portal with `jcctl apply -f`",
            dir.display(),
            dirty.lines().count()
        ));
    }
    let head = git(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
    let main = ["refs/remotes/origin/HEAD", "refs/remotes/origin/main"]
        .iter()
        .find_map(|r| git(dir, &["rev-parse", "--verify", "--quiet", r]).filter(|s| !s.is_empty()))
        .ok_or_else(|| {
            format!(
                "{} has no origin/main to compare with: clone the configuration repository and \
                 check out its main",
                dir.display()
            )
        })?;
    if head != main {
        return Err(format!(
            "{} is at {head}, not at the reviewed main {main}: `git fetch && git checkout \
             origin/main` first; a change goes through a Change",
            dir.display()
        ));
    }
    Ok(())
}

/// The trimmed standard output of `git -C dir <args>`, `None` when git fails.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
