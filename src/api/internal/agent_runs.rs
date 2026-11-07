//! The internal listener's run routes (AG-52): what the credential proxy and the agent runner call
//! back on. No session and no CSRF; every handler checks the caller's bearer first.

use crate::agents::profile::Profile;
use crate::agents::run::AgentRunStatus;
use crate::api::agent_runs::{
    publish_event, status_error, terminal, unavailable, EventReceipt, Inbox, InboxQuery,
    RelayedEvent, RelayedInbound, RunContext, INBOX_KINDS, INBOX_WAIT_SECS,
};
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// The internal listener (AG-52). No session, no CSRF, one bearer: the proxy's own token.
// ---------------------------------------------------------------------------------------------

pub async fn internal_get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunContext>, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;
    let profile = Profile::load(&state.mirror, &run.profile)?;
    let endpoint_slugs = crate::agents::endpoints::of_run(&run)
        .into_iter()
        .map(|endpoint| endpoint.slug)
        .collect();
    // Layout 2: the run's folder is in its project's own repository, at that repository's root
    // (CC-87, AG-86). An application in its own repository is written by the Portal, not
    // through the proxy, and keeps its empty prefix.
    let repository = if run.in_own_repository() {
        None
    } else {
        state.mirror.repository_of(&run.project)
    };
    let path_prefix = match &repository {
        Some(_) => run
            .path_prefix
            .strip_prefix(&format!("projects/{}/", run.project))
            .unwrap_or(&run.path_prefix)
            .to_owned(),
        None => run.path_prefix.clone(),
    };
    Ok(Json(RunContext {
        id: run.id,
        project: run.project,
        app_name: run.app_name,
        endpoint_slug: run.endpoint_slug,
        endpoint_slugs,
        allows_write: run.allows_write,
        branch: run.branch,
        path_prefix,
        repository,
        status: run.status,
        kind: run.kind,
        ticket_hash: run.ticket_hash,
        max_tokens: profile.max_tokens_per_run,
        allowed_hosts: profile.allowed_hosts,
        requests_per_minute: profile.requests_per_minute,
        steps_per_run: profile.steps_per_run,
        max_response_bytes: profile.max_response_bytes,
        max_egress_bytes_per_run: profile.max_egress_bytes_per_run,
        created_by: run.created_by,
        model_name: profile.model_name,
        reasoning_effort: profile.reasoning_effort,
    }))
}

