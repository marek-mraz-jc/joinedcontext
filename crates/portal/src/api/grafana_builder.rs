//! The dashboard builder's doors (owner 2026-11-08: "let the users manually create these
//! dashboards in the UI"). A person who may propose a Dashboard in the project builds one from
//! choices ([`Board`]), previews each panel as themselves, and saves it as a Change that writes
//! `projects/{project}/grafana/{uid}.json` (`crate::grafana_builder`). The approved file reaches
//! the dashboards service through the reconciler, and the dashboard page reads it from there.
//!
//! Every door re-checks propose on Dashboard and read on each Endpoint a choice names. The browser
//! sends no query: the Portal compiles one from the choices.

use std::collections::BTreeMap;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{IntoParams, ToSchema};

use crate::api::grafana::{
    is_public, member, model_attribute, readable_endpoint, refused, slug_of, title_of_endpoint,
};
use crate::api::grafana_dashboards::{
    checked_query, client, organization_held, panel_view, person_for, PanelData, VariableOption,
    VariableValues,
};
use crate::auth::{CurrentUser, Identity};
use crate::change::Change;
use crate::error::{ApiError, ProblemDetails};
use crate::grafana::{space_of, Client};
use crate::grafana_builder::{
    self as builder, check_filter, check_shape, compile, decompile, Board, Source, ValueKind,
};
use crate::permissions::Effective;
use crate::state::AppState;

/// The kind the builder's rights are judged on, as the visualization page's.
const KIND: &str = "Dashboard";

/// Where the builder starts: whether the caller may build here, and the dashboards whose
/// Change waits for approval.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuilderStart {
    /// The caller may propose dashboards in the project.
    pub may_edit: bool,
    /// Open Changes of builder dashboards; empty for a caller who may not edit.
    pub pending: Vec<PendingDashboard>,
    /// What the forms offer, the server's lists: refresh intervals, units, colours.
    pub refreshes: Vec<String>,
    pub units: Vec<String>,
    pub colors: Vec<String>,
}

/// A dashboard whose Change waits for approval.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingDashboard {
    pub uid: String,
    /// The Change's title, which names the dashboard.
    pub title: String,
    /// The Change, `chg-…`.
    pub change: String,
    /// Where the merge request is read.
    pub url: String,
    /// The Change's branch, where its file is read; a retry's branch carries a suffix.
    #[serde(skip)]
    pub branch: String,
}

/// One builder dashboard, for its next edit.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuilderDashboard {
    pub uid: String,
    pub board: Board,
    /// The file's revision on the default branch, sent back with the edit; none while only a
    /// Change holds the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingDashboard>,
}

/// A new dashboard.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({ "board": {
        "title": "Bike stations", "refresh": "5m",
        "variables": [{ "name": "status", "label": "Status", "endpoint": "helsinki-bikes",
                        "type": "BikeHireDockingStation", "attribute": "status", "includeAll": true }],
        "panels": [{ "id": 1, "kind": "stat", "title": "Free bikes",
                     "grid": { "x": 0, "y": 0, "w": 12, "h": 8 },
                     "endpoint": "helsinki-bikes", "type": "BikeHireDockingStation",
                     "attribute": "availableBikeNumber", "aggregation": "avg",
                     "filters": [{ "attribute": "status", "op": "eq", "variable": "status" }],
                     "unit": "none", "decimals": 0,
                     "thresholds": { "base": "green", "steps": [{ "value": 5, "color": "red" }] } }]
    } }))]
#[serde(deny_unknown_fields)]
pub struct NewBoard {
    pub board: Board,
}

/// An edit of a saved dashboard.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({ "board": {
        "title": "Bike stations", "refresh": "5m",
        "variables": [{ "name": "status", "label": "Status", "endpoint": "helsinki-bikes",
                        "type": "BikeHireDockingStation", "attribute": "status", "includeAll": true }],
        "panels": [{ "id": 1, "kind": "stat", "title": "Free bikes",
                     "grid": { "x": 0, "y": 0, "w": 12, "h": 8 },
                     "endpoint": "helsinki-bikes", "type": "BikeHireDockingStation",
                     "attribute": "availableBikeNumber", "aggregation": "avg",
                     "filters": [{ "attribute": "status", "op": "eq", "variable": "status" }],
                     "unit": "none", "decimals": 0,
                     "thresholds": { "base": "green", "steps": [{ "value": 5, "color": "red" }] } }]
    }, "base": "3f2a9c" }))]
#[serde(deny_unknown_fields)]
pub struct EditedBoard {
    pub board: Board,
    /// The `base` the edit started from.
    pub base: String,
}

