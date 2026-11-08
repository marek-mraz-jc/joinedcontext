#![forbid(unsafe_code)]
//! Gitea 1.27.3 REST compatibility routes (Rows 1–39 of R§1.1, Contract S7, `COMPAT.md`).
//! Every request struct uses `#[serde(deny_unknown_fields)]`.
//! Rows 6–11 (Actions) return 404 with Gitea error JSON `{"message":"actions are not served by this forge yet","url":...}`
//! per `00-task.md` section 1.

use crate::outbox::{
    build_pr_webhook_payload, build_push_webhook_payload, deliver_webhook_with_retries,
    emit_and_dispatch_event,
};
use crate::smart_http::sync_repo_mirrors_on_commit;
use crate::state::{
    ShakoState, authenticate_request, gitea_error_response, repo_to_gitea_json,
    resolve_and_authorize_repo, stream_bytes_in_64k_chunks,
};
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use jc_router::Mount;
use serde::Deserialize;
use serde_json::{Value, json};
use shako_core::{
    ARCHIVE_OPERATION_WEIGHT_BYTES, Action, BranchProtectionRule, CommitStatusRecord,
    MERGE_OPERATION_WEIGHT_BYTES, MergePolicyInput, ObjectTarget, ProtectionRuleRecord,
    PullRequestRecord, PushMirrorRecord, RefPolicy, RefUpdate, RepoId, RepoRecord, ReviewRecord,
    OrgId, ReviewVerdict, ShakoError, ShakoResult, WebhookId, WebhookRecord, check_repo_count_quota,
    validate_branch_name, validate_repo_or_owner_name, validate_tree_path,
};
use shako_git::{
    ArchiveFormat, CommitAuthor, FileChangeOp, FileOpKind, MergeOptions, MergeStyle,
    check_mergeability, commit_file_changes, compute_repo_size_bytes, copy_bare_repo,
    create_archive, create_branch, create_single_ref_bundle, delete_branch, diff_refs_files,
    execute_pr_merge, init_bare_repo, list_commits, list_tags, list_tree_entries, open_isolated,
    read_commit_info, read_file_at_ref, set_ref_direct, update_pr_branch,
};

fn default_main() -> String {
    "main".to_owned()
}
fn default_true() -> bool {
    true
}

// =========================================================================
// Version / Health probe
// =========================================================================

/// Liveness probe: no identity, no version, no data (probes carry no token).
async fn api_healthz() -> Response {
    (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response()
}

async fn api_version(State(state): State<ShakoState>, headers: HeaderMap) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, None, "shako") {
        return gitea_error_response(&state, &e);
    }
    (
        StatusCode::OK,
        Json(json!({
            "version": "1.27.3+shako",
            "status": "ok"
        })),
    )
        .into_response()
}

// =========================================================================
// Rows 6-11: Actions (Out of scope in this run -> 404 with Gitea error JSON)
// =========================================================================

async fn actions_not_served_yet(State(state): State<ShakoState>, headers: HeaderMap) -> Response {
    // Deny by default: an unauthenticated caller learns nothing, not even that Actions are absent.
    if let Err(e) = authenticate_request(&state, &headers, None, "shako") {
        return gitea_error_response(&state, &e);
    }
    let err = ShakoError::NotFound("actions are not served by this forge yet".into());
    gitea_error_response(&state, &err)
}

// =========================================================================
// Row 1, 2, 3, 15, 18: Repository CRUD, Migrate, Archive, Delete
// =========================================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrgRepoReq {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub private: bool,
    #[serde(default)]
    pub auto_init: bool,
    #[serde(default = "default_main")]
    pub default_branch: String,
    #[serde(default)]
    pub trust_model: Option<String>,
}

async fn compat_get_repo(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    (StatusCode::OK, Json(repo_to_gitea_json(&repo, &mount))).into_response()
}

async fn compat_create_org_repo(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path(owner): Path<String>,
    Json(req): Json<CreateOrgRepoReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = validate_repo_or_owner_name(&owner, "owner") {
        return gitea_error_response(&state, &e);
    }
    if let Err(e) = validate_repo_or_owner_name(&req.name, "repository") {
        return gitea_error_response(&state, &e);
    }
    if let Err(e) = validate_branch_name(&req.default_branch) {
        return gitea_error_response(&state, &e);
    }

    let org_target = ObjectTarget::Organization {
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
        owner: owner.clone(),
    };
    if let Err(e) = state
        .authorizer
        .check(&ctx, Action::Write, &org_target)
        .into_result("organization not found")
    {
        return gitea_error_response(&state, &e);
    }

    let instance = state.instance_for(ctx.instance_id, ctx.org_id).await;
    let usage = match state
        .store()
        .get_quota_usage(ctx.instance_id, ctx.org_id)
        .await
    {
        Ok(u) => u,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = check_repo_count_quota(&instance, &usage) {
        return gitea_error_response(&state, &e);
    }

    if let Ok(Some(_)) = state
        .store()
        .get_repo_by_name(ctx.instance_id, ctx.org_id, &owner, &req.name)
        .await
    {
        return gitea_error_response(
            &state,
            &ShakoError::Conflict("repository already exists".into()),
        );
    }

    let repo_id = RepoId::new();
    let root = state.repo_root().to_path_buf();
    let inst_id = ctx.instance_id;
    let def_branch = req.default_branch.clone();
    let auto_init = req.auto_init;
    let desc = req.description.clone();

    let disk_path = match tokio::task::spawn_blocking(move || {
        init_bare_repo(&root, inst_id, repo_id, &def_branch, auto_init, &desc)
    })
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return gitea_error_response(&state, &e),
        Err(e) => return gitea_error_response(&state, &ShakoError::Internal(e.to_string())),
    };

    let size_bytes = compute_repo_size_bytes(&disk_path);
    let now = Utc::now();
    let rec = RepoRecord {
        id: repo_id,
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
        project_id: req.name.clone(),
        owner: owner.clone(),
        name: req.name,
        description: req.description,
        default_branch: req.default_branch,
        private: req.private,
        archived: false,
        size_bytes,
        next_pr_number: 1,
        created_at: now,
        updated_at: now,
    };

    let created = match state.store().create_repo(rec).await {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&disk_path);
            return gitea_error_response(&state, &e);
        }
    };
    state.register_owner_org(&owner, ctx.org_id);
    let _ = state
        .store()
        .adjust_quota_usage(ctx.instance_id, ctx.org_id, 1, size_bytes as i64, 0)
        .await;

    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    (
        StatusCode::CREATED,
        Json(repo_to_gitea_json(&created, &mount)),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrateRepoReq {
    pub clone_addr: String,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
    pub repo_owner: String,
    pub repo_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub private: bool,
    #[serde(default)]
    pub mirror: bool,
    #[serde(default)]
    pub issues: bool,
    #[serde(default)]
    pub pull_requests: bool,
    #[serde(default)]
    pub wiki: bool,
    #[serde(default)]
    pub releases: bool,
    #[serde(default)]
    pub labels: bool,
    #[serde(default)]
    pub milestones: bool,
    #[serde(default)]
    pub lfs: bool,
}

/// Row 3: `POST /api/v1/repos/migrate` — accepts only `clone_addr` on the same forge instance
/// (intra-forge copy, Contract S7).
async fn compat_migrate_repo(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(req): Json<MigrateRepoReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&req.repo_owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = validate_repo_or_owner_name(&req.repo_owner, "repo_owner") {
        return gitea_error_response(&state, &e);
    }
    if let Err(e) = validate_repo_or_owner_name(&req.repo_name, "repo_name") {
        return gitea_error_response(&state, &e);
    }
    if req.mirror {
        return gitea_error_response(
            &state,
            &ShakoError::Unprocessable(
                "pull mirrors are not supported; only intra-forge copies".into(),
            ),
        );
    }

    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let parsed_url = match url::Url::parse(&req.clone_addr) {
        Ok(u) => u,
        Err(_) => {
            return gitea_error_response(
                &state,
                &ShakoError::Unprocessable("invalid clone_addr URL".into()),
            );
        }
    };

    // Enforce same-instance check: host must match mount.host, localhost, 127.0.0.1, or .svc.cluster.local
    let url_host = parsed_url.host_str().unwrap_or_default();
    let mount_host_only = mount
        .host
        .split_once(':')
        .map_or(mount.host.as_str(), |(h, _)| h);
    let same_instance = url_host.eq_ignore_ascii_case(mount_host_only)
        || url_host == "127.0.0.1"
        || url_host == "localhost"
        || url_host.ends_with(".svc.cluster.local");
    if !same_instance {
        return gitea_error_response(
            &state,
            &ShakoError::Unprocessable(
                "repos/migrate only accepts clone_addr on the same forge instance".into(),
            ),
        );
    }

    let path_trimmed = parsed_url
        .path()
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let segs: Vec<&str> = path_trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return gitea_error_response(
            &state,
            &ShakoError::Unprocessable("clone_addr must end with /{owner}/{repo}.git".into()),
        );
    }
    let origin_owner = segs[segs.len() - 2];
    let origin_repo_name = segs[segs.len() - 1];

    let (origin_repo, origin_disk) = match resolve_and_authorize_repo(
        &state,
        &ctx,
        origin_owner,
        origin_repo_name,
        Action::Read,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let org_target = ObjectTarget::Organization {
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
        owner: req.repo_owner.clone(),
    };
    if let Err(e) = state
        .authorizer
        .check(&ctx, Action::Write, &org_target)
        .into_result("organization not found")
    {
        return gitea_error_response(&state, &e);
    }

    if let Ok(Some(_)) = state
        .store()
        .get_repo_by_name(ctx.instance_id, ctx.org_id, &req.repo_owner, &req.repo_name)
        .await
    {
        return gitea_error_response(
            &state,
            &ShakoError::Conflict("repository already exists".into()),
        );
    }

    let new_repo_id = RepoId::new();
    let root = state.repo_root().to_path_buf();
    let inst_id = ctx.instance_id;
    let def_branch = origin_repo.default_branch.clone();
    let desc = req.description.clone().unwrap_or_default();

    let dest_disk = match tokio::task::spawn_blocking(move || {
        let dest = init_bare_repo(&root, inst_id, new_repo_id, &def_branch, false, &desc)?;
        copy_bare_repo(&origin_disk, &dest, &def_branch)?;
        Ok::<_, ShakoError>(dest)
    })
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return gitea_error_response(&state, &e),
        Err(e) => return gitea_error_response(&state, &ShakoError::Internal(e.to_string())),
    };

    let size_bytes = compute_repo_size_bytes(&dest_disk);
    let now = Utc::now();
    let rec = RepoRecord {
        id: new_repo_id,
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
        project_id: req.repo_name.clone(),
        owner: req.repo_owner,
        name: req.repo_name,
        description: req.description.unwrap_or_default(),
        default_branch: origin_repo.default_branch,
        private: req.private,
        archived: false,
        size_bytes,
        next_pr_number: 1,
        created_at: now,
        updated_at: now,
    };

    let created = match state.store().create_repo(rec).await {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dest_disk);
            return gitea_error_response(&state, &e);
        }
    };
    let _ = state
        .store()
        .adjust_quota_usage(ctx.instance_id, ctx.org_id, 1, size_bytes as i64, 0)
        .await;

    (
        StatusCode::CREATED,
        Json(repo_to_gitea_json(&created, &mount)),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchRepoReq {
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub private: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
}

async fn compat_patch_repo(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name)): Path<(String, String)>,
    Json(req): Json<PatchRepoReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (mut repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };

    if let Some(arch) = req.archived {
        repo.archived = arch;
    }
    if let Some(desc) = req.description {
        repo.description = desc;
    }
    if let Some(def_b) = req.default_branch {
        if let Err(e) = validate_branch_name(&def_b) {
            return gitea_error_response(&state, &e);
        }
        repo.default_branch = def_b;
    }
    if let Some(priv_flag) = req.private {
        repo.private = priv_flag;
    }
    if let Some(new_name) = req.name {
        if let Err(e) = validate_repo_or_owner_name(&new_name, "repository") {
            return gitea_error_response(&state, &e);
        }
        repo.name = new_name;
    }

    match state.store().update_repo(repo).await {
        Ok(updated) => {
            let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
            (StatusCode::OK, Json(repo_to_gitea_json(&updated, &mount))).into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_delete_repo(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };

    let _guard = state.repo_locks.lock(repo.id).await;
    if let Err(e) = state
        .store()
        .delete_repo(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        return gitea_error_response(&state, &e);
    }
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
    StatusCode::NO_CONTENT.into_response()
}

// =========================================================================
// Row 5: Push Mirrors
// =========================================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddPushMirrorReq {
    pub remote_address: String,
    #[serde(default)]
    pub remote_username: Option<String>,
    #[serde(default)]
    pub remote_password: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub interval: Option<String>,
    #[serde(default = "default_true")]
    pub sync_on_commit: bool,
}

