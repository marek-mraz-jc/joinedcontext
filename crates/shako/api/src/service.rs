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
    build_pr_webhook_payload, build_push_webhook_payload, deliver_webhook_with_retries,
    emit_and_dispatch_event,
};
use crate::smart_http::sync_repo_mirrors_on_commit;
use crate::state::{ShakoState, resolve_and_authorize_repo};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use shako_core::{
    Action, BranchProtectionRule, CommitStatusRecord, DeliveryRecord, MERGE_OPERATION_WEIGHT_BYTES,
    MergePolicyInput, ObjectTarget, PackageFileRecord, PackageId, PackageRecord,
    PackageVersionRecord, ProtectionRuleRecord, PullRequestRecord, PushMirrorRecord, RefPolicy,
    RefUpdate, RepoId, RepoRecord, RequestCtx, ReviewRecord, ReviewVerdict, ShakoError,
    ShakoResult, WebhookFormat, WebhookId, WebhookRecord, check_repo_count_quota,
    remaining_storage_allowance, sha256_hex, validate_branch_name, validate_package_segment,
    validate_repo_or_owner_name, validate_tree_path,
};
use shako_git::{
    CommitAuthor, DiffFileEntry, FileChangeOp, FileOpKind, MergeOptions, MergeStyle,
    check_mergeability, commit_file_changes, compute_repo_size_bytes, copy_bare_repo,
    create_branch as git_create_branch, delete_branch as git_delete_branch, diff_refs_files,
    execute_pr_merge, init_bare_repo, open_isolated, set_ref_direct, update_pr_branch,
};
use std::path::PathBuf;

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ShakoResult<T> + Send + 'static,
) -> ShakoResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))?
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
            .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
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
    rec: RepoRecord,
    disk_path: &std::path::Path,
) -> ShakoResult<RepoRecord> {
    let size = rec.size_bytes;
    let created = match state.store().create_repo(rec).await {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(disk_path);
            return Err(e);
        }
    };
    state.register_owner_org(&created.owner, ctx.org_id);
    let _ = state
        .store()
        .adjust_quota_usage(ctx.instance_id, ctx.org_id, 1, size as i64, 0)
        .await;
    Ok(created)
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
    let instance = state.instance_for(ctx.instance_id, ctx.org_id).await;
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
    let now = Utc::now();
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
    record_new_repo(state, ctx, rec, &disk_path).await
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
    refuse_existing(state, ctx, &req.owner, &req.name).await?;

    let new_id = RepoId::new();
    let root = state.repo_root().to_path_buf();
    let inst_id = ctx.instance_id;
    let def_branch = origin.default_branch.clone();
    let desc = req.description.clone().unwrap_or_default();
    let dest = blocking(move || {
        let dest = init_bare_repo(&root, inst_id, new_id, &def_branch, false, &desc)?;
        copy_bare_repo(&origin_disk, &dest, &def_branch)?;
        Ok(dest)
    })
    .await?;
    let now = Utc::now();
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
    record_new_repo(state, ctx, rec, &dest).await
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
    let (mut repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    if let Some(a) = patch.archived {
        repo.archived = a;
    }
    if let Some(d) = patch.description {
        repo.description = d;
    }
    if let Some(b) = patch.default_branch {
        validate_branch_name(&b)?;
        repo.default_branch = b;
    }
    if let Some(p) = patch.private {
        repo.private = p;
    }
    if let Some(n) = patch.name {
        validate_repo_or_owner_name(&n, "repository")?;
        repo.name = n;
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
    let _ = std::fs::remove_dir_all(&disk_path);
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
        .await
        .unwrap_or_default();
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
    )?;
    let _guard = state.repo_locks.lock(repo.id).await;
    let b = branch.to_owned();
    if blocking(move || git_delete_branch(&disk_path, &b)).await? {
        Ok(())
    } else {
        Err(ShakoError::NotFound("branch does not exist".into()))
    }
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
    let now = Utc::now();
    state
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
        .await
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
    state
        .store()
        .delete_protection_rule(repo.instance_id, repo.org_id, repo.id, rule_name)
        .await
}

