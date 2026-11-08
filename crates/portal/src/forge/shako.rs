//! The forge client: shako's OData 4.01 service (CC-03, CC-32, CC-34, CC-41, CC-42, CC-44).
//!
//! Every repository operation is an OData request to `{JC_SHAKO_URL}/odata/`: a repository is
//! addressed by its alternate key `Repositories(Owner='o',Name='r')`, what lives inside it by
//! navigation under that key, and every write by a bound or unbound action
//! (`crates/shako/PORTAL-ODATA.md` maps each method to its request). Pack data stays on git smart
//! HTTP and images on the OCI distribution API: OData carries metadata and operations only.
//!
//! Mutations in joinedcontext commit to Git merge requests server-side. Commits are attributed
//! to the signed-in human author, while the service token only authenticates the HTTP call.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::error::ApiError;

/// Pages of a recursive tree read before giving up: 100,000 files, far above any project.
const MAX_TREE_PAGES: usize = 100;
/// Entries of one tree page.
const TREE_PAGE: usize = 1000;
/// Tags of one page.
const TAG_PAGE: usize = 50;

/// One repository of the forge, and the token the Portal writes it with.
#[derive(Clone)]
pub struct ShakoClient {
    pub base: Url,
    /// Where a browser reaches the same forge: the API talks to the address the Portal dials,
    /// but a "Source" link opens in the user's browser, which resolves no `.svc.cluster.local`
    /// name. `JC_SHAKO_PUBLIC_URL`; the API base when unset.
    pub public_base: Url,
    pub owner: String,
    pub repo: String,
    /// `projects/{slug}/` for a project repository of layout 2 (CC-87): the render path its
    /// root sits at. Every path this client reads comes back under it and every path it writes
    /// is taken out of it, so a caller speaks render paths whichever repository it talks to.
    mount: Option<String>,
    token: String,
    /// The organization the generated applications' repositories live in and the token that
    /// writes them: another organization and another machine user than the configuration's
    /// (PF-106). `None`: beside the configuration.
    apps: Option<(String, String)>,
    pub http: reqwest::Client,
}

impl std::fmt::Debug for ShakoClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShakoClient")
            .field("base", &self.base.as_str())
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("mount", &self.mount)
            .field("token", &"[redacted]")
            .field("apps", &self.apps.as_ref().map(|(owner, _)| owner))
            .finish()
    }
}

/// Errors returned by the forge client.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("git config error: {0}")]
    Config(String),
    #[error("git transport error: {0}")]
    Transport(String),
    #[error("git resource not found")]
    NotFound,
    #[error("git conflict: {0}")]
    Conflict(String),
    #[error("git api error {status}: {message}")]
    Api { status: u16, message: String },
}

impl From<GitError> for ApiError {
    fn from(err: GitError) -> Self {
        match err {
            GitError::NotFound => {
                ApiError::NotFound("resource not found in git repository".to_string())
            }
            GitError::Conflict(msg) => ApiError::Conflict(msg),
            // The forge down, slow or refusing the Portal's own token is an outage the person
            // can wait out, not a fault of their request (T-4966: a forge 401 emptied the inbox).
            GitError::Transport(_)
            | GitError::Api {
                status: 401 | 403 | 408 | 429 | 500..=599,
                ..
            } => {
                tracing::warn!(error = %err, "git forge unavailable");
                ApiError::Unavailable(
                    "The Git repository did not answer the Portal. Try again in a minute; if it \
                     lasts, ask the operator to check the forge and the Portal's forge token."
                        .to_string(),
                )
            }
            other => {
                tracing::error!(error = %other, "git error");
                ApiError::Internal("git operation failed".to_string())
            }
        }
    }
}

/// The sentence of a forge error answer: shako answers the OData error object
/// `{"error": {"code", "message"}}`, and only the message reaches a person, never the raw body
/// (T-4967). Any other body keeps its first line, capped, since a proxy in front of the forge may
/// answer a whole page.
async fn forge_message(res: reqwest::Response) -> String {
    let body = res.text().await.unwrap_or_default();
    if let Ok(doc) = serde_json::from_str::<Value>(&body) {
        let message = doc
            .pointer("/error/message")
            .or_else(|| doc.get("message"))
            .and_then(Value::as_str);
        if let Some(message) = message {
            return message.trim().to_string();
        }
    }
    body.lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(200)
        .collect()
}

/// An OData string literal: quoted, with a quote inside doubled.
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A workflow run of an application's repository, as the App page links it (AP-86, AP-103).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    /// `queued`, `in_progress`, `waiting` or `completed`, as the forge says it.
    pub status: String,
    /// `success`, `failure`, `cancelled`… once the run is completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    /// The commit the run built.
    pub commit: String,
    /// The run's page, behind the forge's sign-in (PF-81).
    pub url: String,
    /// The run's number in its repository, so a client tells the run it started from the one
    /// before it (T-3245).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// RFC 3339, once a runner took it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// RFC 3339, once it is completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

impl WorkflowRun {
    /// How long a completed run took, in seconds.
    pub fn seconds(&self) -> Option<u64> {
        let at = |text: &Option<String>| {
            text.as_deref()
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        };
        let took = at(&self.completed_at)? - at(&self.started_at)?;
        u64::try_from(took.num_seconds()).ok()
    }
}

/// A forge job waiting for a runner, as the build-pod dispatcher reads it (AP-130).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedJob {
    /// The forge's id of the job, unique in the installation.
    pub id: u64,
    /// The `runs-on` labels the job asks for.
    pub labels: Vec<String>,
    /// The repository of the organization the job belongs to.
    pub repository: String,
}

/// An OData collection answer.
#[derive(Deserialize)]
struct Collection<T> {
    #[serde(default = "Vec::new")]
    value: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JobRow {
    id: u64,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    repository: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WorkflowRunRow {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
}

/// An Actions artifact of a repository and the commit its run built (AP-104).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub id: u64,
    pub size: u64,
    /// The workflow run that uploaded it.
    pub run: u64,
    pub commit: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ArtifactRow {
    id: u64,
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    run_id: u64,
    #[serde(default)]
    head_sha: String,
}

/// A body read to its end, refused once it passes `limit` bytes rather than held in memory.
async fn read_capped(
    mut res: reqwest::Response,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>, GitError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| GitError::Transport(e.to_string()))?
    {
        if (bytes.len() + chunk.len()) as u64 > limit {
            return Err(GitError::Transport(format!(
                "{what} is larger than {limit} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Human author attributed on commits (CC-44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author<'a> {
    #[serde(rename = "Name")]
    pub name: &'a str,
    #[serde(rename = "Email")]
    pub email: &'a str,
}

/// File content read from repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFile {
    pub sha: String,
    pub content: String,
}

/// What `git-receive-pack` reported with `report-status`: `unpack ok`, then `ok {ref}` or
/// `ng {ref} {reason}` per ref, in pkt-lines. Answers the refused refs.
fn receive_pack_status(answer: &[u8]) -> Result<Vec<String>, GitError> {
    let mut rest = answer;
    let mut unpacked = false;
    let mut refused = Vec::new();
    while rest.len() >= 4 {
        let size = std::str::from_utf8(&rest[..4])
            .ok()
            .and_then(|hex| usize::from_str_radix(hex, 16).ok())
            .ok_or_else(|| GitError::Transport("the push answer is not pkt-lines".into()))?;
        if size == 0 {
            rest = &rest[4..];
            continue;
        }
        if size < 4 || size > rest.len() {
            return Err(GitError::Transport("the push answer is cut short".into()));
        }
        let text = String::from_utf8_lossy(&rest[4..size]);
        let text = text.trim_end();
        if text == "unpack ok" {
            unpacked = true;
        } else if let Some(reason) = text.strip_prefix("unpack ") {
            return Err(GitError::Conflict(format!(
                "the forge did not unpack the bundle: {reason}"
            )));
        } else if let Some(ng) = text.strip_prefix("ng ") {
            refused.push(ng.to_owned());
        }
        rest = &rest[size..];
    }
    if !unpacked {
        return Err(GitError::Transport(
            "the push answer reported no unpack".into(),
        ));
    }
    Ok(refused)
}

/// One file a pull request changes; `deleted` when it is gone from the head branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub deleted: bool,
    /// Whether the merge request adds the file, as the forge's own diff status says: the
    /// difference between creating a resource and changing one, without a second read of the
    /// base branch (T-0861).
    pub added: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ChangedFileRow {
    filename: String,
    #[serde(default)]
    status: String,
}

/// File write specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite<'a> {
    pub path: &'a str,
    pub branch: &'a str,
    pub message: &'a str,
    pub content: &'a str,
    pub sha: Option<&'a str>,
    pub author: Author<'a>,
}

/// The text a write commits at `path`: a manifest (YAML with `apiVersion` and `kind`) in the one
/// layout `jcctl fmt` writes (T-5052), so a Portal change and a hand edit look the same in git;
/// any other file, a native YAML file (a Bento config, a LinkML model) included, byte for byte
/// with its comments (MF-42, F-POR2-017). YAML the formatter refuses is committed as given: the
/// layout never blocks a write. A caller that skips a write when the branch already holds its
/// text compares with this.
pub fn committed_text<'a>(path: &str, content: &'a str) -> std::borrow::Cow<'a, str> {
    let yaml = path.ends_with(".yaml") || path.ends_with(".yml");
    let manifest = || {
        serde_norway::from_str::<serde_json::Value>(content).is_ok_and(|doc| {
            doc.get("apiVersion")
                .is_some_and(serde_json::Value::is_string)
                && doc.get("kind").is_some_and(serde_json::Value::is_string)
        })
    };
    match (yaml && manifest())
        .then(|| jc_core::yaml_fmt::format(content).ok())
        .flatten()
    {
        Some(text) => std::borrow::Cow::Owned(text),
        None => std::borrow::Cow::Borrowed(content),
    }
}

/// File deletion specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDelete<'a> {
    pub path: &'a str,
    pub branch: &'a str,
    pub message: &'a str,
    pub sha: &'a str,
    pub author: Author<'a>,
}

/// Pull request representation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: String,
    pub title: String,
    pub body: String,
    pub head_branch: String,
    /// The commit the head branch pointed at when the forge answered. A review reads every file
    /// at this ref and the merge is pinned to it, so a push between the two is refused instead
    /// of merged unread (PF-57, T-1683).
    pub head_sha: String,
    pub base_branch: String,
    /// The commit the base branch pointed at when the forge answered, empty when it did not say.
    /// With `head_sha` it names everything the list reads of an open change (T-6347).
    #[serde(default)]
    pub base_sha: String,
    pub created_at: String,
    pub author_name: String,
    pub author_email: Option<String>,
    /// The forge account that opened the pull request: who pushed, not what a commit claims.
    #[serde(default)]
    pub author_login: String,
    /// Whether the head is the base's own repository: not a fork. shako has no forks, so a pull
    /// request it answers is always its repository's own (T-3433).
    #[serde(default)]
    pub same_repository: bool,
    pub mergeable: Option<bool>,
    pub merged: bool,
    /// The repository the pull request is in, as the client that read it names it (CC-87).
    pub repository: String,
    /// The commit a merge made, whose message names who approved it; empty while open.
    #[serde(default)]
    pub merge_commit_sha: String,
    /// When it was merged or closed, RFC 3339; empty while open.
    #[serde(default)]
    pub closed_at: String,
    /// The commit the head branch was cut from: what a removal removed is still there (T-3247).
    #[serde(default)]
    pub merge_base: String,
}

