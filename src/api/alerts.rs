//! Alerts a person chooses (API/01 §37, PL-71, T-3261): subscribe to a pipeline, a space, a type
//! in a space or an agent profile (T-5301), mute, and read the notices the leader's evaluator left (`crate::alerts`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::alerts::{checked, NewSubscription, Notice, Scope, Subscription};
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::state::AppState;

fn stored(err: sqlx::Error) -> ApiError {
    tracing::warn!(error = %err, "alerts not reachable");
    ApiError::Unavailable("the alerts could not be read or written".into())
}

/// Who the caller is to the alerts: their username, lower case.
fn subject(user: &CurrentUser) -> String {
    user.0.identity.username.to_ascii_lowercase()
}

/// Read on what a subscription names; `404` for a target the caller may not read, as §27 decides.
fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    scope: Scope,
    target: &str,
) -> Result<(), ApiError> {
    match scope {
        Scope::Pipeline => {
            crate::api::pipelines::readable_pipeline(state, user, project, target).map(|_| ())
        }
        Scope::Space => crate::api::entity_trash::readable(state, user, project, target),
        Scope::Type => {
            let (space, _) = target
                .split_once('/')
                .ok_or_else(|| ApiError::BadRequest("a type is named `{space}/{Type}`".into()))?;
            crate::api::entity_trash::readable(state, user, project, space)
        }
        Scope::Agent => agent_readable(state, user, project, target),
    }
}

/// An agent profile's alerts count every person's runs, which only an approver of the project
/// sees on the Activity tab (AG-93): `403` naming the project for anyone else, before the
/// profile is looked up, so the answer tells them nothing of it; `404` for one that does not exist.
fn agent_readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    profile: &str,
) -> Result<(), ApiError> {
    crate::api::changes::may_approve_anything(state, &user.0.identity, project).map_err(|_| {
        ApiError::Denied(format!(
            "project: the alerts of `{profile}` count every person's runs in {project}, which \
                 only an approver of the project sees; pick a project you approve in, or ask a \
                 steward of {project} for the approver role"
        ))
    })?;
    if state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "AgentProfile", profile)
        .is_none()
    {
        return Err(ApiError::NotFound(format!(
            "target: no agent profile `{profile}`"
        )));
    }
    Ok(())
}

/// Whether the caller may still read what a notice names, with their grants now.
fn notice_readable(state: &AppState, user: &CurrentUser, notice: &Notice) -> bool {
    if notice.event.is_agent() {
        return crate::api::changes::may_approve_anything(state, &user.0.identity, &notice.project)
            .is_ok();
    }
    crate::api::pipelines::readable_pipeline(state, user, &notice.project, &notice.pipeline).is_ok()
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = AlertSubscriptions)]
pub struct Subscriptions {
    pub items: Vec<Subscription>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = AlertNotices)]
pub struct Notices {
    pub items: Vec<Notice>,
    pub unread: i64,
}

