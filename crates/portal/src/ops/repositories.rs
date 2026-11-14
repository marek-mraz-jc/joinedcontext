//! The project's repositories on the forge (shako), read through the registry (AG-59, CC-48).
//!
//! One source for the Repositories pages, `POST …/ops/{name}` and MCP: a person browses the
//! repositories their project owns the way a forge's own pages show them, files, commits, branches,
//! pull requests, and an administrator reads the repository's settings. Nothing here writes: a
//! change to a project's configuration goes through the Portal's propose and approve flow.
//!
//! Which repositories a project owns (deny by default): its own configuration repository of layout
//! 2, the repositories shako records under the project, and its applications' repositories
//! (`{project}_{app}`, which no other project's name can produce, AP-75). Any other repository,
//! another project's or another Organization's, is not there: 404, never 403 (PF-59, R20).
//!
//! Every read of shako goes through [`odata`], one GET of its OData service with the Portal's
//! machine user; the person's own right is decided here first, by the registry.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::bounds::text;
use super::{parse_input, Caller, OpError, Operation, OperationAnnotations};
use crate::change::Lane;
use crate::error::ApiError;
use crate::forge::shako::ShakoClient;
use crate::state::AppState;

pub const LIST: &str = "jc_repository_list";
pub const GET: &str = "jc_repository_get";
pub const TREE: &str = "jc_repository_tree";
pub const FILE: &str = "jc_repository_file";
pub const COMMITS: &str = "jc_repository_commits";
pub const COMMIT: &str = "jc_repository_commit";
pub const REFS: &str = "jc_repository_refs";
pub const PULLS: &str = "jc_repository_pulls";
pub const PULL: &str = "jc_repository_pull";
pub const SETTINGS: &str = "jc_repository_settings";

/// The largest file shown as text; a larger one is named, sized and offered for download only.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The largest README rendered on a repository's page.
const MAX_README_BYTES: u64 = 256 * 1024;
/// Entries of one folder listed; the rest is said to be cut.
const MAX_TREE_ENTRIES: usize = 1000;
/// Files of one commit given a patch, and the patch text of one commit in all.
const MAX_PATCH_FILES: usize = 30;
const MAX_PATCH_BYTES: usize = 512 * 1024;
/// Lines of one side of a file the diff compares, and the edit distance it follows before it
/// shows the file as replaced whole.
const MAX_DIFF_LINES: usize = 5000;
const MAX_DIFF_EDITS: usize = 1000;
/// Bytes of one forge answer held: a listing, or one file of [`MAX_FILE_BYTES`] as text and as
/// base64 beside it.
const ANSWER_BYTES: u64 = 4 * MAX_FILE_BYTES + 64 * 1024;
/// Folders deep a path may go.
const MAX_DEPTH: usize = 32;

// ---------------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------------

const REPOSITORY_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$";
const REF_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._/-]{0,254}$";
const SHA_PATTERN: &str = "^[0-9a-f]{40}$";

fn repository_schema() -> Value {
    json!({
        "type": "string",
        "description": "The repository's name, e.g. ovzdusie_dashboard",
        "pattern": REPOSITORY_PATTERN,
        "maxLength": 100
    })
}

fn ref_schema() -> Value {
    json!({
        "type": "string",
        "description": "A branch, tag or commit; the default branch when absent, e.g. main",
        "pattern": REF_PATTERN,
        "maxLength": 255
    })
}

fn path_schema(description: &str) -> Value {
    text(description, 1024)
}

fn page_schema() -> (Value, Value) {
    (
        json!({ "type": "integer", "minimum": 0, "maximum": 100_000, "description": "Items to skip, the nextCursor of the page before" }),
        json!({ "type": "integer", "minimum": 1, "maximum": 100, "description": "Items per page, 30 when absent" }),
    )
}

