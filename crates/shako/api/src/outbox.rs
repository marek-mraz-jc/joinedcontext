#![forbid(unsafe_code)]
//! Transactional outbox event recording, in-process bus dispatch, and Gitea-compatible
//! outbound webhook delivery with HMAC-SHA256 (`X-Gitea-Signature`, `X-Gitea-Event`,
//! `X-Gitea-Delivery`) and 3 retries through `OutboundGuard` (Contract S14, R§5, Threat T3, T11).

use crate::state::ShakoState;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use shako_core::{
    DeliveryRecord, OutboxEvent, PullRequestRecord, RepoId, RepoRecord, ShakoError, ShakoResult,
    WebhookRecord,
};

type HmacSha256 = Hmac<Sha256>;

/// Computes lowercase hex HMAC-SHA256 signature of `body` with `secret` (`X-Gitea-Signature`, S14).
pub fn compute_gitea_signature(secret: &str, body: &[u8]) -> ShakoResult<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| ShakoError::Internal(format!("hmac init: {e}")))?;
    mac.update(body);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Verifies a hex HMAC-SHA256 signature in constant time (`Mac::verify_slice`, Threat T11).
#[must_use]
pub fn verify_gitea_signature(secret: &str, body: &[u8], presented_hex: &str) -> bool {
    let Ok(expected) = hex::decode(presented_hex.trim()) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// Builds a Gitea-compatible `push` webhook payload (`portal:src/api/webhook.rs:61-77`).
#[must_use]
pub fn build_push_webhook_payload(
    repo: &RepoRecord,
    git_ref: &str,
    before_sha: &str,
    after_sha: &str,
    pusher_login: &str,
) -> Value {
    json!({
        "ref": git_ref,
        "before": before_sha,
        "after": after_sha,
        "repository": {
            "id": repo.numeric_id(),
            "name": repo.name,
            "full_name": format!("{}/{}", repo.owner, repo.name),
            "default_branch": repo.default_branch,
            "private": repo.private,
            "owner": {
                "login": repo.owner,
            }
        },
        "pusher": {
            "login": pusher_login,
            "username": pusher_login,
        },
        "sender": {
            "login": pusher_login,
            "username": pusher_login,
        }
    })
}

/// Builds a Gitea-compatible `pull_request` webhook payload (`portal:src/api/webhook.rs:51-59`).
#[must_use]
pub fn build_pr_webhook_payload(
    repo: &RepoRecord,
    pr: &PullRequestRecord,
    action: &str,
    actor_login: &str,
) -> Value {
    json!({
        "action": action,
        "number": pr.number,
        "pull_request": {
            "id": pr.number,
            "number": pr.number,
            "state": pr.state,
            "title": pr.title,
            "body": pr.body,
            "merged": pr.merged,
            "merge_commit_sha": pr.merge_commit_sha,
            "head": {
                "ref": pr.head_ref,
                "sha": pr.head_sha,
            },
            "base": {
                "ref": pr.base_ref,
                "sha": pr.base_sha,
            }
        },
        "repository": {
            "id": repo.numeric_id(),
            "name": repo.name,
            "full_name": format!("{}/{}", repo.owner, repo.name),
            "default_branch": repo.default_branch,
            "private": repo.private,
        },
        "sender": {
            "login": actor_login,
        }
    })
}

/// Appends an event to `shako.outbox` and immediately dispatches pending events to:
/// 1. The in-process broadcast channel and registered `InProcessEventSubscriber`s (Portal sync).
/// 2. Matching outbound webhooks with HMAC-SHA256 signatures and up to 3 attempts (S14).
pub async fn emit_and_dispatch_event(
    state: &ShakoState,
    org_id: shako_core::OrgId,
    repo_id: Option<RepoId>,
    event_kind: &str,
    idempotency_key: String,
    payload: Value,
) -> ShakoResult<OutboxEvent> {
    let event = state
        .store()
        .append_outbox(OutboxEvent {
            id: uuid::Uuid::now_v7(),
            instance_id: state.default_instance_id,
            org_id,
            repo_id,
            event_kind: event_kind.to_owned(),
            idempotency_key,
            payload,
            dispatched: false,
            created_at: Utc::now(),
            dispatched_at: None,
        })
        .await?;

    dispatch_pending_outbox(state, org_id).await?;
    Ok(event)
}

/// Dispatches all undispatched outbox rows for `(state.default_instance_id, org_id)` once (idempotent on restart, S14).
pub async fn dispatch_pending_outbox(
    state: &ShakoState,
    org_id: shako_core::OrgId,
) -> ShakoResult<usize> {
    let pending = state
        .store()
        .list_undispatched_outbox(state.default_instance_id, org_id)
        .await?;
    let mut count = 0usize;

    for ev in pending {
        // 1. Publish to in-process broadcast bus & subscribers
        let _ = state.event_bus.send(ev.clone());
        let subs = state.subscribers.read().clone();
        for sub in subs {
            sub(&ev);
        }

        // 2. Deliver to matching active webhooks on the repository
        if let Some(rid) = ev.repo_id {
            let hooks = state
                .store()
                .list_webhooks(state.default_instance_id, org_id, Some(rid))
                .await?;
            for hook in hooks {
                if !hook.active
                    || (!hook.events.is_empty() && !hook.events.iter().any(|e| e == &ev.event_kind))
                {
                    continue;
                }
                let _ = deliver_webhook_with_retries(state, &hook, &ev.event_kind, &ev.payload).await;
            }
        }

        state
            .store()
            .mark_outbox_dispatched(state.default_instance_id, org_id, ev.id)
            .await?;
        count += 1;
    }

    Ok(count)
}

/// Delivers one webhook event through `state.outbound_guard` with up to 3 attempts,
/// recording a `DeliveryRecord` (without the response body, S14).
pub async fn deliver_webhook_with_retries(
    state: &ShakoState,
    hook: &WebhookRecord,
    event_kind: &str,
    payload: &Value,
) -> ShakoResult<DeliveryRecord> {
    let body_bytes =
        serde_json::to_vec(payload).map_err(|e| ShakoError::Internal(e.to_string()))?;
    let secret = state
        .secret_store
        .get_secret(hook.org_id, &hook.secret_ref)
        .unwrap_or_default();
    let sig = compute_gitea_signature(&secret, &body_bytes)?;
    let delivery_id = uuid::Uuid::now_v7();
    let ts = Utc::now().timestamp().to_string();

    let mut headers = vec![
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("X-Gitea-Event".to_owned(), event_kind.to_owned()),
        ("X-Gitea-Signature".to_owned(), sig.clone()),
        ("X-Hub-Signature-256".to_owned(), format!("sha256={sig}")),
        ("X-Gitea-Delivery".to_owned(), delivery_id.to_string()),
        ("X-Shako-Timestamp".to_owned(), ts),
    ];
    if let Some(prev_ref) = &hook.previous_secret_ref
        && let Some(prev_sec) = state.secret_store.get_secret(hook.org_id, prev_ref)
        && let Ok(prev_sig) = compute_gitea_signature(&prev_sec, &body_bytes)
    {
        headers.push(("X-Gitea-Signature-Previous".to_owned(), prev_sig));
    }

    let mut last_status_code: Option<u16> = None;
    let mut delivered = false;

    for _attempt in 0..3 {
        match state
            .outbound_guard
            .request(
                reqwest::Method::POST,
                &hook.url,
                &headers,
                body_bytes.clone(),
                None,
            )
            .await
        {
            Ok((code, _resp_body_ignored)) => {
                last_status_code = Some(code);
                if (200..300).contains(&code) {
                    delivered = true;
                    break;
                }
            }
            Err(ShakoError::SsrfBlocked(_)) => {
                // SSRF block is permanent: do not retry
                last_status_code = None;
                break;
            }
            Err(_) => {}
        }
    }

    let rec = DeliveryRecord {
        id: delivery_id,
        instance_id: hook.instance_id,
        org_id: hook.org_id,
        webhook_id: hook.id,
        event: event_kind.to_owned(),
        status: if delivered {
            "delivered".into()
        } else {
            "failed".into()
        },
        response_code: last_status_code,
        delivered_at: Utc::now(),
    };
    state.store().record_delivery(rec).await
}
