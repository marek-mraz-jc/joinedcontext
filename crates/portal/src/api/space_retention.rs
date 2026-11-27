//! A space's retention as the broker runs it (DM-85, ADR-N-046): what a draft of
//! `spec.retention` would delete now, before an owner saves it, and what the sweeps deleted.
//!
//! The rules themselves are the space manifest's, saved through the manifest routes, where only
//! an owner changes them (`spaces::check_retention`). These two routes read the broker through
//! the reconciler's writer, which knows the type IRIs it writes the rules under.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::{ContextSpaceSpec, RetentionRule, Verb};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::reconciler::space_rules::RetentionCount;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// A draft of a space's retention rules.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetentionDraft {
    /// The rules as `spec.retention` would hold them.
    #[schema(value_type = Vec<std::collections::HashMap<String, serde_json::Value>>)]
    pub retention: Vec<RetentionRule>,
}

/// Rows per class and kind.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetentionCounts {
    pub counts: Vec<RetentionCount>,
}

/// The space's held rules when the caller may read it, else the 404 every unreadable space gets.
fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<(Vec<RetentionRule>, bool), ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    let held = (is_dns1123(project) && is_dns1123(space))
        .then(|| state.mirror.get(project, "ContextSpace", space))
        .flatten()
        .filter(|_| {
            effective.may_read_project() && effective.may_read_in("ContextSpace", Some(space))
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "context space '{space}' not found in project '{project}'"
            ))
        })?;
    let rules = serde_json::from_value::<ContextSpaceSpec>(held.spec)
        .map(|spec| spec.retention)
        .unwrap_or_default();
    Ok((
        rules,
        effective.may_in("ContextSpace", Verb::Approve, Some(space)),
    ))
}

