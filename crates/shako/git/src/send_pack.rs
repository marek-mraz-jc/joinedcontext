#![forbid(unsafe_code)]
//! Git Smart HTTP `send-pack` client on `gix` for pushing to push mirrors over HTTPS
//! through `OutboundGuard` (Contract S15, R§2.1, Threat T3).

use crate::pktline::{parse_all_pkt_lines, write_flush, write_text_line};
use crate::repo::{is_ancestor, open_isolated};
use crate::upload_pack::{build_pack_bytes, collect_advertised_refs};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gix_hash::ObjectId;
use shako_core::{OutboundGuard, ShakoError, ShakoResult};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Pushes all `refs/heads/*` and `refs/tags/*` from `repo_path` to `remote_address` over
/// Smart HTTP (`GET .../info/refs?service=git-receive-pack` + `POST .../git-receive-pack`)
/// through `guard`, authenticating with HTTP Basic `username:password`.
/// Never includes credentials in returned errors or logs (Contract S15).
///
/// `branch` scopes the mirror (T-6892): only `refs/heads/{branch}` and new tags go, and only
/// as a fast-forward. A remote ref that moved elsewhere is named in the error, never
/// overwritten; the refs that can go still go.
pub async fn push_mirror_over_https(
    repo_path: &Path,
    remote_address: &str,
    username: &str,
    password: &str,
    guard: &OutboundGuard,
    allowed_hosts: Option<&[String]>,
    branch: Option<&str>,
) -> ShakoResult<()> {
    let base = remote_address.trim_end_matches('/');
    let info_url = format!("{base}/info/refs?service=git-receive-pack");

    let mut headers = Vec::new();
    if !password.is_empty() {
        let basic = STANDARD.encode(format!("{username}:{password}"));
        headers.push(("Authorization".to_owned(), format!("Basic {basic}")));
    }

    let (status, adv_bytes) = guard
        .request(
            reqwest::Method::GET,
            &info_url,
            &headers,
            Vec::new(),
            allowed_hosts,
        )
        .await
        .map_err(|e| scrub_secret(&e, password))?;
    if !(200..300).contains(&status) {
        return Err(ShakoError::Git(format!(
            "remote info/refs answered HTTP {status}"
        )));
    }

    let remote_refs = parse_remote_receive_pack_advertisement(&adv_bytes)?;
    let (commands, pack_bytes, diverged) =
        build_send_pack_payload(repo_path, &remote_refs, branch)?;
    if commands.is_empty() {
        return diverged_error(&diverged);
    }

    let mut body = Vec::new();
    for (idx, (old_oid, new_oid, refname)) in commands.iter().enumerate() {
        if idx == 0 {
            write_text_line(
                &mut body,
                &format!(
                    "{old_oid} {new_oid} {refname}\0 report-status ofs-delta agent=shako/0.1.0"
                ),
            )?;
        } else {
            write_text_line(&mut body, &format!("{old_oid} {new_oid} {refname}"))?;
        }
    }
    write_flush(&mut body)?;
    body.extend_from_slice(&pack_bytes);

    let mut post_headers = headers;
    post_headers.push((
        "Content-Type".to_owned(),
        "application/x-git-receive-pack-request".to_owned(),
    ));
    let post_url = format!("{base}/git-receive-pack");
    let (post_status, resp_bytes) = guard
        .request(
            reqwest::Method::POST,
            &post_url,
            &post_headers,
            body,
            allowed_hosts,
        )
        .await
        .map_err(|e| scrub_secret(&e, password))?;

    if !(200..300).contains(&post_status) {
        return Err(ShakoError::Git(format!(
            "remote git-receive-pack answered HTTP {post_status}"
        )));
    }

    verify_remote_report_status(&resp_bytes)?;
    diverged_error(&diverged)
}

fn diverged_error(diverged: &[String]) -> ShakoResult<()> {
    if diverged.is_empty() {
        return Ok(());
    }
    // A conflict, so the mirror's LastError says it: a Git error answers only that it failed.
    Err(ShakoError::Conflict(format!(
        "the remote's {} moved to commits that are not here; it was not overwritten: \
         reconcile the remote by hand",
        diverged.join(", ")
    )))
}

fn scrub_secret(err: &ShakoError, secret: &str) -> ShakoError {
    if secret.is_empty() {
        return err.clone();
    }
    let msg = err.to_string().replace(secret, "[REDACTED]");
    match err {
        ShakoError::SsrfBlocked(_) => ShakoError::SsrfBlocked(msg),
        _ => ShakoError::Git(msg),
    }
}

