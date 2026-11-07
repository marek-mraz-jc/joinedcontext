//! What an assistant will know (API/05 §1.9, AG-121, AG-123, ADR-N-055 §2.6, T-4693): the
//! Endpoints its connectors, selector and named server resolve to, its knowledge sources and its
//! categories, for the editor's live preview of an unsaved manifest and for a saved deployment.
//!
//! The Endpoints come from `jc_core::reach::resolve_connectors`, the one resolver `jcctl
//! validate`, `jc-assistant` and the gateway call too, over the repository the Portal mirrors; this
//! module filters nothing of its own. Counts and crawl times are `jc-assistant`'s and are not
//! shown here.

use axum::extract::{Path, Query, State};
use axum::Json;
use jc_core::kinds::assistant::{
    connector_target, AssistantDeploymentSpec, KnowledgeSourceSpec, Localised,
};
use jc_core::kinds::catalog::DataTheme;
use jc_core::kinds::{EndpointSpec, McpServerSpec};
use jc_core::reach::{resolve_connectors, EndpointFacts};
use serde::Deserialize;
use serde_json::{json, Value};
use utoipa::IntoParams;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::resource::{is_dns1123, ResourceEnvelope};
use crate::state::AppState;
use crate::store::Mirror;

/// The language of the labels, and for the preview the dry-run marker.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ContextQuery {
    /// The language of the category labels and titles: `sk`, `en`, `cs`, `hu`, `de`, `fi` or
    /// `uk`; English for any other.
    pub lang: Option<String>,
    /// `All`: the preview changes nothing (API/01 §dry runs).
    #[serde(rename = "dryRun")]
    pub dry_run: Option<String>,
}

/// The caller reads the project and its AssistantDeployments; anyone else learns nothing,
/// not even that the project exists (AG-113).
fn authorize(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if is_dns1123(project)
        && effective.may_read_project()
        && effective.may_read("AssistantDeployment")
    {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!("project '{project}' not found")))
    }
}

