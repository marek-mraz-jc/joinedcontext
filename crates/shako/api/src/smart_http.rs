#![forbid(unsafe_code)]
//! Git Smart HTTP routes (`GET info/refs?service=git-upload-pack|git-receive-pack`,
//! `POST git-upload-pack`, `POST git-receive-pack`) with `.git` suffix optional on the path,
//! spooling bodies and packs to disk, running `gix` on `spawn_blocking` under a memory permit
//! and streaming 64 KiB chunks to the response body (Contract S3, S4, S22, `crate::work`).

use crate::outbox::{build_push_webhook_payload, emit_and_dispatch_event};
use crate::state::{
    ShakoState, authenticate_request, gitea_error_response, resolve_and_authorize_repo,
};
use crate::work::Spool;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use shako_core::{Action, RequestCtx, ShakoError, ShakoResult, remaining_storage_allowance};
use shako_git::{
    PushGuard, advertise_receive_pack, advertise_upload_pack, handle_receive_pack,
    handle_upload_pack, push_mirror_over_https, read_negotiation,
};
use std::time::Duration;

/// An upload-pack request, before and after its gzip.
const MAX_NEGOTIATION_BYTES: u64 = 16 * 1024 * 1024;
/// How long a push waits behind another write to the same repository.
const REPO_LOCK_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfoRefsQuery {
    pub service: Option<String>,
}

/// A produced spool as an uncached Git response.
async fn answer(
    state: &ShakoState,
    produced: ShakoResult<(Spool, ())>,
    content_type: &'static str,
) -> Response {
    let sent = match produced {
        Ok((spool, ())) => spool.respond(content_type).await,
        Err(e) => Err(e),
    };
    match sent {
        Ok(mut response) => {
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache, max-age=0, must-revalidate"),
            );
            response
        }
        Err(e) => gitea_error_response(state, &e),
    }
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

    let spool = match state
        .git_work
        .receive(ctx.org_id, &headers, body, MAX_NEGOTIATION_BYTES)
        .await
    {
        Ok(spool) => spool,
        Err(e) => return gitea_error_response(&state, &e),
    };
    let gzip = headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"gzip"));
    let git_protocol = headers
        .get("git-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let res = state
        .git_work
        .produce(ctx.org_id, move |out, admit| {
            let request = read_negotiation(spool.file(), gzip, MAX_NEGOTIATION_BYTES)?;
            handle_upload_pack(&disk_path, git_protocol.as_deref(), &request, out, admit)
        })
        .await;
    answer(&state, res, "application/x-git-upload-pack-result").await
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
    if let Err(e) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_seg, Action::Propose).await
    {
        return gitea_error_response(&state, &e);
    }
    let instance = match state.instance_for(ctx.instance_id, ctx.org_id).await {
        Ok(instance) => instance,
        Err(e) => return gitea_error_response(&state, &e),
    };

    // The cap on the body is `max_push_bytes` plus the commands in front of the pack; what is
    // left of the storage quota is judged on the pack, so its refusal names each ref.
    let stream_cap = instance
        .max_push_bytes
        .saturating_add(shako_git::MAX_COMMAND_BYTES as u64);
    let spool = match state
        .git_work
        .receive(ctx.org_id, &headers, body, stream_cap)
        .await
    {
        Ok(spool) => spool,
        Err(ShakoError::QuotaExceeded { .. }) => {
            let mut out = Vec::new();
            let _ = shako_git::write_text_line(&mut out, "unpack error quota");
            let _ = shako_git::write_flush(&mut out);
            return receive_pack_result(out);
        }
        Err(e) => return gitea_error_response(&state, &e),
    };

    // A task of its own: a client that leaves does not release the repository lock under a
    // running write, nor skip the size, the events and the mirrors of refs that moved.
    let task = tokio::spawn({
        let state = state.clone();
        async move { receive_push(&state, ctx, &owner, &repo_seg, spool).await }
    });
    match task
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))
        .and_then(|r| r)
    {
        Ok(out) => receive_pack_result(out),
        Err(e) => gitea_error_response(&state, &e),
    }
}

