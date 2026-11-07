//! Gitea REST API client (CC-03, CC-32, CC-34, CC-41, CC-42, CC-44).
//!
//! Mutations in joinedcontext commit to Git merge requests server-side. Commits are attributed
//! to the signed-in human author, while the service token only authenticates the HTTP call.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::ApiError;

/// Pages of a recursive tree read before giving up: 100,000 files, far above any project.
const MAX_TREE_PAGES: usize = 100;

/// Gitea repository client.
#[derive(Clone)]
pub struct GiteaClient {
    pub base: Url,
    /// Where a browser reaches the same forge: the API talks to the cluster-internal service,
    /// but a "Source" link opens in the user's browser, which resolves no `.svc.cluster.local`
    /// name. `JC_GITEA_PUBLIC_URL`; the API base when unset.
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
    /// The forge user the Context Gateway reads the configuration as (`JC_GITEA_READER`,
    /// PF-106): a repository the Portal opens for a project it reads too. `None`: nobody is
    /// added.
    reader: Option<String>,
    pub http: reqwest::Client,
}

impl std::fmt::Debug for GiteaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GiteaClient")
            .field("base", &self.base.as_str())
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("mount", &self.mount)
            .field("token", &"[redacted]")
            .field("apps", &self.apps.as_ref().map(|(owner, _)| owner))
            .finish()
    }
}

/// Errors returned by the Gitea client.
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
            // can wait out, not a fault of their request (T-4966: a Gitea 401 emptied the inbox).
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

/// The sentence of a forge error answer: Gitea answers `{"message": …, "url": <its swagger
/// page>}`, and only the message reaches a person, never the raw body (T-4967). Any other body
/// keeps its first line, capped, since a proxy in front of the forge may answer a whole page.
async fn forge_message(res: reqwest::Response) -> String {
    let body = res.text().await.unwrap_or_default();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&body) {
        if let Some(serde_json::Value::String(message)) = map.get("message") {
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

/// A team of the forge organization (PF-87).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Team {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub description: String,
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

#[derive(Deserialize)]
struct QueuedJobsResponse {
    #[serde(default)]
    jobs: Vec<RawQueuedJob>,
}

#[derive(Deserialize)]
struct RawQueuedJob {
    id: u64,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    url: String,
}

/// The repository a job's API URL names, `…/api/v1/repos/{owner}/{repo}/actions/jobs/{id}`, when
/// it is a repository of `owner`; `None` for anything else, which the dispatcher then ignores.
fn repository_of_job(url: &str, owner: &str) -> Option<String> {
    let path = url.split("/api/v1/repos/").nth(1)?;
    let mut parts = path.split('/');
    let (found_owner, repo) = (parts.next()?, parts.next()?);
    (found_owner == owner && parts.next() == Some("actions") && !repo.is_empty())
        .then(|| repo.to_owned())
}

#[derive(Deserialize)]
struct WorkflowRunsResponse {
    #[serde(default)]
    workflow_runs: Vec<WorkflowRunResponse>,
}

#[derive(Deserialize)]
struct WorkflowRunResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    run_number: Option<u64>,
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
struct ArtifactsResponse {
    #[serde(default)]
    artifacts: Vec<ArtifactResponse>,
}

#[derive(Deserialize)]
struct ArtifactResponse {
    id: u64,
    name: String,
    #[serde(default)]
    size_in_bytes: u64,
    workflow_run: ArtifactRunResponse,
}

#[derive(Deserialize)]
struct ArtifactRunResponse {
    id: u64,
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
    pub name: &'a str,
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
struct ChangedFileDto {
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
    /// The commit the head branch pointed at when the forge answered, when it said (Gitea
    /// always does). A review reads every file at this ref and the merge is pinned to it, so a
    /// push between the two is refused instead of merged unread (PF-57, T-1683).
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
    /// Whether the forge named the head's repository and it is the base's own: not a fork. A
    /// forge that names neither says nothing, which counts as a fork (T-3433).
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

// Request and response DTOs for Gitea REST API
#[derive(Deserialize)]
struct RepoDetailsResponse {
    default_branch: String,
}

#[derive(Deserialize)]
struct BranchDetailsResponse {
    commit: CommitIdDto,
}

#[derive(Deserialize)]
struct CommitIdDto {
    id: String,
}

#[derive(Serialize)]
struct CreateBranchPayload<'a> {
    new_branch_name: &'a str,
    old_branch_name: &'a str,
}

#[derive(Deserialize)]
struct ContentsDto {
    sha: String,
    #[serde(default)]
    content: String,
}

#[derive(Serialize)]
struct PutFilePayload<'a> {
    content: String,
    message: &'a str,
    branch: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
    author: Author<'a>,
    committer: Author<'a>,
}

#[derive(Serialize)]
struct ChangeFilesPayload<'a> {
    files: Vec<ChangeFileDto>,
    message: &'a str,
    branch: &'a str,
    author: Author<'a>,
    committer: Author<'a>,
}

#[derive(Serialize)]
struct ChangeFileDto {
    operation: &'static str,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<String>,
}

#[derive(Serialize)]
struct DeleteFilePayload<'a> {
    message: &'a str,
    branch: &'a str,
    sha: &'a str,
    author: Author<'a>,
    committer: Author<'a>,
}

#[derive(Deserialize)]
struct FileCommitResponse {
    #[serde(default)]
    commit: Option<FileCommitShaDto>,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Deserialize)]
struct FileCommitShaDto {
    sha: String,
}

#[derive(Deserialize)]
struct CommitDto {
    sha: String,
    #[serde(default)]
    commit: Option<CommitDetailsDto>,
}

#[derive(Deserialize)]
struct CommitDetailsDto {
    #[serde(default)]
    message: String,
    #[serde(default)]
    author: Option<CommitAuthorDto>,
}

#[derive(Deserialize)]
struct CommitAuthorDto {
    #[serde(default)]
    name: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    date: String,
}

#[derive(Serialize)]
struct CreatePullRequestPayload<'a> {
    head: &'a str,
    base: &'a str,
    title: &'a str,
    body: &'a str,
}

#[derive(Deserialize)]
struct GiteaPullResponse {
    number: u64,
    html_url: String,
    state: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    head: Option<BranchRefDto>,
    #[serde(default)]
    base: Option<BranchRefDto>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    user: Option<GiteaUserDto>,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    merge_commit_sha: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
    #[serde(default)]
    merge_base: Option<String>,
}

#[derive(Deserialize)]
struct BranchRefDto {
    #[serde(rename = "ref", default)]
    git_ref: String,
    #[serde(default)]
    sha: String,
    #[serde(default)]
    repo_id: Option<u64>,
}