/// How long a subscription stays muted, or `null` to unmute.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AlertMute)]
pub struct Mute {
    #[serde(rename = "for")]
    pub duration: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/alerts",
    summary = "List My Alerts",
    description = "The caller's alert subscriptions (API/01 §37).",
    tag = "auth",
    responses(
        (status = 200, description = "The caller's subscriptions", body = Subscriptions),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 503, description = "The alerts are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_alerts(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Subscriptions>, ApiError> {
    let items = state
        .alerts
        .subscriptions_of(&subject(&user))
        .await
        .map_err(stored)?;
    Ok(Json(Subscriptions { items }))
}

#[utoipa::path(
    put,
    path = "/api/v1/alerts",
    summary = "Subscribe To An Alert",
    description = "Subscribes the caller to a pipeline's, a space's or a type's failure, stale and zero-output alerts, or to an agent profile's refusal spikes, token-cap share and off-hours writes (scope `agent`, approvers of the project only), or changes their subscription of the same target (API/01 §37). E-mail is refused while the Portal has no mail relay.",
    tag = "auth",
    request_body(
        content = NewSubscription,
        description = "The project, the scope (`pipeline`, `space`, `type` or `agent`), its target, the events and the delivery",
        content_type = "application/json",
        example = json!({ "project": "helsinki", "scope": "space", "target": "bikes", "events": ["failure", "zero"], "delivery": "portal" })
    ),
    responses(
        (status = 200, description = "The subscription", body = Subscription),
        (status = 400, description = "An unknown scope, event or delivery, or e-mail", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "An agent profile in a project the caller does not approve in", body = crate::error::ProblemDetails),
        (status = 404, description = "A target the caller may not read", body = crate::error::ProblemDetails),
        (status = 422, description = "An unknown key", body = crate::error::ProblemDetails),
        (status = 503, description = "The alerts are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn subscribe(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(new): Json<NewSubscription>,
) -> Result<Json<Subscription>, ApiError> {
    let (scope, events, delivery) = checked(&new).map_err(ApiError::BadRequest)?;
    readable(&state, &user, &new.project, scope, &new.target)?;
    let held = state
        .alerts
        .subscriptions_of(&subject(&user))
        .await
        .map_err(stored)?;
    crate::alerts::room_for(&held, &new, scope).map_err(ApiError::BadRequest)?;
    state
        .alerts
        .upsert(
            &subject(&user),
            &new.project,
            scope,
            &new.target,
            &events,
            delivery,
        )
        .await
        .map(Json)
        .map_err(stored)
}

#[utoipa::path(
    delete,
    path = "/api/v1/alerts/{id}",
    summary = "Stop An Alert",
    description = "Removes one of the caller's subscriptions and its notices (API/01 §37). Another caller's is 404.",
    tag = "auth",
    params(("id" = i64, Path, description = "The subscription's id")),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such subscription of the caller's", body = crate::error::ProblemDetails),
        (status = 503, description = "The alerts are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn unsubscribe(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if state
        .alerts
        .remove(&subject(&user), id)
        .await
        .map_err(stored)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("no alert {id} of yours")))
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/alerts/{id}/mute",
    summary = "Mute An Alert",
    description = "Mutes one of the caller's subscriptions for 1h, 1d, 7d or forever, or unmutes it with null (API/01 §37).",
    tag = "auth",
    params(("id" = i64, Path, description = "The subscription's id")),
    request_body(
        content = Mute,
        description = "How long to mute: `1h`, `1d`, `7d` or `forever`; `null` unmutes",
        content_type = "application/json",
        example = json!({ "for": "1d" })
    ),
    responses(
        (status = 200, description = "The subscription", body = Subscription),
        (status = 400, description = "A duration that is not 1h, 1d, 7d or forever", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such subscription of the caller's", body = crate::error::ProblemDetails),
        (status = 503, description = "The alerts are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn mute(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mute): Json<Mute>,
) -> Result<Json<Subscription>, ApiError> {
    let now = Utc::now();
    let until = match mute.duration.as_deref() {
        None => None,
        Some("1h") => Some(now + Duration::hours(1)),
        Some("1d") => Some(now + Duration::days(1)),
        Some("7d") => Some(now + Duration::days(7)),
        // Far enough that a person who chose "forever" never meets it.
        Some("forever") => Some(now + Duration::days(365 * 100)),
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "`{other}` is not 1h, 1d, 7d or forever"
            )))
        }
    };
    state
        .alerts
        .mute(&subject(&user), id, until)
        .await
        .map_err(stored)?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no alert {id} of yours")))
}

#[utoipa::path(
    get,
    path = "/api/v1/alerts/notices",
    summary = "List My Alert Notices",
    description = "The caller's alert notices, newest first, at most 200, only of pipelines they still may read, with how many are unread (API/01 §37).",
    tag = "auth",
    responses(
        (status = 200, description = "The caller's notices", body = Notices),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 503, description = "The alerts are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_notices(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Notices>, ApiError> {
    let notices = state
        .alerts
        .notices(&[subject(&user)])
        .await
        .map_err(stored)?;
    // Read with the caller's grants now: a pipeline they lost access to shows nothing.
    let items: Vec<Notice> = notices
        .into_iter()
        .filter(|notice| notice_readable(&state, &user, notice))
        .collect();
    let unread = items.iter().filter(|notice| !notice.read).count() as i64;
    Ok(Json(Notices { items, unread }))
}

#[utoipa::path(
    post,
    path = "/api/v1/alerts/notices/{id}/read",
    summary = "Mark An Alert Notice Read",
    description = "Marks one of the caller's alert notices read (API/01 §37). Another caller's is 404.",
    tag = "auth",
    params(("id" = i64, Path, description = "The notice's id")),
    responses(
        (status = 204, description = "Read"),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such notice of the caller's", body = crate::error::ProblemDetails),
        (status = 503, description = "The alerts are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn read_notice(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if state
        .alerts
        .mark_read(&[subject(&user)], id)
        .await
        .map_err(stored)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("no notice {id} of yours")))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/alerts", get(list_alerts).put(subscribe))
        .route("/alerts/notices", get(list_notices))
        .route("/alerts/notices/{id}/read", post(read_notice))
        .route("/alerts/{id}", delete(unsubscribe))
        .route("/alerts/{id}/mute", post(mute))
}