/// A rule that keeps some writer off the branch: push closed, or push or merge narrowed to an
/// allow-list. Neither an admin token nor the Portal's removes one through the API (P-24): a bug
/// or a leaked token must not reopen `main` or a Change branch. Removing the repository removes
/// its rules.
fn guards_writers(rule: &BranchProtectionRule) -> bool {
    !rule.enable_push || rule.enable_push_whitelist || rule.enable_merge_whitelist
}

/// Whether `next` lets someone write or merge whom `stored` kept out (P-24). The allow-lists'
/// names may change; dropping a guard may not.
fn loosens(stored: &BranchProtectionRule, next: &BranchProtectionRule) -> bool {
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

    let guard = state.repo_locks.lock(repo.id).await;
    let (disk, b, msg) = (disk_path.clone(), branch.clone(), req.message);
    let commit_sha =
        blocking(move || commit_file_changes(&disk, &b, &msg, &author, &committer, &ops)).await?;
    repo.size_bytes = compute_repo_size_bytes(&disk_path);
    let _ = state.store().update_repo(repo.clone()).await;
    drop(guard);

    let payload = build_push_webhook_payload(
        &repo,
        &format!("refs/heads/{branch}"),
        "",
        &commit_sha,
        &ctx.subject.username,
    );
    let _ = emit_and_dispatch_event(
        state,
        repo.org_id,
        Some(repo.id),
        "push",
        format!("push:{}:{branch}:{commit_sha}", repo.id),
        payload,
    )
    .await;
    sync_repo_mirrors_on_commit(state, &repo, &disk_path).await;
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
    } else if statuses.iter().any(|s| s.state == "pending") {
        "pending"
    } else {
        "success"
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
    let now = Utc::now();
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
            author_login: ctx.subject.username.clone(),
            author_name: ctx.subject.full_name.clone(),
            author_email: ctx.subject.email.clone(),
            created_at: now,
            updated_at: now,
            closed_at: None,
        })
        .await?;
    let payload = build_pr_webhook_payload(&repo, &saved, "opened", &ctx.subject.username);
    let _ = emit_and_dispatch_event(
        state,
        repo.org_id,
        Some(repo.id),
        "pull_request",
        format!("pr_open:{}:{}", repo.id, saved.number),
        payload,
    )
    .await;
    Ok((repo, saved))
}