impl PullRequest {
    /// The ref a review reads every file at: the commit the forge reported, or the branch when
    /// it reported none. Reading the branch and merging the branch is two reads of a moving
    /// target; reading this and merging with `head_commit_id` is one (PF-57, T-1683).
    pub fn head_ref(&self) -> &str {
        if self.head_sha.is_empty() {
            &self.head_branch
        } else {
            &self.head_sha
        }
    }

    /// The ref the "before" of a change is read at: the base branch while the change is open,
    /// the commit it was cut from once it is closed. After a merge the base branch already holds
    /// what the change made, so reading it there showed a merged change as changing nothing
    /// (T-3274).
    pub fn base_ref(&self) -> &str {
        if self.state == "closed" && !self.merge_base.is_empty() {
            &self.merge_base
        } else {
            &self.base_branch
        }
    }
}

/// One commit of the repository, as the revision picker shows it (MF-16, CC-49).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    /// First line of the commit message: the picker renders history as plain sentences.
    pub message: String,
    pub author: String,
    pub email: Option<String>,
    pub date: String,
}

/// Pull request review action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewEvent {
    #[serde(rename = "APPROVED")]
    Approve,
    #[serde(rename = "REQUEST_CHANGES")]
    RequestChanges,
    #[serde(rename = "COMMENT")]
    Comment,
}

/// Pull request merge style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeStyle {
    Merge,
    Squash,
    Rebase,
}

/// A pull request as the `PullRequests` entity set answers it.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PullRequestRow {
    number: u64,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    head: String,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    base_sha: String,
    #[serde(default)]
    merge_base: Option<String>,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    merge_commit_sha: Option<String>,
    #[serde(default)]
    author: String,
    #[serde(default)]
    author_name: Option<String>,
    #[serde(default)]
    author_email: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
}

impl From<PullRequestRow> for PullRequest {
    fn from(row: PullRequestRow) -> Self {
        let author_name = row
            .author_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| row.author.clone());
        PullRequest {
            number: row.number,
            url: row.html_url,
            state: row.state,
            title: row.title,
            body: row.body.unwrap_or_default(),
            head_branch: row.head,
            head_sha: row.head_sha,
            base_branch: row.base,
            base_sha: row.base_sha,
            created_at: row.created_at.unwrap_or_default(),
            author_name,
            author_email: row.author_email.filter(|email| !email.is_empty()),
            author_login: row.author,
            same_repository: true,
            mergeable: row.mergeable,
            merged: row.merged,
            repository: String::new(),
            merge_commit_sha: row.merge_commit_sha.unwrap_or_default(),
            closed_at: row.closed_at.unwrap_or_default(),
            merge_base: row.merge_base.unwrap_or_default(),
        }
    }
}

/// One push mirror of a repository, as `R/PushMirrors` lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct PushMirror {
    #[serde(rename = "Name")]
    pub remote_name: String,
    #[serde(rename = "RemoteAddress")]
    pub remote_address: String,
    /// What the last sync failed with; empty when it succeeded or has not run yet.
    #[serde(rename = "LastError", default, deserialize_with = "null_is_empty")]
    pub last_error: String,
}

