#![forbid(unsafe_code)]
//! Transactional outbox event recording, in-process bus dispatch, and outbound webhook delivery
//! with HMAC-SHA256 through `OutboundGuard` (Contract S14, R§5, Threat T3, T11).
//! A failed delivery waits for a later pass with backoff ([`retry_delay`]); only a 2xx or a
//! refusal for good marks the event dispatched.
//! A hook delivers in its own [`WebhookFormat`]: Gitea's headers and body (`X-Gitea-Signature`,
//! `X-Gitea-Event`, `X-Gitea-Delivery`) for outside clients, or shako's native body with
//! `X-Shako-Signature`, `X-Shako-Event` and `X-Shako-Delivery` (the Portal's receiver).

use crate::state::ShakoState;
use hmac::{Hmac, Mac};
use jc_time::TimestampTz;
use serde_json::{Value, json};
use sha2::Sha256;
use shako_core::{
    DeliveryRecord, OutboxClaim, OutboxEvent, PullRequestRecord, RepoId, RepoRecord, ShakoError,
    ShakoResult, WebhookFormat, WebhookId, WebhookRecord,
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

/// Delivery passes an event gets before it is given up: about 50 minutes of [`retry_delay`],
/// time for a receiver's rollout or restart.
pub const MAX_DELIVERY_ATTEMPTS: u32 = 10;

/// How long an event waits after its `attempts`-th failed pass: 10 s doubling, at most 15 min.
#[must_use]
pub fn retry_delay(attempts: u32) -> std::time::Duration {
    let secs = 10u64.saturating_mul(1u64 << attempts.min(7));
    std::time::Duration::from_secs(secs.min(900))
}

/// Appends an event to `shako.outbox`, then dispatches the pending events to:
/// 1. The in-process broadcast channel and registered `InProcessEventSubscriber`s (Portal sync).
/// 2. Matching outbound webhooks with HMAC-SHA256 signatures (S14).
///
/// The append's error is the caller's ([`record_event`]). A dispatch that fails is logged; the
/// recorded event is sent by the next pass of the outbox loop.
pub async fn emit_and_dispatch_event(
    state: &ShakoState,
    org_id: shako_core::OrgId,
    repo_id: Option<RepoId>,
    event_kind: &str,
    idempotency_key: String,
    payload: Value,
) -> ShakoResult<OutboxEvent> {
    let event = record_event(state, org_id, repo_id, event_kind, idempotency_key, payload).await?;
    dispatch_logged(state, org_id).await;
    Ok(event)
}

/// Appends an event to `shako.outbox`. An append that fails with a storage error is tried twice
/// more, then its error is the caller's: the write it reports happened, its event is not
/// recorded, and the caller answers with the error rather than a silent success.
pub async fn record_event(
    state: &ShakoState,
    org_id: shako_core::OrgId,
    repo_id: Option<RepoId>,
    event_kind: &str,
    idempotency_key: String,
    payload: Value,
) -> ShakoResult<OutboxEvent> {
    let event = OutboxEvent {
        id: uuid::Uuid::now_v7(),
        instance_id: state.default_instance_id,
        org_id,
        repo_id,
        event_kind: event_kind.to_owned(),
        idempotency_key,
        payload,
        dispatched: false,
        created_at: TimestampTz::now(),
        dispatched_at: None,
    };
    let mut pause = std::time::Duration::from_millis(100);
    let mut tries = 1;
    loop {
        match state.store().append_outbox(event.clone()).await {
            Ok(stored) => return Ok(stored),
            Err(ShakoError::Storage(e)) if tries < 3 => {
                tracing::warn!(kind = %event.event_kind, error = %e, "shako: an event was not recorded, trying again");
                tokio::time::sleep(pause).await;
                pause *= 4;
                tries += 1;
            }
            Err(e) => {
                tracing::error!(kind = %event.event_kind, key = %event.idempotency_key, error = %e, "shako: an event of a completed write was not recorded");
                return Err(e);
            }
        }
    }
}

/// [`dispatch_pending_outbox`] once the event stands: inside a write request's unit after its
/// commit (a rolled-back change sends nothing), outside one at once. A failed send is logged and
/// the outbox loop sends what it left.
pub async fn dispatch_logged(state: &ShakoState, org_id: shako_core::OrgId) {
    let st = state.clone();
    jc_audit::after_commit(async move {
        if let Err(e) = dispatch_pending_outbox(&st, org_id).await {
            tracing::warn!(error = %e, "shako: the outbox was not sent, the outbox loop sends it");
        }
    })
    .await;
}

/// Dispatches the due outbox rows for `(state.default_instance_id, org_id)` once (idempotent on
/// restart, S14). An event is marked dispatched when every matching hook took it with a 2xx or
/// refused it for good (a 4xx other than 408 and 429, a blocked address); otherwise it is given
/// back to wait [`retry_delay`], and the hooks that took it are not sent it again. After
/// [`MAX_DELIVERY_ATTEMPTS`] passes it is marked and its failed deliveries stay on record.
pub async fn dispatch_pending_outbox(
    state: &ShakoState,
    org_id: shako_core::OrgId,
) -> ShakoResult<usize> {
    let _one_dispatch = state
        .outbox_locks
        .lock(RepoId::from(org_id.as_uuid()))
        .await;
    let mut count = 0usize;

    // Claimed one by one, sent outside any transaction, then marked: replicas share the outbox,
    // and only a claimant that died leaves an event to be sent again.
    while let Some(OutboxClaim {
        event: ev,
        attempts,
    }) = state
        .store()
        .claim_outbox(state.default_instance_id, org_id)
        .await?
    {
        // 1. Publish to in-process broadcast bus & subscribers, once: a retry is for the hooks.
        if attempts == 0 {
            let _ = state.event_bus.send(ev.clone());
            let subs = state.subscribers.read().clone();
            for sub in subs {
                sub(&ev);
            }
        }

        // 2. Deliver to matching active webhooks on the repository
        let mut undelivered = Vec::new();
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
                let id = delivery_id_of(ev.id, hook.id);
                // ponytail: a retry reads the hook's deliveries to skip the ones that took the
                // event; a delivery lookup by id when hooks keep long histories.
                if attempts > 0
                    && state
                        .store()
                        .list_deliveries(state.default_instance_id, org_id, Some(hook.id))
                        .await?
                        .iter()
                        .any(|d| d.id == id && d.status == "delivered")
                {
                    continue;
                }
                let (_, outcome) =
                    deliver_webhook(state, &hook, &ev.event_kind, &ev.payload, id).await?;
                if outcome == Delivery::Retry {
                    undelivered.push(hook.id);
                }
            }
        }

        if !undelivered.is_empty() && attempts + 1 < MAX_DELIVERY_ATTEMPTS {
            state
                .store()
                .retry_outbox(
                    state.default_instance_id,
                    org_id,
                    ev.id,
                    retry_delay(attempts),
                )
                .await?;
            continue;
        }
        if !undelivered.is_empty() {
            tracing::warn!(event = %ev.id, hooks = ?undelivered, "shako: webhooks did not take an event in {MAX_DELIVERY_ATTEMPTS} passes; their deliveries stay recorded as failed");
        }
        let marked = state
            .store()
            .mark_outbox_dispatched(state.default_instance_id, org_id, ev.id)
            .await?;
        if !marked {
            return Err(ShakoError::Storage(
                "a claimed event could not be marked".into(),
            ));
        }
        count += 1;
    }

    Ok(count)
}