/// What a save proposed.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuilderSaved {
    pub uid: String,
    pub change: Change,
}

/// One panel of an unsaved board, run as the caller.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({ "board": {
        "title": "Bike stations", "refresh": "5m",
        "variables": [{ "name": "status", "label": "Status", "endpoint": "helsinki-bikes",
                        "type": "BikeHireDockingStation", "attribute": "status", "includeAll": true }],
        "panels": [{ "id": 1, "kind": "stat", "title": "Free bikes",
                     "grid": { "x": 0, "y": 0, "w": 12, "h": 8 },
                     "endpoint": "helsinki-bikes", "type": "BikeHireDockingStation",
                     "attribute": "availableBikeNumber", "aggregation": "avg",
                     "filters": [{ "attribute": "status", "op": "eq", "variable": "status" }],
                     "unit": "none", "decimals": 0,
                     "thresholds": { "base": "green", "steps": [{ "value": 5, "color": "red" }] } }]
    }, "panel": 1, "vars": { "status": ["open"] } }))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreviewAsked {
    pub board: Board,
    pub panel: u32,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    /// The values picked per variable (`$__all` for All).
    #[serde(default)]
    pub vars: BTreeMap<String, Vec<String>>,
}

/// The preview of one panel: the panel as the dashboard page draws it, and its data.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanelPreview {
    #[schema(value_type = Object)]
    pub panel: Value,
    #[serde(flatten)]
    pub data: PanelData,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ValuesAsked {
    pub endpoint: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub attribute: String,
}

fn shape(refusal: builder::Refusal) -> ApiError {
    refused(&refusal.field, refusal.detail)
}

fn body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    match body {
        Ok(Json(body)) => Ok(body),
        // A member the builder does not take, a query among them, names itself here.
        Err(JsonRejection::JsonDataError(err)) => Err(refused("body", err.body_text())),
        Err(err) => Err(ApiError::BadRequest(err.body_text())),
    }
}

/// The caller's rights, checked for propose on Dashboard: 404 outside the project, 403 inside it.
fn proposer(state: &AppState, identity: &Identity, project: &str) -> Result<Effective, ApiError> {
    let effective = member(state, identity, project)?;
    effective.check(KIND, Verb::Propose, None)?;
    Ok(effective)
}

fn branch_of(project: &str, uid: &str) -> String {
    format!("portal/grafana-{project}-{uid}")
}

/// The uid a builder branch is for: `portal/grafana-{project}-{uid}`, a retry's `_nonce` after it.
fn uid_of_branch<'a>(project: &str, branch: &'a str) -> Option<&'a str> {
    let rest = branch.strip_prefix(&branch_of(project, ""))?;
    let uid = rest.split('_').next()?;
    builder::is_builder_uid(uid).then_some(uid)
}

fn no_dashboard(project: &str, uid: &str) -> ApiError {
    ApiError::NotFound(format!("dashboard '{uid}' not found in {project}"))
}

/// The open Changes of builder dashboards in the project.
async fn pending(
    state: &AppState,
    shako: &crate::forge::ShakoClient,
    project: &str,
) -> Result<Vec<PendingDashboard>, ApiError> {
    let pulls = match shako.list_pull_requests("open").await {
        Ok(pulls) => pulls,
        Err(crate::forge::GitError::NotFound) => Vec::new(),
        Err(err) => return Err(err.into()),
    };
    Ok(pulls
        .into_iter()
        .filter_map(|pr| {
            let uid = uid_of_branch(project, &pr.head_branch)?.to_owned();
            Some(PendingDashboard {
                uid,
                title: pr.title.clone(),
                change: crate::api::changes::change_meta(state, shako, pr.number, project).name,
                url: pr.url,
                branch: pr.head_branch.clone(),
            })
        })
        .collect())
}