fn null_is_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TreePage {
    #[serde(default)]
    sha: String,
    #[serde(default)]
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TreeEntry {
    path: String,
    #[serde(rename = "Type")]
    entry_type: String,
    #[serde(default)]
    sha: String,
}

/// One file change of a `Shako.CommitFiles` call.
#[derive(Serialize)]
struct FileChange {
    operation: &'static str,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<String>,
}

impl ShakoClient {
    pub fn new(
        base: Url,
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, GitError> {
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| GitError::Transport(e.to_string()))?;

        Ok(Self {
            public_base: base.clone(),
            base,
            owner: owner.into(),
            repo: repo.into(),
            mount: None,
            token: token.into(),
            apps: None,
            http,
        })
    }

    /// Reads the forge's configuration from the environment.
    ///
    /// `JC_SHAKO_URL` (the base the Portal dials; the OData service is `{base}/odata/`),
    /// `JC_SHAKO_OWNER`, `JC_SHAKO_REPO` and `JC_SHAKO_TOKEN` (a secret: the token every push and
    /// merge request is written with). `JC_SHAKO_PUBLIC_URL` is the address a browser follows a
    /// "Source" link to, which differs from the API base whenever the forge is reached through
    /// the edge; the API base when it is unset.
    ///
    /// `JC_SHAKO_APPS_TOKEN` (a secret, the applications' machine user's) and
    /// `JC_SHAKO_APPS_OWNER` move the generated applications' repositories, their packages and
    /// their build queue into an organization of their own, written by a machine user of their
    /// own (PF-106); both or neither.
    ///
    /// Fail-closed: returns `Ok(None)` if all four are absent, or an error if partially set.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, GitError> {
        let apps = match (lookup("JC_SHAKO_APPS_OWNER"), lookup("JC_SHAKO_APPS_TOKEN")) {
            (None, None) => None,
            (Some(owner), Some(token)) if !owner.trim().is_empty() && !token.trim().is_empty() => {
                Some((owner.trim().to_owned(), token.trim().to_owned()))
            }
            _ => {
                return Err(GitError::Config(
                    "JC_SHAKO_APPS_OWNER and JC_SHAKO_APPS_TOKEN must be set together".to_string(),
                ))
            }
        };
        match (
            lookup("JC_SHAKO_URL"),
            lookup("JC_SHAKO_OWNER"),
            lookup("JC_SHAKO_REPO"),
            lookup("JC_SHAKO_TOKEN"),
        ) {
            (None, None, None, None) => Ok(None),
            (Some(url), Some(owner), Some(repo), Some(token)) => {
                let base = Url::parse(&url)
                    .map_err(|e| GitError::Config(format!("invalid JC_SHAKO_URL: {e}")))?;
                let mut client = Self::new(base, owner, repo, token)?;
                client.apps = apps;
                if let Some(public) = lookup("JC_SHAKO_PUBLIC_URL") {
                    client.public_base = Url::parse(&public)
                        .map_err(|e| GitError::Config(format!("invalid JC_SHAKO_PUBLIC_URL: {e}")))?;
                }
                Ok(Some(client))
            }
            _ => Err(GitError::Config(
                "JC_SHAKO_URL, JC_SHAKO_OWNER, JC_SHAKO_REPO and JC_SHAKO_TOKEN must be set together"
                    .to_string(),
            )),
        }
    }

    /// Browser URL of one file at a git ref, the page a "Source" link opens (never the API URL).
    pub fn browse_url(&self, path: &str, git_ref: &str) -> String {
        self.signed_in(&format!(
            "{}/{}/{}/src/branch/{}/{}",
            self.public_base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo,
            git_ref,
            self.local_path(path)
                .unwrap_or_else(|| path.trim_start_matches('/').to_owned()),
        ))
    }

    /// Browser URL of a pull request, on the public address a browser resolves.
    pub fn pull_url(&self, number: u64) -> String {
        self.signed_in(&format!(
            "{}/{}/{}/pulls/{number}",
            self.public_base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo,
        ))
    }

    /// A forge page behind the forge's own sign-in (PF-81). A Portal session is not a forge
    /// session, and the configuration repository is private, so a link straight to the file
    /// answers 404. The link starts the forge's Keycloak sign-in instead of showing a sign-in page
    /// (T-2759): the Keycloak session the Portal holds answers without a prompt, and the forge
    /// returns to `redirect_to`, which it takes only as a local path, so the host is dropped here.
    fn signed_in(&self, url: &str) -> String {
        let Ok(target) = Url::parse(url) else {
            return url.to_owned();
        };
        let mut login = self.public_base.clone();
        // `join` resolves against the last path segment, so a base without the trailing slash
        // would put /user/oauth2 beside the prefix instead of inside it.
        if !login.path().ends_with('/') {
            login.set_path(&format!("{}/", login.path()));
        }
        let Ok(mut login) = login.join("user/oauth2/keycloak") else {
            return url.to_owned();
        };
        login
            .query_pairs_mut()
            .append_pair("redirect_to", target.path());
        login.into()
    }

    /// A pull request as the Portal hands it on: with a public forge configured, its link is
    /// the public one behind the sign-in; without, the forge's own `HtmlUrl`.
    fn pull(&self, row: PullRequestRow) -> PullRequest {
        let mut pull = PullRequest::from(row);
        pull.repository = self.repo.clone();
        if self.public_base != self.base {
            pull.url = self.pull_url(pull.number);
        }
        pull
    }

    /// `{base}/odata/{segments…}`, each segment percent-encoded on its own: a `/` inside a key
    /// value (`Branches('workspace/air')`) stays inside its segment, so nothing a person typed
    /// reaches another route.
    fn odata_url(&self, segments: &[&str]) -> Result<Url, GitError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| GitError::Config(format!("'{}' cannot be a base", self.base)))?
            .pop_if_empty()
            .push("odata")
            .extend(segments);
        Ok(url)
    }

    /// The repository's alternate key, `Repositories(Owner='o',Name='r')`.
    fn repo_key(&self) -> String {
        format!(
            "Repositories(Owner={},Name={})",
            literal(&self.owner),
            literal(&self.repo)
        )
    }

    /// `R/{rest…}`: something under this repository.
    fn repo_url(&self, rest: &[&str]) -> Result<Url, GitError> {
        let key = self.repo_key();
        let mut segments = vec![key.as_str()];
        segments.extend_from_slice(rest);
        self.odata_url(&segments)
    }

    /// `R/PullRequests({number})/{rest…}`.
    fn pull_url_api(&self, number: u64, rest: &[&str]) -> Result<Url, GitError> {
        let pull = format!("PullRequests({number})");
        let mut segments = vec![pull.as_str()];
        segments.extend_from_slice(rest);
        self.repo_url(&segments)
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<reqwest::Response, GitError> {
        crate::request_id::forward(builder)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| GitError::Transport(e.to_string()))
    }

    async fn check_status(res: reqwest::Response) -> Result<reqwest::Response, GitError> {
        let status = res.status();
        if status.is_success() {
            Ok(res)
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Err(GitError::NotFound)
        } else if status == reqwest::StatusCode::CONFLICT {
            Err(GitError::Conflict(forge_message(res).await))
        } else {
            let status_code = status.as_u16();
            let message = forge_message(res).await;
            Err(GitError::Api {
                status: status_code,
                message,
            })
        }
    }

    /// `GET url`, its JSON answer.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: Url,
        what: &str,
    ) -> Result<T, GitError> {
        let res = Self::check_status(self.send(self.http.get(url)).await?).await?;
        res.json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse {what}: {e}")))
    }

    /// `POST url` with the action's parameters.
    async fn act(&self, url: Url, params: &Value) -> Result<reqwest::Response, GitError> {
        Self::check_status(self.send(self.http.post(url).json(params)).await?).await
    }

    /// The same forge, organization and token, another repository of the organization: an
    /// application's own repository beside the configuration one (AP-75).
    pub fn for_repository(&self, repo: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            mount: None,
            ..self.clone()
        }
    }

    /// A generated application's own repository `repo` (AP-75): in the applications'
    /// organization, with their token, when the installation keeps one (PF-106); beside the
    /// configuration otherwise.
    pub fn for_application(&self, repo: impl Into<String>) -> Self {
        let mut client = self.for_repository(repo);
        if let Some((owner, token)) = &self.apps {
            client.owner.clone_from(owner);
            client.token.clone_from(token);
        }
        client
    }

    /// What the applications' organization owns beside their repositories: the packages their
    /// builds are published as and the queue of their builds (AP-101, ADR-N-028).
    pub fn applications(&self) -> Self {
        self.for_application(self.repo.clone())
    }

    /// The project repository `repo` of layout 2, which holds what the render has under
    /// `projects/{slug}/` at its root (CC-85, CC-87).
    pub fn for_project(&self, repo: impl Into<String>, slug: &str) -> Self {
        Self {
            repo: repo.into(),
            mount: Some(format!("projects/{slug}/")),
            ..self.clone()
        }
    }

    /// Whether this is a project repository of layout 2.
    pub fn is_project_repository(&self) -> bool {
        self.mount.is_some()
    }

    /// A render path as this repository spells it, or `None` when it is not one of its files.
    fn local_path(&self, path: &str) -> Option<String> {
        let clean = path.trim_start_matches('/');
        match &self.mount {
            None => Some(clean.to_owned()),
            Some(mount) => clean
                .strip_prefix(mount.as_str())
                .filter(|rest| !rest.is_empty())
                .map(str::to_owned),
        }
    }

    /// [`Self::local_path`] for a write: a path outside the project repository is refused,
    /// so nothing meant for the organization repository lands in a project's (CC-87).
    fn write_path(&self, path: &str) -> Result<String, GitError> {
        let local = self.local_path(path).ok_or_else(|| {
            GitError::Conflict(format!(
                "{} is not a file of the project repository {}; the organization's files are \
                 changed in the organization repository",
                path.trim_start_matches('/'),
                self.repo
            ))
        })?;
        file_path(local)
    }

    /// A path of this repository as the render spells it.
    fn render_path(&self, path: String) -> String {
        match &self.mount {
            None => path,
            Some(mount) => format!("{mount}{path}"),
        }
    }

    /// The `https` address a person clones the repository from: the forge's public URL, never
    /// the internal one (AP-77, AP-78).
    pub fn clone_url(&self) -> String {
        format!(
            "{}/{}/{}.git",
            self.public_base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo
        )
    }

    /// Whether the repository is there: `GET R`, a 404 is `false`.
    async fn exists(&self) -> Result<bool, GitError> {
        let res = self.send(self.http.get(self.repo_url(&[])?)).await?;
        match Self::check_status(res).await {
            Ok(_) => Ok(true),
            Err(GitError::NotFound) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// `POST CreateRepository`, a 409 (it is there already, a lost race) answered `false`.
    async fn create(&self, description: &str, auto_init: bool) -> Result<bool, GitError> {
        if self.exists().await? {
            return Ok(false);
        }
        let params = json!({
            "Owner": self.owner,
            "Name": self.repo,
            "Description": description,
            "Private": true,
            "AutoInit": auto_init,
            "DefaultBranch": "main",
        });
        match self
            .act(self.odata_url(&["CreateRepository"])?, &params)
            .await
        {
            Ok(_) => Ok(true),
            Err(GitError::Conflict(_)) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Creates the repository in the organization unless it is already there, private and
    /// initialised on `main` so a branch can be cut from it; answers whether it created one.
    ///
    /// An existing repository is never recreated or overwritten, and a creation that lost a race
    /// with another (the forge's 409) is the same answer as finding it (AP-75).
    pub async fn ensure_repository(&self, description: &str) -> Result<bool, GitError> {
        self.create(description, true).await
    }

    /// Creates this repository as a copy of `origin` with its whole history (PF-89): private,
    /// with no pull request of the origin's. Answers `false`, and copies nothing, when a
    /// repository of this name is already there.
    pub async fn migrate_repository(&self, origin: &ShakoClient) -> Result<bool, GitError> {
        if self.exists().await? {
            return Ok(false);
        }
        let params = json!({
            "SourceOwner": origin.owner,
            "SourceName": origin.repo,
            "RepoOwner": self.owner,
            "RepoName": self.repo,
            "Private": true,
        });
        match self
            .act(self.odata_url(&["ImportRepository"])?, &params)
            .await
        {
            Ok(_) => Ok(true),
            Err(GitError::Conflict(_)) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Creates the repository in the organization, private and empty, for a push to fill (MF-45);
    /// answers `false`, and creates nothing, when a repository of this name is already there.
    pub async fn create_empty_repository(&self, description: &str) -> Result<bool, GitError> {
        self.create(description, false).await
    }

    /// Pushes the pack of a `git bundle` into this repository and sets `refs` to the commits
    /// given, `(commit, refname)`, with one `git-receive-pack` request (MF-45): the Portal
    /// holds no git, and a bundle is a header and the very pack a push sends. Every ref is
    /// created, so the repository is empty or the forge refuses it. A pack the forge does not
    /// unpack is the error; each ref it refuses on its own (`ng`, a tag whose commit the pack
    /// does not hold) is answered as `ref reason`, and the others are set.
    pub async fn push_bundle(
        &self,
        bundle: &[u8],
        refs: &[(String, String)],
    ) -> Result<Vec<String>, GitError> {
        const ZERO: &str = "0000000000000000000000000000000000000000";
        let pack = bundle
            .windows(2)
            .position(|pair| pair == b"\n\n")
            .map(|end| &bundle[end + 2..])
            .filter(|pack| pack.starts_with(b"PACK"))
            .ok_or_else(|| GitError::Conflict("the bundle carries no pack".into()))?;
        let Some(((first_commit, first_ref), rest)) = refs.split_first() else {
            return Err(GitError::Conflict("a push sets at least one ref".into()));
        };
        let line = |text: String| format!("{:04x}{text}", text.len() + 4);
        let mut body = line(format!(
            "{ZERO} {first_commit} {first_ref}\0 report-status\n"
        ))
        .into_bytes();
        for (commit, name) in rest {
            body.extend_from_slice(line(format!("{ZERO} {commit} {name}\n")).as_bytes());
        }
        body.extend_from_slice(b"0000");
        body.extend_from_slice(pack);

        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| GitError::Config(format!("'{}' cannot be a base", self.base)))?
            .pop_if_empty()
            .push(&self.owner)
            .push(&format!("{}.git", self.repo))
            .push("git-receive-pack");
        let res = self
            .send(
                self.http
                    .post(url)
                    .header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-git-receive-pack-request",
                    )
                    .body(body),
            )
            .await?;
        let res = Self::check_status(res).await?;
        let answer = res
            .bytes()
            .await
            .map_err(|e| GitError::Transport(format!("failed to read the push answer: {e}")))?;
        receive_pack_status(&answer)
    }

    /// `GET R/PushMirrors`: where each mirror pushes, and what its last sync said (AP-79).
    /// Answers only for a caller that administers the repository.
    pub async fn push_mirrors(&self) -> Result<Vec<PushMirror>, GitError> {
        let listed: Collection<PushMirror> = self
            .get_json(self.repo_url(&["PushMirrors"])?, "push mirrors")
            .await?;
        Ok(listed.value)
    }

    /// `POST R/Shako.AddPushMirror`: a push mirror that syncs on every commit, with a periodic
    /// sync behind it for a commit whose sync failed (AP-79).
    ///
    /// The credential travels in its own member and never inside `remote_address`: the forge
    /// shows the address in `PushMirrors`, keeps the credential in its secret store and never
    /// answers it.
    pub async fn add_push_mirror(
        &self,
        remote_address: &str,
        username: &str,
        password: &str,
    ) -> Result<(), GitError> {
        let params = json!({
            "RemoteAddress": remote_address,
            "RemoteUsername": username,
            "Credential": password,
            "Interval": "8h0m0s",
            "SyncOnCommit": true,
        });
        self.act(self.repo_url(&["Shako.AddPushMirror"])?, &params)
            .await?;
        Ok(())
    }

    /// `POST R/Shako.RemovePushMirror`: one push mirror, by the name the forge gave it.
    pub async fn delete_push_mirror(&self, remote_name: &str) -> Result<(), GitError> {
        self.act(
            self.repo_url(&["Shako.RemovePushMirror"])?,
            &json!({ "Name": remote_name }),
        )
        .await?;
        Ok(())
    }

    /// `POST R/Shako.SyncPushMirrors`: every mirror pushes now instead of at its next commit or
    /// interval.
    pub async fn sync_push_mirrors(&self) -> Result<(), GitError> {
        self.act(self.repo_url(&["Shako.SyncPushMirrors"])?, &json!({}))
            .await?;
        Ok(())
    }

    /// `GET R`: the repository's default branch.
    pub async fn default_branch(&self) -> Result<String, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Repository {
            default_branch: String,
        }
        let repo: Repository = self.get_json(self.repo_url(&[])?, "the repository").await?;
        Ok(repo.default_branch)
    }

    /// The repository's page, behind the forge's sign-in (AP-103, PF-81).
    pub fn repository_page_url(&self) -> String {
        self.signed_in(&format!(
            "{}/{}/{}",
            self.public_base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo
        ))
    }

    /// The page of one version of a generic package of the organization (AP-101, AP-103).
    pub fn package_page_url(&self, package: &str, version: &str) -> String {
        self.signed_in(&format!(
            "{}/{}/-/packages/generic/{package}/{version}",
            self.public_base.as_str().trim_end_matches('/'),
            self.owner,
        ))
    }

    /// `GET R/WorkflowRuns?$orderby=Number desc&$top={limit}`: the newest workflow runs of the
    /// repository, newest first; what a build's estimate is read from (T-3245). shako answers
    /// 404 until it runs workflows (`PORTAL-CALLS.md` gap 2).
    pub async fn latest_runs(&self, limit: u32) -> Result<Vec<WorkflowRun>, GitError> {
        let mut url = self.repo_url(&["WorkflowRuns"])?;
        url.query_pairs_mut()
            .append_pair("$orderby", "Number desc")
            .append_pair("$top", &limit.to_string());
        let runs: Collection<WorkflowRunRow> = self.get_json(url, "the workflow runs").await?;
        let page = format!(
            "{}/{}/{}/actions",
            self.public_base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo
        );
        Ok(runs
            .value
            .into_iter()
            .map(|run| WorkflowRun {
                url: self.signed_in(&match run.number {
                    Some(number) => format!("{page}/runs/{number}"),
                    None => page.clone(),
                }),
                status: run.status,
                conclusion: run.conclusion.filter(|c| !c.is_empty()),
                commit: run.head_sha,
                number: run.number,
                started_at: run.started_at.filter(|at| !at.is_empty()),
                completed_at: run.completed_at.filter(|at| !at.is_empty()),
            })
            .collect())
    }

    /// `POST R/Shako.DispatchWorkflow`: runs the workflow `file` on `git_ref` (AP-103). A
    /// refusal keeps the forge's own words, which is what a person acts on.
    pub async fn dispatch_workflow(&self, file: &str, git_ref: &str) -> Result<(), GitError> {
        self.act(
            self.repo_url(&["Shako.DispatchWorkflow"])?,
            &json!({ "File": file, "Ref": git_ref }),
        )
        .await?;
        Ok(())
    }

    /// `GET Jobs?$filter=Owner eq '{owner}' and Status eq 'queued'&$top=50`: every job of the
    /// organization that waits for a runner, each with its labels and the repository it belongs
    /// to (AP-130). One page of 50: a queue longer than that is served on the next pass.
    pub async fn queued_jobs(&self) -> Result<Vec<QueuedJob>, GitError> {
        let mut url = self.odata_url(&["Jobs"])?;
        url.query_pairs_mut()
            .append_pair(
                "$filter",
                &format!("Owner eq {} and Status eq 'queued'", literal(&self.owner)),
            )
            .append_pair("$top", "50");
        let page: Collection<JobRow> = self.get_json(url, "the queued jobs").await?;
        Ok(page
            .value
            .into_iter()
            .filter(|job| !job.repository.is_empty())
            .map(|job| QueuedJob {
                id: job.id,
                labels: job.labels,
                repository: job.repository,
            })
            .collect())
    }

    /// `POST R/Shako.RunnerRegistrationToken`: a token a runner registers with to take this
    /// repository's jobs and no other's (AP-130). It is a credential, so it is returned to be
    /// written into one Secret and never logged.
    pub async fn repository_registration_token(&self) -> Result<String, GitError> {
        let res = self
            .act(
                self.repo_url(&["Shako.RunnerRegistrationToken"])?,
                &json!({}),
            )
            .await?;
        let body: Value = res.json().await.map_err(|e| {
            GitError::Transport(format!("failed to parse the registration token: {e}"))
        })?;
        body.get("Token")
            .and_then(Value::as_str)
            .filter(|token| !token.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| GitError::Transport("the forge answered no registration token".into()))
    }

    /// `GET R/Artifacts?$filter=Name eq '{name}'`: the repository's artifacts of that exact
    /// name, each with the commit its run built (AP-104).
    pub async fn artifacts_named(&self, name: &str) -> Result<Vec<Artifact>, GitError> {
        let mut url = self.repo_url(&["Artifacts"])?;
        url.query_pairs_mut()
            .append_pair("$filter", &format!("Name eq {}", literal(name)));
        let listed: Collection<ArtifactRow> = self.get_json(url, "the artifacts").await?;
        Ok(listed
            .value
            .into_iter()
            .filter(|artifact| artifact.name == name)
            .map(|artifact| Artifact {
                id: artifact.id,
                size: artifact.size,
                run: artifact.run_id,
                commit: artifact.head_sha,
            })
            .collect())
    }

    /// `GET R/Artifacts({id})/$value`: the bytes the run uploaded, at most `limit` of them.
    pub async fn download_artifact(&self, id: u64, limit: u64) -> Result<Vec<u8>, GitError> {
        let artifact = format!("Artifacts({id})");
        let url = self.repo_url(&[&artifact, "$value"])?;
        let res = self
            .send(self.http.get(url).timeout(Duration::from_secs(120)))
            .await?;
        read_capped(
            Self::check_status(res).await?,
            limit,
            &format!("artifact {id}"),
        )
        .await
    }

    /// `POST PublishPackage`: one file of a generic package of the organization (AP-101). A file
    /// the version already holds is a `GitError::Conflict`: the registry never replaces one.
    pub async fn put_generic_file(
        &self,
        package: &str,
        version: &str,
        file: &str,
        bytes: Vec<u8>,
    ) -> Result<(), GitError> {
        let params = json!({
            "Owner": self.owner,
            "Package": package,
            "Version": version,
            "File": file,
            "ContentBase64": STANDARD.encode(bytes),
        });
        let url = self.odata_url(&["PublishPackage"])?;
        let res = self
            .send(
                self.http
                    .post(url)
                    .timeout(Duration::from_secs(120))
                    .json(&params),
            )
            .await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `GET PackageFiles(Owner=…,Name=…,Version=…,File=…)/$value`, at most `limit` bytes.
    pub async fn get_generic_file(
        &self,
        package: &str,
        version: &str,
        file: &str,
        limit: u64,
    ) -> Result<Vec<u8>, GitError> {
        let key = format!(
            "PackageFiles(Owner={},Name={},Version={},File={})",
            literal(&self.owner),
            literal(package),
            literal(version),
            literal(file)
        );
        let url = self.odata_url(&[&key, "$value"])?;
        let res = self
            .send(self.http.get(url).timeout(Duration::from_secs(120)))
            .await?;
        read_capped(
            Self::check_status(res).await?,
            limit,
            &format!("{package}/{version}/{file}"),
        )
        .await
    }

    /// A registry token for pushing to `{owner}/{image}` of the forge's container registry,
    /// asked of `/v2/token` with this client's own token (AP-107). The token realm the registry
    /// names carries the public host the Portal may not reach, so the token is asked of the base
    /// the Portal dials.
    async fn registry_token(&self, image: &str) -> Result<String, GitError> {
        let mut url = Url::parse(&format!(
            "{}/v2/token",
            self.base.as_str().trim_end_matches('/')
        ))
        .map_err(|e| GitError::Config(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("service", "container_registry")
            .append_pair(
                "scope",
                &format!("repository:{}/{image}:push,pull", self.owner),
            );
        let res = self
            .http
            .get(url)
            .basic_auth(&self.owner, Some(&self.token))
            .send()
            .await
            .map_err(|e| GitError::Transport(e.to_string()))?;
        #[derive(Deserialize)]
        struct Token {
            token: String,
        }
        let token: Token =
            Self::check_status(res).await?.json().await.map_err(|e| {
                GitError::Transport(format!("failed to parse the registry token: {e}"))
            })?;
        Ok(token.token)
    }

    /// Pushes an image to `{owner}/{image}:{tag}` of the forge's container registry: every blob
    /// it does not hold yet, then the manifest's bytes as they are, and answers the digest the
    /// registry says it stored (AP-107). A blob goes up in one request with its digest, so the
    /// registry checks each one itself.
    pub async fn push_image(
        &self,
        image: &str,
        tag: &str,
        blobs: &[(&str, &[u8])],
        manifest_type: &str,
        manifest: &[u8],
    ) -> Result<String, GitError> {
        let bearer = self.registry_token(image).await?;
        let base = format!(
            "{}/v2/{}/{image}",
            self.base.as_str().trim_end_matches('/'),
            self.owner
        );
        let parse = |url: String| Url::parse(&url).map_err(|e| GitError::Config(e.to_string()));
        let send = |builder: reqwest::RequestBuilder| async {
            builder
                .bearer_auth(&bearer)
                .timeout(Duration::from_secs(300))
                .send()
                .await
                .map_err(|e| GitError::Transport(e.to_string()))
        };
        for (digest, bytes) in blobs {
            let held = send(self.http.head(parse(format!("{base}/blobs/{digest}"))?)).await?;
            if held.status().is_success() {
                continue;
            }
            let mut upload = parse(format!("{base}/blobs/uploads/"))?;
            upload.query_pairs_mut().append_pair("digest", digest);
            let res = send(
                self.http
                    .post(upload)
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(bytes.to_vec()),
            )
            .await?;
            Self::check_status(res).await?;
        }
        let res = send(
            self.http
                .put(parse(format!("{base}/manifests/{tag}"))?)
                .header(reqwest::header::CONTENT_TYPE, manifest_type)
                .body(manifest.to_vec()),
        )
        .await?;
        let res = Self::check_status(res).await?;
        res.headers()
            .get("docker-content-digest")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| GitError::Transport("the registry answered no digest".into()))
    }

    /// `DELETE R`: removes a repository the Portal created in the same operation, when the rest
    /// of it failed (both or neither, CC-85).
    pub async fn delete_repository(&self) -> Result<(), GitError> {
        let res = self.send(self.http.delete(self.repo_url(&[])?)).await?;
        match Self::check_status(res).await {
            Ok(_) | Err(GitError::NotFound) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// `POST R/Shako.Archive`: a deleted project's repository is kept read-only with its
    /// history, never deleted (PF-77).
    pub async fn archive_repository(&self) -> Result<(), GitError> {
        self.act(self.repo_url(&["Shako.Archive"])?, &json!({}))
            .await
            .map(|_| ())
    }

    /// `POST R/Shako.Protect`: `branch` takes no direct push, so every change reaches it through
    /// a merged pull request (PF-87).
    pub async fn protect_branch(&self, branch: &str) -> Result<(), GitError> {
        let params = json!({ "Rule": { "rule_name": branch, "enable_push": false } });
        self.act(self.repo_url(&["Shako.Protect"])?, &params)
            .await
            .map(|_| ())
    }

    /// Every file path of the tree at `git_ref`.
    pub async fn list_tree(&self, git_ref: &str) -> Result<Vec<String>, GitError> {
        Ok(self
            .list_tree_blobs(git_ref)
            .await?
            .into_iter()
            .map(|(path, _)| path)
            .collect())
    }

    /// `GET R/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=1000)`: the tree's files with their
    /// blob ids, so two trees compare without reading a file. Every page is read, so a project
    /// of more than 1000 files stages (T-5047); a branch name with `/` (`workspace/{name}`,
    /// CC-76) travels as a parameter value and needs no resolving first (T-2266).
    pub async fn list_tree_blobs(&self, git_ref: &str) -> Result<Vec<(String, String)>, GitError> {
        let mut paths = Vec::new();
        let mut tree_sha: Option<String> = None;
        for page in 0..MAX_TREE_PAGES {
            let mut url = self.repo_url(&["Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"])?;
            url.query_pairs_mut()
                .append_pair("@r", &literal(git_ref))
                .append_pair("@s", &(page * TREE_PAGE).to_string())
                .append_pair("@t", &TREE_PAGE.to_string());
            let raw: TreePage = self.get_json(url, "the tree").await?;

            // A ref that moved between two pages would mix two trees into one listing.
            if *tree_sha.get_or_insert_with(|| raw.sha.clone()) != raw.sha {
                return Err(GitError::Api {
                    status: 200,
                    message: format!("git tree of {git_ref} changed while it was read"),
                });
            }
            let empty = raw.tree.is_empty();
            paths.extend(
                raw.tree
                    .into_iter()
                    .filter(|entry| entry.entry_type == "blob")
                    .map(|entry| (self.render_path(entry.path), entry.sha)),
            );
            if !raw.truncated {
                return Ok(paths);
            }
            if empty {
                break;
            }
        }
        Err(GitError::Api {
            status: 200,
            message: format!(
                "git tree was truncated: more than {MAX_TREE_PAGES} pages of {TREE_PAGE} entries"
            ),
        })
    }

    /// `GET R/Branches('{branch}')`: the head commit id of the branch.
    pub async fn branch_head(&self, branch: &str) -> Result<String, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Branch {
            head_sha: String,
        }
        let key = format!("Branches({})", literal(branch));
        let found: Branch = self.get_json(self.repo_url(&[&key])?, "the branch").await?;
        Ok(found.head_sha)
    }

    /// `GET R/Shako.CommitStatus(Sha=@s)`: the combined CI state of one commit (`success`,
    /// `pending`, `failure`, `error`), or `None` while no check has reported on it (CC-90).
    pub async fn commit_state(&self, sha: &str) -> Result<Option<String>, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Combined {
            #[serde(default)]
            state: String,
            #[serde(default)]
            total_count: u64,
        }
        let mut url = self.repo_url(&["Shako.CommitStatus(Sha=@s)"])?;
        url.query_pairs_mut().append_pair("@s", &literal(sha));
        let combined: Combined = self.get_json(url, "the commit status").await?;
        Ok((combined.total_count > 0 && !combined.state.is_empty()).then_some(combined.state))
    }

    /// `POST R/Shako.CreateBranch`: a new branch cut from an existing one.
    pub async fn create_branch(&self, new_branch: &str, from_branch: &str) -> Result<(), GitError> {
        let params = json!({ "NewBranchName": new_branch, "OldBranchName": from_branch });
        self.act(self.repo_url(&["Shako.CreateBranch"])?, &params)
            .await?;
        Ok(())
    }

    /// `POST R/Shako.DeleteBranch`: drops a branch; one already gone is no error (T-0886).
    pub async fn delete_branch(&self, branch: &str) -> Result<(), GitError> {
        match self
            .act(
                self.repo_url(&["Shako.DeleteBranch"])?,
                &json!({ "Branch": branch }),
            )
            .await
        {
            Ok(_) | Err(GitError::NotFound) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// `GET R/Shako.File(Path=@p,Ref=@r)`: a file's blob id and its text.
    pub async fn get_file(&self, path: &str, git_ref: &str) -> Result<Option<RepoFile>, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct File {
            sha: String,
            #[serde(default)]
            content: String,
        }
        // Not a file of this repository, so this repository does not have it.
        let Some(path) = self.local_path(path) else {
            return Ok(None);
        };
        let path = file_path(path)?;
        let mut url = self.repo_url(&["Shako.File(Path=@p,Ref=@r)"])?;
        url.query_pairs_mut()
            .append_pair("@p", &literal(&path))
            .append_pair("@r", &literal(git_ref));

        let res = self.send(self.http.get(url)).await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let raw: File = Self::check_status(res)
            .await?
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse the file: {e}")))?;

        let clean_base64: String = raw.content.chars().filter(|c| !c.is_whitespace()).collect();
        let decoded_bytes = STANDARD
            .decode(clean_base64.as_bytes())
            .map_err(|e| GitError::Transport(format!("invalid base64 content: {e}")))?;
        let content = String::from_utf8(decoded_bytes)
            .map_err(|e| GitError::Transport(format!("content is not valid utf-8: {e}")))?;

        Ok(Some(RepoFile {
            sha: raw.sha,
            content,
        }))
    }

    /// One file written with human commit attribution: a replacement of blob `sha` when it is
    /// given, else created or replaced whatever is there.
    pub async fn put_file(&self, req: &FileWrite<'_>) -> Result<String, GitError> {
        let change = FileChange {
            operation: if req.sha.is_some() {
                "update"
            } else {
                "upload"
            },
            path: self.write_path(req.path)?,
            content: Some(STANDARD.encode(committed_text(req.path, req.content).as_bytes())),
            sha: req.sha.map(str::to_owned),
        };
        self.commit_changes(req.branch, req.message, req.author, vec![change])
            .await
    }

    /// One commit on `branch` that creates or replaces every `(path, content)` of `uploads` and
    /// deletes every `(path, blob sha)` of `deletes`, with human commit attribution. `upload`
    /// needs no blob sha, so a run commits what it changed without reading each file first; only
    /// a deletion names the blob it removes.
    pub async fn change_files(
        &self,
        branch: &str,
        message: &str,
        author: Author<'_>,
        uploads: &[(String, String)],
        deletes: &[(String, String)],
    ) -> Result<String, GitError> {
        let mut files = Vec::with_capacity(uploads.len() + deletes.len());
        for (path, content) in uploads {
            files.push(FileChange {
                operation: "upload",
                path: self.write_path(path)?,
                content: Some(STANDARD.encode(committed_text(path, content).as_bytes())),
                sha: None,
            });
        }
        for (path, sha) in deletes {
            files.push(FileChange {
                operation: "delete",
                path: self.write_path(path)?,
                content: None,
                sha: Some(sha.clone()),
            });
        }
        self.commit_changes(branch, message, author, files).await
    }

    /// One commit on `branch` that replaces every `(path, content, blob sha)` of `updates`. The
    /// forge refuses the whole commit when a file's blob is no longer `sha`, so a change someone
    /// committed after the caller read the file is never overwritten.
    pub async fn update_files(
        &self,
        branch: &str,
        message: &str,
        author: Author<'_>,
        updates: &[(String, String, String)],
    ) -> Result<String, GitError> {
        let mut files = Vec::with_capacity(updates.len());
        for (path, content, sha) in updates {
            files.push(FileChange {
                operation: "update",
                path: self.write_path(path)?,
                content: Some(STANDARD.encode(committed_text(path, content).as_bytes())),
                sha: Some(sha.clone()),
            });
        }
        self.commit_changes(branch, message, author, files).await
    }

    /// One commit on `branch` that writes every `(path, content, blob sha)`: a new file where the
    /// sha is `None`, else a replacement of that blob. Every file lands, or none does
    /// (F-POR1-023).
    pub async fn write_files(
        &self,
        branch: &str,
        message: &str,
        author: Author<'_>,
        writes: &[(String, String, Option<String>)],
    ) -> Result<String, GitError> {
        let mut files = Vec::with_capacity(writes.len());
        for (path, content, sha) in writes {
            files.push(FileChange {
                operation: if sha.is_some() { "update" } else { "create" },
                path: self.write_path(path)?,
                content: Some(STANDARD.encode(committed_text(path, content).as_bytes())),
                sha: sha.clone(),
            });
        }
        self.commit_changes(branch, message, author, files).await
    }

    /// `POST R/Shako.CommitFiles`: every file change of `files` as one commit, all or nothing;
    /// answers the commit.
    async fn commit_changes(
        &self,
        branch: &str,
        message: &str,
        author: Author<'_>,
        files: Vec<FileChange>,
    ) -> Result<String, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Committed {
            sha: String,
        }
        let message = crate::change::ProposedVia::sign(message);
        let params = json!({
            "Branch": branch,
            "Message": message,
            "Author": author,
            "Committer": author,
            "Files": files,
        });
        let res = self
            .act(self.repo_url(&["Shako.CommitFiles"])?, &params)
            .await?;
        let committed: Committed = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse the commit: {e}")))?;
        Ok(committed.sha)
    }

    /// One file deleted with human commit attribution.
    pub async fn delete_file(&self, req: &FileDelete<'_>) -> Result<String, GitError> {
        let change = FileChange {
            operation: "delete",
            path: self.write_path(req.path)?,
            content: None,
            sha: Some(req.sha.to_owned()),
        };
        self.commit_changes(req.branch, req.message, req.author, vec![change])
            .await
    }

    /// `GET R/Shako.History(Ref=@r,Path=@p,Top=@n)`: the history of one path.
    ///
    /// The forge is the only source: the mirror knows the manifests of one revision, never how
    /// they got there, so a revision picker that read the mirror would have nothing to show.
    pub async fn list_commits(
        &self,
        git_ref: &str,
        path: &str,
        limit: usize,
    ) -> Result<Vec<Commit>, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Row {
            sha: String,
            #[serde(default)]
            message: String,
            #[serde(default)]
            author: String,
            #[serde(default)]
            email: Option<String>,
            #[serde(default)]
            date: String,
        }
        // No path, or the project's own root in a project repository, is the whole repository
        // (CC-87). Who wrote a Change is read this way, so an empty answer here is a proposer
        // nobody recognises at the approval (PF-58).
        let wanted = path.trim_matches('/');
        let root = wanted.is_empty()
            || self
                .mount
                .as_deref()
                .is_some_and(|mount| wanted == mount.trim_end_matches('/'));
        let path = if root {
            String::new()
        } else {
            let Some(path) = self.local_path(path) else {
                return Ok(Vec::new());
            };
            file_path(path)?
        };
        // An empty `Path` is the whole repository.
        let mut url = self.repo_url(&["Shako.History(Ref=@r,Path=@p,Top=@n)"])?;
        url.query_pairs_mut()
            .append_pair("@r", &literal(git_ref))
            .append_pair("@p", &literal(&path))
            .append_pair("@n", &limit.to_string());
        let rows: Collection<Row> = self.get_json(url, "the history").await?;
        Ok(rows
            .value
            .into_iter()
            .map(|row| Commit {
                sha: row.sha,
                message: row.message.lines().next().unwrap_or_default().to_string(),
                author: row.author,
                email: row.email.filter(|email| !email.is_empty()),
                date: row.date,
            })
            .collect())
    }

    /// The 50 most recently updated pull requests in `state` (`open`, `closed` or `all`).
    pub async fn list_pull_requests(&self, state: &str) -> Result<Vec<PullRequest>, GitError> {
        self.list_pull_requests_page(state, 1, 50).await
    }

    /// `GET R/PullRequests?$filter=State eq '{state}'&$orderby=UpdatedAt desc&$top&$skip`: one
    /// page of pull requests, most recently updated first. `all` filters nothing.
    pub async fn list_pull_requests_page(
        &self,
        state: &str,
        page: u32,
        limit: u32,
    ) -> Result<Vec<PullRequest>, GitError> {
        let mut url = self.repo_url(&["PullRequests"])?;
        {
            let mut query = url.query_pairs_mut();
            if state != "all" {
                query.append_pair("$filter", &format!("State eq {}", literal(state)));
            }
            query
                .append_pair("$orderby", "UpdatedAt desc")
                .append_pair("$top", &limit.to_string())
                .append_pair(
                    "$skip",
                    &(u64::from(page.max(1) - 1) * u64::from(limit)).to_string(),
                );
        }
        let rows: Collection<PullRequestRow> = self.get_json(url, "the pull requests").await?;
        Ok(rows.value.into_iter().map(|row| self.pull(row)).collect())
    }

    /// `POST R/Shako.OpenPullRequest`: a new pull request from `head` into `base`.
    pub async fn create_pull_request(
        &self,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, GitError> {
        // A merge request opened inside a call over MCP names its channel (AG-137).
        let body = crate::change::ProposedVia::sign_body(body);
        let params = json!({ "Head": head, "Base": base, "Title": title, "Body": body });
        let res = self
            .act(self.repo_url(&["Shako.OpenPullRequest"])?, &params)
            .await?;
        let row: PullRequestRow = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse the pull request: {e}")))?;
        Ok(self.pull(row))
    }

    /// `GET R/PullRequests({number})/Files`: every file the pull request changes.
    pub async fn pull_request_files(&self, number: u64) -> Result<Vec<ChangedFile>, GitError> {
        let rows: Collection<ChangedFileRow> = self
            .get_json(
                self.pull_url_api(number, &["Files"])?,
                "the pull request's files",
            )
            .await?;
        Ok(rows
            .value
            .into_iter()
            .map(|row| ChangedFile {
                deleted: row.status == "deleted",
                added: row.status == "added",
                path: self.render_path(row.filename),
            })
            .collect())
    }

    /// The history of `git_ref` as one `git bundle` (MF-45): `GET R/Shako.BundleLink(Ref=@r)`
    /// names the download, whose bytes are no OData. The link carries the forge's public host,
    /// which the Portal may not reach, so its path is fetched from the base the Portal dials.
    /// The forge bundles that ref alone, as `refs/heads/bundle` and `HEAD`: no other branch and
    /// no tag.
    pub async fn bundle(&self, git_ref: &str) -> Result<Vec<u8>, GitError> {
        #[derive(Deserialize)]
        struct Link {
            value: String,
        }
        let mut url = self.repo_url(&["Shako.BundleLink(Ref=@r)"])?;
        url.query_pairs_mut().append_pair("@r", &literal(git_ref));
        let link: Link = self.get_json(url, "the bundle link").await?;
        let named = Url::parse(&link.value)
            .map_err(|e| GitError::Transport(format!("the bundle link is no URL: {e}")))?;
        let path = named.path();
        let api = path
            .find("/api/v1/repos/")
            .map(|at| &path[at..])
            .ok_or_else(|| GitError::Transport("the bundle link names no repository".into()))?;
        let full = format!("{}{api}", self.base.as_str().trim_end_matches('/'));
        let here = Url::parse(&full)
            .map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))?;
        let res = Self::check_status(self.send(self.http.get(here)).await?).await?;
        res.bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| GitError::Transport(format!("failed to read the bundle: {e}")))
    }

    /// `GET R/Tags?$top=50&$skip=…`: every tag of the repository and the commit it names, all
    /// pages.
    pub async fn list_tags(&self) -> Result<Vec<(String, String)>, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Tag {
            name: String,
            #[serde(default)]
            target_sha: String,
        }
        let mut tags = Vec::new();
        for page in 0.. {
            let mut url = self.repo_url(&["Tags"])?;
            url.query_pairs_mut()
                .append_pair("$top", &TAG_PAGE.to_string())
                .append_pair("$skip", &(page * TAG_PAGE).to_string());
            let rows: Collection<Tag> = self.get_json(url, "the tags").await?;
            let count = rows.value.len();
            tags.extend(rows.value.into_iter().map(|tag| (tag.name, tag.target_sha)));
            if count < TAG_PAGE || page >= 1000 {
                break;
            }
        }
        Ok(tags)
    }

    /// `GET R/PullRequests({number})`: one pull request, its mergeability as of now.
    pub async fn pull_request(&self, number: u64) -> Result<PullRequest, GitError> {
        let row: PullRequestRow = self
            .get_json(self.pull_url_api(number, &[])?, "the pull request")
            .await?;
        Ok(self.pull(row))
    }

    /// `GET R/Commits('{sha}')`: the whole message of one commit, body included.
    pub async fn commit_message(&self, sha: &str) -> Result<String, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct CommitRow {
            #[serde(default)]
            message: String,
        }
        let key = format!("Commits({})", literal(sha));
        let row: CommitRow = self.get_json(self.repo_url(&[&key])?, "the commit").await?;
        Ok(row.message)
    }

    /// `GET R/PullRequests({number})/Reviews`: the text of every review on the pull request,
    /// oldest first.
    pub async fn review_bodies(&self, number: u64) -> Result<Vec<String>, GitError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Review {
            #[serde(default)]
            body: String,
        }
        let rows: Collection<Review> = self
            .get_json(self.pull_url_api(number, &["Reviews"])?, "the reviews")
            .await?;
        Ok(rows.value.into_iter().map(|review| review.body).collect())
    }

    /// `POST R/PullRequests({number})/Shako.Review`: a review on the pull request.
    pub async fn review(
        &self,
        number: u64,
        event: ReviewEvent,
        body: &str,
    ) -> Result<(), GitError> {
        self.act(
            self.pull_url_api(number, &["Shako.Review"])?,
            &json!({ "Event": event, "Body": body }),
        )
        .await?;
        Ok(())
    }

    /// `POST R/PullRequests({number})/Shako.Close`: closes the pull request without merging,
    /// which is what the Portal reads back as a rejected change.
    pub async fn close_pull_request(&self, number: u64) -> Result<(), GitError> {
        self.act(self.pull_url_api(number, &["Shako.Close"])?, &json!({}))
            .await?;
        Ok(())
    }

    /// `POST R/PullRequests({number})/Shako.UpdateBranch`: merges the base branch into the pull
    /// request's branch, since the configuration's branch rule merges nothing over an outdated
    /// base (PF-105). A branch already up to date answers 200; a base that does not merge into
    /// it, 409.
    pub async fn update_pull_request(&self, number: u64) -> Result<(), GitError> {
        self.act(
            self.pull_url_api(number, &["Shako.UpdateBranch"])?,
            &json!({ "Style": "merge" }),
        )
        .await
        .map(|_| ())
    }

    /// `POST R/PullRequests({number})/Shako.Merge`: merges pull request `number`.
    ///
    /// `head_commit_id` is the commit whose content the caller approved: the forge refuses the
    /// merge (409) when the branch has moved past it, so nothing is merged that nobody reviewed
    /// (PF-57, T-1683). `None` only where there is nothing to review — a proposal this Portal
    /// wrote and merges in the same call.
    pub async fn merge(
        &self,
        number: u64,
        style: MergeStyle,
        message: &str,
        head_commit_id: Option<&str>,
    ) -> Result<(), GitError> {
        let mut params = json!({ "Style": style, "Message": message });
        if let Some(sha) = head_commit_id.filter(|sha| !sha.is_empty()) {
            params["HeadCommitId"] = json!(sha);
        }
        self.act(self.pull_url_api(number, &["Shako.Merge"])?, &params)
            .await?;
        Ok(())
    }
}

