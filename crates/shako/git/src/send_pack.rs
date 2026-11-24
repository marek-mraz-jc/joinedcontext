#![forbid(unsafe_code)]
//! Git Smart HTTP `send-pack` client on `gix` for pushing to push mirrors over HTTPS
//! through `OutboundGuard` (Contract S15, R§2.1, Threat T3).

use crate::pack::{Admit, write_pack};
use crate::pktline::{parse_all_pkt_lines, write_flush, write_text_line};
use crate::repo::{is_ancestor, open_isolated};
use crate::upload_pack::collect_advertised_refs;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gix_hash::ObjectId;
use shako_core::{OutboundBody, OutboundGuard, OutboundMethod, ShakoError, ShakoResult};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

/// A push mirror's remote over Smart HTTP (`GET .../info/refs?service=git-receive-pack` +
/// `POST .../git-receive-pack`) through `guard`, authenticated with HTTP Basic
/// `username:password`. Never includes credentials in returned errors or logs (Contract S15).
pub struct MirrorRemote<'a> {
    base: String,
    password: &'a str,
    guard: &'a OutboundGuard,
    headers: Vec<(String, String)>,
}

impl<'a> MirrorRemote<'a> {
    #[must_use]
    pub fn new(
        remote_address: &str,
        username: &str,
        password: &'a str,
        guard: &'a OutboundGuard,
    ) -> Self {
        let mut headers = Vec::new();
        if !password.is_empty() {
            let basic = STANDARD.encode(format!("{username}:{password}"));
            headers.push(("Authorization".to_owned(), format!("Basic {basic}")));
        }
        Self {
            base: remote_address.trim_end_matches('/').to_owned(),
            password,
            guard,
            headers,
        }
    }

    /// The refs the remote holds.
    pub async fn refs(&self) -> ShakoResult<BTreeMap<String, ObjectId>> {
        let info_url = format!("{}/info/refs?service=git-receive-pack", self.base);
        let (status, adv_bytes) = self
            .guard
            .request(
                OutboundMethod::GET,
                &info_url,
                &self.headers,
                Vec::new(),
                None,
            )
            .await
            .map_err(|e| scrub_secret(&e, self.password))?;
        if !(200..300).contains(&status) {
            return Err(remote_answer(format!(
                "the remote answered HTTP {status} to the ref listing{}",
                if matches!(status, 401 | 403) {
                    ": check the mirror's credential"
                } else {
                    ""
                }
            )));
        }
        parse_remote_receive_pack_advertisement(&adv_bytes)
    }

    /// Sends the request [`write_send_pack`] wrote and says what the mirror could not move.
    pub async fn push(&self, plan: &SendPack, request: std::fs::File) -> ShakoResult<()> {
        if plan.commands.is_empty() {
            return diverged_error(&plan.diverged);
        }
        let mut headers = self.headers.clone();
        headers.push((
            "Content-Type".to_owned(),
            "application/x-git-receive-pack-request".to_owned(),
        ));
        let post_url = format!("{}/git-receive-pack", self.base);
        let body = OutboundBody::File(request);
        let (status, answer) = self
            .guard
            .send(OutboundMethod::POST, &post_url, &headers, body, None)
            .await
            .map_err(|e| scrub_secret(&e, self.password))?;
        if !(200..300).contains(&status) {
            return Err(remote_answer(format!(
                "the remote answered HTTP {status} to the push"
            )));
        }
        verify_remote_report_status(&answer, &plan.commands, self.password)?;
        diverged_error(&plan.diverged)
    }
}