async fn compat_list_push_mirrors(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    match state
        .store()
        .list_push_mirrors(repo.instance_id, repo.org_id, Some(repo.id))
        .await
    {
        Ok(list) => {
            let items: Vec<Value> = list
                .into_iter()
                .map(|m| {
                    json!({
                        "remote_name": m.remote_name,
                        "remote_address": m.remote_address,
                        "interval": m.interval,
                        "sync_on_commit": m.sync_on_commit,
                        "last_update": m.last_sync.map(|d| d.to_rfc3339()),
                        "last_error": m.last_error,
                    })
                })
                .collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_add_push_mirror(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
    Json(req): Json<AddPushMirrorReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };

    if let Err(e) = state.outbound_guard.validate_url(&req.remote_address, None) {
        return gitea_error_response(&state, &e);
    }

    let remote_name = format!("mirror-{}", &uuid::Uuid::now_v7().to_string()[..8]);
    let secret_ref = req
        .secret_ref
        .unwrap_or_else(|| format!("mirror:{}:{}", repo.id, remote_name));
    if let Some(pw) = &req.remote_password {
        let user = req.remote_username.as_deref().unwrap_or("x-access-token");
        state
            .secret_store
            .put_secret(repo.org_id, &secret_ref, &format!("{user}:{pw}"));
    }

    let rec = PushMirrorRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: repo.instance_id,
        org_id: repo.org_id,
        repo_id: repo.id,
        remote_name: remote_name.clone(),
        remote_address: req.remote_address,
        credential_secret_ref: secret_ref,
        interval: req.interval.unwrap_or_else(|| "8h0m0s".into()),
        sync_on_commit: req.sync_on_commit,
        last_sync: None,
        last_error: String::new(),
        created_at: Utc::now(),
    };
    match state.store().upsert_push_mirror(rec).await {
        Ok(saved) => (
            StatusCode::OK,
            Json(json!({
                "remote_name": saved.remote_name,
                "remote_address": saved.remote_address,
                "interval": saved.interval,
                "sync_on_commit": saved.sync_on_commit,
                "last_error": saved.last_error,
            })),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_delete_push_mirror(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, remote_name)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let _ = state
        .store()
        .delete_push_mirror(repo.instance_id, repo.org_id, repo.id, &remote_name)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

async fn compat_sync_push_mirrors(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    sync_repo_mirrors_on_commit(&state, &repo, &disk_path).await;
    StatusCode::OK.into_response()
}

// =========================================================================
// Rows 16, 17, 19: Transition no-op routes (User, Teams, Collaborators)
// =========================================================================

async fn compat_get_current_user(State(state): State<ShakoState>, headers: HeaderMap) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    (
        StatusCode::OK,
        Json(json!({
            "id": 1,
            "login": ctx.subject.username,
            "full_name": ctx.subject.full_name,
            "email": ctx.subject.email.unwrap_or_default(),
        })),
    )
        .into_response()
}

async fn compat_get_user_org_permissions(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((_login, org)): Path<(String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&org), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let _ = emit_and_dispatch_event(
        &state,
        ctx.org_id,
        None,
        "transition.warning",
        format!("warn:org_permissions:{}", uuid::Uuid::now_v7()),
        json!({"route": "users/{login}/orgs/{org}/permissions", "note": "permissions come from jc-auth RoleBindings"}),
    )
    .await;
    (
        StatusCode::OK,
        Json(json!({
            "is_owner": false,
            "is_admin": false,
            "can_write": true,
            "can_read": true
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTeamReq {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub includes_all_repositories: bool,
    #[serde(default)]
    pub can_create_org_repo: bool,
    #[serde(default)]
    pub units: Vec<String>,
    #[serde(default)]
    pub units_map: serde_json::Map<String, Value>,
}

async fn compat_list_org_teams(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&org), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let _ = emit_and_dispatch_event(
        &state,
        ctx.org_id,
        None,
        "transition.warning",
        format!("warn:list_teams:{}", uuid::Uuid::now_v7()),
        json!({"route": "orgs/{org}/teams"}),
    )
    .await;
    (StatusCode::OK, Json(json!([]))).into_response()
}

async fn compat_create_org_team(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(req): Json<CreateTeamReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&org), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let _ = emit_and_dispatch_event(
        &state,
        ctx.org_id,
        None,
        "transition.warning",
        format!("warn:create_team:{}", uuid::Uuid::now_v7()),
        json!({"route": "orgs/{org}/teams", "team": req.name}),
    )
    .await;
    (
        StatusCode::CREATED,
        Json(json!({
            "id": 1,
            "name": req.name,
            "description": req.description,
        })),
    )
        .into_response()
}

async fn compat_delete_team(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path(_id): Path<u64>,
) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, None, "shako") {
        return gitea_error_response(&state, &e);
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn compat_list_team_repos(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path(_id): Path<u64>,
) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, None, "shako") {
        return gitea_error_response(&state, &e);
    }
    (StatusCode::OK, Json(json!([]))).into_response()
}

async fn compat_put_or_delete_team_repo(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((_id, org, _repo)): Path<(u64, String, String)>,
) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, Some(&org), "shako") {
        return gitea_error_response(&state, &e);
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaboratorReq {
    #[serde(default)]
    pub permission: Option<String>,
}

async fn compat_put_collaborator(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, reader)): Path<(String, String, String)>,
    body: Option<Json<CollaboratorReq>>,
) -> Response {
    let _ = body;
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Write).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let _ = emit_and_dispatch_event(
        &state,
        repo.org_id,
        Some(repo.id),
        "transition.warning",
        format!("warn:collaborator:{}:{reader}", repo.id),
        json!({"route": "collaborators", "collaborator": reader}),
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

// =========================================================================
// Row 20: Branch Protections (S5)
// =========================================================================

async fn compat_list_branch_protections(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    match state
        .store()
        .list_protection_rules(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(list) => {
            let rules: Vec<BranchProtectionRule> = list.into_iter().map(|r| r.rule).collect();
            (StatusCode::OK, Json(rules)).into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_get_branch_protection(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, rule_name)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    match state
        .store()
        .get_protection_rule(repo.instance_id, repo.org_id, repo.id, &rule_name)
        .await
    {
        Ok(Some(rec)) => (StatusCode::OK, Json(rec.rule)).into_response(),
        Ok(None) => gitea_error_response(
            &state,
            &ShakoError::NotFound("branch protection not found".into()),
        ),
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_create_branch_protection(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
    Json(rule): Json<BranchProtectionRule>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    if let Err(e) = RefPolicy::validate_rule_for_repo(
        &rule,
        state.authorizer.as_ref(),
        repo.instance_id,
        repo.org_id,
        &repo.owner,
        &repo.name,
    ) {
        return gitea_error_response(&state, &e);
    }
    let now = Utc::now();
    let rec = ProtectionRuleRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: repo.instance_id,
        org_id: repo.org_id,
        repo_id: repo.id,
        rule,
        created_at: now,
        updated_at: now,
    };
    match state.store().upsert_protection_rule(rec).await {
        Ok(saved) => (StatusCode::CREATED, Json(saved.rule)).into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBranchProtectionReq {
    #[serde(default)]
    pub rule_name: Option<String>,
    #[serde(default = "default_true")]
    pub enable_push: bool,
    #[serde(default)]
    pub enable_push_whitelist: bool,
    #[serde(default)]
    pub push_whitelist_usernames: Vec<String>,
    #[serde(default)]
    pub push_whitelist_teams: Vec<String>,
    #[serde(default)]
    pub push_whitelist_deploy_keys: bool,
    #[serde(default)]
    pub enable_force_push: bool,
    #[serde(default)]
    pub enable_force_push_allowlist: bool,
    #[serde(default)]
    pub force_push_allowlist_usernames: Vec<String>,
    #[serde(default)]
    pub force_push_allowlist_teams: Vec<String>,
    #[serde(default)]
    pub force_push_allowlist_deploy_keys: bool,
    #[serde(default)]
    pub enable_merge_whitelist: bool,
    #[serde(default)]
    pub merge_whitelist_usernames: Vec<String>,
    #[serde(default)]
    pub merge_whitelist_teams: Vec<String>,
    #[serde(default)]
    pub enable_status_check: bool,
    #[serde(default, alias = "required_status_checks")]
    pub status_check_contexts: Vec<String>,
    #[serde(default)]
    pub required_approvals: u64,
    #[serde(default)]
    pub enable_approvals_whitelist: bool,
    #[serde(default)]
    pub approvals_whitelist_username: Vec<String>,
    #[serde(default)]
    pub approvals_whitelist_teams: Vec<String>,
    #[serde(default)]
    pub block_on_rejected_reviews: bool,
    #[serde(default)]
    pub block_on_official_review_requests: bool,
    #[serde(default)]
    pub block_on_outdated_branch: bool,
    #[serde(default)]
    pub dismiss_stale_approvals: bool,
    #[serde(default)]
    pub ignore_stale_approvals: bool,
    #[serde(default)]
    pub require_signed_commits: bool,
    #[serde(default)]
    pub protected_file_patterns: String,
    #[serde(default)]
    pub unprotected_file_patterns: String,
    #[serde(default = "default_true")]
    pub block_admin_merge_override: bool,
    #[serde(default)]
    pub enable_bypass_allowlist: bool,
}

async fn compat_patch_branch_protection(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, rule_name)): Path<(String, String, String)>,
    Json(req): Json<PatchBranchProtectionReq>,
) -> Response {
    let rule = BranchProtectionRule {
        rule_name: req.rule_name.unwrap_or(rule_name),
        enable_push: req.enable_push,
        enable_push_whitelist: req.enable_push_whitelist,
        push_whitelist_usernames: req.push_whitelist_usernames,
        push_whitelist_teams: req.push_whitelist_teams,
        push_whitelist_deploy_keys: req.push_whitelist_deploy_keys,
        enable_force_push: req.enable_force_push,
        enable_force_push_allowlist: req.enable_force_push_allowlist,
        force_push_allowlist_usernames: req.force_push_allowlist_usernames,
        force_push_allowlist_teams: req.force_push_allowlist_teams,
        force_push_allowlist_deploy_keys: req.force_push_allowlist_deploy_keys,
        enable_merge_whitelist: req.enable_merge_whitelist,
        merge_whitelist_usernames: req.merge_whitelist_usernames,
        merge_whitelist_teams: req.merge_whitelist_teams,
        enable_status_check: req.enable_status_check,
        status_check_contexts: req.status_check_contexts,
        required_approvals: req.required_approvals,
        enable_approvals_whitelist: req.enable_approvals_whitelist,
        approvals_whitelist_username: req.approvals_whitelist_username,
        approvals_whitelist_teams: req.approvals_whitelist_teams,
        block_on_rejected_reviews: req.block_on_rejected_reviews,
        block_on_official_review_requests: req.block_on_official_review_requests,
        block_on_outdated_branch: req.block_on_outdated_branch,
        dismiss_stale_approvals: req.dismiss_stale_approvals,
        ignore_stale_approvals: req.ignore_stale_approvals,
        require_signed_commits: req.require_signed_commits,
        protected_file_patterns: req.protected_file_patterns,
        unprotected_file_patterns: req.unprotected_file_patterns,
        block_admin_merge_override: req.block_admin_merge_override,
        enable_bypass_allowlist: req.enable_bypass_allowlist,
    };
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    if let Err(e) = RefPolicy::validate_rule_for_repo(
        &rule,
        state.authorizer.as_ref(),
        repo.instance_id,
        repo.org_id,
        &repo.owner,
        &repo.name,
    ) {
        return gitea_error_response(&state, &e);
    }
    let now = Utc::now();
    let rec = ProtectionRuleRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: repo.instance_id,
        org_id: repo.org_id,
        repo_id: repo.id,
        rule,
        created_at: now,
        updated_at: now,
    };
    match state.store().upsert_protection_rule(rec).await {
        Ok(saved) => (StatusCode::OK, Json(saved.rule)).into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_delete_branch_protection(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, rule_name)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let _ = state
        .store()
        .delete_protection_rule(repo.instance_id, repo.org_id, repo.id, &rule_name)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

// =========================================================================
// Rows 21, 22, 23, 24, 33, 35: Trees, Branches, Statuses, Tags, Git Commits
// =========================================================================

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeQuery {
    #[serde(default)]
    pub recursive: Option<String>,
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub per_page: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

async fn compat_get_git_tree(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, git_ref)): Path<(String, String, String)>,
    Query(q): Query<TreeQuery>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (_repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let recursive = q
        .recursive
        .as_deref()
        .is_some_and(|v| v == "true" || v == "1");
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.or(q.limit).unwrap_or(1000).clamp(1, 1000);

    let res =
        tokio::task::spawn_blocking(move || list_tree_entries(&disk_path, &git_ref, recursive))
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))
            .and_then(|r| r);

    match res {
        Ok((tree_sha, all_entries)) => {
            let total = all_entries.len();
            let start = (page - 1).saturating_mul(per_page);
            let page_entries: Vec<_> = all_entries.into_iter().skip(start).take(per_page).collect();
            let truncated = start.saturating_add(page_entries.len()) < total;
            (
                StatusCode::OK,
                [("x-total-count", total.to_string())],
                Json(json!({
                    "sha": tree_sha,
                    "tree": page_entries,
                    "truncated": truncated,
                    "page": page,
                    "total_count": total,
                })),
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_get_branch(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, branch)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
        .unwrap_or_default();
    let is_protected = policy.matching_branch_rule(&branch).is_some();

    let branch_clone = branch.clone();
    let res = tokio::task::spawn_blocking(move || {
        let gix_repo = open_isolated(&disk_path)?;
        let full_ref = format!("refs/heads/{branch_clone}");
        let partial: &gix_ref::PartialNameRef = full_ref
            .as_str()
            .try_into()
            .map_err(|_| ShakoError::NotFound("branch not found".into()))?;
        let mut r = gix_repo
            .try_find_reference(partial)
            .map_err(|e| ShakoError::Git(e.to_string()))?
            .ok_or_else(|| ShakoError::NotFound("branch not found".into()))?;
        let commit_oid = r
            .peel_to_id()
            .map_err(|_| ShakoError::NotFound("branch not found".into()))?
            .detach();
        Ok::<_, ShakoError>(commit_oid.to_hex().to_string())
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(head_sha) => (
            StatusCode::OK,
            Json(json!({
                "name": branch,
                "commit": {
                    "id": head_sha,
                },
                "protected": is_protected,
            })),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBranchReq {
    pub new_branch_name: String,
    #[serde(default)]
    pub old_branch_name: Option<String>,
    #[serde(default)]
    pub old_ref_name: Option<String>,
}

async fn compat_create_branch(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
    Json(req): Json<CreateBranchReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let policy = match state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = policy.check_ref_update(
        &ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        RefUpdate {
            refname: &format!("refs/heads/{}", req.new_branch_name),
            is_delete: false,
            is_create: true,
            is_fast_forward: true,
            changed_paths: &[],
        },
    ) {
        return gitea_error_response(&state, &e);
    }

    let _guard = state.repo_locks.lock(repo.id).await;
    let from_ref = req
        .old_branch_name
        .or(req.old_ref_name)
        .unwrap_or_else(|| repo.default_branch.clone());
    let new_b = req.new_branch_name.clone();

    let res = tokio::task::spawn_blocking(move || create_branch(&disk_path, &new_b, &from_ref))
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))
        .and_then(|r| r);

    match res {
        Ok(sha) => (
            StatusCode::CREATED,
            Json(json!({
                "name": req.new_branch_name,
                "commit": { "id": sha },
            })),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_delete_branch(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, branch)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let policy = match state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = policy.check_ref_update(
        &ctx,
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
    ) {
        return gitea_error_response(&state, &e);
    }

    let _guard = state.repo_locks.lock(repo.id).await;
    let res = tokio::task::spawn_blocking(move || delete_branch(&disk_path, &branch))
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))
        .and_then(|r| r);

    match res {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => gitea_error_response(
            &state,
            &ShakoError::NotFound("branch does not exist".into()),
        ),
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_get_commit_status(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, sha)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let statuses = match state
        .store()
        .list_commit_statuses(repo.instance_id, repo.org_id, repo.id, &sha)
        .await
    {
        Ok(s) => s,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let combined_state = if statuses.is_empty() {
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

    (
        StatusCode::OK,
        Json(json!({
            "state": combined_state,
            "total_count": statuses.len(),
            "statuses": statuses,
            "sha": sha,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCommitStatusReq {
    pub state: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
}

async fn compat_post_commit_status(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, sha)): Path<(String, String, String)>,
    Json(req): Json<CreateCommitStatusReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Write).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let rec = CommitStatusRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: repo.instance_id,
        org_id: repo.org_id,
        repo_id: repo.id,
        sha,
        context: req.context.unwrap_or_else(|| "default".into()),
        state: req.state,
        description: req.description.unwrap_or_default(),
        target_url: req.target_url.unwrap_or_default(),
        updated_at: Utc::now(),
    };
    match state.store().upsert_commit_status(rec).await {
        Ok(saved) => (StatusCode::CREATED, Json(saved)).into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

// =========================================================================
// Rows 25, 26, 27: Contents (GET, PUT, DELETE, POST multi-file)
// =========================================================================

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentsQuery {
    #[serde(rename = "ref", default)]
    pub git_ref: Option<String>,
}

async fn compat_get_contents(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, file_path)): Path<(String, String, String)>,
    Query(q): Query<ContentsQuery>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let git_ref = q.git_ref.unwrap_or(repo.default_branch);
    let fp = file_path.clone();

    let res = tokio::task::spawn_blocking(move || read_file_at_ref(&disk_path, &git_ref, &fp))
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))
        .and_then(|r| r);

    match res {
        Ok(Some((blob_sha, bytes))) => {
            let name = file_path
                .rsplit('/')
                .next()
                .unwrap_or(&file_path)
                .to_owned();
            (
                StatusCode::OK,
                Json(json!({
                    "name": name,
                    "path": file_path,
                    "sha": blob_sha,
                    "type": "file",
                    "size": bytes.len(),
                    "encoding": "base64",
                    "content": STANDARD.encode(&bytes),
                })),
            )
                .into_response()
        }
        Ok(None) => gitea_error_response(&state, &ShakoError::NotFound("file not found".into())),
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorDto {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutFileReq {
    pub content: String,
    pub message: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub new_branch: Option<String>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub author: Option<AuthorDto>,
    #[serde(default)]
    pub committer: Option<AuthorDto>,
}

async fn compat_put_contents(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, file_path)): Path<(String, String, String)>,
    Json(req): Json<PutFileReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (mut repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let clean_path = match validate_tree_path(&file_path) {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let branch = req
        .new_branch
        .or(req.branch)
        .unwrap_or_else(|| repo.default_branch.clone());

    let policy = match state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = policy.check_ref_update(
        &ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        RefUpdate {
            refname: &format!("refs/heads/{branch}"),
            is_delete: false,
            is_create: false,
            is_fast_forward: true,
            changed_paths: std::slice::from_ref(&clean_path),
        },
    ) {
        return gitea_error_response(&state, &e);
    }

    let decoded = match STANDARD.decode(
        req.content
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .as_bytes(),
    ) {
        Ok(b) => b,
        Err(e) => {
            return gitea_error_response(
                &state,
                &ShakoError::Unprocessable(format!("invalid base64 content: {e}")),
            );
        }
    };

    let is_create = req.sha.as_deref().is_none_or(str::is_empty);
    let author = req.author.map_or_else(
        || CommitAuthor {
            name: ctx.subject.full_name.clone(),
            email: ctx
                .subject
                .email
                .clone()
                .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
        },
        |a| CommitAuthor {
            name: a.name,
            email: a.email,
        },
    );
    let committer = req.committer.map_or_else(
        || author.clone(),
        |c| CommitAuthor {
            name: c.name,
            email: c.email,
        },
    );

    let _guard = state.repo_locks.lock(repo.id).await;
    let op = FileChangeOp {
        operation: if is_create {
            FileOpKind::Create
        } else {
            FileOpKind::Update
        },
        path: clean_path.clone(),
        content: Some(decoded),
        sha: req.sha,
    };
    let disk_clone = disk_path.clone();
    let branch_clone = branch.clone();
    let msg = req.message;

    let res = tokio::task::spawn_blocking(move || {
        commit_file_changes(&disk_clone, &branch_clone, &msg, &author, &committer, &[op])
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(commit_sha) => {
            repo.size_bytes = compute_repo_size_bytes(&disk_path);
            let _ = state.store().update_repo(repo.clone()).await;
            let payload = build_push_webhook_payload(
                &repo,
                &format!("refs/heads/{branch}"),
                "",
                &commit_sha,
                &ctx.subject.username,
            );
            let _ = emit_and_dispatch_event(
                &state,
                repo.org_id,
                Some(repo.id),
                "push",
                format!("push:{}:{branch}:{commit_sha}", repo.id),
                payload,
            )
            .await;
            sync_repo_mirrors_on_commit(&state, &repo, &disk_path).await;
            let status = if is_create {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(json!({
                    "content": { "path": clean_path },
                    "commit": { "sha": commit_sha },
                })),
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteFileReq {
    pub message: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub new_branch: Option<String>,
    pub sha: String,
    #[serde(default)]
    pub author: Option<AuthorDto>,
    #[serde(default)]
    pub committer: Option<AuthorDto>,
}

async fn compat_delete_contents(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, file_path)): Path<(String, String, String)>,
    Json(req): Json<DeleteFileReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let clean_path = match validate_tree_path(&file_path) {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let branch = req
        .new_branch
        .or(req.branch)
        .unwrap_or_else(|| repo.default_branch.clone());

    let policy = match state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = policy.check_ref_update(
        &ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        RefUpdate {
            refname: &format!("refs/heads/{branch}"),
            is_delete: false,
            is_create: false,
            is_fast_forward: true,
            changed_paths: std::slice::from_ref(&clean_path),
        },
    ) {
        return gitea_error_response(&state, &e);
    }

    let author = req.author.map_or_else(
        || CommitAuthor {
            name: ctx.subject.full_name.clone(),
            email: ctx
                .subject
                .email
                .clone()
                .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
        },
        |a| CommitAuthor {
            name: a.name,
            email: a.email,
        },
    );
    let committer = req.committer.map_or_else(
        || author.clone(),
        |c| CommitAuthor {
            name: c.name,
            email: c.email,
        },
    );

    let _guard = state.repo_locks.lock(repo.id).await;
    let op = FileChangeOp {
        operation: FileOpKind::Delete,
        path: clean_path,
        content: None,
        sha: Some(req.sha),
    };
    let disk_clone = disk_path.clone();
    let branch_clone = branch.clone();
    let msg = req.message;

    let res = tokio::task::spawn_blocking(move || {
        commit_file_changes(&disk_clone, &branch_clone, &msg, &author, &committer, &[op])
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(commit_sha) => {
            sync_repo_mirrors_on_commit(&state, &repo, &disk_path).await;
            (
                StatusCode::OK,
                Json(json!({
                    "content": Value::Null,
                    "commit": { "sha": commit_sha },
                })),
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiFileItemReq {
    pub operation: FileOpKind,
    pub path: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub sha: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeFilesReq {
    pub files: Vec<MultiFileItemReq>,
    pub message: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub new_branch: Option<String>,
    #[serde(default)]
    pub author: Option<AuthorDto>,
    #[serde(default)]
    pub committer: Option<AuthorDto>,
}

async fn compat_post_contents_multi(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
    Json(req): Json<ChangeFilesReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (mut repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let branch = req
        .new_branch
        .or(req.branch)
        .unwrap_or_else(|| repo.default_branch.clone());

    let mut ops = Vec::with_capacity(req.files.len());
    let mut changed_paths = Vec::with_capacity(req.files.len());
    for item in req.files {
        let clean = match validate_tree_path(&item.path) {
            Ok(p) => p,
            Err(e) => return gitea_error_response(&state, &e),
        };
        let bytes = match item.content {
            Some(b64) => {
                let stripped: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
                match STANDARD.decode(stripped.as_bytes()) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        return gitea_error_response(
                            &state,
                            &ShakoError::Unprocessable(format!(
                                "invalid base64 for '{clean}': {e}"
                            )),
                        );
                    }
                }
            }
            None => None,
        };
        changed_paths.push(clean.clone());
        ops.push(FileChangeOp {
            operation: item.operation,
            path: clean,
            content: bytes,
            sha: item.sha,
        });
    }

    let policy = match state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    if let Err(e) = policy.check_ref_update(
        &ctx,
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
    ) {
        return gitea_error_response(&state, &e);
    }

    let author = req.author.map_or_else(
        || CommitAuthor {
            name: ctx.subject.full_name.clone(),
            email: ctx
                .subject
                .email
                .clone()
                .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
        },
        |a| CommitAuthor {
            name: a.name,
            email: a.email,
        },
    );
    let committer = req.committer.map_or_else(
        || author.clone(),
        |c| CommitAuthor {
            name: c.name,
            email: c.email,
        },
    );

    let _guard = state.repo_locks.lock(repo.id).await;
    let disk_clone = disk_path.clone();
    let branch_clone = branch.clone();
    let msg = req.message;

    let res = tokio::task::spawn_blocking(move || {
        commit_file_changes(&disk_clone, &branch_clone, &msg, &author, &committer, &ops)
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(commit_sha) => {
            repo.size_bytes = compute_repo_size_bytes(&disk_path);
            let _ = state.store().update_repo(repo.clone()).await;
            let payload = build_push_webhook_payload(
                &repo,
                &format!("refs/heads/{branch}"),
                "",
                &commit_sha,
                &ctx.subject.username,
            );
            let _ = emit_and_dispatch_event(
                &state,
                repo.org_id,
                Some(repo.id),
                "push",
                format!("push:{}:{branch}:{commit_sha}", repo.id),
                payload,
            )
            .await;
            sync_repo_mirrors_on_commit(&state, &repo, &disk_path).await;
            (
                StatusCode::CREATED,
                Json(json!({
                    "files": [],
                    "commit": { "sha": commit_sha },
                })),
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

// =========================================================================
// Rows 28, 32, 33, 35: Commits, Archives/Bundles, Tags
// =========================================================================

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListCommitsQuery {
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub stat: Option<String>,
    #[serde(default)]
    pub verification: Option<String>,
    #[serde(default)]
    pub files: Option<String>,
}

async fn compat_list_commits(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
    Query(q): Query<ListCommitsQuery>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let git_ref = q.sha.unwrap_or(repo.default_branch);
    let limit = q.limit.unwrap_or(30).clamp(1, 1000);
    let path_filter = q.path;

    let res = tokio::task::spawn_blocking(move || {
        list_commits(&disk_path, &git_ref, path_filter.as_deref(), limit, 10_000)
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(commits) => {
            let items: Vec<Value> = commits
                .into_iter()
                .map(|c| {
                    json!({
                        "sha": c.sha,
                        "commit": {
                            "message": c.message,
                            "author": {
                                "name": c.author_name,
                                "email": c.author_email,
                                "date": c.author_date,
                            },
                            "committer": {
                                "name": c.committer_name,
                                "email": c.committer_email,
                                "date": c.committer_date,
                            }
                        }
                    })
                })
                .collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_get_git_commit(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, sha)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (_repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let res = tokio::task::spawn_blocking(move || read_commit_info(&disk_path, &sha))
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))
        .and_then(|r| r);

    match res {
        Ok(c) => (
            StatusCode::OK,
            Json(json!({
                "sha": c.sha,
                "commit": {
                    "message": c.message,
                    "author": {
                        "name": c.author_name,
                        "email": c.author_email,
                        "date": c.author_date,
                    },
                    "committer": {
                        "name": c.committer_name,
                        "email": c.committer_email,
                        "date": c.committer_date,
                    }
                }
            })),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PagingQuery {
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub per_page: Option<usize>,
}

async fn compat_list_tags(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
    Query(q): Query<PagingQuery>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (_repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.or(q.per_page).unwrap_or(50).clamp(1, 500);

    let res = tokio::task::spawn_blocking(move || list_tags(&disk_path))
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))
        .and_then(|r| r);

    match res {
        Ok(all_tags) => {
            let total = all_tags.len();
            let start = (page - 1).saturating_mul(limit);
            let items: Vec<Value> = all_tags
                .into_iter()
                .skip(start)
                .take(limit)
                .map(|(name, id, commit_sha, _)| {
                    json!({
                        "name": name,
                        "id": id,
                        "commit": { "sha": commit_sha },
                    })
                })
                .collect();
            (
                StatusCode::OK,
                [("x-total-count", total.to_string())],
                Json(items),
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_get_archive(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, ref_and_ext)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let permit = match state
        .memory_budget
        .acquire(ARCHIVE_OPERATION_WEIGHT_BYTES)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let rname = repo.name.clone();
    if let Some(git_ref) = ref_and_ext.strip_suffix(".bundle") {
        let git_ref = git_ref.to_owned();
        let res = tokio::task::spawn_blocking(move || {
            let _hold = permit;
            create_single_ref_bundle(&disk_path, &git_ref)
        })
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))
        .and_then(|r| r);

        return match res {
            Ok(bytes) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/octet-stream")],
                stream_bytes_in_64k_chunks(bytes),
            )
                .into_response(),
            Err(e) => gitea_error_response(&state, &e),
        };
    }

    let (git_ref, format, content_type) = if let Some(r) = ref_and_ext.strip_suffix(".tar.gz") {
        (r.to_owned(), ArchiveFormat::TarGz, "application/gzip")
    } else if let Some(r) = ref_and_ext.strip_suffix(".zip") {
        (r.to_owned(), ArchiveFormat::Zip, "application/zip")
    } else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound(
                "unsupported archive format (use .zip, .tar.gz, or .bundle)".into(),
            ),
        );
    };

    let res = tokio::task::spawn_blocking(move || {
        let _hold = permit;
        create_archive(&disk_path, &rname, &git_ref, format)
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type)],
            stream_bytes_in_64k_chunks(bytes),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

// =========================================================================
// Rows 29, 30, 31, 34, 36, 37, 38, 39: Pull Requests, Reviews, Update, Merge
// =========================================================================

pub fn pr_to_gitea_json(repo: &RepoRecord, pr: &PullRequestRecord, mount: &Mount) -> Value {
    let base = mount.public_base();
    let html_url = format!("{base}/{}/{}/pulls/{}", repo.owner, repo.name, pr.number);
    let num_repo_id = repo.numeric_id();
    json!({
        "id": pr.number,
        "number": pr.number,
        "html_url": html_url,
        "state": pr.state,
        "title": pr.title,
        "body": pr.body,
        "head": {
            "ref": pr.head_ref,
            "sha": pr.head_sha,
            "repo_id": num_repo_id,
        },
        "base": {
            "ref": pr.base_ref,
            "sha": pr.base_sha,
            "repo_id": num_repo_id,
        },
        "user": {
            "login": pr.author_login,
            "full_name": pr.author_name,
            "email": pr.author_email,
        },
        "mergeable": pr.mergeable,
        "merged": pr.merged,
        "merge_commit_sha": if pr.merge_commit_sha.is_empty() { Value::Null } else { Value::String(pr.merge_commit_sha.clone()) },
        "merge_base": pr.merge_base,
        "created_at": pr.created_at.to_rfc3339(),
        "updated_at": pr.updated_at.to_rfc3339(),
        "closed_at": pr.closed_at.map(|d| d.to_rfc3339()),
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListPullsQuery {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

async fn compat_list_pulls(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name)): Path<(String, String)>,
    Query(q): Query<ListPullsQuery>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let prs = match state
        .store()
        .list_pull_requests(
            repo.instance_id,
            repo.org_id,
            Some(repo.id),
            q.state.as_deref(),
        )
        .await
    {
        Ok(l) => l,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let total = prs.len();
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let start = (page - 1).saturating_mul(limit);
    let items: Vec<Value> = prs
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|pr| pr_to_gitea_json(&repo, &pr, &mount))
        .collect();
    (
        StatusCode::OK,
        [("x-total-count", total.to_string())],
        Json(items),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePullReq {
    pub head: String,
    pub base: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

async fn compat_create_pull(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name)): Path<(String, String)>,
    Json(req): Json<CreatePullReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };

    // Gitea keeps one open pull request per head and base: a second is a 409.
    // ponytail: two concurrent creates can both pass this read; a store constraint closes it.
    match state
        .store()
        .list_pull_requests(repo.instance_id, repo.org_id, Some(repo.id), Some("open"))
        .await
    {
        Ok(open) if open.iter().any(|pr| pr.head_ref == req.head && pr.base_ref == req.base) => {
            return gitea_error_response(
                &state,
                &ShakoError::Conflict("pull request already exists for these targets".into()),
            );
        }
        Ok(_) => {}
        Err(e) => return gitea_error_response(&state, &e),
    }

    let head = req.head.clone();
    let base = req.base.clone();
    let disk_clone = disk_path.clone();
    let mergeability =
        match tokio::task::spawn_blocking(move || check_mergeability(&disk_clone, &base, &head))
            .await
        {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => return gitea_error_response(&state, &e),
            Err(e) => return gitea_error_response(&state, &ShakoError::Internal(e.to_string())),
        };

    let number = match state
        .store()
        .allocate_pr_number(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(n) => n,
        Err(e) => return gitea_error_response(&state, &e),
    };

    // Write `refs/pull/{number}/head` so it is fetchable by name (Contract S3)
    let head_sha_clone = mergeability.head_sha.clone();
    let disk_clone2 = disk_path.clone();
    let _ = tokio::task::spawn_blocking(move || {
        set_ref_direct(
            &disk_clone2,
            &format!("refs/pull/{number}/head"),
            &head_sha_clone,
        )
    })
    .await;

    let now = Utc::now();
    let pr = PullRequestRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: repo.instance_id,
        org_id: repo.org_id,
        repo_id: repo.id,
        number,
        state: "open".into(),
        title: req.title,
        body: req.body,
        head_ref: req.head,
        head_sha: mergeability.head_sha,
        base_ref: req.base,
        base_sha: mergeability.base_sha,
        merge_base: mergeability.merge_base,
        mergeable: Some(mergeability.mergeable),
        merged: false,
        merge_commit_sha: String::new(),
        author_login: ctx.subject.username.clone(),
        author_name: ctx.subject.full_name.clone(),
        author_email: ctx.subject.email.clone(),
        created_at: now,
        updated_at: now,
        closed_at: None,
    };

    let saved = match state.store().create_pull_request(pr).await {
        Ok(s) => s,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let payload = build_pr_webhook_payload(&repo, &saved, "opened", &ctx.subject.username);
    let _ = emit_and_dispatch_event(
        &state,
        repo.org_id,
        Some(repo.id),
        "pull_request",
        format!("pr_open:{}:{}", repo.id, saved.number),
        payload,
    )
    .await;

    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    (
        StatusCode::CREATED,
        Json(pr_to_gitea_json(&repo, &saved, &mount)),
    )
        .into_response()
}

async fn compat_get_pull(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let Some(mut pr) = (match state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    }) else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("pull request not found".into()),
        );
    };

    // Refresh mergeability if PR is still open
    if pr.state == "open" {
        let base_ref = pr.base_ref.clone();
        let head_ref = pr.head_ref.clone();
        if let Ok(Ok(m)) = tokio::task::spawn_blocking(move || {
            check_mergeability(&disk_path, &base_ref, &head_ref)
        })
        .await
        {
            pr.head_sha = m.head_sha;
            pr.base_sha = m.base_sha;
            pr.merge_base = m.merge_base;
            pr.mergeable = Some(m.mergeable);
            let _ = state.store().update_pull_request(pr.clone()).await;
        }
    }

    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    (StatusCode::OK, Json(pr_to_gitea_json(&repo, &pr, &mount))).into_response()
}

async fn compat_get_pull_files(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
    Query(q): Query<PagingQuery>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let Some(pr) = (match state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    }) else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("pull request not found".into()),
        );
    };

    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.or(q.per_page).unwrap_or(50).clamp(1, 500);
    let base_ref = if !pr.merge_base.is_empty() {
        pr.merge_base.clone()
    } else {
        pr.base_sha.clone()
    };
    let head_ref = pr.head_sha.clone();

    let res =
        tokio::task::spawn_blocking(move || diff_refs_files(&disk_path, &base_ref, &head_ref))
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))
            .and_then(|r| r);

    match res {
        Ok(all_files) => {
            let total = all_files.len();
            let start = (page - 1).saturating_mul(limit);
            let page_items: Vec<_> = all_files.into_iter().skip(start).take(limit).collect();
            (
                StatusCode::OK,
                [("x-total-count", total.to_string())],
                Json(page_items),
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateReviewReq {
    pub event: ReviewVerdict,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub commit_id: Option<String>,
}

async fn compat_list_reviews(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let reviews = match state
        .store()
        .list_reviews(repo.instance_id, repo.org_id, Some(repo.id), Some(number))
        .await
    {
        Ok(r) => r,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let items: Vec<Value> = reviews
        .into_iter()
        .enumerate()
        .map(|(idx, r)| {
            json!({
                "id": (idx + 1) as u64,
                "state": r.verdict.as_str(),
                "body": r.body,
                "commit_id": r.commit_id,
                "user": { "login": r.author_login },
                "submitted_at": r.created_at.to_rfc3339(),
            })
        })
        .collect();
    (StatusCode::OK, Json(items)).into_response()
}

async fn compat_create_review(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
    Json(req): Json<CreateReviewReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let needed = match req.event {
        ReviewVerdict::Comment => Action::Propose,
        ReviewVerdict::Approved | ReviewVerdict::RequestChanges => Action::Approve,
    };
    let (repo, _) = match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, needed).await
    {
        Ok(r) => r,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let Some(pr) = (match state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    }) else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("pull request not found".into()),
        );
    };

    // Contract S10: a subject may not approve its own PR when the protection rule requires approvals
    if req.event == ReviewVerdict::Approved && ctx.subject.username == pr.author_login {
        let policy = state
            .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
            .await
            .unwrap_or_default();
        if let Some(rule) = policy.matching_branch_rule(&pr.base_ref)
            && rule.required_approvals > 0
        {
            return gitea_error_response(
                &state,
                &ShakoError::Unprocessable(
                    "pull request author cannot approve their own pull request".into(),
                ),
            );
        }
    }

    let rec = ReviewRecord {
        id: uuid::Uuid::now_v7(),
        instance_id: repo.instance_id,
        org_id: repo.org_id,
        repo_id: repo.id,
        pr_number: number,
        verdict: req.event,
        body: req.body,
        author_login: ctx.subject.username.clone(),
        commit_id: req.commit_id.unwrap_or_else(|| pr.head_sha.clone()),
        dismissed: false,
        created_at: Utc::now(),
    };
    match state.store().create_review(rec).await {
        Ok(saved) => (
            StatusCode::OK,
            Json(json!({
                "id": 1,
                "state": saved.verdict.as_str(),
                "body": saved.body,
                "commit_id": saved.commit_id,
            })),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchPullReq {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
}

async fn compat_patch_pull(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
    Json(req): Json<PatchPullReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let Some(mut pr) = (match state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    }) else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("pull request not found".into()),
        );
    };

    if let Some(st) = req.state {
        if st == "closed" && pr.state != "closed" {
            pr.state = "closed".into();
            pr.closed_at = Some(Utc::now());
        } else if st == "open" && !pr.merged {
            pr.state = "open".into();
            pr.closed_at = None;
        }
    }
    if let Some(t) = req.title {
        pr.title = t;
    }
    if let Some(b) = req.body {
        pr.body = b;
    }
    if let Some(base) = req.base {
        pr.base_ref = base;
    }

    match state.store().update_pull_request(pr).await {
        Ok(saved) => {
            let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
            (
                StatusCode::OK,
                Json(pr_to_gitea_json(&repo, &saved, &mount)),
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePullQuery {
    #[serde(default)]
    pub style: Option<String>,
}

async fn compat_update_pull_branch(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
    Query(q): Query<UpdatePullQuery>,
) -> Response {
    if let Some(st) = &q.style
        && st != "merge"
    {
        return gitea_error_response(
            &state,
            &ShakoError::Unprocessable(format!("update style '{st}' is not supported")),
        );
    }
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let Some(mut pr) = (match state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    }) else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("pull request not found".into()),
        );
    };

    let permit = match state
        .memory_budget
        .acquire(MERGE_OPERATION_WEIGHT_BYTES)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let _guard = state.repo_locks.lock(repo.id).await;
    let base_ref = pr.base_ref.clone();
    let head_ref = pr.head_ref.clone();
    let author = CommitAuthor {
        name: ctx.subject.full_name.clone(),
        email: ctx
            .subject
            .email
            .clone()
            .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
    };
    let disk_clone = disk_path.clone();

    let res = tokio::task::spawn_blocking(move || {
        let _hold = permit;
        let new_head = update_pr_branch(&disk_clone, &base_ref, &head_ref, &author)?;
        let m = check_mergeability(&disk_clone, &base_ref, &head_ref)?;
        Ok::<_, ShakoError>((new_head, m))
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok((new_head, m)) => {
            pr.head_sha = new_head;
            pr.base_sha = m.base_sha;
            pr.merge_base = m.merge_base;
            pr.mergeable = Some(m.mergeable);
            let _ = state.store().update_pull_request(pr).await;
            (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergePullReq {
    #[serde(rename = "Do")]
    pub do_field: String,
    #[serde(default)]
    pub merge_message_field: Option<String>,
    #[serde(default)]
    pub merge_title_field: Option<String>,
    #[serde(default)]
    pub head_commit_id: Option<String>,
    #[serde(default)]
    pub delete_branch_after_merge: Option<bool>,
}

async fn compat_merge_pull(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
    Json(req): Json<MergePullReq>,
) -> Response {
    let style = match MergeStyle::parse(&req.do_field) {
        Ok(s) => s,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (mut repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Write).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let Some(mut pr) = (match state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    }) else {
        return gitea_error_response(
            &state,
            &ShakoError::NotFound("pull request not found".into()),
        );
    };

    if pr.merged || pr.state == "closed" {
        return gitea_error_response(
            &state,
            &ShakoError::Conflict("pull request is already closed or merged".into()),
        );
    }

    let permit = match state
        .memory_budget
        .acquire(MERGE_OPERATION_WEIGHT_BYTES)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let _guard = state.repo_locks.lock(repo.id).await;
    let base_ref = pr.base_ref.clone();
    let head_ref = pr.head_ref.clone();
    let disk_clone = disk_path.clone();

    let mergeability = match tokio::task::spawn_blocking(move || {
        check_mergeability(&disk_clone, &base_ref, &head_ref)
    })
    .await
    {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => return gitea_error_response(&state, &e),
        Err(e) => return gitea_error_response(&state, &ShakoError::Internal(e.to_string())),
    };

    // Check `head_commit_id` precondition BEFORE ref policy so a moved head returns 409 (Contract S7, S10, T10)
    if let Some(expected) = req.head_commit_id.as_deref().filter(|s| !s.is_empty())
        && expected != mergeability.head_sha
    {
        return gitea_error_response(
            &state,
            &ShakoError::Conflict("head commit has moved since review".into()),
        );
    }

    let policy = match state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let reviews = state
        .store()
        .list_reviews(repo.instance_id, repo.org_id, Some(repo.id), Some(number))
        .await
        .unwrap_or_default();
    let statuses = state
        .store()
        .list_commit_statuses(
            repo.instance_id,
            repo.org_id,
            repo.id,
            &mergeability.head_sha,
        )
        .await
        .unwrap_or_default();

    if let Err(e) = policy.check_pr_merge(
        &ctx,
        state.authorizer.as_ref(),
        &repo.owner,
        &repo.name,
        &pr.base_ref,
        &MergePolicyInput {
            pr_author_login: &pr.author_login,
            head_sha: &mergeability.head_sha,
            is_behind_base: mergeability.is_behind_base,
            reviews: &reviews,
            statuses: &statuses,
        },
    ) {
        return gitea_error_response(&state, &e);
    }

    let message = req
        .merge_message_field
        .or(req.merge_title_field)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Merge pull request #{number}: {}", pr.title));
    let author = CommitAuthor {
        name: ctx.subject.full_name.clone(),
        email: ctx
            .subject
            .email
            .clone()
            .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
    };
    let base_ref2 = pr.base_ref.clone();
    let head_ref2 = pr.head_ref.clone();
    let expected_head = req.head_commit_id.clone();
    let disk_clone2 = disk_path.clone();

    let merge_res = tokio::task::spawn_blocking(move || {
        let _hold = permit;
        execute_pr_merge(
            &disk_clone2,
            &base_ref2,
            &head_ref2,
            &MergeOptions {
                expected_head_sha: expected_head.as_deref(),
                style,
                message: &message,
                author: &author,
                committer: &author,
            },
        )
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match merge_res {
        Ok(merge_commit_sha) => {
            if req.delete_branch_after_merge == Some(true) {
                let d_path = disk_path.clone();
                let h_ref = pr.head_ref.clone();
                let _ = tokio::task::spawn_blocking(move || delete_branch(&d_path, &h_ref)).await;
            }
            pr.state = "closed".into();
            pr.merged = true;
            pr.mergeable = None;
            pr.head_sha = mergeability.head_sha;
            pr.base_sha = mergeability.base_sha;
            pr.merge_base = mergeability.merge_base;
            pr.merge_commit_sha = merge_commit_sha.clone();
            pr.closed_at = Some(Utc::now());
            let saved = match state.store().update_pull_request(pr).await {
                Ok(s) => s,
                Err(e) => return gitea_error_response(&state, &e),
            };
            repo.size_bytes = compute_repo_size_bytes(&disk_path);
            let _ = state.store().update_repo(repo.clone()).await;

            let payload = build_pr_webhook_payload(&repo, &saved, "closed", &ctx.subject.username);
            let _ = emit_and_dispatch_event(
                &state,
                repo.org_id,
                Some(repo.id),
                "pull_request",
                format!("pr_merge:{}:{}", repo.id, saved.number),
                payload,
            )
            .await;
            sync_repo_mirrors_on_commit(&state, &repo, &disk_path).await;

            (StatusCode::OK, Json(json!({ "sha": merge_commit_sha }))).into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

// =========================================================================
// Webhooks CRUD
// =========================================================================

/// A hook as `POST /repos/{owner}/{repo}/hooks` takes it: Gitea's own shape
/// (`{type: "gitea", config: {url, content_type: "json", secret}, events, active}`), or the
/// native one naming a secret already in the secret store (`{url, secret_ref}`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookReq {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub previous_secret_ref: Option<String>,
    #[serde(default, rename = "type")]
    pub hook_type: Option<String>,
    #[serde(default)]
    pub config: Option<GiteaHookConfig>,
    #[serde(default)]
    pub branch_filter: Option<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default = "default_true")]
    pub active: bool,
}

/// Gitea's `CreateHookOptionConfig`: only a JSON body is delivered, so only `json` is taken.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GiteaHookConfig {
    pub url: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub http_method: Option<String>,
}

/// The hook's URL and the reference its secret is kept under, or why the request is refused.
/// A Gitea-shaped hook's inline secret goes into the secret store under `webhook-{id}`; an
/// unsigned hook is refused, because every receiver this forge serves checks the signature.
fn webhook_target(
    state: &ShakoState,
    org_id: OrgId,
    id: WebhookId,
    req: &CreateWebhookReq,
) -> ShakoResult<(String, String)> {
    // Every push is delivered; a filter this forge would drop silently is refused instead.
    if req
        .branch_filter
        .as_deref()
        .is_some_and(|f| !f.is_empty() && f != "*")
    {
        return Err(ShakoError::Unprocessable(
            "branch_filter is not served; leave it empty or *".into(),
        ));
    }
    match (&req.config, &req.url, &req.secret_ref) {
        (Some(config), None, None) => {
            if req.hook_type.as_deref().is_some_and(|t| t != "gitea") {
                return Err(ShakoError::Unprocessable(
                    "only hooks of type gitea are served".into(),
                ));
            }
            if config.content_type.as_deref().is_some_and(|t| t != "json") {
                return Err(ShakoError::Unprocessable(
                    "only content_type json is served".into(),
                ));
            }
            if config
                .http_method
                .as_deref()
                .is_some_and(|m| !m.eq_ignore_ascii_case("post"))
            {
                return Err(ShakoError::Unprocessable(
                    "only http_method post is served".into(),
                ));
            }
            let secret = config
                .secret
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    ShakoError::Unprocessable("a hook needs a secret to sign its deliveries".into())
                })?;
            let secret_ref = format!("webhook-{id}");
            state.secret_store.put_secret(org_id, &secret_ref, secret);
            Ok((config.url.clone(), secret_ref))
        }
        (None, Some(url), Some(secret_ref)) if req.hook_type.is_none() => {
            Ok((url.clone(), secret_ref.clone()))
        }
        _ => Err(ShakoError::Unprocessable(
            "a hook is either {type, config: {url, secret}} or {url, secret_ref}".into(),
        )),
    }
}

async fn compat_create_webhook(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
    Json(req): Json<CreateWebhookReq>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let hook_url = req.config.as_ref().map(|c| c.url.as_str()).or(req.url.as_deref());
    if let Err(e) = state.outbound_guard.validate_url(hook_url.unwrap_or_default(), None) {
        return gitea_error_response(&state, &e);
    }
    let id = WebhookId::new();
    let (url, secret_ref) = match webhook_target(&state, repo.org_id, id, &req) {
        Ok(t) => t,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let gitea_shape = req.config.is_some();
    let hook = WebhookRecord {
        id,
        instance_id: repo.instance_id,
        org_id: repo.org_id,
        repo_id: repo.id,
        url,
        events: if req.events.is_empty() {
            vec!["push".into(), "pull_request".into()]
        } else {
            req.events
        },
        active: req.active,
        secret_ref,
        previous_secret_ref: req.previous_secret_ref,
        created_at: Utc::now(),
    };
    match state.store().create_webhook(hook).await {
        Ok(saved) if gitea_shape => (
            StatusCode::CREATED,
            Json(json!({
                "id": saved.id,
                "type": "gitea",
                "config": { "url": saved.url, "content_type": "json" },
                "events": saved.events,
                "active": saved.active,
                "branch_filter": req.branch_filter.unwrap_or_default(),
                "created_at": saved.created_at,
                "updated_at": saved.created_at,
            })),
        )
            .into_response(),
        Ok(saved) => (
            StatusCode::CREATED,
            Json(json!({
                "id": saved.id,
                "url": saved.url,
                "events": saved.events,
                "active": saved.active,
            })),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn compat_redeliver_webhook(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, hook_id)): Path<(String, String, WebhookId)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (repo, _) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Admin).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };
    let Some(hook) = (match state
        .store()
        .get_webhook(repo.instance_id, repo.org_id, hook_id)
        .await
    {
        Ok(h) => h,
        Err(e) => return gitea_error_response(&state, &e),
    }) else {
        return gitea_error_response(&state, &ShakoError::NotFound("webhook not found".into()));
    };
    let payload =
        build_push_webhook_payload(&repo, "refs/heads/main", "", "", &ctx.subject.username);
    match deliver_webhook_with_retries(&state, &hook, "push", &payload).await {
        Ok(del) => (StatusCode::OK, Json(del)).into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

/// Builds the Gitea REST compatibility router (`/api/v1/...`, S7) and native REST routes (`/api/v1/projects/{project}/...`, S8).
pub fn compat_and_native_rest_router() -> Router<ShakoState> {
    Router::new()
        .route("/api/v1/version", get(api_version))
        .route("/api/healthz", get(api_healthz))
        // Row 16: User & Org permissions
        .route("/api/v1/user", get(compat_get_current_user))
        .route(
            "/api/v1/users/{login}/orgs/{org}/permissions",
            get(compat_get_user_org_permissions),
        )
        // Row 17: Teams (transition no-ops)
        .route(
            "/api/v1/orgs/{org}/teams",
            get(compat_list_org_teams).post(compat_create_org_team),
        )
        .route("/api/v1/teams/{id}", delete(compat_delete_team))
        .route("/api/v1/teams/{id}/repos", get(compat_list_team_repos))
        .route(
            "/api/v1/teams/{id}/repos/{org}/{repo}",
            put(compat_put_or_delete_team_repo).delete(compat_put_or_delete_team_repo),
        )
        // Row 2 & 3: Org Repo create & Migrate
        .route("/api/v1/orgs/{owner}/repos", post(compat_create_org_repo))
        .route("/api/v1/repos/migrate", post(compat_migrate_repo))
        // Row 1, 15, 18: Repo get, patch, delete
        .route(
            "/api/v1/repos/{owner}/{repo}",
            get(compat_get_repo)
                .patch(compat_patch_repo)
                .delete(compat_delete_repo),
        )
        // Row 19: Collaborators (transition no-op)
        .route(
            "/api/v1/repos/{owner}/{repo}/collaborators/{reader}",
            put(compat_put_collaborator),
        )
        // Row 5: Push Mirrors
        .route(
            "/api/v1/repos/{owner}/{repo}/push_mirrors",
            get(compat_list_push_mirrors).post(compat_add_push_mirror),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/push_mirrors/{name}",
            delete(compat_delete_push_mirror),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/push_mirrors-sync",
            post(compat_sync_push_mirrors),
        )
        // Rows 6-11: Actions (404 with Gitea error JSON until S16)
        .route(
            "/api/v1/repos/{owner}/{repo}/actions/runs",
            get(actions_not_served_yet),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/actions/workflows/{file}/dispatches",
            post(actions_not_served_yet),
        )
        .route(
            "/api/v1/orgs/{owner}/actions/jobs",
            get(actions_not_served_yet),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/actions/runners/registration-token",
            post(actions_not_served_yet),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/actions/artifacts",
            get(actions_not_served_yet),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/actions/artifacts/{id}/zip",
            get(actions_not_served_yet),
        )
        // Row 20: Branch Protections
        .route(
            "/api/v1/repos/{owner}/{repo}/branch_protections",
            get(compat_list_branch_protections).post(compat_create_branch_protection),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/branch_protections/{*name}",
            get(compat_get_branch_protection)
                .patch(compat_patch_branch_protection)
                .delete(compat_delete_branch_protection),
        )
        // Row 21: Git Trees
        .route(
            "/api/v1/repos/{owner}/{repo}/git/trees/{*git_ref}",
            get(compat_get_git_tree),
        )
        // Row 22 & 24: Branches (note: `{*branch}` supports slashes like `workspace/air-v2`)
        .route(
            "/api/v1/repos/{owner}/{repo}/branches",
            post(compat_create_branch),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/branches/{*branch}",
            get(compat_get_branch).delete(compat_delete_branch),
        )
        // Row 23: Commit status
        .route(
            "/api/v1/repos/{owner}/{repo}/commits/{sha}/status",
            get(compat_get_commit_status),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/statuses/{sha}",
            post(compat_post_commit_status),
        )
        // Rows 25, 26, 27: Contents
        .route(
            "/api/v1/repos/{owner}/{repo}/contents",
            post(compat_post_contents_multi),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/contents/{*path}",
            get(compat_get_contents)
                .put(compat_put_contents)
                .delete(compat_delete_contents),
        )
        // Rows 28, 35: Commits
        .route(
            "/api/v1/repos/{owner}/{repo}/commits",
            get(compat_list_commits),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/git/commits/{sha}",
            get(compat_get_git_commit),
        )
        // Row 32: Archives & Bundles
        .route(
            "/api/v1/repos/{owner}/{repo}/archive/{*ref_and_ext}",
            get(compat_get_archive),
        )
        // The page path Gitea serves the same archive at, which a SyncSource's git origin reads
        // (`portal:src/sync/remote.rs` archive_url)
        .route(
            "/{owner}/{repo}/archive/{*ref_and_ext}",
            get(compat_get_archive),
        )
        // Row 33: Tags
        .route("/api/v1/repos/{owner}/{repo}/tags", get(compat_list_tags))
        // Rows 29, 30, 31, 34, 36, 37, 38, 39: Pull Requests
        .route(
            "/api/v1/repos/{owner}/{repo}/pulls",
            get(compat_list_pulls).post(compat_create_pull),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/pulls/{number}",
            get(compat_get_pull).patch(compat_patch_pull),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/pulls/{number}/files",
            get(compat_get_pull_files),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/pulls/{number}/reviews",
            get(compat_list_reviews).post(compat_create_review),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/pulls/{number}/update",
            post(compat_update_pull_branch),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/pulls/{number}/merge",
            post(compat_merge_pull),
        )
        // Webhooks
        .route(
            "/api/v1/repos/{owner}/{repo}/hooks",
            post(compat_create_webhook),
        )
        .route(
            "/api/v1/repos/{owner}/{repo}/hooks/{id}/tests",
            post(compat_redeliver_webhook),
        )
        // Native REST under `/api/v1/projects/{project}/repositories/...` (Contract S8)
        .route(
            "/api/v1/projects/{owner}/repositories",
            post(compat_create_org_repo),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}",
            get(compat_get_repo)
                .patch(compat_patch_repo)
                .delete(compat_delete_repo),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/branches",
            post(compat_create_branch),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/branches/{*branch}",
            get(compat_get_branch).delete(compat_delete_branch),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/contents",
            post(compat_post_contents_multi),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/contents/{*path}",
            get(compat_get_contents)
                .put(compat_put_contents)
                .delete(compat_delete_contents),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/pulls",
            get(compat_list_pulls).post(compat_create_pull),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/pulls/{number}",
            get(compat_get_pull).patch(compat_patch_pull),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/pulls/{number}/reviews",
            get(compat_list_reviews).post(compat_create_review),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/pulls/{number}/merge",
            post(compat_merge_pull),
        )
        .route(
            "/api/v1/projects/{owner}/repositories/{repo}/pulls/{number}/update",
            post(compat_update_pull_branch),
        )
}