/// A repository file path as a parameter value: a `.` or `..` segment (a path someone typed) is
/// refused, since it names no file of the repository (F-R2POR4-004).
fn file_path(path: String) -> Result<String, GitError> {
    if path.split('/').any(|segment| matches!(segment, "." | "..")) {
        return Err(GitError::NotFound);
    }
    Ok(path)
}

#[cfg(test)]
mod committed_text_tests {
    use super::committed_text;

    #[test]
    fn yaml_is_committed_in_the_manifest_layout_and_other_files_as_given() {
        let messy =
            "kind: Policy\napiVersion: joinedcontext.com/v1alpha1\nspec:\n  actions:\n  - read\n";
        let tidy =
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nspec:\n  actions: [read]\n";
        assert_eq!(committed_text("projects/p/policies/a.yaml", messy), tidy);
        assert_eq!(committed_text("projects/p/policies/a.yml", messy), tidy);
        assert_eq!(committed_text("projects/p/README.md", messy), messy);
    }

    /// F-POR2-017: a native YAML file is no manifest; its comments and layout stay as written.
    #[test]
    fn a_native_yaml_file_is_committed_byte_for_byte() {
        let bento = "# the feed, read hourly\ninput:\n  http_client:\n    url: https://example.org/feed\n    verb: GET\n";
        assert_eq!(
            committed_text("projects/p/pipelines/feed/bento.yaml", bento),
            bento
        );
    }

