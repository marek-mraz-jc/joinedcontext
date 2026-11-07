//! The Grafana page of a project (T-6509): which Endpoints a person can draw from, and the
//! visualizations saved into the project's Grafana folder (`crate::grafana`). The browser never
//! speaks to Grafana's API. Every choice the page offers is checked again here when it is saved.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::{CurrentUser, Identity};
use crate::error::{ApiError, ProblemDetails};
use crate::grafana::{self, space_of, Client, Kind, Source, Visualization};
use crate::permissions::Effective;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;

/// The longest title a person may give.
const MAX_TITLE: usize = 120;

/// What saving and removing a visualization is judged as: a project's dashboards, a project-wide
/// kind, so a role bound to one space alone neither saves nor removes one.
const KIND: &str = "Dashboard";

/// The project's page.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrafanaPage {
    /// Grafana answers and holds this project's organization.
    pub ready: bool,
    /// What a person reads when it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The caller may save and remove visualizations here.
    pub may_create: bool,
    /// Those whose Endpoint the caller may read, newest first.
    pub items: Vec<VisualizationItem>,
}

/// One saved visualization.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VisualizationItem {
    pub id: String,
    pub title: String,
    pub endpoint: String,
    /// The Endpoint's title, the name its dropdown shows.
    pub endpoint_title: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub attribute: String,
    pub kind: Kind,
    pub created_at: String,
    /// Where it opens in Grafana; `None` until the organization is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_url: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct OptionsAsked {
    /// The Endpoint chosen so far: the answer then lists its types.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// The type chosen so far: the answer then lists its attributes.
    #[serde(default, rename = "type")]
    pub entity_type: Option<String>,
}

/// One Endpoint the caller may read.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointOption {
    pub name: String,
    pub title: String,
    pub slug: String,
    pub public: bool,
    /// Grafana can read it: public, or its slug is in the project's datasources.
    pub usable: bool,
}

/// One attribute and what can draw it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttributeOption {
    pub name: String,
    pub kinds: Vec<Kind>,
}

/// What the form's dropdowns may offer this person now.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrafanaOptions {
    pub endpoints: Vec<EndpointOption>,
    pub types: Vec<String>,
    pub attributes: Vec<AttributeOption>,
}

/// A new visualization.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({
    "title": "PM2.5 by station", "endpoint": "helsinki-air-quality",
    "type": "AirQualityObserved", "attribute": "pm25", "kind": "timeseries"
}))]
pub struct NewVisualization {
    pub title: String,
    pub endpoint: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub attribute: String,
    pub kind: Kind,
}

fn stored(err: sqlx::Error) -> ApiError {
    tracing::warn!(error = %err, "the visualizations could not be read or written");
    ApiError::Unavailable("the saved visualizations could not be read or written".into())
}

fn refused(field: &str, detail: String) -> ApiError {
    ApiError::Unprocessable {
        detail: detail.clone(),
        errors: vec![format!("{field}: {detail}")],
    }
}

/// The caller's rights in the project; 404 for a project they have no part in.
fn member(state: &AppState, identity: &Identity, project: &str) -> Result<Effective, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !crate::resource::is_dns1123(project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    Ok(effective)
}

fn may_read_endpoint(effective: &Effective, endpoint: &ResourceEnvelope) -> bool {
    serde_json::to_value(endpoint)
        .is_ok_and(|value| effective.may_read_manifest("Endpoint", &value))
}

fn is_public(endpoint: &ResourceEnvelope) -> bool {
    endpoint.spec["audience"] == "public"
}

/// The name a person knows the Endpoint by.
fn title_of_endpoint(endpoint: &ResourceEnvelope) -> String {
    endpoint
        .metadata
        .title
        .as_ref()
        .and_then(|title| crate::resource::title_text(&serde_json::to_value(title).ok()?))
        .unwrap_or_else(|| endpoint.metadata.name.clone())
}

fn slug_of(endpoint: &ResourceEnvelope) -> &str {
    endpoint.spec["slug"].as_str().unwrap_or_default()
}

/// An Endpoint of the project the caller may read.
fn readable_endpoint(
    state: &AppState,
    effective: &Effective,
    project: &str,
    name: &str,
) -> Option<ResourceEnvelope> {
    state
        .mirror
        .get(project, "Endpoint", name)
        .filter(|endpoint| may_read_endpoint(effective, endpoint))
}

fn no_grafana() -> String {
    "Grafana is not set up on this installation; ask your operator to turn the grafana component on"
        .into()
}

