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
        let plan = write_send_pack(repo, remote, branch, &mut out, &mut |_| Ok(()))?;
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
}