fn parse_remote_receive_pack_advertisement(
    bytes: &[u8],
) -> ShakoResult<BTreeMap<String, ObjectId>> {
    let lines = parse_all_pkt_lines(bytes)?;
    let mut map = BTreeMap::new();
    for line in lines {
        let Some(text) = line.as_text() else {
            continue;
        };
        if text.starts_with("# service=") {
            continue;
        }
        let before_nul = text.split('\0').next().unwrap_or(text);
        let mut parts = before_nul.split_whitespace();
        let (Some(oid_s), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if name == "capabilities^{}" || name.ends_with("^{}") {
            continue;
        }
        if let Ok(oid) = ObjectId::from_hex(oid_s.as_bytes()) {
            map.insert(name.to_owned(), oid);
        }
    }
    Ok(map)
}

/// One ref command of a push: `(old id, new id, ref name)`.
type SendPackCommand = (ObjectId, ObjectId, String);

/// The ref commands and the pack for `remote_refs`, and the refs a scoped mirror left alone
/// because the remote's tip is not behind ours.
fn build_send_pack_payload(
    repo_path: &Path,
    remote_refs: &BTreeMap<String, ObjectId>,
    branch: Option<&str>,
) -> ShakoResult<(Vec<SendPackCommand>, Vec<u8>, Vec<String>)> {
    let repo = open_isolated(repo_path)?;
    let (_, local_refs) = collect_advertised_refs(&repo, false)?;
    let zero = ObjectId::null(gix_hash::Kind::Sha1);

    let mut commands = Vec::new();
    let mut new_tips = Vec::new();
    let mut diverged = Vec::new();
    let only = branch.map(|b| format!("refs/heads/{b}"));

    for r in &local_refs {
        if r.name == "HEAD" {
            continue;
        }
        let old_oid = remote_refs.get(&r.name).copied().unwrap_or(zero);
        if let Some(only) = &only {
            if r.name != *only && !r.name.starts_with("refs/tags/") {
                continue;
            }
            // A tag is created, never moved; a branch only moves forward.
            let forward = old_oid == zero
                || (r.name == *only && repo.has_object(old_oid) && is_ancestor(&repo, old_oid, r.oid));
            if old_oid != r.oid && !forward {
                diverged.push(r.name.clone());
                continue;
            }
        }
        if old_oid != r.oid {
            commands.push((old_oid, r.oid, r.name.clone()));
            if let Some(p) = r.peeled {
                new_tips.push(p);
            } else {
                new_tips.push(r.oid);
            }
        }
    }

    if commands.is_empty() {
        return Ok((Vec::new(), Vec::new(), diverged));
    }

    let mut objects: BTreeSet<ObjectId> =
        crate::bundle::collect_all_objects_from_tips(&repo, &new_tips)?
            .into_iter()
            .collect();
    for (_, new_oid, _) in &commands {
        if repo.has_object(*new_oid) {
            objects.insert(*new_oid);
        }
    }

    let oid_vec: Vec<ObjectId> = objects.into_iter().collect();
    let pack = build_pack_bytes(&repo, &oid_vec, false)?;
    Ok((commands, pack, diverged))
}

fn verify_remote_report_status(resp: &[u8]) -> ShakoResult<()> {
    let lines = parse_all_pkt_lines(resp)?;
    for line in lines {
        let Some(text) = line.as_text() else {
            continue;
        };
        if let Some(reason) = text.strip_prefix("unpack ")
            && reason.trim() != "ok"
        {
            return Err(ShakoError::Git(format!("remote unpack failed: {reason}")));
        }
        if let Some(ng) = text.strip_prefix("ng ") {
            return Err(ShakoError::Git(format!("remote rejected ref: {ng}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn oid(hex: &str) -> ObjectId {
        ObjectId::from_hex(hex.as_bytes()).expect("an object id")
    }

    /// `main` with two commits and a tag, `change/1` beside it: what a configuration
    /// repository holds while a Change is open.
    fn repository(dir: &Path) -> (String, String, String) {
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let first = git(dir, &["rev-parse", "HEAD"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let second = git(dir, &["rev-parse", "HEAD"]);
        git(dir, &["tag", "v1"]);
        git(dir, &["checkout", "-q", "-b", "change/1", &first]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "open change"]);
        let change = git(dir, &["rev-parse", "HEAD"]);
        (first, second, change)
    }

    fn names(commands: &[SendPackCommand]) -> Vec<&str> {
        commands.iter().map(|(_, _, name)| name.as_str()).collect()
    }

    #[test]
    fn nothing_but_main_is_pushed() {
        let tmp = tempfile::tempdir().expect("tmp");
        repository(tmp.path());
        let (commands, pack, diverged) =
            build_send_pack_payload(&tmp.path().join(".git"), &BTreeMap::new(), Some("main")).expect("payload");
        assert_eq!(names(&commands), ["refs/heads/main", "refs/tags/v1"]);
        assert!(!pack.is_empty());
        assert!(diverged.is_empty());

        // The App mirror keeps every branch.
        let (all, _, _) =
            build_send_pack_payload(&tmp.path().join(".git"), &BTreeMap::new(), None).expect("payload");
        assert!(names(&all).contains(&"refs/heads/change/1"), "{:?}", names(&all));
    }

    #[test]
    fn no_force_push_ever() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (first, second, change) = repository(tmp.path());

        // Behind: a fast-forward goes.
        let behind = BTreeMap::from([("refs/heads/main".to_owned(), oid(&first))]);
        let (commands, _, diverged) =
            build_send_pack_payload(&tmp.path().join(".git"), &behind, Some("main")).expect("payload");
        assert_eq!(commands[0], (oid(&first), oid(&second), "refs/heads/main".to_owned()));
        assert!(diverged.is_empty());

        // On a sibling commit, on a commit this repository never saw, or a moved tag: reported,
        // never overwritten.
        let unknown = "1111111111111111111111111111111111111111";
        for remote in [
            BTreeMap::from([("refs/heads/main".to_owned(), oid(&change))]),
            BTreeMap::from([("refs/heads/main".to_owned(), oid(unknown))]),
        ] {
            let (commands, _, diverged) =
                build_send_pack_payload(&tmp.path().join(".git"), &remote, Some("main")).expect("payload");
            assert_eq!(names(&commands), ["refs/tags/v1"]);
            assert_eq!(diverged, ["refs/heads/main"]);
        }
        let moved_tag = BTreeMap::from([
            ("refs/heads/main".to_owned(), oid(&second)),
            ("refs/tags/v1".to_owned(), oid(&first)),
        ]);
        let (commands, _, diverged) =
            build_send_pack_payload(&tmp.path().join(".git"), &moved_tag, Some("main")).expect("payload");
        assert!(commands.is_empty());
        assert_eq!(diverged, ["refs/tags/v1"]);
        let err = diverged_error(&diverged).expect_err("reported");
        assert!(err.to_string().contains("not overwritten"), "{err}");
    }
}