fn no_organization(project: &str) -> String {
    format!(
        "Grafana has no organization for {project} yet; ask your operator to add this project to \
         Grafana"
    )
}

/// The organization of `project`, or why there is none to write to.
async fn organization(state: &AppState, project: &str) -> Result<i64, ApiError> {
    let config = state
        .config
        .grafana
        .as_ref()
        .ok_or_else(|| ApiError::Conflict(no_grafana()))?;
    Client::new(config)
        .organization(project)
        .await?
        .ok_or_else(|| ApiError::Conflict(no_organization(project)))
}

fn item(
    state: &AppState,
    row: Visualization,
    endpoint: &ResourceEnvelope,
    org: Option<i64>,
) -> VisualizationItem {
    let open_url = state
        .config
        .grafana
        .as_ref()
        .zip(org)
        .map(|(config, org)| format!("{}/d/{}?orgId={org}", config.public_url, row.uid()));
    VisualizationItem {
        id: row.id,
        title: row.title,
        endpoint: row.endpoint,
        endpoint_title: title_of_endpoint(endpoint),
        entity_type: row.entity_type,
        attribute: row.attribute,
        kind: row.kind,
        created_at: row.created_at,
        open_url,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana",
    summary = "List Grafana Visualizations",
    description = "The visualizations saved into the project's Grafana folder whose Endpoint the caller may read, and whether Grafana is ready for the project. Changes no data.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The page", body = GrafanaPage),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project the caller has a part in", body = ProblemDetails),
    )
)]
pub async fn grafana_page(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<GrafanaPage>, ApiError> {
    let effective = member(&state, &user.0.identity, &project)?;
    let may_create = effective.may(KIND, Verb::Propose);
    let (org, reason) = match state.config.grafana.as_ref() {
        None => (None, Some(no_grafana())),
        Some(config) => match Client::new(config).organization(&project).await {
            Ok(Some(org)) => (Some(org), None),
            Ok(None) => (None, Some(no_organization(&project))),
            Err(err) => (None, Some(err.to_string())),
        },
    };
    let rows = match state.db.as_ref() {
        Some(pool) => grafana::list(pool, Some(&project)).await.map_err(stored)?,
        None => Vec::new(),
    };
    let items = rows
        .into_iter()
        .filter_map(|row| {
            let endpoint = readable_endpoint(&state, &effective, &project, &row.endpoint)?;
            Some(item(&state, row, &endpoint, org))
        })
        .collect();
    Ok(Json(GrafanaPage {
        ready: org.is_some(),
        reason,
        may_create,
        items,
    }))
}

/// The Endpoints of the project the caller may read, sorted by title.
async fn endpoint_options(
    state: &AppState,
    effective: &Effective,
    project: &str,
) -> Result<Vec<EndpointOption>, ApiError> {
    let held = match organization(state, project).await {
        Ok(org) => {
            let config = state.config.grafana.as_ref();
            match config {
                Some(config) => Client::new(config).datasources(org).await?,
                None => Vec::new(),
            }
        }
        Err(ApiError::Conflict(_)) => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut options: Vec<EndpointOption> = state
        .mirror
        .list(project, "Endpoint", &ListOptions::default())
        .items
        .iter()
        .filter(|endpoint| may_read_endpoint(effective, endpoint))
        .map(|endpoint| {
            let slug = slug_of(endpoint);
            EndpointOption {
                name: endpoint.metadata.name.clone(),
                title: title_of_endpoint(endpoint),
                slug: slug.to_owned(),
                public: is_public(endpoint),
                usable: is_public(endpoint) || held.iter().any(|source| source.slug == slug),
            }
        })
        .collect();
    options.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(options)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/grafana/options",
    summary = "Read Grafana Form Options",
    description = "What the visualization form may offer the caller: the Endpoints they may read, the types of the chosen Endpoint's model and the attributes of the chosen type, each with the visualization kinds that can draw it. Changes no data.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name"), OptionsAsked),
    responses(
        (status = 200, description = "The options", body = GrafanaOptions),
        (status = 400, description = "An unknown query parameter", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project, or an Endpoint the caller may not read", body = ProblemDetails),
    )
)]
pub async fn grafana_options(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    asked: Result<Query<OptionsAsked>, QueryRejection>,
) -> Result<Json<GrafanaOptions>, ApiError> {
    let Query(asked) = asked.map_err(|err| ApiError::BadRequest(err.body_text()))?;
    let effective = member(&state, &user.0.identity, &project)?;
    let mut answer = GrafanaOptions {
        endpoints: endpoint_options(&state, &effective, &project).await?,
        ..GrafanaOptions::default()
    };
    let Some(name) = asked.endpoint else {
        return Ok(Json(answer));
    };
    let endpoint = readable_endpoint(&state, &effective, &project, &name)
        .ok_or_else(|| ApiError::NotFound(format!("endpoint '{name}' not found")))?;
    let Some(model) = state.model_schemas.get(&project, space_of(&endpoint)) else {
        return Ok(Json(answer));
    };
    answer.types = model.classes.keys().cloned().collect();
    if let Some(attributes) = asked.entity_type.and_then(|class| model.attributes(&class)) {
        answer.attributes = attributes
            .attributes
            .iter()
            .filter_map(|attribute| {
                let kinds: Vec<Kind> = Kind::ALL
                    .into_iter()
                    .filter(|kind| kind.draws(&attribute.kind, attribute.value_type.as_deref()))
                    .collect();
                (!kinds.is_empty()).then(|| AttributeOption {
                    name: attribute.name.clone(),
                    kinds,
                })
            })
            .collect();
    }
    Ok(Json(answer))
}

