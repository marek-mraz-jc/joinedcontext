//! Usage per Endpoint for publishers (T-5118, API/01 §54): one Endpoint's calls per day and
//! surface, every Endpoint of a project over the same window, and a month as CSV. The counts are
//! the gateway's, names and numbers only; whoever may read an Endpoint may read its usage.

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};

use crate::auth::CurrentUser;
use crate::endpoint_usage::{csv, month, usage, window, EndpointUsage, Stored, MAX_DAYS};
use crate::error::ApiError;
use crate::resource::{is_dns1123, ResourceEnvelope};
use crate::state::AppState;

/// The window a Usage tab reads.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct Days {
    /// The days ending today (UTC), 1 to 90; 30 when absent.
    pub days: Option<i64>,
}

/// The month an export reads.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct Month {
    /// `YYYY-MM`, e.g. `2026-10`.
    pub month: String,
}

/// One Endpoint's line in the project's usage.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointTotal {
    pub name: String,
    pub slug: String,
    pub total: u64,
    pub totals: BTreeMap<String, u64>,
    /// The window's calls by kind of caller: person, workload, agent, anonymous, unknown (T-6186).
    pub callers: BTreeMap<String, u64>,
    /// Calls per day, oldest first, one number for every day of the window.
    pub daily: Vec<u64>,
}

/// Every Endpoint of a project the caller may read, over one window.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectUsage {
    pub from: String,
    pub to: String,
    pub endpoints: Vec<EndpointTotal>,
}

fn days_of(query: &Days) -> Result<i64, ApiError> {
    match query.days {
        None => Ok(30),
        Some(days) if (1..=MAX_DAYS).contains(&days) => Ok(days),
        Some(_) => Err(ApiError::BadRequest(format!(
            "days: a whole number from 1 to {MAX_DAYS}, e.g. 30"
        ))),
    }
}

/// The Endpoints of `project` the caller may read, by name; 404 for a project they cannot read.
fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
) -> Result<Vec<ResourceEnvelope>, ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if !is_dns1123(project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let mut endpoints = state.mirror.matching(|env| {
        env.kind == "Endpoint"
            && env.metadata.namespace.as_deref() == Some(project)
            && effective.may_read_in("Endpoint", space_of(env).as_deref())
    });
    endpoints.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
    Ok(endpoints)
}

fn space_of(env: &ResourceEnvelope) -> Option<String> {
    let value = env.spec.get("contextSpaceRef")?;
    value
        .as_str()
        .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
}