    #[test]
    fn yaml_the_formatter_refuses_is_committed_as_given() {
        let anchored = "a: &x 1\nb: *x\n";
        assert_eq!(committed_text("a.yaml", anchored), anchored);
    }
}

#[cfg(test)]
mod browse_url_tests {
    use super::ShakoClient;

    fn env(public: Option<&str>) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| match name {
            "JC_SHAKO_URL" => Some("http://shako.dev.svc.cluster.local:3000".to_string()),
            "JC_SHAKO_OWNER" => Some("joinedcontext".to_string()),
            "JC_SHAKO_REPO" => Some("configuration".to_string()),
            "JC_SHAKO_TOKEN" => Some("t".to_string()),
            "JC_SHAKO_PUBLIC_URL" => public.map(str::to_string),
            _ => None,
        }
    }

    /// A "Source" link opens in a browser, which resolves no cluster-internal name — and
    /// carries the forge's sign-in, because a Portal session is not a forge session and the
    /// repository is private (PF-81).
    #[test]
    fn the_source_link_uses_the_public_forge_url_behind_the_sign_in() {
        let client = ShakoClient::from_env(env(Some("https://city.example/git")))
            .expect("config")
            .expect("configured");
        assert_eq!(
            client.browse_url("projects/helsinki/pipelines/p/pipeline.yaml", "main"),
            "https://city.example/git/user/oauth2/keycloak?redirect_to=%2Fgit%2Fjoinedcontext%2Fconfiguration%2Fsrc%2Fbranch%2Fmain%2Fprojects%2Fhelsinki%2Fpipelines%2Fp%2Fpipeline.yaml"
        );
    }

    /// Without a public URL the API base is the best the Portal knows.
    #[test]
    fn the_source_link_falls_back_to_the_api_base() {
        let client = ShakoClient::from_env(env(None))
            .expect("config")
            .expect("configured");
        assert_eq!(
            client.browse_url("a.yaml", "main"),
            "http://shako.dev.svc.cluster.local:3000/user/oauth2/keycloak\
             ?redirect_to=%2Fjoinedcontext%2Fconfiguration%2Fsrc%2Fbranch%2Fmain%2Fa.yaml"
        );
    }

    /// The merge request a change links is opened in a browser too (AP-71).
    #[test]
    fn the_merge_request_link_uses_the_public_forge_url() {
        let client = ShakoClient::from_env(env(Some("https://city.example/git/")))
            .expect("config")
            .expect("configured");
        assert_eq!(
            client.pull_url(110),
            "https://city.example/git/user/oauth2/keycloak\
             ?redirect_to=%2Fgit%2Fjoinedcontext%2Fconfiguration%2Fpulls%2F110"
        );
    }

    #[test]
    fn an_invalid_public_url_is_a_config_error() {
        assert!(ShakoClient::from_env(env(Some("not a url"))).is_err());
    }
}

