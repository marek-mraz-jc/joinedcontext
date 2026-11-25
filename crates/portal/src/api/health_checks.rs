//! The health log (T-5067): `GET /projects/{project}/health-checks` lists the checks of one
//! project's resources the caller may read, `GET /organization/health-checks` every project's
//! for an administrator of the organization. The rows are written by the leader's alert run.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::health_log::Check;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The checks, errors first, then warnings, then ok.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = HealthCheckList)]
pub struct HealthChecks {
    pub items: Vec<Check>,
}

fn unavailable(error: sqlx::Error) -> ApiError {
    tracing::warn!(%error, "the health log could not be read");
    ApiError::Unavailable("the health log could not be read; try again in a minute".into())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/health-checks",
    summary = "List Project Health Checks",
    description = "Every check of the project's pipelines, Context Spaces, Endpoints, models and data sources the caller may read (T-5067): its status, since when it holds, when it was last ok, what it found with credentials masked, a fix hint and the statuses of the last 7 days. A project the caller cannot read is 404.",
    tag = "system",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The project's checks, errors first", body = HealthChecks),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
        (status = 503, description = "The health log could not be read", body = ProblemDetails)
    )
)]
pub async fn list_project(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<HealthChecks>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !is_dns1123(&project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let items = state
        .health_log
        .list(Some(&project), jc_time::TimestampTz::now())
        .await
        .map_err(unavailable)?
        .into_iter()
        .filter(|check| {
            let kind = check.kind.manifest_kind();
            match state.mirror.get(&project, kind, &check.name) {
                Some(manifest) => serde_json::to_value(&manifest)
                    .is_ok_and(|value| effective.may_read_manifest(kind, &value)),
                None => effective.may_read(kind),
            }
        })
        .collect();
    Ok(Json(HealthChecks { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/health-checks",
    summary = "List Organization Health Checks",
    description = "Every project's checks (T-5067), errors first. Only an administrator of the organization: approve and delete on RoleBinding at organization scope (PF-03).",
    tag = "system",
    responses(
        (status = 200, description = "Every project's checks, errors first", body = HealthChecks),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the organization", body = ProblemDetails),
        (status = 503, description = "The health log could not be read", body = ProblemDetails)
    )
)]
pub async fn list_all(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<HealthChecks>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, ORG_NAMESPACE);
    if !effective.administers_organization() {
        return Err(ApiError::Denied(
            "only an administrator of the organization reads every project's health: it needs \
             approve and delete on RoleBinding at organization scope (PF-03)"
                .into(),
        ));
    }
    let items = state
        .health_log
        .list(None, jc_time::TimestampTz::now())
        .await
        .map_err(unavailable)?;
    Ok(Json(HealthChecks { items }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/health-checks", get(list_project))
        .route("/organization/health-checks", get(list_all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::{Identity, Session};
    use crate::health_log::{Finding, HealthStore, Kind};
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn pia() -> CurrentUser {
        CurrentUser(Session {
            identity: Identity {
                client: None,
                subject: "sub-pia".into(),
                username: "pia".into(),
                email: Some("pia@hel.fi".into()),
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            stepped_up_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    /// pia reads the Pipelines of `air` and nothing else.
    fn world() -> AppState {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        for (kind, name, namespace, spec) in [
            (
                "Role",
                "pipeline-reader",
                ORG_NAMESPACE,
                serde_json::json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
            ),
            (
                "RoleBinding",
                "pia-reads",
                ORG_NAMESPACE,
                serde_json::json!({ "subjects": [{ "user": "pia@hel.fi" }], "role": "pipeline-reader", "scope": { "project": "air" } }),
            ),
            ("Pipeline", "feed", "air", serde_json::json!({})),
        ] {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.into(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new(name, namespace),
                spec,
                status: None,
            });
        }
        state
    }

    /// A check of a kind the caller reads is listed whether its manifest is there or gone; a
    /// check of a kind they do not read is not.
    #[tokio::test]
    async fn the_list_holds_only_the_checks_the_caller_reads() {
        let state = world();
        let now = jc_time::TimestampTz::now();
        state
            .health_log
            .record(
                now,
                vec![
                    Finding::new("air", Kind::Pipeline, "feed", "reconcile"),
                    Finding::new("air", Kind::Pipeline, "gone", "reconcile"),
                    Finding::new("air", Kind::Endpoint, "hidden", "reconcile"),
                ],
            )
            .await
            .expect("recorded");
        let Json(list) = list_project(pia(), State(state.clone()), Path("air".into()))
            .await
            .expect("listed");
        let mut names: Vec<&str> = list.items.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["feed", "gone"]);
        let refused = list_project(pia(), State(state), Path("Not_A_Project".into())).await;
        assert!(matches!(refused, Err(ApiError::NotFound(_))));
    }

    /// A health log that cannot be read is a 503 that says to try again.
    #[tokio::test]
    async fn an_unreadable_health_log_is_503() {
        let mut state = world();
        // A project of an Organization: its key is read from the database.
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Project".into(),
            metadata: ObjectMeta::new("air", ORG_NAMESPACE),
            spec: serde_json::json!({ "organizationRef": "bbsk" }),
            status: None,
        });
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy("postgres://portal@127.0.0.1:1/never-reached")
            .expect("a lazy pool");
        state.health_log = std::sync::Arc::new(HealthStore::new(Some(pool), state.mirror.clone()));
        let refused = list_project(pia(), State(state), Path("air".into())).await;
        assert!(matches!(refused, Err(ApiError::Unavailable(m)) if m.contains("try again")));
    }
}