fn language(query: &ContextQuery) -> &str {
    query.lang.as_deref().unwrap_or("en")
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/knowledge/deployments/context",
    operation_id = "previewAssistantContext",
    summary = "Preview What An Assistant Will Know",
    description = "Resolves an unsaved AssistantDeployment manifest the way the assistant will: its Endpoints (connectors, select and mcpServer, less exclude, inside the project's reach), its knowledge sources and its categories. Changes nothing; `dryRun=All` is required.",
    tag = "knowledge",
    params(("project" = String, Path, description = "Project name"), ContextQuery),
    request_body(
        content = crate::resource::ResourceEnvelope,
        description = "The AssistantDeployment manifest being edited",
        example = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "AssistantDeployment",
            "metadata": { "name": "doprava", "namespace": "bb" },
            "spec": {
                "publicId": "bb-doprava",
                "channel": "public",
                "select": { "themes": ["TRAN"] },
                "scope": { "projects": ["bbsk"] },
                "categories": ["TRAN"],
                "allowedOrigins": ["https://www.banskabystrica.sk"],
                "rateLimit": { "requestsPerMinute": 60, "perClientPerMinute": 10 },
                "budget": { "tokensPerDay": 2000000, "tokensPerConversation": 40000 },
                "languages": ["sk", "en"]
            }
        })
    ),
    responses(
        (status = 200, description = "What the assistant will read", body = Object),
        (status = 400, description = "Not an AssistantDeployment, or a field its kind refuses", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn preview_context(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ContextQuery>,
    Json(manifest): Json<ResourceEnvelope>,
) -> Result<Json<Value>, ApiError> {
    authorize(&state, &user, &project)?;
    let spec = previewed(&query, manifest)?;
    let lang = language(&query);
    // An unsaved manifest may name any project's source or MCP server: the preview tells only
    // of those the caller reads (F-R2POR2-003). A saved one was approved to name them.
    let identity = &user.0.identity;
    let readable = |namespace: &str, kind: &str| {
        namespace == project
            || crate::permissions::for_request(&state, identity, namespace).may_read(kind)
    };
    Ok(Json(context_of(
        &state.mirror,
        &project,
        &spec,
        lang,
        &readable,
    )))
}

/// The spec of the manifest a preview reads: a dry run of an AssistantDeployment alone.
fn previewed(
    query: &ContextQuery,
    manifest: ResourceEnvelope,
) -> Result<AssistantDeploymentSpec, ApiError> {
    if query.dry_run.as_deref() != Some("All") {
        return Err(ApiError::BadRequest(
            "the preview is a dry run: send it with dryRun=All".into(),
        ));
    }
    if manifest.kind != "AssistantDeployment" {
        return Err(ApiError::BadRequest(format!(
            "the preview reads an AssistantDeployment, not a {}",
            manifest.kind
        )));
    }
    serde_json::from_value(manifest.spec)
        .map_err(|err| ApiError::BadRequest(format!("spec: {err}")))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/deployments/{deployment}/context",
    operation_id = "getAssistantContext",
    summary = "Read What An Assistant Knows",
    description = "The Endpoints, knowledge sources and categories of one saved AssistantDeployment, resolved as the assistant resolves them.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("deployment" = String, Path, description = "AssistantDeployment name"),
        ContextQuery,
    ),
    responses(
        (status = 200, description = "What the assistant reads", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or deployment the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn deployment_context(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, deployment)): Path<(String, String)>,
    Query(query): Query<ContextQuery>,
) -> Result<Json<Value>, ApiError> {
    authorize(&state, &user, &project)?;
    let spec = saved(&state.mirror, &project, &deployment)?;
    let lang = language(&query);
    Ok(Json(context_of(
        &state.mirror,
        &project,
        &spec,
        lang,
        &|_, _| true,
    )))
}

/// The spec of saved deployment `deployment`; a name that is none is 404.
fn saved(
    mirror: &Mirror,
    project: &str,
    deployment: &str,
) -> Result<AssistantDeploymentSpec, ApiError> {
    let not_found = || ApiError::NotFound(format!("deployment '{deployment}' not found"));
    if !is_dns1123(deployment) {
        return Err(not_found());
    }
    let saved = mirror
        .get(project, "AssistantDeployment", deployment)
        .ok_or_else(not_found)?;
    serde_json::from_value(saved.spec).map_err(|err| {
        ApiError::Internal(format!("deployment '{deployment}' does not read: {err}"))
    })
}

/// The classes of the data model of `space` in `project`: the types its Endpoints serve.
pub(crate) fn model_classes(mirror: &Mirror, project: &str, space: &str) -> Vec<String> {
    let Some(space) = mirror.get(project, "ContextSpace", space) else {
        return Vec::new();
    };
    crate::resource::model_reference(&space.spec["dataModelRef"], project)
        .and_then(|(namespace, name)| mirror.get(namespace, "DataModel", name))
        .map(|model| {
            model.spec["classes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|class| class.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn categories(codes: impl IntoIterator<Item = DataTheme>, lang: &str) -> Vec<Value> {
    codes
        .into_iter()
        .map(|code| json!({ "code": code.code(), "label": code.label_in(lang) }))
        .collect()
}

/// A text in `lang`, else the first one there is.
fn localised(text: &Localised, lang: &str) -> Option<String> {
    let texts = text.texts();
    texts
        .iter()
        .find(|(language, _)| *language == Some(lang))
        .or(texts.first())
        .map(|(_, text)| (*text).to_owned())
}

/// What deployment `spec` of `project` reads, from the mirrored repository (API/05 §1.9). A
/// selector's projects are read from the deployment's own project and `scope.projects` alone,
/// the resolver's candidates; the manifest's own rule findings ride along as `problems`.
/// `readable(namespace, kind)` says whether a source or MCP server another project holds is told
/// of; one it refuses reads as absent.
pub fn context_of(
    mirror: &Mirror,
    project: &str,
    spec: &AssistantDeploymentSpec,
    lang: &str,
    readable: &dyn Fn(&str, &str) -> bool,
) -> Value {
    let reach: Vec<&str> = std::iter::once(project)
        .chain(spec.scope_projects().iter().map(String::as_str))
        .collect();
    let manifests = mirror.matching(|env| {
        env.kind == "Endpoint"
            && env
                .metadata
                .namespace
                .as_deref()
                .is_some_and(|ns| reach.contains(&ns))
    });
    let mut titles = std::collections::BTreeMap::new();
    let mut facts = Vec::with_capacity(manifests.len());
    for env in &manifests {
        let (Some(namespace), Ok(endpoint)) = (
            env.metadata.namespace.as_deref(),
            serde_json::from_value::<EndpointSpec>(env.spec.clone()),
        ) else {
            continue;
        };
        let types = model_classes(mirror, namespace, endpoint.context_space_ref.name());
        let fact = EndpointFacts::of(namespace, &env.metadata.name, &endpoint, types);
        let title = env
            .metadata
            .title
            .as_ref()
            .map(|title| title.resolve(&[lang.to_owned()], "en").to_owned())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| env.metadata.name.clone());
        titles.insert(fact.slug.clone(), title);
        facts.push(fact);
    }
    let org_all: Option<McpServerSpec> =
        serde_json::from_value(json!({ "audience": "organization", "select": {} })).ok();
    let server = spec.mcp_server.as_deref().and_then(|named| {
        let (namespace, name) = connector_target(named);
        let namespace = namespace.unwrap_or(project);
        mirror
            .get(namespace, "McpServer", name)
            .filter(|_| readable(namespace, "McpServer"))
            .and_then(|env| serde_json::from_value::<McpServerSpec>(env.spec).ok())
            .or_else(|| (named == "org/all").then(|| org_all.clone()).flatten())
            .map(|server| (namespace.to_owned(), server))
    });
    let resolution = resolve_connectors(
        project,
        spec,
        &facts,
        server
            .as_ref()
            .map(|(namespace, server)| (namespace.as_str(), server)),
    );
    let by_slug: std::collections::BTreeMap<&str, &EndpointFacts> = facts
        .iter()
        .map(|fact| (fact.slug.as_str(), fact))
        .collect();
    let endpoints: Vec<Value> = resolution
        .endpoints
        .iter()
        .map(|resolved| {
            let fact = by_slug.get(resolved.slug.as_str());
            json!({
                "slug": resolved.slug,
                "project": resolved.project,
                "name": resolved.name,
                "title": titles.get(&resolved.slug),
                "audience": fact.map(|f| f.audience.as_str()),
                "themes": categories(fact.map(|f| f.themes.clone()).unwrap_or_default(), lang),
                "types": fact.map(|f| f.types.clone()).unwrap_or_default(),
                "tools": resolved.tools,
                "timeoutSeconds": resolved.timeout_seconds,
            })
        })
        .collect();
    let sources: Vec<Value> = spec
        .sources
        .iter()
        .filter_map(|named| {
            let (namespace, name) = connector_target(named);
            let namespace = namespace.unwrap_or(project);
            let env = mirror
                .get(namespace, "KnowledgeSource", name)
                .filter(|_| readable(namespace, "KnowledgeSource"))?;
            let source: KnowledgeSourceSpec = serde_json::from_value(env.spec).ok()?;
            let sites: std::collections::BTreeSet<String> = source
                .start_urls
                .iter()
                .filter_map(|url| url::Url::parse(url).ok()?.host_str().map(str::to_owned))
                .collect();
            Some(json!({
                "project": namespace,
                "name": name,
                "kind": source.source,
                "visibility": source.visibility,
                "sites": sites,
                "categories": categories(source.categories.iter().copied(), lang),
            }))
        })
        .collect();
    let problems: Vec<String> = spec
        .validate()
        .err()
        .map(|err| err.to_string())
        .into_iter()
        .collect();
    json!({
        "publicId": spec.public_id,
        "channel": spec.channel.as_str(),
        "title": spec.theme.as_ref().and_then(|theme| theme.title.as_ref()).and_then(|title| localised(title, lang)),
        "languages": spec.languages,
        "categories": categories(spec.categories.iter().copied(), lang),
        "limits": {
            "perClientPerHour": spec.rate_limit.as_ref().map(|r| r.per_client_per_hour),
            "perClientPerDay": spec.rate_limit.as_ref().map(|r| r.per_client_per_day),
            "messagesPerConversation": spec.budget.as_ref().map(|b| b.messages_per_conversation),
            "questionChars": spec.budget.as_ref().map(|b| b.question_chars),
        },
        "endpoints": endpoints,
        "omitted": resolution.omitted,
        "sources": sources,
        "problems": problems,
    })
}