fn forge(
    state: &AppState,
    project: &str,
) -> Result<std::sync::Arc<crate::forge::ShakoClient>, ApiError> {
    state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/builder",
    summary = "Read The Dashboard Builder",
    description = "Whether the caller may build dashboards in the project, the builder dashboards whose Change waits for approval, and the refresh intervals, units and colours the forms offer. Changes no data.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The builder's start", body = BuilderStart),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project the caller has a part in", body = ProblemDetails),
    )
)]
pub async fn builder_start(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<BuilderStart>, ApiError> {
    let effective = member(&state, &user.0.identity, &project)?;
    let may_edit = effective.may(KIND, Verb::Propose);
    let pending = match (may_edit, state.forge_for(&project)) {
        (true, Some(shako)) => pending(&state, &shako, &project).await?,
        _ => Vec::new(),
    };
    let words = |list: &[&str]| list.iter().map(|w| (*w).to_owned()).collect();
    Ok(Json(BuilderStart {
        may_edit,
        pending,
        refreshes: words(&builder::REFRESHES),
        units: words(&builder::UNITS),
        colors: words(&builder::COLORS),
    }))
}

/// Every choice of `board` checked against the project: each Endpoint one the caller may read
/// (403 naming the field), each type, attribute and filter one its model has (422).
fn check_choices(
    state: &AppState,
    effective: &Effective,
    project: &str,
    board: &Board,
) -> Result<Vec<Vec<ValueKind>>, ApiError> {
    let endpoint = |field: &str, name: &str| {
        readable_endpoint(state, effective, project, name).ok_or_else(|| {
            ApiError::Denied(format!(
                "{field}: {name} is not an Endpoint of {project} you may read; choose another from the list, or ask a project owner for read access"
            ))
        })
    };
    for (i, variable) in board.variables.iter().enumerate() {
        let at = format!("variables[{i}].");
        let found = endpoint(&format!("{at}endpoint"), &variable.endpoint)?;
        let (kind, value_type) = model_attribute(
            state,
            project,
            space_of(&found),
            &variable.entity_type,
            &variable.attribute,
            &at,
        )?;
        if kind != "Property" || ValueKind::of(value_type.as_deref()) != ValueKind::Text {
            return Err(refused(
                &format!("{at}attribute"),
                format!(
                    "{} is not text; a variable offers text values",
                    variable.attribute
                ),
            ));
        }
    }
    let mut kinds = Vec::new();
    for (i, panel) in board.panels.iter().enumerate() {
        let at = format!("panels[{i}].");
        let Some(drawn) = panel.kind.drawn_as() else {
            // A text panel reads nothing.
            kinds.push(Vec::new());
            continue;
        };
        let found = endpoint(&format!("{at}endpoint"), &panel.endpoint)?;
        let space = space_of(&found);
        let (kind, value_type) = model_attribute(
            state,
            project,
            space,
            &panel.entity_type,
            &panel.attribute,
            &at,
        )?;
        if !drawn.draws(&kind, value_type.as_deref()) {
            return Err(refused(
                &format!("{at}kind"),
                format!(
                    "{} cannot draw {}; choose another kind",
                    panel.kind.words(),
                    panel.attribute
                ),
            ));
        }
        if let Some(bars) = &panel.barchart {
            // The type is the model's: the attribute check above found it.
            let text = state
                .model_schemas
                .get(project, space)
                .and_then(|model| model.attributes(&panel.entity_type))
                .and_then(|class| {
                    class
                        .attributes
                        .iter()
                        .find(|found| found.name == bars.x_field)
                        .map(|found| {
                            found.kind == "Property"
                                && ValueKind::of(found.value_type.as_deref()) == ValueKind::Text
                        })
                });
            if text != Some(true) {
                return Err(refused(
                    &format!("{at}barchart.xField"),
                    format!(
                        "{} is not a text attribute of {}; name the bars by one such as name",
                        bars.x_field, panel.entity_type
                    ),
                ));
            }
        }
        let mut filters = Vec::new();
        for (k, filter) in panel.filters.iter().enumerate() {
            let here = format!("{at}filters[{k}]");
            let (kind, value_type) = model_attribute(
                state,
                project,
                space,
                &panel.entity_type,
                &filter.attribute,
                &format!("{here}."),
            )?;
            if kind != "Property" {
                return Err(refused(
                    &format!("{here}.attribute"),
                    format!("{} is a {kind}; filter by a value", filter.attribute),
                ));
            }
            let value_kind = ValueKind::of(value_type.as_deref());
            check_filter(&here, filter, value_kind).map_err(shape)?;
            filters.push(value_kind);
        }
        kinds.push(filters);
    }
    Ok(kinds)
}

/// The datasource each panel reads its Endpoint through, made for a public one that has none;
/// a closed Endpoint without one is refused by field, as the visualization page refuses it.
async fn sources(
    state: &AppState,
    client: &Client<'_>,
    org: i64,
    project: &str,
    board: &Board,
    filters: Vec<Vec<ValueKind>>,
) -> Result<Vec<Source>, ApiError> {
    let gateway = state.config.gateway_url.as_deref();
    let mut held: BTreeMap<String, String> = BTreeMap::new();
    let mut out = Vec::new();
    for ((i, panel), filters) in board.panels.iter().enumerate().zip(filters) {
        if panel.kind.drawn_as().is_none() {
            out.push(Source {
                datasource: String::new(),
                named: false,
                filters,
            });
            continue;
        }
        let endpoint = state
            .mirror
            .get(project, "Endpoint", &panel.endpoint)
            .ok_or_else(|| no_dashboard(project, &panel.endpoint))?;
        let datasource = match held.get(&panel.endpoint) {
            Some(uid) => uid.clone(),
            None => {
                let source = crate::grafana::Source {
                    name: &panel.endpoint,
                    slug: slug_of(&endpoint),
                    public: is_public(&endpoint),
                };
                let uid = client
                    .datasource_for(org, &source, gateway)
                    .await?
                    .ok_or_else(|| {
                        refused(
                            &format!("panels[{i}].endpoint"),
                            format!(
                                "the dashboards service cannot read the private Endpoint {} yet; ask your \
                                 operator to add it to this project's dashboards, or choose another",
                                title_of_endpoint(&endpoint)
                            ),
                        )
                    })?;
                held.insert(panel.endpoint.clone(), uid.clone());
                uid
            }
        };
        let named = state
            .model_schemas
            .get(project, space_of(&endpoint))
            .and_then(|model| model.attributes(&panel.entity_type))
            .is_some_and(|class| class.attributes.iter().any(|a| a.name == "name"));
        out.push(Source {
            datasource,
            named,
            filters,
        });
    }
    Ok(out)
}

/// The person's token for a members-only Endpoint, `None` for a public one; `Err` is the
/// panel's error line.
async fn person(
    state: &AppState,
    headers: &HeaderMap,
    project: &str,
    endpoint: &str,
) -> Result<Option<String>, String> {
    let Some(found) = state.mirror.get(project, "Endpoint", endpoint) else {
        return Ok(None);
    };
    if is_public(&found) {
        return Ok(None);
    }
    person_for(state, headers, &[slug_of(&found).to_owned()]).await
}

/// The frames of a `POST /api/ds/query` answer, each with its `refId`.
fn frames_of(answer: &Value) -> Vec<Value> {
    answer["results"]
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(ref_id, result)| {
            result["frames"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |mut frame| {
                    if frame.get("refId").is_none() {
                        frame["refId"] = json!(ref_id);
                    }
                    frame
                })
        })
        .collect()
}