fn closed(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn list_input() -> Value {
    let (cursor, limit) = page_schema();
    closed(
        json!({
            "filter": text("Words the name or description holds, e.g. dashboard", 100),
            "cursor": cursor,
            "limit": limit
        }),
        &[],
    )
}

fn repository_input() -> Value {
    closed(
        json!({ "repository": repository_schema() }),
        &["repository"],
    )
}

fn tree_input() -> Value {
    closed(
        json!({
            "repository": repository_schema(),
            "ref": ref_schema(),
            "path": path_schema("A folder, the root when absent, e.g. src/pages")
        }),
        &["repository"],
    )
}

fn file_input() -> Value {
    closed(
        json!({
            "repository": repository_schema(),
            "ref": ref_schema(),
            "path": path_schema("The file, e.g. README.md")
        }),
        &["repository", "path"],
    )
}

fn commits_input() -> Value {
    let (cursor, limit) = page_schema();
    closed(
        json!({
            "repository": repository_schema(),
            "ref": ref_schema(),
            "path": path_schema("Only the commits touching this file or folder, e.g. README.md"),
            "cursor": cursor,
            "limit": limit
        }),
        &["repository"],
    )
}

fn commit_input() -> Value {
    closed(
        json!({
            "repository": repository_schema(),
            "sha": { "type": "string", "pattern": SHA_PATTERN, "description": "The commit's 40-character id", "examples": ["9fceb02d0ae598e95dc970b74767f19372d61af8"] }
        }),
        &["repository", "sha"],
    )
}

fn pulls_input() -> Value {
    let (cursor, limit) = page_schema();
    closed(
        json!({
            "repository": repository_schema(),
            "state": { "type": "string", "enum": ["open", "closed", "all"], "description": "open when absent" },
            "cursor": cursor,
            "limit": limit
        }),
        &["repository"],
    )
}

fn pull_input() -> Value {
    closed(
        json!({
            "repository": repository_schema(),
            "number": { "type": "integer", "minimum": 1, "maximum": 1_000_000_000, "description": "The pull request's number" }
        }),
        &["repository", "number"],
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    filter: Option<String>,
    cursor: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryInput {
    repository: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathInput {
    repository: String,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitsInput {
    repository: String,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    path: Option<String>,
    cursor: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitInput {
    repository: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PullsInput {
    repository: String,
    state: Option<String>,
    cursor: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PullInput {
    repository: String,
    number: u64,
}

fn invalid(path: &str, message: impl Into<String>) -> OpError {
    OpError::InvalidInput {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// A path inside the repository as its folders, or the input's 422: no `..` or `.` folder, no
/// leading `/`, no backslash, no control character, no empty folder. The forge resolves a path
/// itself too; this keeps a traversal from ever being asked.
pub fn path_segments(path: &str) -> Result<Vec<&str>, OpError> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let refused = |why: &str| Err(invalid("/path", format!("the path {why}")));
    if path.starts_with('/') {
        return refused("starts at the root of the machine; give it from the repository's root");
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return refused("holds a backslash or a control character");
    }
    let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return refused("names an empty, `.` or `..` folder");
    }
    if segments.len() > MAX_DEPTH {
        return refused("is deeper than 32 folders");
    }
    Ok(segments)
}

/// A ref the input names, or the input's 422 for a `..` the pattern lets through.
fn checked_ref(git_ref: Option<&str>) -> Result<Option<&str>, OpError> {
    match git_ref {
        Some(r) if r.contains("..") || r.ends_with('/') || r.contains("//") => {
            Err(invalid("/ref", "the ref names no branch, tag or commit"))
        }
        other => Ok(other),
    }
}

fn page(cursor: Option<usize>, limit: Option<usize>) -> (usize, usize) {
    (cursor.unwrap_or(0), limit.unwrap_or(30).clamp(1, 100))
}

// ---------------------------------------------------------------------------------------------
// The forge boundary
// ---------------------------------------------------------------------------------------------

/// One read of shako's OData service: the only call this module makes of the forge.
async fn odata(
    client: &ShakoClient,
    path: &str,
    query: &[(&str, &str)],
) -> Result<Value, ApiError> {
    client
        .odata_get(path, query, ANSWER_BYTES)
        .await
        .map_err(ApiError::from)
}

/// A shako id put into an OData key: a UUID, so nothing the forge answered can break out of the
/// quotes.
fn key(id: &str) -> Result<&str, ApiError> {
    let uuid = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if uuid {
        Ok(id)
    } else {
        Err(ApiError::Internal(
            "the forge answered a repository id that is no UUID".into(),
        ))
    }
}

/// One repository the project owns, as the forge listed it, with the client that reads it.
#[derive(Clone)]
struct Owned {
    row: Value,
    role: &'static str,
    client: ShakoClient,
}

impl Owned {
    fn field(&self, name: &str) -> &str {
        self.row[name].as_str().unwrap_or_default()
    }

    fn id(&self) -> Result<&str, ApiError> {
        key(self.field("Id"))
    }

    fn name(&self) -> &str {
        self.field("Name")
    }

    /// The address a person clones from: the forge's public one (`JC_SHAKO_PUBLIC_URL`), never
    /// the address the Portal dials, which the forge's own `CloneUrl` names (T-6893).
    fn clone_url(&self) -> String {
        let owner = match self.field("Owner") {
            "" => self.client.owner.as_str(),
            owner => owner,
        };
        format!(
            "{}/{owner}/{}.git",
            self.client.public_base.as_str().trim_end_matches('/'),
            self.name()
        )
    }

    fn default_branch(&self) -> &str {
        match self.field("DefaultBranch") {
            "" => "main",
            branch => branch,
        }
    }

    fn reference<'a>(&'a self, asked: Option<&'a str>) -> &'a str {
        asked.unwrap_or_else(|| self.default_branch())
    }

    fn summary(&self) -> Value {
        json!({
            "name": self.name(),
            "owner": self.field("Owner"),
            "role": self.role,
            "description": self.field("Description"),
            "defaultBranch": self.default_branch(),
            "private": self.row["Private"].as_bool().unwrap_or(true),
            "archived": self.row["Archived"].as_bool().unwrap_or(false),
            "sizeBytes": self.row["SizeBytes"].as_u64().unwrap_or(0),
            "updatedAt": self.field("UpdatedAt"),
        })
    }
}

/// What a project's repository is to it, or `None` when it is not the project's (deny by
/// default): its configuration repository of layout 2, one shako records under the project, or
/// one of its applications'.
pub fn role_of(
    project: &str,
    configuration_owner: &str,
    configuration_repository: Option<&str>,
    applications_owner: &str,
    row: &Value,
) -> Option<&'static str> {
    let owner = row["Owner"].as_str()?;
    let name = row["Name"].as_str()?;
    if owner == applications_owner
        && name
            .strip_prefix(project)
            .and_then(|rest| rest.strip_prefix('_'))
            .is_some_and(|app| !app.is_empty())
    {
        return Some("application");
    }
    if owner != configuration_owner {
        return None;
    }
    if configuration_repository == Some(name) {
        return Some("configuration");
    }
    (row["Project"].as_str() == Some(project)).then_some("project")
}

/// Every repository `project` owns, the configuration first, then by name.
async fn owned(state: &AppState, project: &str) -> Result<Vec<Owned>, ApiError> {
    let forge = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("the git forge is not configured".into()))?;
    let configuration = (*forge).clone();
    let applications = configuration.applications();
    let project_repository = configuration
        .is_project_repository()
        .then(|| configuration.repo.clone());
    let mut readers = vec![configuration.clone()];
    if applications.owner != configuration.owner {
        readers.push(applications.clone());
    }
    let mut found: Vec<Owned> = Vec::new();
    for reader in readers {
        let answer = odata(&reader, "Repositories", &[("$top", "500")]).await?;
        for row in answer["value"].as_array().into_iter().flatten() {
            let role = role_of(
                project,
                &configuration.owner,
                project_repository.as_deref(),
                &applications.owner,
                row,
            );
            let duplicate = found.iter().any(|known| known.row["Id"] == row["Id"]);
            if let (Some(role), false) = (role, duplicate) {
                found.push(Owned {
                    row: row.clone(),
                    role,
                    client: reader.clone(),
                });
            }
        }
    }
    found.sort_by(|a, b| {
        (a.role != "configuration", a.name()).cmp(&(b.role != "configuration", b.name()))
    });
    Ok(found)
}

fn not_found(project: &str, repository: &str) -> ApiError {
    ApiError::NotFound(format!(
        "repository '{repository}' not found in project '{project}'"
    ))
}

/// The repository `name` of `project`, or 404 when the project does not own one by that name.
async fn repository(state: &AppState, project: &str, name: &str) -> Result<Owned, ApiError> {
    owned(state, project)
        .await?
        .into_iter()
        .find(|repo| repo.name() == name)
        .ok_or_else(|| not_found(project, name))
}

/// One folder's entries at `git_ref`: the root tree, then each folder's own tree by its id.
async fn folder(repo: &Owned, git_ref: &str, segments: &[&str]) -> Result<Vec<Value>, ApiError> {
    let path = format!("Repositories('{}')/Tree", repo.id()?);
    let mut answer = odata(&repo.client, &path, &[("Ref", git_ref)]).await?;
    for (depth, segment) in segments.iter().enumerate() {
        let sha = answer["Tree"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["Path"] == *segment && entry["Type"] == "tree")
            .and_then(|entry| entry["Sha"].as_str())
            .filter(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_owned)
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "folder '{}' not found at '{git_ref}'",
                    segments[..=depth].join("/")
                ))
            })?;
        answer = odata(&repo.client, &path, &[("Ref", &sha)]).await?;
    }
    Ok(answer["Tree"].as_array().cloned().unwrap_or_default())
}

/// A file's bytes at `git_ref`, or `None` when it is not there.
async fn blob(repo: &Owned, git_ref: &str, path: &str) -> Result<Option<Vec<u8>>, ApiError> {
    let at = format!("Repositories('{}')/File", repo.id()?);
    match odata(&repo.client, &at, &[("Ref", git_ref), ("Path", path)]).await {
        Ok(answer) => {
            let encoded = answer["Content"].as_str().unwrap_or_default();
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map(Some)
                .map_err(|_| {
                    ApiError::Internal("the forge answered a file that does not decode".into())
                })
        }
        Err(ApiError::NotFound(_)) => Ok(None),
        Err(other) => Err(other),
    }
}

/// Whether bytes are shown as text: valid UTF-8 with no NUL in the first 8,000 bytes, as git
/// itself decides.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0) || std::str::from_utf8(bytes).is_err()
}

// ---------------------------------------------------------------------------------------------
// The diff
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Keep,
    Delete,
    Insert,
}

