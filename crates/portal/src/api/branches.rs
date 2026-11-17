//! Branch projects (ARCH/TEN-12, CHG-15; T-7213): `POST /api/v1/projects/{project}/branches`
//! opens a branch of `{project}`, a project of its own with a random id, the name `b-{id}` and a
//! lifetime, and `POST /api/v1/projects/{project}/branches/{id}/renewal` renews one by 7 days, at
//! most to 30. A branch inherits no grant of its parent, so it answers only on its own id address
//! (RTE-05) and only to the rights its creator holds in the parent.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use jc_core::kinds::Verb;
use jc_types::ProjectId;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::project_ids::{self, BranchRow};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// How many live branches one project keeps (`JC_PORTAL_BRANCHES_PER_PROJECT`, 0 to 100, default
/// 5; 0 opens none) and how many days a new one lives (`JC_PORTAL_BRANCH_LIFETIME_DAYS`, 1 to 30,
/// default 7): the deployment's configuration of TEN-12 and CHG-15.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchSettings {
    pub per_parent: u32,
    pub lifetime_days: u32,
}

impl Default for BranchSettings {
    fn default() -> Self {
        Self {
            per_parent: 5,
            lifetime_days: 7,
        }
    }
}

/// The settings from the two variables' values, each within its range; the variable that is out
/// of range or not a number is named.
pub fn parse(
    per_parent: Option<&str>,
    lifetime_days: Option<&str>,
) -> Result<BranchSettings, (&'static str, String)> {
    let number = |var: &'static str, value: Option<&str>, default: u32, max: u32, min: u32| {
        value.map_or(Ok(default), |value| {
            value
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (min..=max).contains(n))
                .ok_or_else(|| {
                    (
                        var,
                        format!("'{value}' is not a whole number from {min} to {max}"),
                    )
                })
        })
    };
    let default = BranchSettings::default();
    Ok(BranchSettings {
        per_parent: number(
            "JC_PORTAL_BRANCHES_PER_PROJECT",
            per_parent,
            default.per_parent,
            100,
            0,
        )?,
        lifetime_days: number(
            "JC_PORTAL_BRANCH_LIFETIME_DAYS",
            lifetime_days,
            default.lifetime_days,
            30,
            1,
        )?,
    })
}

/// A branch project as its doors answer it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    /// The branch's project id in the path form, what its id address carries (RTE-05).
    #[schema(example = "bxq4m2c7d5t6v3n8k9p0r1s2wa")]
    pub id: String,
    /// The branch's project name, `b-` and its id: only its own id address reaches it.
    pub name: String,
    /// The project it branches from.
    pub parent: String,
    /// When the branch stops answering unless renewed (CHG-15).
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
}

impl Branch {
    fn of(row: BranchRow, parent: &str) -> Self {
        Self {
            id: row.id.path_segment(),
            name: row.name,
            parent: parent.to_owned(),
            expires_at: DateTime::from_timestamp(row.expires_at, 0).unwrap_or_default(),
        }
    }
}

fn registry(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal keeps no project registry (no database)".into())
    })
}

fn unread(err: sqlx::Error) -> ApiError {
    tracing::warn!(error = %err, "the branch registry was not read or written");
    ApiError::Unavailable("the branch registry could not be read or written".into())
}

/// The caller's rights in the parent once the caller may read it: a project the caller cannot
/// see, the Organization's reserved namespace (TEN-11) and a branch (whose name never resolves
/// here, RTE-05) all answer like an unknown project.
fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
) -> Result<crate::permissions::Effective, ApiError> {
    let unknown = || ApiError::NotFound(format!("project '{project}' not found"));
    if !is_dns1123(project)
        || project == crate::permissions::ORG_NAMESPACE
        || jc_router::branch_name(project)
    {
        return Err(unknown());
    }
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if !effective.may_read_project() {
        return Err(unknown());
    }
    Ok(effective)
}

async fn key(state: &AppState, project: &str) -> Result<jc_types::ProjectKey, ApiError> {
    project_ids::required(registry(state)?, &state.mirror, project, "branch").await
}

