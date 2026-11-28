#![forbid(unsafe_code)]
//! The forge's operations, one implementation each, called by every surface that offers them:
//! the Gitea-compatible REST (`compat.rs`), the OData service (`odata.rs`) and through it MCP and
//! the CLI (Contract S8: one source). Each function resolves the repository and checks the verb
//! itself, so no surface can reach an operation around its authorization: a repository that is
//! missing, in another organization or unreadable is `NotFound` (404) on every surface.
//!
//! Inputs are plain values; each surface parses its own wire shape and renders the records
//! these functions answer.

use crate::outbox::{
    build_pr_webhook_payload, build_push_webhook_payload, deliver_webhook_now, dispatch_logged,
    emit_and_dispatch_event, record_event,
};
use crate::state::{ShakoState, resolve_and_authorize_repo};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use jc_time::TimestampTz;
use shako_core::{
    Action, AuditRecord, BranchProtectionRule, CommitStatusRecord, DeliveryRecord,
    MergePolicyInput, ObjectTarget, PackageFileRecord, PackageId, PackageRecord,
    PackageVersionRecord, ProtectionRuleRecord, PullRequestRecord, PushMirrorRecord, RefPolicy,
    RefUpdate, RepoId, RepoRecord, RequestCtx, ReviewRecord, ReviewVerdict, ShakoError,
    ShakoResult, WebhookFormat, WebhookId, WebhookRecord, check_repo_count_quota,
    remaining_storage_allowance, sha256_hex, validate_branch_name, validate_package_segment,
    validate_repo_or_owner_name, validate_tree_path,
};
use shako_git::{
    CommitAuthor, DEFAULT_PRUNE_GRACE, DiffFileEntry, FileChangeOp, FileOpKind, MergeOptions,
    MergeStyle, check_mergeability, commit_file_changes, compact_repository, compaction_due,
    compute_repo_size_bytes, copy_bare_repo, create_branch as git_create_branch,
    delete_branch as git_delete_branch, diff_refs_files, execute_pr_merge, init_bare_repo,
    open_isolated, set_ref_direct, update_pr_branch,
};
use std::path::PathBuf;

/// What a merge of two refs holds at its largest.
const MERGE_OPERATION_BYTES: u64 = 8 * 1024 * 1024;

/// Runs `f` on the blocking pool; a task that panicked is an internal error.
pub async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ShakoResult<T> + Send + 'static,
) -> ShakoResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(task_failed)?
}

/// Runs `f` as a task of its own, to its end even when the caller stops waiting; a task that
/// panicked is an internal error.
pub(crate) async fn spawned<T: Send + 'static>(
    f: impl std::future::Future<Output = T> + Send + 'static,
) -> ShakoResult<T> {
    tokio::spawn(f).await.map_err(task_failed)
}

fn task_failed(e: tokio::task::JoinError) -> ShakoError {
    ShakoError::Internal(e.to_string())
}

/// Writes without an operation of their own in the registry, with their audit kind.
const UNLISTED_WRITES: &[(&str, &str)] = &[
    ("PatchRepository", "REPO_UPDATE"),
    ("PatchPullRequest", "PR_UPDATE"),
    ("SetCommitStatus", "STATUS_SET"),
];

/// The longest `target` and `request_id` an audit row keeps.
const AUDIT_TEXT_MAX: usize = 128;

fn clipped(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Writes the audit row of a forge write (S6-20) through the store, inside the request's unit
/// (`jc_audit::layer` on the doors), so the change stands with its row or not at all: an error
/// here fails the write. `operation` is the registry's name (`CreateRepository`), whose
/// `audit_kind` the row carries; `target` names what was written by ids.
pub async fn audit(
    state: &ShakoState,
    ctx: &RequestCtx,
    operation: &str,
    target: String,
) -> ShakoResult<()> {
    let kind = state
        .registry
        .find(operation)
        .filter(|op| op.name == operation)
        .map(|op| op.audit_kind)
        .or_else(|| {
            UNLISTED_WRITES
                .iter()
                .find(|(name, _)| *name == operation)
                .map(|(_, kind)| *kind)
        })
        .ok_or_else(|| ShakoError::Internal(format!("no audit kind for '{operation}'")))?;
    state
        .store()
        .append_audit(AuditRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: ctx.instance_id,
            org_id: ctx.org_id,
            kind: kind.to_owned(),
            operation: operation.to_owned(),
            actor_id: clipped(ctx.subject.login(), 256),
            target: clipped(&target, 4 * AUDIT_TEXT_MAX),
            decision: "allow".into(),
            request_id: clipped(&ctx.request_id, AUDIT_TEXT_MAX),
            created_at: TimestampTz::now(),
        })
        .await
}

/// The person a commit is attributed to when the caller names nobody: the token's subject.
#[must_use]
pub fn caller_as_author(ctx: &RequestCtx) -> CommitAuthor {
    CommitAuthor {
        name: ctx.subject.full_name.clone(),
        email: ctx
            .subject
            .email
            .clone()
            .unwrap_or_else(|| format!("{}@local", ctx.subject.login())),
    }
}

// =========================================================================
// Repositories
// =========================================================================

/// A repository to create in an organization.
#[derive(Debug, Clone)]
pub struct NewRepository {
    pub owner: String,
    pub name: String,
    pub description: String,
    pub private: bool,
    pub auto_init: bool,
    pub default_branch: String,
}

fn check_org_write(state: &ShakoState, ctx: &RequestCtx, owner: &str) -> ShakoResult<()> {
    state
        .authorizer
        .check(
            ctx,
            Action::Write,
            &ObjectTarget::Organization {
                instance_id: ctx.instance_id,
                org_id: ctx.org_id,
                owner: owner.to_owned(),
            },
        )
        .into_result("organization not found")
}

async fn refuse_existing(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
) -> ShakoResult<()> {
    if state
        .store()
        .get_repo_by_name(ctx.instance_id, ctx.org_id, owner, name)
        .await?
        .is_some()
    {
        return Err(ShakoError::Conflict("repository already exists".into()));
    }
    Ok(())
}

async fn record_new_repo(
    state: &ShakoState,
    ctx: &RequestCtx,
    operation: &str,
    rec: RepoRecord,
    disk_path: &std::path::Path,
) -> ShakoResult<RepoRecord> {
    let size = rec.size_bytes;
    let target = format!("repos/{}", rec.id);
    let created = match state.store().create_repo(rec).await {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(disk_path);
            return Err(e);
        }
    };
    // A repository whose creation could not be audited is not kept: the record goes with its
    // directory, so no unaudited repository answers later.
    if let Err(e) = audit(state, ctx, operation, target).await {
        let _ = state
            .store()
            .delete_repo(created.instance_id, created.org_id, created.id)
            .await;
        let _ = std::fs::remove_dir_all(disk_path);
        return Err(e);
    }
    let _ = state
        .store()
        .adjust_quota_usage(ctx.instance_id, ctx.org_id, 1, size as i64, 0)
        .await;
    Ok(created)
}

/// After a write, under the repository lock: compacts `repo` once it holds enough packs, then
/// measures it on disk again and moves its Organization's git usage by the change, so the
/// storage quota sees what a push, an API commit or a merge added.
pub async fn settle_after_write(
    state: &ShakoState,
    repo: &mut RepoRecord,
    disk_path: &std::path::Path,
) {
    // Each commit, merge and push adds a pack; once there are enough, they become one (S6-42).
    // A compaction that fails leaves the write standing, and the next write tries again.
    let path = disk_path.to_path_buf();
    let compacted = state
        .git_work
        .run(repo.org_id, move |admit| {
            if !compaction_due(&path) {
                return Ok(());
            }
            compact_repository(&path, DEFAULT_PRUNE_GRACE, admit).map(drop)
        })
        .await;
    if let Err(e) = compacted {
        tracing::warn!(repo = %repo.id, error = %e, "repository not compacted after a write");
    }
    let path = disk_path.to_path_buf();
    let Ok(new_size) = blocking(move || Ok(compute_repo_size_bytes(&path))).await else {
        return;
    };
    // `repo` was read before the write; a pull request opened or a rename or archive made
    // since then lives only in the stored record, so only its size changes here.
    let Ok(Some(mut current)) = state
        .store()
        .get_repo_by_id(repo.instance_id, repo.org_id, repo.id)
        .await
    else {
        return;
    };
    let delta = new_size as i64 - current.size_bytes as i64;
    current.size_bytes = new_size;
    if let Ok(saved) = state.store().update_repo(current).await {
        *repo = saved;
    }
    if let Err(e) = state
        .store()
        .adjust_quota_usage(repo.instance_id, repo.org_id, 0, delta, 0)
        .await
    {
        tracing::warn!(repo = %repo.id, error = %e, "storage quota not adjusted after a write");
    }
}