/// The shortest edit script from `a` to `b` (Myers 1986), or `None` past [`MAX_DIFF_EDITS`].
fn edits(a: &[&str], b: &[&str]) -> Option<Vec<Edit>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = n + m;
    let offset = max + 1;
    let mut v = vec![0isize; (2 * max + 3) as usize];
    // trace[d] holds v over -(d+1)..=(d+1) as it stood before round d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let at = |k: isize| (k + offset) as usize;
    for d in 0..=max.min(MAX_DIFF_EDITS as isize) {
        trace.push(v[at(-d - 1)..=at(d + 1)].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m));
            }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Edit> {
    let mut script = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let get = |k: isize| v[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            script.push(Edit::Keep);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            script.push(if x == prev_x {
                Edit::Insert
            } else {
                Edit::Delete
            });
        }
        x = prev_x;
        y = prev_y;
    }
    script.reverse();
    script
}

/// The unified diff of two texts with three lines of context, as `git diff` writes a file's
/// hunks; empty when they are equal.
pub fn unified(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().take(MAX_DIFF_LINES).collect();
    let b: Vec<&str> = new.lines().take(MAX_DIFF_LINES).collect();
    let script = edits(&a, &b).unwrap_or_else(|| {
        // Too far apart to follow: the whole file out, the whole file in.
        let mut whole = vec![Edit::Delete; a.len()];
        whole.extend(vec![Edit::Insert; b.len()]);
        whole
    });
    // Each edit with the line numbers it stands at in `a` and `b`.
    let mut rows: Vec<(Edit, usize, usize)> = Vec::with_capacity(script.len());
    let (mut i, mut j) = (0, 0);
    for edit in script {
        rows.push((edit, i, j));
        match edit {
            Edit::Keep => {
                i += 1;
                j += 1;
            }
            Edit::Delete => i += 1,
            Edit::Insert => j += 1,
        }
    }
    const CONTEXT: usize = 3;
    let changed: Vec<usize> = (0..rows.len())
        .filter(|at| rows[*at].0 != Edit::Keep)
        .collect();
    let mut out = String::new();
    let mut at = 0;
    while at < changed.len() {
        let start = changed[at].saturating_sub(CONTEXT);
        let mut end = changed[at];
        while at + 1 < changed.len() && changed[at + 1] <= end + 2 * CONTEXT + 1 {
            at += 1;
            end = changed[at];
        }
        let end = (end + CONTEXT + 1).min(rows.len());
        let hunk = &rows[start..end];
        let old_len = hunk.iter().filter(|r| r.0 != Edit::Insert).count();
        let new_len = hunk.iter().filter(|r| r.0 != Edit::Delete).count();
        let old_start = hunk[0].1 + usize::from(old_len > 0);
        let new_start = hunk[0].2 + usize::from(new_len > 0);
        out.push_str(&format!(
            "@@ -{old_start},{old_len} +{new_start},{new_len} @@\n"
        ));
        for (edit, i, j) in hunk {
            let (mark, line) = match edit {
                Edit::Keep => (' ', a[*i]),
                Edit::Delete => ('-', a[*i]),
                Edit::Insert => ('+', b[*j]),
            };
            out.push(mark);
            out.push_str(line);
            out.push('\n');
        }
        at += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The reads
// ---------------------------------------------------------------------------------------------

/// An activity row for one read: who read what of which repository (OPS-48).
async fn record(state: &AppState, caller: &Caller, project: &str, repository: &str, what: &str) {
    let actor = &caller.identity.username;
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "repository.read".to_owned(),
        source: "portal".to_owned(),
        summary: format!("{actor} read {what} of the repository {repository}"),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "object": format!("repositories/{repository}"), "actor": actor, "read": what }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "a repository read was not recorded");
    }
}

async fn list(state: &AppState, project: &str, input: ListInput) -> Result<Value, ApiError> {
    let words = input.filter.unwrap_or_default().to_lowercase();
    let matching: Vec<Value> = owned(state, project)
        .await?
        .iter()
        .filter(|repo| {
            words.is_empty()
                || repo.name().to_lowercase().contains(&words)
                || repo.field("Description").to_lowercase().contains(&words)
        })
        .map(Owned::summary)
        .collect();
    let (cursor, limit) = page(input.cursor, input.limit);
    Ok(paged(matching, cursor, limit))
}

fn paged(items: Vec<Value>, cursor: usize, limit: usize) -> Value {
    let total = items.len();
    let items: Vec<Value> = items.into_iter().skip(cursor).take(limit).collect();
    let mut answer = json!({ "items": items, "total": total });
    if cursor + limit < total {
        answer["nextCursor"] = json!(cursor + limit);
    }
    answer
}

