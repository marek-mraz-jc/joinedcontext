//! `GET /api/v1/projects/{project}/spaces/{space}/quality`: the last data-quality run's report
//! of one space (DM-74, API/01 §27), and `…/quality/checks`: its checks on write over 24 hours
//! and 7 days (DM-94, §27.1). `GET …/endpoints/{name}/quality`: a dataset's quality score
//! (T-5112, §27.2).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::ContextSpaceSpec;
use serde_json::{json, Value};

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::quality::SpaceQuality;
use crate::quality_checks::SpaceChecks;
use crate::resource::is_dns1123;
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/quality",
    summary = "Read Data Quality",
    description = "The last daily run's report of one space: entities checked and invalid, the failing rules with examples, and the freshness of each pipeline writing into it. `{}` before the first run. Example ids only for a caller who reads Entity in the space (DM-74).",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "The report, or `{}` when the space was not checked yet", body = crate::quality::SpaceQuality),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_quality(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    readable_space(&state, &user, &project, &space)?;
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    let Some(quality) = state.quality.get(&project, &space) else {
        return Ok(Json(json!({})));
    };
    let reads_entities = effective.may_read_in("Entity", Some(&space));
    serde_json::to_value(for_caller(quality, reads_entities))
        .map(Json)
        .map_err(|err| ApiError::Internal(err.to_string()))
}