/// Creates a repository; one of the same owner and name already there is `Conflict` (409).
pub async fn create_repository(
    state: &ShakoState,
    ctx: &RequestCtx,
    req: NewRepository,
) -> ShakoResult<RepoRecord> {
    validate_repo_or_owner_name(&req.owner, "owner")?;
    validate_repo_or_owner_name(&req.name, "repository")?;
    validate_branch_name(&req.default_branch)?;
    check_org_write(state, ctx, &req.owner)?;
    let instance = state.instance_for(ctx.instance_id, ctx.org_id).await?;
    let usage = state
        .store()
        .get_quota_usage(ctx.instance_id, ctx.org_id)
        .await?;
    check_repo_count_quota(&instance, &usage)?;
    refuse_existing(state, ctx, &req.owner, &req.name).await?;

    let repo_id = RepoId::new();
    let root = state.repo_root().to_path_buf();
    let inst_id = ctx.instance_id;
    let (def_branch, auto_init, desc) = (
        req.default_branch.clone(),
        req.auto_init,
        req.description.clone(),
    );
    let disk_path =
        blocking(move || init_bare_repo(&root, inst_id, repo_id, &def_branch, auto_init, &desc))
            .await?;
    let now = TimestampTz::now();
    let rec = RepoRecord {
        id: repo_id,
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
        project_id: req.name.clone(),
        owner: req.owner,
        name: req.name,
        description: req.description,
        default_branch: req.default_branch,
        private: req.private,
        archived: false,
        size_bytes: compute_repo_size_bytes(&disk_path),
        next_pr_number: 1,
        created_at: now,
        updated_at: now,
    };
    record_new_repo(state, ctx, "CreateRepository", rec, &disk_path).await
}

/// A copy of a repository of this forge, with its whole history, under a new owner and name.
#[derive(Debug, Clone)]
pub struct RepositoryCopy {
    pub source_owner: String,
    pub source_name: String,
    pub owner: String,
    pub name: String,
    pub description: Option<String>,
    pub private: bool,
}

/// Copies a repository the caller reads into one it may create; an existing target is 409.
pub async fn import_repository(
    state: &ShakoState,
    ctx: &RequestCtx,
    req: RepositoryCopy,
) -> ShakoResult<RepoRecord> {
    validate_repo_or_owner_name(&req.owner, "repo_owner")?;
    validate_repo_or_owner_name(&req.name, "repo_name")?;
    let (origin, origin_disk) = resolve_and_authorize_repo(
        state,
        ctx,
        &req.source_owner,
        &req.source_name,
        Action::Read,
    )
    .await?;
    check_org_write(state, ctx, &req.owner)?;
    let instance = state.instance_for(ctx.instance_id, ctx.org_id).await?;
    let usage = state
        .store()
        .get_quota_usage(ctx.instance_id, ctx.org_id)
        .await?;
    check_repo_count_quota(&instance, &usage)?;
    if usage.total_bytes().saturating_add(origin.size_bytes) > instance.max_storage_bytes {
        return Err(ShakoError::QuotaExceeded {
            resource: "storage_bytes".into(),
            limit: instance.max_storage_bytes,
        });
    }
    refuse_existing(state, ctx, &req.owner, &req.name).await?;

    let new_id = RepoId::new();
    let root = state.repo_root().to_path_buf();
    let inst_id = ctx.instance_id;
    let def_branch = origin.default_branch.clone();
    let desc = req.description.clone().unwrap_or_default();
    // A push or compaction on the source must not add or drop packs while they are copied.
    let _source_guard = state.repo_locks.lock(origin.id).await;
    let dest = blocking(move || {
        let dest = init_bare_repo(&root, inst_id, new_id, &def_branch, false, &desc)?;
        copy_bare_repo(&origin_disk, &dest, &def_branch)?;
        Ok(dest)
    })
    .await?;
    let now = TimestampTz::now();
    let rec = RepoRecord {
        id: new_id,
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
        project_id: req.name.clone(),
        owner: req.owner,
        name: req.name,
        description: req.description.unwrap_or_default(),
        default_branch: origin.default_branch,
        private: req.private,
        archived: false,
        size_bytes: compute_repo_size_bytes(&dest),
        next_pr_number: 1,
        created_at: now,
        updated_at: now,
    };
    record_new_repo(state, ctx, "ImportRepository", rec, &dest).await
}

/// What a repository update may change; `None` keeps the field.
#[derive(Debug, Clone, Default)]
pub struct RepositoryPatch {
    pub archived: Option<bool>,
    pub description: Option<String>,
    pub default_branch: Option<String>,
    pub private: Option<bool>,
    pub name: Option<String>,
}

/// Changes a repository's settings (Admin).
pub async fn patch_repository(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    patch: RepositoryPatch,
) -> ShakoResult<RepoRecord> {
    let (mut repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    // Archive is the registry's operation; any other change is a settings update.
    let operation = if patch.archived == Some(true)
        && patch.description.is_none()
        && patch.default_branch.is_none()
        && patch.private.is_none()
        && patch.name.is_none()
    {
        "Archive"
    } else {
        "PatchRepository"
    };
    audit(state, ctx, operation, format!("repos/{}", repo.id)).await?;
    if let Some(a) = patch.archived {
        repo.archived = a;
    }
    if let Some(d) = patch.description {
        repo.description = d;
    }
    let new_head = patch.default_branch.filter(|b| *b != repo.default_branch);
    if let Some(b) = &new_head {
        validate_branch_name(b)?;
        repo.default_branch.clone_from(b);
    }
    if let Some(p) = patch.private {
        repo.private = p;
    }
    if let Some(n) = patch.name.filter(|n| *n != repo.name) {
        validate_repo_or_owner_name(&n, "repository")?;
        if state
            .store()
            .get_repo_by_name(repo.instance_id, repo.org_id, &repo.owner, &n)
            .await?
            .is_some()
        {
            return Err(ShakoError::Conflict(format!(
                "repository '{}/{n}' already exists",
                repo.owner
            )));
        }
        repo.name = n;
    }
    // A clone checks out what HEAD names: it follows the default branch.
    if let Some(b) = new_head {
        std::fs::write(disk_path.join("HEAD"), format!("ref: refs/heads/{b}\n"))
            .map_err(|e| ShakoError::Internal(format!("write HEAD: {e}")))?;
    }
    state.store().update_repo(repo).await
}

/// Deletes a repository and its objects (Admin).
pub async fn delete_repository(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
) -> ShakoResult<()> {
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    let _guard = state.repo_locks.lock(repo.id).await;
    state
        .store()
        .delete_repo(repo.instance_id, repo.org_id, repo.id)
        .await?;
    audit(state, ctx, "Delete", format!("repos/{}", repo.id)).await?;
    // The objects go only once the deletion stands: a rolled-back request keeps the repository.
    jc_audit::after_commit(async move {
        let _ = tokio::fs::remove_dir_all(&disk_path).await;
    })
    .await;
    let _ = state
        .store()
        .adjust_quota_usage(
            repo.instance_id,
            repo.org_id,
            -1,
            -(repo.size_bytes as i64),
            0,
        )
        .await;
    Ok(())
}

// =========================================================================
// Branches and protection
// =========================================================================

/// The commit a branch points at and whether a protection rule covers it.
pub async fn branch_head(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    branch: &str,
) -> ShakoResult<(String, bool)> {
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await?;
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await?;
    let protected = policy.matching_branch_rule(branch).is_some();
    let full_ref = format!("refs/heads/{branch}");
    let sha = blocking(move || {
        let gix_repo = open_isolated(&disk_path)?;
        let partial: &gix_ref::PartialNameRef = full_ref
            .as_str()
            .try_into()
            .map_err(|_| ShakoError::NotFound("branch not found".into()))?;
        let mut r = gix_repo
            .try_find_reference(partial)
            .map_err(|e| ShakoError::Git(e.to_string()))?
            .ok_or_else(|| ShakoError::NotFound("branch not found".into()))?;
        let oid = r
            .peel_to_id()
            .map_err(|_| ShakoError::NotFound("branch not found".into()))?
            .detach();
        Ok(oid.to_hex().to_string())
    })
    .await?;
    Ok((sha, protected))
}

/// Cuts `new_branch` from `from` (the default branch when `None`); answers its head.
pub async fn create_branch(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    new_branch: &str,
    from: Option<String>,
) -> ShakoResult<String> {
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Propose).await?;
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await?;
    policy.check_ref_update(
        ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        RefUpdate {
            refname: &format!("refs/heads/{new_branch}"),
            is_delete: false,
            is_create: true,
            is_fast_forward: true,
            changed_paths: &[],
        },
    )?;
    // The row first: a ref on disk does not roll back, a row does.
    audit(
        state,
        ctx,
        "CreateBranch",
        format!("repos/{}/branches/{new_branch}", repo.id),
    )
    .await?;
    let _guard = state.repo_locks.lock(repo.id).await;
    let from_ref = from.unwrap_or_else(|| repo.default_branch.clone());
    let new_b = new_branch.to_owned();
    blocking(move || git_create_branch(&disk_path, &new_b, &from_ref)).await
}