/// The current values of one attribute, read as the caller through the dashboards service.
async fn read_values(
    state: &AppState,
    client: &Client<'_>,
    org: i64,
    headers: &HeaderMap,
    project: &str,
    variable: &builder::BoardVariable,
) -> Result<Vec<String>, ApiError> {
    let endpoint = state
        .mirror
        .get(project, "Endpoint", &variable.endpoint)
        .ok_or_else(|| no_dashboard(project, &variable.endpoint))?;
    let source = crate::grafana::Source {
        name: &variable.endpoint,
        slug: slug_of(&endpoint),
        public: is_public(&endpoint),
    };
    let Some(datasource) = client
        .datasource_for(org, &source, state.config.gateway_url.as_deref())
        .await?
    else {
        return Err(refused(
            "endpoint",
            format!(
                "the dashboards service cannot read the private Endpoint {} yet",
                title_of_endpoint(&endpoint)
            ),
        ));
    };
    let token = person(state, headers, project, &variable.endpoint)
        .await
        .map_err(ApiError::Unavailable)?;
    let query = json!({
        "refId": "A",
        "datasource": { "type": "ngsild-grafana-datasource", "uid": datasource },
        "queryType": "entity",
        "format": "table",
        "entityType": variable.entity_type,
        "attributes": [variable.attribute],
        "limit": 1000,
    });
    let answer = client
        .query(
            org,
            &[query],
            ("now-6h", "now"),
            &BTreeMap::new(),
            token.as_deref(),
        )
        .await?;
    if let Some(error) = answer["error"].as_str() {
        return Err(ApiError::Unavailable(error.to_owned()));
    }
    let values = frames_of(&answer)
        .iter()
        .flat_map(|frame| {
            frame["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|field| field["name"] == variable.attribute.as_str())
                .flat_map(|field| field["values"].as_array().cloned().unwrap_or_default())
                .collect::<Vec<_>>()
        })
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    Ok(builder::offered(values))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/builder/values",
    summary = "Read A Builder Variable's Values",
    description = "The current values of one text attribute of a type on an Endpoint the caller may read, read with the caller's rights: what a dashboard variable over it offers. Needs propose on Dashboard. Changes no data.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name"), ValuesAsked),
    responses(
        (status = 200, description = "The values", body = VariableValues),
        (status = 400, description = "An unknown query parameter", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not build dashboards, or may not read the Endpoint", body = ProblemDetails),
        (status = 404, description = "No such project the caller has a part in", body = ProblemDetails),
        (status = 409, description = "No dashboards service for the project", body = ProblemDetails),
        (status = 422, description = "A type or attribute the model does not have", body = ProblemDetails),
    )
)]
pub async fn builder_values(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    asked: Result<Query<ValuesAsked>, QueryRejection>,
) -> Result<Json<VariableValues>, ApiError> {
    let effective = proposer(&state, &user.0.identity, &project)?;
    let Query(asked) = asked.map_err(|err| ApiError::BadRequest(err.body_text()))?;
    let variable = builder::BoardVariable {
        name: "values".into(),
        label: String::new(),
        endpoint: asked.endpoint,
        entity_type: asked.entity_type,
        attribute: asked.attribute,
        include_all: false,
    };
    let probe = Board {
        title: "values".into(),
        description: String::new(),
        from: "now-6h".into(),
        to: "now".into(),
        refresh: String::new(),
        variables: vec![variable.clone()],
        panels: Vec::new(),
    };
    check_shape(&probe).map_err(shape)?;
    check_choices(&state, &effective, &project, &probe)?;
    let client = client(&state)?;
    let org = organization_held(&client, &project).await?;
    let values = read_values(&state, &client, org, &headers, &project, &variable).await?;
    Ok(Json(VariableValues {
        options: values
            .into_iter()
            .map(|value| VariableOption {
                text: value.clone(),
                value,
            })
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/grafana/builder/preview",
    summary = "Preview A Builder Panel",
    description = "Runs one panel of an unsaved dashboard as the caller: the Portal compiles the panel's query from its choices, checks it as the dashboard page checks a saved panel, and answers the panel and its frames. The body takes choices only; a query member is refused. Needs propose on Dashboard. Changes no data.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name")),
    request_body = PreviewAsked,
    responses(
        (status = 200, description = "The panel and its frames, or why it has none", body = PanelPreview),
        (status = 400, description = "A body that does not parse", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not build dashboards, or may not read an Endpoint a choice names", body = ProblemDetails),
        (status = 404, description = "No such project, or no such panel on the board", body = ProblemDetails),
        (status = 409, description = "No dashboards service for the project", body = ProblemDetails),
        (status = 422, description = "A field the builder does not take; the errors name it", body = ProblemDetails),
    )
)]
pub async fn builder_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    asked: Result<Json<PreviewAsked>, JsonRejection>,
) -> Result<Json<PanelPreview>, ApiError> {
    let effective = proposer(&state, &user.0.identity, &project)?;
    let asked = body(asked)?;
    check_shape(&asked.board).map_err(shape)?;
    let filters = check_choices(&state, &effective, &project, &asked.board)?;
    let index = asked
        .board
        .panels
        .iter()
        .position(|panel| panel.id == asked.panel)
        .ok_or_else(|| ApiError::NotFound(format!("panel {} is not on the board", asked.panel)))?;
    let client = client(&state)?;
    let org = organization_held(&client, &project).await?;
    // Only the asked panel is compiled and run; its variables offer what the caller picked.
    let mut one = asked.board.clone();
    one.panels = vec![asked.board.panels[index].clone()];
    let sources = sources(
        &state,
        &client,
        org,
        &project,
        &one,
        vec![filters[index].clone()],
    )
    .await?;
    let picked: Vec<Vec<String>> = one
        .variables
        .iter()
        .map(|v| builder::offered(asked.vars.get(&v.name).cloned().unwrap_or_default()))
        .collect();
    let dashboard = compile("jcd-preview", &one, &sources, &picked);
    let range = (
        asked.from.unwrap_or_else(|| one.from.clone()),
        asked.to.unwrap_or_else(|| one.to.clone()),
    );
    let mut words = vec![
        ("from".to_owned(), range.0.clone()),
        ("to".to_owned(), range.1.clone()),
    ];
    for (name, values) in &asked.vars {
        words.extend(
            values
                .iter()
                .map(|value| (format!("var-{name}"), value.clone())),
        );
    }
    checked_query(&dashboard, &words)?;
    let panel = dashboard["panels"][0].clone();
    let view = panel_view(&panel);
    if one.panels[0].kind.drawn_as().is_none() {
        // A text panel says what it holds and reads nothing.
        return Ok(Json(PanelPreview {
            panel: view,
            data: PanelData::default(),
        }));
    }
    let token = match person(&state, &headers, &project, &one.panels[0].endpoint).await {
        Ok(token) => token,
        Err(error) => {
            return Ok(Json(PanelPreview {
                panel: view,
                data: PanelData {
                    error: Some(error),
                    ..PanelData::default()
                },
            }))
        }
    };
    let answer = client
        .query(
            org,
            &[panel["targets"][0].clone()],
            (&range.0, &range.1),
            &builder::regex_values(&asked.vars),
            token.as_deref(),
        )
        .await?;
    let data = match answer["error"].as_str() {
        Some(error) => PanelData {
            error: Some(error.to_owned()),
            ..PanelData::default()
        },
        None => PanelData {
            frames: frames_of(&answer),
            ..PanelData::default()
        },
    };
    Ok(Json(PanelPreview { panel: view, data }))
}