/// `POST /api/v1/projects/{project}/branches`: opens a branch of the project for the caller.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/branches",
    summary = "Open A Branch",
    description = "Opens a branch of the project: a new project of its own, empty, with no grant of \
        the parent, that answers only on its own id address and lives 7 days unless renewed \
        (TEN-12, CHG-15). Needs a role that may propose in the project.",
    tag = "resources",
    params(("project" = String, Path, description = "The project to branch from, e.g. helsinki")),
    responses(
        (status = 201, description = "The branch, with its id and expiry", body = Branch),
        (status = 401, description = "Unauthorized", body = crate::error::ProblemDetails),
        (status = 403, description = "No role here may propose", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project for this caller", body = crate::error::ProblemDetails),
        (status = 409, description = "The project keeps as many live branches as allowed", body = crate::error::ProblemDetails),
        (status = 503, description = "No project registry", body = crate::error::ProblemDetails)
    )
)]
pub async fn open_branch(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<(StatusCode, Json<Branch>), ApiError> {
    let effective = readable(&state, &user, &project)?;
    if !effective.may_propose_anything() {
        return Err(ApiError::Denied(format!(
            "opening a branch of project {project} needs a role with propose on some kind there"
        )));
    }
    let parent = key(&state, &project).await?;
    let settings = state.config.branches;
    let row = project_ids::open_branch(
        registry(&state)?,
        parent,
        &user.0.identity.subject,
        settings.lifetime_days,
        settings.per_parent,
    )
    .await
    .map_err(unread)?
    .ok_or_else(|| {
        ApiError::Conflict(format!(
            "project {project} keeps {} live branches, as many as this installation allows; \
             let one expire or ask an operator to raise branches per project",
            settings.per_parent
        ))
    })?;
    tracing::info!(%project, branch = %row.name, "branch opened");
    Ok((StatusCode::CREATED, Json(Branch::of(row, &project))))
}

/// `POST /api/v1/projects/{project}/branches/{id}/renewal`: renews a live branch by 7 days.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/branches/{id}/renewal",
    summary = "Renew A Branch",
    description = "Renews a live branch by 7 days, never past 30 days from its creation; each \
        renewal is audited (CHG-15). Its creator renews it while it may propose in the parent; an \
        approver of the parent renews any; an agent renews only a branch it opened.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "The parent project, e.g. helsinki"),
        ("id" = String, Path, description = "The branch's id in the path form, e.g. bxq4m2c7d5t6v3n8k9p0r1s2wa")
    ),
    responses(
        (status = 200, description = "The branch with its new expiry", body = Branch),
        (status = 401, description = "Unauthorized", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not renew this branch", body = crate::error::ProblemDetails),
        (status = 404, description = "No live branch of that id under the project", body = crate::error::ProblemDetails),
        (status = 409, description = "The branch already lives to its 30 days", body = crate::error::ProblemDetails),
        (status = 503, description = "No project registry", body = crate::error::ProblemDetails)
    )
)]
pub async fn renew_branch(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<Branch>, ApiError> {
    let effective = readable(&state, &user, &project)?;
    let parent = key(&state, &project).await?;
    let gone = || ApiError::NotFound(format!("no live branch '{id}' of project {project}"));
    let id = ProjectId::from_path_segment(&id).map_err(|_| gone())?;
    let pool = registry(&state)?;
    let branch = project_ids::live_branch_of(pool, parent, id)
        .await
        .map_err(unread)?
        .ok_or_else(gone)?;
    let identity = &user.0.identity;
    let own = branch.created_by == identity.subject && effective.may_propose_anything();
    // An agent acts within its own branch only (CHG-15); a person approving the parent's
    // Changes renews any branch of it.
    let approver = identity.client.is_none() && effective.may_anything(Verb::Approve);
    if !own && !approver {
        return Err(ApiError::Denied(format!(
            "renewing branch {} needs to be its creator with propose in project {project}, or a \
             person who approves Changes there",
            branch.name
        )));
    }
    let expires_at = project_ids::renew_branch(pool, parent, id, &identity.subject)
        .await
        .map_err(unread)?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "branch {} already lives to 30 days from its creation, the most CHG-15 allows",
                branch.name
            ))
        })?;
    tracing::info!(%project, branch = %branch.name, "branch renewed");
    Ok(Json(Branch::of(
        BranchRow {
            expires_at,
            ..branch
        },
        &project,
    )))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/branches", post(open_branch))
        .route(
            "/projects/{project}/branches/{id}/renewal",
            post(renew_branch),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_and_refuse_out_of_range_values() {
        assert_eq!(parse(None, None), Ok(BranchSettings::default()));
        assert_eq!(
            parse(Some("0"), Some("30")),
            Ok(BranchSettings {
                per_parent: 0,
                lifetime_days: 30
            })
        );
        for (per_parent, lifetime, var) in [
            (Some("101"), None, "JC_PORTAL_BRANCHES_PER_PROJECT"),
            (Some("-1"), None, "JC_PORTAL_BRANCHES_PER_PROJECT"),
            (None, Some("0"), "JC_PORTAL_BRANCH_LIFETIME_DAYS"),
            (None, Some("31"), "JC_PORTAL_BRANCH_LIFETIME_DAYS"),
            (None, Some("seven"), "JC_PORTAL_BRANCH_LIFETIME_DAYS"),
        ] {
            assert_eq!(parse(per_parent, lifetime).map_err(|(v, _)| v), Err(var));
        }
    }
}