/// The space's manifest spec when the caller may read the space, else the 404 every unreadable
/// space gets.
fn readable_space(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<Value, ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    (is_dns1123(project)
        && effective.may_read_project()
        && effective.may_read_in("ContextSpace", Some(space)))
    .then(|| state.mirror.get(project, "ContextSpace", space))
    .flatten()
    .map(|held| held.spec)
    .ok_or_else(|| {
        ApiError::NotFound(format!(
            "context space '{space}' not found in project '{project}'"
        ))
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/quality/checks",
    summary = "Read Quality Checks",
    description = "The space's quality rules as the gateway checked writes against them, per type and rule over 24 hours and 7 days: passed, rejected, flagged, quarantined, the pass rate, and when a write was last rejected. Names and counts, never a value; `at` is null while no reading of the gateway's counters exists (DM-94).",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "The checks", body = SpaceChecks),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_checks(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<SpaceChecks>, ApiError> {
    let spec = readable_space(&state, &user, &project, &space)?;
    let rules = serde_json::from_value::<ContextSpaceSpec>(spec)
        .map(|spec| spec.quality)
        .unwrap_or_default();
    state.quality_checks.refresh().await;
    // The gateway labels its counters with the space's tenant segment, not its name.
    let segment = crate::spaces::segment(&state.mirror, &project, &space);
    Ok(Json(state.quality_checks.report(&segment, &rules)))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/endpoints/{name}/quality",
    summary = "Read Dataset Quality Score",
    description = "The quality score of the dataset an Endpoint serves, 0 to 100, and its four parts: freshness against the writing pipelines' schedules and health, completeness of the model's required fields, the share of entities that follow the model, and the Endpoint's surfaces answering. Each part has its reason and the time it was measured; a part nothing measures yet has no score. What to fix comes only to a caller who reads the Endpoint's Context Space. Counts and names, never a value (T-5112).",
    tag = "endpoints",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Endpoint name"),
    ),
    responses(
        (status = 200, description = "The score", body = crate::dataset_score::DatasetScore),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such Endpoint the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_endpoint_quality(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<crate::dataset_score::DatasetScore>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    // The Endpoint lives in its space: a grant scoped to another space does not reach it.
    let endpoint = (is_dns1123(&project) && effective.may_read_project())
        .then(|| state.mirror.get(&project, "Endpoint", &name))
        .flatten()
        .filter(|endpoint| {
            serde_json::to_value(endpoint)
                .is_ok_and(|manifest| effective.may_read_manifest("Endpoint", &manifest))
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "endpoint '{name}' not found in project '{project}'"
            ))
        })?;
    let score = crate::dataset_score::of_endpoint(&state, &project, &endpoint).await;
    let publisher = crate::resource::reference_name(&endpoint.spec["contextSpaceRef"])
        .is_some_and(|space| effective.may_read_in("ContextSpace", Some(space)));
    Ok(Json(if publisher { score } else { score.public() }))
}

/// The counts are the space's; which entities they are, and the values they hold (a range, an
/// outlier's id), are for whoever reads its entities.
fn for_caller(mut quality: SpaceQuality, reads_entities: bool) -> SpaceQuality {
    if !reads_entities {
        for rule in &mut quality.rules {
            rule.examples.clear();
        }
        for attribute in quality
            .types
            .iter_mut()
            .flat_map(|kind| kind.attributes.iter_mut())
        {
            attribute.min = None;
            attribute.max = None;
            attribute.outlier_examples.clear();
        }
    }
    quality
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/spaces/{space}/quality",
            get(get_quality),
        )
        .route(
            "/projects/{project}/spaces/{space}/quality/checks",
            get(get_checks),
        )
        .route(
            "/projects/{project}/endpoints/{name}/quality",
            get(get_endpoint_quality),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::{AttributeQuality, RuleCount, TypeQuality};

    fn report() -> SpaceQuality {
        SpaceQuality {
            observed_at: "2026-09-25T02:00:00Z".parse().unwrap(),
            checked: 10,
            invalid: 1,
            truncated: false,
            rules: vec![RuleCount {
                rule: "sh:minCount".into(),
                path: "name".into(),
                count: 1,
                examples: vec!["urn:ngsi-ld:Station:hel.fi:bikes:1".into()],
            }],
            freshness: Vec::new(),
            types: vec![TypeQuality {
                entity_type: "Station".into(),
                count: 10,
                newest: None,
                attributes: vec![AttributeQuality {
                    name: "capacity".into(),
                    present: 9,
                    min: Some(2.0),
                    max: Some(400.0),
                    outliers: 1,
                    outlier_examples: vec!["urn:ngsi-ld:Station:hel.fi:bikes:7".into()],
                }],
            }],
        }
    }

    // DM-74: a caller who reads the space and not its entities learns the counts, not the ids.
    #[test]
    fn example_ids_are_only_for_whoever_reads_the_entities() {
        let hidden = for_caller(report(), false);
        assert!(hidden.rules[0].examples.is_empty());
        let capacity = &hidden.types[0].attributes[0];
        assert_eq!((capacity.min, capacity.max), (None, None));
        assert!(capacity.outlier_examples.is_empty());
        assert_eq!((capacity.present, capacity.outliers), (9, 1));
        assert_eq!((hidden.checked, hidden.rules[0].count), (10, 1));
        assert_eq!(for_caller(report(), true), report());
    }

    // T-5112: a stranger learns nothing, an Endpoint reader the public form, and a reader of
    // its Context Space what to fix.
    #[tokio::test]
    async fn the_endpoint_score_is_shaped_by_what_the_caller_reads() {
        use crate::auth::{session::Session, Identity};
        use crate::permissions::ORG_NAMESPACE;
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let envelope = |kind: &str, name: &str, namespace: &str, spec: Value| ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        };
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(envelope(
            "Endpoint",
            "stations",
            "oulu",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" } }),
        ));
        for (role, kinds, user) in [
            ("endpoint-reader", json!(["Endpoint"]), "reader@hel.fi"),
            (
                "space-reader",
                json!(["Endpoint", "ContextSpace"]),
                "owner@hel.fi",
            ),
        ] {
            state.mirror.upsert(envelope(
                "Role",
                role,
                ORG_NAMESPACE,
                json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
            ));
            state.mirror.upsert(envelope(
                "RoleBinding",
                role,
                ORG_NAMESPACE,
                json!({ "subjects": [{ "user": user }], "role": role, "scope": { "project": "oulu" } }),
            ));
        }
        let mut stale = report();
        stale.freshness.push(crate::quality::Freshness {
            pipeline: "stations".into(),
            entity_type: "Station".into(),
            newest: None,
            target_seconds: Some(3_600),
            state: crate::quality::FreshState::Stale,
            paused: false,
        });
        state.quality.put("oulu", "bikes", stale);
        let user = |email: &str| {
            CurrentUser(Session {
                identity: Identity {
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
        };
        let ask = |email: &str, project: &str, name: &str| {
            get_endpoint_quality(
                user(email),
                State(state.clone()),
                Path((project.to_owned(), name.to_owned())),
            )
        };

        for (email, project, name) in [
            ("stranger@hel.fi", "oulu", "stations"),
            ("reader@hel.fi", "oulu", "nothing"),
            ("reader@hel.fi", "Oulu!", "stations"),
        ] {
            assert!(
                matches!(ask(email, project, name).await, Err(ApiError::NotFound(_))),
                "{email} {project}/{name}"
            );
        }
        let Ok(Json(public)) = ask("reader@hel.fi", "oulu", "stations").await else {
            panic!("an Endpoint reader reads its score");
        };
        assert!(public.parts.iter().all(|part| part.fix.is_none()));
        let Ok(Json(owned)) = ask("owner@hel.fi", "oulu", "stations").await else {
            panic!("a space reader reads its score");
        };
        assert!(owned.parts.iter().any(|part| part.fix.is_some()));
        assert_eq!(owned.score, public.score);
    }
}
