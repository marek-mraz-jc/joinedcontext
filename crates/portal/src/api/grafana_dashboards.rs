//! The dashboards of a project drawn in the Portal (item 15): the list, one dashboard's layout,
//! panels and variables, and each panel's frames. The browser never speaks to Grafana or daiya:
//! the Portal reads them as the dashboards service's administrator after its own checks. A panel
//! runs only its saved queries (daiya's `GET /api/dashboards/{uid}/render`), never a query the
//! browser wrote, and a panel whose datasource reads an Endpoint the caller may not read is
//! answered with that reason and no query.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::{CurrentUser, Identity};
use crate::error::{ApiError, ProblemDetails};
use crate::grafana::{Client, HeldDatasource};
use crate::permissions::{Effective, ORG_NAMESPACE};
use crate::state::AppState;
use crate::store::ListOptions;

/// The organization the installation's own dashboards live in (Grafana's and daiya's first).
const INSTALLATION_ORG: i64 = 1;

/// The longest variable value a person may pick or type.
const MAX_VALUE: usize = 256;

/// The most values a multi-value variable takes at once.
const MAX_VALUES: usize = 50;

/// The dashboards of a project.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardList {
    /// The dashboards service answers and holds this project's organization.
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub items: Vec<DashboardSummary>,
}

/// One dashboard in the list.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub uid: String,
    pub title: String,
    pub tags: Vec<String>,
}

/// One dashboard: what the page lays out. Panel queries stay on the server.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardView {
    pub uid: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The saved time range, Grafana's words (`now-6h`, `now`).
    pub from: String,
    pub to: String,
    /// The saved refresh interval (`30s`, `5m`), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// Whether the dashboard hides its time and refresh pickers (`timepicker.hidden`).
    pub time_picker_hidden: bool,
    /// The saved time zone (`browser`, `utc` or an IANA name), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    pub variables: Vec<VariableView>,
    /// Each panel's `id`, `type`, `title`, `description`, `gridPos`, `fieldConfig`, `options`,
    /// `maxDataPoints`, its own range (`timeFrom`, `timeShift`, `hideTimeOverride`), and for a
    /// row `collapsed` and its `panels`.
    #[schema(value_type = Vec<Object>)]
    pub panels: Vec<Value>,
}

/// One template variable.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VariableView {
    pub name: String,
    pub label: String,
    /// `custom`, `query`, `textbox`, `constant` or `interval`.
    pub kind: String,
    pub multi: bool,
    pub include_all: bool,
    /// The values a `custom` or `interval` variable offers; a `query` variable's come from its
    /// own route.
    pub options: Vec<VariableOption>,
    /// The saved selection.
    pub current: Vec<String>,
}

/// One value a variable offers.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct VariableOption {
    pub text: String,
    pub value: String,
}

/// The values a variable offers now.
#[derive(Debug, Serialize, ToSchema)]
pub struct VariableValues {
    pub options: Vec<VariableOption>,
}

/// One panel's data.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanelData {
    /// daiya's frames, each with its `refId`: `{name, refId, fields: [{name, type, unit,
    /// displayName, labels, values}], meta}`.
    #[schema(value_type = Vec<Object>)]
    pub frames: Vec<Value>,
    /// Why the caller sees no data: a source they may not read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_denied: Option<String>,
    /// Why the panel has no data: its datasource refused or did not answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn member(state: &AppState, identity: &Identity, project: &str) -> Result<Effective, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !crate::resource::is_dns1123(project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    Ok(effective)
}

fn client(state: &AppState) -> Result<Client<'_>, ApiError> {
    state
        .config
        .grafana
        .as_ref()
        .map(Client::new)
        .ok_or_else(|| {
            ApiError::Conflict(
                "dashboards are not set up on this installation; ask your operator to turn the \
             grafana component on"
                    .into(),
            )
        })
}

/// The organization of `project`: the installation's for the organization's own namespace;
/// made on first read when the service is daiya (`provision.sh`'s steps, which Grafana's
/// deployment runs itself).
async fn organization(client: &Client<'_>, project: &str) -> Result<Option<i64>, ApiError> {
    if project == ORG_NAMESPACE {
        return Ok(Some(INSTALLATION_ORG));
    }
    if let Some(org) = client.organization(project).await? {
        return Ok(Some(org));
    }
    if client.derives_organizations().await {
        return client.provision(project).await.map(Some);
    }
    Ok(None)
}