#[derive(Deserialize)]
struct GiteaUserDto {
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl From<GiteaPullResponse> for PullRequest {
    fn from(raw: GiteaPullResponse) -> Self {
        let author_name = raw
            .user
            .as_ref()
            .and_then(|u| {
                u.full_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .or(u.login.as_deref())
            })
            .unwrap_or_default()
            .to_string();
        let author_login = raw
            .user
            .as_ref()
            .and_then(|u| u.login.clone())
            .unwrap_or_default();
        let author_email = raw.user.and_then(|u| u.email);
        let same_repository = matches!(
            (
                raw.head.as_ref().and_then(|h| h.repo_id),
                raw.base.as_ref().and_then(|b| b.repo_id),
            ),
            (Some(head), Some(base)) if head == base
        );
        let (head_branch, head_sha) = raw.head.map(|h| (h.git_ref, h.sha)).unwrap_or_default();
        let (base_branch, base_sha) = raw.base.map(|b| (b.git_ref, b.sha)).unwrap_or_default();

        PullRequest {
            number: raw.number,
            url: raw.html_url,
            state: raw.state,
            title: raw.title,
            body: raw.body.unwrap_or_default(),
            head_branch,
            head_sha,
            base_branch,
            base_sha,
            created_at: raw.created_at.unwrap_or_default(),
            author_name,
            author_email,
            author_login,
            same_repository,
            mergeable: raw.mergeable,
            merged: raw.merged,
            repository: String::new(),
            merge_commit_sha: raw.merge_commit_sha.unwrap_or_default(),
            closed_at: raw.closed_at.unwrap_or_default(),
            merge_base: raw.merge_base.unwrap_or_default(),
        }
    }
}

/// One push mirror of a repository, as `GET push_mirrors` lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct PushMirror {
    pub remote_name: String,
    pub remote_address: String,
    /// What the last sync failed with; empty when it succeeded or has not run yet.
    #[serde(default)]
    pub last_error: String,
}

#[derive(Serialize)]
struct ReviewPayload<'a> {
    event: ReviewEvent,
    body: &'a str,
}

#[derive(Serialize)]
struct MergePayload<'a> {
    #[serde(rename = "Do")]
    do_field: MergeStyle,
    merge_message_field: &'a str,
    /// The commit the caller reviewed. Gitea refuses the merge when the branch has moved past
    /// it, which is the only race-free way to merge what was approved (T-1683).
    #[serde(skip_serializing_if = "Option::is_none")]
    head_commit_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct GitTreeResponse {
    #[serde(default)]
    sha: String,
    #[serde(default)]
    tree: Vec<GitTreeEntryDto>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
struct GitTreeEntryDto {
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(default)]
    sha: String,
}

