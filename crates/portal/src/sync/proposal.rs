//! One reviewable change, written onto a branch and opened as a merge request.
//!
//! Both schedules the reconciler drives end here: the foreign-model mirror (DM-49) and the
//! `SyncSource` loop (MF-28, MF-29). Neither of them writes to the branch the platform
//! applies. `jcctl` decides *what* the change is and this module is the only place that
//! commits it, so there is one answer to "how does the reconciler propose something" rather
//! than one per schedule.
//!
//! A branch is reused rather than recreated: the names both callers pass are deterministic for
//! the revision they carry, so a run that failed halfway through the forge calls continues on
//! the branch it started instead of leaving one behind per attempt (CC-18).

use std::collections::BTreeMap;

use crate::git::{Author, FileDelete, FileWrite, GitError, GiteaClient, MergeStyle, PullRequest};

/// Who the forge records as the author of an automatic proposal.
///
/// Not a person and never a person's token: a reviewer looking at the history has to be able
/// to tell a commit somebody made from a commit a schedule made (MF-30).
pub(crate) const AUTHOR_NAME: &str = "joinedcontext reconciler";
pub(crate) const AUTHOR_EMAIL: &str = "reconciler@joinedcontext.local";

/// What to put on a branch, and what to say about it.
pub struct Proposal<'a> {
    /// Branch name, deterministic for the change it carries.
    pub branch: &'a str,
    /// Merge request title.
    pub title: &'a str,
    /// Merge request body: the plan a reviewer reads.
    pub body: &'a str,
    /// Files to write, by repository-relative path.
    pub files: BTreeMap<String, String>,
    /// Repository paths to remove, when the source no longer carries them (CC-19).
    pub removed: Vec<String>,
    /// Whether the proposal may merge itself once it is open (MF-29, CC-70).
    pub auto_merge: bool,
    /// The branch prefix of this proposal's line, when it has one: another open merge request
    /// of the same line is superseded by this one and closed, so a schedule nobody reviews
    /// leaves one open proposal and not one a day (T-3158).
    pub line: Option<&'a str>,
}

/// Writes the proposal onto its branch and opens the merge request (MF-29, DM-49).
///
/// The merge request is opened even when `auto_merge` is set: the merge is a second step on a
/// request that exists, so what merged is in the forge's history either way.
pub async fn open(gitea: &GiteaClient, proposal: &Proposal<'_>) -> Result<PullRequest, GitError> {
    let default_branch = gitea.default_branch().await?;
    create_or_reuse(gitea, proposal.branch, &default_branch).await?;
    // What is open already: this branch's own merge request is updated, not opened again.
    let open_now = match gitea.list_pull_requests("open").await {
        Ok(pulls) => pulls,
        Err(err) => {
            tracing::warn!(error = %err, "open merge requests not listed; a second one may open");
            Vec::new()
        }
    };

    let author = Author {
        name: AUTHOR_NAME,
        email: AUTHOR_EMAIL,
    };
    for (path, content) in &proposal.files {
        // The sha of what the branch already holds, so a retry updates the file instead of
        // being refused for creating one that exists.
        let existing = gitea.get_file(path, proposal.branch).await.ok().flatten();
        // The same bytes are not written again: a rerun adds no empty commit to the review. A
        // native file an earlier run committed formatted (before F-POR2-017) counts as the same.
        if existing.as_ref().is_some_and(|file| {
            file.content == crate::git::gitea::committed_text(path, content)
                || jc_core::yaml_fmt::format(content).is_ok_and(|text| text == file.content)
        }) {
            continue;
        }
        let existing = existing.map(|file| file.sha);
        gitea
            .put_file(&FileWrite {
                path,
                branch: proposal.branch,
                message: &format!("{}: {path}", proposal.title),
                content,
                sha: existing.as_deref(),
                author,
            })
            .await?;
    }

    for path in &proposal.removed {
        // A path the source dropped and the repository never had is not an error: the run
        // that dropped it may have got this far before and failed on a later file.
        let Some(existing) = gitea.get_file(path, proposal.branch).await.ok().flatten() else {
            continue;
        };
        gitea
            .delete_file(&FileDelete {
                path,
                branch: proposal.branch,
                message: &format!("{}: remove {path}", proposal.title),
                sha: &existing.sha,
                author,
            })
            .await?;
    }

    let pull = match open_now
        .iter()
        .find(|pull| pull.head_branch == proposal.branch)
    {
        Some(pull) => pull.clone(),
        None => {
            gitea
                .create_pull_request(
                    proposal.branch,
                    &default_branch,
                    proposal.title,
                    proposal.body,
                )
                .await?
        }
    };

    if let Some(line) = proposal.line {
        for older in superseded(&open_now, line, proposal.branch) {
            match gitea.close_pull_request(older.number).await {
                Ok(()) => {
                    tracing::info!(number = older.number, by = %proposal.branch, "closed a merge request this proposal supersedes")
                }
                Err(err) => {
                    tracing::warn!(number = older.number, error = %err, "a superseded merge request stayed open")
                }
            }
        }
    }

    if proposal.auto_merge {
        gitea
            .merge(pull.number, MergeStyle::Merge, proposal.title, None)
            .await?;
    }
    Ok(pull)
}

/// The branch, whether or not a previous attempt already created it.
async fn create_or_reuse(
    gitea: &GiteaClient,
    branch: &str,
    default_branch: &str,
) -> Result<(), GitError> {
    match gitea.create_branch(branch, default_branch).await {
        Ok(()) => Ok(()),
        Err(GitError::Conflict(_)) => {
            tracing::info!(branch = %branch, "continuing on the branch an earlier run started");
            Ok(())
        }
        Err(other) => Err(other),
    }
}