async fn organization_held(client: &Client<'_>, project: &str) -> Result<i64, ApiError> {
    organization(client, project).await?.ok_or_else(|| {
        ApiError::Conflict(format!(
            "the dashboards service has no organization for {project} yet; ask your operator to \
             add this project to it"
        ))
    })
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A panel as the page draws it: its queries and datasource stay here.
fn panel_view(panel: &Value) -> Value {
    let mut kept = serde_json::Map::new();
    for key in [
        "id",
        "type",
        "title",
        "description",
        "gridPos",
        "fieldConfig",
        "options",
        "collapsed",
        "maxDataPoints",
        "timeFrom",
        "timeShift",
        "hideTimeOverride",
    ] {
        if let Some(value) = panel.get(key) {
            kept.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(children) = panel.get("panels").and_then(Value::as_array) {
        kept.insert(
            "panels".into(),
            Value::Array(children.iter().map(panel_view).collect()),
        );
    }
    Value::Object(kept)
}

fn current_of(variable: &Value) -> Vec<String> {
    match &variable["current"]["value"] {
        Value::Array(values) => values.iter().map(text).collect(),
        Value::Null => Vec::new(),
        value => vec![text(value)],
    }
}

/// The values a `custom` variable offers: its `options`, else its comma-separated `query`.
fn custom_options(variable: &Value) -> Vec<VariableOption> {
    let listed: Vec<VariableOption> = variable["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let value = text(&option["value"]);
            (!value.is_empty()).then(|| VariableOption {
                text: option["text"]
                    .as_str()
                    .map_or_else(|| value.clone(), str::to_owned),
                value,
            })
        })
        .collect();
    if !listed.is_empty() {
        return listed;
    }
    variable["query"]
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| match value.split_once(" : ") {
            Some((text, value)) => VariableOption {
                text: text.trim().to_owned(),
                value: value.trim().to_owned(),
            },
            None => VariableOption {
                text: value.to_owned(),
                value: value.to_owned(),
            },
        })
        .collect()
}

fn variables_of(dashboard: &Value) -> Vec<&Value> {
    dashboard["templating"]["list"]
        .as_array()
        .map(|list| list.iter().collect())
        .unwrap_or_default()
}

fn variable_view(variable: &Value) -> VariableView {
    let kind = variable["type"].as_str().unwrap_or("custom").to_owned();
    let options = match kind.as_str() {
        "custom" | "interval" => custom_options(variable),
        _ => Vec::new(),
    };
    let name = text(&variable["name"]);
    let include_all = variable["includeAll"] == true;
    // Nothing saved: Grafana selects All where the variable offers it, else the first value
    // (a query variable's first value comes from its own route; until then it sends nothing).
    let mut current = current_of(variable);
    if current.iter().all(String::is_empty) {
        current = if include_all {
            vec!["$__all".to_owned()]
        } else {
            options.first().map(|o| o.value.clone()).into_iter().collect()
        };
    }
    VariableView {
        label: variable["label"]
            .as_str()
            .filter(|label| !label.is_empty())
            .map_or_else(|| name.clone(), str::to_owned),
        name,
        kind,
        multi: variable["multi"] == true,
        include_all,
        options,
        current,
    }
}

async fn dashboard_of(
    client: &Client<'_>,
    org: i64,
    project: &str,
    uid: &str,
) -> Result<Value, ApiError> {
    let not_found = || ApiError::NotFound(format!("dashboard '{uid}' not found in {project}"));
    if uid.is_empty()
        || uid.len() > 64
        || !uid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(not_found());
    }
    client.dashboard(org, uid).await?.ok_or_else(not_found)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/dashboards",
    summary = "List Project Dashboards",
    description = "The dashboards of the project's organization in the dashboards service, to draw in the Portal. Changes no data.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The dashboards", body = DashboardList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project the caller has a part in", body = ProblemDetails),
    )
)]
pub async fn list_dashboards(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<DashboardList>, ApiError> {
    member(&state, &user.0.identity, &project)?;
    let client = match client(&state) {
        Ok(client) => client,
        Err(ApiError::Conflict(reason)) => {
            return Ok(Json(DashboardList {
                ready: false,
                reason: Some(reason),
                items: Vec::new(),
            }))
        }
        Err(err) => return Err(err),
    };
    let org = match organization_held(&client, &project).await {
        Ok(org) => org,
        Err(ApiError::Conflict(reason) | ApiError::Unavailable(reason)) => {
            return Ok(Json(DashboardList {
                ready: false,
                reason: Some(reason),
                items: Vec::new(),
            }))
        }
        Err(err) => return Err(err),
    };
    let mut items: Vec<DashboardSummary> = client
        .dashboards(org)
        .await?
        .iter()
        .filter_map(|hit| {
            Some(DashboardSummary {
                uid: hit["uid"].as_str()?.to_owned(),
                title: text(&hit["title"]),
                tags: hit["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(text)
                    .collect(),
            })
        })
        .collect();
    items.sort_by_key(|a| a.title.to_lowercase());
    Ok(Json(DashboardList {
        ready: true,
        reason: None,
        items,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/dashboards/{uid}",
    summary = "Get Project Dashboard",
    description = "One dashboard's layout, panels and variables, without its queries. Changes no data.",
    tag = "grafana",
    params(
        ("project" = String, Path, description = "Project name"),
        ("uid" = String, Path, description = "Dashboard uid"),
    ),
    responses(
        (status = 200, description = "The dashboard", body = DashboardView),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project or dashboard", body = ProblemDetails),
        (status = 409, description = "No dashboards service or organization", body = ProblemDetails),
    )
)]
pub async fn get_dashboard(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, uid)): Path<(String, String)>,
) -> Result<Json<DashboardView>, ApiError> {
    member(&state, &user.0.identity, &project)?;
    let client = client(&state)?;
    let org = organization_held(&client, &project).await?;
    let dashboard = dashboard_of(&client, org, &project, &uid).await?;
    Ok(Json(DashboardView {
        uid,
        title: text(&dashboard["title"]),
        description: dashboard["description"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(str::to_owned),
        from: dashboard["time"]["from"]
            .as_str()
            .unwrap_or("now-6h")
            .to_owned(),
        to: dashboard["time"]["to"].as_str().unwrap_or("now").to_owned(),
        refresh: dashboard["refresh"]
            .as_str()
            .filter(|r| !r.is_empty())
            .map(str::to_owned),
        time_picker_hidden: dashboard["timepicker"]["hidden"] == true,
        timezone: dashboard["timezone"]
            .as_str()
            .filter(|z| !z.is_empty())
            .map(str::to_owned),
        variables: variables_of(&dashboard)
            .into_iter()
            .map(variable_view)
            .collect(),
        panels: dashboard["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(panel_view)
            .collect(),
    }))
}

/// Grafana's time words: `now`, `now-6h`, `now-1d/d`, epoch milliseconds or RFC 3339.
fn is_time(text: &str) -> bool {
    if text.is_empty() || text.len() > 40 {
        return false;
    }
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    if chrono::DateTime::parse_from_rfc3339(text).is_ok() {
        return true;
    }
    let Some(rest) = text.strip_prefix("now") else {
        return false;
    };
    let (offset, round) = match rest.split_once('/') {
        Some((offset, round)) => (offset, Some(round)),
        None => (rest, None),
    };
    let unit = |u: &str| matches!(u, "s" | "m" | "h" | "d" | "w" | "M" | "y");
    let offset_ok = offset.is_empty()
        || offset
            .strip_prefix('-')
            .or_else(|| offset.strip_prefix('+'))
            .is_some_and(|o| {
                let digits = o.trim_end_matches(|c: char| c.is_ascii_alphabetic());
                !digits.is_empty()
                    && digits.bytes().all(|b| b.is_ascii_digit())
                    && unit(&o[digits.len()..])
            });
    offset_ok && round.is_none_or(unit)
}

fn refused(field: &str, detail: String) -> ApiError {
    ApiError::Unprocessable {
        detail: detail.clone(),
        errors: vec![format!("{field}: {detail}")],
    }
}

/// The saved panel `id`, rows' children included.
fn find_panel(panels: &[Value], id: u32) -> Option<&Value> {
    panels.iter().find_map(|panel| {
        if panel["id"].as_u64() == Some(u64::from(id)) {
            return Some(panel);
        }
        find_panel(
            panel["panels"].as_array().map_or(&[][..], Vec::as_slice),
            id,
        )
    })
}

fn datasource_uid(reference: &Value) -> Option<String> {
    match reference {
        Value::String(uid) => Some(uid.clone()),
        Value::Object(_) => reference["uid"].as_str().map(str::to_owned),
        _ => None,
    }
}

/// The datasources a panel's queries read, the panel's own and each target's.
fn panel_datasources(panel: &Value) -> Vec<String> {
    let mut uids: Vec<String> = datasource_uid(&panel["datasource"]).into_iter().collect();
    for target in panel["targets"].as_array().into_iter().flatten() {
        uids.extend(datasource_uid(&target["datasource"]));
    }
    uids.retain(|uid| {
        !uid.is_empty() && uid != "-- Mixed --" && uid != "__expr__" && uid != "-- Dashboard --"
    });
    uids.sort();
    uids.dedup();
    uids
}

/// Why the caller may not see what `panel` reads; `None` when every source is theirs to read.
fn denial(
    state: &AppState,
    effective: &Effective,
    project: &str,
    panel: &Value,
    held: &[HeldDatasource],
) -> Option<String> {
    let refuse = |uid: &str| Some(format!("You cannot read the source of this panel: {uid}"));
    for uid in panel_datasources(panel) {
        // A datasource named by a variable could be any of them: refused, not guessed.
        if uid.starts_with('$') {
            return refuse(&uid);
        }
        let Some(slug) = held
            .iter()
            .find(|source| source.uid == uid)
            .and_then(|source| source.slug.as_deref())
        else {
            continue;
        };
        let readable = state
            .mirror
            .list(project, "Endpoint", &ListOptions::default())
            .items
            .into_iter()
            .find(|endpoint| endpoint.spec["slug"] == slug)
            .and_then(|endpoint| serde_json::to_value(&endpoint).ok())
            .is_some_and(|endpoint| effective.may_read_manifest("Endpoint", &endpoint));
        if !readable {
            return refuse(&uid);
        }
    }
    None
}

/// The slugs of the members-only Endpoints `panel` reads: those a person's own token must reach.
/// A public Endpoint is read with no token.
fn members_only(
    state: &AppState,
    project: &str,
    panel: &Value,
    held: &[HeldDatasource],
) -> Vec<String> {
    let endpoints = state
        .mirror
        .list(project, "Endpoint", &ListOptions::default())
        .items;
    let mut slugs: Vec<String> = panel_datasources(panel)
        .iter()
        .filter_map(|uid| held.iter().find(|source| source.uid == *uid))
        .filter_map(|source| source.slug.clone())
        .filter(|slug| {
            !endpoints
                .iter()
                .any(|e| e.spec["slug"] == *slug && e.spec["audience"].as_str() == Some("public"))
        })
        .collect();
    slugs.sort();
    slugs.dedup();
    slugs
}

/// The person's token exchanged for the one members-only Endpoint the panel reads, as a KPI
/// history read does (`kpi_check.rs`); `Ok(None)` for a panel reading only public or no
/// Endpoints. `Err` is the panel's error line, which carries no token.
async fn person_for(
    state: &AppState,
    headers: &HeaderMap,
    slugs: &[String],
) -> Result<Option<String>, String> {
    let slug = match slugs {
        [] => return Ok(None),
        [slug] => slug,
        _ => {
            return Err(format!(
                "this panel reads {} members-only Endpoints at once; each panel reads one with your token",
                slugs.len()
            ))
        }
    };
    let token = crate::agents::identity::persons_token(headers, state.config.trust_edge_token)
        .ok_or(
            "this panel is read with your own access token, which this session does not carry; \
             sign in through the platform's address",
        )?;
    let oidc = state
        .oidc
        .as_ref()
        .ok_or("this Portal has no Keycloak client to read the panel with")?;
    oidc.exchange_for_endpoint(token, slug)
        .await
        .map(Some)
        .map_err(|err| err.to_string())
}

/// The `from`, `to`, `maxDataPoints` and `var-<name>` pairs asked, each checked against the
/// saved dashboard; what daiya gets.
fn checked_query(
    dashboard: &Value,
    asked: &[(String, String)],
) -> Result<Vec<(String, String)>, ApiError> {
    let mut out = Vec::new();
    let mut values: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (key, value) in asked {
        match key.as_str() {
            "from" | "to" => {
                if !is_time(value) {
                    return Err(refused(key, format!("'{value}' is not a time such as now-6h, now or an epoch in milliseconds")));
                }
                out.push((key.clone(), value.clone()));
            }
            "maxDataPoints" => match value.parse::<u32>() {
                Ok(points) if (1..=5000).contains(&points) => {
                    out.push((key.clone(), points.to_string()))
                }
                _ => return Err(refused(key, "a number of points from 1 to 5000".into())),
            },
            other => {
                let Some(name) = other.strip_prefix("var-") else {
                    return Err(refused(other, "not a question this route takes".into()));
                };
                values.entry(name).or_default().push(value.as_str());
            }
        }
    }
    let variables = variables_of(dashboard);
    for (name, picked) in values {
        let field = format!("var-{name}");
        let Some(variable) = variables.iter().find(|v| v["name"] == name) else {
            return Err(refused(
                &field,
                format!("the dashboard has no variable {name}"),
            ));
        };
        let kind = variable["type"].as_str().unwrap_or("custom");
        if kind == "constant" || kind == "datasource" {
            return Err(refused(
                &field,
                format!("{name} is not a variable a person picks"),
            ));
        }
        if picked.len() > MAX_VALUES || (picked.len() > 1 && variable["multi"] != true) {
            return Err(refused(&field, format!("{name} takes one value")));
        }
        if let Some(long) = picked
            .iter()
            .find(|v| v.len() > MAX_VALUE || v.contains(['\n', '\r']))
        {
            return Err(refused(
                &field,
                format!(
                    "'{}…' is not a value of {name}",
                    long.chars().take(20).collect::<String>()
                ),
            ));
        }
        let all = picked.contains(&"$__all");
        if all && variable["includeAll"] != true {
            return Err(refused(&field, format!("{name} offers no All")));
        }
        if kind == "custom" || kind == "interval" {
            let offered = custom_options(variable);
            if let Some(stray) = picked
                .iter()
                .find(|v| **v != "$__all" && !offered.iter().any(|o| o.value == **v))
            {
                return Err(refused(
                    &field,
                    format!("'{stray}' is not one of the values {name} offers"),
                ));
            }
        }
        // daiya formats the values as Grafana's datasource does (`(a|b)` regex-escaped for
        // Prometheus, the variable's allValue for All), from the saved variable.
        out.extend(
            picked
                .iter()
                .map(|value| (field.clone(), (*value).to_owned())),
        );
    }
    Ok(out)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/dashboards/{uid}/panels/{panel}",
    summary = "Get Dashboard Panel Data",
    description = "Runs one panel's saved queries and transformations for the time range and variables asked (`from`, `to`, `maxDataPoints`, `var-<name>`) and answers its frames. Changes no data.",
    tag = "grafana",
    params(
        ("project" = String, Path, description = "Project name"),
        ("uid" = String, Path, description = "Dashboard uid"),
        ("panel" = u32, Path, description = "Panel id"),
    ),
    responses(
        (status = 200, description = "The panel's frames, or why it has none", body = PanelData),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project, dashboard or panel", body = ProblemDetails),
        (status = 422, description = "A time or variable the dashboard does not take", body = ProblemDetails),
    )
)]
pub async fn panel_data(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, uid, panel)): Path<(String, String, u32)>,
    headers: HeaderMap,
    Query(asked): Query<Vec<(String, String)>>,
) -> Result<Json<PanelData>, ApiError> {
    let effective = member(&state, &user.0.identity, &project)?;
    let client = client(&state)?;
    let org = organization_held(&client, &project).await?;
    let dashboard = dashboard_of(&client, org, &project, &uid).await?;
    let saved = find_panel(
        dashboard["panels"]
            .as_array()
            .map_or(&[][..], Vec::as_slice),
        panel,
    )
    .ok_or_else(|| ApiError::NotFound(format!("panel {panel} not found on '{uid}'")))?;
    let query = checked_query(&dashboard, &asked)?;
    let held = client.all_datasources(org).await?;
    if let Some(reason) = denial(&state, &effective, &project, saved, &held) {
        return Ok(Json(PanelData {
            access_denied: Some(reason),
            ..PanelData::default()
        }));
    }
    let person = match person_for(
        &state,
        &headers,
        &members_only(&state, &project, saved, &held),
    )
    .await
    {
        Ok(person) => person,
        Err(error) => {
            return Ok(Json(PanelData {
                error: Some(error),
                ..PanelData::default()
            }))
        }
    };
    let answer = client
        .panel(org, &uid, panel, &query, person.as_deref())
        .await?;
    if let Some(error) = answer["error"].as_str() {
        return Ok(Json(PanelData {
            error: Some(error.to_owned()),
            ..PanelData::default()
        }));
    }
    let frames = answer["frameSets"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|set| {
            let ref_id = set["refId"].clone();
            set["frames"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |mut frame| {
                    if frame.get("refId").is_none() {
                        frame["refId"] = ref_id.clone();
                    }
                    frame
                })
        })
        .collect();
    Ok(Json(PanelData {
        frames,
        access_denied: answer["accessDeniedMessage"].as_str().map(str::to_owned),
        error: None,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/dashboards/{uid}/variables/{name}",
    summary = "Get Dashboard Variable Values",
    description = "The values a variable offers now: a custom variable's own, a query variable's asked of its datasource with the saved query. Changes no data.",
    tag = "grafana",
    params(
        ("project" = String, Path, description = "Project name"),
        ("uid" = String, Path, description = "Dashboard uid"),
        ("name" = String, Path, description = "Variable name"),
    ),
    responses(
        (status = 200, description = "The values", body = VariableValues),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project, dashboard or variable", body = ProblemDetails),
    )
)]
pub async fn variable_values(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, uid, name)): Path<(String, String, String)>,
) -> Result<Json<VariableValues>, ApiError> {
    let effective = member(&state, &user.0.identity, &project)?;
    let client = client(&state)?;
    let org = organization_held(&client, &project).await?;
    let dashboard = dashboard_of(&client, org, &project, &uid).await?;
    let variable = variables_of(&dashboard)
        .into_iter()
        .find(|v| v["name"] == name.as_str())
        .ok_or_else(|| ApiError::NotFound(format!("variable '{name}' not found on '{uid}'")))?;
    if variable["type"] != "query" {
        return Ok(Json(VariableValues {
            options: custom_options(variable),
        }));
    }
    // The variable's datasource is read as a panel's would be.
    let probe = json!({ "datasource": variable["datasource"] });
    let held = client.all_datasources(org).await?;
    if denial(&state, &effective, &project, &probe, &held).is_some() {
        return Ok(Json(VariableValues {
            options: Vec::new(),
        }));
    }
    let Some(datasource) = datasource_uid(&variable["datasource"]) else {
        return Ok(Json(VariableValues {
            options: Vec::new(),
        }));
    };
    let query = match &variable["query"] {
        Value::String(query) => query.clone(),
        query => text(&query["query"]),
    };
    let mut values = client.variable_values(org, &datasource, &query).await?;
    values.sort();
    values.dedup();
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/grafana/dashboards",
            get(list_dashboards),
        )
        .route(
            "/projects/{project}/grafana/dashboards/{uid}",
            get(get_dashboard),
        )
        .route(
            "/projects/{project}/grafana/dashboards/{uid}/panels/{panel}",
            get(panel_data),
        )
        .route(
            "/projects/{project}/grafana/dashboards/{uid}/variables/{name}",
            get(variable_values),
        )
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::auth::session::Session;
    use crate::config::{Config, GrafanaConfig};
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    const PROJECT: &str = "helsinki";

    fn person(groups: &[&str]) -> CurrentUser {
        CurrentUser(Session {
            identity: Identity {
                client: None,
                subject: "sub-pia".into(),
                username: "pia".into(),
                email: Some("pia@hel.fi".into()),
                name: None,
                roles: Vec::new(),
                groups: groups.iter().map(|group| (*group).to_owned()).collect(),
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    fn admin() -> CurrentUser {
        person(&["portal-approver"])
    }

    fn outsider() -> CurrentUser {
        person(&[])
    }

    /// A closed and a public Endpoint of the project.
    fn world(grafana: Option<&MockServer>) -> AppState {
        let mut config = Config::for_tests();
        config.grafana = grafana.map(|server| GrafanaConfig {
            url: server.uri(),
            public_url: "https://dev.example/grafana".into(),
            user: "grafana_admin".into(),
            password: "unused".into(),
        });
        let state = AppState::new(config, None);
        for (name, slug, audience) in [
            ("closed", "closedslug", "private"),
            ("open", "openslug", "public"),
        ] {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: "Endpoint".to_owned(),
                metadata: ObjectMeta::new(name, PROJECT),
                spec: json!({ "contextSpaceRef": "air", "slug": slug, "audience": audience }),
                status: None,
            });
        }
        state
    }

    fn board() -> Value {
        json!({
            "uid": "live", "title": "Helsinki live", "time": { "from": "now-7d", "to": "now" },
            "refresh": "5m", "timezone": "browser", "timepicker": { "hidden": true },
            "templating": { "list": [
                { "name": "station", "type": "custom", "multi": true, "includeAll": true,
                  "query": "Kallio,Mannerheimintie", "current": { "value": ["Kallio"] } },
                { "name": "run", "label": "Run", "type": "query", "datasource": { "uid": "prom" },
                  "query": { "query": "label_values(k6_http_reqs_total, testid)" } },
                { "name": "testid", "type": "query", "multi": true, "includeAll": true, "current": {},
                  "datasource": { "uid": "prom" }, "query": "label_values(testid)" },
                { "name": "zone", "type": "custom", "query": "north,south", "current": {} }
            ]},
            "panels": [
                { "id": 1, "type": "timeseries", "title": "Air", "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
                  "timeFrom": "1h", "timeShift": "1d", "hideTimeOverride": true,
                  "datasource": { "uid": "open-ds" },
                  "targets": [{ "refId": "A", "datasource": { "uid": "open-ds" }, "entityType": "AirQualityObserved" }] },
                { "id": 2, "type": "row", "title": "Closed", "collapsed": true, "panels": [
                    { "id": 3, "type": "table", "title": "Secret", "datasource": { "uid": "closed-ds" },
                      "targets": [{ "refId": "A" }] }
                ]},
                { "id": 4, "type": "stat", "title": "Var", "datasource": "${ds}", "targets": [] }
            ]
        })
    }

    async fn service(health: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": health })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/orgs/name/{PROJECT}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 7 })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/dashboards/uid/live"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "dashboard": board() })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/dashboards/uid/gone"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(json!({ "message": "Dashboard not found" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/api/datasources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "uid": "open-ds", "type": "ngsild-grafana-datasource", "url": "https://gw/api/endpoint/openslug" },
                { "uid": "closed-ds", "type": "ngsild-grafana-datasource", "url": "https://gw/api/endpoint/closedslug" },
                { "uid": "prom", "type": "prometheus", "url": "http://prometheus:9090" }
            ])))
            .mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "uid": "live", "title": "Helsinki live", "tags": ["project"] },
                { "uid": "air", "title": "Air", "tags": [] }
            ])))
            .mount(&server)
            .await;
        server
    }

    fn asked(pairs: &[(&str, &str)]) -> Query<Vec<(String, String)>> {
        Query(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    async fn panel(
        user: CurrentUser,
        state: &AppState,
        id: u32,
        pairs: &[(&str, &str)],
    ) -> Result<PanelData, ApiError> {
        panel_with(user, state, id, pairs, HeaderMap::new()).await
    }

    async fn panel_with(
        user: CurrentUser,
        state: &AppState,
        id: u32,
        pairs: &[(&str, &str)],
        headers: HeaderMap,
    ) -> Result<PanelData, ApiError> {
        panel_data(
            user,
            State(state.clone()),
            Path((PROJECT.into(), "live".into(), id)),
            headers,
            asked(pairs),
        )
        .await
        .map(|Json(data)| data)
    }

    /// The person's own token on a bearer call.
    const RAW: &str = "raw-person-token-0451";
    /// What the realm hands back for the Endpoint.
    const EXCHANGED: &str = "exchanged-endpoint-token-9932";

    fn bearer() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {RAW}").parse().unwrap(),
        );
        headers
    }

    /// A realm that exchanges any token for [`EXCHANGED`], on `state`.
    async fn exchanging(state: &mut AppState) -> MockServer {
        let realm = crate::auth::oidc::tests::mock_realm().await;
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": EXCHANGED, "token_type": "Bearer", "expires_in": 300
            })))
            .mount(&realm)
            .await;
        let client = crate::auth::oidc::OidcClient::discover(
            &crate::auth::oidc::tests::config_for(&realm),
            "https://portal.test/cb",
        )
        .await
        .expect("discovery");
        state.oidc = Some(std::sync::Arc::new(client));
        realm
    }

    /// Everything the Portal logs while `run` runs, as text.
    async fn logged<F: std::future::Future>(run: F) -> (F::Output, String) {
        #[derive(Clone, Default)]
        struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        let out = run.await;
        drop(guard);
        let text = String::from_utf8_lossy(&sink.0.lock().unwrap()).into_owned();
        (out, text)
    }

    async fn render_answering(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/dashboards/live/render"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "frameSets": [{ "refId": "A", "frames": [{ "name": "pm25", "fields": [
                    { "name": "pm25", "type": "number", "values": [3.5] }
                ]}]}]
            })))
            .mount(server)
            .await;
    }

    fn person_headers(requests: &[wiremock::Request]) -> Vec<Option<String>> {
        requests
            .iter()
            .filter(|request| request.url.path() == "/api/dashboards/live/render")
            .map(|request| {
                request
                    .headers
                    .get(crate::grafana::PERSON_TOKEN_HEADER)
                    .map(|value| value.to_str().unwrap().to_owned())
            })
            .collect()
    }

    #[tokio::test]
    async fn a_members_only_panel_reaches_daiya_with_the_exchanged_token_only() {
        let server = service("grafana").await;
        render_answering(&server).await;
        let mut state = world(Some(&server));
        let realm = exchanging(&mut state).await;
        let (data, logs) = logged(panel_with(admin(), &state, 3, &[], bearer())).await;
        let data = data.expect("frames");
        assert!(
            data.error.is_none() && data.access_denied.is_none(),
            "{data:?}"
        );
        let requests = server.received_requests().await.expect("recorded");
        assert_eq!(person_headers(&requests), [Some(EXCHANGED.to_owned())]);
        let render = requests
            .iter()
            .find(|request| request.url.path() == "/api/dashboards/live/render")
            .unwrap();
        // The Portal's own credential to daiya stays Basic; neither the raw token nor the
        // Portal's Keycloak secret goes along.
        let sent = format!("{:?}", render.headers);
        assert!(
            !sent.contains(RAW) && !sent.contains("s3cr3t-do-not-leak"),
            "{sent}"
        );
        assert!(render.headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Basic "));
        // The realm got the person's token, for this Endpoint's scope.
        let exchange = realm
            .received_requests()
            .await
            .expect("recorded")
            .into_iter()
            .find(|request| request.method.as_str() == "POST")
            .expect("an exchange");
        let form = String::from_utf8_lossy(&exchange.body).into_owned();
        assert!(
            form.contains(RAW) && form.contains("endpoint%3Aclosedslug"),
            "{form}"
        );
        for token in [RAW, EXCHANGED] {
            assert!(!logs.contains(token), "the log carries a token: {logs}");
            assert!(!serde_json::to_string(&data).unwrap().contains(token));
        }
    }

    #[tokio::test]
    async fn a_public_panel_sends_no_token() {
        let server = service("grafana").await;
        render_answering(&server).await;
        let mut state = world(Some(&server));
        let _realm = exchanging(&mut state).await;
        let data = panel_with(admin(), &state, 1, &[], bearer())
            .await
            .expect("frames");
        assert!(data.error.is_none(), "{data:?}");
        let requests = server.received_requests().await.expect("recorded");
        assert_eq!(person_headers(&requests), [None]);
    }

    #[tokio::test]
    async fn a_cookie_session_reads_no_members_only_panel_and_says_where_to_sign_in() {
        let server = service("grafana").await;
        Mock::given(method("GET"))
            .and(path("/api/dashboards/live/render"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "frameSets": [] })))
            .expect(0)
            .mount(&server)
            .await;
        let mut state = world(Some(&server));
        let _realm = exchanging(&mut state).await;
        let data = panel(admin(), &state, 3, &[]).await.expect("an answer");
        let error = data.error.expect("the panel's error line");
        assert!(
            error.contains("sign in through the platform's address"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn an_outsider_meets_no_project_on_any_door() {
        let server = service("grafana").await;
        let state = world(Some(&server));
        assert!(matches!(
            list_dashboards(outsider(), State(state.clone()), Path(PROJECT.into())).await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            get_dashboard(
                outsider(),
                State(state.clone()),
                Path((PROJECT.into(), "live".into()))
            )
            .await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            panel(outsider(), &state, 1, &[]).await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            variable_values(
                outsider(),
                State(state),
                Path((PROJECT.into(), "live".into(), "run".into()))
            )
            .await,
            Err(ApiError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn without_a_service_the_list_says_why_and_a_dashboard_is_a_conflict() {
        let state = world(None);
        let Json(list) = list_dashboards(admin(), State(state.clone()), Path(PROJECT.into()))
            .await
            .expect("a list");
        assert!(!list.ready && list.reason.is_some() && list.items.is_empty());
        assert!(matches!(
            get_dashboard(admin(), State(state), Path((PROJECT.into(), "live".into()))).await,
            Err(ApiError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn the_dashboard_keeps_its_queries_on_the_server() {
        let server = service("grafana").await;
        let state = world(Some(&server));
        let Json(list) = list_dashboards(admin(), State(state.clone()), Path(PROJECT.into()))
            .await
            .expect("a list");
        let titles: Vec<&str> = list.items.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, ["Air", "Helsinki live"]);
        let Json(view) = get_dashboard(
            admin(),
            State(state.clone()),
            Path((PROJECT.into(), "live".into())),
        )
        .await
        .expect("the dashboard");
        assert_eq!(
            (view.from.as_str(), view.refresh.as_deref()),
            ("now-7d", Some("5m"))
        );
        let text = serde_json::to_string(&view.panels).unwrap();
        assert!(
            !text.contains("targets") && !text.contains("open-ds"),
            "{text}"
        );
        assert_eq!(
            (view.time_picker_hidden, view.timezone.as_deref()),
            (true, Some("browser"))
        );
        assert_eq!(
            (
                &view.panels[0]["timeFrom"],
                &view.panels[0]["timeShift"],
                &view.panels[0]["hideTimeOverride"]
            ),
            (&json!("1h"), &json!("1d"), &json!(true))
        );
        assert_eq!(view.panels[1]["panels"][0]["title"], "Secret");
        let station = &view.variables[0];
        assert_eq!(station.options.len(), 2);
        assert_eq!(station.current, ["Kallio"]);
        assert_eq!(view.variables[1].label, "Run");
        // Nothing saved and no All to fall back on: a query variable waits for its values.
        assert!(view.variables[1].current.is_empty());
        // Grafana's own pick when nothing is saved: All, else the first value.
        assert_eq!(view.variables[2].current, ["$__all"]);
        assert_eq!(view.variables[3].current, ["north"]);
        assert!(matches!(
            get_dashboard(admin(), State(state), Path((PROJECT.into(), "gone".into()))).await,
            Err(ApiError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_panel_runs_its_saved_queries_with_the_variables_asked() {
        let server = service("grafana").await;
        Mock::given(method("GET"))
            .and(path("/api/dashboards/live/render"))
            .and(query_param("panel", "1"))
            .and(query_param("from", "now-1h"))
            .and(query_param("var-station", "Kallio"))
            .and(query_param("var-station", "Mannerheimintie"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "frameSets": [{ "refId": "A", "frames": [{ "name": "pm25", "fields": [
                    { "name": "time", "type": "time", "values": [1, 2] },
                    { "name": "pm25", "type": "number", "values": [3.5, 4.0] }
                ]}]}],
                "htmlTable": "<table></table>"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let state = world(Some(&server));
        let data = panel(
            admin(),
            &state,
            1,
            &[
                ("from", "now-1h"),
                ("var-station", "Kallio"),
                ("var-station", "Mannerheimintie"),
            ],
        )
        .await
        .expect("frames");
        assert_eq!(data.frames.len(), 1);
        assert_eq!(data.frames[0]["refId"], "A");
        assert!(data.access_denied.is_none() && data.error.is_none());
    }

    #[tokio::test]
    async fn a_panel_reading_an_endpoint_the_caller_may_not_read_runs_nothing() {
        let server = service("grafana").await;
        Mock::given(method("GET"))
            .and(path("/api/dashboards/live/render"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "frameSets": [] })))
            .expect(0)
            .mount(&server)
            .await;
        let state = world(Some(&server));
        // A viewer of the project reads only public Endpoints here.
        let viewer = person(&[]);
        let effective = crate::permissions::for_request(&state, &viewer.0.identity, PROJECT);
        let closed = find_panel(board()["panels"].as_array().unwrap(), 3)
            .unwrap()
            .clone();
        let held = vec![HeldDatasource {
            uid: "closed-ds".into(),
            slug: Some("closedslug".into()),
        }];
        assert!(denial(&state, &effective, PROJECT, &closed, &held).is_some());
        // An unknown Endpoint slug is refused too, and a datasource named by a variable.
        let elsewhere = vec![HeldDatasource {
            uid: "closed-ds".into(),
            slug: Some("nobodys".into()),
        }];
        let admin_rights = crate::permissions::for_request(&state, &admin().0.identity, PROJECT);
        assert!(denial(&state, &admin_rights, PROJECT, &closed, &elsewhere).is_some());
        let data = panel(admin(), &state, 4, &[]).await.expect("an answer");
        assert!(data.access_denied.is_some(), "{data:?}");
    }

    #[tokio::test]
    async fn a_datasource_failure_is_the_panels_error_state() {
        let server = service("grafana").await;
        Mock::given(method("GET"))
            .and(path("/api/dashboards/live/render"))
            .respond_with(
                ResponseTemplate::new(502)
                    .set_body_json(json!({ "message": "upstream answered 503" })),
            )
            .mount(&server)
            .await;
        let state = world(Some(&server));
        let data = panel(admin(), &state, 1, &[]).await.expect("an answer");
        assert_eq!(data.error.as_deref(), Some("upstream answered 503"));
        assert!(matches!(
            panel(admin(), &state, 99, &[]).await,
            Err(ApiError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn each_question_the_dashboard_does_not_take_is_refused_by_field() {
        let dashboard = board();
        let refused_field =
            |pairs: &[(&str, &str)]| match checked_query(&dashboard, &asked(pairs).0) {
                Err(ApiError::Unprocessable { errors, .. }) => {
                    errors[0].split(':').next().unwrap().to_owned()
                }
                other => panic!("{pairs:?} passed: {other:?}"),
            };
        assert_eq!(refused_field(&[("from", "yesterday")]), "from");
        assert_eq!(refused_field(&[("to", "now-6x")]), "to");
        assert_eq!(refused_field(&[("maxDataPoints", "0")]), "maxDataPoints");
        assert_eq!(refused_field(&[("expr", "up")]), "expr");
        assert_eq!(refused_field(&[("var-nope", "a")]), "var-nope");
        assert_eq!(
            refused_field(&[("var-station", "Elsewhere")]),
            "var-station"
        );
        assert_eq!(
            refused_field(&[("var-run", "a"), ("var-run", "b")]),
            "var-run"
        );
        assert_eq!(refused_field(&[("var-run", "a\nb")]), "var-run");
        let all = checked_query(
            &dashboard,
            &asked(&[("var-station", "$__all"), ("var-run", "r.1")]).0,
        )
        .unwrap();
        // All and each value go to daiya as asked; daiya formats them from the saved variable.
        assert!(all.contains(&("var-station".into(), "$__all".into())));
        assert!(all.contains(&("var-run".into(), "r.1".into())));
    }

    #[test]
    fn grafana_time_words_pass_and_others_do_not() {
        for good in [
            "now",
            "now-6h",
            "now-30d",
            "now-1d/d",
            "now/d",
            "1700000000000",
            "2026-11-08T10:00:00Z",
        ] {
            assert!(is_time(good), "{good}");
        }
        for bad in [
            "",
            "now-",
            "now-h",
            "now-6x",
            "tomorrow",
            "now-6h;drop",
            "-1",
        ] {
            assert!(!is_time(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn daiya_gets_the_projects_organization_on_first_read_grafana_does_not() {
        for (health, makes) in [("daiya", 1), ("grafana", 0)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/health"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": health })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/api/orgs/name/{PROJECT}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/api/orgs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "orgId": 3 })))
                .expect(makes)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/api/folders"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({ "uid": "project-helsinki" })),
                )
                .expect(makes)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/api/search"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
                .mount(&server)
                .await;
            let state = world(Some(&server));
            let Json(list) = list_dashboards(admin(), State(state), Path(PROJECT.into()))
                .await
                .expect("a list");
            assert_eq!(list.ready, makes == 1, "{health}: {list:?}");
        }
    }

    #[tokio::test]
    async fn the_provisioning_pass_makes_each_projects_organization_and_folder() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": "daiya" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/orgs/name/{PROJECT}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/orgs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "orgId": 3 })))
            .expect(1)
            .mount(&server)
            .await;
        // A folder that is there already answers 412, which the pass takes as done.
        Mock::given(method("POST"))
            .and(path("/api/folders"))
            .respond_with(ResponseTemplate::new(412))
            .expect(1)
            .mount(&server)
            .await;
        let state = world(Some(&server));
        assert_eq!(crate::grafana::projects(&state), [PROJECT]);
        crate::grafana::provision_projects(
            &state,
            &Client::new(state.config.grafana.as_ref().unwrap()),
        )
        .await;
    }
}
