//! Change proposals and approvals API (CC-34, CC-41, CC-63, UI-23..UI-25).
//!
//! Provides the Portal's view of open merge requests so reviewers and domain
//! stewards never need to switch to the forge to review and approve proposals.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{GitError, GiteaClient, MergeStyle, PullRequest, ReviewEvent};
use crate::plan::{self, FieldChange, PlanDiff};
use crate::resource::ResourceEnvelope;
use crate::state::AppState;

/// Human-readable proposal summary parameters derived from the plan diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub key: String,
    #[schema(value_type = Object)]
    pub params: serde_json::Map<String, Value>,
}

/// Human author attributed on the change proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeAuthor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// A change proposal representing an open Git merge request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeProposal {
    pub api_version: String,
    pub kind: String,
    pub metadata: ChangeMeta,
    pub status: ChangeStatus,
    pub summary: ChangeSummary,
    pub author: ChangeAuthor,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_fields: Option<Vec<FieldChange>>,
    /// Every file the merge request changes, on the detail of one change (T-0861). Absent in
    /// a listing, which carries `fileCount` instead: the list would otherwise read every
    /// file of every open change to render a number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ChangeFile>>,
    /// How many files the merge request changes, the headline manifest included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<usize>,
    /// The workspace this Change brings back, so the approver reads that it was worked on as a
    /// copy first, and whose (UI-63, CC-79).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// The changes this one waits on, each with where it stands (MF-48): it names what they
    /// create, so it merges after them, and a `Rejected` one flags it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waits_on: Vec<AwaitedChange>,
    /// Who merged or rejected a closed change, when and why, as the Portal recorded it on the
    /// forge (T-3292); absent on an open change and on one closed in the forge itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ChangeDecision>,
    /// The repository's CI on the head commit (CC-90): `success`, `pending`, `failure` or
    /// `error`; on the detail of one change, and absent while no check has reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<String>,
    /// The context space of the manifest the change proposes, as its head (else its base)
    /// names it: what a reader bound to one space is judged by (PF-59, T-6226). Server-side
    /// only, never on the wire.
    #[serde(skip)]
    pub space: Option<String>,
}

/// Who decided a closed change: the approver of a merge, the rejecter of a closed one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeDecision {
    pub by: String,
    /// The connector an autonomous approval came through (T-6552), e.g. `doprava-in-claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// RFC 3339, when the forge merged or closed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The rejecter's reason, when they gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A change another one waits on (MF-48).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AwaitedChange {
    pub name: String,
    pub phase: ChangePhase,
}

/// The changes the merge request `pr` waits on, read from its body and asked of the forge
/// (MF-48). One the forge no longer knows is `Rejected`: what it would have created never will be.
async fn awaited_changes(
    state: &AppState,
    gitea: &GiteaClient,
    pr: &PullRequest,
    project: &str,
) -> Result<Vec<AwaitedChange>, ApiError> {
    let mut awaited = Vec::new();
    for number in crate::references::waited_on(&pr.body) {
        let phase = match gitea.pull_request(number).await {
            Ok(other) if other.merged => ChangePhase::Merged,
            Ok(other) if other.state == "closed" => ChangePhase::Rejected,
            Ok(_) => ChangePhase::PendingApproval,
            Err(GitError::NotFound) => ChangePhase::Rejected,
            Err(err) => return Err(err.into()),
        };
        awaited.push(AwaitedChange {
            name: change_meta(state, gitea, number, project).name,
            phase,
        });
    }
    Ok(awaited)
}

/// Refuses the approval of a change that waits on one not merged yet (MF-48): it names what that
/// change creates, and merging first would commit a reference to nothing.
fn merges_after(id: &str, awaited: &[AwaitedChange]) -> Result<(), ApiError> {
    match awaited
        .iter()
        .find(|other| other.phase != ChangePhase::Merged)
    {
        None => Ok(()),
        Some(other) if other.phase == ChangePhase::Rejected => Err(ApiError::Conflict(format!(
            "change proposal '{id}' names what {} would have created, and {} was rejected; \
             reject this change, or propose it again naming a resource that is there (MF-48)",
            other.name, other.name
        ))),
        Some(other) => Err(ApiError::Conflict(format!(
            "change proposal '{id}' names what {} creates; approve {} first, then this one (MF-48)",
            other.name, other.name
        ))),
    }
}

impl ChangeProposal {
    pub const KIND: &'static str = "Change";
}

/// One file of the merge request behind a change (T-0861, MF-21, CC-63).
///
/// A bundle merge request carries more than its headline: the approval walks every file and
/// the strictest lane among them decides the confirmation (T-0832), so the page has to show
/// the same list. What the approver reads is what the server checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeFile {
    /// Path in the configuration repository.
    pub path: String,
    /// The manifest's kind, or the kind the directory names for a native file beside one.
    pub kind: String,
    /// What the merge request does to it, from the forge's own diff status.
    pub operation: Operation,
    /// The lane this file alone would take.
    pub lane: Lane,
    /// This manifest's own field-level diff, base against head and redacted (T-1397); absent for
    /// a native file, which carries no manifest to diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldChange>>,
}

/// Collection envelope for change proposals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeList {
    pub api_version: String,
    pub kind: String,
    pub items: Vec<ChangeProposal>,
    /// The `page` that continues a history listing, while the forge has older ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
    /// The open merge requests of the project repository that are not Changes, a fork's or a
    /// non-writer's, and why; listed to an organization administrator alone (T-3433).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outside: Vec<OutsideMergeRequest>,
}

/// An open merge request in the forge that the Portal does not offer for approval (T-3433).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutsideMergeRequest {
    pub number: u64,
    /// Its page in the forge, where it is closed or its author is answered.
    pub url: String,
    /// The forge account that opened it.
    pub author: String,
    /// Why it is not a Change, in words.
    pub reason: String,
}

impl ChangeList {
    pub const KIND: &'static str = "ChangeList";

    pub fn new(items: Vec<ChangeProposal>) -> Self {
        Self {
            api_version: crate::resource::API_VERSION.to_string(),
            kind: Self::KIND.to_string(),
            items,
            next: None,
            outside: Vec::new(),
        }
    }
}

/// Which closed changes `GET …/changes/history` lists (T-3292).
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct HistoryQuery {
    /// The page of closed changes, from 1; `next` of the previous answer.
    pub page: Option<u32>,
    /// Closed changes to one kind only, as `Endpoint`.
    pub kind: Option<String>,
    /// Closed changes whose resource name contains this, any case.
    pub name: Option<String>,
    /// Closed changes whose author's name or e-mail contains this, any case (T-3274).
    pub author: Option<String>,
    /// Closed changes decided on or after this day, `YYYY-MM-DD` (T-3274).
    pub since: Option<String>,
    /// Closed changes decided on or before this day, `YYYY-MM-DD` (T-3274).
    pub until: Option<String>,
}

/// A `YYYY-MM-DD` day of a history filter, or the `400` that names the field.
fn history_day(field: &str, value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    time::Date::parse(
        value,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| ApiError::BadRequest(format!("{field} is a day, YYYY-MM-DD, not '{value}'")))?;
    Ok(Some(value.to_owned()))
}

/// Closed merge requests the forge is asked for per history page; each costs up to three reads.
// ponytail: three forge reads per closed change on a page; keep a decided-change table if the
// history page gets slow.
const HISTORY_PAGE: u32 = 20;

/// Request body for approving or rejecting a change proposal.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApproveBody {
    #[serde(default)]
    pub confirm: Option<String>,
    /// Why a change is rejected; written on the merge request beside who rejected it.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Whether the field at `path` holds a credential by its name (CC-06).
///
/// The one list, [`crate::api::mutate::SECRET_KEYS`], that a write is refused by (MF-24) and an
/// export is stripped by (MF-17). It used to be a shorter copy of that list, and every name the
/// copy lacked — `privateKey`, `accessToken`, `apiToken`, `passphrase` and the rest — travelled
/// in cleartext through the plan diff of a Change to everyone who may read it (T-1685).
pub fn is_sensitive_path(path: &str) -> bool {
    let last_seg = path.rsplit('.').next().unwrap_or(path);
    let clean = last_seg.split('[').next().unwrap_or(last_seg);
    crate::api::mutate::SECRET_KEYS.contains(&clean)
}

/// Replaces sensitive field values with `"[REDACTED]"` (CC-06).
pub fn redact(fields: Vec<FieldChange>) -> Vec<FieldChange> {
    fields
        .into_iter()
        .map(|mut f| {
            if is_sensitive_path(&f.path) {
                if f.from.is_some() {
                    f.from = Some(Value::String("[REDACTED]".to_string()));
                }
                if f.to.is_some() {
                    f.to = Some(Value::String("[REDACTED]".to_string()));
                }
            }
            f
        })
        .collect()
}

/// Validates and parses `{id}` formatted as `chg-` plus eight lowercase hex digits.
pub fn parse_change_id(id: &str) -> Result<u64, ApiError> {
    match parse_change_ref(id)? {
        (false, number) => Ok(number),
        (true, _) => Err(invalid_change_id(id)),
    }
}

fn invalid_change_id(id: &str) -> ApiError {
    ApiError::BadRequest(format!(
        "invalid change id '{id}'; expected 'chg-' or 'chg-org-' followed by 8 lowercase hex digits"
    ))
}

/// A Change id as `(organization, number)`: `chg-org-{hex}` is a merge request of the
/// organization repository, `chg-{hex}` one of the repository the project's own kinds live in
/// (CC-87).
pub fn parse_change_ref(id: &str) -> Result<(bool, u64), ApiError> {
    let invalid = || invalid_change_id(id);
    let (organization, hex) = match id.strip_prefix("chg-org-") {
        Some(hex) => (true, hex),
        None => (false, id.strip_prefix("chg-").ok_or_else(invalid)?),
    };
    if hex.len() != 8
        || !hex
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return Err(invalid());
    }
    u64::from_str_radix(hex, 16)
        .map(|number| (organization, number))
        .map_err(|_| invalid())
}

/// The forge a Change id is read, approved and rejected in, and its merge request number: the
/// organization repository for `chg-org-`, the project's own otherwise (CC-87).
pub fn resolve_change(
    state: &AppState,
    project: &str,
    id: &str,
) -> Result<(std::sync::Arc<GiteaClient>, u64), ApiError> {
    let (organization, number) = parse_change_ref(id)?;
    let forge = if organization {
        state.gitea.clone()
    } else {
        state.forge_for(project)
    };
    let forge = forge.ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    Ok((forge, number))
}

/// The id merge request `number` of `forge` is known by under `project`: `chg-org-` for the
/// organization repository in layout 2, where a project repository numbers its own apart;
/// `chg-` otherwise (CC-87).
pub fn change_meta(
    state: &AppState,
    forge: &GiteaClient,
    number: u64,
    project: &str,
) -> ChangeMeta {
    if state.mirror.layout() == 2 && !forge.is_project_repository() {
        ChangeMeta::from_organization_merge_request(number, project)
    } else {
        ChangeMeta::from_merge_request(number, project)
    }
}

/// Parsed components from a `portal/{op}-{kind}-{name}-{hash}` branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub operation: Operation,
    pub kind_lower: String,
    pub resource_name: String,
    pub hash: String,
}

pub fn parse_branch_name(branch: &str) -> Option<BranchInfo> {
    let clean = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    let rest = clean.strip_prefix("portal/")?;
    // A bundle names no one resource: a blueprint flow, a headless import, a project deletion.
    // `delete-project-helsinki-mobility` would otherwise read as the Project `helsinki` of hash
    // `mobility`; `load_manifest_data` heads a bundle by its files instead (T-1400).
    if BUNDLE_PREFIXES
        .iter()
        .any(|prefix| rest.starts_with(prefix))
    {
        return None;
    }
    // A retry after a rejection carries `_{nonce}` after the hash (T-0887); the name is the same.
    let rest = match rest.rsplit_once('_') {
        Some((base, nonce)) if nonce.len() == 8 && nonce.bytes().all(|b| b.is_ascii_hexdigit()) => {
            base
        }
        _ => rest,
    };
    let mut parts: Vec<&str> = rest.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let op = match parts[0] {
        "create" => Operation::Create,
        "update" => Operation::Update,
        "delete" => Operation::Delete,
        _ => return None,
    };
    let kind_lower = parts[1].to_string();
    let hash = parts.pop()?.to_string();
    let resource_name = parts[2..].join("-");
    if resource_name.is_empty() {
        return None;
    }
    Some(BranchInfo {
        operation: op,
        kind_lower,
        resource_name,
        hash,
    })
}

/// A caller with no `approve` grant in the project is refused before the forge is asked
/// anything: the cheap answer first, the kind-precise one once the change is loaded (PF-50).
pub(crate) fn may_approve_anything(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if effective.bootstrap
        || effective
            .grants
            .iter()
            .any(|grant| grant.rule.verbs.contains(&jc_core::kinds::Verb::Approve))
    {
        return Ok(());
    }
    Err(ApiError::Denied(format!(
        "no role grants approve in project {project} (PF-50)"
    )))
}

/// Approval and rejection need `approve` on the change's kind in a binding that covers the
/// project (T-0526, PF-50); the head manifest is what the constraints are checked against,
/// the base one for a deletion.
fn may_approve(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    data: &ManifestData,
) -> Result<(), ApiError> {
    let target = data
        .head_envelope
        .as_ref()
        .or(data.base_envelope.as_ref())
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    crate::permissions::for_request(state, identity, project).check(
        gate_kind(&data.kind),
        jc_core::kinds::Verb::Approve,
        target.as_ref(),
    )
}

pub(crate) struct ManifestData {
    pub kind: String,
    pub name: String,
    pub operation: Operation,
    pub base_envelope: Option<ResourceEnvelope>,
    pub head_envelope: Option<ResourceEnvelope>,
}

