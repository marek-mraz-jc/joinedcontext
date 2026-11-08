#![forbid(unsafe_code)]
//! Git Smart HTTP routes (`GET info/refs?service=git-upload-pack|git-receive-pack`,
//! `POST git-upload-pack`, `POST git-receive-pack`) with `.git` suffix optional on the path,
//! running blocking `gix` pack work on `spawn_blocking` under `MemoryBudget` and streaming
//! 64 KiB chunks to the response body (Contract S3, S4, S22).

use crate::outbox::{build_push_webhook_payload, emit_and_dispatch_event};
use crate::state::{
    ShakoState, authenticate_request, gitea_error_response, read_body_bounded,
    resolve_and_authorize_repo, stream_bytes_in_64k_chunks,
};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use shako_core::{Action, PACK_OPERATION_WEIGHT_BYTES, ShakoError, remaining_storage_allowance};
use shako_git::{
    PushGuard, advertise_receive_pack, advertise_upload_pack, compute_repo_size_bytes,
    handle_receive_pack, handle_upload_pack, push_mirror_over_https,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfoRefsQuery {
    pub service: Option<String>,
}

async fn handle_info_refs(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_seg)): Path<(String, String)>,
    Query(q): Query<InfoRefsQuery>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let Some(service) = q.service.as_deref() else {
        // Dumb HTTP is not served (Contract S3, gitprotocol-http.txt: 403 Forbidden for disabled service)
        return gitea_error_response(
            &state,
            &ShakoError::Forbidden("dumb HTTP protocol is disabled; use smart HTTP".into()),
        );
    };

    let required_action = match service {
        "git-upload-pack" => Action::Read,
        "git-receive-pack" => Action::Propose,
        _ => {
            return gitea_error_response(
                &state,
                &ShakoError::Forbidden(format!("unsupported git service '{service}'")),
            );
        }
    };

    let (_repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_seg, required_action).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };

    let git_protocol = headers
        .get("git-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let service_owned = service.to_owned();
    let res = tokio::task::spawn_blocking(move || match service_owned.as_str() {
        "git-upload-pack" => advertise_upload_pack(&disk_path, git_protocol.as_deref()),
        "git-receive-pack" => advertise_receive_pack(&disk_path),
        _ => Err(ShakoError::Forbidden("unsupported service".into())),
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(bytes) => {
            let content_type = format!("application/x-{service}-advertisement");
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type.as_str()),
                    (
                        header::CACHE_CONTROL,
                        "no-cache, max-age=0, must-revalidate",
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn handle_post_upload_pack(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_seg)): Path<(String, String)>,
    body: Body,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (_repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_seg, Action::Read).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };

    let permit = match state
        .memory_budget
        .acquire(PACK_OPERATION_WEIGHT_BYTES)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let req_bytes = match read_body_bounded(body, 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let git_protocol = headers
        .get("git-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let res = tokio::task::spawn_blocking(move || {
        let _hold = permit;
        handle_upload_pack(&disk_path, git_protocol.as_deref(), &req_bytes)
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    match res {
        Ok(out_bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/x-git-upload-pack-result"),
                (
                    header::CACHE_CONTROL,
                    "no-cache, max-age=0, must-revalidate",
                ),
            ],
            stream_bytes_in_64k_chunks(out_bytes),
        )
            .into_response(),
        Err(e) => gitea_error_response(&state, &e),
    }
}

async fn handle_post_receive_pack(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_seg)): Path<(String, String)>,
    body: Body,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, Some(&owner), "shako") {
        Ok(c) => c,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let (mut repo, disk_path) =
        match resolve_and_authorize_repo(&state, &ctx, &owner, &repo_seg, Action::Propose).await {
            Ok(r) => r,
            Err(e) => return gitea_error_response(&state, &e),
        };

    let permit = match state
        .memory_budget
        .acquire(PACK_OPERATION_WEIGHT_BYTES)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    let instance = state.instance_for(ctx.instance_id, ctx.org_id).await;
    let usage = match state
        .store()
        .get_quota_usage(ctx.instance_id, ctx.org_id)
        .await
    {
        Ok(u) => u,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let max_push_bytes = remaining_storage_allowance(&instance, &usage).unwrap_or(0);

    // Stream request body up to `max_push_bytes + 64 KiB` (if exceeded while streaming, we still
    // read the command lines at the front if possible or return an unpack quota error).
    let stream_cap = (instance.max_push_bytes as usize).saturating_add(256 * 1024);
    let req_bytes = match read_body_bounded(body, stream_cap).await {
        Ok(b) => b,
        Err(ShakoError::QuotaExceeded { .. }) => {
            let mut out = Vec::new();
            let _ = shako_git::write_text_line(&mut out, "unpack error quota");
            let _ = shako_git::write_flush(&mut out);
            return (
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        "application/x-git-receive-pack-result",
                    ),
                    (header::CACHE_CONTROL, "no-cache"),
                ],
                out,
            )
                .into_response();
        }
        Err(e) => return gitea_error_response(&state, &e),
    };

    let policy = match state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await
    {
        Ok(p) => p,
        Err(e) => return gitea_error_response(&state, &e),
    };

    // Acquire per-repository write lock for the entire receive-pack + ref transaction (S4)
    let _repo_guard = state.repo_locks.lock(repo.id).await;
    let authz = state.authorizer.clone();
    let ctx_clone = ctx.clone();
    let owner_clone = repo.owner.clone();
    let name_clone = repo.name.clone();
    let disk_clone = disk_path.clone();
    let secret_scan = instance.secret_scan_enabled;

    let res = tokio::task::spawn_blocking(move || {
        let _hold = permit;
        handle_receive_pack(
            &disk_clone,
            &PushGuard {
                ctx: &ctx_clone,
                authorizer: authz.as_ref(),
                owner: &owner_clone,
                repo_name: &name_clone,
                policy: &policy,
                secret_scan_enabled: secret_scan,
            },
            max_push_bytes,
            &req_bytes,
        )
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))
    .and_then(|r| r);

    let outcome = match res {
        Ok(o) => o,
        Err(e) => return gitea_error_response(&state, &e),
    };

    if !outcome.updated_refs.is_empty() {
        let new_size = compute_repo_size_bytes(&disk_path);
        let delta = new_size as i64 - repo.size_bytes as i64;
        repo.size_bytes = new_size;
        let _ = state.store().update_repo(repo.clone()).await;
        let _ = state
            .store()
            .adjust_quota_usage(repo.instance_id, repo.org_id, 0, delta, 0)
            .await;

        for cmd in &outcome.updated_refs {
            let payload = build_push_webhook_payload(
                &repo,
                &cmd.refname,
                &cmd.old_oid.to_string(),
                &cmd.new_oid.to_string(),
                &ctx.subject.username,
            );
            let idem = format!("push:{}:{}:{}", repo.id, cmd.refname, cmd.new_oid);
            let _ =
                emit_and_dispatch_event(&state, repo.org_id, Some(repo.id), "push", idem, payload)
                    .await;
        }

        // Sync any push mirrors with `sync_on_commit = true` (S15)
        sync_repo_mirrors_on_commit(&state, &repo, &disk_path).await;
    }

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/x-git-receive-pack-result",
            ),
            (
                header::CACHE_CONTROL,
                "no-cache, max-age=0, must-revalidate",
            ),
        ],
        outcome.response_bytes,
    )
        .into_response()
}