fn receive_pack_result(out: Vec<u8>) -> Response {
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
        out,
    )
        .into_response()
}

/// The write half of a push, to its end whether the client waits or not.
async fn receive_push(
    state: &ShakoState,
    ctx: RequestCtx,
    owner: &str,
    repo_seg: &str,
    spool: Spool,
) -> ShakoResult<Vec<u8>> {
    let (seen, _) =
        resolve_and_authorize_repo(state, &ctx, owner, repo_seg, Action::Propose).await?;
    // Acquire per-repository write lock for the entire receive-pack + ref transaction (S4)
    let repo_guard = tokio::time::timeout(REPO_LOCK_WAIT, state.repo_locks.lock(seen.id))
        .await
        .map_err(|_| ShakoError::Overloaded("another write holds the repository".into()))?;
    // The body may have taken minutes: the repository, the right and the rules are read again.
    let (mut repo, disk_path) =
        resolve_and_authorize_repo(state, &ctx, owner, repo_seg, Action::Propose).await?;
    if repo.id != seen.id {
        return Err(ShakoError::Conflict(
            "the repository was replaced during the push; push again".into(),
        ));
    }
    let instance = state.instance_for(ctx.instance_id, ctx.org_id).await?;
    let usage = state
        .store()
        .get_quota_usage(ctx.instance_id, ctx.org_id)
        .await?;
    let max_push_bytes = remaining_storage_allowance(&instance, &usage).unwrap_or(0);
    let policy = state
        .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
        .await?;

    let outcome = state
        .git_work
        .run(ctx.org_id, {
            let (authz, ctx) = (state.authorizer.clone(), ctx.clone());
            let (owner, name) = (repo.owner.clone(), repo.name.clone());
            let disk_path = disk_path.clone();
            move |admit| {
                handle_receive_pack(
                    &disk_path,
                    &PushGuard {
                        ctx: &ctx,
                        authorizer: authz.as_ref(),
                        owner: &owner,
                        repo_name: &name,
                        policy: &policy,
                    },
                    max_push_bytes,
                    spool.file(),
                    admit,
                )
            }
        })
        .await?;

    if !outcome.updated_refs.is_empty() {
        crate::service::record_repo_size(state, &mut repo, &disk_path).await;

        for cmd in &outcome.updated_refs {
            let payload = build_push_webhook_payload(
                &repo,
                &cmd.refname,
                &cmd.old_oid.to_string(),
                &cmd.new_oid.to_string(),
                &ctx.subject.username,
            );
            // Each ref move is one event: a branch can return to a commit it held before.
            let idem = format!(
                "push:{}:{}:{}:{}",
                repo.id,
                cmd.refname,
                cmd.new_oid,
                uuid::Uuid::now_v7()
            );
            let _ =
                emit_and_dispatch_event(state, repo.org_id, Some(repo.id), "push", idem, payload)
                    .await;
        }
        drop(repo_guard);

        // Sync any push mirrors with `sync_on_commit = true` (S15)
        sync_repo_mirrors_on_commit(state, &repo, &disk_path).await;
    }
    Ok(outcome.response_bytes)
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
        if !shako_core::is_secret_ref(&m.credential_secret_ref) {
            // Stored before the API refused non-key names: still used, never logged.
            tracing::warn!(mirror = %m.id, "a push mirror's secret reference is not a secret-store key name; add the mirror again");
        }
        let cred = match state
            .secret_store
            .get_secret(repo.org_id, &m.credential_secret_ref)
            .await
        {
            Ok(held) => held.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(mirror = %m.id, error = %e, "a push mirror's secret could not be read; not pushed");
                m.last_error =
                    "the mirror's credential could not be read; the next push tries again".into();
                let _ = state.store().upsert_push_mirror(m).await;
                continue;
            }
        };
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
            Some(m.branch.as_str()).filter(|b| !b.is_empty()),
        )
        .await
        {
            Ok(()) => {
                m.last_sync = Some(jc_time::TimestampTz::now());
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