/// Deletes a branch the protection rules allow deleting; a branch that is not there is
/// `NotFound`.
pub async fn delete_branch(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    branch: &str,
) -> ShakoResult<()> {
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Propose).await?;
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await?;
    check_branch_delete(state, ctx, &repo, &policy, branch)?;
    audit(
        state,
        ctx,
        "DeleteBranch",
        format!("repos/{}/branches/{branch}", repo.id),
    )
    .await?;
    let _guard = state.repo_locks.lock(repo.id).await;
    let b = branch.to_owned();
    if blocking(move || git_delete_branch(&disk_path, &b)).await? {
        Ok(())
    } else {
        Err(ShakoError::NotFound("branch does not exist".into()))
    }
}

/// The one decision on deleting a branch, for every path that deletes one: the default branch
/// is never deleted, and the branch rules decide the rest.
fn check_branch_delete(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo: &RepoRecord,
    policy: &RefPolicy,
    branch: &str,
) -> ShakoResult<()> {
    if branch == repo.default_branch {
        return Err(ShakoError::Forbidden(format!(
            "branch '{branch}' is the repository's default branch and cannot be deleted"
        )));
    }
    policy.check_ref_update(
        ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        RefUpdate {
            refname: &format!("refs/heads/{branch}"),
            is_delete: true,
            is_create: false,
            is_fast_forward: true,
            changed_paths: &[],
        },
    )
}

/// Creates or replaces a branch protection rule (Admin).
pub async fn protect_branch(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    rule: BranchProtectionRule,
) -> ShakoResult<ProtectionRuleRecord> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    if let Some(stored) = state
        .store()
        .get_protection_rule(repo.instance_id, repo.org_id, repo.id, &rule.rule_name)
        .await?
        && loosens(&stored.rule, &rule)
    {
        return Err(ShakoError::Forbidden(format!(
            "branch rule '{}' guards who writes the branch and is not loosened through the API",
            rule.rule_name
        )));
    }
    RefPolicy::validate_rule_for_repo(
        &rule,
        state.authorizer.as_ref(),
        repo.instance_id,
        repo.org_id,
        &repo.owner,
        &repo.name,
    )?;
    let now = TimestampTz::now();
    let target = format!("repos/{}/protections/{}", repo.id, rule.rule_name);
    let saved = state
        .store()
        .upsert_protection_rule(ProtectionRuleRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: repo.instance_id,
            org_id: repo.org_id,
            repo_id: repo.id,
            rule,
            created_at: now,
            updated_at: now,
        })
        .await?;
    audit(state, ctx, "Protect", target).await?;
    Ok(saved)
}

/// Changes the fields `patch` names on a stored branch rule and keeps every other field as
/// stored (PATCH); the rule is saved through [`protect_branch`], so its checks and the
/// loosening guard apply.
pub async fn patch_branch_protection(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    rule_name: &str,
    patch: serde_json::Map<String, serde_json::Value>,
) -> ShakoResult<ProtectionRuleRecord> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    let stored = state
        .store()
        .get_protection_rule(repo.instance_id, repo.org_id, repo.id, rule_name)
        .await?
        .ok_or_else(|| ShakoError::NotFound("branch protection rule not found".into()))?;
    if patch
        .get("rule_name")
        .is_some_and(|v| v.as_str() != Some(rule_name))
    {
        return Err(ShakoError::Unprocessable(
            "rule_name cannot change in a PATCH; create the new rule and delete this one".into(),
        ));
    }
    // The stored rule is a struct, so its JSON is an object the patch's fields replace in.
    let merged = serde_json::to_value(&stored.rule).map(|mut rule| {
        let fields = rule.as_object_mut().into_iter().zip([patch]);
        fields.for_each(|(fields, patch)| fields.extend(patch));
        rule
    });
    let rule: BranchProtectionRule = merged
        .and_then(serde_json::from_value)
        .map_err(|e| ShakoError::Unprocessable(format!("invalid branch rule: {e}")))?;
    protect_branch(state, ctx, owner, name, rule).await
}

/// Removes a branch protection rule by its name (Admin); answers whether one was there.
pub async fn unprotect_branch(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    rule_name: &str,
) -> ShakoResult<bool> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    if let Some(stored) = state
        .store()
        .get_protection_rule(repo.instance_id, repo.org_id, repo.id, rule_name)
        .await?
        && guards_writers(&stored.rule)
    {
        return Err(ShakoError::Forbidden(format!(
            "branch rule '{rule_name}' guards who writes the branch and is not removed through the API"
        )));
    }
    let deleted = state
        .store()
        .delete_protection_rule(repo.instance_id, repo.org_id, repo.id, rule_name)
        .await?;
    if deleted {
        audit(
            state,
            ctx,
            "Unprotect",
            format!("repos/{}/protections/{rule_name}", repo.id),
        )
        .await?;
    }
    Ok(deleted)
}

/// A rule that keeps some writer off the branch: push closed, or push or merge narrowed to an
/// allow-list. Neither an admin token nor the Portal's removes one through the API (P-24): a bug
/// or a leaked token must not reopen `main` or a Change branch. Removing the repository removes
/// its rules.
fn guards_writers(rule: &BranchProtectionRule) -> bool {
    !rule.enable_push || rule.enable_push_whitelist || rule.enable_merge_whitelist
}

/// Whether `next` lets someone write or merge whom `stored` kept out, or merges with fewer
/// checks (P-24): a guard dropped, a name added to an allow-list, fewer approvals, a status
/// check or protected path removed. Removing names and adding checks is allowed.
fn loosens(stored: &BranchProtectionRule, next: &BranchProtectionRule) -> bool {
    let adds = |was: &[String], now: &[String]| now.iter().any(|n| !was.contains(n));
    let drops = |was: bool, now: bool| was && !now;
    // A rule that guards no writer can be removed and saved again, so only a guarding rule
    // holds its checks here.
    let weakens_guarding_rule = guards_writers(stored)
        && (adds(
            &stored.push_whitelist_usernames,
            &next.push_whitelist_usernames,
        ) || adds(
            &stored.force_push_allowlist_usernames,
            &next.force_push_allowlist_usernames,
        ) || adds(
            &stored.merge_whitelist_usernames,
            &next.merge_whitelist_usernames,
        ) || adds(
            &stored.approvals_whitelist_username,
            &next.approvals_whitelist_username,
        ) || next.required_approvals < stored.required_approvals
            || drops(stored.enable_status_check, next.enable_status_check)
            || adds(&next.status_check_contexts, &stored.status_check_contexts)
            || drops(
                stored.block_on_rejected_reviews,
                next.block_on_rejected_reviews,
            )
            || drops(
                stored.block_on_outdated_branch,
                next.block_on_outdated_branch,
            )
            || drops(stored.dismiss_stale_approvals, next.dismiss_stale_approvals)
            || (!stored.protected_file_patterns.trim().is_empty()
                && next.protected_file_patterns != stored.protected_file_patterns)
            || (!next.unprotected_file_patterns.trim().is_empty()
                && next.unprotected_file_patterns != stored.unprotected_file_patterns));
    weakens_guarding_rule || drops_guard(stored, next)
}