impl GiteaClient {
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
            reader: None,
            http,
        })
    }

    /// Reads the forge's configuration from the environment.
    ///
    /// `JC_GITEA_URL` (the API base the Portal dials), `JC_GITEA_OWNER`, `JC_GITEA_REPO` and
    /// `JC_GITEA_TOKEN` (a secret: the token every push and merge request is written with).
    /// `JC_GITEA_PUBLIC_URL` is the address a browser follows a "Source" link to, which differs
    /// from the API base whenever the forge is reached through the edge; the API base when it
    /// is unset.
    ///
    /// `JC_GITEA_APPS_TOKEN` (a secret, the applications' machine user's) and
    /// `JC_GITEA_APPS_OWNER` move the generated applications' repositories, their packages and
    /// their build queue into an organization of their own, written by a machine user of their
    /// own (PF-106); both or neither.
    ///
    /// Fail-closed: returns `Ok(None)` if all four are absent, or an error if partially set.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, GitError> {
        let apps = match (lookup("JC_GITEA_APPS_OWNER"), lookup("JC_GITEA_APPS_TOKEN")) {
            (None, None) => None,
            (Some(owner), Some(token)) if !owner.trim().is_empty() && !token.trim().is_empty() => {
                Some((owner.trim().to_owned(), token.trim().to_owned()))
            }
            _ => {
                return Err(GitError::Config(
                    "JC_GITEA_APPS_OWNER and JC_GITEA_APPS_TOKEN must be set together".to_string(),
                ))
            }
        };
        match (
            lookup("JC_GITEA_URL"),
            lookup("JC_GITEA_OWNER"),
            lookup("JC_GITEA_REPO"),
            lookup("JC_GITEA_TOKEN"),
        ) {
            (None, None, None, None) => Ok(None),
            (Some(url), Some(owner), Some(repo), Some(token)) => {
                let base = Url::parse(&url)
                    .map_err(|e| GitError::Config(format!("invalid JC_GITEA_URL: {e}")))?;
                let mut client = Self::new(base, owner, repo, token)?;
                client.apps = apps;
                client.reader = lookup("JC_GITEA_READER")
                    .map(|user| user.trim().to_owned())
                    .filter(|user| !user.is_empty());
                if let Some(public) = lookup("JC_GITEA_PUBLIC_URL") {
                    client.public_base = Url::parse(&public)
                        .map_err(|e| GitError::Config(format!("invalid JC_GITEA_PUBLIC_URL: {e}")))?;
                }
                Ok(Some(client))
            }
            _ => Err(GitError::Config(
                "JC_GITEA_URL, JC_GITEA_OWNER, JC_GITEA_REPO and JC_GITEA_TOKEN must be set together"
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

    /// Browser URL of a pull request. Gitea's own `html_url` carries its ROOT_URL, which on a
    /// cluster is the internal service name no browser resolves.
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
    /// answers 404. The link starts the forge's Keycloak sign-in instead of showing its sign-in
    /// page (T-2759): the Keycloak session the Portal holds answers without a prompt, and Gitea
    /// returns to `redirect_to`. Gitea returns only to a local path, which is why the host is
    /// dropped here. `keycloak` is the auth source the Gitea chart registers (components/gitea).
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
    /// the public one; without, Gitea's own `html_url` is the best there is.
    fn pull(&self, raw: GiteaPullResponse) -> PullRequest {
        let mut pull = PullRequest::from(raw);
        pull.repository = self.repo.clone();
        if self.public_base != self.base {
            pull.url = self.pull_url(pull.number);
        }
        pull
    }

    /// The API address of `path` under this repository, each segment percent-encoded. A `.` or
    /// `..` segment (a file path or ref someone typed) is refused: the URL parser would resolve
    /// it into another forge route, called with the Portal's token, so it names nothing here
    /// (F-R2POR4-004).
    fn repo_url(&self, path: &str) -> Result<Url, GitError> {
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if segments
            .iter()
            .any(|segment| matches!(*segment, "." | ".."))
        {
            return Err(GitError::NotFound);
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| GitError::Config(format!("'{}' cannot be a base", self.base)))?
            .pop_if_empty()
            .extend(["api", "v1", "repos", &self.owner, &self.repo])
            .extend(segments.into_iter().filter(|segment| !segment.is_empty()));
        Ok(url)
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<reqwest::Response, GitError> {
        builder
            .header(
                reqwest::header::AUTHORIZATION,
                format!("token {}", self.token),
            )
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
        self.local_path(path).ok_or_else(|| {
            GitError::Conflict(format!(
                "{} is not a file of the project repository {}; the organization's files are \
                 changed in the organization repository",
                path.trim_start_matches('/'),
                self.repo
            ))
        })
    }

    /// A path of this repository as the render spells it.
    fn render_path(&self, path: String) -> String {
        match &self.mount {
            None => path,
            Some(mount) => format!("{mount}{path}"),
        }
    }

    /// The `https` address a person clones the repository from: the forge's public URL, never
    /// the cluster-internal one (AP-77, AP-78).
    pub fn clone_url(&self) -> String {
        format!(
            "{}/{}/{}.git",
            self.public_base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo
        )
    }

    /// Creates the repository in the organization unless it is already there, private and
    /// initialised on `main` so a branch can be cut from it; answers whether it created one.
    ///
    /// An existing repository is never recreated or overwritten, and a creation that lost a race
    /// with another (the forge's 409) is the same answer as finding it (AP-75).
    pub async fn ensure_repository(&self, description: &str) -> Result<bool, GitError> {
        let res = self.send(self.http.get(self.repo_url("")?)).await?;
        match Self::check_status(res).await {
            Ok(_) => return Ok(false),
            Err(GitError::NotFound) => {}
            Err(err) => return Err(err),
        }
        let full = format!(
            "{}/api/v1/orgs/{}/repos",
            self.base.as_str().trim_end_matches('/'),
            self.owner
        );
        let url = Url::parse(&full)
            .map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))?;
        let payload = serde_json::json!({
            "name": self.repo,
            "description": description,
            "private": true,
            "auto_init": true,
            "default_branch": "main",
        });
        let res = self.send(self.http.post(url).json(&payload)).await?;
        match Self::check_status(res).await {
            Ok(_) => Ok(true),
            Err(GitError::Conflict(_)) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Creates this repository as a copy of `origin` with its whole history, through the
    /// forge's migration from its own address (PF-89): the forge refuses a fork into the owner
    /// that holds the origin. Private, without the origin's issues, pull requests, wiki or
    /// releases. Answers `false`, and copies nothing, when a repository of this name is already
    /// there.
    pub async fn migrate_repository(&self, origin: &GiteaClient) -> Result<bool, GitError> {
        let res = self.send(self.http.get(self.repo_url("")?)).await?;
        match Self::check_status(res).await {
            Ok(_) => return Ok(false),
            Err(GitError::NotFound) => {}
            Err(err) => return Err(err),
        }
        let base = self.base.as_str().trim_end_matches('/');
        let full = format!("{base}/api/v1/repos/migrate");
        let url = Url::parse(&full)
            .map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))?;
        let payload = serde_json::json!({
            "clone_addr": format!("{base}/{}/{}.git", origin.owner, origin.repo),
            "service": "gitea",
            "auth_token": self.token,
            "repo_owner": self.owner,
            "repo_name": self.repo,
            "private": true,
            "mirror": false,
            "issues": false,
            "pull_requests": false,
            "wiki": false,
            "releases": false,
            "labels": false,
            "milestones": false,
            "lfs": false,
        });
        let res = self.send(self.http.post(url).json(&payload)).await?;
        match Self::check_status(res).await {
            Ok(_) => Ok(true),
            Err(GitError::Conflict(_)) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Creates the repository in the organization, private and empty, for a push to fill (MF-45);
    /// answers `false`, and creates nothing, when a repository of this name is already there.
    pub async fn create_empty_repository(&self, description: &str) -> Result<bool, GitError> {
        let res = self.send(self.http.get(self.repo_url("")?)).await?;
        match Self::check_status(res).await {
            Ok(_) => return Ok(false),
            Err(GitError::NotFound) => {}
            Err(err) => return Err(err),
        }
        let full = format!(
            "{}/api/v1/orgs/{}/repos",
            self.base.as_str().trim_end_matches('/'),
            self.owner
        );
        let url = Url::parse(&full)
            .map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))?;
        let payload = serde_json::json!({
            "name": self.repo,
            "description": description,
            "private": true,
            "auto_init": false,
            "default_branch": "main",
        });
        let res = self.send(self.http.post(url).json(&payload)).await?;
        match Self::check_status(res).await {
            Ok(_) => Ok(true),
            Err(GitError::Conflict(_)) => Ok(false),
            Err(err) => Err(err),
        }
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

        let full = format!(
            "{}/{}/{}.git/git-receive-pack",
            self.base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo
        );
        let url = Url::parse(&full)
            .map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))?;
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

    /// The push mirrors of the repository: where each one pushes, and what its last sync said
    /// (AP-79). Answers only for a caller that administers the repository.
    pub async fn push_mirrors(&self) -> Result<Vec<PushMirror>, GitError> {
        let res = self
            .send(self.http.get(self.repo_url("push_mirrors")?))
            .await?;
        let res = Self::check_status(res).await?;
        res.json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse push mirrors: {e}")))
    }

    /// Adds a push mirror that syncs on every commit, with a periodic sync behind it for a
    /// commit whose sync failed (AP-79).
    ///
    /// The credential travels in its own members and never inside `remote_address`: the forge
    /// shows the address on the repository's settings page and in `push_mirrors`, and stores the
    /// credential encrypted.
    pub async fn add_push_mirror(
        &self,
        remote_address: &str,
        username: &str,
        password: &str,
    ) -> Result<(), GitError> {
        let payload = serde_json::json!({
            "remote_address": remote_address,
            "remote_username": username,
            "remote_password": password,
            "interval": "8h0m0s",
            "sync_on_commit": true,
        });
        let url = self.repo_url("push_mirrors")?;
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// Removes one push mirror by the name the forge gave it.
    pub async fn delete_push_mirror(&self, remote_name: &str) -> Result<(), GitError> {
        let url = self.repo_url(&format!("push_mirrors/{remote_name}"))?;
        let res = self.send(self.http.delete(url)).await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// Pushes every mirror of the repository now instead of at its next commit or interval.
    pub async fn sync_push_mirrors(&self) -> Result<(), GitError> {
        let url = self.repo_url("push_mirrors-sync")?;
        let res = self.send(self.http.post(url)).await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `GET ""` — returns the repository's default branch.
    pub async fn default_branch(&self) -> Result<String, GitError> {
        let url = self.repo_url("")?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let repo: RepoDetailsResponse = res.json().await.map_err(|e| {
            GitError::Transport(format!("failed to parse repository response: {e}"))
        })?;
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

    /// `GET /actions/runs?limit={limit}` — the newest workflow runs of the repository, newest
    /// first; what a build's estimate is read from (T-3245).
    pub async fn latest_runs(&self, limit: u32) -> Result<Vec<WorkflowRun>, GitError> {
        let mut url = self.repo_url("actions/runs")?;
        url.query_pairs_mut()
            .append_pair("limit", &limit.to_string());
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let runs: WorkflowRunsResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse the workflow runs: {e}")))?;
        Ok(runs
            .workflow_runs
            .into_iter()
            .map(|run| {
                let page = format!(
                    "{}/{}/{}/actions",
                    self.public_base.as_str().trim_end_matches('/'),
                    self.owner,
                    self.repo
                );
                WorkflowRun {
                    url: self.signed_in(&match run.run_number {
                        Some(number) => format!("{page}/runs/{number}"),
                        None => page,
                    }),
                    status: run.status,
                    conclusion: run.conclusion.filter(|c| !c.is_empty()),
                    commit: run.head_sha,
                    number: run.run_number,
                    started_at: run.started_at.filter(|at| !at.is_empty()),
                    completed_at: run.completed_at.filter(|at| !at.is_empty()),
                }
            })
            .collect())
    }

    /// `POST /actions/workflows/{file}/dispatches` — runs the workflow `file` on `git_ref`
    /// (AP-103). A refusal keeps the forge's own words, which is what a person acts on.
    pub async fn dispatch_workflow(&self, file: &str, git_ref: &str) -> Result<(), GitError> {
        let url = self.repo_url(&format!("actions/workflows/{file}/dispatches"))?;
        let res = self
            .send(
                self.http
                    .post(url)
                    .json(&serde_json::json!({ "ref": git_ref })),
            )
            .await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `GET /orgs/{owner}/actions/jobs?status=queued` — every job of the organization that waits
    /// for a runner, each with its labels and the repository it belongs to (AP-130). One page of
    /// 50: a queue longer than that is served on the next pass.
    pub async fn queued_jobs(&self) -> Result<Vec<QueuedJob>, GitError> {
        let mut url = Url::parse(&format!(
            "{}/api/v1/orgs/{}/actions/jobs",
            self.base.as_str().trim_end_matches('/'),
            self.owner
        ))
        .map_err(|e| GitError::Transport(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("status", "queued")
            .append_pair("limit", "50");
        let res = Self::check_status(self.send(self.http.get(url)).await?).await?;
        let page: QueuedJobsResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse the queued jobs: {e}")))?;
        Ok(page
            .jobs
            .into_iter()
            .filter_map(|job| {
                let repository = repository_of_job(&job.url, &self.owner)?;
                Some(QueuedJob {
                    id: job.id,
                    labels: job.labels,
                    repository,
                })
            })
            .collect())
    }

    /// `POST /actions/runners/registration-token` of this repository: a token a runner registers
    /// with to take this repository's jobs and no other's (AP-130). It is a credential, so it
    /// is returned to be written into one Secret and never logged.
    pub async fn repository_registration_token(&self) -> Result<String, GitError> {
        let url = self.repo_url("actions/runners/registration-token")?;
        let res = Self::check_status(self.send(self.http.post(url)).await?).await?;
        let body: serde_json::Value = res.json().await.map_err(|e| {
            GitError::Transport(format!("failed to parse the registration token: {e}"))
        })?;
        body.get("token")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| GitError::Transport("the forge answered no registration token".into()))
    }

    /// `GET /actions/artifacts?name={name}` — the repository's artifacts of that exact name, each
    /// with the commit its run built (AP-104).
    pub async fn artifacts_named(&self, name: &str) -> Result<Vec<Artifact>, GitError> {
        let mut url = self.repo_url("actions/artifacts")?;
        url.query_pairs_mut().append_pair("name", name);
        let res = Self::check_status(self.send(self.http.get(url)).await?).await?;
        let listed: ArtifactsResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse the artifacts: {e}")))?;
        Ok(listed
            .artifacts
            .into_iter()
            .filter(|artifact| artifact.name == name)
            .map(|artifact| Artifact {
                id: artifact.id,
                size: artifact.size_in_bytes,
                run: artifact.workflow_run.id,
                commit: artifact.workflow_run.head_sha,
            })
            .collect())
    }

    /// `GET /actions/artifacts/{id}/zip` — the bytes the run uploaded, at most `limit` of them.
    ///
    /// The forge answers with a redirect to a signed address on its ROOT_URL, the public host the
    /// cluster may not reach, so the signed path is fetched from the API base the Portal dials;
    /// the signature is the grant and no token goes with it. The path is kept from `/api/v1/` on:
    /// a ROOT_URL with a path (`https://host/git/`) signs `/git/api/v1/…`, which only the public
    /// proxy strips, and the forge itself answers 404 (T-2633).
    pub async fn download_artifact(&self, id: u64, limit: u64) -> Result<Vec<u8>, GitError> {
        let url = self.repo_url(&format!("actions/artifacts/{id}/zip"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = if res.status().is_redirection() {
            let signed = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| Url::parse(value).ok())
                .ok_or_else(|| GitError::Transport("the forge redirected nowhere".into()))?;
            let path = signed.path();
            let api = path.find("/api/v1/").map_or(path, |at| &path[at..]);
            let full = format!("{}{api}", self.base.as_str().trim_end_matches('/'));
            let mut here = Url::parse(&full)
                .map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))?;
            here.set_query(signed.query());
            self.http
                .get(here)
                .timeout(Duration::from_secs(120))
                .send()
                .await
                .map_err(|e| GitError::Transport(e.to_string()))?
        } else {
            res
        };
        read_capped(
            Self::check_status(res).await?,
            limit,
            &format!("artifact {id}"),
        )
        .await
    }

    fn generic_package_url(
        &self,
        package: &str,
        version: &str,
        file: &str,
    ) -> Result<Url, GitError> {
        let full = format!(
            "{}/api/packages/{}/generic/{package}/{version}/{file}",
            self.base.as_str().trim_end_matches('/'),
            self.owner
        );
        Url::parse(&full).map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))
    }

    /// `PUT /api/packages/{owner}/generic/{package}/{version}/{file}` — one file of a generic
    /// package of the organization (AP-101). A file the version already holds is a
    /// `GitError::Conflict`: the registry never replaces one.
    pub async fn put_generic_file(
        &self,
        package: &str,
        version: &str,
        file: &str,
        bytes: Vec<u8>,
    ) -> Result<(), GitError> {
        let url = self.generic_package_url(package, version, file)?;
        let res = self
            .send(
                self.http
                    .put(url)
                    .timeout(Duration::from_secs(120))
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(bytes),
            )
            .await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `GET /api/packages/{owner}/generic/{package}/{version}/{file}`, at most `limit` bytes.
    pub async fn get_generic_file(
        &self,
        package: &str,
        version: &str,
        file: &str,
        limit: u64,
    ) -> Result<Vec<u8>, GitError> {
        let url = self.generic_package_url(package, version, file)?;
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
    /// names carries ROOT_URL, the public host the cluster may not reach, so the token is asked
    /// of the API base the Portal dials.
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

    /// `DELETE /repos/{owner}/{repo}` — removes a repository the Portal created in the same
    /// operation, when the rest of it failed (both or neither, CC-85).
    pub async fn delete_repository(&self) -> Result<(), GitError> {
        let res = self.send(self.http.delete(self.repo_url("")?)).await?;
        match Self::check_status(res).await {
            Ok(_) | Err(GitError::NotFound) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn org_url(&self, path: &str) -> Result<Url, GitError> {
        let full = format!(
            "{}/api/v1/{}",
            self.base.as_str().trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        Url::parse(&full).map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))
    }

    /// Whether the token's own user owns the organization (`GET /user`, then
    /// `GET /users/{login}/orgs/{org}/permissions`): creating a team or giving it a repository
    /// takes an owner, and at layout 2 the Portal's forge user is none (T-2647, T-3323). A user
    /// who is no member of the organization at all owns nothing either.
    pub async fn owns_organization(&self) -> Result<bool, GitError> {
        #[derive(Deserialize)]
        struct Me {
            login: String,
        }
        #[derive(Deserialize)]
        struct Permissions {
            is_owner: bool,
        }
        let me: Me = Self::check_status(self.send(self.http.get(self.org_url("user")?)).await?)
            .await?
            .json()
            .await
            .map_err(|e| GitError::Transport(e.to_string()))?;
        let url = self.org_url(&format!(
            "users/{}/orgs/{}/permissions",
            me.login, self.owner
        ))?;
        match Self::check_status(self.send(self.http.get(url)).await?).await {
            Ok(res) => Ok(res
                .json::<Permissions>()
                .await
                .map_err(|e| GitError::Transport(e.to_string()))?
                .is_owner),
            Err(GitError::NotFound) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Every team of the organization, a page of fifty at a time.
    pub async fn org_teams(&self) -> Result<Vec<Team>, GitError> {
        let mut teams = Vec::new();
        for page in 1.. {
            let url = self.org_url(&format!("orgs/{}/teams?limit=50&page={page}", self.owner))?;
            let res = Self::check_status(self.send(self.http.get(url)).await?).await?;
            let batch: Vec<Team> = res
                .json()
                .await
                .map_err(|e| GitError::Transport(e.to_string()))?;
            let last = batch.len() < 50;
            teams.extend(batch);
            if last || page >= 100 {
                break;
            }
        }
        Ok(teams)
    }

    /// Creates a team of the organization that reaches only the repositories added to it, with
    /// `permission` (`read` or `write`) on code, pull requests, issues and releases; answers it.
    pub async fn create_team(
        &self,
        name: &str,
        description: &str,
        permission: &str,
    ) -> Result<Team, GitError> {
        let units = ["repo.code", "repo.pulls", "repo.issues", "repo.releases"];
        let payload = serde_json::json!({
            "name": name,
            "description": description,
            "permission": permission,
            "includes_all_repositories": false,
            "can_create_org_repo": false,
            "units": units,
            "units_map": units
                .iter()
                .map(|unit| ((*unit).to_owned(), serde_json::json!(permission)))
                .collect::<serde_json::Map<_, _>>(),
        });
        let url = self.org_url(&format!("orgs/{}/teams", self.owner))?;
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res)
            .await?
            .json()
            .await
            .map_err(|e| GitError::Transport(e.to_string()))
    }

    /// Removes a team; a team already gone is the same answer.
    pub async fn delete_team(&self, id: u64) -> Result<(), GitError> {
        let res = self
            .send(self.http.delete(self.org_url(&format!("teams/{id}"))?))
            .await?;
        match Self::check_status(res).await {
            Ok(_) | Err(GitError::NotFound) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// The names of the repositories a team reaches.
    pub async fn team_repositories(&self, id: u64) -> Result<Vec<String>, GitError> {
        #[derive(Deserialize)]
        struct Named {
            name: String,
        }
        let mut names = Vec::new();
        for page in 1.. {
            let url = self.org_url(&format!("teams/{id}/repos?limit=50&page={page}"))?;
            let res = Self::check_status(self.send(self.http.get(url)).await?).await?;
            let batch: Vec<Named> = res
                .json()
                .await
                .map_err(|e| GitError::Transport(e.to_string()))?;
            let last = batch.len() < 50;
            names.extend(batch.into_iter().map(|repo| repo.name));
            if last || page >= 100 {
                break;
            }
        }
        Ok(names)
    }

    /// Gives a team this repository (`true`) or takes it away (`false`).
    pub async fn set_team_repository(
        &self,
        id: u64,
        repo: &str,
        reach: bool,
    ) -> Result<(), GitError> {
        let url = self.org_url(&format!("teams/{id}/repos/{}/{repo}", self.owner))?;
        let builder = if reach {
            self.http.put(url)
        } else {
            self.http.delete(url)
        };
        match Self::check_status(self.send(builder).await?).await {
            Ok(_) => Ok(()),
            Err(GitError::NotFound) if !reach => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// `PATCH /repos/{owner}/{repo}` with `archived: true` — a deleted project's repository is
    /// kept read-only with its history, never deleted (PF-77).
    pub async fn archive_repository(&self) -> Result<(), GitError> {
        let payload = serde_json::json!({ "archived": true });
        let res = self
            .send(self.http.patch(self.repo_url("")?).json(&payload))
            .await?;
        Self::check_status(res).await.map(|_| ())
    }

    /// `PUT /collaborators/{reader}` with `read` — the gateway reads a project repository the
    /// Portal opened (PF-106): its machine user is a collaborator on the repositories it reads
    /// and nothing more, so a new one has to name it. Nothing without `JC_GITEA_READER`.
    pub async fn let_the_reader_in(&self) -> Result<(), GitError> {
        let Some(reader) = &self.reader else {
            return Ok(());
        };
        let res = self
            .send(
                self.http
                    .put(self.repo_url(&format!("collaborators/{reader}"))?)
                    .json(&serde_json::json!({ "permission": "read" })),
            )
            .await?;
        Self::check_status(res).await.map(|_| ())
    }

    /// `POST /branch_protections` — `branch` takes no direct push: every change reaches it
    /// through a merged pull request (PF-87).
    pub async fn protect_branch(&self, branch: &str) -> Result<(), GitError> {
        let payload = serde_json::json!({ "rule_name": branch, "enable_push": false });
        let res = self
            .send(
                self.http
                    .post(self.repo_url("branch_protections")?)
                    .json(&payload),
            )
            .await?;
        Self::check_status(res).await.map(|_| ())
    }

    /// `GET /git/trees/{git_ref}?recursive=true&per_page=1000&page=N` — retrieves the Git tree.
    pub async fn list_tree(&self, git_ref: &str) -> Result<Vec<String>, GitError> {
        Ok(self
            .list_tree_blobs(git_ref)
            .await?
            .into_iter()
            .map(|(path, _)| path)
            .collect())
    }

    /// The tree's files with their blob ids, so two trees compare without reading a file.
    ///
    /// Gitea hands a recursive tree out 1000 entries a page and sets `truncated` while more
    /// remain: every page is read, so a project of more than 1000 files stages (T-5047).
    ///
    /// A branch whose name carries a slash — every workspace branch, `workspace/{name}` (CC-76) —
    /// is resolved to its head commit first: Gitea's `git/trees/{ref}` answers `404` for such a
    /// name, encoded or not, so every read of a copy's tree came back "not found" and the copy
    /// showed the project instead of itself (T-2266).
    pub async fn list_tree_blobs(&self, git_ref: &str) -> Result<Vec<(String, String)>, GitError> {
        let resolved = if git_ref.contains('/') {
            self.branch_head(git_ref).await?
        } else {
            git_ref.to_owned()
        };
        let git_ref = resolved.as_str();
        let mut paths = Vec::new();
        let mut tree_sha: Option<String> = None;
        for page in 1..=MAX_TREE_PAGES {
            let mut url = self.repo_url(&format!("git/trees/{git_ref}"))?;
            url.query_pairs_mut()
                .append_pair("recursive", "true")
                .append_pair("per_page", "1000")
                .append_pair("page", &page.to_string());

            let res = self.send(self.http.get(url)).await?;
            let res = Self::check_status(res).await?;
            let status = res.status().as_u16();
            let raw: GitTreeResponse = res
                .json()
                .await
                .map_err(|e| GitError::Transport(format!("failed to parse tree response: {e}")))?;

            // A ref that moved between two pages would mix two trees into one listing.
            if *tree_sha.get_or_insert_with(|| raw.sha.clone()) != raw.sha {
                return Err(GitError::Api {
                    status,
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
                "git tree was truncated: more than {MAX_TREE_PAGES} pages of 1000 entries"
            ),
        })
    }

    /// `GET /branches/{branch}` — returns the head commit id for the given branch.
    pub async fn branch_head(&self, branch: &str) -> Result<String, GitError> {
        let url = self.repo_url(&format!("branches/{branch}"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let branch_dto: BranchDetailsResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse branch response: {e}")))?;
        Ok(branch_dto.commit.id)
    }

    /// `GET /commits/{sha}/status` — the combined CI state of one commit (`success`, `pending`,
    /// `failure`, `error`), or `None` while no check has reported on it (CC-90).
    pub async fn commit_state(&self, sha: &str) -> Result<Option<String>, GitError> {
        #[derive(Deserialize)]
        struct CombinedStatus {
            #[serde(default)]
            state: String,
            #[serde(default)]
            total_count: u64,
        }
        let url = self.repo_url(&format!("commits/{sha}/status"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let combined: CombinedStatus = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse commit status: {e}")))?;
        Ok((combined.total_count > 0 && !combined.state.is_empty()).then_some(combined.state))
    }

    /// `POST /branches` — creates a new branch from an existing one.
    pub async fn create_branch(&self, new_branch: &str, from_branch: &str) -> Result<(), GitError> {
        let url = self.repo_url("branches")?;
        let payload = CreateBranchPayload {
            new_branch_name: new_branch,
            old_branch_name: from_branch,
        };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `DELETE /branches/{branch}` — drops a branch; one already gone is no error (T-0886).
    pub async fn delete_branch(&self, branch: &str) -> Result<(), GitError> {
        let url = self.repo_url(&format!("branches/{branch}"))?;
        let res = self.send(self.http.delete(url)).await?;
        match Self::check_status(res).await {
            Ok(_) | Err(GitError::NotFound) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// `GET /contents/{path}?ref={git_ref}` — reads a file and decodes its base64 content.
    pub async fn get_file(&self, path: &str, git_ref: &str) -> Result<Option<RepoFile>, GitError> {
        // Not a file of this repository, so this repository does not have it.
        let Some(path) = self.local_path(path) else {
            return Ok(None);
        };
        let mut url = self.repo_url(&format!("contents/{path}"))?;
        url.query_pairs_mut().append_pair("ref", git_ref);

        let res = self.send(self.http.get(url)).await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let res = Self::check_status(res).await?;
        let raw: ContentsDto = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse contents response: {e}")))?;

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

    /// `PUT /contents/{path}` — creates or replaces a file with human commit attribution.
    pub async fn put_file(&self, req: &FileWrite<'_>) -> Result<String, GitError> {
        let url = self.repo_url(&format!("contents/{}", self.write_path(req.path)?))?;
        let encoded = STANDARD.encode(committed_text(req.path, req.content).as_bytes());
        let message = crate::change::ProposedVia::sign(req.message);
        let payload = PutFilePayload {
            content: encoded,
            message: &message,
            branch: req.branch,
            sha: req.sha,
            author: req.author,
            committer: req.author,
        };
        let res = self.send(self.http.put(url).json(&payload)).await?;
        let res = Self::check_status(res).await?;
        let body: FileCommitResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse commit response: {e}")))?;
        let commit_sha = body
            .commit
            .map(|c| c.sha)
            .or(body.sha)
            .ok_or_else(|| GitError::Transport("missing commit sha in response".to_string()))?;
        Ok(commit_sha)
    }

    /// `POST /contents` — one commit on `branch` that creates or replaces every
    /// `(path, content)` of `uploads` and deletes every `(path, blob sha)` of `deletes`, with
    /// human commit attribution. `upload` needs no blob sha, so a run commits what it changed
    /// without reading each file first; only a deletion names the blob it removes.
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
            files.push(ChangeFileDto {
                operation: "upload",
                path: self.write_path(path)?,
                content: Some(STANDARD.encode(committed_text(path, content).as_bytes())),
                sha: None,
            });
        }
        for (path, sha) in deletes {
            files.push(ChangeFileDto {
                operation: "delete",
                path: self.write_path(path)?,
                content: None,
                sha: Some(sha.clone()),
            });
        }
        self.commit_changes(branch, message, author, files).await
    }

    /// `POST /contents` — one commit on `branch` that replaces every `(path, content, blob sha)`
    /// of `updates`. Gitea refuses the whole commit when a file's blob is no longer `sha`, so a
    /// change someone committed after the caller read the file is never overwritten.
    pub async fn update_files(
        &self,
        branch: &str,
        message: &str,
        author: Author<'_>,
        updates: &[(String, String, String)],
    ) -> Result<String, GitError> {
        let mut files = Vec::with_capacity(updates.len());
        for (path, content, sha) in updates {
            files.push(ChangeFileDto {
                operation: "update",
                path: self.write_path(path)?,
                content: Some(STANDARD.encode(committed_text(path, content).as_bytes())),
                sha: Some(sha.clone()),
            });
        }
        self.commit_changes(branch, message, author, files).await
    }

    /// `POST /contents` — one commit on `branch` that writes every `(path, content, blob sha)`:
    /// a new file where the sha is `None`, else a replacement of that blob. Every file lands, or
    /// none does (F-POR1-023).
    pub async fn write_files(
        &self,
        branch: &str,
        message: &str,
        author: Author<'_>,
        writes: &[(String, String, Option<String>)],
    ) -> Result<String, GitError> {
        let mut files = Vec::with_capacity(writes.len());
        for (path, content, sha) in writes {
            files.push(ChangeFileDto {
                operation: if sha.is_some() { "update" } else { "create" },
                path: self.write_path(path)?,
                content: Some(STANDARD.encode(committed_text(path, content).as_bytes())),
                sha: sha.clone(),
            });
        }
        self.commit_changes(branch, message, author, files).await
    }

    async fn commit_changes(
        &self,
        branch: &str,
        message: &str,
        author: Author<'_>,
        files: Vec<ChangeFileDto>,
    ) -> Result<String, GitError> {
        let url = self.repo_url("contents")?;
        let message = crate::change::ProposedVia::sign(message);
        let payload = ChangeFilesPayload {
            files,
            message: &message,
            branch,
            author,
            committer: author,
        };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        let res = Self::check_status(res).await?;
        let body: FileCommitResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse commit response: {e}")))?;
        body.commit
            .map(|c| c.sha)
            .ok_or_else(|| GitError::Transport("missing commit sha in response".to_string()))
    }

    /// `DELETE /contents/{path}` — deletes a file with human commit attribution.
    pub async fn delete_file(&self, req: &FileDelete<'_>) -> Result<String, GitError> {
        let url = self.repo_url(&format!("contents/{}", self.write_path(req.path)?))?;
        let message = crate::change::ProposedVia::sign(req.message);
        let payload = DeleteFilePayload {
            message: &message,
            branch: req.branch,
            sha: req.sha,
            author: req.author,
            committer: req.author,
        };
        let res = self.send(self.http.delete(url).json(&payload)).await?;
        let res = Self::check_status(res).await?;
        let body: Result<FileCommitResponse, _> = res.json().await;
        let commit_sha = match body {
            Ok(b) => b.commit.map(|c| c.sha).or(b.sha).unwrap_or_default(),
            Err(_) => String::new(),
        };
        Ok(commit_sha)
    }

    /// `GET /commits?sha={git_ref}&path={path}&limit={limit}` — the history of one path.
    ///
    /// The forge is the only source: the mirror knows the manifests of one revision, never how
    /// they got there, so a revision picker that read the mirror would have nothing to show.
    pub async fn list_commits(
        &self,
        git_ref: &str,
        path: &str,
        limit: usize,
    ) -> Result<Vec<Commit>, GitError> {
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
            path
        };
        let mut url = self.repo_url("commits")?;
        url.query_pairs_mut().append_pair("sha", git_ref);
        if !path.is_empty() {
            url.query_pairs_mut().append_pair("path", &path);
        }
        url.query_pairs_mut()
            .append_pair("limit", &limit.to_string())
            .append_pair("stat", "false")
            .append_pair("verification", "false")
            .append_pair("files", "false");

        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let raw: Vec<CommitDto> = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse commits response: {e}")))?;

        Ok(raw
            .into_iter()
            .map(|dto| {
                let details = dto.commit.unwrap_or(CommitDetailsDto {
                    message: String::new(),
                    author: None,
                });
                let author = details.author.unwrap_or(CommitAuthorDto {
                    name: String::new(),
                    email: None,
                    date: String::new(),
                });
                Commit {
                    sha: dto.sha,
                    message: details
                        .message
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    author: author.name,
                    email: author.email,
                    date: author.date,
                }
            })
            .collect())
    }

    /// `GET /pulls?state={state}&sort=recentupdate&limit=50` — retrieves pull requests.
    pub async fn list_pull_requests(&self, state: &str) -> Result<Vec<PullRequest>, GitError> {
        self.list_pull_requests_page(state, 1, 50).await
    }

    /// `GET /pulls` — one page of pull requests in `state`, most recently updated first.
    pub async fn list_pull_requests_page(
        &self,
        state: &str,
        page: u32,
        limit: u32,
    ) -> Result<Vec<PullRequest>, GitError> {
        let mut url = self.repo_url("pulls")?;
        url.query_pairs_mut()
            .append_pair("state", state)
            .append_pair("sort", "recentupdate")
            .append_pair("page", &page.to_string())
            .append_pair("limit", &limit.to_string());

        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let raw_list: Vec<GiteaPullResponse> = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse pull requests list: {e}")))?;
        Ok(raw_list.into_iter().map(|raw| self.pull(raw)).collect())
    }

    /// `POST /pulls` — creates a new pull request.
    pub async fn create_pull_request(
        &self,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, GitError> {
        let url = self.repo_url("pulls")?;
        // A merge request opened inside a call over MCP names its channel (AG-137).
        let body = crate::change::ProposedVia::sign_body(body);
        let payload = CreatePullRequestPayload {
            head,
            base,
            title,
            body: &body,
        };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        let res = Self::check_status(res).await?;
        let raw: GiteaPullResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse pull request: {e}")))?;
        Ok(self.pull(raw))
    }

    /// `GET /pulls/{number}/files` — every file the pull request changes, all pages.
    pub async fn pull_request_files(&self, number: u64) -> Result<Vec<ChangedFile>, GitError> {
        const PAGE: usize = 50;
        let mut files = Vec::new();
        for page in 1.. {
            let mut url = self.repo_url(&format!("pulls/{number}/files"))?;
            url.query_pairs_mut()
                .append_pair("page", &page.to_string())
                .append_pair("limit", &PAGE.to_string());
            let res = self.send(self.http.get(url)).await?;
            let res = Self::check_status(res).await?;
            let raw: Vec<ChangedFileDto> = res.json().await.map_err(|e| {
                GitError::Transport(format!("failed to parse pull request files: {e}"))
            })?;
            let count = raw.len();
            files.extend(raw.into_iter().map(|dto| ChangedFile {
                deleted: dto.status == "deleted",
                added: dto.status == "added",
                path: self.render_path(dto.filename),
            }));
            if count < PAGE {
                break;
            }
        }
        Ok(files)
    }

    /// `GET /archive/{git_ref}.bundle` — the history of `git_ref` as one `git bundle` (MF-45).
    /// The forge bundles that ref alone, as `refs/heads/bundle` and `HEAD`: no other branch and
    /// no tag.
    pub async fn bundle(&self, git_ref: &str) -> Result<Vec<u8>, GitError> {
        let url = self.repo_url(&format!("archive/{git_ref}.bundle"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        res.bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| GitError::Transport(format!("failed to read the bundle: {e}")))
    }

    /// `GET /tags` — every tag of the repository and the commit it names, all pages.
    pub async fn list_tags(&self) -> Result<Vec<(String, String)>, GitError> {
        #[derive(Deserialize)]
        struct TagDto {
            name: String,
            commit: TagCommitDto,
        }
        #[derive(Deserialize)]
        struct TagCommitDto {
            sha: String,
        }
        const PAGE: usize = 50;
        let mut tags = Vec::new();
        for page in 1.. {
            let mut url = self.repo_url("tags")?;
            url.query_pairs_mut()
                .append_pair("page", &page.to_string())
                .append_pair("limit", &PAGE.to_string());
            let res = self.send(self.http.get(url)).await?;
            let res = Self::check_status(res).await?;
            let raw: Vec<TagDto> = res
                .json()
                .await
                .map_err(|e| GitError::Transport(format!("failed to parse tags: {e}")))?;
            let count = raw.len();
            tags.extend(raw.into_iter().map(|tag| (tag.name, tag.commit.sha)));
            if count < PAGE {
                break;
            }
        }
        Ok(tags)
    }

    /// `GET /pulls/{number}` — retrieves an existing pull request.
    pub async fn pull_request(&self, number: u64) -> Result<PullRequest, GitError> {
        let url = self.repo_url(&format!("pulls/{number}"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let raw: GiteaPullResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse pull request: {e}")))?;
        Ok(self.pull(raw))
    }

    /// `GET /git/commits/{sha}` — the whole message of one commit, body included.
    pub async fn commit_message(&self, sha: &str) -> Result<String, GitError> {
        let url = self.repo_url(&format!("git/commits/{sha}"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let raw: CommitDto = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse commit: {e}")))?;
        Ok(raw
            .commit
            .map(|details| details.message)
            .unwrap_or_default())
    }

    /// `GET /pulls/{number}/reviews` — the text of every review on the pull request, oldest first.
    pub async fn review_bodies(&self, number: u64) -> Result<Vec<String>, GitError> {
        #[derive(Deserialize)]
        struct ReviewDto {
            #[serde(default)]
            body: String,
        }
        let url = self.repo_url(&format!("pulls/{number}/reviews"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let raw: Vec<ReviewDto> = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse reviews: {e}")))?;
        Ok(raw.into_iter().map(|review| review.body).collect())
    }

    /// `POST /pulls/{number}/reviews` — submits a review on the pull request.
    pub async fn review(
        &self,
        number: u64,
        event: ReviewEvent,
        body: &str,
    ) -> Result<(), GitError> {
        let url = self.repo_url(&format!("pulls/{number}/reviews"))?;
        let payload = ReviewPayload { event, body };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `PATCH /pulls/{number}` with `state: closed` — closes the pull request without merging,
    /// which is what the Portal reads back as a rejected change.
    pub async fn close_pull_request(&self, number: u64) -> Result<(), GitError> {
        let url = self.repo_url(&format!("pulls/{number}"))?;
        let res = self
            .send(
                self.http
                    .patch(url)
                    .json(&serde_json::json!({ "state": "closed" })),
            )
            .await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `POST /pulls/{number}/update?style=merge` — merges the base branch into the pull
    /// request's branch: the configuration's branch rule merges nothing over an outdated base
    /// (PF-105). The forge answers 500 for a branch that is not behind, and 409 when the base
    /// does not merge into it.
    pub async fn update_pull_request(&self, number: u64) -> Result<(), GitError> {
        let mut url = self.repo_url(&format!("pulls/{number}/update"))?;
        url.query_pairs_mut().append_pair("style", "merge");
        let res = self.send(self.http.post(url)).await?;
        Self::check_status(res).await.map(|_| ())
    }

    /// `POST /pulls/{number}/merge` — merges the pull request.
    /// Merges pull request `number`.
    ///
    /// `head_commit_id` is the commit whose content the caller approved: the forge refuses the
    /// merge when the branch has moved past it, so nothing is merged that nobody reviewed
    /// (PF-57, T-1683). `None` only where there is nothing to review — a proposal this Portal
    /// wrote and merges in the same call.
    pub async fn merge(
        &self,
        number: u64,
        style: MergeStyle,
        message: &str,
        head_commit_id: Option<&str>,
    ) -> Result<(), GitError> {
        let url = self.repo_url(&format!("pulls/{number}/merge"))?;
        let payload = MergePayload {
            do_field: style,
            merge_message_field: message,
            head_commit_id: head_commit_id.filter(|sha| !sha.is_empty()),
        };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res).await?;
        Ok(())
    }
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
    use super::GiteaClient;

    fn env(public: Option<&str>) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| match name {
            "JC_GITEA_URL" => Some("http://gitea-http.dev.svc.cluster.local:3000".to_string()),
            "JC_GITEA_OWNER" => Some("joinedcontext".to_string()),
            "JC_GITEA_REPO" => Some("configuration".to_string()),
            "JC_GITEA_TOKEN" => Some("t".to_string()),
            "JC_GITEA_PUBLIC_URL" => public.map(str::to_string),
            _ => None,
        }
    }

    /// A "Source" link opens in a browser, which resolves no cluster-internal name — and
    /// carries the forge's sign-in, because a Portal session is not a forge session and the
    /// repository is private (PF-81).
    #[test]
    fn the_source_link_uses_the_public_forge_url_behind_the_sign_in() {
        let client = GiteaClient::from_env(env(Some("https://city.example/git")))
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
        let client = GiteaClient::from_env(env(None))
            .expect("config")
            .expect("configured");
        assert_eq!(
            client.browse_url("a.yaml", "main"),
            "http://gitea-http.dev.svc.cluster.local:3000/user/oauth2/keycloak\
             ?redirect_to=%2Fjoinedcontext%2Fconfiguration%2Fsrc%2Fbranch%2Fmain%2Fa.yaml"
        );
    }

    /// The merge request a change links is opened in a browser too (AP-71).
    #[test]
    fn the_merge_request_link_uses_the_public_forge_url() {
        let client = GiteaClient::from_env(env(Some("https://city.example/git/")))
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
        assert!(GiteaClient::from_env(env(Some("not a url"))).is_err());
    }
}

#[cfg(test)]
mod application_repository_tests {
    use super::{GitError, GiteaClient};

    fn env(apps: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| match name {
            "JC_GITEA_URL" => Some("http://gitea-http.dev.svc.cluster.local:3000".to_string()),
            "JC_GITEA_OWNER" => Some("joinedcontext".to_string()),
            "JC_GITEA_REPO" => Some("configuration".to_string()),
            "JC_GITEA_TOKEN" => Some("configuration-token".to_string()),
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
        let client = GiteaClient::from_env(env(&[
            ("JC_GITEA_APPS_OWNER", "joinedcontext-apps"),
            ("JC_GITEA_APPS_TOKEN", "apps-token"),
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
            "http://gitea-http.dev.svc.cluster.local:3000/joinedcontext-apps/helsinki_bikes.git"
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
        let client = GiteaClient::from_env(env(&[]))
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
            &[("JC_GITEA_APPS_OWNER", "joinedcontext-apps")][..],
            &[("JC_GITEA_APPS_TOKEN", "apps-token")][..],
            &[
                ("JC_GITEA_APPS_OWNER", " "),
                ("JC_GITEA_APPS_TOKEN", "apps-token"),
            ][..],
        ] {
            assert!(
                matches!(GiteaClient::from_env(env(apps)), Err(GitError::Config(_))),
                "{apps:?} was accepted"
            );
        }
    }
}

#[cfg(test)]
mod project_repository_tests {
    use super::{Author, GiteaClient};

    fn project() -> GiteaClient {
        GiteaClient::new(
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
                "/api/v1/repos/joinedcontext/ovzdusie-repo/commits",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "sha": "head-1",
                "commit": { "message": "m", "author": { "name": "Jana", "email": "jana@hel.fi", "date": "" } }
            }])))
            .mount(&forge)
            .await;
        let client = GiteaClient::new(
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

        // F-R2POR4-004: a `..` in a path or ref would climb out of the repository into another
        // forge route; refused before any request. What a segment carries stays in its segment.
        for climbing in [
            "contents/../../../user",
            "contents/projects/ovzdusie/../../../other/repo/contents/x",
            "git/trees/..",
        ] {
            assert!(client.repo_url(climbing).is_err(), "{climbing}");
        }
        assert!(client
            .get_file("projects/ovzdusie/../../../user", "main")
            .await
            .is_err());
        assert_eq!(
            client
                .repo_url("contents/spaces/a b?x#y.yaml")
                .expect("a file name")
                .as_str(),
            "http://127.0.0.1:9/api/v1/repos/joinedcontext/ovzdusie-repo/contents/spaces/a%20b%3Fx%23y.yaml"
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
        // What Gitea 1.27 answered to a bundle pushed into an empty repository.
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