/// What a run may read about a resource of its own project when a step failed (AG-57).
///
/// The proxy has already refused an unknown component and an id that is not a name; here the
/// run's project is the only project asked, so a run learns nothing about another project's
/// pipelines or changes, not even that they exist.
pub async fn internal_diagnostics(
    State(state): State<AppState>,
    Path((id, component, name)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;
    let body = match component.as_str() {
        "pipeline" => serde_json::to_value(
            crate::api::pipelines::metrics_for(&state, &run.project, &name).await?,
        ),
        "change" => serde_json::to_value(
            crate::api::changes::change_for(&state, &run.project, &name).await?,
        ),
        other => {
            return Err(ApiError::BadRequest(format!(
                "the diagnostics door knows no component '{other}'"
            )))
        }
    }
    .map_err(|err| ApiError::Internal(err.to_string()))?;
    Ok(Json(body))
}

/// What the person said, for the workspace to act on (AG-45, AG-52).
///
/// One call, one channel: an agent that wants to know whether a question was answered and
/// whether it was told to change course asks here and nowhere else. The call waits rather than
/// answering empty immediately, because the alternative is a workspace polling in a loop and
/// spending its request budget on nothing.
pub async fn internal_inbox(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<InboxQuery>,
    headers: HeaderMap,
) -> Result<Json<Inbox>, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;

    // Subscribed before the store is read, so an event that lands between the two is waited
    // for rather than missed.
    let mut live = state.agent_events.subscribe(&run.id).await;
    let items = inbox_items(&state, &run.id, query.after).await?;
    if !items.is_empty() || terminal(&run) {
        return Ok(Json(Inbox { items }));
    }

    let wait = query.wait.unwrap_or(INBOX_WAIT_SECS).min(INBOX_WAIT_SECS);
    if wait == 0 {
        return Ok(Json(Inbox { items }));
    }
    let _ = tokio::time::timeout(Duration::from_secs(wait), async {
        loop {
            match live.recv().await {
                Ok(event)
                    if event.seq > query.after && INBOX_KINDS.contains(&event.kind.as_str()) =>
                {
                    return;
                }
                // A lagged receiver missed something; the store below is the truth either way.
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
    .await;

    Ok(Json(Inbox {
        items: inbox_items(&state, &run.id, query.after).await?,
    }))
}

async fn inbox_items(
    state: &AppState,
    run_id: &str,
    after: i64,
) -> Result<Vec<RelayedInbound>, ApiError> {
    Ok(state
        .agents
        .events_since(run_id, after)
        .await
        .map_err(unavailable)?
        .into_iter()
        .filter(|event| INBOX_KINDS.contains(&event.kind.as_str()))
        .map(|event| RelayedInbound {
            seq: event.seq,
            kind: event.kind,
            payload: event.payload,
        })
        .collect())
}

/// The body arrives as bytes and is parsed after the caller is known (T-2271): an extractor runs
/// before the handler does, so `Json<RelayedEvent>` answered 422 to a call carrying no identity at
/// all, which tells whoever reaches the port what shape the route wants.
pub async fn internal_post_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<EventReceipt>), ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let relayed: RelayedEvent = serde_json::from_slice(&body)
        .map_err(|error| ApiError::BadRequest(format!("this is not a relayed event: {error}")))?;
    let run = state
        .agents
        .get_run(&relayed.run_id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{}' not found", relayed.run_id)))?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{}' is '{}' and takes no more events",
            run.id, run.status
        )));
    }

    // A navigate event is checked before it is recorded: a route that is not a path inside the
    // Portal never reaches the log, let alone a browser (UI-45).
    if relayed.kind == "navigate" {
        navigate_route(&relayed.payload)?;
    }
    // An event is a record first. The three kinds that also move something are applied after
    // it is recorded, so a stream never shows a state the log does not explain.
    let event = publish_event(&state, &run.id, &relayed.kind, relayed.payload.clone()).await?;

    match relayed.kind.as_str() {
        "usage" => {
            crate::telemetry::model_call(&run.kind, &relayed.payload);
            let tokens = relayed
                .payload
                .get("tokensThisStep")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            state
                .agents
                .record_usage(&run.id, tokens, 1)
                .await
                .map_err(unavailable)?;
        }
        "preview" => {
            if let Some(url) = relayed
                .payload
                .get("previewUrl")
                .and_then(serde_json::Value::as_str)
            {
                state
                    .agents
                    .set_preview_url(&run.id, url)
                    .await
                    .map_err(unavailable)?;
                // The first frame is counted once: `set_preview_url` keeps the first value.
                if run.first_frame_ms.is_none() {
                    if let Ok(Some(updated)) = state.agents.get_run(&run.id).await {
                        if let Some(ms) = updated.first_frame_ms {
                            crate::telemetry::record_run_timing(
                                "first_frame",
                                &updated.profile,
                                ms,
                            );
                        }
                    }
                }
            }
        }
        "status" => {
            let next = relayed
                .payload
                .get("status")
                .and_then(serde_json::Value::as_str)
                .and_then(AgentRunStatus::parse)
                .ok_or_else(|| {
                    ApiError::BadRequest("a status event names no state of a run".into())
                })?;
            let error = relayed
                .payload
                .get("error")
                .and_then(serde_json::Value::as_str);
            state
                .agents
                .set_status(&run.id, next, error)
                .await
                .map_err(status_error)?;
            if next == AgentRunStatus::AwaitingApproval {
                crate::api::agent_runs::lease_for_approval(&state, &run.id).await;
            }
        }
        _ => {}
    }

    Ok((StatusCode::CREATED, Json(EventReceipt { seq: event.seq })))
}

/// The longest route a `navigate` event may name (UI-45).
const MAX_ROUTE_CHARS: usize = 512;

/// The route of a `navigate` event, or why it is refused (UI-45, API/04 §4).
///
/// Only a path inside the Portal passes: one leading `/`, no scheme, no `//` (a
/// protocol-relative URL), no `#`, no control character, at most 512 characters. The prefill,
/// when present, is an object; its values are the form's problem, not this gate's.
fn navigate_route(payload: &serde_json::Value) -> Result<&str, ApiError> {
    let route = payload
        .get("route")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("a navigate event names no route".into()))?;
    let inside = route.starts_with('/')
        && !route.starts_with("//")
        && route.len() <= MAX_ROUTE_CHARS
        && !route.contains('#')
        && !route.contains(':')
        && !route.chars().any(char::is_control);
    if !inside {
        return Err(ApiError::BadRequest(
            "a navigate route must be a path inside the Portal".into(),
        ));
    }
    match payload.get("prefill") {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Object(_)) => Ok(route),
        Some(_) => Err(ApiError::BadRequest(
            "a navigate prefill must be an object".into(),
        )),
    }
}

/// The credential proxy's own ServiceAccount token, audience-bound to this listener and matched by
/// the client it was issued to (AG-52, T-2271).
///
/// It was a string both sides held, read from `JC_AGENT_PROXY_TOKEN` and compared in constant time.
/// Constant time was the least of it: a shared secret between two services never rotates, appears in
/// two configurations, and gives whoever reads either of them every callback of every run. The token
/// is minted per proxy from the realm now and expires by itself.
async fn authenticate_proxy(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    crate::auth::internal::authenticate_agent_proxy(state, headers).await
}