#[cfg(test)]
mod application_repository_tests {
    use super::{GitError, ShakoClient};

    fn env(apps: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| match name {
            "JC_SHAKO_URL" => Some("http://shako.dev.svc.cluster.local:3000".to_string()),
            "JC_SHAKO_OWNER" => Some("joinedcontext".to_string()),
            "JC_SHAKO_REPO" => Some("configuration".to_string()),
            "JC_SHAKO_TOKEN" => Some("configuration-token".to_string()),
            other => apps
                .iter()
                .find(|(key, _)| *key == other)
                .map(|(_, value)| (*value).to_string()),
        }
    }

    /// PF-106: an application's repository, its packages and its builds are the applications'
    /// organization's, written with their machine user's token; the configuration keeps its own.
    #[test]
    fn an_application_lives_in_its_own_organization_with_its_own_token() {
        let client = ShakoClient::from_env(env(&[
            ("JC_SHAKO_APPS_OWNER", "joinedcontext-apps"),
            ("JC_SHAKO_APPS_TOKEN", "apps-token"),
        ]))
        .expect("config")
        .expect("configured");
        let app = client.for_application("helsinki_bikes");
        assert_eq!(
            (app.owner.as_str(), app.repo.as_str()),
            ("joinedcontext-apps", "helsinki_bikes")
        );
        assert_eq!(app.token, "apps-token");
        assert_eq!(client.applications().owner, "joinedcontext-apps");
        assert_eq!(
            app.clone_url(),
            "http://shako.dev.svc.cluster.local:3000/joinedcontext-apps/helsinki_bikes.git"
        );
        let configuration = client.for_repository("helsinki");
        assert_eq!(
            (configuration.owner.as_str(), configuration.token.as_str()),
            ("joinedcontext", "configuration-token")
        );
        assert!(
            !format!("{client:?}").contains("apps-token"),
            "the token reached a log line"
        );
    }