/// The dashboard file of `board` at `uid`, compiled from checked choices with each variable's
/// values read as the caller.
async fn file_of(
    state: &AppState,
    effective: &Effective,
    headers: &HeaderMap,
    project: &str,
    uid: &str,
    board: &Board,
) -> Result<String, ApiError> {
    check_shape(board).map_err(shape)?;
    let filters = check_choices(state, effective, project, board)?;
    let client = client(state)?;
    let org = organization_held(&client, project).await?;
    let sources = sources(state, &client, org, project, board, filters).await?;
    let mut values = Vec::new();
    for variable in &board.variables {
        values.push(read_values(state, &client, org, headers, project, variable).await?);
    }
    let text = serde_json::to_string_pretty(&compile(uid, board, &sources, &values))
        .map_err(|err| ApiError::Internal(err.to_string()))?
        + "\n";
    if text.len() > builder::MAX_FILE {
        return Err(refused(
            "board",
            format!(
                "the dashboard is over {} KiB; take panels out",
                builder::MAX_FILE / 1024
            ),
        ));
    }
    Ok(text)
}

/// The Change of one dashboard file: written (or removed with `content` `None`) on its own
/// branch, refused while another Change of it is open.
#[allow(clippy::too_many_arguments)]
async fn propose(
    state: &AppState,
    identity: &Identity,
    project: &str,
    uid: &str,
    title: &str,
    content: Option<&str>,
    existing: Option<&crate::forge::RepoFile>,
) -> Result<Change, ApiError> {
    let shako = forge(state, project)?;
    let base = shako.default_branch().await?;
    let branch = branch_of(project, uid);
    let held = crate::api::mutate::turn(&shako, &branch).await;
    if let Some(open) = crate::api::mutate::open_change_on(state, &shako, &branch, project).await? {
        return Err(ApiError::Conflict(format!(
            "a change of this dashboard is open already: {}; approve or reject it first",
            open.name
        )));
    }
    let branch = crate::api::mutate::create_or_reuse_branch(&shako, &held, &base).await?;
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, project);
    let author = crate::forge::Author {
        name: &author_name,
        email: &author_email,
    };
    let path = builder::path_of(project, uid);
    let (verb, summary) = match (content, existing) {
        (Some(_), None) => ("add", crate::change::PlanSummary::new(1, 0, 0)),
        (Some(_), Some(_)) => ("change", crate::change::PlanSummary::new(0, 1, 0)),
        (None, _) => ("remove", crate::change::PlanSummary::new(0, 0, 1)),
    };
    let message = format!("{verb} dashboard {title}");
    match (content, existing) {
        (Some(content), existing) => {
            shako
                .put_file(&crate::forge::FileWrite {
                    path: &path,
                    branch: &branch,
                    message: &message,
                    content,
                    sha: existing.map(|f| f.sha.as_str()),
                    author,
                })
                .await?;
        }
        (None, Some(existing)) => {
            shako
                .delete_file(&crate::forge::FileDelete {
                    path: &path,
                    branch: &branch,
                    message: &message,
                    sha: &existing.sha,
                    author,
                })
                .await?;
        }
        (None, None) => return Err(no_dashboard(project, uid)),
    }
    let pr_body = format!(
        "{verb}s the dashboard `{title}` in `{path}`, built in the Portal from choices. Once this \
         merges, the reconciler provisions it into the project's dashboards."
    );
    let pull = shako
        .create_pull_request(&branch, &base, &message, &pr_body)
        .await?;
    tracing::info!(%project, %uid, pull = pull.number, "dashboard proposed");
    Ok(crate::api::import::waiting_change(
        state,
        &shako,
        project,
        crate::change::Lane::Green,
        summary,
        pull,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/grafana/builder",
    summary = "Propose A New Dashboard",
    description = "Compiles a dashboard from choices into a Grafana dashboard file and proposes it as a Change that adds `projects/{project}/grafana/{uid}.json`. The uid is minted here. Needs propose on Dashboard and read on every Endpoint a choice names.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name")),
    request_body = NewBoard,
    responses(
        (status = 202, description = "Proposed; waits for approval", body = BuilderSaved),
        (status = 400, description = "A body that does not parse", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not build dashboards, or may not read an Endpoint a choice names", body = ProblemDetails),
        (status = 404, description = "No such project the caller has a part in", body = ProblemDetails),
        (status = 409, description = "No dashboards service for the project", body = ProblemDetails),
        (status = 422, description = "A field the builder does not take; the errors name it", body = ProblemDetails),
        (status = 503, description = "The forge or the dashboards service did not answer", body = ProblemDetails),
    )
)]
pub async fn builder_create(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    sent: Result<Json<NewBoard>, JsonRejection>,
) -> Result<(StatusCode, Json<BuilderSaved>), ApiError> {
    let identity = &user.0.identity;
    let effective = proposer(&state, identity, &project)?;
    let NewBoard { board } = body(sent)?;
    let uid = builder::mint_uid();
    let text = file_of(&state, &effective, &headers, &project, &uid, &board).await?;
    let change = propose(
        &state,
        identity,
        &project,
        &uid,
        board.title.trim(),
        Some(&text),
        None,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(BuilderSaved { uid, change })))
}

