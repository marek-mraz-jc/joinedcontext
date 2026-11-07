//! `GET /api/v1/projects/{project}/flow-map`: every path data takes through the project, from
//! MQTT broker or DataSource to space and on through derived and KPI pipelines (T-4862).

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::Verb;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::flow_map::{FlowMap, FlowRun, Reader};
use crate::resource::{is_dns1123, is_manifest_name};
use crate::state::AppState;

#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct FlowMapQuery {
    /// Only the paths that read or write this Context Space.
    pub space: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/flow-map",
    summary = "Read Data Flow Map",
    description = "Every MQTT broker, DataSource, Pipeline, Endpoint and Context Space of the project the caller may read, with the edges between them: which source feeds which pipeline, which Endpoint it reads and writes, the Policy that grants the pipelines account each read and write (named only to a caller who reads Policy in that space; or `missing` with the fix), each pipeline's phase and latest run. Another project's Endpoint a pipeline reads is named only, unless the caller reads that project.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        FlowMapQuery,
    ),
    responses(
        (status = 200, description = "The map", body = FlowMap),
        (status = 400, description = "Not a space name", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_flow_map(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<FlowMapQuery>,
) -> Result<Json<FlowMap>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !is_dns1123(&project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let space = query.space.as_deref();
    if let Some(space) = space.filter(|s| !is_manifest_name("ContextSpace", s)) {
        return Err(ApiError::BadRequest(format!(
            "'{space}' is not a space name"
        )));
    }
    let mut map = map_for(&state, &user, &effective, space);
    let pipelines = map.pipelines();
    let runs = last_runs(&pipelines, |name| {
        let state = state.clone();
        let project = project.clone();
        async move { state.pipeline_log.runs(&project, &name, 1).await }
    })
    .await;
    map.attach_runs(runs);
    Ok(Json(map))
}

/// The map as this caller reads it: their grants in the project, and per other project whether
/// they read its Endpoints.
fn map_for(
    state: &AppState,
    user: &CurrentUser,
    effective: &crate::permissions::Effective,
    space: Option<&str>,
) -> FlowMap {
    let may = |kind: &str, space: Option<&str>| effective.may_in(kind, Verb::Read, space);
    let may_project = |other: &str| {
        crate::permissions::for_request(state, &user.0.identity, other).may_read("Endpoint")
    };
    let reader = Reader {
        may: &may,
        may_project: &may_project,
    };
    crate::flow_map::build(&state.mirror, &effective.project, &reader, space)
}

/// The latest run of each named pipeline, read at once; a log that cannot be read for one
/// leaves that pipeline without a last run.
async fn last_runs<F, Fut, E>(pipelines: &[String], read: F) -> BTreeMap<String, FlowRun>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<crate::pipeline_log::Run>, E>>,
    E: std::fmt::Display,
{
    let reads = pipelines.iter().map(|name| {
        let read = read(name.clone());
        async move { (name, read.await) }
    });
    let mut runs = BTreeMap::new();
    for (name, read) in futures_util::future::join_all(reads).await {
        match read {
            Ok(latest) => {
                if let Some(run) = latest.first() {
                    runs.insert(name.clone(), FlowRun::of(run));
                }
            }
            Err(error) => tracing::warn!(pipeline = %name, %error, "the flow map read no runs"),
        }
    }
    runs
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/flow-map", get(get_flow_map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline_log::Run;

    fn run(sent: u64) -> Run {
        Run {
            run: "r1".to_owned(),
            first_at: "2026-10-21T04:00:00Z".to_owned(),
            last_at: "2026-10-21T04:10:00Z".to_owned(),
            sent,
            rejected: 0,
            failed: 0,
            created: None,
            updated: None,
            unchanged: None,
        }
    }

    #[tokio::test]
    async fn one_failed_log_read_leaves_only_that_pipeline_without_a_run() {
        let names = ["broken", "fine", "idle"].map(str::to_owned);
        let runs = last_runs(&names, |name| async move {
            match name.as_str() {
                "broken" => Err("the log is down"),
                "fine" => Ok(vec![run(7)]),
                _ => Ok(Vec::new()),
            }
        })
        .await;
        assert_eq!(runs.keys().collect::<Vec<_>>(), vec!["fine"]);
        assert_eq!(runs["fine"].sent, 7);
    }

    /// Project helsinki, where `reader` may read `kinds`, holding `extra` too.
    fn reader_world(
        reader: &str,
        kinds: serde_json::Value,
        extra: Vec<crate::resource::ResourceEnvelope>,
    ) -> AppState {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let envelope =
            |kind: &str, name: &str, namespace: &str, spec: serde_json::Value| ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new(name, namespace),
                spec,
                status: None,
            };
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let org = crate::permissions::ORG_NAMESPACE;
        state.mirror.upsert(envelope(
            "Role",
            "reader",
            org,
            serde_json::json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            "reader",
            org,
            serde_json::json!({ "subjects": [{ "user": reader }], "role": "reader", "scope": { "project": "helsinki" } }),
        ));
        for env in extra {
            state.mirror.upsert(env);
        }
        state
    }

    fn signed_in(email: &str) -> CurrentUser {
        CurrentUser(crate::auth::session::Session {
            identity: crate::auth::session::Identity {
                client: None,
                subject: format!("f:1:{email}"),
                username: email.to_owned(),
                email: Some(email.to_owned()),
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: 0,
            issued_at: 0,
            signed_in_at: None,
            id_token: String::new(),
            access_expires_at: 0,
            refresh_token: None,
        })
    }

    #[tokio::test]
    async fn the_map_is_for_a_reader_of_the_project_and_a_space_name_only() {
        let state = reader_world("reader@hel.fi", serde_json::json!(["Pipeline"]), Vec::new());
        let ask = |who: &str, project: &str, space: Option<&str>| {
            get_flow_map(
                signed_in(who),
                State(state.clone()),
                Path(project.to_owned()),
                Query(FlowMapQuery {
                    space: space.map(str::to_owned),
                }),
            )
        };
        let map = ask("reader@hel.fi", "helsinki", None)
            .await
            .expect("the map");
        assert!(map.0.nodes.is_empty(), "nothing flows in an empty project");
        assert!(ask("reader@hel.fi", "helsinki", Some("vehicles"))
            .await
            .is_ok());
        assert!(matches!(
            ask("reader@hel.fi", "helsinki", Some("Not a space!")).await,
            Err(ApiError::BadRequest(_))
        ));
        // A stranger, or a name that is no project, learns nothing about it.
        assert!(matches!(
            ask("stranger@hel.fi", "helsinki", None).await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            ask("reader@hel.fi", "Helsinki!", None).await,
            Err(ApiError::NotFound(_))
        ));
    }
}