/// What the Portal accepts on one relayed event: the proxy's 64 KiB ceiling
/// (`Architecture/19 §4`) plus the envelope the proxy wraps it in — the run id and the kind.
///
/// The proxy already refuses a larger event, and this is the same ceiling on the door behind it,
/// so a caller that reaches the internal listener another way is held to what the documented
/// route promises rather than to axum's default (AG-45, AG-46).
const MAX_RELAYED_EVENT_BYTES: usize = 64 * 1024 + 1024;

/// The operations registry, reached by an agent run through the proxy (AG-64, AG-70).
///
/// One registry behind every door: this is the same dispatcher a person's MCP client speaks to,
/// entered as the person who started the run and narrowed by the run's profile. The two halves
/// are the point — a profile may take away and never add, and `Via::Agent` keeps AG-11 whatever
/// the profile says, so no agent approves a change through this door either.
pub async fn internal_mcp(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{}' is '{}' and calls nothing more",
            run.id, run.status
        )));
    }
    let identity: crate::auth::session::Identity = serde_json::from_value(run.starter.clone())
        .map_err(|_| {
            ApiError::Denied(format!(
                "run '{}' carries no starter, so there is nobody to run its calls as",
                run.id
            ))
        })?;
    if signed_out_since(&state, &identity.subject, &run.created_at) {
        return Err(ApiError::Denied(format!(
            "run '{}' acts for a person who signed out since it started; start it again",
            run.id
        )));
    }
    let profile = Profile::load(&state.mirror, &run.profile)?;
    let caller = crate::ops::Caller::for_run(identity, profile.access.clone());
    Ok(crate::mcp::dispatch_for(state, caller, body).await)
}

/// Whether the person a run acts for logged out (themselves, or an administrator signing them out)
/// since `started` (RFC 3339): that takes back what the run does in their name (T-6226). A start
/// time that does not parse counts as the oldest.
fn signed_out_since(state: &AppState, subject: &str, started: &str) -> bool {
    let started =
        chrono::DateTime::parse_from_rfc3339(started).map_or(i64::MIN, |at| at.timestamp());
    state.revoked_since(subject, started)
}

/// The routes the credential proxy calls (a run, its MCP, events, inbox and diagnostics), served
/// on the internal listener alone (AG-52).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/agent-runs/{id}/mcp", post(internal_mcp))
        .route(
            "/internal/agent-runs/events",
            post(internal_post_event).layer(axum::extract::DefaultBodyLimit::max(
                MAX_RELAYED_EVENT_BYTES,
            )),
        )
        .route("/internal/agent-runs/{id}", get(internal_get_run))
        .route("/internal/agent-runs/{id}/inbox", get(internal_inbox))
        .route(
            "/internal/agent-runs/{id}/diagnostics/{component}/{name}",
            get(internal_diagnostics),
        )
}

#[cfg(test)]
mod tests {
    use super::{navigate_route, signed_out_since, AppState};
    use serde_json::json;

    /// T-6226 (F-R2POR2-007): a run's calls stop once the person it acts for signs out; a run
    /// started after that sign-out, and another person's, go on.
    #[tokio::test]
    async fn a_run_stops_acting_for_a_person_who_signed_out_since_it_started() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let started = "2026-10-31T10:00:00Z";
        let at = chrono::DateTime::parse_from_rfc3339(started)
            .expect("time")
            .timestamp();
        assert!(!signed_out_since(&state, "sub-ana", started));
        state.revoke_subject("sub-ana", at + 60).await;
        assert!(signed_out_since(&state, "sub-ana", started));
        assert!(!signed_out_since(&state, "sub-ana", "2026-10-31T11:00:00Z"));
        assert!(!signed_out_since(&state, "sub-bob", started));
        assert!(signed_out_since(&state, "sub-ana", "not a time"));
    }

    #[test]
    fn a_portal_path_with_a_prefill_passes() {
        let payload = json!({
            "route": "/projects/helsinki/endpoints?tab=all",
            "prefill": {"name": "air-quality-public"}
        });
        assert_eq!(
            navigate_route(&payload).unwrap(),
            "/projects/helsinki/endpoints?tab=all"
        );
        assert!(navigate_route(&json!({"route": "/"})).is_ok());
        assert!(navigate_route(&json!({"route": "/x", "prefill": null})).is_ok());
    }

    #[test]
    fn anything_that_is_not_a_path_inside_the_portal_is_refused() {
        for route in [
            "https://evil.example/",
            "//evil.example/projects",
            "javascript:alert(1)",
            "projects/helsinki",
            "/projects/helsinki#/x",
            "/projects/hel\nsinki",
            "",
        ] {
            assert!(
                navigate_route(&json!({ "route": route })).is_err(),
                "{route:?}"
            );
        }
        let long = format!("/{}", "a".repeat(512));
        assert!(navigate_route(&json!({ "route": long })).is_err());
        assert!(navigate_route(&json!({ "prefill": {} })).is_err());
        assert!(navigate_route(&json!({ "route": "/x", "prefill": "name=x" })).is_err());
        assert!(navigate_route(&json!({ "route": "/x", "prefill": [1] })).is_err());
    }
}
