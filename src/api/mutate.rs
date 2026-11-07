use std::hash::{Hash, Hasher};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::api::dry_run::{self, DryRunQuery, DryRunResult};
use crate::auth::session::Front;
use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangeMeta, ChangePhase, ChangeStatus, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, FileWrite, GitError};
use crate::plan;
use crate::resource::{self, ResourceEnvelope};
use crate::state::AppState;

/// Field names whose string values are credentials wherever they appear: refused on a write
/// (MF-24), redacted in a plan diff (CC-06) and dropped from an export (MF-17).
pub const SECRET_KEYS: &[&str] = &[
    "password",
    "token",
    "secret",
    "clientSecret",
    "apiKey",
    "client_secret",
    "api_key",
    // A CKAN instance is named by `apiTokenRef`; a pasted `apiToken` is the same mistake as
    // a pasted password and is refused the same way (EP-67).
    "apiToken",
    "api_token",
    // The keys a runner input writes a credential under, which the runner's own catalog did not
    // call secrets (T-2239). `key` is deliberately absent: a `secretRef` has one, and so does
    // every map in a manifest — the runner's own rule guards the paths where `key` is a private
    // key (`tls.client_certs[].key`, `jc_core::kinds::bento_inputs::SECRET_FIELDS`).
    "access_token",
    "accessToken",
    "access_token_secret",
    "accessTokenSecret",
    "consumer_secret",
    "consumerSecret",
    "auth_secret",
    "authSecret",
    "account_key",
    "accountKey",
    "bot_token",
    "botToken",
    "private_key",
    "privateKey",
    "private_key_pass",
    "privateKeyPass",
    "passphrase",
    "user_jwt",
    "userJwt",
    "user_nkey_seed",
    "userNkeySeed",
];

/// Detects string-valued literal secrets in manifest payloads (MF-24).
///
/// Returns the first offending key name if any string-valued key named `password`, `token`,
/// `secret`, `clientSecret` or `apiKey` exists at any depth. Object-valued secret references
/// (such as `secretRef: { name: "..." }`) are permitted.
pub fn find_literal_secret(val: &Value) -> Option<String> {
    match val {
        Value::Object(map) => {
            for (k, v) in map {
                // Any scalar: YAML reads `password: 123456` unquoted as a number, and it is still a
                // secret typed into a manifest (T-2540). `null` is an absent value.
                // A whole `${VAR}` names a secret the runner injects and holds none; jc-core
                // decides which names a manifest may read (PL-15, T-3972).
                let scalar = (v.is_string() && !v.as_str().is_some_and(is_variable_reference))
                    || v.is_number()
                    || v.is_boolean();
                if SECRET_KEYS.contains(&k.as_str()) && scalar {
                    return Some(k.clone());
                }
                if let Some(header) = (k == "headers").then(|| credential_header(v)).flatten() {
                    return Some(format!("headers.{header}"));
                }
                if let Some(found) = find_literal_secret(v) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for item in arr {
                if let Some(found) = find_literal_secret(item) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// The first request header in `headers` that holds a credential as plain text: a header's name
/// is the vendor's (`X-Api-Key`), so [`SECRET_KEYS`] never meets it. A value naming a `${VAR}`
/// holds none (T-6488).
fn credential_header(headers: &Value) -> Option<&str> {
    const WORDS: [&str; 6] = ["auth", "token", "secret", "password", "key", "cookie"];
    headers.as_object()?.iter().find_map(|(name, value)| {
        let plain: String = name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        let holds = value.as_str().is_some_and(|text| !text.contains("${"));
        (holds && WORDS.iter().any(|word| plain.contains(word))).then_some(name.as_str())
    })
}

/// Whether `value` is exactly one runner variable reference, `${NAME}` with `NAME` a shell name.
pub(crate) fn is_variable_reference(value: &str) -> bool {
    value
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|name| {
            name.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

fn parse_body_to_value(headers: &HeaderMap, bytes: &[u8]) -> Result<Value, ApiError> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    let mime = ct.split(';').next().unwrap_or("").trim();

    if mime.is_empty() || mime == "application/json" || mime == "text/json" {
        serde_json::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))
    } else if mime == "application/yaml" || mime == "application/x-yaml" || mime == "text/yaml" {
        serde_norway::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid yaml body: {e}")))
    } else {
        Err(ApiError::UnsupportedMediaType(format!(
            "content type '{mime}' is not supported; expected application/json or application/yaml"
        )))
    }
}

fn parse_patch_to_value(headers: &HeaderMap, bytes: &[u8]) -> Result<Value, ApiError> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime = ct.split(';').next().unwrap_or("").trim();

    if mime == "application/merge-patch+json" {
        serde_json::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid json patch: {e}")))
    } else if mime == "application/apply-patch+yaml" {
        serde_norway::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid yaml patch: {e}")))
    } else {
        Err(ApiError::UnsupportedMediaType(format!(
            "content type '{mime}' is not supported for PATCH; expected application/merge-patch+json or application/apply-patch+yaml"
        )))
    }
}

/// The branch a proposal lives on: one per project, kind, name and operation, so a retry
/// lands on the same pull request and a second proposal finds the open one (T-0883).
pub fn branch_name(project: &str, kind: &str, name: &str, operation: Operation) -> String {
    let op_str = match operation {
        Operation::Create => "create",
        Operation::Update => "update",
        Operation::Delete => "delete",
    };
    let kind_lower = kind.to_ascii_lowercase();

    let mut hasher = std::hash::DefaultHasher::new();
    (project, kind, name, operation).hash(&mut hasher);
    let hash_val = hasher.finish();
    let hex = format!("{hash_val:016x}");
    let short = &hex[..8];

    format!("portal/{op_str}-{kind_lower}-{name}-{short}")
}

/// Refuses a manifest the workspace does not cover (CC-76): another project, a resource not in
/// its list, a space not in its subtree.
pub(crate) fn within_workspace(
    workspace: &crate::ops::workspaces::Workspace,
    project: &str,
    kind: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    let name = &envelope.metadata.name;
    let outside = |what: String| {
        Err(ApiError::BadRequest(format!(
            "workspace '{}' does not cover {what}; open one that does, or propose it on its own",
            workspace.name
        )))
    };
    if workspace.project != project {
        return outside(format!("project '{project}'"));
    }
    let space = if kind == "ContextSpace" {
        None
    } else {
        crate::permissions::space_ref(&serde_json::to_value(envelope).unwrap_or_default())
    };
    if !workspace.scope.covers(kind, name, space.as_deref()) {
        return outside(format!("{kind} '{name}'"));
    }
    Ok(())
}

/// The open change whose pull request still uses `branch`, if any (T-0883, T-0886).
pub(crate) async fn open_change_on(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    branch: &str,
    project: &str,
) -> Result<Option<ChangeMeta>, ApiError> {
    // A forge that answers 404 here has no repository at all, and the next step says so.
    let pulls = match gitea.list_pull_requests("open").await {
        Ok(pulls) => pulls,
        Err(GitError::NotFound) => Vec::new(),
        Err(err) => return Err(err.into()),
    };
    // A retry after a rejection opens on `{branch}-{nonce}` (T-0887): the same resource.
    let suffixed = format!("{branch}_");
    Ok(pulls
        .into_iter()
        .find(|pr| pr.head_branch == branch || pr.head_branch.starts_with(&suffixed))
        .map(|pr| crate::api::changes::change_meta(state, gitea, pr.number, project)))
}

/// The branch a proposal is written on, starting from the default branch. A branch left by
/// an earlier attempt is recreated, never reused: it may hold that attempt's writes (a file
/// already deleted, another content), which turned a delete into a forge 404 (T-0886). One a
/// live pull request still uses is left alone and the caller told (CC-34).
/// Whether main holds the resource the checks read from the mirror otherwise, at the path the
/// change writes (T-2674): another spec, labels, annotations, title or description. Only what both
/// say for certain counts: a resource the mirror does not hold yet (the branch read covers that),
/// no file at that path (a manifest kept under another path) or one the envelope cannot read is no
/// evidence, and the namespace is left out, which a file may leave to its path. The mirror holds
/// a manifest as the loader rendered it, so main's `{orgDomain}` (CC-74) is compared rendered:
/// without that every Policy written the way the plan demands read as moved (T-6451).
fn moved_on_main(
    judged: Option<&ResourceEnvelope>,
    on_main: Option<&crate::git::RepoFile>,
    org_domain: &str,
) -> bool {
    let rendered = |file: &crate::git::RepoFile| {
        serde_norway::from_str::<ResourceEnvelope>(&file.content.replace("{orgDomain}", org_domain))
    };
    let (Some(judged), Some(Ok(main))) = (judged, on_main.map(rendered)) else {
        return false;
    };
    let (a, b) = (&judged.metadata, &main.metadata);
    judged.spec != main.spec
        || a.labels != b.labels
        || a.annotations != b.annotations
        || a.title != b.title
        || a.description != b.description
}

/// The refusal of a create whose file is already there (T-2855): what holds the path, and what to
/// do instead, so nobody's manifest is replaced by a change that reads as a create.
fn path_taken(kind: &str, held: &str, name: &str, path: &str) -> ApiError {
    let holder = if held.is_empty() {
        "a manifest".to_owned()
    } else {
        format!("{kind} '{held}'")
    };
    ApiError::Conflict(format!(
        "{holder} is already filed at {path}; creating {kind} '{name}' would replace it. \
         Change {holder} instead, or remove it first"
    ))
}

/// Proposals by forge repository and branch name: one at a time from the open-change check
/// to the pull request, so a second proposal of one resource finds the first one's pull request
/// instead of deleting the branch the first is still writing (F-POR2-011).
///
/// ponytail: one Portal replica (owner 2026-10-23); a second replica needs a Postgres advisory
/// lock keyed the same way.
static BRANCHES: std::sync::LazyLock<crate::keyed::Keyed> =
    std::sync::LazyLock::new(Default::default);

/// A proposal's turn on its branch name: held from before the open-change check until the pull
/// request is open, and the only way to reach [`create_or_reuse_branch`].
pub(crate) struct Turn {
    branch: String,
    _held: tokio::sync::OwnedMutexGuard<()>,
}

/// Waits until no other proposal holds `branch` in this repository.
pub(crate) async fn turn(gitea: &crate::git::GiteaClient, branch: &str) -> Turn {
    let key = format!("{}/{}/{}\n{branch}", gitea.base, gitea.owner, gitea.repo);
    Turn {
        branch: branch.to_owned(),
        _held: BRANCHES.lock(&key).await,
    }
}

pub(crate) async fn create_or_reuse_branch(
    gitea: &crate::git::GiteaClient,
    turn: &Turn,
    default_branch: &str,
) -> Result<String, ApiError> {
    let branch = turn.branch.as_str();
    match gitea.create_branch(branch, default_branch).await {
        Ok(()) => Ok(branch.to_string()),
        Err(GitError::Conflict(_)) => {
            let suffixed = format!("{branch}_");
            if let Some(open) = gitea
                .list_pull_requests("open")
                .await?
                .into_iter()
                .find(|pr| pr.head_branch == branch || pr.head_branch.starts_with(&suffixed))
            {
                return Err(ApiError::Conflict(format!(
                    "a change is already open on this resource: chg-{:08x}; approve or reject it first",
                    open.number
                )));
            }
            // The forge closes, a moment later, every pull request whose head is a branch that
            // was deleted, matched by name: a request opened on the recreated name is closed at
            // birth (T-0887). The stale branch goes, the change opens on a fresh name; `_` is
            // no character of a resource name, so the list still reads the name (T-0889).
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or_default();
            let fresh = format!("{suffixed}{nanos:08x}");
            tracing::info!(stale = %branch, branch = %fresh, from = %default_branch, "replacing a stale branch");
            gitea.delete_branch(branch).await?;
            gitea.create_branch(&fresh, default_branch).await?;
            Ok(fresh)
        }
        Err(other) => Err(other.into()),
    }
}

/// The one status a proposal may carry: the build lane's `status.build` on an App (AP-73).
///
/// Everything else about `status` is refused as it always was, and so is a `status.build` from a
/// caller no role names as the build lane — the role whose `propose` on `App` is constrained to
/// that field is the field's only writer, and the refusal says so.
fn build_lane_write(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    kind: &str,
    body: &Value,
) -> Result<(), ApiError> {
    let computed = || {
        ApiError::BadRequest(
            "status is computed by the platform and cannot be specified in the manifest (MF-04)"
                .to_owned(),
        )
    };
    if kind != "App" {
        return Err(computed());
    }
    // `status.build` and nothing beside it: a phase or a condition is the reconciler's.
    let status = body.get("status").and_then(Value::as_object);
    let build_only = status.is_some_and(|members| {
        members.len() == 1 && members.contains_key("build") && !members["build"].is_null()
    });
    if !build_only {
        return Err(computed());
    }
    if !crate::permissions::for_request(state, identity, project)
        .may_write_status_field("App", "status.build")
    {
        return Err(ApiError::Denied(
            "status.build is written by the build lane when it publishes the artifact: the one \
             role whose propose on App is constrained to that field writes it, and nobody else \
             (AP-13a, AP-73)"
                .to_owned(),
        ));
    }
    // The lane writes the build of an App on `main` and nothing else of it (AP-73): a stolen
    // lane token can neither create an App nor change what one reads, shows or who opens it.
    let name = body
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(current) = state.mirror.get(project, "App", name) else {
        return Err(ApiError::Denied(format!(
            "status.build is written back to an App on main; project {project} holds no App \
             '{name}' (AP-73)"
        )));
    };
    let map_at = |pointer: &str| -> std::collections::BTreeMap<String, String> {
        body.pointer(pointer)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    };
    if body.get("spec") != Some(&current.spec)
        || map_at("/metadata/labels") != current.metadata.labels
        || map_at("/metadata/annotations") != current.metadata.annotations
    {
        return Err(ApiError::Denied(format!(
            "the build lane writes status.build and nothing else: the spec, labels and \
             annotations of App '{name}' must stay as they are on main (AP-73)"
        )));
    }
    Ok(())
}

pub(crate) fn resolve_repo_path(
    envelope: &ResourceEnvelope,
    kind_info: &resource::KindInfo,
    project: &str,
) -> Result<String, ApiError> {
    // An Endpoint, a DataModel and a Subscription are filed under the space their
    // `contextSpaceRef` names, as jc-core's `context_space` says; every other kind under the
    // project's own space.
    let by_reference = matches!(kind_info.kind, "Endpoint" | "DataModel" | "Subscription");
    let space = envelope
        .metadata
        .labels
        .get("joinedcontext.com/space")
        .cloned()
        .or_else(|| {
            by_reference
                .then(|| crate::api::assistant::ref_name(&envelope.spec["contextSpaceRef"]))
                .flatten()
        })
        // A DataModel no space owns has a folder of its own, not the project's space (DM-75).
        .or_else(|| {
            (kind_info.kind != "DataModel")
                .then(|| envelope.metadata.namespace.clone())
                .flatten()
        });

    resource::repository_path(
        kind_info,
        project,
        space.as_deref(),
        &envelope.metadata.name,
    )
    .map_err(ApiError::BadRequest)
}

/// The file that holds `envelope`, and where it is: `path`, else the folder of the space its
/// `contextSpaceRef` names. A Policy lives in either: the single door files it under the project's
/// own space, an import and `jc_space_complete` under the space it names. Without a file the path
/// stays `path`.
pub(crate) async fn held_file(
    gitea: &crate::git::GiteaClient,
    path: &str,
    envelope: &ResourceEnvelope,
    kind_info: &resource::KindInfo,
    project: &str,
    git_ref: &str,
) -> Result<(String, Option<crate::git::RepoFile>), ApiError> {
    let at_path = gitea.get_file(path, git_ref).await?;
    let space = crate::api::assistant::ref_name(&envelope.spec["contextSpaceRef"]);
    if let (None, Some(space)) = (&at_path, space) {
        let name = &envelope.metadata.name;
        let filed = resource::repository_path(kind_info, project, Some(&space), name)
            .map_err(ApiError::BadRequest)?;
        if filed != path {
            if let Some(file) = gitea.get_file(&filed, git_ref).await? {
                return Ok((filed, Some(file)));
            }
        }
    }
    Ok((path.to_owned(), at_path))
}

pub(crate) fn author_credentials(
    identity: &crate::auth::session::Identity,
    project: &str,
) -> (String, String) {
    let author_name = identity
        .name
        .as_deref()
        .unwrap_or(&identity.username)
        .to_string();
    let fallback_email = format!("{}@{project}.local", identity.username);
    let author_email = identity
        .email
        .as_deref()
        .unwrap_or(&fallback_email)
        .to_string();
    (author_name, author_email)
}

/// Shared mutation engine: validates manifest constraints, plans diffs, and submits
/// merge requests to Git under human authorship (MF-12, CC-03, CC-44, CC-63).
#[allow(clippy::too_many_arguments)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ProposeOutcome {
    DryRun(DryRunResult),
    Change(Change),
    /// Committed to a workspace's branch; no Change exists until the workspace is brought back
    /// (CC-76, CC-79).
    Workspace(WorkspaceCommit),
    /// Committed to the shared branch of a plan of several changes, which opens one Change once
    /// every change of it is written (T-6282).
    Bundled(BundledCommit),
}

/// What one change of a bundled plan wrote on the plan's branch (T-6282).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BundledCommit {
    /// The manifest's path in the repository.
    pub path: String,
    pub lane: crate::change::Lane,
    /// The `Waits-On:` lines of the open changes outside the plan this one waits on (MF-48).
    pub waits_on: Vec<String>,
}

/// What a proposal into a workspace wrote (CC-76).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCommit {
    pub workspace: String,
    pub branch: String,
    /// The manifest's path in the repository.
    pub path: String,
    pub lane: crate::change::Lane,
}