/// Whether `next` switches off a guard `stored` had (the flags of the rule).
fn drops_guard(stored: &BranchProtectionRule, next: &BranchProtectionRule) -> bool {
    let opens = |was_on: bool, was_listed: bool, on: bool, listed: bool| {
        (!was_on && on) || (was_listed && on && !listed)
    };
    opens(
        stored.enable_push,
        stored.enable_push_whitelist,
        next.enable_push,
        next.enable_push_whitelist,
    ) || opens(
        stored.enable_force_push,
        stored.enable_force_push_allowlist,
        next.enable_force_push,
        next.enable_force_push_allowlist,
    ) || (stored.enable_merge_whitelist && !next.enable_merge_whitelist)
        || (stored.block_admin_merge_override && !next.block_admin_merge_override)
}

// =========================================================================
// Contents
// =========================================================================

/// One file of a commit as a caller sends it: `content` is base64 (whitespace allowed).
#[derive(Debug, Clone)]
pub struct FileChange {
    pub operation: FileOpKind,
    pub path: String,
    pub content: Option<String>,
    pub sha: Option<String>,
}

/// One commit of several file changes to a branch.
#[derive(Debug, Clone)]
pub struct CommitRequest {
    /// The branch the commit goes on; the default branch when `None`.
    pub branch: Option<String>,
    pub message: String,
    /// Who the commit is attributed to; the caller when `None`.
    pub author: Option<CommitAuthor>,
    /// Who committed it; the author when `None`.
    pub committer: Option<CommitAuthor>,
    pub files: Vec<FileChange>,
}

/// Writes every file change as one commit, all or nothing: one stale blob `sha` is `Conflict`
/// and the branch does not move. Answers the commit; emits the `push` event (webhooks) and runs
/// the repository's push mirrors.
pub async fn commit_files(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    req: CommitRequest,
) -> ShakoResult<String> {
    let (mut repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Propose).await?;
    if req.files.is_empty() {
        return Err(ShakoError::Unprocessable(
            "a commit changes at least one file".into(),
        ));
    }
    let branch = req
        .branch
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| repo.default_branch.clone());
    let mut ops = Vec::with_capacity(req.files.len());
    let mut changed_paths = Vec::with_capacity(req.files.len());
    for item in req.files {
        let clean = validate_tree_path(&item.path)?;
        let content = match item.content {
            Some(b64) => {
                let stripped: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
                Some(STANDARD.decode(stripped.as_bytes()).map_err(|e| {
                    ShakoError::Unprocessable(format!("invalid base64 for '{clean}': {e}"))
                })?)
            }
            None => None,
        };
        // The same scan a push passes (SEC-06): an API or MCP commit is no way around it.
        if let Some(reason) = content
            .as_deref()
            .and_then(|bytes| shako_git::scan_blob_for_secrets(&clean, bytes))
        {
            return Err(ShakoError::Unprocessable(reason));
        }
        changed_paths.push(clean.clone());
        ops.push(FileChangeOp {
            operation: item.operation,
            path: clean,
            content,
            sha: item.sha.filter(|s| !s.is_empty()),
        });
    }
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await?;
    policy.check_ref_update(
        ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        RefUpdate {
            refname: &format!("refs/heads/{branch}"),
            is_delete: false,
            is_create: false,
            is_fast_forward: true,
            changed_paths: &changed_paths,
        },
    )?;
    let author = req.author.unwrap_or_else(|| caller_as_author(ctx));
    let committer = req.committer.unwrap_or_else(|| author.clone());

    audit(
        state,
        ctx,
        "CommitFiles",
        format!("repos/{}/branches/{branch}", repo.id),
    )
    .await?;
    let guard = state.repo_locks.lock(repo.id).await;
    let (disk, b, msg) = (disk_path.clone(), branch.clone(), req.message);
    // Only the default branch may start without history (the first commit of an empty
    // repository); any other branch is cut first, so a commit never makes an orphan branch.
    let may_start = branch == repo.default_branch;
    let commit_sha = blocking(move || {
        if !may_start {
            let full_ref = format!("refs/heads/{b}");
            let partial: &gix_ref::PartialNameRef = full_ref
                .as_str()
                .try_into()
                .map_err(|_| ShakoError::NotFound(format!("branch '{b}' does not exist")))?;
            if open_isolated(&disk)?
                .try_find_reference(partial)
                .map_err(|e| ShakoError::Git(e.to_string()))?
                .is_none()
            {
                return Err(ShakoError::NotFound(format!(
                    "branch '{b}' does not exist; create it first or name new_branch with branch as its base"
                )));
            }
        }
        commit_file_changes(&disk, &b, &msg, &author, &committer, &ops)
    })
    .await?;
    settle_after_write(state, &mut repo, &disk_path).await;

    // Recorded under the repository's lock, so the next write to it comes after its event.
    let payload = build_push_webhook_payload(
        &repo,
        &format!("refs/heads/{branch}"),
        "",
        &commit_sha,
        ctx.subject.login(),
    );
    record_event(
        state,
        repo.org_id,
        Some(repo.id),
        "push",
        format!("push:{}:{branch}:{commit_sha}", repo.id),
        payload,
    )
    .await?;
    drop(guard);
    dispatch_logged(state, repo.org_id).await;
    crate::mirror::follow_write(state, &repo).await;
    Ok(commit_sha)
}

/// The combined state of a commit's statuses (`""` when it has none) and the statuses.
pub async fn commit_status(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    sha: &str,
) -> ShakoResult<(String, Vec<CommitStatusRecord>)> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await?;
    let statuses = state
        .store()
        .list_commit_statuses(repo.instance_id, repo.org_id, repo.id, sha)
        .await?;
    let combined = if statuses.is_empty() {
        ""
    } else if statuses.iter().any(|s| s.state == "error") {
        "error"
    } else if statuses.iter().any(|s| s.state == "failure") {
        "failure"
    } else if statuses
        .iter()
        .all(|s| matches!(s.state.as_str(), "success" | "warning"))
    {
        "success"
    } else {
        // A state the forge does not know never reads as green.
        "pending"
    };
    Ok((combined.to_owned(), statuses))
}

// =========================================================================
// Pull requests
// =========================================================================

async fn load_pull(
    state: &ShakoState,
    repo: &RepoRecord,
    number: u64,
) -> ShakoResult<PullRequestRecord> {
    state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await?
        .ok_or_else(|| ShakoError::NotFound("pull request not found".into()))
}

/// A pull request to open.
pub struct NewPullRequest {
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
}