/// The title as it is stored, or the refusal that names it.
fn title_of(raw: &str) -> Result<&str, ApiError> {
    let title = raw.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE || title.chars().any(char::is_control)
    {
        return Err(refused(
            "title",
            format!(
                "give a title of 1 to {MAX_TITLE} characters, for example Air quality by station"
            ),
        ));
    }
    if crate::secret_scan::secret_text(title) {
        return Err(refused(
            "title",
            "this title reads like a credential; choose another".into(),
        ));
    }
    Ok(title)
}

/// The type and attribute of `new` as the Endpoint's model has them, for the chosen kind.
fn check_choice(
    state: &AppState,
    project: &str,
    space: &str,
    new: &NewVisualization,
) -> Result<(), ApiError> {
    let model = state.model_schemas.get(project, space).ok_or_else(|| {
        refused(
            "endpoint",
            format!("the space {space} pins no data model, so it has nothing to draw"),
        )
    })?;
    let class = model.attributes(&new.entity_type).ok_or_else(|| {
        refused(
            "type",
            format!(
                "{} is not a type of this Endpoint; choose one from the list",
                new.entity_type
            ),
        )
    })?;
    let attribute = class
        .attributes
        .iter()
        .find(|attribute| attribute.name == new.attribute)
        .ok_or_else(|| {
            refused(
                "attribute",
                format!(
                    "{} has no attribute {}; choose one from the list",
                    new.entity_type, new.attribute
                ),
            )
        })?;
    if !new
        .kind
        .draws(&attribute.kind, attribute.value_type.as_deref())
    {
        return Err(refused(
            "kind",
            format!(
                "a {} cannot draw {}; choose another kind",
                new.kind.as_str(),
                new.attribute
            ),
        ));
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/grafana",
    summary = "Save A Grafana Visualization",
    description = "Saves one visualization of an Endpoint into the project's Grafana folder. Needs propose on Dashboard and read on the Endpoint. A closed Endpoint needs its datasource from the deployment; a public one gets an anonymous datasource here. A panel reads as whoever opens it.",
    tag = "grafana",
    params(("project" = String, Path, description = "Project name")),
    request_body = NewVisualization,
    responses(
        (status = 201, description = "Saved", body = VisualizationItem),
        (status = 400, description = "A body that does not parse", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not save dashboards, or may not read the Endpoint", body = ProblemDetails),
        (status = 404, description = "No such project the caller has a part in", body = ProblemDetails),
        (status = 409, description = "Grafana is not set up for the project", body = ProblemDetails),
        (status = 422, description = "A field names what the page does not offer; the errors name it", body = ProblemDetails),
        (status = 503, description = "Grafana or the database did not answer", body = ProblemDetails),
    )
)]
pub async fn save_visualization(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    body: Result<Json<NewVisualization>, JsonRejection>,
) -> Result<(StatusCode, Json<VisualizationItem>), ApiError> {
    let effective = member(&state, &user.0.identity, &project)?;
    effective.check(KIND, Verb::Propose, None)?;
    let Json(new) = body.map_err(|err| ApiError::BadRequest(err.body_text()))?;
    let title = title_of(&new.title)?.to_owned();
    let endpoint =
        readable_endpoint(&state, &effective, &project, &new.endpoint).ok_or_else(|| {
            ApiError::Denied(format!(
            "endpoint: {} is not an Endpoint of {project} you may read; choose one from the list",
            new.endpoint
        ))
        })?;
    check_choice(&state, &project, space_of(&endpoint), &new)?;
    let pool = state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no database to keep visualizations in".into())
    })?;
    let org = organization(&state, &project).await?;
    let config = state
        .config
        .grafana
        .as_ref()
        .ok_or_else(|| ApiError::Conflict(no_grafana()))?;
    let client = Client::new(config);
    let row = Visualization {
        id: grafana::new_id(),
        project: project.clone(),
        title,
        endpoint: new.endpoint.clone(),
        entity_type: new.entity_type,
        attribute: new.attribute,
        kind: new.kind,
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let source = Source {
        name: &new.endpoint,
        slug: slug_of(&endpoint),
        public: is_public(&endpoint),
    };
    if !grafana::push(&state, &client, org, &row, &source, space_of(&endpoint)).await? {
        return Err(refused(
            "endpoint",
            format!(
                "Grafana cannot read the private Endpoint {} yet; ask your operator to add it to \
                 this project's Grafana, or choose another",
                title_of_endpoint(&endpoint)
            ),
        ));
    }
    grafana::insert(pool, &row).await.map_err(stored)?;
    Ok((
        StatusCode::CREATED,
        Json(item(&state, row, &endpoint, Some(org))),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/grafana/{id}",
    summary = "Remove A Grafana Visualization",
    description = "Takes one saved visualization out of the project's Grafana folder. Needs propose on Dashboard.",
    tag = "grafana",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "The visualization"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not change dashboards", body = ProblemDetails),
        (status = 404, description = "No such project or visualization", body = ProblemDetails),
        (status = 409, description = "Grafana is not set up for the project", body = ProblemDetails),
        (status = 503, description = "Grafana or the database did not answer", body = ProblemDetails),
    )
)]
pub async fn remove_visualization(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let effective = member(&state, &user.0.identity, &project)?;
    effective.check(KIND, Verb::Propose, None)?;
    let pool = state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no database to keep visualizations in".into())
    })?;
    let row = grafana::find(pool, &project, &id)
        .await
        .map_err(stored)?
        .ok_or_else(|| ApiError::NotFound(format!("visualization {id} not found")))?;
    let org = organization(&state, &project).await?;
    let config = state
        .config
        .grafana
        .as_ref()
        .ok_or_else(|| ApiError::Conflict(no_grafana()))?;
    Client::new(config).remove(org, &row.uid()).await?;
    grafana::delete(pool, &id).await.map_err(stored)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/grafana",
            get(grafana_page).post(save_visualization),
        )
        .route("/projects/{project}/grafana/options", get(grafana_options))
        .route(
            "/projects/{project}/grafana/{id}",
            delete(remove_visualization),
        )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::auth::session::Session;
    use crate::config::{Config, GrafanaConfig};
    use crate::pipeline_validation::ModelSchema;
    use crate::resource::{ObjectMeta, API_VERSION};

    const PROJECT: &str = "helsinki";

    /// `groups` decides the rights: Config::for_tests names `portal-approver` its administrators.
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

    /// One closed and one public Endpoint, each naming its space by a typed reference as a
    /// manifest may, and a model on that space.
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
                spec: json!({
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                    "slug": slug,
                    "audience": audience,
                }),
                status: None,
            });
        }
        let schema = json!({ "definitions": { "AirQualityObserved": { "type": "object", "properties": {
            "pm25": { "type": "number", "x-ngsi-ld-kind": "Property" },
        }}}});
        let model = ModelSchema::compile(
            "air",
            "1.0.0",
            &schema,
            &["AirQualityObserved".to_owned()],
            false,
        );
        state
            .model_schemas
            .replace(std::collections::HashMap::from([(
                (PROJECT.to_owned(), "air".to_owned()),
                std::sync::Arc::new(model),
            )]));
        state
    }

    async fn grafana_of_the_project() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/orgs/name/{PROJECT}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 7 })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/datasources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        server
    }

    fn new_visualization(endpoint: &str, attribute: &str) -> NewVisualization {
        NewVisualization {
            title: "PM2.5".into(),
            endpoint: endpoint.into(),
            entity_type: "AirQualityObserved".into(),
            attribute: attribute.into(),
            kind: Kind::Timeseries,
        }
    }

    async fn save(
        user: CurrentUser,
        state: &AppState,
        new: NewVisualization,
    ) -> Result<(StatusCode, Json<VisualizationItem>), ApiError> {
        save_visualization(
            user,
            State(state.clone()),
            Path(PROJECT.to_owned()),
            Ok(Json(new)),
        )
        .await
    }

    async fn options(
        user: CurrentUser,
        state: &AppState,
        endpoint: Option<&str>,
    ) -> Result<Json<GrafanaOptions>, ApiError> {
        let asked = OptionsAsked {
            endpoint: endpoint.map(str::to_owned),
            entity_type: None,
        };
        grafana_options(
            user,
            State(state.clone()),
            Path(PROJECT.to_owned()),
            Ok(Query(asked)),
        )
        .await
    }

    #[tokio::test]
    async fn the_page_says_why_there_is_no_grafana() {
        let state = world(None);
        let Json(page) = grafana_page(admin(), State(state), Path(PROJECT.to_owned()))
            .await
            .expect("a page");
        assert!(!page.ready && page.may_create);
        assert_eq!(page.reason, Some(no_grafana()));
        assert!(page.items.is_empty());
    }

    #[tokio::test]
    async fn the_page_names_a_project_grafana_does_not_hold() {
        let server = MockServer::start().await;
        let state = world(Some(&server));
        let Json(page) = grafana_page(admin(), State(state), Path(PROJECT.to_owned()))
            .await
            .expect("a page");
        assert!(!page.ready);
        assert_eq!(page.reason, Some(no_organization(PROJECT)));
    }

    #[tokio::test]
    async fn an_outsider_meets_no_project_on_any_door() {
        let state = world(None);
        let page = grafana_page(outsider(), State(state.clone()), Path(PROJECT.to_owned())).await;
        assert!(matches!(page, Err(ApiError::NotFound(_))));
        assert!(matches!(
            options(outsider(), &state, None).await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            save(outsider(), &state, new_visualization("open", "pm25")).await,
            Err(ApiError::NotFound(_))
        ));
        let removed = remove_visualization(
            outsider(),
            State(state),
            Path((PROJECT.to_owned(), "ab12".to_owned())),
        )
        .await;
        assert!(matches!(removed, Err(ApiError::NotFound(_))));
    }

    #[tokio::test]
    async fn the_form_offers_a_public_endpoint_and_flags_a_closed_one_without_a_datasource() {
        let server = grafana_of_the_project().await;
        let state = world(Some(&server));
        let Json(answer) = options(admin(), &state, None).await.expect("options");
        let usable: Vec<(&str, bool)> = answer
            .endpoints
            .iter()
            .map(|endpoint| (endpoint.name.as_str(), endpoint.usable))
            .collect();
        assert_eq!(usable, [("closed", false), ("open", true)]);
    }

    #[tokio::test]
    async fn the_form_reads_the_model_of_a_typed_space_reference() {
        let state = world(None);
        let Json(answer) = options(admin(), &state, Some("open"))
            .await
            .expect("options");
        assert_eq!(answer.types.len(), 1, "{answer:?}");
    }

    #[tokio::test]
    async fn the_form_knows_no_endpoint_outside_the_project() {
        let state = world(None);
        assert!(matches!(
            options(admin(), &state, Some("elsewhere")).await,
            Err(ApiError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_save_refuses_each_field_before_it_writes() {
        let state = world(None);
        let mut blank = new_visualization("open", "pm25");
        blank.title = "  ".into();
        assert!(matches!(
            save(admin(), &state, blank).await,
            Err(ApiError::Unprocessable { .. })
        ));
        let Err(ApiError::Denied(detail)) =
            save(admin(), &state, new_visualization("elsewhere", "pm25")).await
        else {
            panic!("an unknown Endpoint is refused");
        };
        assert!(detail.starts_with("endpoint:"), "{detail}");
        let Err(ApiError::Unprocessable { errors, .. }) =
            save(admin(), &state, new_visualization("open", "noise")).await
        else {
            panic!("an unknown attribute is refused");
        };
        assert!(
            errors[..]
                .iter()
                .any(|error| error.starts_with("attribute:")),
            "{errors:?}"
        );
    }

    #[tokio::test]
    async fn a_save_and_a_remove_need_a_database() {
        let state = world(None);
        assert!(matches!(
            save(admin(), &state, new_visualization("open", "pm25")).await,
            Err(ApiError::Unavailable(_))
        ));
        let removed = remove_visualization(
            admin(),
            State(state),
            Path((PROJECT.to_owned(), "ab12".to_owned())),
        )
        .await;
        assert!(matches!(removed, Err(ApiError::Unavailable(_))));
    }
}