/// The file of a builder dashboard on the default branch; 404 for a uid that is not one, or one
/// another project holds.
async fn saved_file(
    state: &AppState,
    project: &str,
    uid: &str,
) -> Result<Option<crate::forge::RepoFile>, ApiError> {
    if !builder::is_builder_uid(uid) {
        return Err(no_dashboard(project, uid));
    }
    let shako = forge(state, project)?;
    let base = shako.default_branch().await?;
    Ok(shako
        .get_file(&builder::path_of(project, uid), &base)
        .await?)
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/grafana/builder/{uid}",
    summary = "Propose A Dashboard Edit",
    description = "Compiles the edited dashboard from choices and proposes it as a Change of `projects/{project}/grafana/{uid}.json`. `base` is the revision the edit started from; a file changed since is a conflict. Needs propose on Dashboard and read on every Endpoint a choice names.",
    tag = "grafana",
    params(
        ("project" = String, Path, description = "Project name"),
        ("uid" = String, Path, description = "Dashboard uid"),
    ),
    request_body = EditedBoard,
    responses(
        (status = 202, description = "Proposed; waits for approval", body = BuilderSaved),
        (status = 400, description = "A body that does not parse", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not build dashboards, or may not read an Endpoint a choice names", body = ProblemDetails),
        (status = 404, description = "No such project, or no builder dashboard of that uid in it", body = ProblemDetails),
        (status = 409, description = "The file changed since the edit started, or a Change of it is open", body = ProblemDetails),
        (status = 422, description = "A field the builder does not take; the errors name it", body = ProblemDetails),
        (status = 503, description = "The forge or the dashboards service did not answer", body = ProblemDetails),
    )
)]
pub async fn builder_update(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, uid)): Path<(String, String)>,
    headers: HeaderMap,
    sent: Result<Json<EditedBoard>, JsonRejection>,
) -> Result<(StatusCode, Json<BuilderSaved>), ApiError> {
    let identity = &user.0.identity;
    let effective = proposer(&state, identity, &project)?;
    let existing = saved_file(&state, &project, &uid)
        .await?
        .ok_or_else(|| no_dashboard(&project, &uid))?;
    let EditedBoard { board, base } = body(sent)?;
    if base != existing.sha {
        return Err(ApiError::Conflict(
            "the dashboard changed since you opened it; open it again to edit the saved version"
                .into(),
        ));
    }
    let text = file_of(&state, &effective, &headers, &project, &uid, &board).await?;
    let change = propose(
        &state,
        identity,
        &project,
        &uid,
        board.title.trim(),
        Some(&text),
        Some(&existing),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(BuilderSaved { uid, change })))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/grafana/builder/{uid}",
    summary = "Propose Removing A Dashboard",
    description = "Proposes a Change that removes `projects/{project}/grafana/{uid}.json`; once it merges the reconciler takes the dashboard out of the dashboards service. Needs propose on Dashboard.",
    tag = "grafana",
    params(
        ("project" = String, Path, description = "Project name"),
        ("uid" = String, Path, description = "Dashboard uid"),
    ),
    responses(
        (status = 202, description = "Proposed; waits for approval", body = BuilderSaved),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not build dashboards", body = ProblemDetails),
        (status = 404, description = "No such project, or no builder dashboard of that uid in it", body = ProblemDetails),
        (status = 409, description = "A Change of the dashboard is open", body = ProblemDetails),
        (status = 503, description = "The forge did not answer", body = ProblemDetails),
    )
)]
pub async fn builder_delete(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, uid)): Path<(String, String)>,
) -> Result<(StatusCode, Json<BuilderSaved>), ApiError> {
    let identity = &user.0.identity;
    proposer(&state, identity, &project)?;
    let existing = saved_file(&state, &project, &uid)
        .await?
        .ok_or_else(|| no_dashboard(&project, &uid))?;
    let title = serde_json::from_str::<Value>(&existing.content)
        .ok()
        .and_then(|file| file["title"].as_str().map(str::to_owned))
        .unwrap_or_else(|| uid.clone());
    let change = propose(
        &state,
        identity,
        &project,
        &uid,
        &title,
        None,
        Some(&existing),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(BuilderSaved { uid, change })))
}