/// What a mirror push changes, and the refs a scoped mirror left alone because the remote's tip
/// is not behind ours.
#[derive(Debug)]
pub struct SendPack {
    commands: Vec<SendPackCommand>,
    diverged: Vec<String>,
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

/// What the remote answered, kept readable in the mirror's LastError (a Git error answers only
/// that something failed). It carries no secret of ours; the remote's own text is cut at 200
/// characters.
fn remote_answer(text: String) -> ShakoError {
    ShakoError::Conflict(text.chars().take(200).collect())
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

/// Writes the push that brings the remote from `remote_refs` to this repository: the commands
/// and a pack of only what the remote lacks (T-7522).
///
/// All of `refs/heads/*` and `refs/tags/*` go. `branch` scopes the mirror (T-6892): only
/// `refs/heads/{branch}` and new tags go, and only as a fast-forward. A remote ref that moved
/// elsewhere is named in the error, never overwritten; the refs that can go still go.
pub fn write_send_pack(
    repo_path: &Path,
    remote_refs: &BTreeMap<String, ObjectId>,
    branch: Option<&str>,
    mut out: impl Write,
    admit: Admit<'_>,
) -> ShakoResult<SendPack> {
    let repo = open_isolated(repo_path)?;
    let (_, local_refs) = collect_advertised_refs(&repo, false)?;
    let zero = ObjectId::null(gix_hash::Kind::Sha1);

    let mut commands = Vec::new();
    let mut new_tips = Vec::new();
    let mut diverged = Vec::new();
    let only = branch.map(|b| format!("refs/heads/{b}"));
    let main_tip = only
        .as_ref()
        .and_then(|only| local_refs.iter().find(|r| r.name == *only))
        .map(|r| r.oid);

    for r in &local_refs {
        if r.name == "HEAD" {
            continue;
        }
        let old_oid = remote_refs.get(&r.name).copied().unwrap_or(zero);
        if let Some(only) = &only {
            if r.name != *only && !r.name.starts_with("refs/tags/") {
                continue;
            }
            // A tag goes only when it points at main's head or one of its ancestors.
            if r.name != *only
                && !main_tip.is_some_and(|tip| is_ancestor(&repo, r.peeled.unwrap_or(r.oid), tip))
            {
                continue;
            }
            // A tag is created, never moved; a branch only moves forward.
            let forward = old_oid == zero
                || (r.name == *only
                    && repo.has_object(old_oid)
                    && is_ancestor(&repo, old_oid, r.oid));
            if old_oid != r.oid && !forward {
                diverged.push(r.name.clone());
                continue;
            }
        }
        if old_oid != r.oid {
            commands.push((old_oid, r.oid, r.name.clone()));
            new_tips.push(r.oid);
        }
    }

    if commands.is_empty() {
        return Ok(SendPack { commands, diverged });
    }

    for (idx, (old_oid, new_oid, refname)) in commands.iter().enumerate() {
        let caps = if idx == 0 {
            "\0 report-status ofs-delta agent=shako/0.1.0"
        } else {
            ""
        };
        write_text_line(&mut out, &format!("{old_oid} {new_oid} {refname}{caps}"))?;
    }
    write_flush(&mut out)?;
    let held: Vec<ObjectId> = remote_refs.values().copied().collect();
    let objects = objects_to_send(&repo, &new_tips, &held)?;
    write_pack(&repo, &objects, &mut out, admit)?;
    out.flush()
        .map_err(|e| ShakoError::Storage(format!("mirror spool: {e}")))?;
    Ok(SendPack { commands, diverged })
}

/// The objects reachable from `tips` that a holder of `remote` lacks. A failed walk fails the
/// push; where in doubt an object goes.
fn objects_to_send(
    repo: &gix::Repository,
    tips: &[ObjectId],
    remote: &[ObjectId],
) -> ShakoResult<Vec<ObjectId>> {
    let failed = |e: &dyn std::fmt::Display| ShakoError::Git(format!("mirror walk failed: {e}"));
    let mut known = BTreeSet::new();
    let mut hidden = Vec::new();
    for id in remote.iter().filter(|id| repo.has_object(**id)) {
        let mut at = *id;
        loop {
            known.insert(at);
            let object = repo.find_object(at).map_err(|e| failed(&e))?;
            match object.kind {
                gix_object::Kind::Tag => {
                    let tag = object.try_into_tag().map_err(|e| failed(&e))?;
                    at = tag.target_id().map_err(|e| failed(&e))?.detach();
                }
                gix_object::Kind::Commit => {
                    hidden.push(at);
                    break;
                }
                _ => break,
            }
        }
    }

    let mut send = BTreeSet::new();
    let mut starts = Vec::new();
    let mut loose_trees = Vec::new();
    for tip in tips {
        let mut at = *tip;
        loop {
            let object = repo.find_object(at).map_err(|e| failed(&e))?;
            match object.kind {
                gix_object::Kind::Tag => {
                    let tag = object.try_into_tag().map_err(|e| failed(&e))?;
                    let target = tag.target_id().map_err(|e| failed(&e))?.detach();
                    send.insert(at);
                    at = target;
                }
                gix_object::Kind::Commit => {
                    starts.push(at);
                    break;
                }
                gix_object::Kind::Tree => {
                    loose_trees.push(at);
                    break;
                }
                gix_object::Kind::Blob => {
                    send.insert(at);
                    break;
                }
            }
        }
    }

    let mut commits = BTreeSet::new();
    if !starts.is_empty() {
        let walk = repo
            .rev_walk(starts)
            .with_hidden(hidden)
            .all()
            .map_err(|e| failed(&e))?;
        for info in walk {
            commits.insert(info.map_err(|e| failed(&e))?.id);
        }
    }
    let mut trees = loose_trees;
    let mut boundary = BTreeSet::new();
    for id in &commits {
        let commit = repo.find_commit(*id).map_err(|e| failed(&e))?;
        trees.push(commit.tree_id().map_err(|e| failed(&e))?.detach());
        for parent in commit.parent_ids() {
            if !commits.contains(&parent.detach()) {
                boundary.insert(parent.detach());
            }
        }
    }
    // A parent outside the walk is one the remote holds, with all its tree reaches.
    for id in boundary {
        let tree = repo
            .find_commit(id)
            .map_err(|e| failed(&e))?
            .tree()
            .map_err(|e| failed(&e))?;
        crate::bundle::collect_all_tree_objects(repo, &tree, &mut known)?;
    }
    let held = known.clone();
    for id in trees {
        let tree = repo.find_tree(id).map_err(|e| failed(&e))?;
        crate::bundle::collect_all_tree_objects(repo, &tree, &mut known)?;
    }
    send.extend(commits);
    send.extend(known.difference(&held).copied());
    Ok(send.into_iter().collect())
}

fn verify_remote_report_status(
    resp: &[u8],
    commands: &[SendPackCommand],
    secret: &str,
) -> ShakoResult<()> {
    let lines = parse_all_pkt_lines(resp)?;
    let shown = |text: &str| {
        if secret.is_empty() {
            text.to_owned()
        } else {
            text.replace(secret, "[REDACTED]")
        }
    };
    let mut unpacked = false;
    let mut moved = BTreeSet::new();
    for line in lines {
        let Some(text) = line.as_text() else {
            continue;
        };
        if let Some(reason) = text.strip_prefix("unpack ") {
            if reason.trim() != "ok" {
                return Err(remote_answer(format!(
                    "the remote could not unpack the push: {}",
                    shown(reason)
                )));
            }
            unpacked = true;
        }
        if let Some(ng) = text.strip_prefix("ng ") {
            return Err(remote_answer(format!(
                "the remote refused the ref: {}",
                shown(ng)
            )));
        }
        if let Some(name) = text.strip_prefix("ok ") {
            moved.insert(name.trim().to_owned());
        }
    }
    if !unpacked || commands.iter().any(|(_, _, name)| !moved.contains(name)) {
        return Err(remote_answer(
            "the remote did not confirm every ref of the push".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{
        CommitAuthor, FileChangeOp, FileOpKind, commit_file_changes, init_bare_repo, set_ref_direct,
    };
    use shako_core::{InstanceId, RepoId};
    use std::path::PathBuf;

    fn oid(hex: &str) -> ObjectId {
        ObjectId::from_hex(hex.as_bytes()).expect("an object id")
    }

    /// `main` with two commits and a tag, `change/1` beside it: what a configuration
    /// repository holds while a Change is open.
    fn repository(dir: &Path) -> (PathBuf, String, String, String) {
        let repo = init_bare_repo(
            dir,
            InstanceId::from_u128(1),
            RepoId::from_u128(7),
            "main",
            false,
            "t",
        )
        .expect("init");
        let author = CommitAuthor {
            name: "t".into(),
            email: "t@example.invalid".into(),
        };
        let commit = |branch: &str, path: &str| {
            commit_file_changes(
                &repo,
                branch,
                path,
                &author,
                &author,
                &[FileChangeOp {
                    operation: FileOpKind::Create,
                    path: path.into(),
                    content: Some(b"kind: Endpoint\n".to_vec()),
                    sha: None,
                }],
            )
            .expect("commit")
        };
        let first = commit("main", "one.yaml");
        let second = commit("main", "two.yaml");
        set_ref_direct(&repo, "refs/tags/v1", &second).expect("tag");
        set_ref_direct(&repo, "refs/heads/change/1", &first).expect("branch");
        let change = commit("change/1", "open.yaml");
        // A tag on the open Change's commit is not on main and stays home.
        set_ref_direct(&repo, "refs/tags/unapproved", &change).expect("tag");
        (repo, first, second, change)
    }

    fn payload(
        repo: &Path,
        remote: &BTreeMap<String, ObjectId>,
        branch: Option<&str>,
    ) -> ShakoResult<(Vec<SendPackCommand>, Vec<u8>, Vec<String>)> {
        let mut out = Vec::new();
        let plan = write_send_pack(
            repo,
            remote,
            branch,
            &mut out,
            &mut crate::testutil::admit_all,
        )?;
        Ok((plan.commands, out, plan.diverged))
    }

    fn names(commands: &[SendPackCommand]) -> Vec<&str> {
        commands.iter().map(|(_, _, name)| name.as_str()).collect()
    }

    #[test]
    fn nothing_but_main_is_pushed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, ..) = repository(tmp.path());
        let (commands, pack, diverged) =
            payload(&repo, &BTreeMap::new(), Some("main")).expect("payload");
        assert_eq!(names(&commands), ["refs/heads/main", "refs/tags/v1"]);
        assert!(!pack.is_empty());
        assert!(diverged.is_empty());

        // The App mirror keeps every branch.
        let (all, _, _) = payload(&repo, &BTreeMap::new(), None).expect("payload");
        assert!(
            names(&all).contains(&"refs/heads/change/1"),
            "{:?}",
            names(&all)
        );
    }

    #[test]
    fn no_force_push_ever() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, first, second, change) = repository(tmp.path());

        // Behind: a fast-forward goes.
        let behind = BTreeMap::from([("refs/heads/main".to_owned(), oid(&first))]);
        let (commands, _, diverged) = payload(&repo, &behind, Some("main")).expect("payload");
        assert_eq!(
            commands[0],
            (oid(&first), oid(&second), "refs/heads/main".to_owned())
        );
        assert!(diverged.is_empty());

        // On a sibling commit, on a commit this repository never saw, or a moved tag: reported,
        // never overwritten.
        let unknown = "1111111111111111111111111111111111111111";
        for remote in [
            BTreeMap::from([("refs/heads/main".to_owned(), oid(&change))]),
            BTreeMap::from([("refs/heads/main".to_owned(), oid(unknown))]),
        ] {
            let (commands, _, diverged) = payload(&repo, &remote, Some("main")).expect("payload");
            assert_eq!(names(&commands), ["refs/tags/v1"]);
            assert_eq!(diverged, ["refs/heads/main"]);
        }
        let moved_tag = BTreeMap::from([
            ("refs/heads/main".to_owned(), oid(&second)),
            ("refs/tags/v1".to_owned(), oid(&first)),
        ]);
        let (commands, _, diverged) = payload(&repo, &moved_tag, Some("main")).expect("payload");
        assert!(commands.is_empty());
        assert_eq!(diverged, ["refs/tags/v1"]);
        let err = diverged_error(&diverged).expect_err("reported");
        assert!(err.to_string().contains("not overwritten"), "{err}");
    }

    use crate::testutil::{annotated_tag, pack_count, put, receive, repo};
    use shako_core::{OutboundPolicy, RefPolicy};
    use std::sync::{Arc, Mutex};

    const PASSWORD: &str = "s3cr3t-pw";

    /// A guard that lets `mirror.example` through and answers every request with `answer`.
    fn guard(
        answer: impl Fn(&str, &str, &[(String, String)], &[u8]) -> (u16, Vec<u8>)
        + Send
        + Sync
        + 'static,
    ) -> OutboundGuard {
        OutboundGuard::new(OutboundPolicy {
            allowed_hosts: Some(vec!["mirror.example".into()]),
            mock_handler: Some(Arc::new(
                move |method: &str, url: &str, headers: &[(String, String)], body: &[u8]| {
                    Some(answer(method, url, headers, body))
                },
            )),
            ..OutboundPolicy::default()
        })
    }

    fn plan(repo: &Path, remote: &BTreeMap<String, ObjectId>) -> (SendPack, std::fs::File) {
        let mut spool = tempfile::tempfile().expect("spool");
        let plan = write_send_pack(
            repo,
            remote,
            Some("main"),
            &mut spool,
            &mut crate::testutil::admit_all,
        )
        .expect("plan");
        (plan, spool)
    }

    #[tokio::test]
    async fn a_mirror_push_lands_in_a_real_receive_pack_with_basic_auth() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (source, _, second, _) = repository(tmp.path());
        let dest = repo(tmp.path(), 99, false);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (into, log) = (dest.clone(), seen.clone());
        let guard = guard(move |method, url, headers, body| {
            log.lock()
                .expect("log")
                .push((method.to_owned(), url.to_owned(), headers.to_vec()));
            if method == "GET" {
                (
                    200,
                    crate::receive_pack::advertise_receive_pack(&into).expect("adv"),
                )
            } else {
                let out = receive(&into, &RefPolicy::default(), body).expect("receive");
                (200, out.response_bytes)
            }
        });
        let remote = MirrorRemote::new("https://mirror.example/o/r.git/", "bot", PASSWORD, &guard);
        let refs = remote.refs().await.expect("refs of an empty remote");
        assert!(refs.is_empty());
        let (sent, spool) = plan(&source, &refs);
        remote.push(&sent, spool).await.expect("push");
        assert_eq!(
            crate::repo::list_branches(&dest).expect("branches"),
            [("main".to_owned(), second.clone())]
        );
        let refs = remote.refs().await.expect("refs");
        assert_eq!(refs.get("refs/heads/main"), Some(&oid(&second)));
        assert_eq!(refs.get("refs/tags/v1"), Some(&oid(&second)));
        // Nothing new: no request goes out.
        let before = seen.lock().expect("log").len();
        let (idle, spool) = plan(&source, &refs);
        remote.push(&idle, spool).await.expect("nothing to push");
        assert_eq!(seen.lock().expect("log").len(), before);

        let log = seen.lock().expect("log");
        let (method, url, headers) = &log[1];
        assert_eq!(
            (method.as_str(), url.as_str()),
            ("POST", "https://mirror.example/o/r.git/git-receive-pack")
        );
        let basic = format!("Basic {}", STANDARD.encode(format!("bot:{PASSWORD}")));
        assert!(
            headers.contains(&("Authorization".to_owned(), basic)),
            "{headers:?}"
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v.ends_with("receive-pack-request"))
        );
        assert_eq!(
            log[0].1,
            "https://mirror.example/o/r.git/info/refs?service=git-receive-pack"
        );
    }

    #[tokio::test]
    async fn a_mirror_without_a_password_sends_no_authorization() {
        let headers = Arc::new(Mutex::new(Vec::new()));
        let log = headers.clone();
        let guard = guard(move |_, _, sent, _| {
            log.lock().expect("log").extend_from_slice(sent);
            (200, Vec::new())
        });
        let refs = MirrorRemote::new("https://mirror.example/r", "", "", &guard)
            .refs()
            .await
            .expect("an empty listing");
        assert!(refs.is_empty());
        assert!(headers.lock().expect("log").is_empty());
    }

    #[tokio::test]
    async fn every_remote_refusal_is_a_conflict_that_never_shows_the_password() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (source, ..) = repository(tmp.path());
        for (status, hint) in [(401, true), (403, true), (500, false)] {
            let guard = guard(move |_, _, _, _| (status, Vec::new()));
            let err = MirrorRemote::new("https://mirror.example/r", "u", PASSWORD, &guard)
                .refs()
                .await
                .expect_err("refused listing");
            assert!(matches!(err, ShakoError::Conflict(_)), "{err}");
            assert_eq!(
                err.to_string().contains("check the mirror's credential"),
                hint,
                "{err}"
            );
        }
        let answers: [(u16, &str, &str); 5] = [
            (500, "", "HTTP 500 to the push"),
            (
                200,
                "unpack error index-pack died s3cr3t-pw",
                "could not unpack the push: error index-pack died [REDACTED]",
            ),
            (
                200,
                "unpack ok\nng refs/heads/main s3cr3t-pw declined",
                "refused the ref: refs/heads/main [REDACTED] declined",
            ),
            (200, "unpack ok", "did not confirm every ref"),
            (
                200,
                "ok refs/heads/main\nok refs/tags/v1",
                "did not confirm every ref",
            ),
        ];
        for (status, report, says) in answers {
            let mut body = Vec::new();
            for line in report.lines() {
                write_text_line(&mut body, line).expect("pkt");
            }
            write_flush(&mut body).expect("flush");
            let guard = guard(move |_, _, _, _| (status, body.clone()));
            let remote = MirrorRemote::new("https://mirror.example/r", "u", PASSWORD, &guard);
            let (sent, spool) = plan(&source, &BTreeMap::new());
            let err = remote.push(&sent, spool).await.expect_err(says);
            assert!(matches!(err, ShakoError::Conflict(_)), "{err}");
            assert!(err.to_string().contains(says), "{says}: {err}");
            assert!(!err.to_string().contains(PASSWORD), "{err}");
        }
    }

    #[tokio::test]
    async fn a_blocked_remote_names_the_guard_without_the_password() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (source, ..) = repository(tmp.path());
        let guard = guard(|_, _, _, _| (200, Vec::new()));
        let remote = MirrorRemote::new("https://s3cr3t-pw.evil.example/r", "u", PASSWORD, &guard);
        let err = remote.refs().await.expect_err("not allowed");
        assert!(matches!(err, ShakoError::SsrfBlocked(_)), "{err}");
        assert!(
            err.to_string().contains("[REDACTED]") && !err.to_string().contains(PASSWORD),
            "{err}"
        );
        let (sent, spool) = plan(&source, &BTreeMap::new());
        let err = remote.push(&sent, spool).await.expect_err("not allowed");
        assert!(matches!(err, ShakoError::SsrfBlocked(_)), "{err}");
        assert!(!err.to_string().contains(PASSWORD), "{err}");
        // Without a password the guard's words stay as they are.
        let open = MirrorRemote::new("http://mirror.example/r", "u", "", &guard);
        let err = open.refs().await.expect_err("plain http");
        assert!(err.to_string().contains("https required"), "{err}");
        // The same guard answers a remote it lets through.
        let allowed = MirrorRemote::new("https://mirror.example/r", "u", PASSWORD, &guard);
        assert!(allowed.refs().await.expect("listed").is_empty());
        // Any other failure becomes a Git error with the secret scrubbed.
        let scrubbed = scrub_secret(&ShakoError::Storage(format!("spool {PASSWORD}")), PASSWORD);
        assert!(
            matches!(&scrubbed, ShakoError::Git(m) if m.contains("[REDACTED]") && !m.contains(PASSWORD))
        );
    }

    #[tokio::test]
    async fn a_push_reports_a_diverged_ref_after_the_refs_that_went() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (source, first, _, change) = repository(tmp.path());
        let guard = guard(|_, _, _, body| {
            let text = String::from_utf8_lossy(body).into_owned();
            let mut report = Vec::new();
            write_text_line(&mut report, "unpack ok").expect("pkt");
            for name in ["refs/heads/main", "refs/tags/v1"] {
                if text.contains(name) {
                    write_text_line(&mut report, &format!("ok {name}")).expect("pkt");
                }
            }
            write_flush(&mut report).expect("flush");
            (200, report)
        });
        let remote = MirrorRemote::new("https://mirror.example/r", "u", PASSWORD, &guard);
        // main sits on a sibling commit: v1 goes, main is reported.
        let held = BTreeMap::from([("refs/heads/main".to_owned(), oid(&change))]);
        let (sent, spool) = plan(&source, &held);
        let err = remote.push(&sent, spool).await.expect_err("diverged");
        assert!(err.to_string().contains("refs/heads/main moved"), "{err}");
        // Nothing to send but the diverged ref: no request, the same report.
        let held = BTreeMap::from([
            ("refs/heads/main".to_owned(), oid(&change)),
            ("refs/tags/v1".to_owned(), oid(&first)),
        ]);
        let (sent, spool) = plan(&source, &held);
        let err = remote.push(&sent, spool).await.expect_err("diverged");
        assert!(
            err.to_string().contains("refs/heads/main, refs/tags/v1"),
            "{err}"
        );
    }

    #[test]
    fn the_remote_listing_reads_refs_and_skips_everything_else() {
        let mut adv = Vec::new();
        let tip = "2222222222222222222222222222222222222222";
        for line in [
            "# service=git-receive-pack".to_owned(),
            format!("{tip} refs/heads/main\0report-status delete-refs"),
            format!("{tip} refs/tags/v1^{{}}"),
            "0000000000000000000000000000000000000000 capabilities^{}".to_owned(),
            "lonely".to_owned(),
            "zz refs/heads/bad".to_owned(),
        ] {
            write_text_line(&mut adv, &line).expect("pkt");
        }
        write_flush(&mut adv).expect("flush");
        let refs = parse_remote_receive_pack_advertisement(&adv).expect("parse");
        assert_eq!(
            refs,
            BTreeMap::from([("refs/heads/main".to_owned(), oid(tip))])
        );
        assert!(parse_remote_receive_pack_advertisement(b"zz").is_err());
    }

    #[test]
    fn tags_naming_trees_and_blobs_go_with_what_they_name_and_held_objects_stay_home() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = repo(tmp.path(), 1, true);
        let root = crate::repo::resolve_commit_oid(&open_isolated(&path).expect("open"), "main")
            .expect("root");
        let tip = put(&path, "main", "a.txt", b"a\n");
        let handle = open_isolated(&path).expect("open");
        let tree = handle
            .find_commit(tip)
            .expect("tip")
            .tree_id()
            .expect("tree")
            .detach();
        let blob = gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, b"a\n")
            .expect("hash");
        let tag = annotated_tag(&path, "ann-tree", tree, gix_object::Kind::Tree);
        set_ref_direct(&path, "refs/tags/blob", &blob.to_string()).expect("blob tag");
        let send = |remote: &BTreeMap<String, ObjectId>| {
            let mut out = Vec::new();
            let plan = write_send_pack(
                &path,
                remote,
                None,
                &mut out,
                &mut crate::testutil::admit_all,
            )
            .expect("plan");
            (names(&plan.commands).len(), pack_count(&out))
        };
        // Everything to an empty remote: the tag, both commits, both trees, both blobs.
        assert_eq!(send(&BTreeMap::new()), (3, Some(7)));
        // The remote holds the tag (so its tree) and the root commit: a.txt and the tip go.
        let held = BTreeMap::from([
            ("refs/tags/ann-tree".to_owned(), tag),
            ("refs/heads/main".to_owned(), root),
            (
                "refs/heads/gone".to_owned(),
                oid("1111111111111111111111111111111111111111"),
            ),
            ("refs/tags/blob-elsewhere".to_owned(), blob),
        ]);
        assert_eq!(send(&held), (2, Some(2)));
    }
}