/// The newest commit at `git_ref`, as `{sha, message, author, date}`.
async fn last_commit(repo: &Owned, git_ref: &str) -> Result<Option<Value>, ApiError> {
    let at = format!("Repositories('{}')/History", repo.id()?);
    let answer = odata(&repo.client, &at, &[("Ref", git_ref)]).await?;
    Ok(answer["value"].get(0).map(commit_row))
}

fn commit_row(commit: &Value) -> Value {
    let message = commit["Message"].as_str().unwrap_or_default();
    json!({
        "sha": commit["Sha"],
        "message": message.lines().next().unwrap_or_default(),
        "author": commit["Author"],
        "date": commit["Date"],
    })
}

async fn get(state: &AppState, project: &str, name: &str) -> Result<Value, ApiError> {
    let repo = repository(state, project, name).await?;
    let branch = repo.default_branch().to_owned();
    let mut answer = repo.summary();
    answer["cloneUrl"] = json!(repo.clone_url());
    // An empty repository has no branch yet: no commit and no README, not an error.
    let root = match folder(&repo, &branch, &[]).await {
        Ok(root) => root,
        Err(ApiError::NotFound(_)) => Vec::new(),
        Err(other) => return Err(other),
    };
    answer["empty"] = json!(root.is_empty());
    if !root.is_empty() {
        answer["lastCommit"] = last_commit(&repo, &branch).await?.unwrap_or(Value::Null);
    }
    let readme = root.iter().find(|entry| {
        entry["Type"] == "blob"
            && ["readme.md", "readme", "readme.txt"].contains(
                &entry["Path"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str(),
            )
    });
    if let Some(entry) = readme {
        let path = entry["Path"].as_str().unwrap_or_default();
        if entry["Size"].as_u64().unwrap_or(u64::MAX) <= MAX_README_BYTES {
            if let Some(bytes) = blob(&repo, &branch, path).await? {
                if !is_binary(&bytes) {
                    answer["readme"] = json!({
                        "path": path,
                        "text": String::from_utf8_lossy(&bytes),
                    });
                }
            }
        }
    }
    Ok(answer)
}

/// The entries of one folder, folders first, each with its whole path.
async fn tree(state: &AppState, project: &str, input: PathInput) -> Result<Value, ApiError> {
    let segments = path_segments(input.path.as_deref().unwrap_or_default())
        .map_err(OpError::into_api_error)?;
    let git_ref = checked_ref(input.git_ref.as_deref()).map_err(OpError::into_api_error)?;
    let repo = repository(state, project, &input.repository).await?;
    let git_ref = repo.reference(git_ref).to_owned();
    let prefix = segments.join("/");
    let mut entries: Vec<Value> = folder(&repo, &git_ref, &segments)
        .await?
        .iter()
        .map(|entry| {
            let name = entry["Path"].as_str().unwrap_or_default();
            let kind = match entry["Type"].as_str() {
                Some("tree") => "folder",
                Some("commit") => "submodule",
                _ => "file",
            };
            json!({
                "name": name,
                "path": if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") },
                "type": kind,
                "size": entry["Size"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        (a["type"] != "folder", a["name"].as_str())
            .cmp(&(b["type"] != "folder", b["name"].as_str()))
    });
    let truncated = entries.len() > MAX_TREE_ENTRIES;
    entries.truncate(MAX_TREE_ENTRIES);
    Ok(json!({
        "repository": repo.name(),
        "ref": git_ref,
        "path": prefix,
        "entries": entries,
        "truncated": truncated,
    }))
}

/// One file: its text when it is text and small enough, else its size and why it is not shown.
/// The text is data: a page draws it as characters, never as markup.
async fn file(state: &AppState, project: &str, input: PathInput) -> Result<Value, ApiError> {
    let path = input.path.as_deref().unwrap_or_default();
    let segments = path_segments(path).map_err(OpError::into_api_error)?;
    let Some((leaf, parents)) = segments.split_last() else {
        return Err(invalid("/path", "the path names no file").into_api_error());
    };
    let git_ref = checked_ref(input.git_ref.as_deref()).map_err(OpError::into_api_error)?;
    let repo = repository(state, project, &input.repository).await?;
    let git_ref = repo.reference(git_ref).to_owned();
    let path = segments.join("/");
    let entry = folder(&repo, &git_ref, parents)
        .await?
        .into_iter()
        .find(|entry| entry["Path"] == *leaf && entry["Type"] == "blob")
        .ok_or_else(|| ApiError::NotFound(format!("file '{path}' not found at '{git_ref}'")))?;
    let size = entry["Size"].as_u64().unwrap_or(0);
    let mut answer = json!({
        "repository": repo.name(),
        "ref": git_ref,
        "path": path,
        "size": size,
        "binary": false,
        "tooLarge": size > MAX_FILE_BYTES,
    });
    if size > MAX_FILE_BYTES {
        return Ok(answer);
    }
    let bytes = blob(&repo, &git_ref, &path)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("file '{path}' not found at '{git_ref}'")))?;
    if is_binary(&bytes) {
        answer["binary"] = json!(true);
    } else {
        answer["text"] = json!(String::from_utf8_lossy(&bytes));
    }
    Ok(answer)
}

async fn commits(state: &AppState, project: &str, input: CommitsInput) -> Result<Value, ApiError> {
    let segments = path_segments(input.path.as_deref().unwrap_or_default())
        .map_err(OpError::into_api_error)?;
    let git_ref = checked_ref(input.git_ref.as_deref()).map_err(OpError::into_api_error)?;
    let repo = repository(state, project, &input.repository).await?;
    let git_ref = repo.reference(git_ref).to_owned();
    let (cursor, limit) = page(input.cursor, input.limit);
    let id = repo.id()?.to_owned();
    if !segments.is_empty() {
        // The history of one path is the forge's newest fifty commits touching it.
        let path = segments.join("/");
        let at = format!("Repositories('{id}')/History");
        let answer = odata(&repo.client, &at, &[("Ref", &git_ref), ("Path", &path)]).await?;
        let rows: Vec<Value> = answer["value"]
            .as_array()
            .into_iter()
            .flatten()
            .map(commit_row)
            .collect();
        let mut answer = paged(rows, cursor, limit);
        answer["ref"] = json!(git_ref);
        return Ok(answer);
    }
    let filter = format!("RepoId eq '{id}' and Ref eq '{git_ref}'");
    let (top, skip) = (limit.to_string(), cursor.to_string());
    let answer = odata(
        &repo.client,
        "Commits",
        &[
            ("$filter", &filter),
            // The walk's order: a page without $orderby comes back by key, the commits' shas.
            ("$orderby", "Position"),
            ("$top", &top),
            ("$skip", &skip),
            ("$count", "true"),
        ],
    )
    .await?;
    let items: Vec<Value> = answer["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| json!({ "sha": c["Sha"], "message": c["Message"], "author": c["Author"], "date": c["Date"] }))
        .collect();
    let total = answer["@odata.count"]
        .as_u64()
        .map_or(cursor + items.len(), |t| t as usize);
    let mut out = json!({ "items": items, "total": total, "ref": git_ref });
    if cursor + limit < total {
        out["nextCursor"] = json!(cursor + limit);
    }
    Ok(out)
}

/// One commit with the files it changed and, for the first thirty, their patch.
async fn commit(state: &AppState, project: &str, input: CommitInput) -> Result<Value, ApiError> {
    let repo = repository(state, project, &input.repository).await?;
    let id = repo.id()?.to_owned();
    let history = odata(
        &repo.client,
        &format!("Repositories('{id}')/History"),
        &[("Ref", &input.sha)],
    )
    .await?;
    let info = history["value"]
        .get(0)
        .filter(|c| c["Sha"] == input.sha)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("commit '{}' not found", input.sha)))?;
    let parent = info["Parents"]
        .get(0)
        .and_then(Value::as_str)
        .map(str::to_owned);
    // The files the commit changed against its first parent; a root commit adds its whole tree.
    let changed: Vec<(String, String)> = match &parent {
        Some(parent) => odata(
            &repo.client,
            &format!("Repositories('{id}')/Compare"),
            &[("Base", parent), ("Head", &input.sha)],
        )
        .await?["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| {
                (
                    f["filename"].as_str().unwrap_or_default().to_owned(),
                    f["status"].as_str().unwrap_or("modified").to_owned(),
                )
            })
            .collect(),
        None => odata(
            &repo.client,
            &format!("Repositories('{id}')/Tree"),
            &[("Ref", &input.sha), ("Recursive", "true")],
        )
        .await?["Tree"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["Type"] == "blob")
            .map(|e| {
                (
                    e["Path"].as_str().unwrap_or_default().to_owned(),
                    "added".to_owned(),
                )
            })
            .collect(),
    };
    let mut files = Vec::new();
    let mut spent = 0usize;
    let mut truncated = false;
    for (index, (path, status)) in changed.iter().enumerate() {
        let mut row = json!({ "path": path, "status": status });
        if index >= MAX_PATCH_FILES || spent >= MAX_PATCH_BYTES {
            truncated = true;
            files.push(row);
            continue;
        }
        let before = match (&parent, status.as_str()) {
            (Some(parent), "modified" | "deleted") => blob(&repo, parent, path).await?,
            _ => None,
        };
        let after = if status == "deleted" {
            None
        } else {
            blob(&repo, &input.sha, path).await?
        };
        let sides = [before.as_deref(), after.as_deref()];
        if sides.iter().flatten().any(|bytes| is_binary(bytes)) {
            row["binary"] = json!(true);
        } else {
            let text = |side: Option<&[u8]>| {
                side.map(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_default()
            };
            let patch = unified(&text(sides[0]), &text(sides[1]));
            spent += patch.len();
            row["patch"] = json!(patch);
        }
        files.push(row);
    }
    Ok(json!({
        "repository": repo.name(),
        "sha": info["Sha"],
        "message": info["Message"],
        "author": info["Author"],
        "email": info["Email"],
        "date": info["Date"],
        "parents": info["Parents"],
        "files": files,
        "truncated": truncated,
    }))
}

async fn refs(state: &AppState, project: &str, name: &str) -> Result<Value, ApiError> {
    let repo = repository(state, project, name).await?;
    let filter = format!("RepoId eq '{}'", repo.id()?);
    let query = [("$filter", filter.as_str()), ("$top", "500")];
    let branches = odata(&repo.client, "Branches", &query).await?;
    let tags = odata(&repo.client, "Tags", &query).await?;
    let default = repo.default_branch();
    let mut branches: Vec<Value> = branches["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            json!({
                "name": b["Name"],
                "headSha": b["HeadSha"],
                "protected": b["Protected"].as_bool().unwrap_or(false),
                "default": b["Name"] == default,
            })
        })
        .collect();
    branches.sort_by(|a, b| {
        (a["default"] != true, a["name"].as_str()).cmp(&(b["default"] != true, b["name"].as_str()))
    });
    let tags: Vec<Value> = tags["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| json!({ "name": t["Name"], "targetSha": t["TargetSha"] }))
        .collect();
    Ok(json!({ "repository": repo.name(), "branches": branches, "tags": tags }))
}