/// Opens a pull request from `head` into `base`. One open pull request per head and base, as
/// Gitea keeps it: a second is `Conflict`.
pub async fn open_pull_request(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    new: NewPullRequest,
) -> ShakoResult<(RepoRecord, PullRequestRecord)> {
    let NewPullRequest {
        head,
        base,
        title,
        body,
    } = new;
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Propose).await?;
    // ponytail: two concurrent creates can both pass this read; a store constraint closes it.
    let open = state
        .store()
        .list_pull_requests(repo.instance_id, repo.org_id, Some(repo.id), Some("open"))
        .await?;
    if open
        .iter()
        .any(|pr| pr.head_ref == head && pr.base_ref == base)
    {
        return Err(ShakoError::Conflict(
            "pull request already exists for these targets".into(),
        ));
    }
    let (d, b, h) = (disk_path.clone(), base.clone(), head.clone());
    let m = blocking(move || check_mergeability(&d, &b, &h)).await?;
    let number = state
        .store()
        .allocate_pr_number(repo.instance_id, repo.org_id, repo.id)
        .await?;
    // `refs/pull/{number}/head` is fetchable by name (Contract S3).
    let (d, head_sha) = (disk_path.clone(), m.head_sha.clone());
    let _ =
        blocking(move || set_ref_direct(&d, &format!("refs/pull/{number}/head"), &head_sha)).await;
    let now = TimestampTz::now();
    let saved = state
        .store()
        .create_pull_request(PullRequestRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: repo.instance_id,
            org_id: repo.org_id,
            repo_id: repo.id,
            number,
            state: "open".into(),
            title,
            body,
            head_ref: head,
            head_sha: m.head_sha,
            base_ref: base,
            base_sha: m.base_sha,
            merge_base: m.merge_base,
            mergeable: Some(m.mergeable),
            merged: false,
            merge_commit_sha: String::new(),
            author_login: ctx.subject.login().to_owned(),
            author_name: ctx.subject.full_name.clone(),
            author_email: ctx.subject.email.clone(),
            created_at: now,
            updated_at: now,
            closed_at: None,
        })
        .await?;
    audit(
        state,
        ctx,
        "OpenPullRequest",
        format!("repos/{}/pulls/{}", repo.id, saved.number),
    )
    .await?;
    let payload = build_pr_webhook_payload(&repo, &saved, "opened", ctx.subject.login());
    emit_and_dispatch_event(
        state,
        repo.org_id,
        Some(repo.id),
        "pull_request",
        format!("pr_open:{}:{}", repo.id, saved.number),
        payload,
    )
    .await?;
    Ok((repo, saved))
}

/// One pull request; an open one has its mergeability refreshed against the branches.
pub async fn get_pull_request(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    number: u64,
) -> ShakoResult<(RepoRecord, PullRequestRecord)> {
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await?;
    let mut pr = load_pull(state, &repo, number).await?;
    if pr.state == "open" {
        let (b, h) = (pr.base_ref.clone(), pr.head_ref.clone());
        if let Ok(m) = blocking(move || check_mergeability(&disk_path, &b, &h)).await {
            // A read stores the refreshed state only when a ref moved since the last one.
            let changed = pr.head_sha != m.head_sha
                || pr.base_sha != m.base_sha
                || pr.merge_base != m.merge_base
                || pr.mergeable != Some(m.mergeable);
            pr.head_sha = m.head_sha;
            pr.base_sha = m.base_sha;
            pr.merge_base = m.merge_base;
            pr.mergeable = Some(m.mergeable);
            if changed && let Err(e) = state.store().update_pull_request(pr.clone()).await {
                tracing::warn!(repo = %repo.id, error = %e, "pull request state not refreshed");
            }
        }
    }
    Ok((repo, pr))
}

/// The files a pull request changes, from its merge base to its head.
pub async fn pull_request_files(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    number: u64,
) -> ShakoResult<Vec<DiffFileEntry>> {
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await?;
    let pr = load_pull(state, &repo, number).await?;
    let base = if pr.merge_base.is_empty() {
        pr.base_sha
    } else {
        pr.merge_base
    };
    let head = pr.head_sha;
    blocking(move || diff_refs_files(&disk_path, &base, &head)).await
}

/// The reviews of a pull request, oldest first.
pub async fn list_reviews(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    number: u64,
) -> ShakoResult<Vec<ReviewRecord>> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await?;
    load_pull(state, &repo, number).await?;
    state
        .store()
        .list_reviews(repo.instance_id, repo.org_id, Some(repo.id), Some(number))
        .await
}

/// A review to submit on a pull request.
pub struct NewReview {
    pub verdict: ReviewVerdict,
    pub body: String,
    pub commit_id: Option<String>,
}

/// Submits a review. A comment takes Propose, a verdict Approve; an author may not approve
/// their own pull request when its base branch requires approvals (Contract S10).
pub async fn create_review(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    number: u64,
    review: NewReview,
) -> ShakoResult<ReviewRecord> {
    let NewReview {
        verdict,
        body,
        commit_id,
    } = review;
    let needed = match verdict {
        ReviewVerdict::Comment => Action::Propose,
        ReviewVerdict::Approved | ReviewVerdict::RequestChanges => Action::Approve,
    };
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, needed).await?;
    let pr = load_pull(state, &repo, number).await?;
    if verdict == ReviewVerdict::Approved && ctx.subject.login() == pr.author_login {
        let policy = state
            .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
            .await?;
        if policy
            .matching_branch_rules(&pr.base_ref)
            .any(|rule| rule.required_approvals > 0)
        {
            return Err(ShakoError::Unprocessable(
                "pull request author cannot approve their own pull request".into(),
            ));
        }
    }
    let saved = state
        .store()
        .create_review(ReviewRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: repo.instance_id,
            org_id: repo.org_id,
            repo_id: repo.id,
            pr_number: number,
            verdict,
            body,
            author_login: ctx.subject.login().to_owned(),
            commit_id: commit_id
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| pr.head_sha.clone()),
            dismissed: false,
            created_at: TimestampTz::now(),
        })
        .await?;
    audit(
        state,
        ctx,
        "Review",
        format!("repos/{}/pulls/{number}/reviews/{}", repo.id, saved.id),
    )
    .await?;
    Ok(saved)
}

/// What a pull request update may change; `None` keeps the field.
#[derive(Debug, Clone, Default)]
pub struct PullRequestPatch {
    pub state: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub base: Option<String>,
}

/// Closes, reopens or retitles a pull request (Propose). A merged one stays closed.
pub async fn patch_pull_request(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    number: u64,
    patch: PullRequestPatch,
) -> ShakoResult<(RepoRecord, PullRequestRecord)> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Propose).await?;
    let mut pr = load_pull(state, &repo, number).await?;
    // Close is the registry's operation; any other change is an edit of the pull request.
    let operation = match &patch {
        PullRequestPatch {
            state: Some(st),
            title: None,
            body: None,
            base: None,
        } if st == "closed" => "Close",
        _ => "PatchPullRequest",
    };
    match patch.state.as_deref() {
        Some("closed") if pr.state != "closed" => {
            pr.state = "closed".into();
            pr.closed_at = Some(TimestampTz::now());
        }
        Some("open") if !pr.merged => {
            pr.state = "open".into();
            pr.closed_at = None;
        }
        _ => {}
    }
    if let Some(t) = patch.title {
        pr.title = t;
    }
    if let Some(b) = patch.body {
        pr.body = b;
    }
    if let Some(base) = patch.base.filter(|b| *b != pr.base_ref) {
        // A merged pull request records where it landed: its target stays.
        if pr.merged {
            return Err(ShakoError::Unprocessable(
                "a merged pull request keeps its base branch".into(),
            ));
        }
        validate_branch_name(&base)?;
        pr.base_ref = base;
    }
    let saved = state.store().update_pull_request(pr).await?;
    audit(
        state,
        ctx,
        operation,
        format!("repos/{}/pulls/{number}", repo.id),
    )
    .await?;
    Ok((repo, saved))
}

/// Merges the base branch into the pull request's head branch; already up to date is fine.
pub async fn update_pull_branch(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    number: u64,
) -> ShakoResult<String> {
    let (mut repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Propose).await?;
    let mut pr = load_pull(state, &repo, number).await?;
    if pr.merged || pr.state == "closed" {
        return Err(ShakoError::Conflict(
            "only an open pull request can be updated".into(),
        ));
    }
    let _guard = state.repo_locks.lock(repo.id).await;
    let (b, h) = (pr.base_ref.clone(), pr.head_ref.clone());
    // Updating moves the head branch like a push does: its protection rules decide (S5), with the
    // paths the base brings in as the changed paths.
    let (disk, head, base) = (disk_path.clone(), h.clone(), b.clone());
    let incoming = blocking(move || diff_refs_files(&disk, &head, &base)).await?;
    let changed_paths: Vec<String> = incoming.into_iter().map(|f| f.filename).collect();
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await?;
    policy.check_ref_update(
        ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        RefUpdate {
            refname: &format!("refs/heads/{h}"),
            is_delete: false,
            is_create: false,
            is_fast_forward: true,
            changed_paths: &changed_paths,
        },
    )?;
    audit(
        state,
        ctx,
        "UpdateBranch",
        format!("repos/{}/pulls/{number}", repo.id),
    )
    .await?;
    let author = caller_as_author(ctx);
    let disk = disk_path.clone();
    let (new_head, m) = state
        .git_work
        .run(ctx.org_id, move |admit| {
            admit(MERGE_OPERATION_BYTES)?;
            let new_head = update_pr_branch(&disk, &b, &h, &author)?;
            let m = check_mergeability(&disk, &b, &h)?;
            Ok((new_head, m))
        })
        .await?;
    settle_after_write(state, &mut repo, &disk_path).await;
    pr.head_sha = new_head.clone();
    pr.base_sha = m.base_sha;
    pr.merge_base = m.merge_base;
    pr.mergeable = Some(m.mergeable);
    let _ = state.store().update_pull_request(pr).await;
    Ok(new_head)
}