    /// Without the two variables an application stays beside the configuration, as before.
    #[test]
    fn without_an_applications_organization_nothing_moves() {
        let client = ShakoClient::from_env(env(&[]))
            .expect("config")
            .expect("configured");
        let app = client.for_application("helsinki_bikes");
        assert_eq!(
            (app.owner.as_str(), app.token.as_str()),
            ("joinedcontext", "configuration-token")
        );
    }

    /// One of the two, or a blank one, is a mistake in the deployment and never half a move.
    #[test]
    fn half_an_applications_organization_is_a_config_error() {
        for apps in [
            &[("JC_SHAKO_APPS_OWNER", "joinedcontext-apps")][..],
            &[("JC_SHAKO_APPS_TOKEN", "apps-token")][..],
            &[
                ("JC_SHAKO_APPS_OWNER", " "),
                ("JC_SHAKO_APPS_TOKEN", "apps-token"),
            ][..],
        ] {
            assert!(
                matches!(ShakoClient::from_env(env(apps)), Err(GitError::Config(_))),
                "{apps:?} was accepted"
            );
        }
    }
}

#[cfg(test)]
mod project_repository_tests {
    use super::{Author, ShakoClient};

    fn project() -> ShakoClient {
        ShakoClient::new(
            "http://127.0.0.1:9".parse().expect("url"),
            "joinedcontext",
            "configuration",
            "t",
        )
        .expect("client")
        .for_project("ovzdusie-repo", "ovzdusie")
    }