fn pull_row(pr: &Value) -> Value {
    json!({
        "number": pr["Number"],
        "title": pr["Title"],
        "state": if pr["Merged"] == true { "merged" } else { pr["State"].as_str().unwrap_or("open") },
        "author": pr["Author"],
        "head": pr["Head"],
        "base": pr["Base"],
        "createdAt": pr["CreatedAt"],
        "closedAt": pr["ClosedAt"],
    })
}

async fn pulls(state: &AppState, project: &str, input: PullsInput) -> Result<Value, ApiError> {
    let repo = repository(state, project, &input.repository).await?;
    let mut filter = format!("RepoId eq '{}'", repo.id()?);
    match input.state.as_deref().unwrap_or("open") {
        "open" => filter.push_str(" and State eq 'open'"),
        "closed" => filter.push_str(" and State ne 'open'"),
        _ => {}
    }
    let (cursor, limit) = page(input.cursor, input.limit);
    let (top, skip) = (limit.to_string(), cursor.to_string());
    let answer = odata(
        &repo.client,
        "PullRequests",
        &[
            ("$filter", &filter),
            ("$orderby", "Number desc"),
            ("$top", &top),
            ("$skip", &skip),
            ("$count", "true"),
        ],
    )
    .await?;
    let items: Vec<Value> = answer["value"]
        .as_array()
        .into_iter()
        .flatten()
        .map(pull_row)
        .collect();
    let total = answer["@odata.count"]
        .as_u64()
        .map_or(cursor + items.len(), |t| t as usize);
    let mut out = json!({ "items": items, "total": total });
    if cursor + limit < total {
        out["nextCursor"] = json!(cursor + limit);
    }
    Ok(out)
}