async fn counts(
    state: &AppState,
    project: &str,
    space: &str,
    draft: Option<&[RetentionRule]>,
    held: &[RetentionRule],
) -> Result<Json<RetentionCounts>, ApiError> {
    let views = state.views.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no broker address or admin token".into())
    })?;
    let tenant =
        crate::spaces::tenant(&state.mirror, project, space).map_err(ApiError::Unavailable)?;
    views
        .retention_counts(&tenant, draft, held)
        .await
        .map(|counts| Json(RetentionCounts { counts }))
        .map_err(|reason| {
            tracing::warn!(%reason, "the broker did not count a space's retention");
            ApiError::Unavailable("the broker did not count; try again shortly".into())
        })
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/retention/preview",
    summary = "Preview Space Retention",
    description = "What a draft of the space's `spec.retention` would delete now, per class and kind (`entity`, `history`), counted by the broker without deleting anything and at most 100 000 each. For the space's owners, who alone save retention (DM-85).",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    request_body(content = RetentionDraft, example = json!({"retention": [{"entityType": "Vehicle", "current": "PT5M", "history": "PT8H"}]})),
    responses(
        (status = 200, description = "What the draft would delete", body = RetentionCounts),
        (status = 400, description = "A rule the space manifest would refuse", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads the space but does not own it", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No broker, or the broker did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn preview_retention(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    body: Result<Json<RetentionDraft>, JsonRejection>,
) -> Result<Json<RetentionCounts>, ApiError> {
    let (held, owner) = readable(&state, &user, &project, &space)?;
    if !owner {
        return Err(ApiError::Denied(format!(
            "only an owner of context space '{space}' sets its retention (DM-85)"
        )));
    }
    let Json(draft) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    ContextSpaceSpec {
        retention: draft.retention.clone(),
        ..ContextSpaceSpec::default()
    }
    .validate()
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    counts(&state, &project, &space, Some(&draft.retention), &held).await
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/retention/deleted",
    summary = "Read Space Retention Deletions",
    description = "What the broker's retention sweeps deleted of the space since the broker started, per class and kind (`entity`, `history`). The same numbers for everyone who reads the space.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "What retention deleted", body = RetentionCounts),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No broker, or the broker did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn retention_deleted(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<RetentionCounts>, ApiError> {
    let (held, _) = readable(&state, &user, &project, &space)?;
    counts(&state, &project, &space, None, &held).await
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/spaces/{space}/retention/preview",
            post(preview_retention),
        )
        .route(
            "/projects/{project}/spaces/{space}/retention/deleted",
            get(retention_deleted),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::FromRequest;
    use serde_json::json;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn owner(state: &AppState) -> CurrentUser {
        CurrentUser(crate::auth::session::Session {
            identity: crate::auth::session::Identity {
                client: None,
                subject: "sub-ana".into(),
                username: "ana".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: vec![state.config.bootstrap_admins.clone()],
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            stepped_up_at: None,
            id_token: String::new(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    /// The space `city/air`, its rules counted by the broker at `broker`.
    fn world(broker: &MockServer, tag: &str) -> AppState {
        let token = std::env::temp_dir().join(format!("jc-retention-{}-{tag}", std::process::id()));
        std::fs::write(&token, "broker-admin").expect("a token file");
        let mut state = AppState::new(crate::config::Config::for_tests(), None);
        state.views = Some(std::sync::Arc::new(
            crate::reconciler::views::ViewSync::new(broker.uri(), token),
        ));
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: crate::resource::ObjectMeta::new("air", "city"),
            spec: json!({}),
            status: None,
        });
        state
    }

    fn draft(rules: serde_json::Value) -> Result<Json<RetentionDraft>, JsonRejection> {
        Ok(Json(
            serde_json::from_value(json!({ "retention": rules })).expect("a draft"),
        ))
    }

    fn at() -> Path<(String, String)> {
        Path(("city".to_owned(), "air".to_owned()))
    }

    /// DM-85: an owner's draft is counted by the broker, per class and kind; what the sweeps
    /// deleted is read the same way.
    #[tokio::test]
    async fn an_owners_draft_and_the_deletions_are_counted_by_the_broker() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/q/retention/[^/]+/preview$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "counts": [{ "type": "https://e.org/Bus", "kind": "entity", "count": 3 }]
            })))
            .mount(&broker)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/q/retention/[^/]+/deleted$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "counts": [{ "type": "https://e.org/Bus", "kind": "history", "count": 7 }]
            })))
            .mount(&broker)
            .await;
        let state = world(&broker, "counted");
        let rows = |counts: &[RetentionCount]| {
            counts
                .iter()
                .map(|c| (c.entity_type.clone(), c.kind.clone(), c.count))
                .collect::<Vec<_>>()
        };
        let Json(preview) = preview_retention(
            owner(&state),
            State(state.clone()),
            at(),
            draft(json!([{ "entityType": "Bus", "current": "PT5M" }])),
        )
        .await
        .expect("counted");
        assert_eq!(
            rows(&preview.counts),
            [("Bus".to_owned(), "entity".to_owned(), 3)]
        );
        let Json(deleted) = retention_deleted(owner(&state), State(state.clone()), at())
            .await
            .expect("counted");
        assert_eq!(
            rows(&deleted.counts),
            [("Bus".to_owned(), "history".to_owned(), 7)]
        );
    }

    /// A body that is no draft, a rule the manifest refuses, and a broker that refuses the
    /// count: each answered before or instead of a count.
    #[tokio::test]
    async fn a_bad_draft_or_a_refusing_broker_is_said_plainly() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/q/retention/[^/]+/preview$"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&broker)
            .await;
        let state = world(&broker, "refused");
        let request = axum::http::Request::builder()
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from("[1]"))
            .expect("a request");
        let rejected = Json::<RetentionDraft>::from_request(request, &()).await;
        assert!(rejected.is_err());
        let answer = preview_retention(owner(&state), State(state.clone()), at(), rejected).await;
        assert!(
            matches!(answer, Err(ApiError::BadRequest(_))),
            "{:?}",
            answer.err()
        );
        let refused = preview_retention(
            owner(&state),
            State(state.clone()),
            at(),
            draft(json!([{ "entityType": "*", "current": "PT5M" }])),
        )
        .await;
        assert!(
            matches!(&refused, Err(ApiError::BadRequest(why)) if why.contains("DM-97")),
            "{:?}",
            refused.err()
        );
        let down = preview_retention(
            owner(&state),
            State(state.clone()),
            at(),
            draft(json!([{ "entityType": "Bus", "current": "PT5M" }])),
        )
        .await;
        assert!(
            matches!(&down, Err(ApiError::Unavailable(why)) if why.contains("did not count")),
            "{:?}",
            down.err()
        );
    }
}