    /// PF-58: a Change's head commit is read with no path; in a project repository that is the
    /// whole repository, not a path outside it, or its proposer reads as the Portal's account.
    #[tokio::test]
    async fn a_project_repository_lists_its_head_commit_for_no_path() {
        let forge = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/odata/Repositories(Owner='joinedcontext',Name='ovzdusie-repo')/Shako.History(Ref=@r,Path=@p,Top=@n)",
            ))
            .and(wiremock::matchers::query_param("@r", "'head-1'"))
            .and(wiremock::matchers::query_param("@p", "''"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": [{ "Sha": "head-1", "Message": "m", "Author": "Jana", "Email": "jana@hel.fi", "Date": "" }]
            })))
            .mount(&forge)
            .await;
        let client = ShakoClient::new(
            forge.uri().parse().expect("url"),
            "joinedcontext",
            "configuration",
            "t",
        )
        .expect("client")
        .for_project("ovzdusie-repo", "ovzdusie");
        let commits = client.list_commits("head-1", "", 1).await.expect("commits");
        assert_eq!(
            commits.first().and_then(|c| c.email.as_deref()),
            Some("jana@hel.fi")
        );
    }

    /// CC-87: a project repository is spoken to in render paths; one outside the project is not
    /// its file on a read and is refused on a write, before anything reaches the forge (the
    /// address above answers nothing).
    #[tokio::test]
    async fn a_path_outside_the_project_is_neither_read_nor_written() {
        let client = project();
        assert!(client.is_project_repository());
        assert_eq!(
            client
                .local_path("projects/ovzdusie/spaces/air/space.yaml")
                .as_deref(),
            Some("spaces/air/space.yaml")
        );
        assert_eq!(
            client.render_path("project.yaml".into()),
            "projects/ovzdusie/project.yaml"
        );
        for outside in [
            "org.yaml",
            "projects/doprava/project.yaml",
            "projects/ovzdusie/",
            "users/roles/r.yaml",
        ] {
            assert_eq!(client.local_path(outside), None, "{outside}");
            assert_eq!(
                client.get_file(outside, "main").await.expect("no request"),
                None
            );
        }
        let author = Author {
            name: "Jana",
            email: "jana@hel.fi",
        };
        let refused = client
            .change_files(
                "portal/x",
                "m",
                author,
                &[
                    ("projects/ovzdusie/project.yaml".into(), "a".into()),
                    ("org.yaml".into(), "b".into()),
                ],
                &[],
            )
            .await
            .expect_err("org.yaml is not a file of the project repository");
        assert!(refused.to_string().contains("org.yaml"), "{refused}");
        let refused = client
            .write_files(
                "portal/x",
                "m",
                author,
                &[
                    ("projects/ovzdusie/project.yaml".into(), "a".into(), None),
                    ("org.yaml".into(), "b".into(), Some("abc".into())),
                ],
            )
            .await
            .expect_err("write_files refuses org.yaml in the project repository too");
        assert!(refused.to_string().contains("org.yaml"), "{refused}");

        // F-R2POR4-004: a `..` in a path is refused before any request, and a value that holds
        // `/`, `?` or `#` stays inside its own segment: it never reaches another forge route.
        for climbing in [
            "projects/ovzdusie/../../../user",
            "projects/ovzdusie/spaces/../../../other/repo",
            "projects/ovzdusie/./x",
        ] {
            assert!(
                client.get_file(climbing, "main").await.is_err(),
                "{climbing}"
            );
            assert!(
                client
                    .change_files(
                        "portal/x",
                        "m",
                        author,
                        &[(climbing.into(), "a".into())],
                        &[]
                    )
                    .await
                    .is_err(),
                "{climbing}"
            );
        }
        assert_eq!(
            client
                .repo_url(&["Branches('../../a b?x#y')"])
                .expect("a branch key")
                .as_str(),
            "http://127.0.0.1:9/odata/Repositories(Owner='joinedcontext',Name='ovzdusie-repo')/Branches('..%2F..%2Fa%20b%3Fx%23y')"
        );
        assert_eq!(
            super::literal("o'brien"),
            "'o''brien'",
            "a quote inside a key value is doubled"
        );

        let organization = client.for_repository("configuration");
        assert!(!organization.is_project_repository());
        assert_eq!(
            organization.local_path("org.yaml").as_deref(),
            Some("org.yaml")
        );
    }
}

#[cfg(test)]
mod receive_pack_tests {
    use super::receive_pack_status;

    fn lines(texts: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for text in texts {
            out.extend_from_slice(format!("{:04x}{text}\n", text.len() + 5).as_bytes());
        }
        out.extend_from_slice(b"0000");
        out
    }

    #[test]
    fn an_unpacked_push_with_every_ref_taken_is_ok() {
        // What the forge answers to a bundle pushed into an empty repository.
        let none: Vec<String> = Vec::new();
        assert_eq!(
            receive_pack_status(b"000eunpack ok\n0017ok refs/heads/main\n0000").expect("ok"),
            none
        );
        assert_eq!(
            receive_pack_status(&lines(&[
                "unpack ok",
                "ok refs/heads/main",
                "ok refs/tags/v1"
            ]))
            .expect("ok"),
            none
        );
    }

    #[test]
    fn a_refused_ref_is_answered_and_a_failed_unpack_is_the_error() {
        let refused = receive_pack_status(&lines(&[
            "unpack ok",
            "ok refs/heads/main",
            "ng refs/tags/v1 missing necessary objects",
        ]))
        .expect("the other refs are set");
        assert_eq!(refused, ["refs/tags/v1 missing necessary objects"]);
        let unpack = receive_pack_status(&lines(&["unpack index-pack abnormal exit"]))
            .expect_err("not unpacked");
        assert!(unpack.to_string().contains("index-pack"), "{unpack}");
    }

    #[test]
    fn an_answer_that_is_not_a_report_is_an_error() {
        for answer in [&b""[..], b"0000", b"zzzz", b"00ffunpack ok"] {
            assert!(receive_pack_status(answer).is_err(), "{answer:?}");
        }
    }
}