async fn pull(state: &AppState, project: &str, input: PullInput) -> Result<Value, ApiError> {
    let repo = repository(state, project, &input.repository).await?;
    let at = format!("PullRequests('{}:{}')", repo.id()?, input.number);
    let pr = match odata(&repo.client, &at, &[("$expand", "Files,Reviews")]).await {
        Err(ApiError::NotFound(_)) => {
            return Err(ApiError::NotFound(format!(
                "pull request {} not found in '{}'",
                input.number,
                repo.name()
            )))
        }
        other => other?,
    };
    let mut answer = pull_row(&pr);
    answer["repository"] = json!(repo.name());
    answer["body"] = json!(pr["Body"].as_str().unwrap_or_default());
    answer["files"] = json!(pr["Files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| json!({ "path": f["Filename"], "status": f["Status"] }))
        .collect::<Vec<_>>());
    answer["reviews"] = json!(pr["Reviews"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| json!({ "author": r["Author"], "verdict": r["Verdict"], "body": r["Body"], "commit": r["CommitId"] }))
        .collect::<Vec<_>>());
    // A pull request of the project's own configuration repository is a Change the Portal
    // shows on its approval page (CC-87).
    if repo.role == "configuration" {
        let forge = repo.client.clone();
        answer["change"] =
            json!(crate::api::changes::change_meta(state, &forge, input.number, project).name);
    }
    Ok(answer)
}

/// A webhook's address as an administrator reads it: scheme, host and path, never a user,
/// password or query, where a secret could ride along.
pub fn shown_address(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(url) => {
            format!(
                "{}://{}{}",
                url.scheme(),
                url.host_str().unwrap_or_default(),
                url.port().map(|p| format!(":{p}")).unwrap_or_default()
            ) + url.path()
        }
        Err(_) => "(an address that does not parse)".to_owned(),
    }
}