/// How a pull request is merged.
#[derive(Debug, Clone)]
pub struct MergeRequest {
    pub style: MergeStyle,
    /// The merge commit's message; `Merge pull request #{n}: {title}` when `None` or blank.
    pub message: Option<String>,
    /// The head the caller reviewed: a head that moved since is `Conflict` (T10).
    pub head_commit_id: Option<String>,
    pub delete_branch_after_merge: bool,
    /// Who the merge commit is attributed to (the person who merged in the Portal); the caller
    /// when `None`. The caller stays the committer, so the history names both.
    pub author: Option<CommitAuthor>,
}

/// Merges a pull request under the base branch's protection rules; answers the merge commit and
/// emits the `pull_request` `closed` event with `merged: true`.
pub async fn merge_pull_request(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    number: u64,
    req: MergeRequest,
) -> ShakoResult<String> {
    let (mut repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Write).await?;
    let mut pr = load_pull(state, &repo, number).await?;
    if pr.merged || pr.state == "closed" {
        return Err(ShakoError::Conflict(
            "pull request is already closed or merged".into(),
        ));
    }
    let _guard = state.repo_locks.lock(repo.id).await;
    let (d, b, h) = (disk_path.clone(), pr.base_ref.clone(), pr.head_ref.clone());
    let m = blocking(move || check_mergeability(&d, &b, &h)).await?;
    // The head precondition comes before the ref policy, so a moved head is 409 (S7, S10, T10).
    let expected = req.head_commit_id.filter(|s| !s.is_empty());
    if expected.as_deref().is_some_and(|e| e != m.head_sha) {
        return Err(ShakoError::Conflict(
            "head commit has moved since review".into(),
        ));
    }
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await?;
    let reviews = state
        .store()
        .list_reviews(repo.instance_id, repo.org_id, Some(repo.id), Some(number))
        .await?;
    let statuses = state
        .store()
        .list_commit_statuses(repo.instance_id, repo.org_id, repo.id, &m.head_sha)
        .await?;
    policy.check_pr_merge(
        ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        &pr.base_ref,
        &MergePolicyInput {
            pr_author_login: &pr.author_login,
            head_sha: &m.head_sha,
            is_behind_base: m.is_behind_base,
            reviews: &reviews,
            statuses: &statuses,
        },
    )?;
    // The head branch goes only when deleting it is allowed on its own; asked and refused, the
    // merge does not start.
    if req.delete_branch_after_merge {
        check_branch_delete(state, ctx, &repo, &policy, &pr.head_ref)?;
    }
    let message = req
        .message
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Merge pull request #{number}: {}", pr.title));
    audit(
        state,
        ctx,
        "Merge",
        format!("repos/{}/pulls/{number}", repo.id),
    )
    .await?;
    let committer = caller_as_author(ctx);
    let author = req.author.unwrap_or_else(|| committer.clone());
    let (d, b, h, style) = (
        disk_path.clone(),
        pr.base_ref.clone(),
        pr.head_ref.clone(),
        req.style,
    );
    let merge_sha = state
        .git_work
        .run(ctx.org_id, move |admit| {
            admit(MERGE_OPERATION_BYTES)?;
            execute_pr_merge(
                &d,
                &b,
                &h,
                &MergeOptions {
                    expected_head_sha: expected.as_deref(),
                    style,
                    message: &message,
                    author: &author,
                    committer: &committer,
                },
            )
        })
        .await?;
    if req.delete_branch_after_merge {
        let (d, h) = (disk_path.clone(), pr.head_ref.clone());
        if let Err(e) = blocking(move || git_delete_branch(&d, &h)).await {
            tracing::warn!(repo = %repo.id, error = %e, "head branch not deleted after the merge");
        }
    }
    pr.state = "closed".into();
    pr.merged = true;
    pr.mergeable = None;
    pr.head_sha = m.head_sha;
    pr.base_sha = m.base_sha;
    pr.merge_base = m.merge_base;
    pr.merge_commit_sha = merge_sha.clone();
    pr.closed_at = Some(TimestampTz::now());
    let saved = state.store().update_pull_request(pr).await?;
    settle_after_write(state, &mut repo, &disk_path).await;
    let payload = build_pr_webhook_payload(&repo, &saved, "closed", ctx.subject.login());
    emit_and_dispatch_event(
        state,
        repo.org_id,
        Some(repo.id),
        "pull_request",
        format!("pr_merge:{}:{}", repo.id, saved.number),
        payload,
    )
    .await?;
    crate::mirror::follow_write(state, &repo).await;
    Ok(merge_sha)
}

// =========================================================================
// Push mirrors and webhooks
// =========================================================================

/// A push mirror to add. The credential goes into the secret store and never comes back.
#[derive(Debug, Clone)]
pub struct NewPushMirror {
    pub remote_address: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub secret_ref: Option<String>,
    pub interval: Option<String>,
    pub sync_on_commit: bool,
    /// Push only this branch and new tags, fast-forward only (T-6892).
    pub branch: Option<String>,
}

/// The push mirrors of a repository (Admin).
pub async fn list_push_mirrors(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
) -> ShakoResult<(RepoRecord, Vec<PushMirrorRecord>)> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    let mirrors = state
        .store()
        .list_push_mirrors(repo.instance_id, repo.org_id, Some(repo.id))
        .await?;
    Ok((repo, mirrors))
}

/// Adds a push mirror to an allow-listed HTTPS host (Admin, Threat T3).
/// The secret-store prefix of the secrets the forge keeps for one repository's hooks and
/// mirrors.
fn repo_secret_prefix(repo: &RepoRecord) -> String {
    format!("repo:{}:", repo.id)
}

/// Checks a secret reference a caller names for `repo`: a key-shaped name, and never a key the
/// forge keeps for another repository (answered as not found).
fn check_caller_secret_ref(field: &str, r: &str, repo: &RepoRecord) -> ShakoResult<()> {
    shako_core::check_secret_ref(field, r)?;
    let generated = r.starts_with("repo:") || r.starts_with("webhook-") || r.starts_with("mirror:");
    if generated && !r.starts_with(&repo_secret_prefix(repo)) {
        return Err(ShakoError::NotFound(format!("{field}: secret not found")));
    }
    Ok(())
}

pub async fn add_push_mirror(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    req: NewPushMirror,
) -> ShakoResult<PushMirrorRecord> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    state
        .outbound_guard
        .validate_url(&req.remote_address, None)?;
    if let Some(r) = &req.secret_ref {
        check_caller_secret_ref("secretRef", r, &repo)?;
    }
    if let Some(b) = &req.branch {
        shako_core::validate_branch_name(b)?;
    }
    let remote_name = format!(
        "mirror-{}",
        &uuid::Uuid::now_v7().simple().to_string()[24..]
    );
    let secret_ref = req
        .secret_ref
        .unwrap_or_else(|| format!("{}mirror:{remote_name}", repo_secret_prefix(&repo)));
    if let Some(pw) = req.password.as_deref().filter(|p| !p.is_empty()) {
        // A caller-named reference may hold another hook's or mirror's secret: a password never
        // overwrites one that is already there.
        if state
            .secret_store
            .get_secret(repo.org_id, &secret_ref)
            .await?
            .is_some()
        {
            return Err(ShakoError::Conflict(
                "the named secret already exists; send the password or the secretRef, not both"
                    .into(),
            ));
        }
        let user = req.username.as_deref().unwrap_or("x-access-token");
        state
            .secret_store
            .put_secret(repo.org_id, &secret_ref, &format!("{user}:{pw}"))
            .await?;
    }
    let saved = state
        .store()
        .upsert_push_mirror(PushMirrorRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: repo.instance_id,
            org_id: repo.org_id,
            repo_id: repo.id,
            remote_name,
            remote_address: req.remote_address,
            credential_secret_ref: secret_ref,
            interval: req.interval.unwrap_or_else(|| "8h0m0s".into()),
            sync_on_commit: req.sync_on_commit,
            branch: req.branch.unwrap_or_default(),
            last_sync: None,
            last_error: String::new(),
            created_at: TimestampTz::now(),
        })
        .await?;
    audit(
        state,
        ctx,
        "AddPushMirror",
        format!("repos/{}/mirrors/{}", repo.id, saved.remote_name),
    )
    .await?;
    Ok(saved)
}