async fn stored(
    state: &AppState,
    project: &str,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<Stored>, ApiError> {
    state
        .endpoint_usage
        .read(project, from, to)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "the usage rows were not read");
            ApiError::Unavailable("the usage database did not answer; try again in a minute".into())
        })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/endpoints/{name}/usage",
    summary = "Read Endpoint Usage",
    description = "The Endpoint's answered calls per UTC day and surface (ngsi-ld, ogc, odata, sta, ical, feed, mcp, websocket, download) over the last `days` days, every day present, as the gateways count them (T-5118), and the window's calls by kind of caller (person, workload, agent, anonymous; unknown before the gateway told them apart, T-6186). No address, person or query is kept. Whoever may read the Endpoint may read its usage; anyone else gets 404.",
    tag = "resources",
    params(("project" = String, Path, description = "Project name"), ("name" = String, Path, description = "Endpoint name"), Days),
    responses(
        (status = 200, description = "Calls per day", body = EndpointUsage),
        (status = 400, description = "`days` out of 1 to 90", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such Endpoint the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "The usage database did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_endpoint_usage(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Query(query): Query<Days>,
) -> Result<Json<EndpointUsage>, ApiError> {
    let days = days_of(&query)?;
    if !readable(&state, &user, &project)?
        .iter()
        .any(|e| e.metadata.name == name)
    {
        return Err(ApiError::NotFound(format!(
            "endpoint '{name}' not found in project '{project}'"
        )));
    }
    let (from, to) = window(chrono::Utc::now().date_naive(), days);
    let rows = stored(&state, &project, from, to).await?;
    Ok(Json(usage(&name, from, to, &rows)))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/usage/endpoints",
    summary = "List Endpoint Usage",
    description = "Every Endpoint of the project the caller may read, with its calls over the last `days` days: the total, the total per surface, the total per kind of caller (person, workload, agent, anonymous, unknown; T-6186) and one number per day (T-5118). An Endpoint nobody called is listed with zeros.",
    tag = "resources",
    params(("project" = String, Path, description = "Project name"), Days),
    responses(
        (status = 200, description = "Usage per Endpoint", body = ProjectUsage),
        (status = 400, description = "`days` out of 1 to 90", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "The usage database did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_project_usage(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<Days>,
) -> Result<Json<ProjectUsage>, ApiError> {
    let days = days_of(&query)?;
    let endpoints = readable(&state, &user, &project)?;
    let (from, to) = window(chrono::Utc::now().date_naive(), days);
    let rows = stored(&state, &project, from, to).await?;
    Ok(Json(ProjectUsage {
        from: from.to_string(),
        to: to.to_string(),
        endpoints: endpoints
            .iter()
            .map(|env| line(env, from, to, &rows))
            .collect(),
    }))
}

fn line(env: &ResourceEnvelope, from: NaiveDate, to: NaiveDate, rows: &[Stored]) -> EndpointTotal {
    let one = usage(&env.metadata.name, from, to, rows);
    EndpointTotal {
        slug: env
            .spec
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        name: one.name,
        total: one.total,
        totals: one.totals,
        callers: one.callers,
        daily: one.days.iter().map(|day| day.total).collect(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/usage/endpoints/export",
    summary = "Export Endpoint Usage",
    description = "One month of the project's Endpoint usage as CSV, `day,endpoint,surface,calls`, one line per UTC day, Endpoint and surface with calls, for the Endpoints the caller may read (T-5118).",
    tag = "resources",
    params(("project" = String, Path, description = "Project name"), Month),
    responses(
        (status = 200, description = "text/csv", content_type = "text/csv", body = String),
        (status = 400, description = "`month` is not YYYY-MM", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "The usage database did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn export_project_usage(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<Month>,
) -> Result<Response, ApiError> {
    let (from, to) = month(&query.month)
        .ok_or_else(|| ApiError::BadRequest("month: a month as YYYY-MM, e.g. 2026-10".into()))?;
    let names: Vec<String> = readable(&state, &user, &project)?
        .into_iter()
        .map(|e| e.metadata.name)
        .collect();
    let mut rows = stored(&state, &project, from, to).await?;
    rows.retain(|row| names.contains(&row.name));
    Ok(csv_download(&project, &query.month, &rows))
}

/// The month's rows as a CSV file the browser saves and no cache keeps.
fn csv_download(project: &str, month: &str, rows: &[Stored]) -> Response {
    let file = format!("attachment; filename=\"{project}-usage-{month}.csv\"");
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, file),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        csv(rows),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/endpoints/{name}/usage",
            get(get_endpoint_usage),
        )
        .route(
            "/projects/{project}/usage/endpoints",
            get(list_project_usage),
        )
        .route(
            "/projects/{project}/usage/endpoints/export",
            get(export_project_usage),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_is_a_csv_file_no_cache_keeps() {
        let answer = csv_download("city", "2026-10", &[]);
        let head = |name| answer.headers().get(name).and_then(|v| v.to_str().ok());
        assert_eq!(head(header::CONTENT_TYPE), Some("text/csv; charset=utf-8"));
        assert_eq!(
            head(header::CONTENT_DISPOSITION),
            Some("attachment; filename=\"city-usage-2026-10.csv\"")
        );
        assert_eq!(head(header::CACHE_CONTROL), Some("no-store"));
    }
}