/// The pull requests of a repository, optionally of one state (`open`, `closed`; `all` or
/// `None` for every one), newest number first.
pub async fn list_pull_requests(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
    pr_state: Option<&str>,
) -> ShakoResult<(RepoRecord, Vec<PullRequestRecord>)> {
    let (repo, _) = resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await?;
    let wanted = pr_state.filter(|s| *s != "all");
    let prs = state
        .store()
        .list_pull_requests(repo.instance_id, repo.org_id, Some(repo.id), wanted)
        .await?;
    Ok((repo, prs))
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
            pr.head_sha = m.head_sha;
            pr.base_sha = m.base_sha;
            pr.merge_base = m.merge_base;
            pr.mergeable = Some(m.mergeable);
            let _ = state.store().update_pull_request(pr.clone()).await;
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
    if verdict == ReviewVerdict::Approved && ctx.subject.username == pr.author_login {
        let policy = state
            .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
            .await
            .unwrap_or_default();
        if policy
            .matching_branch_rule(&pr.base_ref)
            .is_some_and(|rule| rule.required_approvals > 0)
        {
            return Err(ShakoError::Unprocessable(
                "pull request author cannot approve their own pull request".into(),
            ));
        }
    }
    state
        .store()
        .create_review(ReviewRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: repo.instance_id,
            org_id: repo.org_id,
            repo_id: repo.id,
            pr_number: number,
            verdict,
            body,
            author_login: ctx.subject.username.clone(),
            commit_id: commit_id
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| pr.head_sha.clone()),
            dismissed: false,
            created_at: Utc::now(),
        })
        .await
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
    match patch.state.as_deref() {
        Some("closed") if pr.state != "closed" => {
            pr.state = "closed".into();
            pr.closed_at = Some(Utc::now());
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
    if let Some(base) = patch.base {
        pr.base_ref = base;
    }
    let saved = state.store().update_pull_request(pr).await?;
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
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Propose).await?;
    let mut pr = load_pull(state, &repo, number).await?;
    let permit = state
        .memory_budget
        .acquire(MERGE_OPERATION_WEIGHT_BYTES)
        .await?;
    let _guard = state.repo_locks.lock(repo.id).await;
    let (b, h) = (pr.base_ref.clone(), pr.head_ref.clone());
    let author = caller_as_author(ctx);
    let (new_head, m) = blocking(move || {
        let _hold = permit;
        let new_head = update_pr_branch(&disk_path, &b, &h, &author)?;
        let m = check_mergeability(&disk_path, &b, &h)?;
        Ok((new_head, m))
    })
    .await?;
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
    let permit = state
        .memory_budget
        .acquire(MERGE_OPERATION_WEIGHT_BYTES)
        .await?;
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
        .await
        .unwrap_or_default();
    let statuses = state
        .store()
        .list_commit_statuses(repo.instance_id, repo.org_id, repo.id, &m.head_sha)
        .await
        .unwrap_or_default();
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
    let message = req
        .message
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Merge pull request #{number}: {}", pr.title));
    let committer = caller_as_author(ctx);
    let author = req.author.unwrap_or_else(|| committer.clone());
    let (d, b, h, style) = (
        disk_path.clone(),
        pr.base_ref.clone(),
        pr.head_ref.clone(),
        req.style,
    );
    let merge_sha = blocking(move || {
        let _hold = permit;
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
        let _ = blocking(move || git_delete_branch(&d, &h)).await;
    }
    pr.state = "closed".into();
    pr.merged = true;
    pr.mergeable = None;
    pr.head_sha = m.head_sha;
    pr.base_sha = m.base_sha;
    pr.merge_base = m.merge_base;
    pr.merge_commit_sha = merge_sha.clone();
    pr.closed_at = Some(Utc::now());
    let saved = state.store().update_pull_request(pr).await?;
    repo.size_bytes = compute_repo_size_bytes(&disk_path);
    let _ = state.store().update_repo(repo.clone()).await;
    let payload = build_pr_webhook_payload(&repo, &saved, "closed", &ctx.subject.username);
    let _ = emit_and_dispatch_event(
        state,
        repo.org_id,
        Some(repo.id),
        "pull_request",
        format!("pr_merge:{}:{}", repo.id, saved.number),
        payload,
    )
    .await;
    sync_repo_mirrors_on_commit(state, &repo, &disk_path).await;
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
        shako_core::check_secret_ref("secretRef", r)?;
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
        .unwrap_or_else(|| format!("mirror:{}:{}", repo.id, remote_name));
    if let Some(pw) = req.password.as_deref().filter(|p| !p.is_empty()) {
        // A caller-named reference may hold another hook's or mirror's secret: a password never
        // overwrites one that is already there.
        if state
            .secret_store
            .get_secret(repo.org_id, &secret_ref)
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
            .put_secret(repo.org_id, &secret_ref, &format!("{user}:{pw}"));
    }
    state
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
            created_at: Utc::now(),
        })
        .await
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
    let _ = state
        .store()
        .delete_push_mirror(repo.instance_id, repo.org_id, repo.id, remote_name)
        .await;
    Ok(())
}

/// Pushes every mirror of the repository now (Admin).
pub async fn sync_push_mirrors(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
) -> ShakoResult<()> {
    let (repo, disk_path) =
        resolve_and_authorize_repo(state, ctx, owner, name, Action::Admin).await?;
    sync_repo_mirrors_on_commit(state, &repo, &disk_path).await;
    Ok(())
}

/// Where a webhook's signing secret comes from.
#[derive(Debug, Clone)]
pub enum HookSecret {
    /// Given inline; stored in the secret store under `webhook-{id}` and never answered.
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
            let r = format!("webhook-{id}");
            state.secret_store.put_secret(repo.org_id, &r, &secret);
            r
        }
        HookSecret::Ref(r) if !r.trim().is_empty() => {
            shako_core::check_secret_ref("secretRef", &r)?;
            r
        }
        HookSecret::Ref(_) => {
            return Err(ShakoError::Unprocessable(
                "a hook needs a secret to sign its deliveries".into(),
            ));
        }
    };
    if let Some(r) = &req.previous_secret_ref {
        shako_core::check_secret_ref("previousSecretRef", r)?;
    }
    state
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
            created_at: Utc::now(),
        })
        .await
}