/// Removes a push mirror by its name (Admin); a name that is not there is fine.
pub async fn remove_push_mirror(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    remote_name: &str,
) -> ShakoResult<()> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    let deleted = state
        .store()
        .delete_push_mirror(repo.instance_id, repo.org_id, repo.id, remote_name)
        .await
        .unwrap_or(false);
    if deleted {
        audit(
            state,
            ctx,
            "RemovePushMirror",
            format!("repos/{}/mirrors/{remote_name}", repo.id),
        )
        .await?;
    }
    Ok(())
}

/// Pushes every mirror of the repository now (Admin).
pub async fn sync_push_mirrors(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
) -> ShakoResult<()> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    audit(
        state,
        ctx,
        "SyncPushMirrors",
        format!("repos/{}/mirrors", repo.id),
    )
    .await?;
    // Outside the request's unit: the sync claims and records in transactions of its own while
    // this request waits for it, which a mark held open in the unit would block.
    let (state, repo) = (state.clone(), repo);
    spawned(async move { crate::mirror::sync_now(&state, &repo).await }).await?
}

/// Where a webhook's signing secret comes from.
#[derive(Debug, Clone)]
pub enum HookSecret {
    /// Given inline; stored in the secret store under `repo:{repo id}:webhook-{id}` and never answered.
    Inline(String),
    /// Already in the secret store under this reference.
    Ref(String),
}

/// A webhook to register on a repository.
#[derive(Debug, Clone)]
pub struct NewWebhook {
    pub url: String,
    pub secret: HookSecret,
    pub previous_secret_ref: Option<String>,
    pub events: Vec<String>,
    pub active: bool,
    pub format: WebhookFormat,
}

/// Registers a webhook (Admin). Its URL passes the outbound guard (Threat T3); a hook without a
/// secret is refused, since every receiver this forge serves checks the signature.
pub async fn add_webhook(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    req: NewWebhook,
) -> ShakoResult<WebhookRecord> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    state.outbound_guard.validate_url(&req.url, None)?;
    let id = WebhookId::new();
    let secret_ref = match req.secret {
        HookSecret::Inline(secret) => {
            if secret.trim().is_empty() {
                return Err(ShakoError::Unprocessable(
                    "a hook needs a secret to sign its deliveries".into(),
                ));
            }
            let r = format!("{}webhook-{id}", repo_secret_prefix(&repo));
            state
                .secret_store
                .put_secret(repo.org_id, &r, &secret)
                .await?;
            r
        }
        HookSecret::Ref(r) if !r.trim().is_empty() => {
            check_caller_secret_ref("secretRef", &r, &repo)?;
            r
        }
        HookSecret::Ref(_) => {
            return Err(ShakoError::Unprocessable(
                "a hook needs a secret to sign its deliveries".into(),
            ));
        }
    };
    if let Some(r) = &req.previous_secret_ref {
        check_caller_secret_ref("previousSecretRef", r, &repo)?;
    }
    let saved = state
        .store()
        .create_webhook(WebhookRecord {
            id,
            instance_id: repo.instance_id,
            org_id: repo.org_id,
            repo_id: repo.id,
            url: req.url,
            events: if req.events.is_empty() {
                vec!["push".into(), "pull_request".into()]
            } else {
                req.events
            },
            active: req.active,
            secret_ref,
            previous_secret_ref: req.previous_secret_ref,
            format: req.format,
            created_at: TimestampTz::now(),
        })
        .await?;
    audit(
        state,
        ctx,
        "AddWebhook",
        format!("repos/{}/hooks/{}", repo.id, saved.id),
    )
    .await?;
    Ok(saved)
}

// =========================================================================
// Generic packages
// =========================================================================

/// The largest generic package file or container blob this forge stores.
pub const MAX_PACKAGE_BLOB_BYTES: usize = 256 * 1024 * 1024;

/// Checks `action` on a package of `owner`, whose organization the owner names (Threat T6).
/// An owner that is no plain name is `NotFound`: it names a directory of the blob store (T1).
pub fn authorize_package(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    pkg_type: &str,
    image_or_pkg: &str,
    action: Action,
) -> ShakoResult<()> {
    validate_repo_or_owner_name(owner, "owner")
        .map_err(|_| ShakoError::NotFound("package not found".into()))?;
    let target = ObjectTarget::Package {
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
        owner: owner.to_owned(),
        package_type: pkg_type.to_owned(),
        name: image_or_pkg.to_owned(),
    };
    state
        .authorizer
        .check(ctx, action, &target)
        .into_result("package not found")
}

/// Where a blob of `owner` lives on disk, by its SHA-256.
#[must_use]
pub fn blob_disk_path(state: &ShakoState, owner: &str, sha256_hex_digest: &str) -> PathBuf {
    blob_path(
        state.repo_root(),
        state.default_instance_id,
        owner,
        sha256_hex_digest,
    )
}

/// [`blob_disk_path`] for a caller that has no state yet (the manifest copy at start).
#[must_use]
pub fn blob_path(
    repo_root: &std::path::Path,
    instance_id: shako_core::InstanceId,
    owner: &str,
    sha256_hex_digest: &str,
) -> PathBuf {
    let hex_part = sha256_hex_digest
        .strip_prefix("sha256:")
        .unwrap_or(sha256_hex_digest);
    repo_root
        .join(instance_id.to_string())
        .join("blobs")
        .join(owner)
        .join("sha256")
        .join(hex_part)
}

/// The one writer of blob files: a temporary file of its own in the same directory, synced, renamed
/// over the name, and the directory synced. A crash leaves the old file or the new one, and two
/// writers of one digest leave the same bytes.
pub async fn write_blob_file(path: PathBuf, bytes: Vec<u8>) -> ShakoResult<()> {
    let failed = |e: std::io::Error| ShakoError::Storage(format!("blob write failed: {e}"));
    blocking(move || {
        use std::io::Write as _;
        let dir = path
            .parent()
            .ok_or_else(|| ShakoError::Storage("a blob path names a file".into()))?;
        std::fs::create_dir_all(dir).map_err(failed)?;
        let mut file = tempfile::NamedTempFile::new_in(dir).map_err(failed)?;
        file.write_all(&bytes).map_err(failed)?;
        file.as_file().sync_all().map_err(failed)?;
        file.persist(&path).map_err(|e| failed(e.error))?;
        std::fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(failed)
    })
    .await
}

/// Stores `bytes` as a blob of `owner`'s `image` in `org_id` and answers its digest. A blob new
/// to the image counts in the Organization's quota and past it is `QuotaExceeded`; one the image
/// holds counts nothing, also when the quota is used up.
pub async fn store_image_blob(
    state: &ShakoState,
    instance_id: shako_core::InstanceId,
    org_id: shako_core::OrgId,
    owner: &str,
    image: &str,
    bytes: Vec<u8>,
) -> ShakoResult<String> {
    let digest = format!("sha256:{}", sha256_hex(&bytes));
    let size_bytes = bytes.len() as u64;
    let limit = state
        .instance_for(instance_id, org_id)
        .await?
        .max_storage_bytes;
    let store = state.store();
    let held = store
        .get_blob(instance_id, org_id, owner, Some(image), &digest)
        .await?
        .is_some();
    // Looked at before the file is written; `store_blob` decides again under its lock.
    if !held {
        let used = store
            .get_quota_usage(instance_id, org_id)
            .await?
            .total_bytes();
        if used.saturating_add(size_bytes) > limit {
            return Err(ShakoError::QuotaExceeded {
                resource: "package storage".into(),
                limit,
            });
        }
    }
    write_blob_file(blob_disk_path(state, owner, &digest), bytes).await?;
    let blob = shako_core::BlobRecord {
        id: uuid::Uuid::now_v7(),
        instance_id,
        org_id,
        owner: owner.to_owned(),
        repo_name: image.to_owned(),
        digest: digest.clone(),
        size_bytes,
        created_at: TimestampTz::now(),
    };
    store.store_blob(blob, Some(limit)).await?;
    Ok(digest)
}