impl ProposeOutcome {
    pub fn into_value(self) -> serde_json::Value {
        match self {
            Self::DryRun(dr) => serde_json::to_value(dr).unwrap_or(serde_json::Value::Null),
            Self::Change(chg) => serde_json::json!({
                "changeId": chg.metadata.name,
                "lane": chg.status.lane,
                "url": chg.status.merge_request,
                "change": chg,
            }),
            Self::Workspace(commit) => {
                serde_json::to_value(commit).unwrap_or(serde_json::Value::Null)
            }
            Self::Bundled(commit) => {
                serde_json::to_value(commit).unwrap_or(serde_json::Value::Null)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn propose(
    user: &CurrentUser,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    dry_run: bool,
    body_val: Value,
) -> Result<Response, ApiError> {
    let outcome = propose_with_identity(
        &user.0.identity,
        state,
        project,
        plural,
        path_name,
        operation,
        dry_run,
        body_val,
    )
    .await?;

    match outcome {
        ProposeOutcome::DryRun(res) => Ok((StatusCode::OK, Json(res)).into_response()),
        ProposeOutcome::Change(change) => Ok((StatusCode::ACCEPTED, Json(change)).into_response()),
        ProposeOutcome::Workspace(commit) => Ok((StatusCode::OK, Json(commit)).into_response()),
        ProposeOutcome::Bundled(commit) => Ok((StatusCode::OK, Json(commit)).into_response()),
    }
}

/// The check this person just ran, kept under the manifest's own kind and name (PF-57): one
/// place, because a rejected manifest records the same kind of verdict a passing one does.
async fn record_check_as(
    user: &CurrentUser,
    front: Front,
    state: &AppState,
    project: &str,
    manifest: &Value,
    verdict: &crate::ops::verdict::Verdict,
) {
    let caller = crate::ops::Caller {
        bearer: None,
        identity: user.0.identity.clone(),
        via: match front {
            Front::Portal | Front::Edge => crate::ops::Via::Session,
            Front::Bearer => crate::ops::Via::Bearer,
        },
        access: None,
    };
    crate::ops::record_check(&caller, state, project, manifest, verdict).await;
}

/// The REST doors of a manifest that names no draft (PF-57, owner decision T-0956): its dry run
/// records a verdict under the manifest's own kind and name, and its proposal needs that verdict,
/// green and fresh for the same manifest, exactly as the draft door and the operations do. A
/// proposal that became a Change forgets the draft its check created.
#[allow(clippy::too_many_arguments)]
async fn propose_checked(
    user: &CurrentUser,
    front: Front,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    dry_run: bool,
    body_val: Value,
    workspace: Option<&str>,
    confirm: Option<&str>,
) -> Result<Response, ApiError> {
    let mut body_val = body_val;
    // The slug is the platform's to mint (EP-75); the Portal's own form brings the one it minted.
    let typed_allowed = front != Front::Bearer && user.0.identity.client.is_none();
    crate::endpoint_slug::settle_in(state, project, None, typed_allowed, &mut body_val)
        .await
        .map_err(|message| ApiError::Unprocessable {
            detail: format!("the manifest is refused: {message}"),
            errors: vec![format!("spec.slug: {message}")],
        })?;
    let manifest = body_val.clone();
    if dry_run {
        let answer = propose_with_identity(
            &user.0.identity,
            state,
            project,
            plural,
            path_name,
            operation,
            true,
            body_val,
        )
        .await;
        let outcome = match answer {
            Ok(outcome) => outcome,
            Err(err) => {
                // A check that rejects the manifest records the red verdict and answers it, the way
                // a `DataSource` always has (T-2234): one shape for one outcome. Anything that is
                // not a judgement about the manifest stays the error it was.
                let Some(mut refused) = crate::api::dry_run::refused_check(&err, &manifest) else {
                    return Err(err);
                };
                let verdict = refused
                    .verdict
                    .clone()
                    .expect("refused_check sets a verdict");
                // The red verdict reaches the draft the person is working in and writes none of its
                // own: what this check refused can be a credential typed into the manifest, and a
                // draft created to hold it would store the value the refusal exists to stop.
                crate::ops::record_refused_check(state, project, &manifest, &verdict).await;
                refused.verdict = Some(verdict);
                return Ok((StatusCode::OK, Json(refused)).into_response());
            }
        };
        let ProposeOutcome::DryRun(mut result) = outcome else {
            return Err(ApiError::Internal("a dry run proposed a change".into()));
        };
        let verdict = if manifest.get("kind") == Some(&Value::from("DataSource")) {
            let answer =
                serde_json::to_value(&result).map_err(|e| ApiError::Internal(e.to_string()))?;
            crate::ops::datasource_verdict(&answer, &manifest)
        } else {
            // A dry run that got this far answered `valid: true`: every fault of the manifest is a
            // refusal above, where `refused_check` turns it into the findings (T-2234). So the
            // empty list here is a green verdict's own, and not a red one with nothing to resolve.
            // A reference an open change creates is a warning and the change named (MF-48): the
            // verdict passes, and says what it waits on.
            let mut verdict = crate::ops::verdict::Verdict::new(
                result.valid,
                result
                    .awaited
                    .iter()
                    .map(crate::references::Awaited::finding)
                    .collect(),
                serde_json::to_value(&result.plan).ok(),
                &manifest,
            );
            verdict.waits_on = result.awaited.iter().map(|a| a.change.clone()).collect();
            verdict
                .findings
                .extend(crate::apps::renamed_shape(&manifest));
            verdict
        };
        record_check_as(user, front, state, project, &manifest, &verdict).await;
        result.verdict = Some(verdict);
        return Ok((StatusCode::OK, Json(result)).into_response());
    }
    let outcome = propose_gated_in(
        &user.0.identity,
        state,
        project,
        plural,
        path_name,
        operation,
        body_val,
        workspace,
    )
    .await?;
    crate::ops::forget_check(state, project, &manifest).await;
    match outcome {
        ProposeOutcome::DryRun(res) => Ok((StatusCode::OK, Json(res)).into_response()),
        // A person at the Portal who administers every kind of it has it approved now (PF-58);
        // a bearer caller never does (AG-11).
        ProposeOutcome::Change(change) if front != Front::Bearer => {
            let kind = manifest
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let change = crate::api::changes::approve_as_proposed(
                state,
                &user.0.identity,
                project,
                kind,
                change,
                confirm,
            )
            .await;
            Ok((StatusCode::ACCEPTED, Json(change)).into_response())
        }
        ProposeOutcome::Change(change) => Ok((StatusCode::ACCEPTED, Json(change)).into_response()),
        ProposeOutcome::Workspace(commit) => Ok((StatusCode::OK, Json(commit)).into_response()),
        ProposeOutcome::Bundled(commit) => Ok((StatusCode::OK, Json(commit)).into_response()),
    }
}

/// Shared mutation engine that operates on `Identity`: validates manifest constraints, plans diffs,
/// and submits merge requests to Git under human authorship (MF-12, CC-03, CC-44, CC-63).
/// Called by both session-based REST routes and the operations registry / MCP server. Asks for no
/// verdict: the draft door has asked already, and a published application was checked by its
/// preview. A manifest proposed without a draft goes through [`propose_gated`].
#[allow(clippy::too_many_arguments)]
pub async fn propose_with_identity(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    dry_run: bool,
    body_val: Value,
) -> Result<ProposeOutcome, ApiError> {
    propose_engine(
        identity, state, project, plural, path_name, operation, dry_run, body_val, false, None,
        None,
    )
    .await
}

/// [`propose_with_identity`] into the workspace `workspace`: the same checks (PF-82), then a
/// commit to its branch and no pull request; the workspace comes back as one Change (CC-76,
/// CC-79).
#[allow(clippy::too_many_arguments)]
pub async fn propose_into_workspace(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    body_val: Value,
    workspace: &str,
) -> Result<ProposeOutcome, ApiError> {
    propose_engine(
        identity,
        state,
        project,
        plural,
        path_name,
        operation,
        false,
        body_val,
        false,
        Some(workspace),
        None,
    )
    .await
}

/// [`propose_with_identity`] for a manifest that names no draft (PF-57, owner decision T-0956):
/// once the manifest has passed every check of its own, its proposal needs the verdict its check
/// recorded, green and fresh for this exact manifest, before anything reaches the forge.
#[allow(clippy::too_many_arguments)]
pub async fn propose_gated(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    body_val: Value,
) -> Result<ProposeOutcome, ApiError> {
    propose_gated_in(
        identity, state, project, plural, path_name, operation, body_val, None,
    )
    .await
}

/// [`propose_gated`] into a workspace when one is named: the REST doors' `?workspace=`
/// (API/01 §22), which asks for the same verdict as a write outside one (PF-82).
#[allow(clippy::too_many_arguments)]
pub async fn propose_gated_in(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    body_val: Value,
    workspace: Option<&str>,
) -> Result<ProposeOutcome, ApiError> {
    propose_engine(
        identity, state, project, plural, path_name, operation, false, body_val, true, workspace,
        None,
    )
    .await
}

/// [`propose_gated`] onto `branch`, the shared branch of a plan of several changes (T-6282): the
/// same checks and the same verdict, a commit and no pull request; the plan's apply opens one
/// for the branch once every change of it is written. Call it inside
/// [`crate::references::BUNDLE`] holding the plan's manifests, so references to siblings resolve.
#[allow(clippy::too_many_arguments)]
pub async fn propose_gated_onto(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    body_val: Value,
    branch: &str,
) -> Result<BundledCommit, ApiError> {
    match propose_engine(
        identity,
        state,
        project,
        plural,
        path_name,
        operation,
        false,
        body_val,
        true,
        None,
        Some(branch),
    )
    .await?
    {
        ProposeOutcome::Bundled(commit) => Ok(commit),
        _ => Err(ApiError::Internal(
            "a bundled proposal answered something else than its commit".into(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn propose_engine(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    dry_run: bool,
    mut body_val: Value,
    gated: bool,
    workspace: Option<&str>,
    bundle: Option<&str>,
) -> Result<ProposeOutcome, ApiError> {
    // The manifest as it was sent, which is what its check judged and what the verdict is fresh for.
    let received = gated.then(|| body_val.clone());
    // 0. The files a manifest names but cannot contain (a Mapping's golden examples, DM-39),
    // taken out of the body the way `draft` is, before anything reads it as an envelope.
    let sidecar_files = body_val.as_object_mut().and_then(|map| map.remove("files"));
    // A Pipeline is written at v1alpha2 whoever writes it: the UI, an agent or MCP (PL-54).
    if operation != Operation::Delete {
        pipeline_second_shape(&mut body_val);
    }

    // 1. Resolve plural catalogue entry
    let kind_info = resource::by_plural(plural).ok_or_else(|| {
        ApiError::NotFound(format!(
            "plural '{plural}' not found in project '{project}'"
        ))
    })?;

    // 1a. Whether the caller may propose this kind here at all, before anything of the
    //     manifest is read: a viewer's create is a 403 whatever it sends, never a validation
    //     answer that teaches the kind's schema (T-2576, PF-50, PF-51). What the content may
    //     be is step 4c's.
    //     An App's default group is proposed by whoever may propose the App (AP-119).
    if !crate::groups::for_proposal(
        state,
        identity,
        project,
        kind_info.kind,
        operation == Operation::Update,
        &body_val,
    )
    .may(kind_info.kind, jc_core::kinds::Verb::Propose)
    {
        return Err(ApiError::Denied(format!(
            "no role grants propose on {} in project {project} (PF-50)",
            kind_info.kind
        )));
    }

    // 2-3. The envelope, its apiVersion, kind, path name and namespace.
    let mut envelope = envelope_for(&body_val, kind_info.kind, plural, project, path_name)?;

    // 3a. One string written over a stored legacy map replaces the entry it resolves to and keeps
    //     every other language, whichever door sent it (UI-50, T-2764).
    if operation == Operation::Update {
        if let Some(stored) = state
            .mirror
            .get(project, kind_info.kind, &envelope.metadata.name)
        {
            keep_languages(&mut envelope.metadata.title, stored.metadata.title.as_ref());
            keep_languages(
                &mut envelope.metadata.description,
                stored.metadata.description.as_ref(),
            );
            body_val["metadata"] = serde_json::to_value(&envelope.metadata)
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        }
    }

    // 4. Metadata DNS-1123, status rejection (MF-04) and secret rejection (MF-24)
    resource::validate_meta(&envelope.metadata).map_err(ApiError::BadRequest)?;

    // `status` is the platform's own computation and no manifest carries it (MF-04) — with one
    // door: the build lane writes `status.build` back in the commit that publishes the artifact
    // an App runs (AP-13a, AP-73). That write keeps its status; every other one is refused.
    let build_write = if body_val.get("status").is_some() || envelope.status.is_some() {
        if bundle.is_some() {
            return Err(ApiError::BadRequest(
                "a manifest carrying status is the build lane's write, proposed on its own and \
                 never in a plan of several changes (MF-04, AP-73)"
                    .into(),
            ));
        }
        build_lane_write(state, identity, project, kind_info.kind, &body_val)?;
        true
    } else {
        false
    };

    refuse_platform_fields(&body_val, &envelope)?;

    // A published static App names its repository, unless the Portal image ships its bundle
    // (AP-87). jc-core believes the annotation; this door checks the bundle is really here, so
    // the annotation cannot publish an application nothing serves.
    if operation != Operation::Delete {
        if let Some(refusal) =
            crate::apps::static_host::unshipped_claim(state.config.apps_dir.as_deref(), &envelope)
        {
            return Err(ApiError::BadRequest(refusal));
        }
    }

    check_content(
        state,
        identity,
        project,
        kind_info.kind,
        operation,
        &envelope,
    )
    .await?;

    // 4c. Who may propose this kind here, with this content (T-0526, PF-50): the bindings of
    //     the organization repository, before a Change exists. 403 names the verb or the field.
    //     A build write was judged by `build_lane_write`: the lane's rule, the App on main and
    //     nothing of it changed (AP-73).
    if !build_write {
        crate::groups::for_proposal(
            state,
            identity,
            project,
            kind_info.kind,
            operation == Operation::Update,
            &body_val,
        )
        .check(
            kind_info.kind,
            jc_core::kinds::Verb::Propose,
            Some(&body_val),
        )?;
    }
    // 4d. Nobody grants above their own rights (PF-52, AG-77), measured against what the change
    //     replaces, so a revoke is not held to the grants it leaves (T-5016).
    let replaced = state
        .mirror
        .get(project, kind_info.kind, &envelope.metadata.name)
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    //     Judged in the namespace the envelope filled in: a project Role sent without one is the
    //     project's, never the organization's (T-6266, PF-68).
    let mut judged = body_val.clone();
    judged["metadata"]["namespace"] = Value::from(envelope.metadata.namespace.clone());
    crate::permissions::within_own_rights(state, identity, &judged, replaced.as_ref(), "proposer")?;
    // 4e. A pipeline reaches only what its proposer reads and writes (PL-94).
    if operation != Operation::Delete {
        crate::pipeline_rights::check(
            state,
            identity,
            project,
            &body_val,
            jc_core::kinds::Verb::Propose,
        )?;
    }

    if operation != Operation::Delete {
        check_organization_names(state, identity, project, kind_info.kind, &envelope).await?;
    }

    // Every resource this manifest names has to be there, so a person meets a missing name in the
    // form they typed it into and not in the reconciler's log (MF-13, T-2233).
    // A resource an open change creates resolves once that change is approved (MF-48): the check
    // says so, and the proposal records the change so it cannot merge before it.
    let awaited = if operation != Operation::Delete {
        let missing =
            crate::references::missing(&state.mirror, project, kind_info.kind, &envelope.spec);
        crate::references::awaited(state, project, missing).await?
    } else {
        Vec::new()
    };

    // 4h. Every role of an App comes with its default group, in the same Change (AP-118,
    //     ADR-N-031): the group of a new role and its access entry are written, the group of a
    //     role taken away, or of a retired App, is removed. A project in its own repository
    //     (layout 2) cannot carry an organization file in its Change, so its Apps keep their
    //     access as written. A build write leaves the App as it is on main (AP-73).
    // 4g. An App's owner is who created it, never what the manifest says (T-4910).
    if kind_info.kind == "App" && operation != Operation::Delete && !build_write {
        let stored = state
            .mirror
            .get(project, "App", &envelope.metadata.name)
            .map(|app| app.metadata.annotations);
        crate::apps::roles::stamp_owner(
            &mut envelope.metadata.annotations,
            stored.as_ref(),
            identity,
        );
    }
    let default_groups = if kind_info.kind == "App"
        && operation != Operation::Delete
        && !build_write
        && state.mirror.repository_of(project).is_none()
    {
        let planned = crate::apps::default_groups::plan(state, identity, project, &mut envelope)?;
        crate::apps::default_groups::release(&mut envelope, &planned.removed);
        planned
    } else {
        crate::apps::default_groups::DefaultGroups::default()
    };

    // 4d. A new space comes with its one model, in the same Change (DM-61, DM-62, ADR-N-033),
    //     whoever creates it: the form, the API, the assistant or MCP.
    let space_model = if kind_info.kind == "ContextSpace" && operation == Operation::Create {
        space_model(state, project, &mut envelope)
    } else {
        Vec::new()
    };

    // 5. Diff against current mirror state
    let current = state
        .mirror
        .get(project, kind_info.kind, &envelope.metadata.name);
    // 5'. A kind filed once per scope keeps one file whatever its name (DS-07): a create under a
    //     new name would replace the one there and read as a create (T-2855).
    // Filed once when two names in two spaces render one path, whichever template the namespace
    // picks: a ContextSpace is filed by its `{space}`, which is its own name.
    let filed_once =
        kind_info.repo_path(project, "s1", "a") == kind_info.repo_path(project, "s2", "b");
    if operation == Operation::Create && filed_once {
        if let Some(refusal) =
            held_under_another_name(state, kind_info, project, &envelope.metadata.name)
        {
            return Err(refusal);
        }
    }
    let mut plan = plan::diff(current.as_ref(), Some(&envelope));

    // The same folder as the manifest, so a path is checked against where it will be written.
    let mut manifest_path = resolve_repo_path(&envelope, kind_info, project)?;
    let mut sidecars = sidecars(sidecar_files, &manifest_path, &envelope.metadata.name)?;
    let folder = manifest_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    for (relative, content) in space_model {
        let full = format!("{folder}/{relative}");
        // A caller that sent the model's files itself keeps its own.
        if !sidecars.iter().any(|(path, _)| *path == full) {
            sidecars.push((full, content));
        }
    }
    for (path, content) in &sidecars {
        plan.fields.push(plan::FieldChange {
            path: format!("files.{path}"),
            from: None,
            to: Some(Value::from(content.len())),
        });
    }

    // 5a. What an App grants rides in the same change as the App (CC-61, AP-96, T-2632): the
    //     gateway reads endpoints and policies from the repository alone, so a grant nobody
    //     commits is a grant it never enforces. The reviewer reads each one in the plan.
    // A build write leaves the App as it is on main (AP-73), so it carries no grant either.
    let mut grants = if kind_info.kind == "App" && operation != Operation::Delete && !build_write {
        app_grants(state, project, &envelope)?
    } else {
        AppGrants::default()
    };
    group_files(&default_groups, project, &mut grants)?;
    plan.fields.extend(grants.review.iter().cloned());

    // 6. Risk-classified approval lane
    let lane = change::classify_manifest(&envelope, operation);
    // OPS-16: the one series no other component can produce. A rise in red proposals is a
    // change in what people are asking the platform to do.
    crate::telemetry::proposed(lane, kind_info.kind);

    // 7. Dry run short-circuit; an `http` DataSource is also fetched once (MF-39).
    if dry_run {
        let probe = if kind_info.kind == "DataSource" {
            crate::api::pipeline_test::probe_source(state, project, &envelope.spec).await
        } else {
            None
        };
        return Ok(ProposeOutcome::DryRun(DryRunResult {
            valid: true,
            restarts_stream: crate::plan::restarts_stream(kind_info.kind, &plan),
            lane,
            plan,
            probe,
            verdict: None,
            findings: if kind_info.kind == "Environment" {
                // An overlay is where the domain is written out (CC-73).
                Vec::new()
            } else {
                crate::api::dry_run::literal_domain_findings(
                    &envelope.spec,
                    // No fallback: an instance that does not know its domain finds nothing.
                    &crate::api::assistant::org_domain(state, ""),
                )
            },
            awaited,
            goes_with: Vec::new(),
        }));
    }

    // 7a. The verdict, after every check of the manifest's own and before the forge (T-0956).
    if let Some(received) = &received {
        crate::ops::verdict_for_manifest(state, project, received).await?;
    }

    // 8. Commit to Git merge request via Gitea client
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;

    let default_branch = gitea.default_branch().await?;
    if kind_info.kind == "Organization" && operation != Operation::Create {
        manifest_path = organization_path(
            gitea,
            &default_branch,
            &envelope.metadata.name,
            manifest_path,
        )
        .await?;
    }

    // 8a. Every check above judged the body against the mirror, and a fresh branch starts at
    //     main. A mirror that has not caught up with main on this resource would put the body
    //     over a newer main, and whatever the checks took for unchanged would revert what main
    //     moved to: the build lane's status write once undid the bootstrap's new pin (T-2674).
    if workspace.is_none() {
        let (held_at, on_main) = held_file(
            gitea,
            &manifest_path,
            &envelope,
            kind_info,
            project,
            &default_branch,
        )
        .await?;
        // An update goes over the file it updates, or the resource ends up in two files.
        if operation == Operation::Update {
            manifest_path = held_at;
        }
        // A create of a kind filed once writes a file that is not there yet: one at its path on
        // main is the resource the mirror has not read, and writing over it would be a
        // replacement the approver reads as a create (T-2855).
        if operation == Operation::Create && filed_once {
            if let Some(file) = &on_main {
                let held = serde_norway::from_str::<ResourceEnvelope>(&file.content)
                    .map(|held| held.metadata.name)
                    .unwrap_or_default();
                return Err(path_taken(
                    kind_info.kind,
                    &held,
                    &envelope.metadata.name,
                    &manifest_path,
                ));
            }
        }
        let domain = crate::api::assistant::org_domain(state, project);
        if moved_on_main(current.as_ref(), on_main.as_ref(), &domain) {
            return Err(ApiError::Conflict(format!(
                "{} '{}' changed on {default_branch} after the Portal last read it; read it \
                 again and send the change once more",
                kind_info.kind, envelope.metadata.name
            )));
        }
    }

    // 8b. A lane's build is checked against the App's repository and published by the Portal
    //     before anything is written: the lane's token names a build, it never makes one
    //     (AP-101, AP-104).
    let mut published = None;
    if build_write {
        published = crate::apps::built::check_and_publish(
            gitea,
            state.wasm_apps.as_deref(),
            project,
            &envelope.metadata.name,
            &envelope.spec,
            body_val.pointer("/status/build").unwrap_or(&Value::Null),
        )
        .await?;
    }

    let op_str = match operation {
        Operation::Create => "create",
        Operation::Update => "update",
        Operation::Delete => "delete",
    };

    let (branch, _turn) = match (workspace, bundle) {
        // A workspace is one branch for everything it holds (CC-76): no per-resource branch,
        // no pull request, and a change open on `main` does not stop an edit here.
        (Some(name), _) => {
            let open = state
                .workspaces
                .live(name)
                .await
                .map_err(|err| ApiError::NotFound(err.to_string()))?;
            within_workspace(&open, project, kind_info.kind, &envelope)?;
            if !crate::ops::workspaces::owns(&open, identity) {
                return Err(ApiError::Denied(format!(
                    "workspace '{name}' belongs to {}; only its owner writes into it",
                    open.owner
                )));
            }
            let branch = open.branch();
            match gitea.create_branch(&branch, &default_branch).await {
                Ok(()) | Err(GitError::Conflict(_)) => {}
                Err(err) => return Err(err.into()),
            }
            (branch, None)
        }
        // A plan of several changes is one branch for all of them (T-6282); a change already
        // open on one of its resources still refuses it, as a proposal of its own would be (CC-34).
        (None, Some(shared)) => {
            let own = branch_name(project, kind_info.kind, &envelope.metadata.name, operation);
            if let Some(pending) = open_change_on(state, gitea, &own, project).await? {
                return Err(ApiError::Conflict(format!(
                    "a change for {} '{}' is already open: {}; approve or reject it first",
                    kind_info.kind, envelope.metadata.name, pending.name
                )));
            }
            match gitea.create_branch(shared, &default_branch).await {
                Ok(()) | Err(GitError::Conflict(_)) => {}
                Err(err) => return Err(err.into()),
            }
            (shared.to_owned(), None)
        }
        (None, None) => {
            let branch = branch_name(project, kind_info.kind, &envelope.metadata.name, operation);
            let held = turn(gitea, &branch).await;
            // One open change per resource (CC-34): the branch is one per resource and
            // operation, so a second proposal while one is pending would rewrite the open pull
            // request under its approver. Refused before anything is written, naming the change
            // to decide first (T-0883).
            if let Some(pending) = open_change_on(state, gitea, &branch, project).await? {
                return Err(ApiError::Conflict(format!(
                    "a change for {} '{}' is already open: {}; approve or reject it first",
                    kind_info.kind, envelope.metadata.name, pending.name
                )));
            }
            let branch = create_or_reuse_branch(gitea, &held, &default_branch).await?;
            (branch, Some(held))
        }
    };
    let repo_path = manifest_path;

    let yaml_content = manifest_yaml(&envelope, build_write, published.as_ref())?;

    // Gitea wants the blob sha of the file being replaced, which only `get_file` knows;
    // `status.observedRevision` is a commit id and would be rejected. Asking the branch (not the
    // mirror) also covers a manifest that exists in Git but has not been mirrored yet.
    let existing_sha = gitea
        .get_file(&repo_path, &branch)
        .await
        .ok()
        .flatten()
        .map(|f| f.sha);

    let (author_name, author_email) = author_credentials(identity, project);
    let commit_msg = format!("{op_str} {} {}", kind_info.kind, envelope.metadata.name);

    let file_write = FileWrite {
        path: &repo_path,
        branch: &branch,
        message: &commit_msg,
        content: &yaml_content,
        sha: existing_sha.as_deref(),
        author: Author {
            name: &author_name,
            email: &author_email,
        },
    };

    // The files first: a reviewer opening the merge request never reads a manifest naming a file
    // the change does not carry, and a refusal on one of them leaves no manifest behind.
    for (path, content) in &sidecars {
        let existing = gitea
            .get_file(path, &branch)
            .await
            .ok()
            .flatten()
            .map(|f| f.sha);
        gitea
            .put_file(&FileWrite {
                path,
                branch: &branch,
                message: &commit_msg,
                content,
                sha: existing.as_deref(),
                author: Author {
                    name: &author_name,
                    email: &author_email,
                },
            })
            .await?;
    }

    if !grants.uploads.is_empty() || !grants.removed.is_empty() {
        let mut deletes = Vec::new();
        for path in &grants.removed {
            if let Some(file) = gitea.get_file(path, &branch).await? {
                deletes.push((path.clone(), file.sha));
            }
        }
        gitea
            .change_files(
                &branch,
                &commit_msg,
                Author {
                    name: &author_name,
                    email: &author_email,
                },
                &grants.uploads,
                &deletes,
            )
            .await?;
    }

    gitea.put_file(&file_write).await?;

    if let Some(name) = workspace {
        return Ok(ProposeOutcome::Workspace(WorkspaceCommit {
            workspace: name.to_owned(),
            branch,
            path: repo_path,
            lane,
        }));
    }
    if bundle.is_some() {
        return Ok(ProposeOutcome::Bundled(BundledCommit {
            path: repo_path,
            lane,
            waits_on: awaited
                .iter()
                .map(crate::references::waits_on_line)
                .collect(),
        }));
    }

    let pr_title = format!("{op_str} {} {}", kind_info.kind, envelope.metadata.name);
    let mut pr_body = format!(
        "Proposed {op_str} of {} `{}` in project `{project}` via joinedcontext Portal.",
        kind_info.kind, envelope.metadata.name
    );
    // The changes it waits on, one line each, which the approval reads back (MF-48).
    for waiting in &awaited {
        pr_body.push_str("\n\n");
        pr_body.push_str(&crate::references::waits_on_line(waiting));
    }

    let pr = gitea
        .create_pull_request(&branch, &default_branch, &pr_title, &pr_body)
        .await?;

    // 9. Answer 202 Accepted with Change resource
    let change_meta = crate::api::changes::change_meta(state, gitea, pr.number, project);
    let change_status = ChangeStatus::new(lane, ChangePhase::PendingApproval, plan.summary)
        .in_repository(&pr.repository)
        .proposed_through(&pr.body)
        .with_merge_request(pr.url.clone());
    let change = Change::new(change_meta, change_status);

    // 10. The build lane's write, checked and published in 8b, is approved by the Portal: a person
    //     approved the source, the lane approves nothing (AP-73, AP-104, AG-11).
    let change = if build_write {
        crate::api::changes::approve_build(state, gitea, &pr, &repo_path, change).await
    } else {
        change
    };

    Ok(ProposeOutcome::Change(change))
}

/// A kind filed once per scope already held under another name (DS-07, T-2855): a create would
/// replace that file and read as a create.
fn held_under_another_name(
    state: &AppState,
    kind_info: &resource::KindInfo,
    project: &str,
    name: &str,
) -> Option<ApiError> {
    let held = state
        .mirror
        .list(
            project,
            kind_info.kind,
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .find(|held| held.metadata.name != name)?;
    let path = kind_info.repo_path(project, "", &held.metadata.name);
    Some(path_taken(kind_info.kind, &held.metadata.name, name, &path))
}

/// An Organization still in the legacy `org.yaml` is changed or deleted there (PF-121): written to
/// `organizations/{name}.yaml` instead, the repository would hold it twice and load neither.
async fn organization_path(
    gitea: &crate::git::GiteaClient,
    branch: &str,
    name: &str,
    path: String,
) -> Result<String, ApiError> {
    let legacy = jc_core::kinds::LEGACY_ORGANIZATION_PATH;
    let held = gitea.get_file(legacy, branch).await?.and_then(|file| {
        serde_norway::from_str::<ResourceEnvelope>(&file.content)
            .ok()
            .map(|held| held.metadata.name)
    });
    Ok(if held.as_deref() == Some(name) {
        legacy.to_owned()
    } else {
        path
    })
}

/// The manifest as committed: no status, except a build write's, whose component and shard are
/// the Portal's from the bundle it checked, whatever the proposal said of them (AP-73, AP-151).
fn manifest_yaml(
    envelope: &ResourceEnvelope,
    build_write: bool,
    published: Option<&crate::apps::wasm_apps::Published>,
) -> Result<String, ApiError> {
    let mut envelope_to_commit = envelope.clone();
    if build_write {
        if let Some(status) = envelope_to_commit.status.as_mut() {
            status.shard = published.map(|p| p.shard);
            if let Some(build) = status.build.as_mut() {
                build.component = published.map(|p| p.component.clone());
            }
        }
    } else {
        envelope_to_commit.strip_status();
    }
    serde_norway::to_string(&envelope_to_commit)
        .map_err(|e| ApiError::Internal(format!("serialize manifest to yaml: {e}")))
}

/// The envelope of a proposal (steps 2 and 3): an apiVersion jc-core serves, the kind its
/// plural names, the name its path names, and this project as its namespace.
fn envelope_for(
    body_val: &Value,
    kind: &str,
    plural: &str,
    project: &str,
    path_name: Option<&str>,
) -> Result<ResourceEnvelope, ApiError> {
    // 2. Deserialize envelope and validate apiVersion, kind and path name
    let mut envelope: ResourceEnvelope = serde_json::from_value(body_val.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid resource envelope: {e}")))?;

    if !jc_core::serves(&envelope.kind, &envelope.api_version) {
        return Err(ApiError::BadRequest(format!(
            "apiVersion '{}' is not supported (expected '{}')",
            envelope.api_version,
            resource::API_VERSION
        )));
    }

    if envelope.kind != kind {
        return Err(ApiError::BadRequest(format!(
            "kind '{}' does not match plural '{}' (expected '{}')",
            envelope.kind, plural, kind
        )));
    }

    if let Some(expected_name) = path_name {
        if envelope.metadata.name != expected_name {
            return Err(ApiError::BadRequest(format!(
                "metadata.name '{}' does not match path '{}'",
                envelope.metadata.name, expected_name
            )));
        }
    }

    // 3. Namespace validation and default filling
    match envelope.metadata.namespace.as_deref() {
        None | Some("") => {
            envelope.metadata.namespace = Some(project.to_string());
        }
        Some(ns) if ns == project => {}
        Some(foreign) => {
            return Err(ApiError::BadRequest(format!(
                "metadata.namespace '{foreign}' does not match project '{project}'"
            )));
        }
    }
    Ok(envelope)
}

/// What a proposal never carries: a literal secret (MF-24) or an annotation only the build lane
/// writes (AP-11, AP-13a).
fn refuse_platform_fields(body_val: &Value, envelope: &ResourceEnvelope) -> Result<(), ApiError> {
    if let Some(secret_key) = find_literal_secret(body_val) {
        return Err(ApiError::BadRequest(format!(
            "literal secret in field '{secret_key}' is forbidden; use secretRef instead (MF-24)"
        )));
    }

    // A digest is not something a person types: the build lane writes it back when it publishes
    // the image it built here, and an App deploys what it names. Typed in, or carried in from
    // another instance, it would run an image this platform never built (AP-11, AP-13a).
    if let Some(key) = crate::apps::converge::BUILT_ANNOTATIONS
        .into_iter()
        .find(|key| envelope.metadata.annotations.contains_key(*key))
    {
        return Err(ApiError::BadRequest(format!(
            "annotation '{key}' is written by the build lane when it publishes an image and \
             cannot be set in a proposal (AP-11, AP-13a)"
        )));
    }
    Ok(())
}

/// The checks on the manifest itself and on what it names in this project (4a' to
/// 4b-contact): the kind is defined, its spec parses, it keeps inside the organization's bounds
/// and quotas, and the names it derives are free.
async fn check_content(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    kind: &str,
    operation: Operation,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    // 4a'. A kind jc-core does not define is a kind no loader can read: `jcctl`, the Portal's
    //       own sync and the gateway's store all refuse an unknown kind and refuse the whole
    //       repository with it, so one such file stops configuration reaching every endpoint.
    //       What is left of that gap is the seed entity: a plain `.json` NGSI-LD entity (CC-72),
    //       never a `kind: Entity` manifest. `Subscription` closed it with jc-core-v0.7.30 and
    //       is written like any other kind now (T-0833, T-0913).
    if jc_core::registry::by_kind(kind).is_none() {
        return Err(ApiError::BadRequest(format!(
            "kind '{}' is declared but not defined by the platform yet, and a repository holding \
             one stops loading for every component; it cannot be written (CC-72, T-0833)",
            kind
        )));
    }

    // 4a. An Endpoint's representation outputs, member by member (API/11 EP-159): a 422 that
    //     names the output and the member, which the form shows beside the field.
    if kind == "Endpoint" && operation != Operation::Delete {
        check_representation_shape(&envelope.spec)?;
    }

    // 4b. The kind's own parse and invariants (T-0412, CC-08, MF-24). `jcctl apply` would
    //     refuse this manifest on `main`, after an approval; refusing it here turns a broken
    //     repository into a form error that names the field. Kinds without a jc-core type
    //     (PORTAL_ONLY_KINDS) have nothing to check against and pass as before.
    if let Some(checked) = jc_core::registry::validate_yaml(
        kind,
        &serde_json::to_string(&envelope).map_err(|e| ApiError::Internal(e.to_string()))?,
    ) {
        checked.map_err(|e| ApiError::BadRequest(format!("spec is not a valid {}: {e}", kind)))?;
    }

    // 4e. An Organization's limits stay inside the operator's bounds (PF-97): refused here,
    //     before a Change exists, and again at approval, since the bounds may tighten between.
    if operation != Operation::Delete {
        organization_within_bounds(state, kind, &envelope.spec, ApiError::BadRequest)?;
        // 4f. The organization's public-app policy (PF-103), held again at approval.
        public_app_allowed(state, project, envelope, ApiError::BadRequest)?;
        // 4g. An App's hostnames lie in the organization's verified domain and are no other
        //     App's (AP-172); the reconciler routes only the names that still pass.
        if kind == "App" {
            let apex = state.config.app_settings.as_ref().map(|s| s.apex.as_str());
            crate::apps::hostnames::check(
                &state.mirror,
                project,
                &envelope.metadata.name,
                &envelope.spec,
                apex,
            )
            .map_err(ApiError::BadRequest)?;
        }
    }

    // 4d. A projection names only what its model version has (MP-01, T-2558): the same check
    //     `jcctl validate` runs, so the two cannot disagree about what is stale, and every stale
    //     name is listed at once. The model is looked up in this project only, so a model of
    //     another project is absent and nothing of it is named (R20).
    if kind == "ModelProjection" && operation != Operation::Delete {
        check_projection(state, project, &envelope.spec).await?;
    }

    // 4c. A ServiceAccount's Keycloak client id is derived, `{project}-{name}`, and the hyphen
    //     is a character of both, so another project's account may derive the same id. The
    //     gateway resolves such an id to nobody (T-1454); refusing it here keeps a proposal from
    //     switching off the other project's account. The other account is not named: it may live
    //     in a project the author cannot read (PF-59).
    if kind == "ServiceAccount" {
        if let Some(id) = service_account_id_taken(&state.mirror, project, &envelope.metadata.name)
        {
            return Err(ApiError::BadRequest(format!(
                "metadata.name '{}' gives the Keycloak client id '{id}', which another \
                 ServiceAccount already derives; choose another name (T-1456)",
                envelope.metadata.name
            )));
        }
    }

    // 4b''. An agent profile names only operations this Portal registers (MF-40): jc-core checks
    //       the shape of an operation name, the registry is the Portal's to know.
    if kind == "AgentProfile" {
        if let Some(name) = unknown_operation(&envelope.spec) {
            return Err(ApiError::BadRequest(format!(
                "spec.access.operations names '{name}', which is not a registered operation (MF-40)"
            )));
        }
    }

    // 4b-surfaces. A domain-standard representation (`ical`) needs its type and that type's
    //       mandatory attributes in the space's model (EP-125): the rule `jcctl validate` runs,
    //       in the same words, so the reconciler is never handed an Endpoint it would refuse.
    if kind == "Endpoint" && operation != Operation::Delete {
        check_surfaces(state, project, &envelope.metadata.name, &envelope.spec).await?;
        check_policy_refs(state, project, envelope)?;
    }

    // 4b'''. A Pipeline in replace drops what another Pipeline writes onto the same ids in its
    //        space (PL-79, T-4409): the rule `jcctl validate` runs, over the project's Pipelines
    //        with this proposal in place of its own.
    if kind == "Pipeline" && operation != Operation::Delete {
        check_replace(state, project, &envelope.metadata.name, &envelope.spec)?;
    }

    // 4b-model. A space has one data model, whose classes are its types (DM-61, T-6394): a
    //       second model of the space would plan and merge, then every Pipeline and Endpoint
    //       of its classes is refused. The rule `jcctl validate` runs, said before the merge.
    if kind == "DataModel" && operation != Operation::Delete {
        check_one_model(state, project, &envelope.metadata.name, &envelope.spec)?;
    }

    // 4b-writes. A write nobody can make (PL-111, ADR-N-057 decision 5): a write Policy no
    //       Endpoint binds, a Pipeline output whose targetEndpoint does not bind its write. The
    //       rule `jcctl validate` runs, over the project with this change in place.
    check_unmade_writes(state, project, operation, envelope)?;

    // 4b'. A public dashboard reads only through public Endpoints (UI-19, T-0528): the one
    //      rule that spans three manifests, so it is checked against the mirror here.
    crate::dashboards::check(
        &state.mirror,
        project,
        kind,
        &envelope.metadata.name,
        &envelope.spec,
    )?;

    // 4b-quota. What the project may hold (PF-73, PF-74): the count the repository would have
    //       after this write, against the quota in force. Refused here, before a Change exists,
    //       so every door - the route, an operation, the assistant, an import - refuses it.
    if operation != Operation::Delete {
        crate::quotas::check(
            &state.mirror,
            project,
            kind,
            &envelope.metadata.name,
            &envelope.spec,
        )?;
    }

    // 4b-mqtt. A broker inside what the project's MQTT allowance leaves, and a project's own
    //       allowance written only by an org admin (PF-117, PF-118, ADR-N-048 §2.9).
    if operation != Operation::Delete {
        crate::mqtt_allowance::check_manifest(state, identity, project, envelope)?;
    }

    // 4b-contact. A catalogue record publishes its contact point to everyone, so a person's own
    //       address is refused at every door (EP-80, T-2789).
    if operation != Operation::Delete {
        crate::catalog::check_contact(&state.mirror, identity, kind, &envelope.spec)?;
        crate::catalog::check_publication(kind, &envelope.spec)?;
    }
    Ok(())
}

/// The Keycloak client id `{project}-{name}` derives, when another project's ServiceAccount
/// already derives it.
fn service_account_id_taken(
    mirror: &crate::store::Mirror,
    project: &str,
    name: &str,
) -> Option<String> {
    use jc_core::kinds::service_account::keycloak_client_id;
    let id = keycloak_client_id(project, name);
    let taken = mirror.namespaces().into_iter().any(|namespace| {
        mirror
            .list(
                &namespace,
                "ServiceAccount",
                &crate::store::ListOptions::default(),
            )
            .items
            .iter()
            .any(|account| {
                (namespace.as_str(), account.metadata.name.as_str()) != (project, name)
                    && keycloak_client_id(&namespace, &account.metadata.name) == id
            })
    });
    taken.then_some(id)
}

/// The first of `spec.access.operations` this Portal does not register (MF-40).
fn unknown_operation(spec: &Value) -> Option<&str> {
    spec.pointer("/access/operations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|name| crate::ops::find(name).is_none())
}

/// Refuses an `McpServer` whose `spec.allowedProjects` adds a project the proposer may not read,
/// by the project list's own rule (T-4948): the picker offers those alone, and an id typed
/// through the API meets the same answer. An unknown project reads like an unreadable one. A
/// project already admitted stays, so a later edit of the title still goes through.
fn check_admitted_projects(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    kind: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if kind != "McpServer" {
        return Ok(());
    }
    let listed = |spec: &serde_json::Value| -> Vec<String> {
        spec.get("allowedProjects")
            .and_then(serde_json::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|n| n.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let kept = state
        .mirror
        .get(project, kind, &envelope.metadata.name)
        .map(|stored| listed(&stored.spec))
        .unwrap_or_default();
    match listed(&envelope.spec)
        .into_iter()
        .find(|name| !kept.contains(name) && !super::projects::readable(state, identity, name))
    {
        Some(name) => Err(ApiError::Invalid {
            detail: format!(
                "spec.allowedProjects names project '{name}', which is not one you may read; pick \
                 one from the list"
            ),
            errors: vec!["spec.allowedProjects".to_owned()],
        }),
        None => Ok(()),
    }
}

/// The names one organization shares (4e to 4i): Context Spaces, retention, brokers, Apps,
/// Groups and the pipelines that expire entities, checked for a create or an update.
async fn check_organization_names(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    kind: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    // 4e. A Context Space name is unique in the organization (PF-44, PF-76): a name another
    //     project holds is refused here — after the caller's right to propose one at all, so
    //     nobody probes the organization's names through this door, and before a Change exists,
    //     so every door answers the same refusal, the dry run included.
    crate::spaces::check(
        state,
        identity,
        project,
        kind,
        &envelope.metadata.name,
        &envelope.spec,
    )?;

    // 4e-retention. Retention deletes data: only the space's owner sets or changes it (DM-85).
    crate::spaces::check_retention(
        state,
        identity,
        project,
        kind,
        &envelope.metadata.name,
        &envelope.spec,
    )?;

    // 4e-broker. An Endpoint serves only what its space's broker serves (PF-110), and a space
    //       holding entities changes broker only by the move job (PF-115, ADR-N-047).
    crate::brokers::check_endpoint(&state.mirror, project, kind, &envelope.spec)?;
    crate::brokers::check_move(
        state,
        project,
        kind,
        &envelope.metadata.name,
        &envelope.spec,
    )
    .await?;

    // 4f. An App name is one address for the organization (AP-14a): `/apps/{name}/` carries no
    //     project, so another project's App of the same name is refused before a Change exists.
    crate::apps::names::check(state, identity, project, kind, &envelope.metadata.name)?;

    // 4f-mcp. A named MCP server admits only projects its proposer reads (T-4948).
    check_admitted_projects(state, identity, project, kind, envelope)?;

    // 4g. A Group name is one Keycloak group for the organization (AP-115): one owned by the
    //     organization or another App, or held by the realm unmanaged, is refused here.
    crate::groups::check(state, identity, kind, &envelope.metadata)?;

    // 4i. A pipeline that removes stale entities removes only its own, by a grant its author
    //     wrote (PL-64): refused while another pipeline writes the space, a type is not the
    //     model's, or no Policy grants the delete.
    crate::pipeline_expiry::check(
        state,
        project,
        kind,
        &envelope.metadata.name,
        &envelope.spec,
    )?;

    // 4j. A lifetime names only what the spaces it lands in hold (PL-81): each type a class of
    //     the space's model, each name one of its slots, each `from` a date-time slot.
    crate::pipeline_lifetime::check(state, project, kind, &envelope.spec)?;
    // 4k. A history plan names only the slots of the spaces it lands in (PL-106).
    crate::pipeline_history::check(state, project, kind, &envelope.spec)?;
    Ok(())
}

/// The grants one App proposal commits and removes (CC-61, AP-96).
#[derive(Default)]
struct AppGrants {
    /// `(path, yaml)` of the App's Endpoint and every Policy it compiles to.
    uploads: Vec<(String, String)>,
    /// Paths of generated grants of this App that it no longer compiles to: a role taken away,
    /// a need removed, or the App leaving `preview`/`published`, where it grants nothing.
    removed: Vec<String>,
    /// One line per grant, so the review names who may do what (AP-98).
    review: Vec<plan::FieldChange>,
}

/// Compiles an App's Endpoint and Policies for the change that proposes it (T-2632).
///
/// The slug is the App's Endpoint's own once it has one, and a fresh one (EP-02) the first time,
/// so republishing never moves the endpoint under its readers. Only what the reconciler generated
/// is replaced or removed: an Endpoint or Policy of the same name somebody wrote by hand is
/// refused, never overwritten.
fn app_grants(
    state: &AppState,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<AppGrants, ApiError> {
    use crate::apps::reconciler::{generate_slug, grants, RenderError, GENERATOR};
    use jc_core::annotations::GENERATED_BY;

    let name = &envelope.metadata.name;
    let endpoint_name = format!("app-{name}");
    let generated = |env: &ResourceEnvelope| {
        env.metadata
            .annotations
            .get(GENERATED_BY)
            .map(String::as_str)
            == Some(GENERATOR)
    };
    let current_endpoint = state.mirror.get(project, "Endpoint", &endpoint_name);
    if let Some(endpoint) = current_endpoint.as_ref().filter(|env| !generated(env)) {
        return Err(ApiError::Conflict(format!(
            "Endpoint '{}' in project '{project}' was not generated for App '{name}', and the \
             App's own endpoint has that name: rename one of them (AP-04, T-2632)",
            endpoint.metadata.name
        )));
    }
    let slug = current_endpoint
        .as_ref()
        .and_then(|env| env.spec.get("slug").and_then(Value::as_str))
        .and_then(|slug| jc_core::kinds::EndpointSlug::new(slug).ok())
        .unwrap_or_else(generate_slug);

    let manifest: jcctl::loader::RawManifest = serde_json::from_value(
        serde_json::to_value(envelope).map_err(|e| ApiError::Internal(e.to_string()))?,
    )
    .map_err(|e| ApiError::Internal(format!("the App as a manifest: {e}")))?;
    // The assigner of the App's Policies is the project's publisher (PF-108).
    let domain = crate::api::assistant::publisher_domain(state, project);
    let compiled = match grants(&manifest, &slug, &domain) {
        Ok((endpoint, policies)) => std::iter::once(endpoint).chain(policies).collect(),
        // A draft or a retired App grants nothing (AP-18, AP-21).
        Err(RenderError::NotDeployable { .. }) => Vec::new(),
        Err(err) => return Err(ApiError::BadRequest(err.to_string())),
    };

    let mut out = AppGrants::default();
    let mut kept = std::collections::BTreeSet::new();
    for raw in compiled {
        let env: ResourceEnvelope = serde_json::from_value(
            serde_json::to_value(&raw).map_err(|e| ApiError::Internal(e.to_string()))?,
        )
        .map_err(|e| ApiError::Internal(format!("a generated {}: {e}", raw.kind)))?;
        if raw.kind == "Policy" {
            if let Some(held) = state
                .mirror
                .get(project, "Policy", &env.metadata.name)
                .filter(|held| !generated(held))
            {
                return Err(ApiError::Conflict(format!(
                    "Policy '{}' in project '{project}' was not generated for App '{name}' and \
                     has the name its grant needs: rename it (AP-05, T-2632)",
                    held.metadata.name
                )));
            }
            out.review.push(plan::FieldChange {
                path: format!("grants.{}", env.metadata.name),
                from: None,
                to: Some(Value::from(review_line(name, &env.spec))),
            });
        }
        // The same check the App itself passed: a grant the gateway's loader would refuse
        // fails here, as a form error, not after the approval.
        if let Some(checked) = jc_core::registry::validate_yaml(
            &raw.kind,
            &serde_json::to_string(&env).map_err(|e| ApiError::Internal(e.to_string()))?,
        ) {
            checked.map_err(|e| {
                ApiError::BadRequest(format!("the App's {} is not valid: {e}", raw.kind))
            })?;
        }
        let info = resource::by_kind(&raw.kind)
            .ok_or_else(|| ApiError::Internal(format!("no catalogue entry for {}", raw.kind)))?;
        let path = resolve_repo_path(&env, info, project)?;
        kept.insert((raw.kind.clone(), env.metadata.name.clone()));
        let yaml = serde_norway::to_string(&env)
            .map_err(|e| ApiError::Internal(format!("serialize {}: {e}", raw.kind)))?;
        out.uploads.push((path, yaml));
    }

    for held in held_grants(&state.mirror, project, name) {
        if kept.contains(&(held.kind.clone(), held.metadata.name.clone())) {
            continue;
        }
        let info = resource::by_kind(&held.kind)
            .ok_or_else(|| ApiError::Internal(format!("no catalogue entry for {}", held.kind)))?;
        out.removed.push(resolve_repo_path(&held, info, project)?);
        out.review.push(plan::FieldChange {
            path: format!("grants.{}", held.metadata.name),
            from: Some(Value::from(held.kind.clone())),
            to: None,
        });
    }
    Ok(out)
}

/// The default groups an App's Change writes and removes (AP-118), as files and review lines
/// beside its grants; a group removed with members in it is said in the plan.
fn group_files(
    groups: &crate::apps::default_groups::DefaultGroups,
    project: &str,
    grants: &mut AppGrants,
) -> Result<(), ApiError> {
    let info = resource::by_kind("Group")
        .ok_or_else(|| ApiError::Internal("no catalogue entry for Group".into()))?;
    for group in &groups.written {
        let yaml = serde_norway::to_string(group)
            .map_err(|e| ApiError::Internal(format!("serialize Group: {e}")))?;
        grants
            .uploads
            .push((resolve_repo_path(group, info, project)?, yaml));
        grants.review.push(plan::FieldChange {
            path: format!("groups.{}", group.metadata.name),
            from: None,
            to: Some(Value::from("an empty default group, ready to assign")),
        });
    }
    for group in &groups.removed {
        grants
            .removed
            .push(resolve_repo_path(group, info, project)?);
        grants.review.push(plan::FieldChange {
            path: format!("groups.{}", group.metadata.name),
            from: Some(Value::from("Group")),
            to: None,
        });
    }
    for (index, warning) in groups.warnings.iter().enumerate() {
        grants.review.push(plan::FieldChange {
            path: format!("warnings.groups.{index}"),
            from: None,
            to: Some(Value::from(warning.clone())),
        });
    }
    Ok(())
}

/// The Endpoint and Policies the reconciler generated for App `app` of `project` (T-2632).
///
/// A generated Policy is the App's when it is granted to the App's endpoint, one of its roles, or
/// its service account: by name alone, `app-bikes-2-1` could be App `bikes-2`'s.
pub(crate) fn held_grants(
    mirror: &crate::store::Mirror,
    project: &str,
    app: &str,
) -> Vec<ResourceEnvelope> {
    use jc_core::annotations::GENERATED_BY;
    let endpoint_name = format!("app-{app}");
    let caller = jc_core::kinds::endpoint_role(project, &endpoint_name, None);
    let role_of_app = format!("{caller}/");
    let this_apps = |assignee: &Value| {
        let id = assignee["id"].as_str().unwrap_or_default();
        match assignee["kind"].as_str() {
            Some("role") => id == caller || id.starts_with(&role_of_app),
            Some("serviceAccount") => id == endpoint_name,
            _ => false,
        }
    };
    mirror.matching(|env| {
        env.metadata.namespace.as_deref() == Some(project)
            && env
                .metadata
                .annotations
                .get(GENERATED_BY)
                .map(String::as_str)
                == Some(crate::apps::reconciler::GENERATOR)
            && ((env.kind == "Endpoint" && env.metadata.name == endpoint_name)
                || (env.kind == "Policy" && this_apps(&env.spec["assignee"])))
    })
}

/// One grant as a reviewer reads it: who, which operations, on which types (AP-98).
fn review_line(app: &str, policy: &Value) -> String {
    let join = |value: &Value, key: &str| {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        if key.is_empty() {
                            item.as_str().map(str::to_owned)
                        } else {
                            item.get(key).and_then(Value::as_str).map(str::to_owned)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let operations = join(&policy["operations"], "");
    let types = join(&policy["information"][0]["entities"], "type");
    let who = policy["assignee"]["id"]
        .as_str()
        .and_then(|id| id.rsplit_once('/'))
        .map(|(_, last)| last.to_owned())
        .filter(|last| last != &format!("app-{app}"));
    match who {
        Some(role) => format!("role {role} of {app} can {operations} {types}"),
        None => format!("everyone who can open {app} can {operations} {types}"),
    }
}

/// What a body's `files` member commits beside the manifest, each path resolved against the
/// manifest's own folder (DM-39, `API/01 §4`).
///
/// A Mapping cannot be accepted without a golden test, and the two documents that test reads are
/// files, not manifest fields. Rather than a route per kind that needs one, the propose body
/// carries them and the Change commits them to the same branch. Every path stays under the
/// manifest's folder: the body decides what a change contains, so a path that climbs out of it
/// would let a proposal of a Mapping rewrite a Policy, a Role binding or the project file, under
/// the propose permission of a different kind.
const MAX_SIDECARS: usize = 16;
const MAX_SIDECAR_BYTES: usize = 256 * 1024;

/// A `ModelProjection` against the LinkML of the DataModel version it references (MP-01).
/// Refuses an Organization spec that sets a limit outside the operator's bounds, naming the
/// entry, the value, the bound and who moves the bound (PF-97, ADR-N-035). Any other kind passes.
/// `refusal` is the answer's kind: a form error on a door, a conflict at approval.
pub(crate) fn organization_within_bounds(
    state: &AppState,
    kind: &str,
    spec: &Value,
    refusal: fn(String) -> ApiError,
) -> Result<(), ApiError> {
    if kind != "Organization" {
        return Ok(());
    }
    // The kind's own parse ran before; a spec that does not parse was refused there.
    let Ok(spec) = serde_json::from_value::<jc_core::kinds::OrganizationSpec>(spec.clone()) else {
        return Ok(());
    };
    spec.check_limits(&state.config.organization_bounds)
        .map_err(|e| {
            refusal(format!(
                "{e}; an organization sets a value inside the bound, and the operator moves the \
                 bound in the deployment (portal.organizationBounds), never the Portal (PF-97)"
            ))
        })
}

/// Whether the Organization refuses public Apps (`spec.policies.apps.public`, PF-103).
pub(crate) fn public_apps_refused(state: &AppState) -> bool {
    use jc_core::kinds::{OrganizationPolicies, PublicApps};
    state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "Organization",
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .find_map(|org| {
            serde_json::from_value::<OrganizationPolicies>(org.spec.get("policies")?.clone()).ok()
        })
        .is_some_and(|policies| policies.apps.public == PublicApps::Refused)
}

/// An App made public while the organization refuses public Apps (`spec.policies.apps.public`,
/// PF-100, PF-103). An App that is public already stays as it is: the policy stops new public
/// Apps, it takes nothing offline, so an edit that keeps it public passes.
pub(crate) fn public_app_allowed(
    state: &AppState,
    project: &str,
    envelope: &ResourceEnvelope,
    refusal: fn(String) -> ApiError,
) -> Result<(), ApiError> {
    let public = |spec: &Value| spec.get("visibility").and_then(Value::as_str) == Some("public");
    if envelope.kind != "App" || !public(&envelope.spec) {
        return Ok(());
    }
    let name = &envelope.metadata.name;
    if state
        .mirror
        .get(project, "App", name)
        .is_some_and(|stored| public(&stored.spec))
    {
        return Ok(());
    }
    if public_apps_refused(state) {
        return Err(refusal(format!(
            "App '{name}' may not be public: the organization refuses public Apps \
             (spec.policies.apps.public). Keep it visible to the project or the organization, or \
             ask an organization admin to allow public Apps in Organization settings (PF-103)"
        )));
    }
    Ok(())
}

/// Refuses a Pipeline proposal that, with the project's other Pipelines, makes a replace output
/// meet another Pipeline's writes of the same ids in its space (PL-79), whichever side it is on. A
/// spec or an Endpoint that does not parse is judged elsewhere.
fn check_replace(
    state: &AppState,
    project: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    use jc_core::kinds::{replace_conflicts, EndpointSpec, PipelineSpec};
    let Ok(proposed) = serde_json::from_value::<PipelineSpec>(spec.clone()) else {
        return Ok(());
    };
    let options = crate::store::ListOptions::default();
    let others: Vec<(String, PipelineSpec)> = state
        .mirror
        .list(project, "Pipeline", &options)
        .items
        .into_iter()
        .filter(|p| p.metadata.name != name)
        .filter_map(|p| Some((p.metadata.name, serde_json::from_value(p.spec).ok()?)))
        .collect();
    let spaces: std::collections::BTreeMap<String, String> = state
        .mirror
        .list(project, "Endpoint", &options)
        .items
        .into_iter()
        .filter_map(|e| {
            let spec = serde_json::from_value::<EndpointSpec>(e.spec).ok()?;
            Some((e.metadata.name, spec.context_space_ref.name().to_owned()))
        })
        .collect();
    let mut specs: Vec<(&str, &PipelineSpec)> = vec![(name, &proposed)];
    specs.extend(others.iter().map(|(n, s)| (n.as_str(), s)));
    let conflict = replace_conflicts(&specs, |endpoint| spaces.get(endpoint).cloned())
        .into_iter()
        .find(|c| c.pipeline == name || c.others.iter().any(|o| o == name));
    match conflict {
        Some(conflict) => Err(ApiError::BadRequest(conflict.to_string())),
        None => Ok(()),
    }
}

/// The kinds whose change can leave a write nobody can make (PL-111): the space segment of a
/// policyRef, the Endpoints that bind, the Policies and Pipelines that write, an App's own Endpoint.
const WRITE_BINDING_KINDS: [&str; 5] = ["ContextSpace", "Endpoint", "Policy", "Pipeline", "App"];

/// The drafts proposed in the same Change that stand in for what the mirror holds of them.
fn bundled_bindings(project: &str) -> Vec<ResourceEnvelope> {
    crate::references::bundled()
        .into_iter()
        .filter_map(|m| serde_json::from_value::<ResourceEnvelope>(m).ok())
        .filter(|e| WRITE_BINDING_KINDS.contains(&e.kind.as_str()))
        .map(|mut e| {
            e.metadata.namespace = Some(project.to_owned());
            e.status = None;
            e
        })
        .collect()
}

/// Refuses a Policy reference the proposed Endpoint would not bind (EP-117, T-6435): another
/// organization's, another space's (a person writes the space's name, the URN wants its rendered
/// segment), or one naming no Policy of the space in the mirror or the same Change. Such an
/// Endpoint plans, merges and goes Live granting nothing. Each line names the URN to write. A
/// reference the Endpoint already holds passes, so an unrelated edit can still land.
fn check_policy_refs(
    state: &AppState,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    use jc_core::kinds::EndpointSpec;
    let parsed = |spec: Value| serde_json::from_value::<EndpointSpec>(spec).ok();
    let Some(spec) = parsed(envelope.spec.clone()) else {
        return Ok(());
    };
    let name = &envelope.metadata.name;
    let held = state
        .mirror
        .get(project, "Endpoint", name)
        .and_then(|endpoint| parsed(endpoint.spec));
    let held = held.as_ref().and_then(EndpointSpec::policy_refs);
    let fresh: Vec<&jc_core::Urn> = spec
        .policy_refs()
        .unwrap_or_default()
        .iter()
        .filter(|reference| !held.is_some_and(|held| held.contains(reference)))
        .collect();
    if fresh.is_empty() {
        return Ok(());
    }
    let space = spec.context_space_ref.name();
    let bundle = bundled_bindings(project);
    let segment = bundle
        .iter()
        .find(|e| e.kind == "ContextSpace" && e.metadata.name == space)
        .map_or_else(
            || crate::spaces::segment(&state.mirror, project, space),
            crate::spaces::segment_of,
        );
    let mirrored = state
        .mirror
        .list(project, "Policy", &crate::store::ListOptions::default());
    let policies: std::collections::BTreeSet<String> = mirrored
        .items
        .into_iter()
        .chain(bundle.into_iter().filter(|e| e.kind == "Policy"))
        .filter(|e| {
            crate::api::assistant::ref_name(&e.spec["contextSpaceRef"]).as_deref() == Some(space)
        })
        .map(|e| e.metadata.name)
        .collect();
    // With Organizations and none the installation's, no domain is known to be the binder's: the
    // reference keeps its own, and only its segment and name are judged.
    let unclaimed = crate::permissions::installation_organization(&state.mirror).is_none()
        && !crate::permissions::organizations(&state.mirror).is_empty();
    let domain = match unclaimed {
        true => String::new(),
        false => crate::api::assistant::org_domain(state, ""),
    };
    let field = match spec.policy_ref {
        Some(_) => "spec.policyRef",
        None => "spec.policyRefs",
    };
    let lines: Vec<String> = fresh
        .into_iter()
        .filter_map(|reference| {
            let domain = Some(domain.as_str())
                .filter(|domain| !domain.is_empty())
                .or(reference.org_domain())
                .unwrap_or_default();
            let policy = reference.local_id();
            let urn = format!("urn:ngsi-ld:Policy:{domain}:{segment}:{policy}");
            if !policies.contains(policy) {
                let known = policies.iter().cloned().collect::<Vec<_>>().join(", ");
                return Some(format!(
                    "{field}: {reference} names no Policy of the space {space}, so this Endpoint \
                     would grant nothing: propose the Policy {policy} of that space in the same \
                     plan and write {urn}, or name one the space holds ({known})"
                ));
            }
            (reference.to_string() != urn).then(|| {
                format!(
                    "{field}: {reference} binds nothing, so this Endpoint would grant nothing: \
                     the space {space} renders as the segment {segment} in a URN, write {urn}"
                )
            })
        })
        .collect();
    match lines.is_empty() {
        true => Ok(()),
        false => Err(unprocessable(lines)),
    }
}

/// Refuses a change after which a write Policy no Endpoint of its project binds, or a Pipeline
/// output whose `targetEndpoint` does not grant its account the write (PL-111, ADR-N-057
/// decision 5), with `jcctl validate`'s sentences. Only the findings this change brings count, so a
/// project that already holds one can still be repaired one manifest at a time. Each line starts
/// with the field to change when the finding is on the proposed manifest or it is an Endpoint.
fn check_unmade_writes(
    state: &AppState,
    project: &str,
    operation: Operation,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if !WRITE_BINDING_KINDS.contains(&envelope.kind.as_str()) {
        return Ok(());
    }
    let options = crate::store::ListOptions::default();
    let bundle = bundled_bindings(project);
    let bundled = |e: &ResourceEnvelope| {
        bundle
            .iter()
            .any(|b| b.kind == e.kind && b.metadata.name == e.metadata.name)
    };
    let current: Vec<ResourceEnvelope> = WRITE_BINDING_KINDS
        .iter()
        .flat_map(|kind| state.mirror.list(project, kind, &options).items)
        .collect();
    let same =
        |e: &ResourceEnvelope| e.kind == envelope.kind && e.metadata.name == envelope.metadata.name;
    let mut proposed: Vec<ResourceEnvelope> = current
        .iter()
        .filter(|e| !same(e) && !bundled(e))
        .chain(bundle.iter().filter(|e| !same(e)))
        .cloned()
        .collect();
    if operation != Operation::Delete {
        let mut mine = envelope.clone();
        mine.metadata.namespace = Some(project.to_owned());
        mine.status = None;
        proposed.push(mine);
    }
    let domain = crate::api::assistant::org_domain(state, "");
    let domain = Some(domain.as_str()).filter(|d| !d.is_empty());
    let before = unmade_writes(&current, domain);
    let field = |unmade: &jcctl::commands::validate::UnmadeWrite| {
        if unmade.id.kind == envelope.kind && unmade.id.name == envelope.metadata.name {
            return unmade.field.clone();
        }
        (envelope.kind == "Endpoint" && operation != Operation::Delete).then(|| {
            match envelope.spec.get("policyRef") {
                Some(_) => "spec.policyRef".to_owned(),
                None => "spec.contextSpaceRef".to_owned(),
            }
        })
    };
    let lines: Vec<String> = unmade_writes(&proposed, domain)
        .into_iter()
        .filter(|unmade| !before.contains(unmade))
        .map(|unmade| match field(&unmade) {
            Some(field) => format!("{field}: {}", unmade.message),
            None => unmade.message,
        })
        .collect();
    match lines.is_empty() {
        true => Ok(()),
        false => Err(unprocessable(lines)),
    }
}

/// Refuses a DataModel that would be a second own model of its space (DM-61): the space's types
/// are the classes of the one model it holds, so the classes of this one would never be. The
/// drafts of the same Change count beside the mirror; a mirrored peer copy (DM-48) is none.
fn check_one_model(
    state: &AppState,
    project: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    use crate::api::assistant::ref_name;
    let own = |spec: &Value| spec["lifecycle"] != "mirrored";
    let Some(space) = ref_name(&spec["contextSpaceRef"]).filter(|_| own(spec)) else {
        return Ok(());
    };
    let of_space = |kind: &str, model: &str, spec: &Value| {
        kind == "DataModel"
            && model != name
            && own(spec)
            && ref_name(&spec["contextSpaceRef"]).as_deref() == Some(space.as_str())
    };
    let in_project = |m: &ResourceEnvelope| m.metadata.namespace.as_deref() == Some(project);
    // A model the space already counts adds no second one: a repository that holds two from
    // before this check can still change either.
    let counted = state.mirror.find(|m| {
        in_project(m)
            && m.kind == "DataModel"
            && m.metadata.name == name
            && own(&m.spec)
            && ref_name(&m.spec["contextSpaceRef"]).as_deref() == Some(space.as_str())
    });
    if counted.is_some() {
        return Ok(());
    }
    let held = state
        .mirror
        .find(|m| in_project(m) && of_space(&m.kind, &m.metadata.name, &m.spec))
        .map(|m| m.metadata.name)
        .or_else(|| {
            crate::references::bundled().into_iter().find_map(|m| {
                let model = m["metadata"]["name"].as_str()?;
                let kind = m["kind"].as_str()?;
                of_space(kind, model, &m["spec"]).then(|| model.to_owned())
            })
        });
    match held {
        None => Ok(()),
        Some(first) => Err(unprocessable(vec![format!(
            "spec.contextSpaceRef: DataModel `{name}` would be a second model of space `{space}` \
             beside `{first}`; a space has one model and its classes are the space's types \
             (DM-61), so Pipelines and Endpoints would refuse the classes of `{name}`. Add the \
             classes to DataModel `{first}` (change its source), or give `{name}` a space of its own"
        )])),
    }
}

/// [`jcctl::commands::validate::unmade_writes`] over manifests of the mirror; a manifest that is no
/// manifest, or a set the loader refuses, is judged by the checks that own it and yields nothing.
fn unmade_writes(
    envelopes: &[ResourceEnvelope],
    domain: Option<&str>,
) -> Vec<jcctl::commands::validate::UnmadeWrite> {
    let manifests = envelopes
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .filter_map(|v| serde_json::from_value::<jcctl::RawManifest>(v).ok())
        .collect();
    jcctl::Repository::from_manifests(manifests, domain)
        .map(|repo| jcctl::commands::validate::unmade_writes(&repo))
        .unwrap_or_default()
}

/// Refuses an Endpoint whose representation mapping does not fit its space's model (EP-125), with
/// `jcctl validate`'s sentences, one line per gap. A spec that does not parse is judged by the kind's own check.
async fn check_surfaces(
    state: &AppState,
    project: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    use jc_core::kinds::EndpointSpec;
    let Ok(endpoint) = serde_json::from_value::<EndpointSpec>(spec.clone()) else {
        return Ok(());
    };
    let Some(wanted) = endpoint
        .served_representations()
        .into_iter()
        .find(|r| r.needs_model())
    else {
        return Ok(());
    };
    let space = endpoint.context_space_ref.name();
    let model = crate::api::surface_fit::backing_model(state, project, space);
    if let Some(gap) = model.as_ref().and_then(|(lifecycle, _)| {
        crate::api::surface_fit::lifecycle_gap(wanted, space, *lifecycle)
    }) {
        return Err(ApiError::BadRequest(format!(
            "Endpoint {project}/{name}: {gap}"
        )));
    }
    let linkml = match model {
        Some((_, model)) => {
            Some(crate::api::datamodels::read_source(state, project, &model).await?)
        }
        None => None,
    };
    let at = format!("Endpoint {project}/{name}, space {space}");
    match endpoint.representation_gaps(linkml.as_deref()) {
        Ok(gaps) if gaps.is_empty() => Ok(()),
        Ok(gaps) => Err(unprocessable(
            gaps.iter()
                .map(|gap| format!("{at}: {gap} (EP-125)"))
                .collect(),
        )),
        Err(err) => Err(ApiError::BadRequest(format!(
            "{at}: the representations cannot be checked against the data model: {err} (EP-125)"
        ))),
    }
}

/// A 422 whose detail is every line and whose `errors` lists each, so the form's dry run shows
/// one finding per member (API/11 EP-159).
fn unprocessable(lines: Vec<String>) -> ApiError {
    ApiError::Unprocessable {
        detail: lines.join("\n"),
        errors: lines,
    }
}

/// The shape of `spec.representations` alone, before the kind's own parse: an unknown member, a
/// duplicate or malformed output name, a 21st output, a filter the broker would refuse. Each is
/// a 422 naming the member's path (API/11 EP-157, EP-159); the model check is `check_surfaces`.
fn check_representation_shape(spec: &Value) -> Result<(), ApiError> {
    use jc_core::kinds::{feed_gaps, outputs_gaps, widget_gaps, Representations};
    let Some(block) = spec.get("representations") else {
        return Ok(());
    };
    let representations: Representations = serde_json::from_value(block.clone())
        .map_err(|e| unprocessable(vec![format!("spec.representations: {e}")]))?;
    let gaps: Vec<String> = outputs_gaps(&representations.ical)
        .into_iter()
        .map(|gap| format!("spec.representations.ical: {gap}"))
        .chain(
            feed_gaps(&representations.feed, None)
                .map_err(|e| unprocessable(vec![format!("spec.representations.feed: {e}")]))?
                .into_iter()
                .map(|gap| format!("spec.representations.feed: {gap}")),
        )
        .chain(
            widget_gaps(&representations.widget, None)
                .map_err(|e| unprocessable(vec![format!("spec.representations.widget: {e}")]))?
                .into_iter()
                .map(|gap| format!("spec.representations.widget: {gap}")),
        )
        .collect();
    if gaps.is_empty() {
        Ok(())
    } else {
        Err(unprocessable(gaps))
    }
}

async fn check_projection(state: &AppState, project: &str, spec: &Value) -> Result<(), ApiError> {
    use jc_core::kinds::{DataModelSpec, ModelProjectionSpec};
    // The kind's own parse ran above; a spec it accepted parses here too.
    let projection: ModelProjectionSpec = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::BadRequest(format!("spec is not a valid ModelProjection: {e}")))?;
    let wanted = &projection.data_model_ref;
    let model = state
        .mirror
        .get(project, "DataModel", &wanted.name)
        .and_then(|model| serde_json::from_value::<DataModelSpec>(model.spec).ok())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "spec.dataModelRef names DataModel '{}', which this project does not declare \
                 (MP-01)",
                wanted.name
            ))
        })?;
    if model.version.major().to_string() != wanted.version {
        return Err(ApiError::BadRequest(format!(
            "spec.dataModelRef names version {} of DataModel '{}', which is at {} (MP-01)",
            wanted.version, wanted.name, model.version
        )));
    }
    let linkml = crate::api::datamodels::read_source(state, project, &wanted.name).await?;
    projection
        .check_against_linkml(&linkml)
        .map_err(|e| ApiError::BadRequest(format!("spec is not a valid ModelProjection: {e}")))
}

fn sidecars(
    files: Option<Value>,
    manifest_path: &str,
    name: &str,
) -> Result<Vec<(String, String)>, ApiError> {
    let Some(files) = files else {
        return Ok(Vec::new());
    };
    let files = files.as_object().ok_or_else(|| {
        ApiError::BadRequest(
            "'files' is an object of path to file content; see API/01 §4".to_owned(),
        )
    })?;
    if files.len() > MAX_SIDECARS {
        return Err(ApiError::BadRequest(format!(
            "a proposal carries at most {MAX_SIDECARS} files beside its manifest, not {}; a repository's worth of files is an import (POST …/import)",
            files.len()
        )));
    }
    let folder = manifest_path.rsplit_once('/').map_or("", |(dir, _)| dir);

    let mut written: Vec<(String, String)> = Vec::new();
    let mut bytes = 0usize;
    for (path, content) in files {
        let content = content.as_str().ok_or_else(|| {
            ApiError::BadRequest(format!(
                "file '{path}' is not text; its content is a string"
            ))
        })?;
        let relative = path.strip_prefix("./").unwrap_or(path);
        let bad = relative.is_empty()
            || relative.starts_with('/')
            || relative.contains('\\')
            || relative.contains('\0')
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..");
        if bad {
            return Err(ApiError::BadRequest(format!(
                "file path '{path}' is not a path under the manifest's own folder; it is relative, has no '..' and no leading '/'"
            )));
        }
        let full = format!("{folder}/{relative}");
        // The reconciler reads every YAML file of the repository as a manifest, so a YAML file
        // beside this one is only the kind's own native file; anything else would land a second
        // manifest no review names (F-POR2-004).
        let yaml = relative.ends_with(".yaml") || relative.ends_with(".yml");
        if yaml && !crate::api::import::owns_native(&full, manifest_path, name) {
            return Err(ApiError::BadRequest(format!(
                "files: '{path}' is a YAML file, and the only YAML file beside a manifest is its own native file named after it (for example ./{name}.linkml.yaml); propose another manifest on its own"
            )));
        }
        if written.iter().any(|(already, _)| already == &full) {
            return Err(ApiError::BadRequest(format!(
                "file path '{path}' is sent twice"
            )));
        }
        bytes += content.len();
        if bytes > MAX_SIDECAR_BYTES {
            return Err(ApiError::BadRequest(format!(
                "the files beside the manifest are more than {MAX_SIDECAR_BYTES} bytes together; a repository's worth of files is an import (POST …/import)"
            )));
        }
        written.push((full, content.to_owned()));
    }
    Ok(written)
}

/// The empty model a new space is created with, as files beside the space manifest, and the
/// space's `spec.dataModelRef` naming it (DM-61, DM-62).
///
/// A space that already names a model keeps it, and a space the project already holds a model
/// for gets no second one: the model is named by the space, so reusing another project's types
/// is an `imports` entry of this model, never a second model (ADR-N-033). The model is a draft
/// with no classes, the shape DM-57 gives a model created by name; its first class is written
/// in the model editor.
fn space_model(
    state: &AppState,
    project: &str,
    envelope: &mut ResourceEnvelope,
) -> Vec<(String, String)> {
    let space = envelope.metadata.name.clone();
    let named = envelope
        .spec
        .get("dataModelRef")
        .is_some_and(|reference| !reference.is_null());
    let held = state
        .mirror
        .find(|model| {
            model.kind == "DataModel"
                && model.metadata.namespace.as_deref() == Some(project)
                && crate::api::assistant::ref_name(&model.spec["contextSpaceRef"]).as_deref()
                    == Some(space.as_str())
        })
        .is_some();
    if named || held {
        return Vec::new();
    }
    let Some(spec) = envelope.spec.as_object_mut() else {
        return Vec::new();
    };
    spec.insert(
        "dataModelRef".into(),
        serde_json::json!({ "kind": "DataModel", "name": space }),
    );
    let manifest = serde_json::json!({
        "apiVersion": resource::API_VERSION,
        "kind": "DataModel",
        "metadata": { "name": space, "namespace": project },
        "spec": {
            "contextSpaceRef": space,
            "linkml": format!("./{space}.linkml.yaml"),
            "version": "0.1.0",
            "lifecycle": "draft",
            "classes": [],
        }
    });
    let base = match state.config.org_domain.as_deref() {
        Some(domain) if !domain.is_empty() => format!("https://{domain}/models/{project}/{space}"),
        _ => format!("urn:joinedcontext:model:{project}:{space}"),
    };
    // A prefix is an NCName, which a DNS label that starts with a digit is not.
    let prefix = if project.starts_with(|c: char| c.is_ascii_alphabetic()) {
        project.to_owned()
    } else {
        "local".to_owned()
    };
    let source = format!(
        "id: {base}\n\
         name: {space}\n\
         description: The types of the {space} space, one class per entity type (DM-61).\n\
         prefixes:\n\
         \x20 linkml: https://w3id.org/linkml/\n\
         \x20 ngsi-ld: https://uri.etsi.org/ngsi-ld/\n\
         \x20 {prefix}: {base}/\n\
         default_prefix: {prefix}\n\
         default_range: string\n\
         imports:\n\
         \x20 - linkml:types\n\
         \x20 - ngsi-ld-core\n\
         classes: {{}}\n"
    );
    let manifest = serde_norway::to_string(&manifest).unwrap_or_default();
    vec![
        (format!("datamodels/{space}.yaml"), manifest),
        (format!("datamodels/{space}.linkml.yaml"), source),
    ]
}

/// What a proposal of one resource sends: the manifest, and beside it the draft the form holds
/// (AG-61) and the files the manifest names but cannot contain (DM-39). Both are taken out of the
/// body before it is read as an envelope; the type is what the OpenAPI document says, so the
/// generated client sends them without a cast (T-1488).
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct ResourceProposal {
    #[serde(flatten)]
    pub manifest: ResourceEnvelope,
    /// The draft this proposal is made from; the proposal then takes the draft's own manifest.
    #[serde(default)]
    pub draft: Option<crate::ops::DraftRef>,
    /// Files beside the manifest, by path relative to its folder, each a text: at most 16 and
    /// 256 KiB together.
    #[serde(default)]
    pub files: Option<std::collections::BTreeMap<String, String>>,
}

/// The `draft` member a form sends beside its manifest (AG-61), taken out of the body.
fn take_draft(body: &mut Value) -> Option<Value> {
    body.as_object_mut().and_then(|map| map.remove("draft"))
}

/// A form proposing or checking the draft it holds: the registered operation of the kind
/// owns the check and the gate (PF-57), so this door reaches the same operation as MCP and
/// the ops route (ADR-N-021) instead of parsing the reference as a manifest. The body's
/// manifest is what a check verifies; a proposal takes the draft's own manifest.
#[allow(clippy::too_many_arguments)]
async fn propose_draft(
    user: CurrentUser,
    front: Front,
    state: &AppState,
    project: &str,
    plural: &str,
    dry_run: bool,
    draft: Value,
    manifest: Value,
) -> Result<Response, ApiError> {
    let name = match (plural, dry_run) {
        ("datasources", true) => "jc_datasource_check",
        (_, true) => "jc_manifest_dry_run",
        ("endpoints", false) => "jc_endpoint_propose",
        // Every other kind proposes through the registry's one propose (AG-77, ADR-N-021): the
        // draft names its own kind, so a Dashboard, a Role or an App draft reaches the operation
        // MCP and the assistant reach, instead of a 400 in the form alone (T-0842).
        (_, false) => "jc_resource_propose",
    };
    let op = crate::ops::find(name)
        .ok_or_else(|| ApiError::Internal(format!("operation '{name}' is not registered")))?;
    let caller = crate::ops::Caller {
        bearer: None,
        identity: user.0.identity,
        via: match front {
            Front::Portal | Front::Edge => crate::ops::Via::Session,
            Front::Bearer => crate::ops::Via::Bearer,
        },
        access: None,
    };
    let input = if dry_run {
        serde_json::json!({ "draft": draft, "manifest": manifest })
    } else {
        serde_json::json!({ "draft": draft })
    };
    // The pages read the same Change envelope the plain door answers (ChangeNotice, its
    // review link); the operation wraps it as `change` beside `changeId` and `lane`.
    let result = crate::ops::call(op, &caller, state, project, input)
        .await
        .map(
            |mut output| match output.get_mut("change").map(Value::take) {
                Some(change) if change.is_object() => change,
                _ => output,
            },
        );
    crate::api::ops::respond(op, result)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/{plural}",
    summary = "Propose Resource",
    description = "Proposes creating or changing a resource of any kind from its manifest or a draft; the change waits for a person's approval.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
        ("confirm" = Option<String>, Query, description = "The resource's name typed back: an administrator's own red-lane change is approved as it is proposed only with it (PF-58, CC-39)"),
    ),
    request_body(content = ResourceProposal, example = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Endpoint",
            "metadata": { "name": "helsinki-air", "namespace": "helsinki", "title": "Air quality" },
            "spec": {
                "contextSpaceRef": "air",
                "slug": "mluyob4nz52lok3ssk7pgn5vwt",
                "audience": "organization",
                "enabledRepresentations": ["ngsi-ld", "csv"]
            }
        })),
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run validation result", body = DryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 415, description = "Unsupported media type", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn create(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, plural)): Path<(String, String)>,
    Query(dry_run_q): Query<DryRunQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let is_dry = dry_run::is_dry_run(&dry_run_q)?;
    let mut body_val = parse_body_to_value(&headers, &body)?;
    // Inside a copy the draft a form names is the copy's own, and the manifest beside it is what
    // the copy takes: written to its branch like any manifest, never a Change of its own (T-4868).
    let draft = take_draft(&mut body_val);
    if let (Some(draft), None) = (draft, dry_run_q.workspace.as_ref()) {
        return propose_draft(
            user, front, &state, &project, &plural, is_dry, draft, body_val,
        )
        .await;
    }
    propose_checked(
        &user,
        front,
        &state,
        &project,
        &plural,
        None,
        Operation::Create,
        is_dry,
        body_val,
        dry_run_q.workspace.as_deref(),
        dry_run_q.confirm.as_deref(),
    )
    .await
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/{plural}/{name}",
    summary = "Propose Resource",
    description = "Proposes creating or changing a resource of any kind from its manifest or a draft; the change waits for a person's approval.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("name" = String, Path, description = "Resource name"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
        ("confirm" = Option<String>, Query, description = "The resource's name typed back: an administrator's own red-lane change is approved as it is proposed only with it (PF-58, CC-39)"),
        ("If-Match" = Option<String>, Header, description = "The ETag of the manifest as read; a stale one is 412 and opens no Change (API/13 section 3)"),
    ),
    request_body(content = ResourceProposal, example = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Endpoint",
            "metadata": { "name": "helsinki-air", "namespace": "helsinki", "title": "Air quality" },
            "spec": {
                "contextSpaceRef": "air",
                "slug": "mluyob4nz52lok3ssk7pgn5vwt",
                "audience": "organization",
                "enabledRepresentations": ["ngsi-ld", "csv"]
            }
        })),
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run validation result", body = DryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 412, description = "If-Match is not the manifest's current ETag, which the answer carries", body = ProblemDetails),
        (status = 415, description = "Unsupported media type", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn replace(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, plural, name)): Path<(String, String, String)>,
    Query(dry_run_q): Query<DryRunQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let is_dry = dry_run::is_dry_run(&dry_run_q)?;
    crate::api::if_match::check_if_match(
        &state,
        &user.0.identity,
        &project,
        &plural,
        &name,
        dry_run_q.workspace.as_deref(),
        &headers,
    )
    .await?;
    let mut body_val = parse_body_to_value(&headers, &body)?;
    // Inside a copy the draft a form names is the copy's own, and the manifest beside it is what
    // the copy takes: written to its branch like any manifest, never a Change of its own (T-4868).
    let draft = take_draft(&mut body_val);
    if let (Some(draft), None) = (draft, dry_run_q.workspace.as_ref()) {
        return propose_draft(
            user, front, &state, &project, &plural, is_dry, draft, body_val,
        )
        .await;
    }
    propose_checked(
        &user,
        front,
        &state,
        &project,
        &plural,
        Some(&name),
        Operation::Update,
        is_dry,
        body_val,
        dry_run_q.workspace.as_deref(),
        dry_run_q.confirm.as_deref(),
    )
    .await
}

#[utoipa::path(
    patch,
    path = "/api/v1/projects/{project}/{plural}/{name}",
    summary = "Propose Resource",
    description = "Proposes creating or changing a resource of any kind from its manifest or a draft; the change waits for a person's approval.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("name" = String, Path, description = "Resource name"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
        ("confirm" = Option<String>, Query, description = "The resource's name typed back: an administrator's own red-lane change is approved as it is proposed only with it (PF-58, CC-39)"),
        ("If-Match" = Option<String>, Header, description = "The ETag of the manifest as read; a stale one is 412 and opens no Change (API/13 section 3)"),
    ),
    // Declared by hand: the handler takes the raw `Bytes` because the media type decides how the
    // body is parsed, and utoipa cannot derive a schema from that extractor.
    request_body(
        content = Object,
        description = "RFC 7386 merge patch, as JSON or as the YAML apply-patch document",
        content_type = "application/merge-patch+json",
        example = json!({ "spec": { "audience": "organization" } }),
    ),
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run validation result", body = DryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 412, description = "If-Match is not the manifest's current ETag, which the answer carries", body = ProblemDetails),
        (status = 415, description = "Unsupported media type", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn patch(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, plural, name)): Path<(String, String, String)>,
    Query(dry_run_q): Query<DryRunQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let is_dry = dry_run::is_dry_run(&dry_run_q)?;
    crate::api::if_match::check_if_match(
        &state,
        &user.0.identity,
        &project,
        &plural,
        &name,
        dry_run_q.workspace.as_deref(),
        &headers,
    )
    .await?;
    let patch_val = parse_patch_to_value(&headers, &body)?;

    if patch_val.get("status").is_some() {
        return Err(ApiError::BadRequest(
            "status is computed by the platform and cannot be specified in the manifest (MF-04)"
                .into(),
        ));
    }

    if let Some(secret_key) = find_literal_secret(&patch_val) {
        return Err(ApiError::BadRequest(format!(
            "literal secret in field '{secret_key}' is forbidden; use secretRef instead (MF-24)"
        )));
    }

    let kind_info = resource::by_plural(&plural).ok_or_else(|| {
        ApiError::NotFound(format!(
            "plural '{plural}' not found in project '{project}'"
        ))
    })?;

    let mirror = match dry_run_q.workspace.as_deref() {
        Some(workspace) => std::sync::Arc::new(
            crate::ops::workspaces::mirror_of(&state, &user.0.identity, workspace, &project)
                .await?,
        ),
        None => state.mirror.clone(),
    };
    // A manifest the caller may not read is answered as absent, so a patch tells nobody what
    // exists (F-POR2-006).
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    let current = mirror
        .get(&project, kind_info.kind, &name)
        .filter(|current| {
            serde_json::to_value(current)
                .is_ok_and(|value| effective.may_read_manifest(kind_info.kind, &value))
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "resource '{name}' not found in project '{project}'"
            ))
        })?;

    let mut curr_to_patch = current;
    curr_to_patch.strip_status();
    let mut desired_val = serde_json::to_value(&curr_to_patch).map_err(|e| {
        ApiError::Internal(format!("failed to serialize current resource to json: {e}"))
    })?;

    plan::merge_patch(&mut desired_val, &patch_val);

    propose_checked(
        &user,
        front,
        &state,
        &project,
        &plural,
        Some(&name),
        Operation::Update,
        is_dry,
        desired_val,
        dry_run_q.workspace.as_deref(),
        dry_run_q.confirm.as_deref(),
    )
    .await
}

/// A Pipeline sent in the first shape, rewritten as the second (PL-54, ADR-N-023):
/// `source` → `sources: [source]`, `compute` → `steps: [compute]`, `targetEndpoint` with its
/// `output` → `outputs: [{ targetEndpoint, type, mode }]`, and `apiVersion` v1alpha2. Anything
/// already in the second shape, mixed, or with no `source` (its input lives in `bento.yaml`,
/// which v1alpha2 has no place for) is left as it came, for validation to judge.
pub(crate) fn pipeline_second_shape(body: &mut Value) {
    if body.get("kind").and_then(Value::as_str) != Some("Pipeline") {
        return;
    }
    let Some(spec) = body.get_mut("spec").and_then(Value::as_object_mut) else {
        return;
    };
    let second = ["sources", "steps", "outputs"]
        .iter()
        .any(|key| spec.contains_key(*key));
    if second || !spec.contains_key("source") || !spec.contains_key("targetEndpoint") {
        return;
    }
    if let Some(source) = spec.remove("source") {
        spec.insert("sources".to_owned(), Value::Array(vec![source]));
    }
    if let Some(compute) = spec.remove("compute") {
        spec.insert("steps".to_owned(), Value::Array(vec![compute]));
    }
    let mut output = serde_json::Map::new();
    if let Some(target) = spec.remove("targetEndpoint") {
        output.insert("targetEndpoint".to_owned(), target);
    }
    if let Some(Value::Object(written)) = spec.remove("output") {
        output.extend(written);
    }
    spec.insert(
        "outputs".to_owned(),
        Value::Array(vec![Value::Object(output)]),
    );
    body["apiVersion"] = Value::String(jc_core::API_VERSION_V1ALPHA2.to_owned());
}

/// `sent` over `stored` (UI-50, T-2764): a plain string over a legacy language map goes into the
/// entry the map resolves to (`en`, else its first language) and the other languages stay. A map,
/// or a string over a string, is what the person wrote.
fn keep_languages(sent: &mut Option<jc_core::i18n::Text>, stored: Option<&jc_core::i18n::Text>) {
    use jc_core::i18n::Text;
    let (Some(Text::Plain(text)), Some(Text::Localized(map))) = (sent.as_ref(), stored) else {
        return;
    };
    let Some(locale) = map
        .get("en")
        .map(|_| "en".to_owned())
        .or_else(|| map.iter().next().map(|(locale, _)| locale.to_owned()))
    else {
        return;
    };
    let mut kept = map.clone();
    // The key came out of the map itself, so the insert cannot refuse it.
    if kept.insert(&locale, text.clone()).is_ok() {
        *sent = Some(Text::Localized(kept));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::{Identity, Session};

    /// T-6471: a Policy `jc_space_complete` filed under the space it names is found there, the
    /// path `resolve_repo_path` gives wins when it holds the file, and no file leaves the path.
    #[tokio::test]
    async fn held_file_finds_a_policy_in_either_folder() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let gitea =
            crate::git::GiteaClient::new(server.uri().parse().unwrap(), "o", "r", "token-xyz")
                .unwrap();
        let info = resource::by_kind("Policy").expect("Policy");
        let policy = |space: &str| ResourceEnvelope {
            api_version: "joinedcontext.com/v1alpha1".into(),
            kind: "Policy".into(),
            metadata: crate::resource::ObjectMeta {
                name: "grant".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec: serde_json::json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space } }),
            status: None,
        };
        let found = |sha: &str| {
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "sha": sha, "content": "YTogYg==" }))
        };
        let contents = "/api/v1/repos/o/r/contents/projects/helsinki/spaces";
        Mock::given(method("GET"))
            .and(path(format!("{contents}/excel/policies/grant.yaml")))
            .respond_with(found("in-space"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{contents}/helsinki/policies/grant.yaml")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let own = "projects/helsinki/spaces/helsinki/policies/grant.yaml";
        let held = "projects/helsinki/spaces/excel/policies/grant.yaml";

        let (at, file) = held_file(&gitea, own, &policy("excel"), info, "helsinki", "main")
            .await
            .unwrap();
        assert_eq!(at, held);
        assert_eq!(file.expect("the file").sha, "in-space");

        let (at, file) = held_file(&gitea, held, &policy("elsewhere"), info, "helsinki", "main")
            .await
            .unwrap();
        assert_eq!(at, held);
        assert!(file.is_some());

        let (at, file) = held_file(&gitea, own, &policy("empty"), info, "helsinki", "main")
            .await
            .unwrap();
        assert_eq!(at, own);
        assert!(file.is_none());
    }

    /// T-6451: main keeps `{orgDomain}`, the mirror the rendered domain; only a real move counts.
    #[test]
    fn an_org_domain_placeholder_on_main_is_no_move() {
        let manifest = |assigner: &str| {
            format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  name: p\n\
                 spec:\n  assigner: {assigner}\n"
            )
        };
        let file = |content: String| crate::git::RepoFile {
            sha: "s".to_owned(),
            content,
        };
        let judged: ResourceEnvelope =
            serde_norway::from_str(&manifest("did:web:hel.fi")).expect("manifest");
        let on_main = file(manifest("\"did:web:{orgDomain}\""));
        assert!(!moved_on_main(Some(&judged), Some(&on_main), "hel.fi"));
        assert!(moved_on_main(Some(&judged), Some(&on_main), "espoo.fi"));
        let moved = file(manifest("did:web:espoo.fi"));
        assert!(moved_on_main(Some(&judged), Some(&moved), "hel.fi"));
    }

    /// F-POR2-004: a YAML file beside a manifest is only the kind's own native file; a second
    /// manifest smuggled in as a sidecar is refused, a JSON test fixture in a subfolder is not.
    #[test]
    fn a_yaml_sidecar_is_only_the_kinds_own_native_file() {
        let at = "projects/p/spaces/s/datamodels/air.yaml";
        let refused = sidecars(
            Some(serde_json::json!({ "other.yaml": "kind: Policy\n" })),
            at,
            "air",
        );
        assert!(
            matches!(refused, Err(ApiError::BadRequest(ref m)) if m.starts_with("files:")),
            "{refused:?}"
        );
        assert!(sidecars(
            Some(serde_json::json!({ "./deep/x.yml": "a: 1\n" })),
            at,
            "air"
        )
        .is_err());
        let kept = sidecars(
            Some(serde_json::json!({
                "./air.linkml.yaml": "id: x\n",
                "./tests/air.input.json": "{}",
            })),
            at,
            "air",
        )
        .expect("the model's own files");
        assert_eq!(kept.len(), 2);
    }

    /// PF-121: an Organization loaded from the legacy `org.yaml` is written back there; any other
    /// Organization at `organizations/{name}.yaml`, so the repository never holds one twice.
    #[tokio::test]
    async fn an_organization_at_the_legacy_path_is_written_back_there() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let legacy = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  name: hel\nspec:\n  domain: hel.fi\n";
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/o/r/contents/org.yaml"))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "b1",
                "content": base64::engine::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    legacy.as_bytes()
                ),
            })))
            .mount(&server)
            .await;
        let gitea = crate::git::GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "o",
            "r",
            "token",
        )
        .expect("client");
        let path_of = |name: &str| {
            let info = resource::by_kind("Organization").expect("a kind");
            resource::repository_path(info, crate::permissions::ORG_NAMESPACE, None, name)
                .expect("a path")
        };
        assert_eq!(path_of("zilina"), "organizations/zilina.yaml");
        let hel = organization_path(&gitea, "main", "hel", path_of("hel"))
            .await
            .expect("answers");
        assert_eq!(hel, "org.yaml");
        let zilina = organization_path(&gitea, "main", "zilina", path_of("zilina"))
            .await
            .expect("answers");
        assert_eq!(zilina, "organizations/zilina.yaml");
    }

    use crate::change::Lane;
    use crate::config::Config;
    use crate::resource::ObjectMeta;
    use http_body_util::BodyExt;
    use serde_json::json;

    /// T-2716 (PF-97): the operator's bound holds an Organization's limit at approval too, as a
    /// conflict naming the entry, the value, the bound and who moves it; other kinds pass.
    #[test]
    fn an_organization_limit_outside_the_operators_bound_is_refused_with_who_moves_it() {
        let mut config = Config::for_tests();
        config.organization_bounds = serde_json::from_value(serde_json::json!({
            "spec.projects.quota.contextSpaces": { "max": 20 }
        }))
        .expect("bounds");
        let state = AppState::new(config, None);
        let spec = |spaces: u32| {
            serde_json::json!({
                "domain": "hel.fi",
                "locales": ["en"],
                "defaultLocale": "en",
                "projects": { "quota": { "contextSpaces": spaces } }
            })
        };
        let refused =
            organization_within_bounds(&state, "Organization", &spec(50), ApiError::Conflict)
                .expect_err("50 is over the operator's 20");
        let ApiError::Conflict(message) = refused else {
            panic!("a conflict at approval: {refused:?}");
        };
        for part in [
            "spec.projects.quota.contextSpaces",
            "50",
            "0 … 20",
            "portal.organizationBounds",
        ] {
            assert!(message.contains(part), "{part}: {message}");
        }
        assert!(
            organization_within_bounds(&state, "Organization", &spec(20), ApiError::Conflict)
                .is_ok()
        );
        assert!(
            organization_within_bounds(&state, "ContextSpace", &spec(50), ApiError::Conflict)
                .is_ok()
        );
        // Without an operator file a quota has no ceiling (ADR-N-035: the operator sets one).
        let open = AppState::new(Config::for_tests(), None);
        assert!(organization_within_bounds(
            &open,
            "Organization",
            &spec(500),
            ApiError::BadRequest
        )
        .is_ok());
    }

    /// T-2870 (PF-103): with public Apps refused, a Change that makes an App public is refused
    /// naming the setting and who changes it; an App public already keeps being edited, and an
    /// App kept to its project, another kind, or an organization that allows them all pass.
    #[test]
    fn a_new_public_app_is_refused_while_the_organization_refuses_public_apps() {
        let state = AppState::new(Config::for_tests(), None);
        let manifest = |kind: &str, name: &str, namespace: &str, spec: Value| ResourceEnvelope {
            api_version: resource::API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some(namespace.into()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        let app = |name: &str, visibility: &str| {
            manifest("App", name, "helsinki", json!({ "visibility": visibility }))
        };
        let organization = |public: &str| {
            manifest(
                "Organization",
                "hel-fi",
                crate::permissions::ORG_NAMESPACE,
                json!({ "policies": { "apps": { "public": public } } }),
            )
        };
        let check = |envelope: &ResourceEnvelope| {
            public_app_allowed(&state, "helsinki", envelope, ApiError::BadRequest)
        };

        // No Organization, or one that says nothing: public Apps are allowed (the default).
        assert!(check(&app("bikes", "public")).is_ok());
        state.mirror.upsert(organization("allowed"));
        assert!(check(&app("bikes", "public")).is_ok());

        state.mirror.upsert(organization("refused"));
        let refused = check(&app("bikes", "public")).expect_err("a new public App");
        let ApiError::BadRequest(message) = refused else {
            panic!("a bad request at the door: {refused:?}");
        };
        for part in [
            "App 'bikes'",
            "spec.policies.apps.public",
            "Organization settings",
        ] {
            assert!(message.contains(part), "{part}: {message}");
        }
        assert!(matches!(
            public_app_allowed(
                &state,
                "helsinki",
                &app("bikes", "public"),
                ApiError::Conflict
            ),
            Err(ApiError::Conflict(_))
        ));
        assert!(check(&app("bikes", "project")).is_ok());
        assert!(check(&manifest(
            "Endpoint",
            "bikes",
            "helsinki",
            json!({ "visibility": "public" })
        ))
        .is_ok());

        // Public before the policy: the policy takes nothing offline, so its edits pass; a private
        // App of the same name in another project is not that App.
        state.mirror.upsert(app("map", "public"));
        assert!(check(&app("map", "public")).is_ok());
        state.mirror.upsert(manifest(
            "App",
            "atlas",
            "espoo",
            json!({ "visibility": "public" }),
        ));
        assert!(check(&app("atlas", "public")).is_err());
    }

    /// T-4470: the parts `propose_engine` was cut into that read nothing but their input.
    #[test]
    fn a_derived_client_id_is_taken_only_by_another_projects_account() {
        let mirror = crate::store::Mirror::default();
        mirror.upsert(ResourceEnvelope {
            api_version: resource::API_VERSION.into(),
            kind: "ServiceAccount".into(),
            metadata: ObjectMeta {
                name: "c".into(),
                namespace: Some("a-b".into()),
                ..Default::default()
            },
            spec: json!({}),
            status: None,
        });
        // `a` + `b-c` and `a-b` + `c` both derive `a-b-c`.
        assert_eq!(
            service_account_id_taken(&mirror, "a", "b-c"),
            Some("a-b-c".to_owned())
        );
        assert_eq!(
            service_account_id_taken(&mirror, "a-b", "c"),
            None,
            "its own account"
        );
        assert_eq!(service_account_id_taken(&mirror, "a", "d"), None);
    }

    #[test]
    fn the_envelope_takes_this_project_and_refuses_another_kind_name_or_namespace() {
        let body = |kind: &str, name: &str, namespace: Option<&str>| {
            json!({ "apiVersion": resource::API_VERSION, "kind": kind,
                    "metadata": { "name": name, "namespace": namespace }, "spec": {} })
        };
        let fine = envelope_for(
            &body("Endpoint", "a", None),
            "Endpoint",
            "endpoints",
            "p",
            Some("a"),
        )
        .expect("an envelope");
        assert_eq!(fine.metadata.namespace.as_deref(), Some("p"));
        for (kind, name, namespace) in [
            ("Policy", "a", None),
            ("Endpoint", "b", None),
            ("Endpoint", "a", Some("q")),
        ] {
            assert!(envelope_for(
                &body(kind, name, namespace),
                "Endpoint",
                "endpoints",
                "p",
                Some("a")
            )
            .is_err());
        }
        let mut old = body("Endpoint", "a", None);
        old["apiVersion"] = json!("joinedcontext.com/v0");
        assert!(envelope_for(&old, "Endpoint", "endpoints", "p", None).is_err());
    }

    #[test]
    fn a_literal_secret_or_a_build_annotation_is_never_proposed() {
        let body = json!({ "apiVersion": resource::API_VERSION, "kind": "DataSource",
                           "metadata": { "name": "a" }, "spec": { "http": { "password": "hunter2" } } });
        let envelope: ResourceEnvelope = serde_json::from_value(body.clone()).expect("envelope");
        let refused = refuse_platform_fields(&body, &envelope).unwrap_err();
        assert!(
            !format!("{refused:?}").contains("hunter2"),
            "the value is not echoed"
        );
        let mut built = envelope.clone();
        built.metadata.annotations.insert(
            crate::apps::converge::BUILT_ANNOTATIONS[0].into(),
            "sha256:x".into(),
        );
        assert!(refuse_platform_fields(&json!({}), &built).is_err());
        assert!(refuse_platform_fields(&json!({}), &envelope).is_ok());
    }

    #[test]
    fn a_name_held_by_another_resource_of_the_kind_is_named_and_its_own_is_not() {
        let state = AppState::new(Config::for_tests(), None);
        let info = resource::by_kind("Endpoint").expect("Endpoint");
        let held: ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": resource::API_VERSION, "kind": "Endpoint",
            "metadata": { "name": "x", "namespace": "p" }, "spec": {}
        }))
        .expect("envelope");
        state.mirror.upsert(held);
        assert!(held_under_another_name(&state, info, "p", "y").is_some());
        assert!(held_under_another_name(&state, info, "p", "x").is_none());
        assert!(held_under_another_name(&state, info, "q", "y").is_none());
    }

    #[test]
    fn only_a_build_write_commits_a_status() {
        let envelope: ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": resource::API_VERSION, "kind": "App",
            "metadata": { "name": "a" }, "spec": {}, "status": { "phase": "Live" }
        }))
        .expect("envelope");
        assert!(!manifest_yaml(&envelope, false, None)
            .expect("yaml")
            .contains("status"));
        assert!(manifest_yaml(&envelope, true, None)
            .expect("yaml")
            .contains("status"));
    }

    #[test]
    fn an_agent_profile_names_the_first_operation_nobody_registered() {
        assert_eq!(unknown_operation(&json!({})), None);
        let spec = json!({ "access": { "operations": ["no.such.operation", 7] } });
        assert_eq!(unknown_operation(&spec), Some("no.such.operation"));
    }

    fn dummy_user() -> CurrentUser {
        CurrentUser(Session {
            identity: Identity {
                client: None,
                subject: "sub-123".into(),
                username: "demo.developer".into(),
                email: Some("demo@example.com".into()),
                name: Some("Demo Developer".into()),
                roles: vec![],
                groups: vec!["portal-approver".into()],
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy-token".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    /// T-3972: a credential field of a runner step names the pipeline's secret as `${VAR}`, which
    /// is a reference and no secret; jc-core holds which names a manifest may read (PL-15). A
    /// value that is anything more than one whole reference is still a literal.
    #[test]
    fn a_credential_in_a_request_header_is_a_literal_secret() {
        let spec = |headers: Value| json!({ "spec": { "http": { "headers": headers } } });
        for name in [
            "X-Api-Key",
            "x_api_key",
            "Authorization",
            "X-Auth-Token",
            "Cookie",
        ] {
            assert_eq!(
                find_literal_secret(&spec(json!({ name: "abc123pastedkey456" }))),
                Some(format!("headers.{name}"))
            );
            assert_eq!(
                find_literal_secret(&spec(json!({ name: "${API_KEY}" }))),
                None
            );
        }
        assert_eq!(
            find_literal_secret(&spec(json!({ "Accept": "text/csv", "User-Agent": "jc" }))),
            None
        );
    }

    #[test]
    fn a_whole_variable_reference_is_no_literal_secret() {
        let step = |password: Value| {
            json!({ "spec": {
                "secretRefs": [{ "name": "feed", "key": "pw", "envVar": "FEED_PW" }],
                "steps": [{ "processor": { "http": {
                    "url": "https://example.org",
                    "basic_auth": { "enabled": true, "username": "u", "password": password }
                } } }]
            } })
        };
        assert_eq!(find_literal_secret(&step(json!("${FEED_PW}"))), None);
        assert_eq!(find_literal_secret(&step(json!("${_PW2}"))), None);
        for literal in [
            json!("hunter2"),
            json!("x${FEED_PW}"),
            json!("${FEED_PW}x"),
            json!("${FEED_PW}${OTHER}"),
            json!("${FEED-PW}"),
            json!("${}"),
            json!("${! meta(\"pw\") }"),
            json!(1234),
        ] {
            assert_eq!(
                find_literal_secret(&step(literal.clone())),
                Some("password".into()),
                "{literal}"
            );
        }
    }

    #[test]
    fn detects_literal_secrets_and_permits_secret_refs() {
        assert_eq!(
            find_literal_secret(&json!({ "password": "supersecret" })),
            Some("password".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "spec": { "token": "ghp_123" } })),
            Some("token".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "spec": { "nested": { "clientSecret": "plain" } } })),
            Some("clientSecret".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "apiKey": "12345" })),
            Some("apiKey".into())
        );
        // T-2540, MF-24: a number or a boolean under a credential name is a literal too.
        assert_eq!(
            find_literal_secret(&json!({ "spec": { "password": 123456 } })),
            Some("password".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "spec": { "token": true } })),
            Some("token".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "spec": { "password": null } })),
            None
        );

        assert_eq!(
            find_literal_secret(&json!({
                "spec": {
                    "password": {
                        "secretRef": { "name": "db-secret", "key": "password" }
                    }
                }
            })),
            None
        );
        assert_eq!(
            find_literal_secret(&json!({
                "spec": {
                    "secretRef": { "name": "vault-key" }
                }
            })),
            None
        );
        assert_eq!(
            find_literal_secret(&json!({
                "metadata": { "name": "public-air" },
                "spec": { "audience": "public" }
            })),
            None
        );
    }

    /// EP-75 (T-6285): a raw manifest from a bearer caller names no slug of its own; the
    /// Portal's form, which mints the slug in the browser, still brings it.
    #[tokio::test]
    async fn a_bearer_caller_cannot_type_a_new_endpoints_slug() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();
        let typed = || {
            json!({
                "apiVersion": resource::API_VERSION,
                "kind": "Endpoint",
                "metadata": { "name": "t6242-probe", "namespace": "ovzdusie" },
                "spec": { "contextSpaceRef": "air", "audience": "public",
                          "slug": "aaaaaaaaaaaaaaaaaaaaaaaaaa" }
            })
        };
        let answer = |front: Front, dry_run: bool| {
            let (state, user) = (&state, &user);
            async move {
                propose_checked(
                    user,
                    front,
                    state,
                    "ovzdusie",
                    "endpoints",
                    None,
                    Operation::Create,
                    dry_run,
                    typed(),
                    None,
                    None,
                )
                .await
            }
        };
        for dry_run in [true, false] {
            match answer(Front::Bearer, dry_run).await {
                Err(ApiError::Unprocessable { errors, .. }) => {
                    let first = errors.first().map(String::as_str).unwrap_or_default();
                    assert!(first.starts_with("spec.slug: "), "{errors:?}");
                    assert!(!first.contains("aaaa"), "{errors:?}");
                }
                other => panic!(
                    "expected 422 at spec.slug, got {:?}",
                    other.map(|r| r.status())
                ),
            }
        }
        assert!(
            !matches!(
                answer(Front::Portal, true).await,
                Err(ApiError::Unprocessable { .. })
            ),
            "the Portal form brings the slug it minted"
        );
    }

    #[tokio::test]
    async fn propose_dry_run_create_returns_dry_run_result() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();
        let payload = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility",
                "namespace": "ovzdusie"
            },
            "spec": {
                "isSandbox": true
            }
        });

        let resp = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            payload,
        )
        .await
        .expect("dry run create should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: DryRunResult = serde_json::from_slice(&body_bytes).unwrap();
        assert!(result.valid);
        assert_eq!(result.lane, Lane::Green);
        assert_eq!(result.plan.summary.create, 1);
        assert_eq!(result.plan.summary.update, 0);
    }

    #[tokio::test]
    async fn propose_validation_rejects_wrong_apiversion_or_kind() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let bad_api = json!({
            "apiVersion": "v1",
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {}
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            bad_api,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("apiVersion")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        let bad_kind = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "Endpoint",
            "metadata": { "name": "mobility" },
            "spec": {}
        });
        let err2 = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            bad_kind,
        )
        .await
        .unwrap_err();
        match err2 {
            ApiError::BadRequest(msg) => assert!(msg.contains("kind 'Endpoint'")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn propose_validation_rejects_foreign_namespace_and_fills_absent() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let foreign_ns = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility",
                "namespace": "foreign-project"
            },
            "spec": {}
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            foreign_ns,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("foreign-project")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        let absent_ns = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility"
            },
            "spec": {}
        });
        let resp = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            absent_ns,
        )
        .await
        .expect("absent namespace should be filled in as project");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn propose_validation_rejects_status_and_literal_secrets() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let with_status = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {},
            "status": { "phase": "Live" }
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            with_status,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("status")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        let with_secret = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {
                "auth": { "password": "supersecretpassword" }
            }
        });
        let err2 = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            with_secret,
        )
        .await
        .unwrap_err();
        match err2 {
            ApiError::BadRequest(msg) => assert!(msg.contains("literal secret")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn propose_rejects_name_mismatch_on_replace() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let payload = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility-b" },
            "spec": {}
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            Some("mobility-a"),
            Operation::Update,
            true,
            payload,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("mobility-b")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn propose_non_dry_run_without_gitea_answers_503() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();
        let payload = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {}
        });

        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            false,
            payload,
        )
        .await
        .unwrap_err();

        match err {
            ApiError::Unavailable(msg) => assert!(msg.contains("git forge")),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn parse_patch_content_types() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json".parse().expect("header"),
        );
        let err = parse_patch_to_value(&headers, b"{}").unwrap_err();
        match err {
            ApiError::UnsupportedMediaType(msg) => {
                assert!(msg.contains("application/merge-patch+json"))
            }
            other => panic!("expected UnsupportedMediaType, got {other:?}"),
        }

        let mut headers_valid = HeaderMap::new();
        headers_valid.insert(
            header::CONTENT_TYPE,
            "application/merge-patch+json".parse().expect("header"),
        );
        let val = parse_patch_to_value(&headers_valid, b"{\"spec\":{\"audience\":\"public\"}}")
            .expect("parse valid json patch");
        assert_eq!(val["spec"]["audience"], "public");

        let mut headers_yaml = HeaderMap::new();
        headers_yaml.insert(
            header::CONTENT_TYPE,
            "application/apply-patch+yaml".parse().expect("header"),
        );
        let val_yaml = parse_patch_to_value(&headers_yaml, b"spec:\n  audience: public\n")
            .expect("parse valid yaml patch");
        assert_eq!(val_yaml["spec"]["audience"], "public");
    }

    #[test]
    fn a_space_scoped_resource_is_filed_under_the_space_it_references() {
        let endpoint = |spec: serde_json::Value| ResourceEnvelope {
            api_version: resource::API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: "citybikes-2046-all".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        let info = resource::by_kind("Endpoint").expect("Endpoint");
        for reference in [
            json!("citybikes-2046"),
            json!({ "kind": "ContextSpace", "name": "citybikes-2046" }),
        ] {
            assert_eq!(
                resolve_repo_path(
                    &endpoint(json!({ "contextSpaceRef": reference })),
                    info,
                    "helsinki"
                )
                .expect("a path"),
                "projects/helsinki/spaces/citybikes-2046/endpoints/citybikes-2046-all.yaml"
            );
        }
        assert_eq!(
            resolve_repo_path(&endpoint(json!({})), info, "helsinki").expect("a path"),
            "projects/helsinki/spaces/helsinki/endpoints/citybikes-2046-all.yaml"
        );
    }

    #[tokio::test]
    async fn patch_merges_with_current_and_plans_diff() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(ResourceEnvelope {
            api_version: resource::API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "mobility".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({ "isSandbox": false }),
            status: None,
        });

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/merge-patch+json".parse().expect("header"),
        );
        let patch_body = Bytes::from(r#"{"spec":{"isSandbox":true}}"#);

        let user = dummy_user();
        let resp = patch(
            user,
            Front::Portal,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery {
                workspace: None,
                dry_run: Some("All".into()),
                validate_only: None,
                confirm: None,
            }),
            headers,
            patch_body,
        )
        .await
        .expect("patch dry run should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: DryRunResult = serde_json::from_slice(&body_bytes).unwrap();
        assert!(result.valid);
        assert_eq!(result.lane, Lane::Green);
        assert_eq!(result.plan.summary.update, 1);
        assert_eq!(result.plan.fields.len(), 1);
        assert_eq!(result.plan.fields[0].path, "spec.isSandbox");
        assert_eq!(result.plan.fields[0].from, Some(json!(false)));
        assert_eq!(result.plan.fields[0].to, Some(json!(true)));
    }

    /// The space `ovzdusie` of project `ovzdusie` with its public Endpoint `public-air`, narrowed
    /// to the read Policy `public-read` by `policyRef`: the bbsk podujatia shape (T-5033).
    fn narrowed_space() -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(manifest(
            "Endpoint",
            "public-air",
            public_air(Some(
                "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie-ovzdusie:public-read",
            )),
        ));
        state.mirror.upsert(manifest(
            "Policy",
            "public-read",
            space_policy(
                "{ \"kind\": \"role\", \"id\": \"public\" }",
                &["retrieveOps"],
            ),
        ));
        state
    }

    fn manifest(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: resource::API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    fn public_air(policy_ref: Option<&str>) -> Value {
        let mut spec = json!({
            "contextSpaceRef": "ovzdusie", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
            "audience": "public", "enabledRepresentations": ["ngsi-ld"]
        });
        if let Some(reference) = policy_ref {
            spec["policyRef"] = json!(reference);
        }
        spec
    }

    fn space_policy(assignee: &str, operations: &[&str]) -> Value {
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "assigner": "did:web:banskabystrica.sk",
            "assignee": serde_json::from_str::<Value>(assignee).expect("an assignee"),
            "operations": operations,
        })
    }

    fn air_pipeline(endpoint: &str) -> Value {
        json!({
            "class": "scheduled", "schedule": "0 * * * *",
            "source": { "dataSourceRef": "feed" },
            "targetEndpoint": format!("urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:{endpoint}"),
            "output": { "type": "AirQualityObserved", "mode": "upsert" }
        })
    }

    fn refusal(result: Result<(), ApiError>) -> Vec<String> {
        match result {
            Err(ApiError::Unprocessable { errors, .. }) => errors,
            other => panic!("expected a 422, got {other:?}"),
        }
    }

    /// EP-117, T-6435: a Policy reference that would bind nothing is refused naming the URN to
    /// write: the bare space name (the T-6377 walk), another organization, a Policy the space
    /// lacks. The right URN, a Policy of the same Change, a pinned segment and a reference the
    /// Endpoint already holds pass.
    #[test]
    fn a_policy_ref_keeps_its_own_domain_when_no_organization_is_the_installations() {
        let state = AppState::new(Config::for_tests(), None);
        for (name, domain) in [("a-other", "barcelona.cat"), ("city", "hel.fi")] {
            state
                .mirror
                .upsert(organization(name, json!({ "domain": domain })));
        }
        state.mirror.upsert(manifest(
            "Policy",
            "public-read",
            space_policy(
                "{ \"kind\": \"role\", \"id\": \"public\" }",
                &["retrieveOps"],
            ),
        ));
        let check = |reference: &str| {
            let endpoint = manifest("Endpoint", "public-air", public_air(Some(reference)));
            check_policy_refs(&state, "ovzdusie", &endpoint)
        };

        check("urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:public-read")
            .expect("no domain is known to be wrong");
        let errors = refusal(check("urn:ngsi-ld:Policy:hel.fi:ovzdusie:public-read"));
        assert!(
            errors[0].contains("write urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:public-read"),
            "{errors:?}"
        );
    }

    /// An `Organization` in the namespace that holds them.
    fn organization(name: &str, spec: Value) -> ResourceEnvelope {
        let mut org = manifest("Organization", name, spec);
        org.metadata.namespace = Some(crate::api::blueprints::ORG_NAMESPACE.into());
        org
    }

    #[tokio::test]
    async fn a_policy_ref_that_binds_nothing_is_refused_naming_the_urn_to_write() {
        const RIGHT: &str = "urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:public-read";
        let state = AppState::new(Config::for_tests(), None);
        // The gateway binds on the installation Organization: one sorting first must not lead.
        state.mirror.upsert(organization(
            "a-other",
            json!({ "domain": "barcelona.cat" }),
        ));
        state.mirror.upsert(organization(
            "city",
            json!({ "domain": "hel.fi", "installation": true }),
        ));
        state.mirror.upsert(manifest(
            "Policy",
            "public-read",
            space_policy(
                "{ \"kind\": \"role\", \"id\": \"public\" }",
                &["retrieveOps"],
            ),
        ));
        let check = |reference: &str| {
            let endpoint = manifest("Endpoint", "public-air", public_air(Some(reference)));
            check_policy_refs(&state, "ovzdusie", &endpoint)
        };

        check(RIGHT).expect("the rendered segment binds");
        for wrong in [
            "urn:ngsi-ld:Policy:hel.fi:ovzdusie:public-read",
            "urn:ngsi-ld:Policy:other.example:ovzdusie-ovzdusie:public-read",
        ] {
            let errors = refusal(check(wrong));
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].starts_with("spec.policyRef: "), "{errors:?}");
            assert!(errors[0].contains(&format!("write {RIGHT}")), "{errors:?}");
        }
        let errors = refusal(check("urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:nobody"));
        assert!(
            errors[0].contains("urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:nobody, or name")
                && errors[0].contains("(public-read)"),
            "{errors:?}"
        );

        // `policyRefs` is judged entry by entry, and says which field to change.
        let listed = json!({ "contextSpaceRef": "ovzdusie", "policyRefs": [
            RIGHT, "urn:ngsi-ld:Policy:hel.fi:ovzdusie:public-read"
        ] });
        let mut spec = public_air(None);
        spec.as_object_mut()
            .expect("object")
            .extend(listed.as_object().expect("object").clone());
        let endpoint = manifest("Endpoint", "public-air", spec);
        let errors = refusal(check_policy_refs(&state, "ovzdusie", &endpoint));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("spec.policyRefs: "), "{errors:?}");

        // A Policy and a pinned space proposed in the same Change count.
        let mut space = manifest("ContextSpace", "ovzdusie", json!({ "urnSegment": "air" }));
        space.metadata.namespace = None;
        let sibling = manifest(
            "Policy",
            "kiosks",
            space_policy(
                "{ \"kind\": \"role\", \"id\": \"public\" }",
                &["retrieveOps"],
            ),
        );
        let drafts = vec![
            serde_json::to_value(&space).expect("space"),
            serde_json::to_value(&sibling).expect("policy"),
        ];
        crate::references::BUNDLE
            .scope(drafts, async {
                check("urn:ngsi-ld:Policy:hel.fi:air:kiosks").expect("the draft binds");
                let errors = refusal(check("urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:kiosks"));
                assert!(errors[0].contains("write urn:ngsi-ld:Policy:hel.fi:air:kiosks"));
            })
            .await;

        // A repository already holding one can still change the Endpoint's other fields.
        const OLD: &str = "urn:ngsi-ld:Policy:hel.fi:ovzdusie:public-read";
        state
            .mirror
            .upsert(manifest("Endpoint", "public-air", public_air(Some(OLD))));
        check(OLD).expect("a reference the Endpoint already holds");
    }

    /// T-6435: a write Policy and the Endpoint that fails to bind it, proposed in one Change, are
    /// a finding of that Change: both drafts used to count as already there, so it went unsaid.
    #[tokio::test]
    async fn a_write_nobody_can_make_inside_one_change_is_refused() {
        let state = narrowed_space();
        let writer = manifest(
            "Policy",
            "sensors-write",
            space_policy(
                "{ \"kind\": \"serviceAccount\", \"id\": \"sensors\" }",
                &["upsertBatch"],
            ),
        );
        let narrow = manifest(
            "Endpoint",
            "public-air",
            public_air(Some(
                "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie-ovzdusie:public-read",
            )),
        );
        let drafts = vec![
            serde_json::to_value(&writer).expect("policy"),
            serde_json::to_value(&narrow).expect("endpoint"),
        ];
        let result = crate::references::BUNDLE
            .scope(drafts, async {
                check_unmade_writes(&state, "ovzdusie", Operation::Create, &writer)
            })
            .await;
        assert!(refusal(result)[0].contains("sensors-write"));
    }

    /// DM-61, T-6394: a second own model of a space is refused naming the space's model and the
    /// two ways out; its first model, a re-proposal of the space's own model, a model of another
    /// space and a mirrored peer copy pass.
    #[test]
    fn a_second_model_of_a_space_is_refused_naming_the_spaces_model() {
        let state = AppState::new(Config::for_tests(), None);
        let model = |space: &str, lifecycle: &str| {
            json!({ "contextSpaceRef": space, "linkml": "./m.linkml.yaml", "version": "0.1.0",
                    "lifecycle": lifecycle })
        };
        let check = |name: &str, spec: Value| check_one_model(&state, "ovzdusie", name, &spec);
        check("walk", model("walk", "draft")).expect("the space's first model");
        state
            .mirror
            .upsert(manifest("DataModel", "walk", model("walk", "draft")));
        check("walk", model("walk", "published")).expect("the space's own model again");
        check("other", model("other-space", "draft")).expect("a model of another space");
        check("peer", model("walk", "mirrored")).expect("a peer's read-only copy");
        let errors = refusal(check("bikes", model("walk", "draft")));
        assert_eq!(errors.len(), 1, "{errors:?}");
        let line = &errors[0];
        assert!(line.starts_with("spec.contextSpaceRef: "), "{line}");
        for part in [
            "second model of space `walk`",
            "beside `walk`",
            "DM-61",
            "Add the classes to DataModel `walk`",
        ] {
            assert!(line.contains(part), "{part}: {line}");
        }
        // A repository holding two models of one space from before DM-61 changes either.
        state
            .mirror
            .upsert(manifest("DataModel", "reports", model("walk", "published")));
        check("reports", model("walk", "published")).expect("a model the space already counts");
        refusal(check("third", model("walk", "draft")));
    }

    /// PL-111, ADR-N-057 decision 5, T-5040: a write Policy no Endpoint binds and a Pipeline aimed
    /// at an Endpoint that does not grant its write are refused with jcctl validate's sentence and
    /// the field to change; the fixed proposals pass.
    #[test]
    fn a_write_nobody_can_make_is_refused_naming_what_to_change() {
        let state = narrowed_space();
        let check = |operation, envelope: ResourceEnvelope| {
            check_unmade_writes(&state, "ovzdusie", operation, &envelope)
        };
        let writer = || {
            manifest(
                "Policy",
                "sensors-write",
                space_policy(
                    "{ \"kind\": \"serviceAccount\", \"id\": \"sensors\" }",
                    &["upsertBatch"],
                ),
            )
        };

        let errors = refusal(check(Operation::Create, writer()));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].starts_with("Policy sensors-write grants a write")
                && errors[0].contains("public-air binds only its policyRef")
                && errors[0].contains("add an Endpoint of the space without one"),
            "{errors:?}"
        );

        let errors = refusal(check(
            Operation::Create,
            manifest("Pipeline", "stations", air_pipeline("public-air")),
        ));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].starts_with(
                "spec.targetEndpoint: Pipeline stations output 1 writes as pl-stations"
            ) && errors[0].contains("aim it at an Endpoint of the space without policyRef"),
            "{errors:?}"
        );
        let errors = refusal(check(
            Operation::Create,
            manifest("Pipeline", "lost", air_pipeline("no-such-endpoint")),
        ));
        assert!(
            errors[0].contains("declares no Endpoint no-such-endpoint"),
            "{errors:?}"
        );

        // Fixed: an Endpoint of the space without policyRef binds both.
        let mut air_write = public_air(None);
        air_write["slug"] = json!("bq3kz7xw2mdv6ksb3ncf5arw2y");
        state
            .mirror
            .upsert(manifest("Endpoint", "air-write", air_write.clone()));
        assert!(check(Operation::Create, writer()).is_ok());
        assert!(check(
            Operation::Create,
            manifest("Pipeline", "stations", air_pipeline("air-write"))
        )
        .is_ok());

        // An Endpoint change that takes the only binding away is refused at its own field.
        state.mirror.upsert(writer());
        let errors = refusal(check(
            Operation::Update,
            manifest(
                "Endpoint",
                "air-write",
                public_air(Some(
                    "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie-ovzdusie:public-read",
                )),
            ),
        ));
        assert!(
            errors[0].starts_with("spec.policyRef: Policy sensors-write grants a write"),
            "{errors:?}"
        );
        let errors = refusal(check(
            Operation::Delete,
            manifest("Endpoint", "air-write", air_write),
        ));
        assert!(errors[0].starts_with("Policy sensors-write"), "{errors:?}");
    }

    /// T-5040: a finding the project already holds does not stop an unrelated change, so a broken
    /// project can be repaired one manifest at a time; a read Policy is no write.
    #[test]
    fn a_write_finding_already_in_the_project_does_not_block_another_change() {
        let state = narrowed_space();
        state.mirror.upsert(manifest(
            "Policy",
            "sensors-write",
            space_policy(
                "{ \"kind\": \"serviceAccount\", \"id\": \"sensors\" }",
                &["upsertBatch"],
            ),
        ));
        let check = |envelope: ResourceEnvelope| {
            check_unmade_writes(&state, "ovzdusie", Operation::Create, &envelope)
        };
        assert!(check(manifest(
            "Policy",
            "more-readers",
            space_policy(
                "{ \"kind\": \"role\", \"id\": \"public\" }",
                &["retrieveOps"]
            ),
        ))
        .is_ok());
        assert!(check(manifest("DataSource", "feed", json!({}))).is_ok());
    }

    /// T-5040: the dry run a person sees answers a red verdict whose finding names the field
    /// (`spec.outputs.0.targetEndpoint`) and says what to change; the form marks that field.
    #[tokio::test]
    async fn a_dry_run_of_a_pipeline_aimed_at_a_policy_ref_endpoint_is_red() {
        let state = narrowed_space();
        let payload =
            serde_json::to_value(manifest("Pipeline", "stations", air_pipeline("public-air")))
                .expect("a manifest");
        let result = propose(
            &dummy_user(),
            &state,
            "ovzdusie",
            "pipelines",
            None,
            Operation::Create,
            true,
            payload,
        )
        .await;
        let Err(err) = result else {
            panic!("the dry run refuses the pipeline");
        };
        let result = crate::api::dry_run::refused_check(&err, &serde_json::Value::Null)
            .expect("a refusal of the manifest is a red verdict");
        assert!(!result.valid);
        let verdict = result.verdict.expect("a verdict");
        assert!(!verdict.ok);
        let errors: Vec<String> = verdict
            .findings
            .iter()
            .map(|f| format!("{}|{}", f.path, f.message))
            .collect();
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("spec.outputs.0.targetEndpoint|Pipeline stations output 1")),
            "{errors:?}"
        );
    }

    /// PL-79, T-4409: a replace beside another writer of the same ids in its space is refused
    /// naming both, from either side; a replace alone, or into another space, is accepted.
    #[test]
    fn a_replace_beside_another_writer_of_its_type_is_refused_naming_both() {
        let state = AppState::new(Config::for_tests(), None);
        let envelope = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
            api_version: resource::API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        let pipeline = |endpoint: &str, mode: &str| {
            json!({
                "class": "scheduled", "schedule": "0 * * * *",
                "source": { "dataSourceRef": "feed" },
                "targetEndpoint": format!("urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:{endpoint}"),
                "output": { "type": "AirQualityObserved", "mode": mode, "id": {
                    "mint": "template", "template": "urn:ngsi-ld:AirQualityObserved:{station.code}"
                } }
            })
        };
        for (endpoint, space) in [("public-air", "ovzdusie"), ("verejne-air", "verejne")] {
            state.mirror.upsert(envelope(
                "Endpoint",
                endpoint,
                json!({
                    "contextSpaceRef": space, "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
                    "audience": "public", "enabledRepresentations": ["ngsi-ld"]
                }),
            ));
        }
        let check = |name: &str, spec: Value| check_replace(&state, "ovzdusie", name, &spec);

        assert!(
            check("stations", pipeline("public-air", "replace")).is_ok(),
            "alone"
        );
        state.mirror.upsert(envelope(
            "Pipeline",
            "enrich",
            pipeline("public-air", "upsert"),
        ));
        let Err(ApiError::BadRequest(why)) = check("stations", pipeline("public-air", "replace"))
        else {
            panic!("a replace beside an upsert writer is refused");
        };
        assert!(
            why.contains("Pipeline stations")
                && why.contains("Pipeline enrich")
                && why.contains("PL-79"),
            "{why}"
        );
        assert!(
            check("stations", pipeline("verejne-air", "replace")).is_ok(),
            "another space"
        );
        assert!(
            check("enrich", pipeline("public-air", "upsert")).is_ok(),
            "its own old self is no other"
        );

        state.mirror.upsert(envelope(
            "Pipeline",
            "stations",
            pipeline("public-air", "replace"),
        ));
        assert!(
            matches!(
                check("enrich", pipeline("public-air", "upsert")),
                Err(ApiError::BadRequest(_))
            ),
            "an upsert beside an existing replace writer is refused"
        );
    }
}