/// The disk path of a repository the caller reads, for read-only git work.
pub async fn readable_repo(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    name: &str,
) -> ShakoResult<(RepoRecord, PathBuf)> {
    resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await
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
    let target_org = state.resolve_org_for_owner(owner, &ctx.subject);
    let mut check_ctx = ctx.clone();
    check_ctx.org_id = target_org;
    let target = ObjectTarget::Package {
        instance_id: ctx.instance_id,
        org_id: target_org,
        owner: owner.to_owned(),
        package_type: pkg_type.to_owned(),
        name: image_or_pkg.to_owned(),
    };
    state
        .authorizer
        .check(&check_ctx, action, &target)
        .into_result("package not found")
}

/// Where a blob of `owner` lives on disk, by its SHA-256.
#[must_use]
pub fn blob_disk_path(state: &ShakoState, owner: &str, sha256_hex_digest: &str) -> PathBuf {
    let hex_part = sha256_hex_digest
        .strip_prefix("sha256:")
        .unwrap_or(sha256_hex_digest);
    state
        .repo_root()
        .join(state.default_instance_id.to_string())
        .join("blobs")
        .join(owner)
        .join("sha256")
        .join(hex_part)
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
    let target_org = state.resolve_org_for_owner(owner, &ctx.subject);
    let instance = state.instance_for(ctx.instance_id, target_org).await;
    let usage = state
        .store()
        .get_quota_usage(ctx.instance_id, target_org)
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
    let target_org = state.resolve_org_for_owner(owner, &ctx.subject);
    if state
        .store()
        .get_package_file(ctx.instance_id, target_org, owner, pkg, ver, file)
        .await?
        .is_some()
    {
        return Err(ShakoError::Conflict("package file already exists".into()));
    }
    let sha256 = sha256_hex(&bytes);
    let disk_path = blob_disk_path(state, owner, &sha256);
    if let Some(p) = disk_path.parent() {
        tokio::fs::create_dir_all(p)
            .await
            .map_err(|e| ShakoError::Storage(e.to_string()))?;
    }
    tokio::fs::write(&disk_path, &bytes)
        .await
        .map_err(|e| ShakoError::Storage(e.to_string()))?;
    let now = Utc::now();
    let pkg_rec = state
        .store()
        .upsert_package(PackageRecord {
            id: PackageId::new(),
            instance_id: ctx.instance_id,
            org_id: target_org,
            owner: owner.to_owned(),
            package_type: "generic".into(),
            name: pkg.to_owned(),
            created_at: now,
        })
        .await?;
    let ver_rec = state
        .store()
        .upsert_package_version(PackageVersionRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: ctx.instance_id,
            org_id: target_org,
            package_id: pkg_rec.id,
            owner: owner.to_owned(),
            package_type: "generic".into(),
            package_name: pkg.to_owned(),
            version: ver.to_owned(),
            digest: format!("sha256:{sha256}"),
            media_type: "application/octet-stream".into(),
            manifest_bytes: Vec::new(),
            subject_digest: None,
            artifact_type: None,
            size_bytes: bytes.len() as u64,
            signed: false,
            created_at: now,
        })
        .await?;
    let saved = state
        .store()
        .insert_package_file(PackageFileRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: ctx.instance_id,
            org_id: target_org,
            version_id: ver_rec.id,
            owner: owner.to_owned(),
            package_name: pkg.to_owned(),
            version: ver.to_owned(),
            filename: file.to_owned(),
            sha256,
            size_bytes: bytes.len() as u64,
            created_at: now,
        })
        .await?;
    let _ = state
        .store()
        .adjust_quota_usage(ctx.instance_id, target_org, 0, 0, bytes.len() as i64)
        .await;
    Ok(saved)
}

/// The bytes of one generic package file (Read).
pub async fn read_generic_file(
    state: &ShakoState,
    ctx: &RequestCtx,
    owner: &str,
    pkg: &str,
    ver: &str,
    file: &str,
) -> ShakoResult<Vec<u8>> {
    authorize_package(state, ctx, owner, "generic", pkg, Action::Read)?;
    let target_org = state.resolve_org_for_owner(owner, &ctx.subject);
    let rec = state
        .store()
        .get_package_file(ctx.instance_id, target_org, owner, pkg, ver, file)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ShakoError::NotFound("package file not found".into()))?;
    tokio::fs::read(blob_disk_path(state, owner, &rec.sha256))
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
        &ctx.subject.username,
    );
    deliver_webhook_with_retries(state, &hook, "push", &payload).await
}