/// The open merge requests of `line` on another branch than `branch`: what a new proposal of the
/// line replaces. A line's branches are `{line}` itself and `{line}-{8 hex digits}`, the name a
/// digest gave each day's proposal before T-3158; another line that merely starts the same way
/// (`mirror/bb-peer-2` beside `mirror/bb-peer`) is not one of them.
pub(crate) fn superseded<'p>(
    open: &'p [PullRequest],
    line: &str,
    branch: &str,
) -> Vec<&'p PullRequest> {
    let of_line = |head: &str| {
        head == line
            || head
                .strip_prefix(line)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|digest| {
                    digest.len() == 8 && digest.chars().all(|c| c.is_ascii_hexdigit())
                })
    };
    open.iter()
        .filter(|pull| pull.head_branch != branch && of_line(&pull.head_branch))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(number: u64, head: &str) -> PullRequest {
        PullRequest {
            number,
            url: String::new(),
            state: "open".into(),
            title: String::new(),
            body: String::new(),
            head_branch: head.into(),
            head_sha: String::new(),
            base_branch: "main".into(),
            base_sha: String::new(),
            created_at: String::new(),
            author_name: String::new(),
            author_email: None,
            mergeable: None,
            merged: false,
            repository: String::new(),
            ..Default::default()
        }
    }

    /// A forge where `mirror/bb-peer` already has merge request 7 open beside two of the
    /// digest-named days before it, and the branch already holds `files`.
    async fn forge(files: &[(&str, &str)]) -> (wiremock::MockServer, GiteaClient) {
        use base64::Engine;
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, ResponseTemplate};
        let server = wiremock::MockServer::start().await;
        let repo = "/api/v1/repos/o/r";
        Mock::given(method("GET"))
            .and(path(repo))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/branches")))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_json(serde_json::json!({ "message": "exists" })),
            )
            .mount(&server)
            .await;
        let pr = |number: u64, head: &str| {
            serde_json::json!({ "number": number, "html_url": format!("https://forge/pulls/{number}"), "state": "open",
                "title": "mirror", "head": { "ref": head, "sha": "abc" }, "base": { "ref": "main" } })
        };
        Mock::given(method("GET"))
            .and(path(format!("{repo}/pulls")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                pr(7, "mirror/bb-peer"),
                pr(5, "mirror/bb-peer-0a1b2c3d"),
                pr(4, "mirror/other"),
            ])))
            .mount(&server)
            .await;
        for (name, content) in files {
            Mock::given(method("GET"))
                .and(path(format!("{repo}/contents/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "s1",
                    "content": base64::engine::general_purpose::STANDARD.encode(content),
                })))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path_regex(format!("^{repo}/contents/.*$")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path_regex(format!("^{repo}/contents/.*$")))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "commit": { "sha": "c2" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path_regex(format!("^{repo}/pulls/[0-9]+$")))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let gitea =
            GiteaClient::new(server.uri().parse().expect("url"), "o", "r", "t").expect("client");
        (server, gitea)
    }

    async fn calls(server: &wiremock::MockServer, verb: &str) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.method.as_str() == verb)
            .map(|request| request.url.path().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn a_proposal_updates_its_open_merge_request_and_closes_the_days_before_it() {
        // What an earlier run committed: the text in the commit layout (T-5052).
        let (server, gitea) = forge(&[("models/air.yaml", "same\n")]).await;
        let mut files = BTreeMap::new();
        files.insert("models/air.yaml".to_owned(), "same".to_owned());
        files.insert("models/air/schema.linkml.yaml".to_owned(), "new".to_owned());
        let pull = open(
            &gitea,
            &Proposal {
                branch: "mirror/bb-peer",
                title: "mirror 2 foreign model(s) for bb/peer",
                body: "{}",
                files,
                removed: Vec::new(),
                auto_merge: false,
                line: Some("mirror/bb-peer"),
            },
        )
        .await
        .expect("proposed");
        assert_eq!(
            pull.number, 7,
            "the open merge request of the branch is the proposal"
        );
        assert!(
            calls(&server, "POST")
                .await
                .iter()
                .all(|p| !p.ends_with("/pulls")),
            "no second merge request"
        );
        assert_eq!(
            calls(&server, "PUT").await,
            ["/api/v1/repos/o/r/contents/models/air/schema.linkml.yaml"],
            "the unchanged file is not written again"
        );
        assert_eq!(
            calls(&server, "PATCH").await,
            ["/api/v1/repos/o/r/pulls/5"],
            "only the line's older day is closed"
        );
    }

    #[test]
    fn a_proposal_supersedes_its_own_lines_older_branches_and_no_other_line() {
        let open = [
            pull(1, "mirror/bb-peer-0a1b2c3d"),
            pull(2, "mirror/bb-peer-ffffffff"),
            pull(3, "mirror/bb-peer"),
            pull(4, "mirror/bb-peer-2"),
            pull(5, "mirror/bb-peer-2-0a1b2c3d"),
            pull(6, "sync/helsinki-blueprints-abc"),
        ];
        let closed: Vec<u64> = superseded(&open, "mirror/bb-peer", "mirror/bb-peer")
            .iter()
            .map(|pull| pull.number)
            .collect();
        assert_eq!(closed, [1, 2]);
    }
}
