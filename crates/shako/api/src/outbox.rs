#![forbid(unsafe_code)]
//! Transactional outbox event recording, in-process bus dispatch, and outbound webhook delivery
//! with HMAC-SHA256 and 3 retries through `OutboundGuard` (Contract S14, R§5, Threat T3, T11).
//! A hook delivers in its own [`WebhookFormat`]: Gitea's headers and body (`X-Gitea-Signature`,
//! `X-Gitea-Event`, `X-Gitea-Delivery`) for outside clients, or shako's native body with
//! `X-Shako-Signature`, `X-Shako-Event` and `X-Shako-Delivery` (the Portal's receiver).

use crate::state::ShakoState;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use shako_core::{
    DeliveryRecord, OutboxEvent, PullRequestRecord, RepoId, RepoRecord, ShakoError, ShakoResult,
    WebhookFormat, WebhookRecord,
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

/// The native body of an event, built from the Gitea-shaped payload the outbox stores: what a
/// [`WebhookFormat::Shako`] hook delivers (`PORTAL-ODATA.md` section 8).
#[must_use]
pub fn native_webhook_payload(event_kind: &str, gitea: &Value) -> Value {
    let text = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let repo = gitea.get("repository").cloned().unwrap_or(Value::Null);
    let owner = repo
        .get("owner")
        .map(|o| text(o, "login"))
        .filter(|o| !o.is_empty())
        .or_else(|| {
            text(&repo, "full_name")
                .split_once('/')
                .map(|(o, _)| o.to_owned())
        })
        .unwrap_or_default();
    let repository = json!({
        "owner": owner,
        "name": text(&repo, "name"),
        "defaultBranch": text(&repo, "default_branch"),
        "private": repo.get("private").and_then(Value::as_bool).unwrap_or(true),
    });
    let sender = gitea
        .get("sender")
        .map(|s| text(s, "login"))
        .unwrap_or_default();
    match event_kind {
        "pull_request" => {
            let pr = gitea.get("pull_request").cloned().unwrap_or(Value::Null);
            let side = |k: &str| {
                let s = pr.get(k).cloned().unwrap_or(Value::Null);
                json!({ "ref": text(&s, "ref"), "sha": text(&s, "sha") })
            };
            json!({
                "event": "pull_request",
                "action": text(gitea, "action"),
                "pullRequest": {
                    "number": pr.get("number").cloned().unwrap_or(Value::Null),
                    "state": text(&pr, "state"),
                    "title": text(&pr, "title"),
                    "merged": pr.get("merged").and_then(Value::as_bool).unwrap_or(false),
                    "mergeCommitSha": text(&pr, "merge_commit_sha"),
                    "head": side("head"),
                    "base": side("base"),
                },
                "repository": repository,
                "sender": sender,
            })
        }
        "push" => json!({
            "event": "push",
            "ref": text(gitea, "ref"),
            "before": text(gitea, "before"),
            "after": text(gitea, "after"),
            "repository": repository,
            "sender": sender,
        }),
        other => json!({ "event": other, "repository": repository, "sender": sender }),
    }
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
                let _ =
                    deliver_webhook_with_retries(state, &hook, &ev.event_kind, &ev.payload).await;
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
    let body = match hook.format {
        WebhookFormat::Gitea => payload.clone(),
        WebhookFormat::Shako => native_webhook_payload(event_kind, payload),
    };
    let body_bytes = serde_json::to_vec(&body).map_err(|e| ShakoError::Internal(e.to_string()))?;
    let delivery_id = uuid::Uuid::now_v7();
    // A hook whose secret is gone is not delivered: a body signed with an empty key would
    // pass any receiver that, like this forge, accepts an empty secret, and fail the others.
    let Some(secret) = state
        .secret_store
        .get_secret(hook.org_id, &hook.secret_ref)
        .filter(|s| !s.is_empty())
    else {
        return state
            .store()
            .record_delivery(DeliveryRecord {
                id: delivery_id,
                instance_id: hook.instance_id,
                org_id: hook.org_id,
                webhook_id: hook.id,
                event: event_kind.to_owned(),
                status: "failed".into(),
                response_code: None,
                delivered_at: Utc::now(),
            })
            .await;
    };
    let sig = compute_gitea_signature(&secret, &body_bytes)?;
    let ts = Utc::now().timestamp().to_string();

    let prefix = match hook.format {
        WebhookFormat::Gitea => "X-Gitea",
        WebhookFormat::Shako => "X-Shako",
    };
    let mut headers = vec![
        ("Content-Type".to_owned(), "application/json".to_owned()),
        (format!("{prefix}-Event"), event_kind.to_owned()),
        (format!("{prefix}-Signature"), sig.clone()),
        (format!("{prefix}-Delivery"), delivery_id.to_string()),
        ("X-Shako-Timestamp".to_owned(), ts),
    ];
    if hook.format == WebhookFormat::Gitea {
        headers.push(("X-Hub-Signature-256".to_owned(), format!("sha256={sig}")));
    }
    if let Some(prev_ref) = &hook.previous_secret_ref
        && let Some(prev_sec) = state
            .secret_store
            .get_secret(hook.org_id, prev_ref)
            .filter(|s| !s.is_empty())
        && let Ok(prev_sig) = compute_gitea_signature(&prev_sec, &body_bytes)
    {
        headers.push((format!("{prefix}-Signature-Previous"), prev_sig));
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
                // A receiver's 4xx (a refused signature, a gone route) is its verdict on this
                // body; sending the same body again only repeats it.
                if (400..500).contains(&code) {
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