async fn find_manifest_in_tree(
    gitea: &GiteaClient,
    git_ref: &str,
    project: &str,
    resource_name: &str,
) -> Result<Option<String>, ApiError> {
    let tree = match gitea.list_tree(git_ref).await {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    Ok(tree
        .into_iter()
        .find(|p| names_manifest(p, project, resource_name)))
}

/// Whether `path` is the manifest of the resource `name` in `project`: a `{name}.yaml` under the
/// project, or the folder file of a kind that lives in one (`{name}/space.yaml`, ...).
fn names_manifest(path: &str, project: &str, name: &str) -> bool {
    let Some(rest) = path.strip_prefix(&format!("projects/{project}/")) else {
        return false;
    };
    let rest = format!("/{rest}");
    [
        ".yaml",
        ".yml",
        "/space.yaml",
        "/pipeline.yaml",
        "/app.yaml",
    ]
    .iter()
    .any(|suffix| rest.ends_with(&format!("/{name}{suffix}")))
}

/// The manifest among the merge request's own files, deleted ones for a deletion: one list
/// call the listing pays anyway, where the tree walk read every file of the repository once per
/// open change (T-6347).
fn manifest_in_files(
    files: Option<&[crate::git::gitea::ChangedFile]>,
    project: &str,
    name: &str,
    deleted: bool,
) -> Option<String> {
    files?
        .iter()
        .find(|file| file.deleted == deleted && names_manifest(&file.path, project, name))
        .map(|file| file.path.clone())
}

/// The project a file of the configuration repository belongs to: the segment after `projects/`.
///
/// `None` for a file of the organization itself — `users/`, `environments/`, `blueprints/`,
/// `portal/forms/`, `org.yaml` — which every project's changes may legitimately carry and whose
/// own rules (`within_own_rights`, the organization-scoped grant) decide who may write it.
fn project_of_path(path: &str) -> Option<&str> {
    path.strip_prefix("projects/")?
        .split('/')
        .next()
        .filter(|segment| !segment.is_empty())
}

/// Whether a pull request on `branch` is a Change of this Portal: a proposal of its own, or a
/// workspace brought back (CC-79).
fn is_change_branch(branch: &str) -> bool {
    let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    branch.starts_with("portal/") || branch.starts_with("workspace/")
}

/// What an open or closed pull request is to this Portal (T-3433).
enum Standing {
    /// A Change: a branch of the Portal's own, or a writer's branch of the project repository.
    Change,
    /// A merge request of the project repository that is not one, and why.
    Outside(String),
    /// Anything else of the organization repository, which no list mentions.
    Unlisted,
}

/// Whether `pr` is a Change. A Portal branch is (CC-79); so is, in a project repository of
/// layout 2, a branch of that repository itself whose forge author is in `{slug}-writers` by the
/// project's bindings in force and which merges into its default branch (ADR-N-029, PF-87). The
/// author is the forge account that opened it, never a commit's own claim, and a forge that says
/// neither repository nor e-mail lets nothing through.
async fn standing(
    state: &AppState,
    gitea: &GiteaClient,
    project: &str,
    pr: &PullRequest,
) -> Result<Standing, ApiError> {
    if is_change_branch(&pr.head_branch) {
        return Ok(Standing::Change);
    }
    if !gitea.is_project_repository() {
        return Ok(Standing::Unlisted);
    }
    if !pr.same_repository {
        return Ok(Standing::Outside(
            "it comes from a fork; push the branch to the project repository itself".into(),
        ));
    }
    let writer = pr.author_email.as_deref().is_some_and(|email| {
        crate::permissions::project_members(&state.mirror, project, chrono::Utc::now())
            .writers
            .contains(&email.trim().to_ascii_lowercase())
    });
    if !writer {
        return Ok(Standing::Outside(format!(
            "its author is not a writer of the project '{project}'; a binding granting propose \
             on the project makes them one"
        )));
    }
    let main = gitea.default_branch().await?;
    let base = pr
        .base_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(&pr.base_branch);
    if base != main {
        return Ok(Standing::Outside(format!(
            "it merges into '{base}', not into '{main}', which the platform deploys"
        )));
    }
    Ok(Standing::Change)
}

/// The `404` of a pull request that is not a Change, said like one that is not there (R20).
async fn require_change(
    state: &AppState,
    gitea: &GiteaClient,
    project: &str,
    id: &str,
    pr: &PullRequest,
) -> Result<(), ApiError> {
    match standing(state, gitea, project, pr).await? {
        Standing::Change => Ok(()),
        Standing::Outside(_) | Standing::Unlisted => Err(ApiError::NotFound(format!(
            "change proposal '{id}' not found in project '{project}'"
        ))),
    }
}

/// The workspace a Change brings back, when its branch is one (UI-63).
fn workspace_of(branch: &str) -> Option<String> {
    let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    branch.strip_prefix("workspace/").map(str::to_owned)
}

/// The branch prefixes of a change that carries several resources and names none of them:
/// `blueprints::flow_branch`, `import`'s headless bundle, `projects`' deletion.
// `rename-app-shapes-`: every old-shape App of a project in one Change (AP-124, T-2940).
// `restore-`: what a merged removal took, proposed again (T-3247).
// `adopt-`: a seed entity rewritten to what its space holds now (UI-26, F-POR4-004).
const BUNDLE_PREFIXES: [&str; 7] = [
    "flow-",
    "import-",
    "delete-project-",
    "remove-person-",
    "rename-app-shapes-",
    "restore-",
    "adopt-",
];

/// A seed entity file (`projects/{p}/spaces/{s}/entities/seed/{name}.json`) as an `Entity`
/// envelope, so a change that carries only seed files has a headline the list and the plan read:
/// its name is the file's, its spec the file's JSON. `None` for any other path or for text that
/// is no JSON.
pub(crate) fn seed_envelope(path: &str, content: &str) -> Option<ResourceEnvelope> {
    let mut segments = path.split('/');
    let (Some("projects"), Some(project), Some("spaces"), Some(space)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    let (Some("entities"), Some("seed"), Some(file), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    let name = file.strip_suffix(".json").filter(|name| !name.is_empty())?;
    let spec: Value = serde_json::from_str(content).ok()?;
    Some(ResourceEnvelope {
        api_version: crate::resource::API_VERSION.to_owned(),
        kind: "Entity".to_owned(),
        metadata: crate::resource::ObjectMeta {
            name: name.to_owned(),
            namespace: Some(project.to_owned()),
            labels: [("joinedcontext.com/space".to_owned(), space.to_owned())].into(),
            ..Default::default()
        },
        spec,
        status: None,
    })
}

/// The headline of a bundle: the first manifest among its files that lives in `project`, the
/// `Project` manifest first when the bundle deletes one. Every file is still approved on its own
/// (`approve_every_file`); the headline is what the list and the detail page show, and the kind
/// the first, cheap approval check reads (T-1400).
/// Which file of a bundle heads its Change, lowest first (stable, so the forge's order breaks a
/// tie): the Project manifest heads a project's deletion, and a Context Space heads the files it
/// brings, so a new space is headed "Create ContextSpace", never by its DataModel (T-6390).
fn headline_rank(path: &str) -> u8 {
    if path.ends_with("/project.yaml") {
        0
    } else if path.ends_with("/space.yaml") {
        1
    } else {
        2
    }
}

async fn bundle_headline(
    gitea: &GiteaClient,
    pr: &PullRequest,
    project: &str,
    files: Option<&[crate::git::gitea::ChangedFile]>,
) -> Result<Option<ManifestData>, ApiError> {
    let branch = pr
        .head_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(&pr.head_branch);
    // A workspace brought back is a bundle too: several resources under one Change (CC-79); so
    // is a writer's own branch, which names nothing (T-3433). `standing` decided it is a Change.
    let bundle = match branch.strip_prefix("portal/") {
        Some(rest) => BUNDLE_PREFIXES
            .iter()
            .any(|prefix| rest.starts_with(prefix)),
        None => true,
    };
    if !bundle {
        return Ok(None);
    }
    // The organization's own manifests are under `users/`; a person's removal is one (PF-93).
    let home = if project == crate::permissions::ORG_NAMESPACE {
        "users/".to_owned()
    } else {
        format!("projects/{project}/")
    };
    let fetched;
    let files = match files {
        Some(files) => files,
        None => {
            fetched = gitea.pull_request_files(pr.number).await?;
            &fetched
        }
    };
    let mut files: Vec<_> = files
        .iter()
        .filter(|file| file.path.starts_with(&home))
        .collect();
    files.sort_by_key(|file| headline_rank(&file.path));
    for file in &files {
        let git_ref = if file.deleted {
            pr.base_ref()
        } else {
            pr.head_ref()
        };
        let Some(content) = gitea.get_file(&file.path, git_ref).await? else {
            continue;
        };
        let Ok(envelope) = serde_norway::from_str::<ResourceEnvelope>(&content.content) else {
            continue;
        };
        let (operation, base_envelope, head_envelope) = if file.deleted {
            (Operation::Delete, Some(envelope.clone()), None)
        } else if file.added {
            (Operation::Create, None, Some(envelope.clone()))
        } else {
            let base = gitea
                .get_file(&file.path, pr.base_ref())
                .await?
                .and_then(|base| serde_norway::from_str::<ResourceEnvelope>(&base.content).ok());
            (Operation::Update, base, Some(envelope.clone()))
        };
        return Ok(Some(ManifestData {
            kind: envelope.kind,
            name: envelope.metadata.name,
            operation,
            base_envelope,
            head_envelope,
        }));
    }
    // No manifest: a change of seed entities alone (an adopted drift) is headed by its first
    // seed file, read on both sides.
    for file in files.iter().filter(|file| !file.deleted) {
        let Some(head) = gitea.get_file(&file.path, pr.head_ref()).await? else {
            continue;
        };
        let Some(head) = seed_envelope(&file.path, &head.content) else {
            continue;
        };
        let base = gitea
            .get_file(&file.path, pr.base_ref())
            .await?
            .and_then(|base| seed_envelope(&file.path, &base.content));
        let operation = if base.is_some() {
            Operation::Update
        } else {
            Operation::Create
        };
        return Ok(Some(ManifestData {
            kind: head.kind.clone(),
            name: head.metadata.name.clone(),
            operation,
            base_envelope: base,
            head_envelope: Some(head),
        }));
    }
    Ok(None)
}

/// The headline kind of a change to a project's encrypted secrets file (CC-06, T-5971). No
/// manifest kind: every right on it is DataSource's, as the secret form's (T-6359).
pub(crate) const SECRET: &str = "Secret";

/// The kind whose grants decide a change of headline `kind`.
fn gate_kind(kind: &str) -> &str {
    if kind == SECRET {
        crate::api::secrets::KIND
    } else {
        kind
    }
}

/// Whether `branch` is the secret form's (`portal/secret-{project}-{name}`, secrets.rs).
fn is_secret_branch(branch: &str) -> bool {
    let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    branch.starts_with("portal/secret-")
}

/// The headline of a secret's change: the encrypted file of `project` it adds, replaces or
/// removes, named by its path. The branch is no key to the name (a project and a name both hold
/// `-`), and no content is read: the file holds ciphertext and the headline names no value.
async fn secret_headline(
    gitea: &GiteaClient,
    pr: &PullRequest,
    project: &str,
    files: Option<&[crate::git::gitea::ChangedFile]>,
) -> Result<Option<ManifestData>, ApiError> {
    let fetched;
    let files = match files {
        Some(files) => files,
        None => {
            fetched = gitea.pull_request_files(pr.number).await?;
            &fetched
        }
    };
    Ok(files.iter().find_map(|file| {
        let (home, name) = crate::secret_files::parse_path(&file.path)?;
        (home == project).then(|| ManifestData {
            kind: SECRET.to_owned(),
            name: name.to_owned(),
            operation: if file.deleted {
                Operation::Delete
            } else if file.added {
                Operation::Create
            } else {
                Operation::Update
            },
            base_envelope: None,
            head_envelope: None,
        })
    }))
}

/// The headline manifest of `pr`. `files` is the merge request's file list when the caller holds
/// it: the manifest is found there before any probe (T-6347); `None` reads it only for a bundle.
async fn load_manifest_data(
    gitea: &GiteaClient,
    pr: &PullRequest,
    project: &str,
    files: Option<&[crate::git::gitea::ChangedFile]>,
) -> Result<Option<ManifestData>, ApiError> {
    if is_secret_branch(&pr.head_branch) {
        return secret_headline(gitea, pr, project, files).await;
    }
    let Some(branch_info) = parse_branch_name(&pr.head_branch) else {
        return bundle_headline(gitea, pr, project, files).await;
    };

    let kind_info =
        crate::resource::kinds().find(|k| k.kind.to_ascii_lowercase() == branch_info.kind_lower);

    let (base_envelope, head_envelope) = match branch_info.operation {
        Operation::Delete => {
            // A kind that lives in either place has two candidate paths; the one the base
            // branch actually carries is the manifest (PF-68).
            let candidates: Vec<String> = kind_info
                .map(|info| {
                    crate::resource::homes(info, project)
                        .into_iter()
                        .filter_map(|home| {
                            crate::resource::repository_path(
                                info,
                                &home,
                                Some(project),
                                &branch_info.resource_name,
                            )
                            .ok()
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut path = manifest_in_files(files, project, &branch_info.resource_name, true);
            for candidate in candidates {
                if path.is_some() {
                    break;
                }
                if let Ok(Some(_)) = gitea.get_file(&candidate, pr.base_ref()).await {
                    path = Some(candidate);
                    break;
                }
            }

            let path = match path {
                Some(p) => Some(p),
                None => {
                    find_manifest_in_tree(gitea, pr.base_ref(), project, &branch_info.resource_name)
                        .await?
                }
            };

            let Some(repo_path) = path else {
                return Ok(None);
            };

            let base_file = gitea
                .get_file(&repo_path, pr.base_ref())
                .await?
                .ok_or_else(|| {
                    ApiError::NotFound(format!(
                        "manifest file '{repo_path}' not found on base branch"
                    ))
                })?;

            let env: ResourceEnvelope = serde_norway::from_str(&base_file.content)
                .map_err(|e| ApiError::Internal(format!("failed to parse base manifest: {e}")))?;

            (Some(env), None)
        }
        Operation::Create | Operation::Update => {
            let direct = kind_info.and_then(|info| {
                crate::resource::repository_path(
                    info,
                    project,
                    Some(project),
                    &branch_info.resource_name,
                )
                .ok()
            });

            let path = if let Some(found) =
                manifest_in_files(files, project, &branch_info.resource_name, false)
            {
                Some(found)
            } else if let Some(cand) = direct {
                if let Ok(Some(_)) = gitea.get_file(&cand, pr.head_ref()).await {
                    Some(cand)
                } else {
                    None
                }
            } else {
                None
            };

            // A project's registry entry, `projects/{slug}.yaml` of the organization repository
            // of layout 2, is the Project manifest a repoint or a new project changes (PF-86).
            let registry = format!("projects/{}.yaml", branch_info.resource_name);
            let path = match path {
                Some(p) => Some(p),
                None if branch_info.kind_lower == "project"
                    && !gitea.is_project_repository()
                    && matches!(gitea.get_file(&registry, pr.head_ref()).await, Ok(Some(_))) =>
                {
                    Some(registry)
                }
                None => {
                    find_manifest_in_tree(gitea, pr.head_ref(), project, &branch_info.resource_name)
                        .await?
                }
            };

            let Some(repo_path) = path else {
                return Ok(None);
            };

            // The two sides are independent reads: one round trip, not two (T-6347).
            let (head_file, base_file) =
                tokio::join!(gitea.get_file(&repo_path, pr.head_ref()), async {
                    if branch_info.operation == Operation::Update {
                        gitea
                            .get_file(&repo_path, pr.base_ref())
                            .await
                            .ok()
                            .flatten()
                    } else {
                        None
                    }
                });
            let head_file = head_file?.ok_or_else(|| {
                ApiError::NotFound(format!(
                    "manifest file '{repo_path}' not found on head branch"
                ))
            })?;

            let head_env: ResourceEnvelope = serde_norway::from_str(&head_file.content)
                .map_err(|e| ApiError::Internal(format!("failed to parse head manifest: {e}")))?;

            let base_env = base_file
                .and_then(|file| serde_norway::from_str::<ResourceEnvelope>(&file.content).ok());

            (base_env, Some(head_env))
        }
    };

    let kind = head_envelope
        .as_ref()
        .map(|e| e.kind.clone())
        .or_else(|| base_envelope.as_ref().map(|e| e.kind.clone()))
        .unwrap_or_else(|| {
            kind_info
                .map(|k| k.kind.to_string())
                .unwrap_or_else(|| branch_info.kind_lower.clone())
        });

    let name = head_envelope
        .as_ref()
        .map(|e| e.metadata.name.clone())
        .or_else(|| base_envelope.as_ref().map(|e| e.metadata.name.clone()))
        .unwrap_or(branch_info.resource_name);

    Ok(Some(ManifestData {
        kind,
        name,
        operation: branch_info.operation,
        base_envelope,
        head_envelope,
    }))
}

/// The human behind a pull request: the author of its head commit (CC-44; the forge token only
/// posts the pull request), the poster when the branch has no commit yet or the forge does not
/// answer.
async fn human_author(gitea: &GiteaClient, pr: &crate::git::gitea::PullRequest) -> ChangeAuthor {
    head_author(gitea, pr).await.unwrap_or_else(|_| poster(pr))
}

/// What [`human_author`] names, or the forge's error when it could not say: the list keeps only
/// an answer, never the fallback a forge blip gave (T-6347).
async fn head_author(
    gitea: &GiteaClient,
    pr: &crate::git::gitea::PullRequest,
) -> Result<ChangeAuthor, GitError> {
    // A writer's own branch: the forge account that opened it. Its commits say whatever their
    // author configured, and an approver named there could approve their own Change (T-3433).
    if !is_change_branch(&pr.head_branch) {
        return Ok(poster(pr));
    }
    // The head commit, not the branch: a merged change's branch may be gone (T-3292).
    let commits = gitea.list_commits(pr.head_ref(), "", 1).await?;
    Ok(match commits.first() {
        Some(commit) if !commit.author.trim().is_empty() => ChangeAuthor {
            name: commit.author.clone(),
            email: commit.email.clone(),
        },
        _ => poster(pr),
    })
}

/// Whether `identity` proposed `pr`, by the e-mail its author carries: never by a display
/// name a second person could share. A forge error answers no, so the caller falls back to the
/// approve rules (T-6535).
async fn proposed_by(
    gitea: &GiteaClient,
    pr: &crate::git::gitea::PullRequest,
    identity: &crate::auth::session::Identity,
) -> bool {
    let author = match head_author(gitea, pr).await {
        Ok(author) => author,
        Err(GitError::NotFound) => poster(pr),
        Err(_) => return false,
    };
    same_email(author.email.as_deref(), identity.email.as_deref())
}

/// Two e-mails equal ignoring case and surrounding space; a missing or blank one equals nothing.
fn same_email(a: Option<&str>, b: Option<&str>) -> bool {
    match (a.map(str::trim), b.map(str::trim)) {
        (Some(a), Some(b)) => !a.is_empty() && a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

/// The forge account that opened `pr`.
fn poster(pr: &crate::git::gitea::PullRequest) -> ChangeAuthor {
    ChangeAuthor {
        name: pr.author_name.clone(),
        email: pr.author_email.clone(),
    }
}

/// The lane every file of a merge request carries, from its path alone (CC-63, T-1224).
///
/// The approval walks the files and reads each one's spec; this is the same walk without the
/// reads, so the list and the detail can say the lane the approval will apply without a fetch
/// per file. It is a lower bound and never a looser one: a kind that is Red whatever it holds
/// (a `Policy`, a `Role`, a registration) is Red here too, and the kinds whose lane depends on
/// their spec — a public `Endpoint`, a sandbox `ContextSpace` — are what the headline manifest
/// already classifies with its spec in hand.
fn lane_of_paths(files: &[crate::git::gitea::ChangedFile]) -> Lane {
    files.iter().fold(Lane::Green, |lane, file| {
        let operation = if file.deleted {
            Operation::Delete
        } else {
            Operation::Create
        };
        let Some(kind) = crate::api::import::native_kind(&file.path) else {
            // A native file beside a manifest carries no kind of its own; the manifest it
            // belongs to is in the same merge request and is classified above.
            return lane;
        };
        crate::api::import::riskiest(
            lane,
            change::classify(kind, operation, &serde_json::Value::Null),
        )
    })
}

fn build_proposal(
    pr: &PullRequest,
    project: &str,
    data: &ManifestData,
    plan: PlanDiff,
    plan_fields: Option<Vec<FieldChange>>,
    carried: Lane,
) -> ChangeProposal {
    let headline = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify_manifest(env, data.operation)
    } else {
        Lane::Yellow
    };
    // The lane a person is shown is the lane the approval enforces (CC-63, UI-23). It used to
    // be the headline manifest's alone, while `approve_change_for` took the riskiest of every
    // file: a bundle whose endpoint is Yellow and whose policy is Red was listed Yellow, so the
    // page never drew the "type the name back" field CC-19 requires and the API then refused
    // the approval for want of it. The change could not be approved at all (T-1224).
    let lane = crate::api::import::riskiest(headline, carried);

    let summary_key = match data.operation {
        Operation::Create => "change.summary.create",
        Operation::Update => "change.summary.update",
        Operation::Delete => "change.summary.delete",
    };

    let mut params = serde_json::Map::new();
    params.insert("kind".to_string(), Value::String(data.kind.clone()));
    params.insert("name".to_string(), Value::String(data.name.clone()));
    params.insert(
        "fields".to_string(),
        Value::Number(plan.fields.len().into()),
    );

    let summary = ChangeSummary {
        key: summary_key.to_string(),
        params,
    };

    let phase = if pr.merged {
        ChangePhase::Merged
    } else if pr.state == "closed" {
        ChangePhase::Rejected
    } else {
        ChangePhase::PendingApproval
    };

    let change_meta = ChangeMeta::from_merge_request(pr.number, project);
    let change_status = ChangeStatus::new(lane, phase, plan.summary)
        .in_repository(&pr.repository)
        .proposed_through(&pr.body)
        .with_merge_request(pr.url.clone());

    ChangeProposal {
        api_version: crate::resource::API_VERSION.to_string(),
        kind: ChangeProposal::KIND.to_string(),
        metadata: change_meta,
        status: change_status,
        summary,
        author: ChangeAuthor {
            name: pr.author_name.clone(),
            email: pr.author_email.clone(),
        },
        created_at: pr.created_at.clone(),
        plan_fields,
        // Filled by the caller that read the merge request's files; a proposal built from the
        // headline alone says nothing about them rather than claiming there is one.
        files: None,
        file_count: None,
        workspace: workspace_of(&pr.head_branch),
        // Filled by the caller, which asks the forge (MF-48).
        waits_on: Vec::new(),
        decision: None,
        // Filled by the detail, which asks the forge (CC-90).
        ci: None,
        space: space_of(data),
    }
}

/// The context space a change's headline manifest lives in: a ContextSpace is its own, any
/// other kind names it in `spec.contextSpaceRef` of its head, else of its base.
fn space_of(data: &ManifestData) -> Option<String> {
    if data.kind == "ContextSpace" {
        return Some(data.name.clone());
    }
    [&data.head_envelope, &data.base_envelope]
        .into_iter()
        .flatten()
        .find_map(|envelope| crate::permissions::space_ref(&serde_json::to_value(envelope).ok()?))
}

/// Whether the caller reads this change: read on its kind and, for a grant bound to one context
/// space, a manifest of that space (PF-59, T-6226). `may_read` alone is true for a grant bound
/// to any space, so a steward of one space read the changes of every other one.
fn may_read_change(effective: &crate::permissions::Effective, proposal: &ChangeProposal) -> bool {
    kind_of(proposal).is_some_and(|kind| {
        // A secret's names are listed to who may write or approve one, as the secrets list is
        // (T-5971, T-6359): `read` on DataSource does not reach them.
        if kind == SECRET {
            return [jc_core::kinds::Verb::Propose, jc_core::kinds::Verb::Approve]
                .into_iter()
                .any(|verb| effective.check(gate_kind(kind), verb, None).is_ok());
        }
        effective.may_read(kind) && effective.may_read_in(kind, proposal.space.as_deref())
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/changes",
    summary = "List Changes",
    description = "Lists open change proposals and merge requests for review.",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
    ),
    responses(
        (status = 200, description = "List of change proposals", body = ChangeList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn list_changes(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<ChangeList>, ApiError> {
    Ok(Json(
        list_changes_readable(&state, &user.0.identity, &project).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/changes/history",
    summary = "List Closed Changes",
    description = "One page of the merged and rejected changes, most recently closed first, each \
                   naming who decided it; `next` is the page that continues it.",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        HistoryQuery,
    ),
    responses(
        (status = 200, description = "One page of closed changes", body = ChangeList),
        (status = 400, description = "An unknown parameter or page 0", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn list_change_history(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    query: Result<Query<HistoryQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ChangeList>, ApiError> {
    let Query(query) = query.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    Ok(Json(
        change_history_readable(&state, &user.0.identity, &project, &query).await?,
    ))
}

/// One page of the closed changes this caller may read, with the read filter of the open list
/// (PF-59, T-3292): a project no binding covers is `404`, a change to a kind the caller does not
/// read is not listed.
pub async fn change_history_readable(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    query: &HistoryQuery,
) -> Result<ChangeList, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let page = query.page.unwrap_or(1);
    if page == 0 {
        return Err(ApiError::BadRequest("page counts from 1".into()));
    }
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &GiteaClient = &gitea;
    let name = query.name.as_deref().map(str::trim).map(str::to_lowercase);
    let author = query
        .author
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_lowercase);
    let since = history_day("since", query.since.as_deref())?;
    let until = history_day("until", query.until.as_deref())?;

    let prs = gitea
        .list_pull_requests_page("closed", page, HISTORY_PAGE)
        .await?;
    let next = (prs.len() == HISTORY_PAGE as usize).then_some(page + 1);
    let mut items = Vec::new();
    for pr in &prs {
        if !matches!(standing(state, gitea, project, pr).await?, Standing::Change) {
            continue;
        }
        // The day it was decided, `closed_at`'s first ten characters, against the filter's days.
        let day = pr.closed_at.get(..10).unwrap_or_default();
        if since.as_deref().is_some_and(|since| day < since)
            || until.as_deref().is_some_and(|until| day > until)
        {
            continue;
        }
        let files = gitea.pull_request_files(pr.number).await?;
        let Some(data) = closed_headline(gitea, pr, project, &files).await? else {
            continue;
        };
        if query
            .kind
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case(&data.kind))
            || name
                .as_deref()
                .is_some_and(|name| !data.name.to_lowercase().contains(name))
        {
            continue;
        }
        let plan = plan::diff(None, None);
        let mut proposal = build_proposal(pr, project, &data, plan, None, lane_of_paths(&files));
        if !may_read_change(&effective, &proposal) {
            continue;
        }
        proposal.metadata = change_meta(state, gitea, pr.number, project);
        proposal.author = human_author(gitea, pr).await;
        if let Some(author) = &author {
            let who = format!(
                "{} {}",
                proposal.author.name,
                proposal.author.email.as_deref().unwrap_or_default()
            )
            .to_lowercase();
            if !who.contains(author.as_str()) {
                continue;
            }
        }
        proposal.file_count = Some(files.len());
        proposal.decision = decision_of(gitea, pr).await?;
        items.push(proposal);
    }
    Ok(ChangeList {
        next,
        ..ChangeList::new(items)
    })
}

/// The resource a closed change was about, from its files: the base of a merged change already
/// holds what it made, so the open list's base-against-head read says nothing any more. The
/// branch names the resource when it is a single one; a bundle is headed by its first manifest in
/// the project, the `Project` first, as `bundle_headline` heads an open one.
async fn closed_headline(
    gitea: &GiteaClient,
    pr: &PullRequest,
    project: &str,
    files: &[crate::git::gitea::ChangedFile],
) -> Result<Option<ManifestData>, ApiError> {
    let home = if project == crate::permissions::ORG_NAMESPACE {
        "users/".to_owned()
    } else {
        format!("projects/{project}/")
    };
    let mut ours: Vec<_> = files
        .iter()
        .filter(|file| file.path.starts_with(&home))
        .collect();
    if ours.is_empty() {
        return Ok(None);
    }
    if is_secret_branch(&pr.head_branch) {
        return secret_headline(gitea, pr, project, Some(files)).await;
    }
    if let Some(branch) = parse_branch_name(&pr.head_branch) {
        let kind = crate::resource::kinds()
            .find(|k| k.kind.to_ascii_lowercase() == branch.kind_lower)
            .map(|k| k.kind.to_string())
            .unwrap_or(branch.kind_lower);
        return Ok(Some(ManifestData {
            kind,
            name: branch.resource_name,
            operation: branch.operation,
            base_envelope: None,
            head_envelope: None,
        }));
    }
    ours.sort_by_key(|file| !file.path.ends_with("/project.yaml"));
    for file in ours {
        let operation = if file.deleted {
            Operation::Delete
        } else if file.added {
            Operation::Create
        } else {
            Operation::Update
        };
        // What the head holds names itself; a deleted file is gone from every ref a merge
        // leaves, so its path is all there is.
        let named = if file.deleted {
            named_by_path(&file.path).map(|(kind, name)| (kind, name, None))
        } else {
            gitea
                .get_file(&file.path, pr.head_ref())
                .await?
                .and_then(|content| {
                    serde_norway::from_str::<ResourceEnvelope>(&content.content).ok()
                })
                .map(|envelope| {
                    (
                        envelope.kind.clone(),
                        envelope.metadata.name.clone(),
                        Some(envelope),
                    )
                })
        };
        if let Some((kind, name, head_envelope)) = named {
            return Ok(Some(ManifestData {
                kind,
                name,
                operation,
                base_envelope: None,
                head_envelope,
            }));
        }
    }
    Ok(None)
}

/// The kind and name a manifest's path gives: `…/{plural}/{name}.yaml`, or the directory of a
/// kind kept as `{name}/space.yaml`, `{name}/pipeline.yaml`, `{name}/app.yaml`, and the project's
/// own `projects/{name}/project.yaml`.
fn named_by_path(path: &str) -> Option<(String, String)> {
    let mut segments: Vec<&str> = path.split('/').collect();
    let file = segments.pop()?;
    let stem = file
        .strip_suffix(".yaml")
        .or_else(|| file.strip_suffix(".yml"))?;
    let directory = |segments: &[&str]| segments.last().map(|s| (*s).to_owned());
    match stem {
        "project" => Some(("Project".to_owned(), directory(&segments)?)),
        "space" => Some(("ContextSpace".to_owned(), directory(&segments)?)),
        "pipeline" | "app" => {
            let kind = crate::api::import::native_kind(path)?;
            Some((kind.to_owned(), directory(&segments)?))
        }
        _ => Some((
            crate::api::import::native_kind(path)?.to_owned(),
            stem.to_owned(),
        )),
    }
}

/// Who merged or rejected `pr`, read back from what the Portal wrote on the forge: the merge
/// commit's "Approved in the Portal by …" line, or the rejection review. A change merged or closed
/// in the forge itself names nobody.
async fn decision_of(
    gitea: &GiteaClient,
    pr: &PullRequest,
) -> Result<Option<ChangeDecision>, ApiError> {
    let at = Some(pr.closed_at.clone()).filter(|at| !at.is_empty());
    if pr.merged {
        if pr.merge_commit_sha.is_empty() {
            return Ok(None);
        }
        let message = gitea.commit_message(&pr.merge_commit_sha).await?;
        return Ok(approver_in(&message).map(|by| ChangeDecision {
            by,
            via: connector_in(&message),
            at,
            reason: None,
        }));
    }
    let reviews = gitea.review_bodies(pr.number).await?;
    Ok(reviews
        .iter()
        .rev()
        .find_map(|body| rejection_in(body))
        .map(|(by, reason)| ChangeDecision {
            by,
            via: None,
            at,
            reason,
        }))
}

/// The approver a merge commit names, as `approve_change_for` writes it.
pub(crate) fn approver_in(message: &str) -> Option<String> {
    named_after(message, "Approved in the Portal by ")
}

/// The connector an autonomous approval names on its approval line (T-6552).
pub(crate) fn connector_in(message: &str) -> Option<String> {
    let line = message
        .lines()
        .rev()
        .find(|text| text.starts_with("Approved in the Portal by "))?;
    let rest = line.split_once(VIA_CONNECTOR)?.1;
    let name = rest.split([' ', ',']).next()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// What an autonomous approval writes after the approver (T-6552).
const VIA_CONNECTOR: &str = ", via connector ";

/// The line `approve_change_for` writes the proposer on (PL-95).
const PROPOSED_BY: &str = "Proposed in the Portal by ";

/// The proposer a merge commit names, as `approve_change_for` writes it (PL-95).
pub(crate) fn proposer_in(message: &str) -> Option<String> {
    named_after(message, PROPOSED_BY)
}

/// The name on the last line that starts with `line`: the Portal writes its lines after the
/// title, so a title that quotes one names nobody.
fn named_after(message: &str, line: &str) -> Option<String> {
    let rest = message
        .lines()
        .rev()
        .find_map(|text| text.strip_prefix(line))?;
    let by = rest.split(',').next()?.trim();
    (!by.is_empty()).then(|| by.to_owned())
}

/// The rejecter and reason a review names, as `reject_change_for` writes them.
fn rejection_in(body: &str) -> Option<(String, Option<String>)> {
    let rest = body.strip_prefix("Change proposal rejected in the Portal by ")?;
    let (by, reason) = match rest.split_once(": ") {
        Some((by, reason)) => (by, Some(reason.trim().to_owned()).filter(|r| !r.is_empty())),
        None => (rest, None),
    };
    let by = by.trim();
    (!by.is_empty()).then(|| (by.to_owned(), reason))
}

/// The open changes this caller may read: the ones whose resource a binding of theirs reads
/// (PF-59, T-0918).
///
/// A change carries the head and base manifests, so reading one is reading the resource. A
/// project no binding covers is `404`, the one answer for "not there" and "not yours" (R20);
/// inside a project the caller reads, a change to a kind they do not read is simply not in the
/// list, the way the resource itself is not.
pub async fn list_changes_readable(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
) -> Result<ChangeList, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let list = list_changes_for(state, project).await?;
    let outside =
        if crate::permissions::for_request(state, identity, crate::permissions::ORG_NAMESPACE)
            .administers_organization()
        {
            list.outside
        } else {
            Vec::new()
        };
    let items = list
        .items
        .into_iter()
        // A change whose manifest names no kind is unreadable rather than open to all.
        .filter(|proposal| may_read_change(&effective, proposal))
        .collect();
    Ok(ChangeList {
        outside,
        ..ChangeList::new(items)
    })
}

/// One change this caller may read, or the `404` that says nothing about which of the two
/// reasons it was (PF-59, R20, T-0918).
pub async fn change_readable(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    id: &str,
) -> Result<ChangeProposal, ApiError> {
    let missing = || {
        ApiError::NotFound(format!(
            "change proposal '{id}' not found in project '{project}'"
        ))
    };
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(missing());
    }
    let proposal = change_for(state, project, id).await?;
    if may_read_change(&effective, &proposal) {
        Ok(proposal)
    } else {
        Err(missing())
    }
}

/// The kind of the resource a change proposes, as `build_proposal` records it.
fn kind_of(proposal: &ChangeProposal) -> Option<&str> {
    proposal.summary.params.get("kind").and_then(Value::as_str)
}

/// Core proposal listing reusable by the REST route, operations registry and MCP.
/// How many open merge requests the listing reads at once: a project's open changes in one
/// wave (helsinki held ~15 when four at a time took ~1 s, T-6347), still a bound on the forge.
const LISTED_AT_ONCE: usize = 16;

/// What the list reads of one open merge request: its files, its headline manifest on head and
/// base, and its author.
pub(crate) struct ChangeRead {
    carried: Vec<crate::git::gitea::ChangedFile>,
    data: Option<ManifestData>,
    author: ChangeAuthor,
}

/// Project, repository, merge request, head commit, base commit.
type ChangeReadKey = (String, String, u64, String, String);

/// The `ChangeRead`s of open changes, per head and base commit (T-6347). The notification
/// badge lists the changes every 30 s, and dev's forge took ~1 s for a list of open changes that
/// had not moved. The forge answers these reads with the Portal's own token, the same for every
/// person, and a commit never changes, so one read per commit pair is the whole truth; who may
/// read a change is still decided per request by [`list_changes_readable`], after this.
/// ponytail: cleared whole at `CHANGE_READS_HELD`; an LRU when a forge holds that many open.
#[derive(Clone, Default)]
pub struct ChangeReads(
    std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<ChangeReadKey, std::sync::Arc<ChangeRead>>>,
    >,
);

/// How many commit pairs [`ChangeReads`] keeps: a project holds tens of open changes.
const CHANGE_READS_HELD: usize = 512;

impl ChangeReads {
    fn get(&self, key: &ChangeReadKey) -> Option<std::sync::Arc<ChangeRead>> {
        self.0.lock().ok()?.get(key).cloned()
    }

    fn keep(&self, key: ChangeReadKey, read: std::sync::Arc<ChangeRead>) {
        let Ok(mut held) = self.0.lock() else {
            return;
        };
        if held.len() >= CHANGE_READS_HELD {
            held.clear();
        }
        held.insert(key, read);
    }
}

/// The [`ChangeRead`] of `pr` at its head and base commits: kept from an earlier list, or read
/// now. A merge request whose forge answer names no head or no base commit is read every time:
/// a branch name moves.
async fn change_read(
    state: &AppState,
    gitea: &GiteaClient,
    project: &str,
    pr: &PullRequest,
) -> Result<std::sync::Arc<ChangeRead>, ApiError> {
    let key = (!pr.head_sha.is_empty() && !pr.base_sha.is_empty()).then(|| {
        (
            project.to_owned(),
            pr.repository.clone(),
            pr.number,
            pr.head_sha.clone(),
            pr.base_sha.clone(),
        )
    });
    if let Some(read) = key.as_ref().and_then(|key| state.change_reads.get(key)) {
        return Ok(read);
    }
    // The file list is fetched here anyway for the count, and its paths carry the lane the
    // approval will apply (T-1224) and the headline manifest's path (T-6347): no extra call,
    // and the list stops promising a lane the detail page contradicts. The author needs nothing
    // the manifest reads find, so both run at once.
    let manifest = async {
        let carried = gitea.pull_request_files(pr.number).await?;
        let data = load_manifest_data(gitea, pr, project, Some(&carried)).await?;
        Ok::<_, ApiError>((carried, data))
    };
    let (manifest, author) = tokio::join!(manifest, head_author(gitea, pr));
    let (carried, data) = manifest?;
    let settled = author.is_ok();
    let read = std::sync::Arc::new(ChangeRead {
        carried,
        data,
        author: author.unwrap_or_else(|_| poster(pr)),
    });
    if let Some(key) = key.filter(|_| settled) {
        state.change_reads.keep(key, std::sync::Arc::clone(&read));
    }
    Ok(read)
}

/// What the listing shows for one open merge request.
enum Listed {
    Change(Box<ChangeProposal>),
    Outside(OutsideMergeRequest),
    Unlisted,
}

async fn listed(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    project: &str,
    pr: &PullRequest,
) -> Result<Listed, ApiError> {
    match standing(state, gitea, project, pr).await? {
        Standing::Change => {}
        Standing::Outside(reason) => {
            return Ok(Listed::Outside(OutsideMergeRequest {
                number: pr.number,
                url: pr.url.clone(),
                author: pr.author_login.clone(),
                reason,
            }))
        }
        Standing::Unlisted => return Ok(Listed::Unlisted),
    }

    // The awaited changes move without a commit of this one: asked on every list, beside the
    // reads its head and base commits fix (T-6347).
    let (read, waits_on) = tokio::join!(
        change_read(state, gitea, project, pr),
        awaited_changes(state, gitea, pr, project)
    );
    let read = read?;
    let Some(data) = read.data.as_ref() else {
        return Ok(Listed::Unlisted);
    };

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let mut proposal = build_proposal(pr, project, data, plan, None, lane_of_paths(&read.carried));
    proposal.metadata = change_meta(state, gitea, pr.number, project);
    proposal.author = read.author.clone();
    // The count only: a listing that read every file of every open change to render
    // "+3 files" would pay for the detail page on the way past it (T-0861).
    proposal.file_count = Some(read.carried.len());
    proposal.waits_on = waits_on?;
    Ok(Listed::Change(Box::new(proposal)))
}

/// Every open change of `project`, unfiltered: a caller reads it through
/// [`list_changes_readable`], which keeps what its rights read (F-R2POR4-011).
async fn list_changes_for(state: &AppState, project: &str) -> Result<ChangeList, ApiError> {
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;

    let prs = gitea.list_pull_requests("open").await?;
    // Each merge request costs a few forge reads: four at a time, in the forge's order. One
    // merged or closed between the list and its own reads answers 404 there: it is no longer
    // open, so it leaves the list instead of failing it whole (T-6324).
    let listed: Vec<Listed> = futures_util::stream::iter(prs.into_iter().map(|pr| async move {
        match listed(state, gitea, project, &pr).await {
            Err(ApiError::NotFound(_)) => Ok(Listed::Unlisted),
            other => other,
        }
    }))
    .buffered(LISTED_AT_ONCE)
    .try_collect()
    .await?;
    let mut proposals = Vec::new();
    let mut outside = Vec::new();
    for entry in listed {
        match entry {
            Listed::Change(proposal) => proposals.push(*proposal),
            Listed::Outside(request) => outside.push(request),
            Listed::Unlisted => {}
        }
    }

    proposals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(ChangeList {
        outside,
        ..ChangeList::new(proposals)
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/changes/{id}",
    summary = "Read One Change",
    description = "One proposed change: what it does, who wrote it, its lane and where it stands.",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Change proposal ID: chg- + 8 hex digits, or chg-org- + 8 hex digits for the organization repository in layout 2 (CC-87)"),
    ),
    responses(
        (status = 200, description = "Change proposal with plan diff", body = ChangeProposal),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Change proposal not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn get_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<ChangeProposal>, ApiError> {
    Ok(Json(
        change_readable(&state, &user.0.identity, &project, &id).await?,
    ))
}

/// One change with its plan; the read behind the change route and the assistant's
/// diagnostics door (AG-57).
pub async fn change_for(
    state: &AppState,
    project: &str,
    id: &str,
) -> Result<ChangeProposal, ApiError> {
    let (gitea, pr_number) = resolve_change(state, project, id)?;
    let gitea: &crate::git::GiteaClient = &gitea;

    let pr = gitea.pull_request(pr_number).await?;
    require_change(state, gitea, project, id, &pr).await?;

    let pr_files = gitea.pull_request_files(pr.number).await?;
    let data = load_manifest_data(gitea, &pr, project, Some(&pr_files))
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let redacted_fields = redact(plan.fields.clone());
    let author = human_author(gitea, &pr).await;
    let files = changed_files(gitea, &pr, pr_files).await?;
    // Here every file's spec was read, so the lane each one carries is the exact one the
    // approval computes rather than the path's lower bound (T-1224).
    let carried = files.iter().fold(Lane::Green, |lane, file| {
        crate::api::import::riskiest(lane, file.lane)
    });
    // A forge that cannot say is a Change without a CI line, never a detail that fails (CC-90).
    let ci = match gitea.commit_state(pr.head_ref()).await {
        Ok(ci) => ci,
        Err(error) => {
            tracing::debug!(change = %id, %error, "the forge did not say the head commit's CI");
            None
        }
    };
    Ok(ChangeProposal {
        ci,
        metadata: change_meta(state, gitea, pr_number, project),
        waits_on: awaited_changes(state, gitea, &pr, project).await?,
        author,
        file_count: Some(files.len()),
        files: Some(files),
        ..build_proposal(&pr, project, &data, plan, Some(redacted_fields), carried)
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/changes/{id}/approve",
    summary = "Approve Change",
    description = "Approves and merges a change proposal.",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Change proposal ID: chg- + 8 hex digits, or chg-org- + 8 hex digits for the organization repository in layout 2 (CC-87)"),
    ),
    request_body(
        content = Option<ApproveBody>,
        description = "Optional approval confirmation for red-lane changes: `confirm` repeats \
                       the resource's name",
        content_type = "application/json",
        example = json!({ "confirm": "helsinki-air" })
    ),
    responses(
        (status = 202, description = "Change proposal approved and deploying", body = Change),
        (status = 400, description = "Bad request or missing red-lane confirmation", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden (missing role or self-approval)", body = ProblemDetails),
        (status = 404, description = "Change proposal not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn approve_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let approve_body: Option<ApproveBody> = if body.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice(&body)
                .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?,
        )
    };
    let confirm = approve_body.as_ref().and_then(|b| b.confirm.as_deref());
    let change = approve_change_for(
        &state,
        &user.0.identity,
        &project,
        &id,
        confirm,
        ApprovedBy::Person,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// Who presses approve: a person at the Portal's button, an operation of the registry, which an
/// agent calls on a person's behalf (AG-11), or a connector its owner set `autonomous` (T-6552),
/// named here: the Portal approves what it opened as that person, and a change that grants access,
/// publishes publicly or deletes still waits for a click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovedBy {
    Person,
    Operation,
    Connector(String),
}

/// Kinds whose change grants or shapes access: a connector's autonomous approval leaves them for
/// a person's click, because prod is public (T-6552).
pub(crate) fn keeps_its_click(kind: &str) -> bool {
    is_access_kind(kind) || matches!(kind, "Policy" | "DelegationPolicy" | "Organization")
}

/// Whether a binding covering the project grants `approve` and `delete` on the change's kind: an
/// administrator of that kind, who may approve their own change (PF-58). The bootstrap group is
/// not a binding and does not count.
fn administers(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    data: &ManifestData,
) -> bool {
    let effective = crate::permissions::for_request(state, identity, project);
    if effective.bootstrap {
        return false;
    }
    let target = data
        .head_envelope
        .as_ref()
        .or(data.base_envelope.as_ref())
        .and_then(|envelope| serde_json::to_value(envelope).ok());
    [jc_core::kinds::Verb::Approve, jc_core::kinds::Verb::Delete]
        .into_iter()
        .all(|verb| {
            effective
                .check(gate_kind(&data.kind), verb, target.as_ref())
                .is_ok()
        })
}

/// Every file the merge request changes, with the kind, the operation and the lane each one
/// carries (T-0861).
///
/// The same walk `approve_every_file` makes, over the same reads: a page that summarised the
/// merge request differently from the checks would be a page an approver cannot trust. A file
/// whose kind this platform does not serve is still listed — the approval refuses it, and an
/// approver who cannot see it cannot understand the refusal.
/// ponytail: one `get_file` per changed file, as the approval does; the tree diff is the
/// upgrade for both at once.
async fn changed_files(
    gitea: &GiteaClient,
    pr: &PullRequest,
    files: Vec<crate::git::gitea::ChangedFile>,
) -> Result<Vec<ChangeFile>, ApiError> {
    let mut listed = Vec::new();
    for file in files {
        let read = |git_ref: &str| {
            let path = file.path.clone();
            let git_ref = git_ref.to_owned();
            async move {
                gitea
                    .get_file(&path, &git_ref)
                    .await
                    .map(|found| found.map(|found| found.content))
            }
        };
        // Both sides of the file, so each manifest of a bundle carries its own diff (T-1397).
        let head = if file.deleted {
            None
        } else {
            read(pr.head_ref()).await?
        };
        let base = if file.added {
            None
        } else {
            read(pr.base_ref()).await?
        };
        let parse = |content: &Option<String>| {
            content
                .as_deref()
                .and_then(|content| serde_norway::from_str::<ResourceEnvelope>(content).ok())
        };
        let (head_envelope, base_envelope) = (parse(&head), parse(&base));
        let fields = (head_envelope.is_some() || base_envelope.is_some())
            .then(|| redact(plan::diff(base_envelope.as_ref(), head_envelope.as_ref()).fields));
        // What the file is, as the side the merge request leaves: head, or base for a delete.
        let envelope = head_envelope.or(base_envelope);
        let kind = envelope
            .as_ref()
            .map(|envelope| envelope.kind.clone())
            .or_else(|| crate::api::import::native_kind(&file.path).map(str::to_owned))
            .or_else(|| crate::secret_files::parse_path(&file.path).map(|_| SECRET.to_owned()))
            .unwrap_or_default();
        let operation = match (file.deleted, file.added) {
            (true, _) => Operation::Delete,
            (_, true) => Operation::Create,
            _ => Operation::Update,
        };
        let lane = match (&envelope, operation) {
            // A delete is red whatever it removes, which is what the approval decides too.
            (_, Operation::Delete) => Lane::Red,
            (Some(envelope), _) => change::classify_manifest(envelope, Operation::Create),
            // A native file beside a manifest carries no spec to classify; the manifest it
            // belongs to is in the same merge request and carries the lane.
            (None, _) => Lane::Green,
        };
        listed.push(ChangeFile {
            path: file.path,
            kind,
            operation,
            lane,
            fields,
        });
    }
    Ok(listed)
}

/// The approval checks of every file the merge request changes, the strictest lane among them
/// (T-0832), and whether the caller may also delete every kind they touch: an author approves
/// their own change only as an administrator of every kind in it (PF-58). The headline manifest
/// is among them and passes the same checks twice, which costs nothing and keeps this loop free
/// of a special case.
/// ponytail: one `get_file` per changed file; a bundle is a handful, the tree diff is the upgrade.
async fn approve_every_file(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    gitea: &GiteaClient,
    pr: &PullRequest,
) -> Result<(Lane, bool, bool, bool, bool, bool), ApiError> {
    // The caller's grants, per project, computed where each file actually lives (PF-50, T-1682).
    // Asking the project in the *path* once would check a file under `projects/other/` against a
    // binding that never reached it: an organization-scoped grant covers every project and is
    // meant to, a grant scoped to one project covers that one, and the difference only shows when
    // the file's own project is the one asked about.
    let mut grants: std::collections::HashMap<String, crate::permissions::Effective> =
        std::collections::HashMap::new();
    let mut lane = Lane::Green;
    let mut deletes_every_kind = true;
    let mut changes_access = false;
    let mut publishes = false;
    let mut grants_approver = false;
    let mut grants_access = false;
    let mut files = Vec::new();
    for file in gitea.pull_request_files(pr.number).await? {
        let git_ref = if file.deleted {
            pr.base_ref()
        } else {
            pr.head_ref()
        };
        let content = gitea
            .get_file(&file.path, git_ref)
            .await?
            .ok_or_else(|| {
                ApiError::Internal(format!(
                    "'{}' is in change proposal {} but not on {git_ref}",
                    file.path, pr.number
                ))
            })?
            .content;
        let envelope = serde_norway::from_str::<ResourceEnvelope>(&content).ok();
        // A secrets file approves only as the form writes it, encrypted; a plain text one under
        // `secrets/` would put a credential in the repository (CC-06, T-6359).
        // Only its `sops` block may be plain: a fake `sops` key beside a plain value, or a
        // manifest saved under `secrets/`, is refused before any kind is asked.
        let sealed = crate::secret_files::parse_path(&file.path).is_some()
            && (file.deleted || crate::secret_files::sealed(&content));
        if crate::secret_files::parse_path(&file.path).is_some() && !sealed {
            return Err(ApiError::Denied(format!(
                "'{}' is under secrets/ but holds a value SOPS did not encrypt; enter the \
                 credential in the project's Secrets form instead (CC-06)",
                file.path
            )));
        }
        files.push((file, envelope, sealed));
    }
    for (file, envelope, sealed) in &files {
        // A file outside `projects/` belongs to the organization — `users/`, `environments/`,
        // `blueprints/`, `org.yaml` — and is decided where the approval is made, as before.
        let home = project_of_path(&file.path).unwrap_or(project).to_owned();
        let effective = grants
            .entry(home.clone())
            .or_insert_with(|| crate::permissions::for_request(state, identity, &home));
        let Some(envelope) = envelope else {
            // A secrets file is approved as the secret form proposes it: on DataSource (T-6359).
            let kind = crate::api::import::native_kind(&file.path)
                .or_else(|| sealed.then_some(crate::api::secrets::KIND))
                .ok_or_else(|| {
                    ApiError::Denied(format!(
                    "'{}' belongs to no kind this platform serves, so no role grants approving \
                     it (PF-50)",
                    file.path
                ))
                })?;
            // A native file whose manifest is in the same change (a DataModel's `.linkml.yaml`)
            // is judged as that manifest, which this walk checks too: a role constrained by
            // `metadata.name` reads the owner's name (T-5162). Without one the path still names
            // the space; a target that holds only the space leaves every constraint as it was
            // and lets a grant scoped to that space reach the file (PF-35, T-1404).
            let owner = files.iter().find_map(|(other, manifest, _)| {
                manifest.as_ref().filter(|manifest| {
                    manifest.kind == kind
                        && crate::api::import::owns_native(
                            &file.path,
                            &other.path,
                            &manifest.metadata.name,
                        )
                })
            });
            let target = match owner {
                Some(manifest) => Some(
                    serde_json::to_value(manifest)
                        .map_err(|e| ApiError::Internal(e.to_string()))?,
                ),
                None => crate::api::import::space_in_path(&file.path)
                    .map(|space| serde_json::json!({ "spec": { "contextSpaceRef": space } })),
            };
            effective.check(kind, jc_core::kinds::Verb::Approve, target.as_ref())?;
            if file.deleted {
                effective.check(kind, jc_core::kinds::Verb::Delete, target.as_ref())?;
            }
            deletes_every_kind &= effective
                .check(kind, jc_core::kinds::Verb::Delete, target.as_ref())
                .is_ok();
            continue;
        };
        // An organization model is every project's schema: a person approves it, never a
        // service account's token (PF-58, DM-77).
        if envelope.kind == "DataModel"
            && envelope.metadata.namespace.as_deref() == Some(crate::permissions::ORG_NAMESPACE)
            && identity.client.is_some()
        {
            return Err(ApiError::Denied(format!(
                "'{}' changes an organization data model, which a person approves in the Portal, never a service account (PF-58, DM-77)",
                file.path
            )));
        }
        let manifest =
            serde_json::to_value(envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
        effective.check(
            &envelope.kind,
            jc_core::kinds::Verb::Approve,
            Some(&manifest),
        )?;
        deletes_every_kind &= effective
            .check(
                &envelope.kind,
                jc_core::kinds::Verb::Delete,
                Some(&manifest),
            )
            .is_ok();
        let access = matches!(
            envelope.kind.as_str(),
            "Role" | "RoleBinding" | "ServiceAccount"
        );
        let namespace = envelope.metadata.namespace.as_deref().unwrap_or(&home);
        let was_public = state
            .mirror
            .get(namespace, "App", &envelope.metadata.name)
            .is_some_and(|stored| app_is_public(&stored.kind, &stored.spec));
        let public = public_publication(identity, file, envelope, was_public)?;
        publishes |= public;
        // A public publication is held for a second person like an access change (EP-142).
        changes_access |= public;
        changes_access |= needs_second_person(&envelope.kind);
        grants_access |= keeps_its_click(&envelope.kind);
        grants_approver |= !file.deleted && grants_its_caller(state, identity, envelope);
        if file.deleted {
            // `delete` is its own verb on every kind, not only the access ones: a cascade that
            // removes a whole project is held to `approve` and `delete` on each kind it takes
            // with it, so nobody approves away what their role never let them delete (PF-77).
            // Against the manifest being deleted, so a steward scoped to its space may delete
            // what that space holds and nothing else (PF-35, T-1404).
            effective.check(
                &envelope.kind,
                jc_core::kinds::Verb::Delete,
                Some(&manifest),
            )?;
            lane = Lane::Red;
            continue;
        }
        if access {
            let replaced = state
                .mirror
                .get(namespace, &envelope.kind, &envelope.metadata.name)
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            crate::permissions::within_own_rights(
                state,
                identity,
                &manifest,
                replaced.as_ref(),
                "approver",
            )?;
        }
        // The approver reads every source and may write every target too (PL-94).
        crate::pipeline_rights::check(
            state,
            identity,
            &home,
            &manifest,
            jc_core::kinds::Verb::Approve,
        )?;
        // The bounds are the operator's and may have tightened since the proposal (PF-97).
        crate::api::mutate::organization_within_bounds(
            state,
            &envelope.kind,
            &envelope.spec,
            ApiError::Conflict,
        )?;
        // And the public-app policy may have been set since (PF-103).
        crate::api::mutate::public_app_allowed(
            state,
            envelope.metadata.namespace.as_deref().unwrap_or_default(),
            envelope,
            ApiError::Conflict,
        )?;
        lane = crate::api::import::riskiest(
            lane,
            change::classify_manifest(envelope, Operation::Create),
        );
    }
    Ok((
        lane,
        deletes_every_kind,
        changes_access,
        publishes,
        grants_approver,
        grants_access,
    ))
}

/// Whether this file of a change publishes in the public catalogue (EP-142) or makes an App
/// public (AP-175; `was_public` says whether the stored App already is). Making data or an App
/// public is a person's decision, taken by a second person: never its proposer, a service
/// account, the assistant or MCP, so a service account approving such a file is refused here.
fn public_publication(
    identity: &crate::auth::session::Identity,
    file: &crate::git::gitea::ChangedFile,
    envelope: &ResourceEnvelope,
    was_public: bool,
) -> Result<bool, ApiError> {
    let app = !was_public && app_is_public(&envelope.kind, &envelope.spec);
    let public = !file.deleted && (publishes_publicly(envelope) || app);
    if public && identity.client.is_some() {
        let what = if app {
            "makes an App public (AP-175)"
        } else {
            "publishes an Endpoint in the public catalogue (EP-142)"
        };
        return Err(ApiError::Denied(format!(
            "'{}' {what}, which a person approves in the Portal, never a service account",
            file.path
        )));
    }
    Ok(public)
}

/// An App anyone opens without a login (AP-120).
fn app_is_public(kind: &str, spec: &serde_json::Value) -> bool {
    kind == "App" && spec.get("visibility").and_then(|v| v.as_str()) == Some("public")
}

/// A public publication (EP-142): an Endpoint whose audience is `public` and that declares
/// `spec.publish.ckan`, so the reconciler puts it in the open-data catalogue.
pub(crate) fn publishes_publicly(envelope: &ResourceEnvelope) -> bool {
    envelope.kind == "Endpoint"
        && envelope.spec.get("audience").and_then(|v| v.as_str()) == Some("public")
        && envelope
            .spec
            .pointer("/publish/ckan")
            .is_some_and(|ckan| !ckan.is_null())
}

/// Who may do what: a change to one of these kinds is approved by a second person, never by
/// the one who proposed it, administrator or not (PF-58, T-4833).
pub(crate) fn is_access_kind(kind: &str) -> bool {
    matches!(kind, "Role" | "RoleBinding" | "Group" | "ServiceAccount")
}

/// Kinds a second person approves: the access kinds, and a DataModel, whose new version every
/// space, Endpoint and pipeline of it validates against (PF-58, T-4830).
pub(crate) fn needs_second_person(kind: &str) -> bool {
    is_access_kind(kind) || kind == "DataModel"
}

/// Whether `envelope` hands `identity` access: a Policy whose grant names them, a Group they are
/// in or a realm role they hold, or a Group change that adds them as a member. Such a change is
/// approved by a second person, never by the one it grants (PF-58, T-5563).
fn grants_its_caller(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    envelope: &ResourceEnvelope,
) -> bool {
    let is_caller = |user: &str| {
        let user = user.trim();
        user.eq_ignore_ascii_case(&identity.username)
            || user == identity.subject
            || identity
                .email
                .as_deref()
                .is_some_and(|email| user.eq_ignore_ascii_case(email.trim()))
    };
    let members_of = |spec: &serde_json::Value| -> Vec<String> {
        spec.get("members")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|member| member.get("user").and_then(serde_json::Value::as_str))
            .map(str::to_owned)
            .collect()
    };
    match envelope.kind.as_str() {
        "Policy" => {
            let rules = envelope
                .spec
                .get("rules")
                .and_then(serde_json::Value::as_array)
                .map(|rules| rules.iter().collect::<Vec<_>>())
                .unwrap_or_else(|| vec![&envelope.spec]);
            rules
                .into_iter()
                .filter(|rule| {
                    rule.get("effect").and_then(serde_json::Value::as_str) != Some("prohibition")
                })
                .filter_map(|rule| rule.get("assignee"))
                .any(|assignee| {
                    let id = assignee
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    match assignee.get("kind").and_then(serde_json::Value::as_str) {
                        Some("user") => is_caller(id),
                        Some("group") => {
                            identity.groups.iter().any(|group| group == id)
                                || state
                                    .mirror
                                    .get(crate::permissions::ORG_NAMESPACE, "Group", id)
                                    .is_some_and(|group| {
                                        members_of(&group.spec).iter().any(|m| is_caller(m))
                                    })
                        }
                        Some("role") => identity.roles.iter().any(|role| role == id),
                        Some("serviceAccount") => identity
                            .client
                            .as_deref()
                            .is_some_and(|client| id.rsplit('/').next() == Some(client)),
                        _ => false,
                    }
                })
        }
        "Group" => {
            let was_member = state
                .mirror
                .get(
                    crate::permissions::ORG_NAMESPACE,
                    "Group",
                    &envelope.metadata.name,
                )
                .is_some_and(|group| members_of(&group.spec).iter().any(|m| is_caller(m)));
            !was_member && members_of(&envelope.spec).iter().any(|m| is_caller(m))
        }
        _ => false,
    }
}

/// Core approval function factored out for reuse by both the REST route and the operations registry.
pub async fn approve_change_for(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    id: &str,
    confirm: Option<&str>,
    by: ApprovedBy,
) -> Result<Change, ApiError> {
    // A connector's token is a client's; only the Portal itself, for a connector its owner set
    // autonomous, approves through it (T-6552).
    let connector = match &by {
        ApprovedBy::Connector(name) => Some(name.as_str()),
        _ => None,
    };
    if connector.is_none() {
        refuse_bearer_decision(identity)?;
    }
    may_approve_anything(state, identity, project)?;
    crate::auth::session::require_second_factor(identity, &state.config)?;
    let (gitea, pr_number) = resolve_change(state, project, id)?;
    let gitea: &crate::git::GiteaClient = &gitea;

    let pr = gitea.pull_request(pr_number).await?;
    require_change(state, gitea, project, id, &pr).await?;
    let data = load_manifest_data(gitea, &pr, project, None)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;
    may_approve(state, identity, project, &data)?;
    // Access changes hold the approver to what they grant, and a removal to `delete` (PF-52).
    if matches!(
        data.kind.as_str(),
        "Role" | "RoleBinding" | "ServiceAccount"
    ) {
        match (data.operation, &data.head_envelope) {
            (Operation::Delete, _) => {
                let base = data
                    .base_envelope
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
                crate::permissions::for_request(state, identity, project).check(
                    &data.kind,
                    jc_core::kinds::Verb::Delete,
                    base.as_ref(),
                )?;
                // PF-03: what was fine when proposed may be the last administrator by now.
                if let Some(base) = &data.base_envelope {
                    crate::permissions::keeps_an_administrator(
                        &state.mirror,
                        crate::permissions::AccessChange::Remove {
                            kind: &data.kind,
                            namespace: base.metadata.namespace.as_deref().unwrap_or(project),
                            name: &base.metadata.name,
                        },
                    )?;
                }
            }
            (_, Some(head)) => {
                let manifest =
                    serde_json::to_value(head).map_err(|e| ApiError::Internal(e.to_string()))?;
                let base = data
                    .base_envelope
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
                crate::permissions::within_own_rights(
                    state,
                    identity,
                    &manifest,
                    base.as_ref(),
                    "approver",
                )?;
            }
            (_, None) => {}
        }
    }

    // Every file of the merge request, not only the headline (T-0832, MF-21, CC-63): each
    // manifest needs approve on its kind, the PF-52 hold when it grants access, and its own
    // lane; a native file approves under the kind its directory names.
    let (
        bundle_lane,
        deletes_every_kind,
        changes_access,
        publishes,
        grants_approver,
        grants_access,
    ) = approve_every_file(state, identity, project, gitea, &pr).await?;
    if connector.is_some() && (publishes || grants_access) {
        return Err(ApiError::Denied(
            "a change that grants access or publishes publicly waits for a person's approval in the Portal, even through an autonomous connector (T-6552, PF-58, EP-142)".to_string(),
        ));
    }
    if publishes && by == ApprovedBy::Operation {
        tracing::warn!(change = %id, approver = %identity.username, "refused: an operation may not approve a public publication");
        return Err(ApiError::Denied(
            "a public publication or a public App is approved by a person in the Portal, never by the assistant, an agent or MCP (EP-142, AP-175)".to_string(),
        ));
    }
    merges_after(id, &awaited_changes(state, gitea, &pr, project).await?)?;

    // A forge blip must not make a proposer a stranger to their own Change: only a head the
    // forge says is not there falls back to who opened the merge request.
    let author = match head_author(gitea, &pr).await {
        Err(GitError::NotFound) => poster(&pr),
        read => read?,
    };
    let is_author = match (&author.email, &identity.email) {
        (Some(pr_email), Some(user_email)) if !pr_email.trim().is_empty() => {
            pr_email.eq_ignore_ascii_case(user_email.trim())
        }
        _ => {
            let user_name = identity.name.as_deref().unwrap_or(&identity.username);
            author.name.eq_ignore_ascii_case(user_name)
                || author.name.eq_ignore_ascii_case(&identity.username)
        }
    };

    // Nobody grants themselves access alone: a Policy naming its author, a Group they are in or
    // a role they hold, or a Group change adding them, waits for someone else, an administrator
    // of Policy included (PF-58, T-5563).
    if is_author && grants_approver {
        tracing::warn!(change = %id, kind = %data.kind, approver = %identity.username, "refused: a change granting access to its own proposer");
        return Err(ApiError::SelfApproval(
            "this change grants access to its own author (a Policy naming them, a Group they are in, or a Group adding them), so a second person approves it, never its author (PF-58)".to_string(),
        ));
    }
    // Access is never granted, nor a model changed, by its own proposer: someone else approves
    // it, an administrator of its kind or the bootstrap group (PF-58, T-4833, T-4830).
    // An autonomous connector's model change is its owner's standing approval (T-6552); access
    // never reaches here, refused above.
    if is_author && connector.is_none() && (changes_access || needs_second_person(&data.kind)) {
        tracing::warn!(change = %id, kind = %data.kind, approver = %identity.username, "refused: the proposer of an access or model change may not approve it");
        return Err(ApiError::SelfApproval(format!(
            "a change to {} is approved by a second administrator, never by its proposer (PF-58)",
            data.kind
        )));
    }
    // An author's own change is approved only by a person, and only as an administrator of every
    // kind in it (PF-58); an agent never does (AG-11).
    if is_author
        && !(matches!(by, ApprovedBy::Person | ApprovedBy::Connector(_))
            && deletes_every_kind
            && administers(state, identity, project, &data))
    {
        return Err(ApiError::SelfApproval(
            "proposal author cannot approve their own change (AG-11); an administrator of its kind may, in the Portal (PF-58)".to_string(),
        ));
    }

    let lane = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify_manifest(env, data.operation)
    } else {
        Lane::Yellow
    };
    let lane = crate::api::import::riskiest(lane, bundle_lane);

    // PF-13: the explicit deletion protocol; a deletion is always Red (`change::classify`).
    if lane == Lane::Red && confirm != Some(&data.name) {
        return Err(ApiError::BadRequest(format!(
            "red lane change requires confirm to be '{}' (CC-19, CC-39)",
            data.name
        )));
    }

    // The Portal's forge token authored the pull request, and Gitea refuses a review from the
    // author (422 "approve your own pull is not allowed"), so the approval is not a forge review:
    // the Portal checked the binding (PF-50) and the author (CC-34) above, and the merge commit
    // records who approved.
    let approver = identity.email.as_deref().unwrap_or(&identity.username);
    let merge_msg = if let Some(name) = connector {
        format!(
            "Merge change proposal {id}: {}\n\nApproved in the Portal by {approver}{VIA_CONNECTOR}{name}, autonomous, as an administrator of {} (T-6552)",
            pr.title, data.kind
        )
    } else if is_author {
        format!(
            "Merge change proposal {id}: {}\n\nApproved in the Portal by {approver}, its author, as an administrator of {} (PF-58)",
            pr.title, data.kind
        )
    } else {
        format!(
            "Merge change proposal {id}: {}\n\nApproved in the Portal by {approver}",
            pr.title
        )
    };
    // The proposer beside the approver, so the reconciler re-checks both people's reach at each
    // render (PL-95). An approver who is the author proposed it too.
    let proposer = if is_author {
        Some(approver)
    } else {
        author.email.as_deref().map(str::trim)
    };
    let merge_msg = match proposer {
        Some(proposer) if !proposer.is_empty() => {
            format!("{merge_msg}\n{PROPOSED_BY}{proposer}")
        }
        _ => merge_msg,
    };
    merge_when_ready(gitea, pr_number, &merge_msg, &pr.head_sha, id).await?;
    say_merged(state, project, id, &data, approver, connector).await;

    // A published application's own repository merges once its Change has (AP-77).
    crate::api::agent_runs::merge_published_application(
        state,
        project,
        data.head_envelope.as_ref(),
        approver,
    )
    .await;
    // An edited dashboard's branch merges at the commit the approved App names (T-6402).
    crate::api::app_dashboard::merge_approved_edit(
        state,
        project,
        data.head_envelope.as_ref(),
        approver,
    )
    .await;

    sync_soon(state);

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let change_meta = change_meta(state, gitea, pr_number, project);
    let change_status = ChangeStatus::new(lane, ChangePhase::Deploying, plan.summary)
        .in_repository(&pr.repository)
        .proposed_through(&pr.body)
        .with_merge_request(pr.url);
    let change = Change::new(change_meta, change_status);

    Ok(change)
}

/// The `change.merged` activity event of an approval (UI-31, T-3292): the feed names the change,
/// its resource and who approved it, and an object page finds it under `{plural}/{name}`.
async fn say_merged(
    state: &AppState,
    project: &str,
    id: &str,
    data: &ManifestData,
    approver: &str,
    connector: Option<&str>,
) {
    let operation = operation_word(data.operation);
    let by = match connector {
        Some(name) => format!("{approver} via connector {name}"),
        None => approver.to_owned(),
    };
    let mut details = serde_json::json!({
        "change": id,
        "resourceKind": data.kind,
        "name": data.name,
        "operation": operation,
        "approvedBy": approver,
    });
    if let Some(name) = connector {
        details["viaConnector"] = Value::String(name.to_owned());
    }
    if let Some(info) = crate::resource::by_kind(&data.kind) {
        details["object"] = Value::String(format!("{}/{}", info.plural, data.name));
    }
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "change.merged".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "Change {id} merged: {operation} {} {}, approved by {by}.",
            data.kind, data.name
        ),
        severity: "info".to_owned(),
        correlation_id: Some(id.to_owned()),
        details,
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(change = %id, error = %err, "the merge is not on the activity feed");
    }
}

/// How the feed names what a Change does to its resource.
fn operation_word(operation: Operation) -> &'static str {
    match operation {
        Operation::Create => "create",
        Operation::Update => "update",
        Operation::Delete => "delete",
    }
}

/// The longest rejection reason the activity feed keeps, in characters: the reason is free text
/// the rejecter typed, and an event is a line on a feed, not a document (OPS-48).
const REJECTION_REASON_CHARS: usize = 500;

/// The `change.rejected` activity event of a rejection (T-5613): the feed names the change, its
/// resource, who rejected it and why, as `change.merged` names the approver. Only kind, name and
/// reason travel; the manifest stays on the forge.
async fn say_rejected(
    state: &AppState,
    project: &str,
    id: &str,
    data: &ManifestData,
    rejecter: &str,
    reason: Option<&str>,
    withdrawn: bool,
) {
    let operation = operation_word(data.operation);
    let reason = reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(
            |reason| match reason.char_indices().nth(REJECTION_REASON_CHARS) {
                Some((end, _)) => format!("{}…", &reason[..end]),
                None => reason.to_owned(),
            },
        );
    let mut details = serde_json::json!({
        "change": id,
        "resourceKind": data.kind,
        "name": data.name,
        "operation": operation,
        "rejectedBy": rejecter,
        "withdrawnByProposer": withdrawn,
    });
    if let Some(ref reason) = reason {
        details["reason"] = Value::String(reason.clone());
    }
    if let Some(info) = crate::resource::by_kind(&data.kind) {
        details["object"] = Value::String(format!("{}/{}", info.plural, data.name));
    }
    let summary = match reason {
        Some(reason) => format!(
            "Change {id} rejected: {operation} {} {}, by {rejecter}: {reason}",
            data.kind, data.name
        ),
        None => format!(
            "Change {id} rejected: {operation} {} {}, by {rejecter}.",
            data.kind, data.name
        ),
    };
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "change.rejected".to_owned(),
        source: "portal".to_owned(),
        summary,
        severity: "info".to_owned(),
        correlation_id: Some(id.to_owned()),
        details,
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(change = %id, error = %err, "the rejection is not on the activity feed");
    }
}

/// Merges an approved pull request at the commit its approval read (PF-57, T-1683).
///
/// Gitea checks a fresh pull's mergeability in the background and answers 405 "Please try again
/// later" until it has; an approval that follows the proposal within seconds (the demo's, a
/// script's, the build lane's) waits it out instead of failing. The forge refuses to merge a
/// commit other than `head_sha`, and a branch that no longer merges into the base; both come back
/// 409 and both mean the same thing to whoever approved (T-1683, CC-80). A branch another merge
/// overtook is brought up to date first (PF-105, `bring_up_to_date`).
async fn merge_when_ready(
    gitea: &GiteaClient,
    pr_number: u64,
    message: &str,
    head_sha: &str,
    id: &str,
) -> Result<(), ApiError> {
    let mut head = head_sha.to_owned();
    let mut attempt = 0;
    loop {
        match gitea
            .merge(pr_number, MergeStyle::Squash, message, Some(&head))
            .await
        {
            Err(err) if behind_base(&err) && attempt < 15 => {
                attempt += 1;
                head = bring_up_to_date(gitea, pr_number, head_sha, id).await?;
            }
            Err(GitError::Api { status: 405, .. }) if attempt < 15 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(GitError::Conflict(_)) => return Err(moved_on(id)),
            other => return other.map_err(ApiError::from),
        }
    }
}

fn moved_on(id: &str) -> ApiError {
    ApiError::Conflict(format!(
        "change proposal '{id}' is not what it was when it was reviewed: its branch has moved \
         past the commit this approval read, or it no longer merges into the base branch. Open \
         the change again, read what it says now, and approve that (PF-57)."
    ))
}

/// The forge's refusal of a branch that is behind its base (PF-105): a 405 like "not checked
/// yet", told apart only by its message.
fn behind_base(err: &GitError) -> bool {
    matches!(err, GitError::Api { status: 405, message } if message.contains("behind the base branch"))
}

/// Merges `main` into a Change's branch that fell behind it, and answers the branch's new head
/// (PF-105). The approval read the files at `approved`; it still covers the updated head only
/// when every file the Change touches reads the same there, so a file `main` changed meanwhile
/// sends the Change back to its reviewer, naming the file.
async fn bring_up_to_date(
    gitea: &GiteaClient,
    pr_number: u64,
    approved: &str,
    id: &str,
) -> Result<String, ApiError> {
    match gitea.update_pull_request(pr_number).await {
        Err(GitError::Conflict(_)) => return Err(moved_on(id)),
        other => other?,
    }
    let pr = gitea.pull_request(pr_number).await?;
    // The branch, not the pull request: the forge refreshes the pull's head in the background.
    let head = gitea.branch_head(&pr.head_branch).await?;
    let before: std::collections::HashMap<String, String> =
        gitea.list_tree_blobs(approved).await?.into_iter().collect();
    let after: std::collections::HashMap<String, String> =
        gitea.list_tree_blobs(&head).await?.into_iter().collect();
    let changed: Vec<String> = gitea
        .pull_request_files(pr_number)
        .await?
        .into_iter()
        .filter(|file| before.get(&file.path) != after.get(&file.path))
        .map(|file| file.path)
        .collect();
    if !changed.is_empty() {
        return Err(ApiError::Conflict(format!(
            "change proposal '{id}' fell behind main, and main has changed {} since it was \
             reviewed. Open the change again, read what it says now, and approve that (PF-57, \
             PF-105).",
            changed.join(", ")
        )));
    }
    Ok(head)
}

/// The mirror catches up with `main` now rather than at the next poll, so what an approval merged
/// shows at once.
fn sync_soon(state: &AppState) {
    if let Some(syncer) = state.syncer.clone() {
        syncer.merged();
    }
}

/// The build lane's `status.build`, approved by the Portal itself (AP-104, AP-73; owner decision
/// T-2661). The caller has already held the write to the lane's rule, to an App on `main` whose
/// spec, labels and annotations it leaves as they are, and to a build its repository's workflow
/// made and the Portal published; a person approved that source when its own Change merged
/// (AP-77). So the Change merges now, pinned to the commit the Portal wrote, with the Portal named
/// as its approver. The lane approves nothing (AG-11): it holds no `approve`, and this runs only
/// inside the proposal that passed those checks.
///
/// A Change that carries any file beside `manifest_path` is not the lane's field alone, and waits
/// for a person like every other; so does one the forge will not merge. Either way the answer is
/// the pending Change, as it was before this approval existed.
pub(crate) async fn approve_build(
    state: &AppState,
    gitea: &GiteaClient,
    pr: &PullRequest,
    manifest_path: &str,
    change: Change,
) -> Change {
    let message = format!(
        "Merge change proposal {}: {}\n\nApproved by the Portal: the build lane's status.build, \
         checked against the App's repository and published (AP-73, AP-104)",
        change.metadata.name, pr.title
    );
    merge_if_only(state, gitea, pr, &[manifest_path], &message, change).await
}

/// Merges `pr` now, pinned to the commit the Portal wrote, when every file it changes is one of
/// `paths` and none is deleted: the build lane's `status.build` (AP-104) and a green blueprint
/// flow (CC-63, CC-65, AG-14) are merged by the Portal only while the merge request holds exactly
/// what the Portal checked. A merge request with any other file, one whose files cannot be read,
/// or one the forge will not merge comes back as the pending Change it was, waiting for a person.
pub(crate) async fn merge_if_only(
    state: &AppState,
    gitea: &GiteaClient,
    pr: &PullRequest,
    paths: &[&str],
    message: &str,
    change: Change,
) -> Change {
    let id = change.metadata.name.clone();
    let only_these = match gitea.pull_request_files(pr.number).await {
        Ok(files) => {
            !files.is_empty()
                && files
                    .iter()
                    .all(|file| !file.deleted && paths.contains(&file.path.as_str()))
        }
        Err(err) => {
            tracing::warn!(change = %id, error = %err, "the change waits for a person: its files could not be read");
            return change;
        }
    };
    if !only_these {
        tracing::warn!(change = %id, "the change carries a file the Portal did not check and waits for a person");
        return change;
    }
    if let Err(err) = merge_when_ready(gitea, pr.number, message, &pr.head_sha, &id).await {
        tracing::warn!(change = %id, error = %err, "the change waits for a person: the forge did not merge it");
        return change;
    }
    tracing::info!(change = %id, "the Portal merged the change as proposed");
    sync_soon(state);
    let mut change = change;
    change.status.phase = ChangePhase::Deploying;
    change
}

/// Merges a Change the Portal just wrote that removes access and adds none (AG-132): its one file
/// must be a manifest `narrows` accepts against the base the forge holds now, not the mirror, so a
/// grant the main branch dropped since the mirror's last sync is never written back. Anything
/// else comes back pending, for a second person like every access change (PF-58).
pub(crate) async fn merge_if_narrowing(
    state: &AppState,
    project: &str,
    change: Change,
    message: &str,
    narrows: impl Fn(&ResourceEnvelope, &ResourceEnvelope) -> bool,
) -> Change {
    let id = change.metadata.name.clone();
    let Ok((gitea, number)) = resolve_change(state, project, &id) else {
        return change;
    };
    let pr = match gitea.pull_request(number).await {
        Ok(pr) => pr,
        Err(err) => {
            tracing::warn!(change = %id, error = %err, "the revoke waits for a person: its merge request could not be read");
            return change;
        }
    };
    let files = match gitea.pull_request_files(pr.number).await {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!(change = %id, error = %err, "the revoke waits for a person: its files could not be read");
            return change;
        }
    };
    let narrowed = match load_manifest_data(&gitea, &pr, project, Some(&files)).await {
        Ok(Some(ManifestData {
            base_envelope: Some(base),
            head_envelope: Some(head),
            ..
        })) => narrows(&base, &head),
        _ => false,
    };
    if !narrowed {
        tracing::warn!(change = %id, "the change does not only remove access and waits for a person");
        return change;
    }
    let [file] = files.as_slice() else {
        return change;
    };
    let path = file.path.clone();
    merge_if_only(state, &gitea, &pr, &[path.as_str()], message, change).await
}

/// A person decides a Change with their own Portal session; a bearer token is a workload's or an
/// agent's, and one exchanged for a person (AG-94) still speaks for the agent that holds it, so
/// it never approves or rejects, whatever the person's own bindings grant (PF-58, AG-11).
fn refuse_bearer_decision(identity: &crate::auth::session::Identity) -> Result<(), ApiError> {
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "a change is approved or rejected by a person in the Portal, never with a service \
             account's or an agent's token (PF-58, AG-11)"
                .into(),
        ));
    }
    Ok(())
}

/// A change a person just proposed with their own session, approved in the same call when they
/// administer every kind it touches (PF-58): the button's one approval path, so the checks, the
/// merge and its commit message are the same. Anyone else's change comes back as it was, waiting
/// for an approver (PF-70), and so do an administrator's access change, which a second person
/// approves (PF-58, T-4833), and their red-lane change without its typed name (CC-39). Only the Portal's session doors call this; an agent run, a bearer caller and MCP
/// never do (AG-11, AG-82).
pub async fn approve_as_proposed(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    kind: &str,
    change: Change,
    confirm: Option<&str>,
) -> Change {
    let effective = crate::permissions::for_request(state, identity, project);
    // The bootstrap group is not a binding and administers nothing (PF-58).
    let administers = !effective.bootstrap
        && effective.may(kind, jc_core::kinds::Verb::Approve)
        && effective.may(kind, jc_core::kinds::Verb::Delete);
    // Access and a model wait for a second administrator (PF-58).
    if !administers
        || needs_second_person(kind)
        || (change.status.lane == Lane::Red && confirm.is_none())
    {
        return change;
    }
    match approve_change_for(
        state,
        identity,
        project,
        &change.metadata.name,
        confirm,
        ApprovedBy::Person,
    )
    .await
    {
        Ok(approved) => approved,
        Err(err) => {
            tracing::info!(change = %change.metadata.name, error = %err, "the proposer's own change waits for an approver");
            change
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/changes/{id}/reject",
    summary = "Reject Change",
    description = "Rejects a change proposal with a reason and closes its merge request.",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Change proposal ID: chg- + 8 hex digits, or chg-org- + 8 hex digits for the organization repository in layout 2 (CC-87)"),
    ),
    request_body(
        content = Option<ApproveBody>,
        description = "Optional reject payload",
        content_type = "application/json",
        example = json!({ "reason": "The endpoint would publish the stations' maintenance notes" })
    ),
    responses(
        (status = 202, description = "Change proposal rejected", body = Change),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Change proposal not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn reject_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let body: ApproveBody = if body.is_empty() {
        ApproveBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?
    };
    let change = reject_change_for(
        &state,
        &user.0.identity,
        &project,
        &id,
        body.reason.as_deref(),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// Rejects a change proposal as `identity`: the one path behind the REST route and
/// `jc_change_reject` (AG-77). Needs `approve` on the change's kind (PF-50); the merge request is
/// closed with a comment naming who rejected it and why.
pub async fn reject_change_for(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    id: &str,
    reason: Option<&str>,
) -> Result<Change, ApiError> {
    refuse_bearer_decision(identity)?;
    let approves = may_approve_anything(state, identity, project);
    if approves.is_ok() {
        crate::auth::session::require_second_factor(identity, &state.config)?;
    }
    // A caller without `approve` learns nothing about the change unless it turns out theirs.
    let hide = |found: ApiError| {
        may_approve_anything(state, identity, project)
            .err()
            .unwrap_or(found)
    };
    let (gitea, pr_number) = resolve_change(state, project, id).map_err(hide)?;
    let gitea: &crate::git::GiteaClient = &gitea;

    let pr = gitea
        .pull_request(pr_number)
        .await
        .map_err(|e| hide(e.into()))?;
    require_change(state, gitea, project, id, &pr)
        .await
        .map_err(hide)?;
    let data = load_manifest_data(gitea, &pr, project, None)
        .await
        .map_err(hide)?
        .ok_or_else(|| {
            hide(ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            )))
        })?;
    // The proposer withdraws their own change: closing never merges, so the second-person rule
    // of approval does not hold it open (T-6535). Anyone else rejects with `approve` (PF-50).
    let withdrawn = proposed_by(gitea, &pr, identity).await;
    if !withdrawn {
        approves?;
        may_approve(state, identity, project, &data)?;
    }

    // A comment, not a "request changes" review: the forge token is the pull request's author
    // and Gitea refuses the author's verdict on their own pull; the Portal's role check above is
    // the gate (PF-50), the comment says who rejected and why.
    let rejecter = identity.email.as_deref().unwrap_or(&identity.username);
    let comment = match reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        Some(reason) => format!("Change proposal rejected in the Portal by {rejecter}: {reason}"),
        None => format!("Change proposal rejected in the Portal by {rejecter}"),
    };
    gitea
        .review(pr_number, ReviewEvent::Comment, &comment)
        .await?;
    // Closed, not merged, is what the list reads back as Rejected; a comment alone would leave
    // the proposal pending.
    gitea.close_pull_request(pr_number).await?;

    // The run that proposed a rejected application stops waiting for it (T-3015). An
    // organization Change (`chg-org-`) never publishes an application, and its number counts
    // in another repository than the runs' own.
    if let (Some(run_id), Ok(_)) = (
        crate::api::agent_runs::publishing_run(data.head_envelope.as_ref()),
        parse_change_id(id),
    ) {
        let why = match reason.map(str::trim).filter(|reason| !reason.is_empty()) {
            Some(reason) => format!(
                "its change {id} was rejected by {rejecter}: {reason}; the application was not \
                 published, start a new run from the Apps page to try again"
            ),
            None => format!(
                "its change {id} was rejected by {rejecter}; the application was not published, \
                 start a new run from the Apps page to try again"
            ),
        };
        crate::api::agent_runs::end_rejected_publication(state, project, run_id, pr_number, &why)
            .await;
    }

    say_rejected(state, project, id, &data, rejecter, reason, withdrawn).await;

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let lane = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify_manifest(env, data.operation)
    } else {
        Lane::Yellow
    };

    let change_meta = change_meta(state, gitea, pr_number, project);
    let change_status = ChangeStatus::new(lane, ChangePhase::Rejected, plan.summary)
        .in_repository(&pr.repository)
        .proposed_through(&pr.body)
        .with_merge_request(pr.url);
    Ok(Change::new(change_meta, change_status))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/changes",
            axum::routing::get(list_changes),
        )
        // A static segment outranks `{id}`, and `history` is no change id (`chg-…`).
        .route(
            "/projects/{project}/changes/history",
            axum::routing::get(list_change_history),
        )
        .route(
            "/projects/{project}/changes/{id}",
            axum::routing::get(get_change),
        )
        .route(
            "/projects/{project}/changes/{id}/approve",
            axum::routing::post(approve_change),
        )
        .route(
            "/projects/{project}/changes/{id}/reject",
            axum::routing::post(reject_change),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-6390: a new space proposed with its DataModel and LinkML source is headed by the space,
    /// whatever order the forge lists the files in; a project's own manifest still comes first.
    #[test]
    fn a_space_heads_the_bundle_it_brings_with_its_model() {
        let space = "projects/t6025-full/spaces/t6377-walk/space.yaml";
        let model = "projects/t6025-full/spaces/t6377-walk/datamodels/t6377-walk/t6377-walk.yaml";
        let source =
            "projects/t6025-full/spaces/t6377-walk/datamodels/t6377-walk/t6377-walk.linkml.yaml";
        let mut files = [model, source, space];
        files.sort_by_key(|path| headline_rank(path));
        assert_eq!(files, [space, model, source]);
        let mut files = [space, "projects/t6025-full/project.yaml"];
        files.sort_by_key(|path| headline_rank(path));
        assert_eq!(files[0], "projects/t6025-full/project.yaml");
    }

    #[test]
    fn a_seed_file_heads_a_change_as_an_entity_and_nothing_else_does() {
        let path = "projects/p/spaces/air/entities/seed/stations.json";
        let envelope = seed_envelope(path, r#"{"id": "urn:a", "type": "T"}"#).expect("a seed");
        assert_eq!(envelope.kind, "Entity");
        assert_eq!(envelope.metadata.name, "stations");
        assert_eq!(envelope.metadata.namespace.as_deref(), Some("p"));
        assert_eq!(envelope.metadata.labels["joinedcontext.com/space"], "air");
        for other in [
            "projects/p/spaces/air/entities/seed/stations.yaml",
            "projects/p/spaces/air/entities/seed/deeper/s.json",
            "projects/p/spaces/air/pipelines/seed/s.json",
            "projects/p/spaces/air/entities/seed/.json",
        ] {
            assert!(seed_envelope(other, "{}").is_none(), "{other}");
        }
        assert!(seed_envelope(path, "not json").is_none());
    }
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::auth::session::{Identity, Session};
    use crate::config::Config;

    /// PF-59 (T-6226, F-R2POR2-001): a reader bound to one context space reads the changes of
    /// that space alone. `may_read(kind)` held for a grant bound to any space, so a steward of
    /// `air` read the plan of every Endpoint change of `water`.
    #[test]
    fn a_reader_bound_to_one_space_reads_only_that_spaces_changes() {
        use crate::resource::{ObjectMeta, API_VERSION};
        let state = AppState::new(Config::for_tests(), None);
        let envelope =
            |kind: &str, name: &str, namespace: &str, spec: serde_json::Value| ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new(name, namespace),
                spec,
                status: None,
            };
        for space in ["air", "water"] {
            state
                .mirror
                .upsert(envelope("ContextSpace", space, "p", json!({})));
        }
        state.mirror.upsert(envelope(
            "Role",
            "endpoint-reader",
            crate::permissions::ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace"], "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            "jana-reads-air",
            crate::permissions::ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": "jana@hel.fi" }],
                "role": "endpoint-reader",
                "scope": { "contextSpace": "air" },
            }),
        ));
        let jana = test_user("jana@hel.fi", vec![]).0.identity;
        let jana = Identity {
            username: "jana@hel.fi".into(),
            ..jana
        };
        let effective = crate::permissions::for_request(&state, &jana, "p");
        let change = |kind: &str, name: &str, space: &str| {
            let data = ManifestData {
                kind: kind.to_owned(),
                name: name.to_owned(),
                operation: Operation::Update,
                base_envelope: None,
                head_envelope: Some(envelope(
                    kind,
                    name,
                    "p",
                    json!({ "contextSpaceRef": space }),
                )),
            };
            build_proposal(
                &PullRequest::default(),
                "p",
                &data,
                plan::diff(None, None),
                None,
                Lane::Green,
            )
        };
        assert!(may_read_change(
            &effective,
            &change("Endpoint", "air-now", "air")
        ));
        assert!(
            !may_read_change(&effective, &change("Endpoint", "water-level", "water")),
            "a reader of air read a change of water"
        );
        assert!(may_read_change(
            &effective,
            &change("ContextSpace", "air", "air")
        ));
        assert!(!may_read_change(
            &effective,
            &change("ContextSpace", "water", "water")
        ));
        let unknown = ChangeProposal {
            space: None,
            ..change("Endpoint", "gone", "air")
        };
        assert!(
            !may_read_change(&effective, &unknown),
            "a change whose space is unknown is a project-wide reader's alone"
        );
        let bootstrap = crate::permissions::Effective {
            project: "p".into(),
            bootstrap: true,
            grants: Vec::new(),
            projects: None,
        };
        assert!(may_read_change(&bootstrap, &unknown));
    }

    fn test_user(email: &str, roles: Vec<&str>) -> CurrentUser {
        CurrentUser(Session {
            identity: Identity {
                client: None,
                subject: "sub-approver".into(),
                username: "jana.kovacova".into(),
                email: Some(email.into()),
                name: Some("Jana Kováčová".into()),
                roles: roles.into_iter().map(String::from).collect(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    #[test]
    fn public_publication_refuses_a_service_account_and_skips_a_deletion() {
        let endpoint: ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "Endpoint", "metadata": {"name": "air"},
            "spec": {"audience": "public", "publish": {"ckan": {}}}
        }))
        .unwrap();
        let file = |deleted| crate::git::gitea::ChangedFile {
            path: "projects/p/endpoints/air.yaml".into(),
            deleted,
            added: false,
        };
        let person = test_user("jana@example.org", vec![]).0.identity;
        let robot = Identity {
            client: Some("svc-ingest".into()),
            ..person.clone()
        };
        assert!(public_publication(&person, &file(false), &endpoint, false).unwrap());
        assert!(!public_publication(&robot, &file(true), &endpoint, false).unwrap());
        let refused = public_publication(&robot, &file(false), &endpoint, false).unwrap_err();
        assert!(matches!(refused, ApiError::Denied(m) if m.contains("EP-142")));
    }

    #[test]
    fn making_an_app_public_is_a_public_publication_and_keeping_it_public_is_not() {
        let app: ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "App", "metadata": {"name": "map"},
            "spec": {"visibility": "public"}
        }))
        .unwrap();
        let file = crate::git::gitea::ChangedFile {
            path: "projects/p/apps/map.yaml".into(),
            deleted: false,
            added: false,
        };
        let person = test_user("jana@example.org", vec![]).0.identity;
        let robot = Identity {
            client: Some("svc-ingest".into()),
            ..person.clone()
        };
        assert!(public_publication(&person, &file, &app, false).unwrap());
        assert!(!public_publication(&person, &file, &app, true).unwrap());
        assert!(public_publication(&robot, &file, &app, true).is_ok());
        let refused = public_publication(&robot, &file, &app, false).unwrap_err();
        assert!(matches!(refused, ApiError::Denied(m) if m.contains("AP-175")));
    }

    #[test]
    fn test_parse_change_id() {
        assert_eq!(parse_change_id("chg-00000001").unwrap(), 1);
        assert_eq!(parse_change_id("chg-0000002a").unwrap(), 42);
        assert_eq!(parse_change_id("chg-0000019c").unwrap(), 412);

        assert!(parse_change_id("mr-00000001").is_err());
        assert!(parse_change_id("chg-1").is_err());
        assert!(parse_change_id("chg-000000001").is_err());
        assert!(parse_change_id("chg-0000001A").is_err());
        assert!(parse_change_id("chg-0000000z").is_err());
        // CC-87: the organization repository's merge requests under a project of layout 2.
        assert_eq!(parse_change_ref("chg-org-0000002a").unwrap(), (true, 42));
        assert_eq!(parse_change_ref("chg-0000002a").unwrap(), (false, 42));
        assert!(parse_change_id("chg-org-0000002a").is_err());
        assert!(parse_change_ref("chg-org-2a").is_err());
        assert!(parse_change_ref("chg-org-").is_err());
        assert!(parse_change_ref("chg-ORG-0000002a").is_err());
    }

    #[test]
    fn test_parse_branch_name() {
        // A restore names no one resource: its files head it, as a bundle's do (T-3247).
        assert_eq!(parse_branch_name("portal/restore-00000007"), None);
        let b1 = parse_branch_name("portal/create-contextspace-mobility-12345678").unwrap();
        assert_eq!(b1.operation, Operation::Create);
        assert_eq!(b1.kind_lower, "contextspace");
        assert_eq!(b1.resource_name, "mobility");
        assert_eq!(b1.hash, "12345678");

        let b2 = parse_branch_name("portal/update-endpoint-public-air-87654321").unwrap();
        assert_eq!(b2.operation, Operation::Update);
        assert_eq!(b2.kind_lower, "endpoint");
        assert_eq!(b2.resource_name, "public-air");
        assert_eq!(b2.hash, "87654321");

        let b3 = parse_branch_name("portal/delete-pipeline-traffic-stream-abcdef01").unwrap();
        assert_eq!(b3.operation, Operation::Delete);
        assert_eq!(b3.kind_lower, "pipeline");
        assert_eq!(b3.resource_name, "traffic-stream");
        assert_eq!(b3.hash, "abcdef01");

        assert!(parse_branch_name("main").is_none());
        assert!(parse_branch_name("portal/invalid").is_none());
        assert!(parse_branch_name("portal/foo-bar-baz-123").is_none());
        assert_eq!(
            crate::api::import::space_in_path("projects/ovzdusie/spaces/air/mappings/aq.jsonata"),
            Some("air")
        );
        assert_eq!(
            crate::api::import::space_in_path("projects/ovzdusie/spaces/air"),
            None
        );
        assert_eq!(
            crate::api::import::space_in_path("projects/ovzdusie/pipelines/aq/pipeline.yaml"),
            None
        );
        assert_eq!(
            crate::api::import::space_in_path("users/bindings/x.yaml"),
            None
        );
        // Bundles name no one resource and are headed by their files (T-1400); a hyphenated
        // project's deletion no longer reads as the Project of its first word.
        for bundle in [
            "portal/flow-air-quality-0123456789abcdef",
            "portal/import-helsinki-01234567",
            "portal/delete-project-ovzdusie",
            "portal/delete-project-helsinki-mobility",
        ] {
            assert!(parse_branch_name(bundle).is_none(), "{bundle}");
        }
        // A retry's nonce (T-0887) is not part of the name.
        let b4 = parse_branch_name("portal/update-pipeline-hel-news-0711bca4_144d2358").unwrap();
        assert_eq!(b4.operation, Operation::Update);
        assert_eq!(b4.kind_lower, "pipeline");
        assert_eq!(b4.resource_name, "hel-news");
        assert_eq!(b4.hash, "0711bca4");
    }

    #[test]
    fn test_redact() {
        let fields = vec![
            FieldChange {
                path: "metadata.name".to_string(),
                from: None,
                to: Some(json!("public-air")),
            },
            FieldChange {
                path: "spec.password".to_string(),
                from: Some(json!("old-pass")),
                to: Some(json!("new-pass")),
            },
            FieldChange {
                path: "spec.auth.apiKey".to_string(),
                from: None,
                to: Some(json!("my-api-key")),
            },
            FieldChange {
                path: "spec.token".to_string(),
                from: Some(json!("deleted-token")),
                to: None,
            },
            FieldChange {
                path: "spec.clientSecret".to_string(),
                from: Some(json!("secret-1")),
                to: Some(json!("secret-2")),
            },
        ];

        let redacted = redact(fields);
        assert_eq!(redacted[0].to, Some(json!("public-air")));
        assert_eq!(redacted[1].from, Some(json!("[REDACTED]")));
        assert_eq!(redacted[1].to, Some(json!("[REDACTED]")));
        assert_eq!(redacted[2].from, None);
        assert_eq!(redacted[2].to, Some(json!("[REDACTED]")));
        assert_eq!(redacted[3].from, Some(json!("[REDACTED]")));
        assert_eq!(redacted[3].to, None);
        assert_eq!(redacted[4].from, Some(json!("[REDACTED]")));
        assert_eq!(redacted[4].to, Some(json!("[REDACTED]")));
    }

    /// T-1142, CC-06: the rule is the path's last segment, which is what makes it predictable.
    /// A field whose last segment is not a credential name is shown, even where an earlier
    /// segment is — `spec.auth.headerRef.name` names a Secret, and a reference is what a
    /// manifest is supposed to carry, so hiding it would hide the review.
    #[test]
    fn what_is_redacted_is_decided_by_the_last_segment_of_the_path() {
        let field = |path: &str| FieldChange {
            path: path.to_string(),
            from: Some(json!("before")),
            to: Some(json!("after")),
        };
        let shown = |path: &str| redact(vec![field(path)])[0].to.clone() == Some(json!("after"));

        // A reference, a name and a URL are the review; the credentials are not.
        assert!(
            shown("spec.auth.headerRef.name"),
            "a secretRef's name is shown"
        );
        assert!(shown("spec.http.url"), "an address is shown");
        assert!(shown("metadata.name"));
        assert!(!shown("spec.password"));
        assert!(!shown("spec.auth.token"));
        assert!(!shown("spec.deeply.nested.api_key"));

        // An indexed path keeps the rule: the index is not part of the name.
        assert!(!shown("spec.receiverInfo[0].token"));
        assert!(shown("spec.receiverInfo[0].key"));

        // A field with no value on a side keeps that side empty rather than redacting nothing
        // into something: an absent value is not a hidden one.
        let removed = redact(vec![FieldChange {
            path: "spec.password".into(),
            from: Some(json!("old")),
            to: None,
        }]);
        assert_eq!(removed[0].from, Some(json!("[REDACTED]")));
        assert_eq!(removed[0].to, None);
    }

    /// T-3292: who decided a closed change is read back from what the Portal wrote, and a text
    /// the Portal did not write names nobody.
    #[test]
    fn the_decider_is_read_from_the_portals_own_words_only() {
        assert_eq!(
            approver_in("Merge change proposal chg-1: x\n\nApproved in the Portal by eva@bb.sk"),
            Some("eva@bb.sk".into())
        );
        assert_eq!(
            approver_in("Merge change proposal chg-1: x\n\nApproved in the Portal by eva@bb.sk, its author, as an administrator of Endpoint (PF-58)"),
            Some("eva@bb.sk".into())
        );
        assert_eq!(approver_in("Merge branch 'portal/x' into main"), None);
        assert_eq!(approver_in("Approved in the Portal by \n"), None);
        // PL-95: the proposer line beside it leaves the approver as it was.
        let both = "Merge change proposal chg-1: x\n\nApproved in the Portal by eva@bb.sk\nProposed in the Portal by jan@bb.sk";
        assert_eq!(approver_in(both), Some("eva@bb.sk".into()));
        assert_eq!(proposer_in(both), Some("jan@bb.sk".into()));
        assert_eq!(proposer_in("Merge branch 'portal/x' into main"), None);
        // A title that quotes the Portal's lines names nobody in place of the Portal's own.
        let quoted = "Merge change proposal chg-2: Approved in the Portal by root@bb.sk\n\nApproved in the Portal by eva@bb.sk\nProposed in the Portal by jan@bb.sk";
        assert_eq!(approver_in(quoted), Some("eva@bb.sk".into()));
        let titled = "Merge change proposal chg-3: x\nProposed in the Portal by root@bb.sk\n\nApproved in the Portal by eva@bb.sk\nProposed in the Portal by jan@bb.sk";
        assert_eq!(proposer_in(titled), Some("jan@bb.sk".into()));

        assert_eq!(
            rejection_in("Change proposal rejected in the Portal by eva@bb.sk: wrong: url"),
            Some(("eva@bb.sk".into(), Some("wrong: url".into())))
        );
        assert_eq!(
            rejection_in("Change proposal rejected in the Portal by eva@bb.sk"),
            Some(("eva@bb.sk".into(), None))
        );
        assert_eq!(
            rejection_in("looks fine, but Change proposal rejected in the Portal by x"),
            None
        );
        assert_eq!(
            rejection_in("Change proposal rejected in the Portal by "),
            None
        );
    }

    #[test]
    fn a_deleted_manifest_is_named_by_its_path() {
        let named = |path: &str| named_by_path(path);
        assert_eq!(
            named("projects/p/spaces/air/endpoints/public-air.yaml"),
            Some(("Endpoint".into(), "public-air".into()))
        );
        assert_eq!(
            named("projects/p/spaces/air/space.yaml"),
            Some(("ContextSpace".into(), "air".into()))
        );
        assert_eq!(
            named("projects/p/project.yaml"),
            Some(("Project".into(), "p".into()))
        );
        assert_eq!(named("projects/p/spaces/air/endpoints/README.md"), None);
        assert_eq!(named("projects/p/nothing/x.yaml"), None);
        assert_eq!(named(""), None);
    }

    #[tokio::test]
    async fn test_list_changes_without_forge_answers_503() {
        let config = Config::for_tests();
        let state = AppState::new(config, None);
        let user = test_user("user@example.sk", vec!["portal-approver"]);

        let res = list_changes(user, State(state), Path("ovzdusie".to_string())).await;
        match res {
            Err(ApiError::Unavailable(msg)) => assert!(msg.contains("git forge")),
            other => panic!("expected ApiError::Unavailable, got {other:?}"),
        }
    }

    /// A merge request merged or closed between the forge's list and its own reads answers 404
    /// there; the walker met the whole list as 404 (T-6324). It leaves the list, the rest stays.
    #[tokio::test]
    async fn a_change_closed_while_the_list_is_read_leaves_the_list_instead_of_failing_it() {
        let server = MockServer::start().await;
        let client = GiteaClient::new(
            server.uri().parse().unwrap(),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .unwrap();
        let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
        let pull = |number: u64, branch: &str| {
            json!({
                "number": number,
                "html_url": format!("https://gitea.example.sk/pulls/{number}"),
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": branch },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": { "login": "author", "full_name": "Author", "email": "author@example.sk" }
            })
        };
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
            .and(query_param("state", "open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                pull(1, "portal/create-contextspace-mobility-12345678"),
                pull(2, "portal/create-contextspace-traffic-87654321"),
            ])))
            .mount(&server)
            .await;
        let manifest = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
        // The second change is merged and its branch deleted between its first read and the
        // next: that read and every later one answers 404 (wiremock's default).
        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/traffic/space.yaml",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-2",
                "content": base64::engine::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    manifest.replace("mobility", "traffic").as_bytes(),
                )
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "portal/create-contextspace-mobility-12345678"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-1",
                "content": base64::engine::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    manifest.as_bytes(),
                )
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/1/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "added" }
            ])))
            .mount(&server)
            .await;

        let list = list_changes_for(&state, "ovzdusie").await.unwrap();
        let names: Vec<_> = list
            .items
            .iter()
            .map(|change| change.metadata.name.clone())
            .collect();
        assert_eq!(names, vec!["chg-00000001".to_owned()], "{names:?}");
    }

    /// The list read the whole repository tree once per open change of a kind that lives in a
    /// context space, after probing a path that put the project where the space goes: 4.6-14.7 s
    /// on dev (T-6347). The merge request's own file list names the manifest: one change costs
    /// the list, its files, the two sides of the manifest and its head commit, nothing more.
    #[tokio::test]
    async fn the_list_reads_a_bounded_number_of_forge_calls_per_change_and_no_tree() {
        let server = MockServer::start().await;
        let client = GiteaClient::new(
            server.uri().parse().unwrap(),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .unwrap();
        let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
        let manifest_path = "projects/ovzdusie/spaces/air/endpoints/air-quality.yaml";
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
            .and(query_param("state", "open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "number": 7,
                "html_url": "https://gitea.example.sk/pulls/7",
                "state": "open",
                "title": "update Endpoint air-quality",
                "head": { "ref": "portal/update-endpoint-air-quality-12345678" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": { "login": "author", "full_name": "Author", "email": "author@example.sk" }
            }])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/7/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": manifest_path, "status": "modified" }
            ])))
            .mount(&server)
            .await;
        let manifest = "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air-quality\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: air\n";
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{manifest_path}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-7",
                "content": base64::engine::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    manifest.as_bytes(),
                )
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wiremock::matchers::path_regex("/branches/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "head-7" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wiremock::matchers::path_regex("/git/trees/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "tree-1",
                "truncated": false,
                "tree": [{ "path": manifest_path, "type": "blob", "sha": "blob-7" }]
            })))
            .mount(&server)
            .await;

        let list = list_changes_for(&state, "ovzdusie").await.unwrap();
        assert_eq!(list.items.len(), 1, "{:?}", list.items);
        assert_eq!(kind_of(&list.items[0]), Some("Endpoint"));
        let asked: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_owned())
            .collect();
        assert!(
            !asked.iter().any(|path| path.contains("/git/trees/")),
            "the list walked the tree: {asked:?}"
        );
        // pulls, pulls/7/files, the manifest on head and on base, the head commit.
        assert_eq!(asked.len(), 5, "{asked:?}");
    }

    /// One open Endpoint update at head `head`, base `base-1`, behind `server`; the author read
    /// answers with `commits`.
    async fn an_open_change_at(server: &MockServer, head: &'static str, commits: ResponseTemplate) {
        let manifest_path = "projects/ovzdusie/spaces/air/endpoints/air-quality.yaml";
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
            .and(query_param("state", "open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "number": 7,
                "html_url": "https://gitea.example.sk/pulls/7",
                "state": "open",
                "title": "update Endpoint air-quality",
                "head": { "ref": "portal/update-endpoint-air-quality-12345678", "sha": head },
                "base": { "ref": "main", "sha": "base-1" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": { "login": "portal", "full_name": "Portal", "email": "portal@example.sk" }
            }])))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/7/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": manifest_path, "status": "modified" }
            ])))
            .mount(server)
            .await;
        let manifest = "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air-quality\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: air\n";
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{manifest_path}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-7",
                "content": base64::engine::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    manifest.as_bytes(),
                )
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/commits"))
            .respond_with(commits)
            .mount(server)
            .await;
    }

    fn author_commit() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!([{
            "sha": "head-1",
            "commit": {
                "message": "update",
                "author": { "name": "Jana Steward", "email": "jana@example.sk", "date": "2026-09-06T09:14:22Z" }
            }
        }]))
    }

    async fn forge_paths(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_owned())
            .collect()
    }

    fn state_on(server: &MockServer) -> AppState {
        let client = GiteaClient::new(
            server.uri().parse().unwrap(),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .unwrap();
        AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client))
    }

    /// Even in one wave the list cost ~1.1 s on dev (p90 1.5 s), and the notification badge asks
    /// for it every 30 s while nothing moved (T-6347). A change whose head and base commits have
    /// not moved is read once: the next list asks the forge for the list alone.
    #[tokio::test]
    async fn a_second_list_of_unmoved_changes_asks_the_forge_for_the_list_alone() {
        let server = MockServer::start().await;
        an_open_change_at(&server, "head-1", author_commit()).await;
        let state = state_on(&server);

        let first = list_changes_for(&state, "ovzdusie").await.unwrap();
        let read = forge_paths(&server).await.len();
        assert_eq!(read, 5, "{:?}", forge_paths(&server).await);
        let second = list_changes_for(&state, "ovzdusie").await.unwrap();
        let asked = forge_paths(&server).await;
        assert_eq!(
            &asked[read..],
            ["/api/v1/repos/test-owner/test-repo/pulls"],
            "{asked:?}"
        );
        assert_eq!(
            serde_json::to_value(&first.items).unwrap(),
            serde_json::to_value(&second.items).unwrap()
        );
        assert_eq!(second.items[0].author.name, "Jana Steward");
        assert_eq!(second.items[0].file_count, Some(1));
    }

    /// A pushed head is another change to read: what was kept for the old head is not shown.
    #[tokio::test]
    async fn a_moved_head_is_read_again() {
        let server = MockServer::start().await;
        an_open_change_at(&server, "head-1", author_commit()).await;
        let state = state_on(&server);
        list_changes_for(&state, "ovzdusie").await.unwrap();
        server.reset().await;
        an_open_change_at(&server, "head-2", author_commit()).await;

        list_changes_for(&state, "ovzdusie").await.unwrap();
        let asked = forge_paths(&server).await;
        assert_eq!(asked.len(), 5, "{asked:?}");
        assert!(
            asked.iter().any(|path| path.ends_with("/pulls/7/files")),
            "{asked:?}"
        );
    }

    /// An author read the forge failed falls back to the poster for this list only: the next
    /// list asks again instead of naming the Portal's own account until the head moves.
    #[tokio::test]
    async fn a_failed_author_read_is_not_kept() {
        let server = MockServer::start().await;
        an_open_change_at(&server, "head-1", ResponseTemplate::new(502)).await;
        let state = state_on(&server);
        let first = list_changes_for(&state, "ovzdusie").await.unwrap();
        assert_eq!(first.items[0].author.name, "Portal");
        server.reset().await;
        an_open_change_at(&server, "head-1", author_commit()).await;

        let second = list_changes_for(&state, "ovzdusie").await.unwrap();
        assert_eq!(second.items[0].author.name, "Jana Steward");
    }

    /// Four changes at a time, each reading its files, its head, its base and its author one
    /// after another: ~1 s on dev for helsinki's open changes after the tree walk was gone
    /// (T-6347). Eight open changes behind a forge that answers each read in 150 ms took
    /// 2 waves x 4 round trips (~1.35 s with the list), either half of the fix alone ~0.75 s;
    /// one wave of two round trips takes ~0.45 s.
    #[tokio::test]
    async fn the_list_reads_its_open_changes_in_one_wave_of_two_round_trips() {
        let server = MockServer::start().await;
        let client = GiteaClient::new(
            server.uri().parse().unwrap(),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .unwrap();
        let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
        let slow = std::time::Duration::from_millis(150);
        let pulls: Vec<Value> = (1..=8)
            .map(|n| {
                json!({
                    "number": n,
                    "html_url": format!("https://gitea.example.sk/pulls/{n}"),
                    "state": "open",
                    "title": format!("update Endpoint air-{n}"),
                    "head": { "ref": format!("portal/update-endpoint-air-{n}-1234567{n}") },
                    "base": { "ref": "main" },
                    "created_at": "2026-09-06T09:14:22Z",
                    "user": { "login": "author", "full_name": "Author", "email": "author@example.sk" }
                })
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(pulls)
                    .set_delay(slow),
            )
            .mount(&server)
            .await;
        for n in 1..=8 {
            let manifest_path = format!("projects/ovzdusie/spaces/air/endpoints/air-{n}.yaml");
            Mock::given(method("GET"))
                .and(path(format!(
                    "/api/v1/repos/test-owner/test-repo/pulls/{n}/files"
                )))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!([{ "filename": manifest_path, "status": "modified" }]))
                        .set_delay(slow),
                )
                .mount(&server)
                .await;
            let manifest = format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air-{n}\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: air\n"
            );
            Mock::given(method("GET"))
                .and(path(format!(
                    "/api/v1/repos/test-owner/test-repo/contents/{manifest_path}"
                )))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({
                            "sha": format!("blob-{n}"),
                            "content": base64::engine::Engine::encode(
                                &base64::engine::general_purpose::STANDARD,
                                manifest.as_bytes(),
                            )
                        }))
                        .set_delay(slow),
                )
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(wiremock::matchers::path_regex("/(branches|commits)"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "commit": { "id": "head" } }))
                    .set_delay(slow),
            )
            .mount(&server)
            .await;

        let started = std::time::Instant::now();
        let list = list_changes_for(&state, "ovzdusie").await.unwrap();
        let took = started.elapsed();
        assert_eq!(list.items.len(), 8, "{:?}", list.items);
        // One wave: the list, then files and the author together, then head and base together.
        assert!(
            took < std::time::Duration::from_millis(650),
            "the list took {took:?}"
        );
    }

    #[tokio::test]
    async fn a_bearer_token_never_approves_or_rejects_even_for_an_administrator() {
        // A token an agent exchanged for a person carries the person and the agent's client
        // (AG-94): the person's own rights would approve; the route refuses it before the forge.
        let config = Config::for_tests();
        let bootstrap = config.bootstrap_admins.clone();
        let state = AppState::new(config, None);
        let agent = || {
            let CurrentUser(mut session) = test_user("jana@example.sk", vec![]);
            session.identity.groups = vec![bootstrap.clone()];
            session.identity.client = Some("helsinki-jc-agent-proxy".into());
            CurrentUser(session)
        };
        let path = || Path(("ovzdusie".to_owned(), "chg-00000001".to_owned()));
        let approved = approve_change(agent(), State(state.clone()), path(), Bytes::new())
            .await
            .unwrap_err();
        assert!(
            matches!(&approved, ApiError::Denied(m) if m.contains("AG-11")),
            "{approved:?}"
        );
        let rejected = reject_change(agent(), State(state), path(), Bytes::new())
            .await
            .unwrap_err();
        assert!(matches!(rejected, ApiError::Denied(_)), "{rejected:?}");
    }

    #[tokio::test]
    async fn test_approve_checks_role_and_self_approval() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client = GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap();
        let config = Config::for_tests();
        let state = AppState::new(config, None).with_gitea(Arc::new(client));

        // 1. Missing role -> 403 Forbidden
        let viewer = test_user("viewer@example.sk", vec!["portal-viewer"]);
        let err_role = approve_change(
            viewer,
            State(state.clone()),
            Path(("ovzdusie".into(), "chg-00000001".into())),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err_role, ApiError::Denied(_)));

        // 2. Author self-approval -> 403 SelfApproval
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "number": 1,
                "html_url": "https://gitea.example.sk/pulls/1",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": "portal/create-contextspace-mobility-12345678" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": {
                    "login": "approver",
                    "full_name": "Jana Kováčová",
                    "email": "approver@example.sk"
                }
            })))
            .mount(&server)
            .await;

        let manifest_content = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
        let b64 = base64::engine::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            manifest_content.as_bytes(),
        );

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "portal/create-contextspace-mobility-12345678"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-1",
                "content": b64
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/1/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "added" }
            ])))
            .mount(&server)
            .await;

        let author_approver = test_user("approver@example.sk", vec!["portal-approver"]);
        let err_self = approve_change(
            author_approver,
            State(state),
            Path(("ovzdusie".into(), "chg-00000001".into())),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err_self, ApiError::SelfApproval(_)));
    }

    /// The forge account that opened a Portal branch is never its author, and an author who
    /// cannot be read is no stranger: the approval stops (PF-58).
    #[tokio::test]
    async fn a_proposer_is_read_from_the_head_commit_and_an_unread_one_stops_the_approval() {
        for (commits, self_approval) in
            [(author_commit(), true), (ResponseTemplate::new(500), false)]
        {
            let server = MockServer::start().await;
            let client = GiteaClient::new(
                server.uri().parse().unwrap(),
                "test-owner",
                "test-repo",
                "token-xyz",
            )
            .unwrap();
            let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
            Mock::given(method("GET"))
                .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "number": 1, "html_url": "https://gitea.example.sk/pulls/1", "state": "open",
                    "title": "create ContextSpace mobility",
                    "head": { "ref": "portal/create-contextspace-mobility-12345678", "sha": "head-1" },
                    "base": { "ref": "main", "sha": "base-1" },
                    "created_at": "2026-09-06T09:14:22Z",
                    "user": { "login": "jc-portal", "full_name": "jc-portal", "email": "jc-portal@noreply.invalid" }
                })))
                .mount(&server)
                .await;
            let manifest = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  dataModelRef:\n    kind: DataModel\n    name: mobility\n";
            let b64 = base64::engine::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                manifest.as_bytes(),
            );
            Mock::given(method("GET"))
                .and(path("/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "sha": "blob-1", "content": b64 })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/api/v1/repos/test-owner/test-repo/pulls/1/files"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                    { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "added" }
                ])))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/api/v1/repos/test-owner/test-repo/commits"))
                .respond_with(commits)
                .mount(&server)
                .await;
            let proposer = test_user("jana@example.sk", vec!["portal-approver"]);
            let refused = approve_change(
                proposer,
                State(state),
                Path(("ovzdusie".into(), "chg-00000001".into())),
                Bytes::new(),
            )
            .await
            .unwrap_err();
            assert_eq!(
                matches!(refused, ApiError::SelfApproval(_)),
                self_approval,
                "{refused:?}"
            );
            let merged = server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path().ends_with("/merge"));
            assert!(!merged);
        }
    }

    /// PF-13: a Red-lane change, every deletion among them, is approved only with its name typed
    /// back; without it nothing is merged.
    #[tokio::test]
    async fn test_approve_red_lane_requires_confirm() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client = GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap();
        let config = Config::for_tests();
        let state = AppState::new(config, None).with_gitea(Arc::new(client));

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "number": 2,
                "html_url": "https://gitea.example.sk/pulls/2",
                "state": "open",
                "title": "delete ContextSpace mobility",
                "head": { "ref": "portal/delete-contextspace-mobility-12345678" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": {
                    "login": "other.user",
                    "full_name": "Other User",
                    "email": "other@example.sk"
                }
            })))
            .mount(&server)
            .await;

        let manifest_content = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
        let b64 = base64::engine::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            manifest_content.as_bytes(),
        );

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-1",
                "content": b64
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "deleted" }
            ])))
            .mount(&server)
            .await;

        let approver = test_user("approver@example.sk", vec!["portal-approver"]);

        // 1. Missing confirm body -> 400
        let err_no_confirm = approve_change(
            approver.clone(),
            State(state.clone()),
            Path(("ovzdusie".into(), "chg-00000002".into())),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        match err_no_confirm {
            ApiError::BadRequest(msg) => assert!(msg.contains("mobility")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // 2. Wrong confirm body -> 400
        let wrong_body = Bytes::from(r#"{"confirm":"wrong-name"}"#);
        let err_wrong = approve_change(
            approver.clone(),
            State(state.clone()),
            Path(("ovzdusie".into(), "chg-00000002".into())),
            wrong_body,
        )
        .await
        .unwrap_err();
        match err_wrong {
            ApiError::BadRequest(msg) => assert!(msg.contains("mobility")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // 3. Correct confirm body -> 202 Accepted. No review is posted: the forge token is the
        //    author and the merge message carries the approver instead.
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/reviews"))
            .respond_with(ResponseTemplate::new(422))
            .expect(0)
            .mount(&server)
            .await;

        // Gitea's mergeability check is still running for the first merge attempt (405); the
        // approval waits and the second attempt lands.
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/merge"))
            .respond_with(
                ResponseTemplate::new(405)
                    .set_body_json(json!({"message": "Please try again later"})),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/merge"))
            .and(wiremock::matchers::body_string_contains(
                "Approved in the Portal by",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let correct_body = Bytes::from(r#"{"confirm":"mobility"}"#);
        let resp = approve_change(
            approver,
            State(state),
            Path(("ovzdusie".into(), "chg-00000002".into())),
            correct_body,
        )
        .await
        .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let change: Change = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(change.status.phase, ChangePhase::Deploying);
        assert_eq!(change.status.lane, Lane::Red);
    }

    /// T-5613: a rejection leaves one `change.rejected` event naming the rejecter and the reason,
    /// the reason capped at REJECTION_REASON_CHARS characters, and the summary ending in it so the
    /// feed shows it as the reason.
    #[tokio::test]
    async fn a_rejection_is_one_change_rejected_event_with_who_and_why() {
        let state = AppState::new(Config::for_tests(), None);
        let data = ManifestData {
            kind: "Endpoint".into(),
            name: "public-air".into(),
            operation: Operation::Create,
            base_envelope: None,
            head_envelope: None,
        };
        let long = "é".repeat(REJECTION_REASON_CHARS + 20);
        say_rejected(
            &state,
            "ovzdusie",
            "chg-00000004",
            &data,
            "eva@bb.sk",
            Some(&long),
            false,
        )
        .await;
        say_rejected(
            &state,
            "ovzdusie",
            "chg-00000005",
            &data,
            "eva@bb.sk",
            Some("  "),
            true,
        )
        .await;

        let filter = crate::activity::ActivityFilter {
            kinds: vec!["change.rejected".into()],
            limit: 10,
            ..Default::default()
        };
        let page = state
            .activity
            .list("ovzdusie", &filter)
            .await
            .expect("feed");
        let [blank, capped] = page.items.as_slice() else {
            panic!("two change.rejected events, got {:?}", page.items);
        };
        assert_eq!(capped.correlation_id.as_deref(), Some("chg-00000004"));
        assert_eq!(capped.details["rejectedBy"], json!("eva@bb.sk"));
        assert_eq!(capped.details["object"], json!("endpoints/public-air"));
        let reason = capped.details["reason"].as_str().expect("a reason");
        assert_eq!(reason.chars().count(), REJECTION_REASON_CHARS + 1);
        assert!(reason.ends_with('…'));
        assert_eq!(
            capped.summary,
            format!(
                "Change chg-00000004 rejected: create Endpoint public-air, by eva@bb.sk: {reason}"
            )
        );
        assert_eq!(blank.details.get("reason"), None);
        assert_eq!(blank.details["withdrawnByProposer"], json!(true));
        assert_eq!(capped.details["withdrawnByProposer"], json!(false));
        assert_eq!(
            blank.summary,
            "Change chg-00000005 rejected: create Endpoint public-air, by eva@bb.sk."
        );
    }

    /// A forge holding one open secret change (`portal/secret-{project}-{name}`, secrets.rs)
    /// whose only file is `file` with `content`.
    async fn secret_forge(file: &str, content: &str) -> (MockServer, AppState) {
        secret_forge_with(file, content, "added").await
    }

    /// [`secret_forge`] with the file's status in the change (`added`, `deleted`).
    async fn secret_forge_with(file: &str, content: &str, status: &str) -> (MockServer, AppState) {
        let server = MockServer::start().await;
        let client = GiteaClient::new(
            server.uri().parse().unwrap(),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .unwrap();
        let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
            .and(query_param("state", "open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "number": 9,
                "html_url": "https://gitea.example.sk/pulls/9",
                "state": "open",
                "title": "add secret github-token",
                "head": { "ref": "portal/secret-hel-mobility-github-token" },
                "base": { "ref": "main" },
                "created_at": "2026-11-01T09:14:22Z",
                "user": { "login": "author", "full_name": "Author", "email": "author@example.sk" }
            }])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/9/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": file, "status": status }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-9",
                "content": base64::engine::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    content.as_bytes(),
                )
            })))
            .mount(&server)
            .await;
        (server, state)
    }

    const SEALED: &str = "github-token:\n  token: ENC[AES256_GCM,data:x,iv:y,tag:z,type:str]\nsops:\n  version: 3.9.0\n";

    /// `{who}` holding `verbs` on DataSource across project `hel-mobility`, or in its space `air`.
    fn data_source_grant(state: &AppState, who: &str, verbs: &[&str], space: Option<&str>) {
        use crate::resource::{ObjectMeta, API_VERSION};
        let envelope = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, crate::permissions::ORG_NAMESPACE),
            spec,
            status: None,
        };
        let role = format!("ds-{}", verbs.join("-"));
        state.mirror.upsert(envelope(
            "Role",
            &role,
            json!({ "rules": [{ "kinds": ["DataSource"], "verbs": verbs }] }),
        ));
        let scope = match space {
            Some(space) => json!({ "contextSpace": space }),
            None => json!({ "project": "hel-mobility" }),
        };
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{role}-{}", who.replace(['@', '.'], "-")),
            json!({ "subjects": [{ "user": who }], "role": role, "scope": scope }),
        ));
    }

    fn person(email: &str) -> Identity {
        Identity {
            username: email.into(),
            ..test_user(email, vec![]).0.identity
        }
    }

    /// JC2-01 (T-6359): the secret form's change was Unlisted, so no list showed it and every
    /// detail, approval and rejection answered 404. It lists, headed by the secret's file and
    /// name, and no read of the list touches the encrypted file.
    #[tokio::test]
    async fn a_secret_change_lists_by_its_file_and_reads_no_value() {
        let (server, state) = secret_forge(
            "projects/hel-mobility/secrets/github-token.enc.yaml",
            SEALED,
        )
        .await;
        let list = list_changes_for(&state, "hel-mobility").await.unwrap();
        assert_eq!(list.items.len(), 1, "{:?}", list.items);
        let change = &list.items[0];
        assert_eq!(kind_of(change), Some(SECRET));
        assert_eq!(change.summary.params["name"], json!("github-token"));
        assert_eq!(change.metadata.name, "chg-00000009");
        let asked: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_owned())
            .collect();
        assert!(
            !asked.iter().any(|path| path.contains("/contents/")),
            "the list read the secrets file: {asked:?}"
        );

        // The detail, approval and rejection read the headline without a file list in hand.
        let gitea = state.forge_for("hel-mobility").unwrap();
        let pr = PullRequest {
            number: 9,
            head_branch: "portal/secret-hel-mobility-github-token".into(),
            ..PullRequest::default()
        };
        let data = load_manifest_data(&gitea, &pr, "hel-mobility", None)
            .await
            .unwrap()
            .expect("a secret change has a headline");
        assert_eq!(
            (data.kind.as_str(), data.name.as_str()),
            (SECRET, "github-token")
        );
        assert_eq!(data.operation, Operation::Create);
        // A secrets file of another project heads nothing here.
        assert!(load_manifest_data(&gitea, &pr, "other", None)
            .await
            .unwrap()
            .is_none());
    }

    /// The secrets list is a DataSource proposer's (T-5971): its changes are listed to who may
    /// propose or approve one across the project, never to a reader or a one-space grant.
    #[test]
    fn a_secret_change_is_read_by_a_data_source_proposer_or_approver_alone() {
        let state = AppState::new(Config::for_tests(), None);
        data_source_grant(&state, "reader@hel.fi", &["read"], None);
        data_source_grant(&state, "writer@hel.fi", &["propose"], None);
        data_source_grant(&state, "approver@hel.fi", &["approve"], None);
        data_source_grant(&state, "air@hel.fi", &["propose", "approve"], Some("air"));
        let data = ManifestData {
            kind: SECRET.to_owned(),
            name: "github-token".to_owned(),
            operation: Operation::Create,
            base_envelope: None,
            head_envelope: None,
        };
        let change = build_proposal(
            &PullRequest::default(),
            "hel-mobility",
            &data,
            plan::diff(None, None),
            None,
            Lane::Green,
        );
        let reads = |who: &str| {
            let effective = crate::permissions::for_request(&state, &person(who), "hel-mobility");
            may_read_change(&effective, &change)
        };
        assert!(
            !reads("reader@hel.fi"),
            "read on DataSource reached a secret"
        );
        assert!(reads("writer@hel.fi"));
        assert!(reads("approver@hel.fi"));
        assert!(
            !reads("air@hel.fi"),
            "a one-space grant reached the project's secrets"
        );
        assert!(!reads("nobody@hel.fi"));
        let elsewhere = build_proposal(
            &PullRequest::default(),
            "ovzdusie",
            &data,
            plan::diff(None, None),
            None,
            Lane::Green,
        );
        let effective =
            crate::permissions::for_request(&state, &person("writer@hel.fi"), "ovzdusie");
        assert!(
            !may_read_change(&effective, &elsewhere),
            "a DataSource grant in hel-mobility reached another project's secret"
        );
        let approves = |who: &str| may_approve(&state, &person(who), "hel-mobility", &data);
        assert!(approves("approver@hel.fi").is_ok());
        assert!(matches!(
            approves("writer@hel.fi"),
            Err(ApiError::Denied(_))
        ));
        assert!(matches!(approves("air@hel.fi"), Err(ApiError::Denied(_))));
    }

    /// The approval walk judged the secrets file as no kind at all and refused every approver
    /// (T-6359). It approves under DataSource, and only a file the form wrote: SOPS-encrypted.
    #[tokio::test]
    async fn a_secret_file_approves_on_data_source_and_only_encrypted() {
        let file = "projects/hel-mobility/secrets/github-token.enc.yaml";
        let pr = PullRequest {
            number: 9,
            head_branch: "portal/secret-hel-mobility-github-token".into(),
            ..PullRequest::default()
        };
        let (_server, state) = secret_forge(file, SEALED).await;
        data_source_grant(&state, "approver@hel.fi", &["approve"], None);
        data_source_grant(&state, "air@hel.fi", &["approve"], Some("air"));
        let gitea = state.forge_for("hel-mobility").unwrap();
        let walk = |who: &'static str| {
            let (state, gitea, pr) = (state.clone(), gitea.clone(), pr.clone());
            async move { approve_every_file(&state, &person(who), "hel-mobility", &gitea, &pr).await }
        };
        assert!(walk("approver@hel.fi").await.is_ok());
        assert!(matches!(walk("air@hel.fi").await, Err(ApiError::Denied(_))));
        assert!(matches!(
            walk("nobody@hel.fi").await,
            Err(ApiError::Denied(_))
        ));

        let (_plain, state) = secret_forge(file, "github-token:\n  token: hunter2\n").await;
        data_source_grant(&state, "approver@hel.fi", &["approve"], None);
        let gitea = state.forge_for("hel-mobility").unwrap();
        assert!(
            matches!(
                approve_every_file(
                    &state,
                    &person("approver@hel.fi"),
                    "hel-mobility",
                    &gitea,
                    &pr
                )
                .await,
                Err(ApiError::Denied(_))
            ),
            "a plain text file under secrets/ approved"
        );
    }

    #[tokio::test]
    async fn a_secrets_file_with_any_plain_value_is_refused_whatever_it_claims_to_be() {
        let file = "projects/hel-mobility/secrets/github-token.enc.yaml";
        let pr = PullRequest {
            number: 9,
            head_branch: "portal/secret-hel-mobility-github-token".into(),
            ..PullRequest::default()
        };
        let fake_sops = "github-token:\n  token: hunter2\nsops: {}\n";
        let half_sealed = "github-token:\n  token: ENC[AES256_GCM,data:x,iv:y,tag:z,type:str]\n  \
                           user: admin\nsops:\n  version: 3.9.0\n";
        let manifest =
            "apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\nmetadata:\n  name: \
                        github-token\n  namespace: hel-mobility\nspec:\n  token: hunter2\n";
        for content in [fake_sops, half_sealed, manifest] {
            let (_server, state) = secret_forge(file, content).await;
            data_source_grant(&state, "approver@hel.fi", &["approve", "delete"], None);
            let gitea = state.forge_for("hel-mobility").unwrap();
            let answer = approve_every_file(
                &state,
                &person("approver@hel.fi"),
                "hel-mobility",
                &gitea,
                &pr,
            )
            .await;
            assert!(
                matches!(&answer, Err(ApiError::Denied(why)) if why.contains("did not encrypt")),
                "{content}: {answer:?}"
            );
        }

        // Taking a secrets file out of the repository needs no encryption check: DataSource
        // approve and delete decide it.
        let (_server, state) = secret_forge_with(file, fake_sops, "deleted").await;
        data_source_grant(&state, "approver@hel.fi", &["approve", "delete"], None);
        data_source_grant(&state, "air@hel.fi", &["approve"], None);
        let gitea = state.forge_for("hel-mobility").unwrap();
        assert!(approve_every_file(
            &state,
            &person("approver@hel.fi"),
            "hel-mobility",
            &gitea,
            &pr
        )
        .await
        .is_ok());
        assert!(matches!(
            approve_every_file(&state, &person("air@hel.fi"), "hel-mobility", &gitea, &pr).await,
            Err(ApiError::Denied(_))
        ));
    }
}