fn check_package_names(pkg: &str, ver: &str, file: &str) -> ShakoResult<()> {
    for (val, label) in [(pkg, "package"), (ver, "version"), (file, "filename")] {
        validate_package_segment(val, label)?;
    }
    Ok(())
}

/// How many bytes a new generic file of `owner`'s package may hold (Publish), so a surface
/// reading a body stops there.
pub async fn package_allowance(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    pkg: &str,
) -> ShakoResult<usize> {
    authorize_package(state, ctx, owner, "generic", pkg, Action::Publish)?;
    let instance = state.instance_for(ctx.instance_id, ctx.org_id).await?;
    let usage = state
        .store()
        .get_quota_usage(ctx.instance_id, ctx.org_id)
        .await?;
    Ok((remaining_storage_allowance(&instance, &usage)? as usize).min(MAX_PACKAGE_BLOB_BYTES))
}

/// Publishes one file of a generic package version. Files never change: one already there is
/// `Conflict`.
pub async fn publish_generic_file(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    pkg: &str,
    ver: &str,
    file: &str,
    bytes: Vec<u8>,
) -> ShakoResult<PackageFileRecord> {
    check_package_names(pkg, ver, file)?;
    let allowance = package_allowance(state, ctx, owner, pkg).await?;
    if bytes.len() > allowance {
        return Err(ShakoError::QuotaExceeded {
            resource: "package storage".into(),
            limit: allowance as u64,
        });
    }
    if state
        .store()
        .get_package_file(ctx.instance_id, ctx.org_id, owner, pkg, ver, file)
        .await?
        .is_some()
    {
        return Err(ShakoError::Conflict("package file already exists".into()));
    }
    let sha256 = sha256_hex(&bytes);
    let disk_path = blob_disk_path(state, owner, &sha256);
    let size_bytes = bytes.len() as u64;
    write_blob_file(disk_path, bytes).await?;
    let now = TimestampTz::now();
    let pkg_rec = state
        .store()
        .upsert_package(PackageRecord {
            id: PackageId::new(),
            instance_id: ctx.instance_id,
            org_id: ctx.org_id,
            owner: owner.to_owned(),
            package_type: "generic".into(),
            name: pkg.to_owned(),
            created_at: now,
        })
        .await?;
    // A version holds several files: the first file creates it, later files join it, so the
    // files already stored keep a version id that exists.
    let existing = state
        .store()
        .get_package_version(ctx.instance_id, ctx.org_id, owner, "generic", pkg, ver)
        .await?;
    let ver_rec = match existing {
        Some(v) => v,
        None => {
            state
                .store()
                .upsert_package_version(PackageVersionRecord {
                    id: uuid::Uuid::now_v7(),
                    instance_id: ctx.instance_id,
                    org_id: ctx.org_id,
                    package_id: pkg_rec.id,
                    owner: owner.to_owned(),
                    package_type: "generic".into(),
                    package_name: pkg.to_owned(),
                    version: ver.to_owned(),
                    digest: format!("sha256:{sha256}"),
                    media_type: "application/octet-stream".into(),
                    subject_digest: None,
                    artifact_type: None,
                    size_bytes,
                    signed: false,
                    created_at: now,
                })
                .await?
        }
    };
    let saved = state
        .store()
        .insert_package_file(PackageFileRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: ctx.instance_id,
            org_id: ctx.org_id,
            version_id: ver_rec.id,
            owner: owner.to_owned(),
            package_name: pkg.to_owned(),
            version: ver.to_owned(),
            filename: file.to_owned(),
            sha256,
            size_bytes,
            created_at: now,
        })
        .await?;
    let _ = state
        .store()
        .adjust_quota_usage(ctx.instance_id, ctx.org_id, 0, 0, size_bytes as i64)
        .await;
    audit(
        state,
        ctx,
        "PublishPackage",
        format!(
            "packages/{}/versions/{}/files/{}",
            pkg_rec.id, ver_rec.id, saved.id
        ),
    )
    .await?;
    Ok(saved)
}

/// One generic package file (Read), opened for streaming.
pub async fn read_generic_file(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    pkg: &str,
    ver: &str,
    file: &str,
) -> ShakoResult<tokio::fs::File> {
    authorize_package(state, ctx, owner, "generic", pkg, Action::Read)?;
    let rec = state
        .store()
        .get_package_file(ctx.instance_id, ctx.org_id, owner, pkg, ver, file)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ShakoError::NotFound("package file not found".into()))?;
    tokio::fs::File::open(blob_disk_path(state, owner, &rec.sha256))
        .await
        .map_err(|_| ShakoError::NotFound("package blob not found".into()))
}

/// Delivers a test `push` event to one webhook of the repository now (Admin). A hook of
/// another repository is `NotFound`, also inside the same organization.
pub async fn redeliver_webhook(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    hook_id: WebhookId,
) -> ShakoResult<DeliveryRecord> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    let hook = state
        .store()
        .get_webhook(repo.instance_id, repo.org_id, hook_id)
        .await?
        .filter(|h| h.repo_id == repo.id)
        .ok_or_else(|| ShakoError::NotFound("webhook not found".into()))?;
    let payload = build_push_webhook_payload(
        &repo,
        &format!("refs/heads/{}", repo.default_branch),
        "",
        "",
        ctx.subject.login(),
    );
    audit(
        state,
        ctx,
        "RedeliverWebhook",
        format!("repos/{}/hooks/{hook_id}", repo.id),
    )
    .await?;
    deliver_webhook_now(state, &hook, "push", &payload).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blocking task answers its own result, its error included.
    #[tokio::test]
    async fn a_blocking_task_answers_its_result() -> ShakoResult<()> {
        assert_eq!(blocking(|| Ok(7)).await?, 7);
        let refused = blocking::<()>(|| Err(ShakoError::Conflict("busy".into()))).await;
        assert!(matches!(refused, Err(ShakoError::Conflict(m)) if m == "busy"));
        Ok(())
    }

    /// A blocking task that panics answers an internal error naming the panic; the request
    /// does not hang and the runtime goes on.
    #[tokio::test]
    async fn a_blocking_task_that_panics_is_an_internal_error() {
        let answer = blocking::<()>(|| panic!("gix gave up")).await;
        assert!(
            matches!(&answer, Err(ShakoError::Internal(m)) if m.contains("panicked") && m.contains("gix gave up")),
            "{answer:?}"
        );
    }

    fn guarding() -> BranchProtectionRule {
        BranchProtectionRule {
            rule_name: "main".into(),
            enable_push: false,
            ..BranchProtectionRule::default()
        }
    }

    /// A guarding rule's protected paths changed, or unprotected paths named or changed, loosen
    /// it; the same paths, or a rule guarding no writer, do not.
    #[test]
    fn changed_file_patterns_loosen_a_guarding_rule() {
        let stored = BranchProtectionRule {
            protected_file_patterns: "deploy/**".into(),
            ..guarding()
        };
        let moved = BranchProtectionRule {
            protected_file_patterns: "docs/**".into(),
            ..stored.clone()
        };
        assert!(loosens(&stored, &moved));
        assert!(!loosens(&stored, &stored.clone()));
        let unprotected = BranchProtectionRule {
            unprotected_file_patterns: "*.md".into(),
            ..stored.clone()
        };
        assert!(loosens(&stored, &unprotected));
        assert!(!loosens(&unprotected, &unprotected.clone()));
        let open = BranchProtectionRule {
            enable_push: true,
            ..BranchProtectionRule::default()
        };
        let open_moved = BranchProtectionRule {
            protected_file_patterns: "docs/**".into(),
            ..open.clone()
        };
        assert!(!guards_writers(&open));
        assert!(!loosens(&open, &open_moved));
    }
}