async fn settings(state: &AppState, project: &str, name: &str) -> Result<Value, ApiError> {
    let repo = repository(state, project, name).await?;
    let by_repo = format!("RepoId eq '{}'", repo.id()?);
    let query = [("$filter", by_repo.as_str()), ("$top", "500")];
    let rows = |answer: Value| answer["value"].as_array().cloned().unwrap_or_default();
    let protection: Vec<Value> = rows(odata(&repo.client, "ProtectionRules", &query).await?)
        .iter()
        .map(|r| {
            json!({
                "pattern": r["Pattern"],
                "pushAllowed": r["EnablePush"],
                "forcePushAllowed": r["EnableForcePush"],
                "requiredApprovals": r["RequiredApprovals"],
                "blockOnRejectedReviews": r["BlockOnRejectedReviews"],
                "blockOnOutdatedBranch": r["BlockOnOutdatedBranch"],
            })
        })
        .collect();
    // The secret is write-only: shako never declares it, and only these members are copied.
    let webhooks: Vec<Value> = rows(odata(&repo.client, "Webhooks", &query).await?)
        .iter()
        .map(|h| {
            json!({
                "address": shown_address(h["Url"].as_str().unwrap_or_default()),
                "active": h["Active"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    let collaborators: Vec<Value> = rows(odata(&repo.client, "Teams", &query).await?)
        .iter()
        .map(|t| json!({ "group": t["Group"], "access": t["Verb"] }))
        .collect();
    let by_package = format!(
        "Owner eq '{}' and Name eq '{}'",
        repo.field("Owner"),
        repo.name()
    );
    let packages: Vec<Value> = rows(
        odata(
            &repo.client,
            "PackageVersions",
            &[("$filter", by_package.as_str()), ("$top", "100")],
        )
        .await?,
    )
    .iter()
    .map(|v| {
        json!({
            "type": v["Type"],
            "name": v["Name"],
            "version": v["Version"],
            "digest": v["Digest"],
            "size": v["Size"],
            "signed": v["Signed"].as_bool().unwrap_or(false),
            "createdAt": v["CreatedAt"],
        })
    })
    .collect();
    Ok(json!({
        "repository": repo.name(),
        "protection": protection,
        "webhooks": webhooks,
        "collaborators": collaborators,
        "packages": packages,
    }))
}

// ---------------------------------------------------------------------------------------------
// Output schemas
// ---------------------------------------------------------------------------------------------

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn summary_schema() -> Value {
    object(
        json!({
            "name": { "type": "string" },
            "owner": { "type": "string" },
            "role": { "type": "string", "enum": ["configuration", "project", "application"] },
            "description": { "type": "string" },
            "defaultBranch": { "type": "string" },
            "private": { "type": "boolean" },
            "archived": { "type": "boolean" },
            "sizeBytes": { "type": "integer" },
            "updatedAt": { "type": "string" }
        }),
        &["name", "role", "defaultBranch"],
    )
}

fn commit_row_schema() -> Value {
    object(
        json!({
            "sha": { "type": "string" },
            "message": { "type": "string" },
            "author": { "type": "string" },
            "date": { "type": "string" }
        }),
        &["sha"],
    )
}

fn page_of(item: Value) -> Value {
    object(
        json!({
            "items": { "type": "array", "items": item },
            "total": { "type": "integer" },
            "nextCursor": { "type": "integer" }
        }),
        &["items", "total"],
    )
}

fn list_output() -> Value {
    page_of(summary_schema())
}

fn get_output() -> Value {
    let mut schema = summary_schema();
    let properties = schema["properties"].as_object_mut().expect("object");
    properties.insert("cloneUrl".into(), json!({ "type": "string" }));
    properties.insert("empty".into(), json!({ "type": "boolean" }));
    properties.insert("lastCommit".into(), commit_row_schema());
    properties.insert(
        "readme".into(),
        object(
            json!({ "path": { "type": "string" }, "text": { "type": "string" } }),
            &["path", "text"],
        ),
    );
    schema
}

fn tree_output() -> Value {
    object(
        json!({
            "repository": { "type": "string" },
            "ref": { "type": "string" },
            "path": { "type": "string" },
            "truncated": { "type": "boolean" },
            "entries": { "type": "array", "items": object(json!({
                "name": { "type": "string" },
                "path": { "type": "string" },
                "type": { "type": "string", "enum": ["folder", "file", "submodule"] },
                "size": { "type": "integer" }
            }), &["name", "path", "type"]) }
        }),
        &["repository", "ref", "path", "entries", "truncated"],
    )
}

fn file_output() -> Value {
    object(
        json!({
            "repository": { "type": "string" },
            "ref": { "type": "string" },
            "path": { "type": "string" },
            "size": { "type": "integer" },
            "binary": { "type": "boolean" },
            "tooLarge": { "type": "boolean" },
            "text": { "type": "string", "description": "The file as text, absent when binary or too large" }
        }),
        &["repository", "ref", "path", "size", "binary", "tooLarge"],
    )
}

fn commits_output() -> Value {
    let mut schema = page_of(commit_row_schema());
    schema["properties"]["ref"] = json!({ "type": "string" });
    schema
}

fn commit_output() -> Value {
    object(
        json!({
            "repository": { "type": "string" },
            "sha": { "type": "string" },
            "message": { "type": "string" },
            "author": { "type": "string" },
            "email": { "type": "string" },
            "date": { "type": "string" },
            "parents": { "type": "array", "items": { "type": "string" } },
            "truncated": { "type": "boolean" },
            "files": { "type": "array", "items": object(json!({
                "path": { "type": "string" },
                "status": { "type": "string" },
                "binary": { "type": "boolean" },
                "patch": { "type": "string", "description": "Unified diff hunks" }
            }), &["path", "status"]) }
        }),
        &["repository", "sha", "files", "truncated"],
    )
}

fn refs_output() -> Value {
    object(
        json!({
            "repository": { "type": "string" },
            "branches": { "type": "array", "items": object(json!({
                "name": { "type": "string" },
                "headSha": { "type": "string" },
                "protected": { "type": "boolean" },
                "default": { "type": "boolean" }
            }), &["name"]) },
            "tags": { "type": "array", "items": object(json!({
                "name": { "type": "string" },
                "targetSha": { "type": "string" }
            }), &["name"]) }
        }),
        &["repository", "branches", "tags"],
    )
}

fn pull_row_schema() -> Map<String, Value> {
    json!({
        "number": { "type": "integer" },
        "title": { "type": "string" },
        "state": { "type": "string", "enum": ["open", "closed", "merged"] },
        "author": { "type": "string" },
        "head": { "type": "string" },
        "base": { "type": "string" },
        "createdAt": { "type": "string" },
        "closedAt": { "type": ["string", "null"] }
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

fn pulls_output() -> Value {
    page_of(object(
        Value::Object(pull_row_schema()),
        &["number", "title", "state"],
    ))
}

fn pull_output() -> Value {
    let mut properties = pull_row_schema();
    properties.insert("repository".into(), json!({ "type": "string" }));
    properties.insert("body".into(), json!({ "type": "string" }));
    properties.insert(
        "change".into(),
        json!({ "type": "string", "description": "The Change this pull request is, on the approval page" }),
    );
    properties.insert(
        "files".into(),
        json!({ "type": "array", "items": object(json!({ "path": { "type": "string" }, "status": { "type": "string" } }), &["path"]) }),
    );
    properties.insert(
        "reviews".into(),
        json!({ "type": "array", "items": object(json!({
            "author": { "type": "string" },
            "verdict": { "type": "string" },
            "body": { "type": "string" },
            "commit": { "type": "string" }
        }), &["verdict"]) }),
    );
    object(
        Value::Object(properties),
        &["number", "title", "state", "files", "reviews"],
    )
}

fn settings_output() -> Value {
    object(
        json!({
            "repository": { "type": "string" },
            "protection": { "type": "array", "items": { "type": "object" } },
            "webhooks": { "type": "array", "items": object(json!({
                "address": { "type": "string", "description": "Scheme, host and path; the secret is never read back" },
                "active": { "type": "boolean" }
            }), &["address", "active"]) },
            "collaborators": { "type": "array", "items": object(json!({
                "group": { "type": "string" },
                "access": { "type": "string" }
            }), &["group"]) },
            "packages": { "type": "array", "items": { "type": "object" } }
        }),
        &[
            "repository",
            "protection",
            "webhooks",
            "collaborators",
            "packages",
        ],
    )
}

// ---------------------------------------------------------------------------------------------
// The registry entries
// ---------------------------------------------------------------------------------------------

const READ: OperationAnnotations = OperationAnnotations {
    read_only_hint: true,
    destructive_hint: false,
    idempotent_hint: true,
};

/// The settings' gate: approve on the project itself, as its administrators and stewards hold it.
/// The registry already answered a caller who reads nothing here with 404 (PF-59), so the 403
/// names the project only to a member.
pub fn may_administer(state: &AppState, caller: &Caller, project: &str) -> Result<(), OpError> {
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    if effective.may("Project", jc_core::kinds::Verb::Approve) {
        Ok(())
    } else {
        Err(OpError::Api(ApiError::Denied(format!(
            "a repository's settings need approve on Project in project {project} (PF-50)"
        ))))
    }
}

/// The name the activity row and the 404 carry, from an input already validated.
fn named(input: &Value) -> String {
    input["repository"].as_str().unwrap_or_default().to_owned()
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: LIST,
            title: "List Repositories",
            description: "The project's repositories on the forge: its configuration, its own and its applications'",
            input: list_input,
            output: list_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ListInput>(val.clone()).map(|_| ()),
            run: |_caller, state, project, val| {
                Box::pin(async move { Ok(list(state, project, parse_input(val)?).await?) })
            },
        },
        Operation {
            name: GET,
            title: "Read A Repository",
            description: "One repository: description, default branch, clone address, size, newest commit and README",
            input: repository_input,
            output: get_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<RepositoryInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: RepositoryInput = parse_input(val)?;
                    let answer = get(state, project, &input.repository).await?;
                    record(state, caller, project, &input.repository, "the summary").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: TREE,
            title: "List A Folder",
            description: "The files and folders of one folder of a repository at a branch, tag or commit",
            input: tree_input,
            output: tree_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                let input = parse_input::<PathInput>(val.clone())?;
                path_segments(input.path.as_deref().unwrap_or_default())?;
                checked_ref(input.git_ref.as_deref()).map(|_| ())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let name = named(&val);
                    let answer = tree(state, project, parse_input(val)?).await?;
                    record(state, caller, project, &name, "a folder").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: FILE,
            title: "Read A File",
            description: "One file of a repository: its text, or its size when it is binary or larger than 1 MiB",
            input: file_input,
            output: file_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                let input = parse_input::<PathInput>(val.clone())?;
                path_segments(input.path.as_deref().unwrap_or_default())?;
                checked_ref(input.git_ref.as_deref()).map(|_| ())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let name = named(&val);
                    let answer = file(state, project, parse_input(val)?).await?;
                    record(state, caller, project, &name, "a file").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: COMMITS,
            title: "List Commits",
            description: "A repository's commits at a branch, tag or commit, newest first, optionally of one path",
            input: commits_input,
            output: commits_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                let input = parse_input::<CommitsInput>(val.clone())?;
                path_segments(input.path.as_deref().unwrap_or_default())?;
                checked_ref(input.git_ref.as_deref()).map(|_| ())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let name = named(&val);
                    let answer = commits(state, project, parse_input(val)?).await?;
                    record(state, caller, project, &name, "the commits").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: COMMIT,
            title: "Read A Commit",
            description: "One commit: message, author, parents and the files it changed with their diff",
            input: commit_input,
            output: commit_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<CommitInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let name = named(&val);
                    let answer = commit(state, project, parse_input(val)?).await?;
                    record(state, caller, project, &name, "a commit").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: REFS,
            title: "List Branches And Tags",
            description: "A repository's branches, the default first, and its tags",
            input: repository_input,
            output: refs_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<RepositoryInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: RepositoryInput = parse_input(val)?;
                    let answer = refs(state, project, &input.repository).await?;
                    record(state, caller, project, &input.repository, "the branches").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: PULLS,
            title: "List Pull Requests",
            description: "A repository's pull requests, newest first: open, closed or all",
            input: pulls_input,
            output: pulls_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<PullsInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let name = named(&val);
                    let answer = pulls(state, project, parse_input(val)?).await?;
                    record(state, caller, project, &name, "the pull requests").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: PULL,
            title: "Read A Pull Request",
            description: "One pull request with its files and reviews, and the Change it is when the Portal opened it",
            input: pull_input,
            output: pull_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<PullInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let name = named(&val);
                    let answer = pull(state, project, parse_input(val)?).await?;
                    record(state, caller, project, &name, "a pull request").await;
                    Ok(answer)
                })
            },
        },
        Operation {
            name: SETTINGS,
            title: "Read Repository Settings",
            description: "For who approves the project: branch protection, webhooks, collaborators and packages",
            input: repository_input,
            output: settings_output,
            annotations: READ,
            kind: "Project",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<RepositoryInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: RepositoryInput = parse_input(val)?;
                    may_administer(state, caller, project)?;
                    let answer = settings(state, project, &input.repository).await?;
                    record(state, caller, project, &input.repository, "the settings").await;
                    Ok(answer)
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_traversal_or_an_odd_path_is_refused() {
        for path in [
            "../etc/passwd",
            "a/../../b",
            "/etc",
            "a//b",
            "a/./b",
            "a\\b",
            "a\u{0}b",
        ] {
            assert!(path_segments(path).is_err(), "{path:?}");
        }
        assert_eq!(
            path_segments("src/pages/").expect("ok"),
            vec!["src", "pages"]
        );
        assert!(path_segments("").expect("root").is_empty());
        assert!(checked_ref(Some("main..dev")).is_err());
        assert!(checked_ref(Some("feature/x")).is_ok());
    }

    #[test]
    fn a_repository_is_the_projects_only_by_its_rule() {
        let row = |owner: &str, name: &str, project: &str| json!({ "Owner": owner, "Name": name, "Project": project });
        let role = |r: Value| role_of("ovzdusie", "org", Some("ovzdusie-config"), "apps", &r);
        assert_eq!(
            role(row("org", "ovzdusie-config", "")),
            Some("configuration")
        );
        assert_eq!(role(row("org", "tools", "ovzdusie")), Some("project"));
        assert_eq!(role(row("apps", "ovzdusie_dash", "")), Some("application"));
        // Another project's, a prefix that is not the project, the shared repository, another
        // owner's: none of them.
        assert_eq!(role(row("org", "tools", "doprava")), None);
        assert_eq!(role(row("apps", "ovzdusiex_dash", "")), None);
        assert_eq!(role(row("apps", "ovzdusie_", "")), None);
        assert_eq!(role(row("org", "configuration", "")), None);
        assert_eq!(role(row("elsewhere", "ovzdusie-config", "ovzdusie")), None);
    }

    #[test]
    fn a_unified_diff_is_what_git_writes() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\ni\n";
        assert_eq!(
            unified(old, new),
            "@@ -1,8 +1,9 @@\n a\n b\n c\n-d\n+D\n e\n f\n g\n h\n+i\n"
        );
        assert_eq!(unified("same\n", "same\n"), "");
        assert_eq!(unified("", "new\n"), "@@ -0,0 +1,1 @@\n+new\n");
        assert_eq!(unified("gone\n", ""), "@@ -1,1 +0,0 @@\n-gone\n");
        // Two changes far apart are two hunks.
        let long: String = (0..20).map(|i| format!("{i}\n")).collect();
        let changed = long.replace("2\n", "two\n").replace("17\n", "seventeen\n");
        let patch = unified(&long, &changed);
        assert_eq!(patch.matches("@@ -").count(), 2, "{patch}");
    }

    #[test]
    fn a_diff_past_its_bound_shows_the_file_replaced() {
        let old: String = (0..3000).map(|i| format!("a{i}\n")).collect();
        let new: String = (0..3000).map(|i| format!("b{i}\n")).collect();
        let patch = unified(&old, &new);
        assert_eq!(patch.lines().filter(|l| l.starts_with('-')).count(), 3000);
        assert_eq!(patch.lines().filter(|l| l.starts_with('+')).count(), 3000);
    }

    #[test]
    fn binary_is_a_nul_or_bytes_that_are_no_text() {
        assert!(is_binary(b"PNG\0\x01"));
        assert!(is_binary(&[0xff, 0xfe, 0x41]));
        assert!(!is_binary("<script>alert(1)</script> ž".as_bytes()));
    }

    #[test]
    fn a_webhook_address_never_shows_its_credentials() {
        assert_eq!(
            shown_address("https://user:s3cret@hooks.example.org:8443/in?token=abc#x"),
            "https://hooks.example.org:8443/in"
        );
        assert_eq!(
            shown_address("not a url"),
            "(an address that does not parse)"
        );
    }
}