pub async fn sync_repo_mirrors_on_commit(
    state: &ShakoState,
    repo: &shako_core::RepoRecord,
    disk_path: &std::path::Path,
) {
    let Ok(mirrors) = state
        .store()
        .list_push_mirrors(repo.instance_id, repo.org_id, Some(repo.id))
        .await
    else {
        return;
    };
    for mut m in mirrors {
        if !m.sync_on_commit {
            continue;
        }
        let cred = state
            .secret_store
            .get_secret(repo.org_id, &m.credential_secret_ref)
            .unwrap_or_default();
        let (user, pass) = cred
            .split_once(':')
            .map_or(("x-access-token", cred.as_str()), |(u, p)| (u, p));
        match push_mirror_over_https(
            disk_path,
            &m.remote_address,
            user,
            pass,
            &state.outbound_guard,
            None,
        )
        .await
        {
            Ok(()) => {
                m.last_sync = Some(chrono::Utc::now());
                m.last_error = String::new();
            }
            Err(e) => {
                m.last_error = e.public_detail();
            }
        }
        let _ = state.store().upsert_push_mirror(m).await;
    }
}

/// Builds the Git Smart HTTP router (`/{owner}/{repo}/info/refs`, `/{owner}/{repo}/git-upload-pack`,
/// `/{owner}/{repo}/git-receive-pack`, with `.git` suffix optional on `{repo}`, S3 & S4).
pub fn smart_http_router() -> Router<ShakoState> {
    Router::new()
        .route("/{owner}/{repo}/info/refs", get(handle_info_refs))
        .route(
            "/{owner}/{repo}/git-upload-pack",
            post(handle_post_upload_pack),
        )
        .route(
            "/{owner}/{repo}/git-receive-pack",
            post(handle_post_receive_pack),
        )
}