/// The id a receiver sees for `event` at `hook`: the same when a claim is sent again, so it can
/// tell the repeat.
fn delivery_id_of(event: uuid::Uuid, hook: WebhookId) -> uuid::Uuid {
    use sha2::Digest as _;
    let digest = Sha256::new()
        .chain_update(event.as_bytes())
        .chain_update(hook.as_uuid().as_bytes())
        .finalize();
    let mut id = [0u8; 16];
    id.iter_mut()
        .zip(digest.iter())
        .for_each(|(to, from)| *to = *from);
    uuid::Builder::from_random_bytes(id).into_uuid()
}

/// Whether a delivery is done with: taken or refused for good, or worth a later pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Done,
    Retry,
}

/// Delivers one webhook event through `state.outbound_guard` once, now, and returns its
/// `DeliveryRecord` (without the response body, S14): a hook's test delivery.
pub async fn deliver_webhook_now(
    state: &ShakoState,
    hook: &WebhookRecord,
    event_kind: &str,
    payload: &Value,
) -> ShakoResult<DeliveryRecord> {
    deliver_webhook(state, hook, event_kind, payload, uuid::Uuid::now_v7())
        .await
        .map(|(record, _)| record)
}

async fn deliver_webhook(
    state: &ShakoState,
    hook: &WebhookRecord,
    event_kind: &str,
    payload: &Value,
    delivery_id: uuid::Uuid,
) -> ShakoResult<(DeliveryRecord, Delivery)> {
    let body = match hook.format {
        WebhookFormat::Gitea => payload.clone(),
        WebhookFormat::Shako => native_webhook_payload(event_kind, payload),
    };
    let body_bytes = serde_json::to_vec(&body).map_err(|e| ShakoError::Internal(e.to_string()))?;
    // A reference stored before the API refused non-key names still signs; the warning names
    // the hook, never the reference, which may be a pasted secret.
    if !shako_core::is_secret_ref(&hook.secret_ref)
        || hook
            .previous_secret_ref
            .as_deref()
            .is_some_and(|r| !shako_core::is_secret_ref(r))
    {
        tracing::warn!(hook = %hook.id, "a webhook's secret reference is not a secret-store key name; register the hook again");
    }
    // A hook whose secret is gone is not delivered: a body signed with an empty key would
    // pass any receiver that, like this forge, accepts an empty secret, and fail the others.
    // A store that cannot answer counts as a missing secret: the delivery is recorded failed and
    // retried with the next event, never sent unsigned.
    let held = state
        .secret_store
        .get_secret(hook.org_id, &hook.secret_ref)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(hook = %hook.id, error = %e, "a webhook's secret could not be read");
            None
        });
    let Some(secret) = held.filter(|s| !s.is_empty()) else {
        let record = state
            .store()
            .record_delivery(DeliveryRecord {
                id: delivery_id,
                instance_id: hook.instance_id,
                org_id: hook.org_id,
                webhook_id: hook.id,
                event: event_kind.to_owned(),
                status: "failed".into(),
                response_code: None,
                delivered_at: TimestampTz::now(),
            })
            .await?;
        return Ok((record, Delivery::Retry));
    };
    let sig = compute_gitea_signature(&secret, &body_bytes)?;
    let ts = TimestampTz::now().as_second().to_string();

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
        && let Ok(Some(prev_sec)) = state.secret_store.get_secret(hook.org_id, prev_ref).await
        && !prev_sec.is_empty()
        && let Ok(prev_sig) = compute_gitea_signature(&prev_sec, &body_bytes)
    {
        headers.push((format!("{prefix}-Signature-Previous"), prev_sig));
    }

    // One request a pass: a failure waits for the next pass of the outbox, not a few
    // milliseconds for a receiver that is restarting.
    let (last_status_code, outcome) = match state
        .outbound_guard
        .request(reqwest::Method::POST, &hook.url, &headers, body_bytes, None)
        .await
    {
        Ok((code, _resp_body_ignored)) => (Some(code), delivery_outcome(code)),
        // A blocked address stays blocked.
        Err(ShakoError::SsrfBlocked(_)) => (None, Delivery::Done),
        Err(_) => (None, Delivery::Retry),
    };
    let delivered = last_status_code.is_some_and(|code| (200..300).contains(&code));

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
        delivered_at: TimestampTz::now(),
    };
    Ok((state.store().record_delivery(rec).await?, outcome))
}

/// A 2xx took the event, and a receiver's other 4xx (a refused signature, a gone route) is its
/// verdict on this body: sending it again only repeats it. 408, 429 and 5xx are worth a later
/// pass.
fn delivery_outcome(code: u16) -> Delivery {
    match code {
        200..=299 => Delivery::Done,
        408 | 429 => Delivery::Retry,
        400..=499 => Delivery::Done,
        _ => Delivery::Retry,
    }
}