#[cfg(test)]
mod second_shape_tests {
    use super::pipeline_second_shape;
    use serde_json::json;

    fn first() -> serde_json::Value {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "p", "namespace": "helsinki" },
            "spec": {
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "gbfs" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:ops",
                "output": { "type": "BikeHireDockingStation", "mode": "upsert" },
                "secretRefs": [{ "name": "s", "key": "k", "envVar": "V" }]
            }
        })
    }

    #[test]
    fn a_first_shape_pipeline_is_written_as_the_second() {
        let mut body = first();
        pipeline_second_shape(&mut body);
        assert_eq!(body["apiVersion"], "joinedcontext.com/v1alpha2");
        let spec = &body["spec"];
        assert_eq!(spec["sources"][0]["dataSourceRef"]["name"], "gbfs");
        assert_eq!(spec["steps"][0]["bloblang"], "root = this");
        assert_eq!(
            spec["outputs"][0]["targetEndpoint"],
            "urn:ngsi-ld:Endpoint:hel.fi:helsinki:ops"
        );
        assert_eq!(spec["outputs"][0]["type"], "BikeHireDockingStation");
        assert_eq!(spec["outputs"][0]["mode"], "upsert");
        for gone in ["source", "compute", "targetEndpoint", "output"] {
            assert!(spec.get(gone).is_none(), "{gone} is left behind");
        }
        assert_eq!(
            spec["secretRefs"][0]["envVar"], "V",
            "what is not moved stays"
        );
        // The result is a Pipeline jc-core accepts at v1alpha2.
        let text = serde_json::to_string(&body).expect("json");
        jc_core::registry::validate_yaml("Pipeline", &text)
            .expect("a catalogued kind")
            .expect("valid at v1alpha2");
    }

    #[test]
    fn a_pipeline_without_compute_or_output_gets_no_steps_and_a_bare_output() {
        let mut body = first();
        let spec = body["spec"].as_object_mut().expect("spec");
        spec.remove("compute");
        spec.remove("output");
        pipeline_second_shape(&mut body);
        assert!(body["spec"].get("steps").is_none());
        assert_eq!(
            body["spec"]["outputs"],
            json!([{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:ops" }])
        );
    }

    #[test]
    fn what_is_not_a_first_shape_pipeline_is_left_as_it_came() {
        // Its input lives in bento.yaml: v1alpha2 has no place for it.
        let mut no_source = first();
        no_source["spec"]
            .as_object_mut()
            .expect("spec")
            .remove("source");
        let before = no_source.clone();
        pipeline_second_shape(&mut no_source);
        assert_eq!(no_source, before);

        // Already the second shape, or mixed: validation judges it, not a rewrite.
        let mut mixed = first();
        mixed["spec"]["outputs"] = json!([]);
        let before = mixed.clone();
        pipeline_second_shape(&mut mixed);
        assert_eq!(mixed, before);

        let mut other = json!({ "kind": "Endpoint", "apiVersion": "joinedcontext.com/v1alpha1", "spec": { "source": 1, "targetEndpoint": 2 } });
        let before = other.clone();
        pipeline_second_shape(&mut other);
        assert_eq!(other, before);

        let mut no_spec = json!({ "kind": "Pipeline" });
        pipeline_second_shape(&mut no_spec);
        assert_eq!(no_spec, json!({ "kind": "Pipeline" }));
    }

    #[test]
    fn the_targets_of_either_shape_are_read() {
        let first = first();
        assert_eq!(
            crate::resource::pipeline_targets(&first["spec"]),
            vec!["urn:ngsi-ld:Endpoint:hel.fi:helsinki:ops"]
        );
        let second = json!({ "outputs": [
            { "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:a" },
            { "mode": "upsert" },
            { "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:b" }
        ] });
        assert_eq!(crate::resource::pipeline_targets(&second).len(), 2);
        assert!(crate::resource::pipeline_targets(&json!({})).is_empty());
    }
}