/// The Endpoint each datasource uid of the project's organization reads, by name; a datasource
/// the service does not list falls back to an Endpoint of that name, as the visualization page
/// names its datasources.
async fn endpoints_by_datasource(state: &AppState, project: &str) -> BTreeMap<String, String> {
    let endpoints = state
        .mirror
        .list(project, "Endpoint", &crate::store::ListOptions::default())
        .items;
    let mut out: BTreeMap<String, String> = endpoints
        .iter()
        .map(|e| (e.metadata.name.clone(), e.metadata.name.clone()))
        .collect();
    if let Ok(client) = client(state) {
        if let Ok(org) = organization_held(&client, project).await {
            if let Ok(held) = client.all_datasources(org).await {
                for source in held {
                    let Some(slug) = source.slug else { continue };
                    if let Some(endpoint) = endpoints.iter().find(|e| slug_of(e) == slug) {
                        out.insert(source.uid, endpoint.metadata.name.clone());
                    }
                }
            }
        }
    }
    out
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/builder/{uid}",
    summary = "Read A Builder Dashboard",
    description = "A builder dashboard's file read back into choices for its next edit, with the revision the edit starts from and the Change of it that waits for approval, if any. Needs propose on Dashboard. Changes no data.",
    tag = "grafana",
    params(
        ("project" = String, Path, description = "Project name"),
        ("uid" = String, Path, description = "Dashboard uid"),
    ),
    responses(
        (status = 200, description = "The dashboard as choices", body = BuilderDashboard),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not build dashboards", body = ProblemDetails),
        (status = 404, description = "No such project, or no builder dashboard of that uid in it", body = ProblemDetails),
        (status = 422, description = "The file was not written by the builder", body = ProblemDetails),
    )
)]
pub async fn builder_get(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, uid)): Path<(String, String)>,
) -> Result<Json<BuilderDashboard>, ApiError> {
    proposer(&state, &user.0.identity, &project)?;
    let saved = saved_file(&state, &project, &uid).await?;
    let shako = forge(&state, &project)?;
    let waiting = pending(&state, &shako, &project)
        .await?
        .into_iter()
        .find(|p| p.uid == uid);
    let (text, base) = match (&saved, &waiting) {
        (Some(file), _) => (file.content.clone(), Some(file.sha.clone())),
        (None, Some(waiting)) => {
            // Only the Change holds it: read it from its branch.
            let file = shako
                .get_file(&builder::path_of(&project, &uid), &waiting.branch)
                .await?
                .ok_or_else(|| no_dashboard(&project, &uid))?;
            (file.content, None)
        }
        (None, None) => return Err(no_dashboard(&project, &uid)),
    };
    let file = builder::read_file(&uid, &text).map_err(|why| refused("file", why))?;
    let names = endpoints_by_datasource(&state, &project).await;
    let board =
        decompile(&file, &|uid| names.get(uid).cloned()).map_err(|why| refused("file", why))?;
    Ok(Json(BuilderDashboard {
        uid,
        board,
        base,
        pending: waiting,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/grafana/builder",
            get(builder_start).post(builder_create),
        )
        .route(
            "/projects/{project}/grafana/builder/values",
            get(builder_values),
        )
        .route(
            "/projects/{project}/grafana/builder/preview",
            post(builder_preview),
        )
        .route(
            "/projects/{project}/grafana/builder/{uid}",
            get(builder_get).put(builder_update).delete(builder_delete),
        )
}
