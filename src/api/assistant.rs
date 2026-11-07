//! The assistant finds data (AG-58, API/01 §18): the spaces, endpoints and data models of a
//! project whose manifests match the words of a question, each with the caller's access verdict
//! and the freshness of the pipeline feeding it. Answered from the mirror, never from a guess;
//! the same search is the kit run's `search_catalog` tool (Architecture/09 §9).

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::agents::access::{self, refusal};
use crate::agents::oneshot;
use crate::agents::profile::Profile;
use crate::agents::run::{digest_prompt, mint_run_id, mint_ticket, AgentRun, AgentRunStatus};
use crate::agents::share;
use crate::agents::store::now_rfc3339;
use crate::api::agent_runs::{
    agent_settings, conversation_profile, expiry, publish_event, status_payload, unavailable,
    CreatedRun, MAX_PROMPT_CHARS,
};
use crate::api::pipelines::metrics_for;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::{is_dns1123, ResourceEnvelope};
use crate::state::AppState;
use crate::store::ListOptions;

/// The best matches only: a person reads a screen of cards, a model a page of context.
pub const MAX_ITEMS: usize = 20;
/// In the order ties are broken: what a person opens first.
pub const KINDS: [&str; 3] = ["Endpoint", "ContextSpace", "DataModel"];

/// What a search of the project's own catalogue answered (API/01 §18, AG-58).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// The words the search ran on, with the function words removed.
    pub q: String,
    pub items: Vec<CatalogItem>,
}

/// One manifest the search matched. A restricted item carries its kind and name and nothing
/// else: what the caller may not read is not described to them (AG-58).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    pub kind: String,
    pub name: String,
    pub space: String,
    pub owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_slug: Option<String>,
    pub match_reason: Vec<String>,
    pub access: Access,
    pub freshness: Option<Freshness>,
    #[serde(skip)]
    hits: usize,
}

/// Whether the caller may read the item, and why.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Access {
    /// `allowed` or `restricted`.
    pub verdict: String,
    pub reason: String,
}

/// The runner's counters for the pipeline that feeds an endpoint, read when the search ran.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Freshness {
    pub pipeline: String,
    pub scraped_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<u64>,
}

impl Access {
    fn allowed(reason: impl Into<String>) -> Self {
        Self {
            verdict: "allowed".into(),
            reason: reason.into(),
        }
    }
    fn restricted(reason: impl Into<String>) -> Self {
        Self {
            verdict: "restricted".into(),
            reason: reason.into(),
        }
    }
    pub(crate) fn is_allowed(&self) -> bool {
        self.verdict == "allowed"
    }
}

/// The function words of a request, which name nothing in a catalog (API/01 §18).
const FILLER: &[&str] = &[
    "a", "about", "add", "again", "all", "also", "an", "and", "any", "are", "as", "at", "be",
    "build", "but", "by", "can", "change", "could", "create", "do", "does", "for", "from", "get",
    "give", "hello", "here", "hi", "how", "i", "in", "into", "is", "it", "its", "just", "list",
    "make", "me", "more", "my", "new", "no", "not", "now", "of", "ok", "okay", "on", "or", "our",
    "page", "please", "right", "set", "show", "so", "some", "still", "than", "thank", "thanks",
    "that", "the", "their", "them", "then", "there", "these", "this", "those", "to", "up", "us",
    "was", "we", "were", "what", "when", "where", "which", "who", "why", "will", "with", "would",
    "yes", "you", "your",
];

/// The words of a question: two characters or more, lower-cased without diacritics ("Banská"
/// finds "banska"), no function word, each once.
pub fn words(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(jc_core::ngsi_catalog::fold)
        .filter(|w| !FILLER.contains(&w.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The words of a field, split at punctuation and at camel case (`BikeHireDockingStation`).
fn field_words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut after_lower = false;
    for c in text.chars() {
        if (!c.is_alphanumeric() || (c.is_uppercase() && after_lower)) && !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        }
        after_lower = c.is_lowercase() || c.is_numeric();
    }
    if !word.is_empty() {
        out.push(word);
    }
    out.iter().map(|w| jc_core::ngsi_catalog::fold(w)).collect()
}

/// Every string leaf of a value, joined: a title in four languages is four strings.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(text_of).collect::<Vec<_>>().join(" "),
        Value::Object(map) => map.values().map(text_of).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// A `contextSpaceRef` is a name or an object naming one.
pub(crate) fn ref_name(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(map) => map.get("name").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn space_of(env: &ResourceEnvelope) -> String {
    if env.kind == "ContextSpace" {
        return env.metadata.name.clone();
    }
    ref_name(&env.spec["contextSpaceRef"])
        .or_else(|| env.metadata.labels.get("joinedcontext.com/space").cloned())
        .unwrap_or_default()
}

/// The fields a word can hit, named as `matchReason` names them.
fn fields(env: &ResourceEnvelope) -> Vec<(&'static str, String)> {
    let meta = serde_json::to_value(&env.metadata).unwrap_or(Value::Null);
    let labels: String = env
        .metadata
        .labels
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = vec![
        ("name", env.metadata.name.clone()),
        ("title", text_of(&meta["title"])),
        ("description", text_of(&meta["description"])),
        ("labels", labels),
    ];
    if env.kind == "Endpoint" {
        out.push(("slug", text_of(&env.spec["slug"])));
    }
    if env.kind == "DataModel" {
        out.push(("classes", text_of(&env.spec["classes"])));
    }
    if env.kind != "ContextSpace" {
        out.push(("space", space_of(env)));
    }
    out
}

/// How many words hit, and the fields they hit.
fn score(fields: &[(&'static str, String)], words: &[String]) -> (usize, Vec<String>) {
    let split: Vec<(&str, Vec<String>)> = fields
        .iter()
        .map(|(name, text)| (*name, field_words(text)))
        .collect();
    // A word matches where a word of the field begins with it: "bike" finds "bikes", "the"
    // does not find "weather". A word of five letters or more also matches inside a compound:
    // "bystrica" finds "banskabystrica" (T-6104).
    let matches = |field: &[String], word: &str| {
        field
            .iter()
            .any(|f| f.starts_with(word) || (word.chars().count() >= 5 && f.contains(word)))
    };
    let hits = words
        .iter()
        .filter(|w| split.iter().any(|(_, field)| matches(field, w)))
        .count();
    let reasons = split
        .iter()
        .filter(|(_, field)| words.iter().any(|w| matches(field, w)))
        .map(|(name, _)| (*name).to_owned())
        .collect();
    (hits, reasons)
}

/// Whether the endpoint's audience admits a signed-in member of `project` (AG-58). The data's
/// own grants are the gateway's decision (EP-55); this says whether the door is open at all.
pub(crate) fn endpoint_access(spec: &Value, project: &str) -> Access {
    match spec["audience"].as_str().unwrap_or("project-list") {
        "public" => Access::allowed("audience public"),
        "organization" => Access::allowed("audience organization: every signed-in member"),
        "project-list" => {
            let named = spec["allowedProjects"]
                .as_array()
                .is_some_and(|list| list.iter().any(|p| p.as_str() == Some(project)));
            if named {
                Access::allowed(format!("audience project-list names {project}"))
            } else {
                Access::restricted(format!("audience project-list does not name {project}"))
            }
        }
        other => Access::restricted(format!("audience {other}")),
    }
}

/// The Endpoints of other projects that `project`'s `SharedSpaceReference`s point at and whose
/// audience admits `project`, each with the reference that reaches it (EP-15, T-6104). A
/// reference names its Endpoint by project and name, or by the slug the loader resolved that to
/// (EP-77). An Endpoint whose audience does not admit the project is never returned: the
/// reference alone opens nothing.
pub(crate) fn shared_endpoints(
    mirror: &crate::store::Mirror,
    project: &str,
) -> Vec<(ResourceEnvelope, ResourceEnvelope)> {
    mirror
        .list(project, "SharedSpaceReference", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|reference| {
            let target = &reference.spec["endpointRef"];
            let endpoint = match (
                target["project"].as_str(),
                target["name"].as_str(),
                reference.spec["endpointSlug"].as_str(),
            ) {
                (Some(source), Some(name), _) => mirror.get(source, "Endpoint", name),
                (_, _, Some(slug)) if !slug.is_empty() => {
                    mirror.find(|env| env.kind == "Endpoint" && env.spec["slug"] == slug)
                }
                _ => None,
            }?;
            let foreign = endpoint.metadata.namespace.as_deref() != Some(project);
            (foreign && endpoint_access(&endpoint.spec, project).is_allowed())
                .then_some((reference, endpoint))
        })
        .collect()
}

pub(crate) fn title_of(env: &ResourceEnvelope) -> Option<String> {
    let meta = serde_json::to_value(&env.metadata).unwrap_or(Value::Null);
    let title = &meta["title"];
    title["en"]
        .as_str()
        .or_else(|| title.as_str())
        .or_else(|| {
            title
                .as_object()
                .and_then(|m| m.values().find_map(Value::as_str))
        })
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
}

/// The endpoint each pipeline of the project writes through: the last segment of the
/// `targetEndpoint` URN, or the whole value when it is a bare name.
fn feeders(state: &AppState, project: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for env in state
        .mirror
        .list(project, "Pipeline", &ListOptions::default())
        .items
    {
        for target in crate::resource::pipeline_targets(&env.spec) {
            let endpoint = target.rsplit(':').next().unwrap_or(target).to_owned();
            out.entry(endpoint).or_insert(env.metadata.name.clone());
        }
    }
    out
}

/// The models that match `words`, each with the verdict of its space's door.
fn model_items(
    models: &[ResourceEnvelope],
    project: &str,
    space_access: &BTreeMap<String, Access>,
    words: &[String],
    items: &mut Vec<CatalogItem>,
) {
    for env in models {
        let space = space_of(env);
        let access = space_access
            .get(&space)
            .cloned()
            .unwrap_or_else(|| Access::restricted("no endpoint of the model's space admits you"));
        let (hits, reasons) = score(&fields(env), words);
        if hits == 0 {
            continue;
        }
        items.push(item(env, project, space, hits, reasons, access));
    }
}

/// The Endpoints other projects share with this one that match `words`, as far as their
/// audience admits the project; the reference is the caller's to read like any manifest of the
/// project, and an Endpoint already in `items` stays there once (T-6104).
fn shared_items(
    state: &AppState,
    project: &str,
    effective: &crate::permissions::Effective,
    words: &[String],
    items: &mut Vec<CatalogItem>,
) {
    for (reference, env) in shared_endpoints(&state.mirror, project) {
        if !effective.may_read_envelope("SharedSpaceReference", &reference)
            || items
                .iter()
                .any(|i| i.kind == "Endpoint" && i.name == env.metadata.name)
        {
            continue;
        }
        let mut searched = fields(&env);
        searched.push(("alias", text_of(&reference.spec["alias"])));
        let (hits, mut reasons) = score(&searched, words);
        if hits == 0 {
            continue;
        }
        let owner = env.metadata.namespace.clone().unwrap_or_default();
        reasons.push(format!("shared with this project by {owner}"));
        let access = endpoint_access(&env.spec, project);
        items.push(item(&env, &owner, space_of(&env), hits, reasons, access));
    }
}

/// The search over the mirror, with the freshness read from the runner for the endpoints that
/// are returned and open to the caller.
///
/// Searched for one reader (T-5698): a space, Endpoint or model `effective` does not read is
/// neither a hit nor a door that opens a space.
pub async fn search(
    state: &AppState,
    project: &str,
    effective: &crate::permissions::Effective,
    q: &str,
    scope: Option<&str>,
) -> Catalog {
    let words = words(q);
    let catalog = Catalog {
        q: q.to_owned(),
        items: Vec::new(),
    };
    if words.is_empty() {
        return catalog;
    }
    let opts = ListOptions::default();
    let of = |kind: &str| -> Vec<ResourceEnvelope> {
        state
            .mirror
            .list(project, kind, &opts)
            .items
            .into_iter()
            .filter(|env| effective.may_read_envelope(kind, env))
            .collect()
    };
    let endpoints = of("Endpoint");
    let spaces = of("ContextSpace");
    let models = of("DataModel");

    // Which spaces an endpoint opens, so a space and a model inherit the verdict of a door.
    let mut space_access: BTreeMap<String, Access> = BTreeMap::new();
    let mut matched_endpoints_of_space: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut items: Vec<CatalogItem> = Vec::new();

    for env in &endpoints {
        let access = endpoint_access(&env.spec, project);
        let space = space_of(env);
        if access.is_allowed() {
            space_access.entry(space.clone()).or_insert_with(|| {
                Access::allowed(format!("endpoint {} admits you", env.metadata.name))
            });
        }
        let (hits, reasons) = score(&fields(env), &words);
        if hits == 0 {
            continue;
        }
        matched_endpoints_of_space
            .entry(space.clone())
            .or_default()
            .push(env.metadata.name.clone());
        items.push(item(env, project, space, hits, reasons, access));
    }
    shared_items(state, project, effective, &words, &mut items);
    for env in &spaces {
        let name = env.metadata.name.clone();
        let access = space_access
            .get(&name)
            .cloned()
            .unwrap_or_else(|| Access::restricted("no endpoint of the space admits you"));
        let (mut hits, mut reasons) = score(&fields(env), &words);
        if let Some(through) = matched_endpoints_of_space.get(&name) {
            hits = hits.max(1);
            reasons.extend(through.iter().map(|e| format!("endpoint {e}")));
        }
        if hits == 0 {
            continue;
        }
        items.push(item(env, project, name, hits, reasons, access));
    }
    model_items(&models, project, &space_access, &words, &mut items);

    if let Some(scope) = scope {
        items.retain(|i| i.kind == scope);
    }
    let rank = |kind: &str| KINDS.iter().position(|k| *k == kind).unwrap_or(KINDS.len());
    items.sort_by(|a, b| {
        b.hits
            .cmp(&a.hits)
            .then_with(|| rank(&a.kind).cmp(&rank(&b.kind)))
            .then_with(|| a.name.cmp(&b.name))
    });
    items.truncate(MAX_ITEMS);

    // Freshness, read in parallel for the endpoints a pipeline feeds; a runner that does not
    // answer leaves `null`, never a guess (AG-58).
    let feeders = feeders(state, project);
    let mut reads = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if item.kind != "Endpoint" || !item.access.is_allowed() || item.owner != project {
            continue;
        }
        let Some(pipeline) = feeders.get(&item.name).cloned() else {
            continue;
        };
        let state = state.clone();
        let project = project.to_owned();
        reads.push(tokio::spawn(async move {
            let metrics = metrics_for(&state, &project, &pipeline).await.ok()?;
            Some((
                index,
                Freshness {
                    pipeline,
                    scraped_at: metrics.scraped_at,
                    received: metrics.received,
                    errors: metrics.errors,
                },
            ))
        }));
    }
    for read in reads {
        if let Ok(Some((index, freshness))) = read.await {
            items[index].freshness = Some(freshness);
        }
    }
    Catalog {
        q: q.to_owned(),
        items,
    }
}

fn item(
    env: &ResourceEnvelope,
    project: &str,
    space: String,
    hits: usize,
    reasons: Vec<String>,
    access: Access,
) -> CatalogItem {
    // A restricted item shows its name and kind and nothing else (AG-58).
    let open = access.is_allowed();
    CatalogItem {
        kind: env.kind.clone(),
        name: env.metadata.name.clone(),
        space,
        owner: project.to_owned(),
        title: if open { title_of(env) } else { None },
        endpoint_slug: if open && env.kind == "Endpoint" {
            env.spec["slug"].as_str().map(str::to_owned)
        } else {
            None
        },
        match_reason: reasons,
        access,
        freshness: None,
        hits,
    }
}

/// The organization's domain, from the installation's `Organization` manifest (PF-121: the one
/// the gateway binds Policy URNs on), else the first of the repository that names one, else the
/// installation's `JC_PORTAL_ORG_DOMAIN`; the project name stands in when none is known, so a
/// draft still renders (an entity URN needs a dotted domain, so that draft is red).
pub fn org_domain(state: &AppState, fallback: &str) -> String {
    let domain_of =
        |env: crate::resource::ResourceEnvelope| env.spec["domain"].as_str().map(str::to_owned);
    crate::permissions::installation_organization_manifest(&state.mirror)
        .and_then(domain_of)
        .or_else(|| {
            state
                .mirror
                .list(
                    crate::api::blueprints::ORG_NAMESPACE,
                    "Organization",
                    &ListOptions::default(),
                )
                .items
                .into_iter()
                .find_map(domain_of)
        })
        .or_else(|| {
            state
                .config
                .app_settings
                .as_ref()
                .map(|settings| settings.org_domain.clone())
        })
        .unwrap_or_else(|| fallback.to_owned())
}

/// The domain of the body that publishes `project`'s data: the Project's `spec.domain`, else
/// the organization's ([`org_domain`]) (PF-108). It names who publishes, as the `did:web`
/// assigner of the project's App grants, and nothing derives a right from it (ADR-N-041).
pub fn publisher_domain(state: &AppState, project: &str) -> String {
    state
        .mirror
        .get(crate::api::blueprints::ORG_NAMESPACE, "Project", project)
        .and_then(|env| env.spec["domain"].as_str().map(str::to_owned))
        .filter(|domain| !domain.is_empty())
        .unwrap_or_else(|| org_domain(state, project))
}

/// Executes an endpoint proposal rendering and permission check (EP-72, API/01 §19).
pub async fn execute_propose_endpoint(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    params: share::ProposeEndpoint,
) -> Result<share::Proposal, ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let proposal = share::render(project, &params, &declared_groups(state, project))
        .map_err(ApiError::BadRequest)?;
    crate::permissions::for_request(state, identity, project).check(
        "Endpoint",
        Verb::Propose,
        Some(&proposal.endpoint),
    )?;
    // After the permission check: the refusal names the space model's classes.
    let classes =
        crate::ops::endpoint_reach::space_classes(state, project, params.context_space.trim());
    share::check_types(&params, &classes).map_err(ApiError::BadRequest)?;
    Ok(proposal)
}

/// The share request rendered, not written (EP-72, API/01 §19): the manifests the person will
/// submit, refused for a caller who may not propose an Endpoint here (PF-50).
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/assistant/propose-endpoint",
    summary = "Propose Endpoint",
    description = "Renders an Endpoint and its draft Policy manifests from a request to share data.",
    tag = "assistant",
    params(("project" = String, Path, description = "Project slug")),
    request_body(
        content = Object,
        description = "What to share and with whom: `contextSpace`, `name`, and optionally \
                       `title`, `audience`, `allowedProjects`, `representations`, \
                       `hiddenAttributes`, `entityTypes`, `rateLimits`, and `access` (`read`, \
                       `update`, `full`) for an endpoint Build an app proposes inline (AP-132), \
                       or `create` for a public form, with the fields it asks for in \
                       `writeAttributes` and `writeRelationships`, its daily count in `createsPerDay` \
                       and the sites that may embed it in `embedOrigins` (T-3172, T-3177, EP-101). \
                       API/04.",
        content_type = "application/json",
        example = json!({
            "contextSpace": "mobility",
            "name": "bikes-regional-transport",
            "title": "City bikes for the regional transport team",
            "allowedProjects": ["regional-transport"],
            "hiddenAttributes": ["maintenanceNote"],
            "entityTypes": ["BikeHireDockingStation"]
        })
    ),
    responses(
        (status = 200, description = "The rendering, written nowhere: `lane`, `slug`, \
                                      `endpoint`, `policies`, `groups` and the `prefill` the \
                                      endpoint form opens with", body = Object),
        (status = 400, description = "A name, a space, an audience, a representation or an \
                                      attribute the platform does not take", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No grant proposes an Endpoint in this project", body = ProblemDetails),
        (status = 404, description = "No such project for this caller", body = ProblemDetails)
    )
)]
pub async fn propose_endpoint(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(params): Json<share::ProposeEndpoint>,
) -> Result<Json<share::Proposal>, ApiError> {
    execute_propose_endpoint(&user.0.identity, &state, &project, params)
        .await
        .map(Json)
}

/// Request payload for starting or continuing an assistant conversation.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartConversation {
    /// Required unless `path` is given (API/04 §8).
    #[serde(default)]
    pub message: String,
    /// The path the person picked in the empty assistant (AG-87, ADR-N-032).
    #[serde(default)]
    #[schema(inline)]
    pub path: Option<crate::agents::paths::Path>,
    /// Where the person is standing when they ask (UI-61, AG-77): the form that is open, the draft
    /// it edits and the field they were last in. Values never travel here; the assistant reads the
    /// person's own draft with `jc_draft_get` under their own grants.
    #[serde(default)]
    pub form_context: Option<FormContextRequest>,
    /// The page the person is on (T-2763, API/04 §"Start or Continue"): its route, read against
    /// the Portal's page table, names only.
    #[serde(default)]
    pub page_context: Option<PageContextRequest>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub continues: Option<String>,
    /// The endpoints the person chose, zero to five, which the assistant may query (AG-75).
    #[serde(default)]
    pub endpoint_names: Vec<String>,
    /// The capabilities the person chose (AG-92, T-2718): they only narrow.
    #[serde(default)]
    pub access: Option<crate::agents::capabilities::Capabilities>,
}

/// The form the question was asked from, as the browser sends it (API/04 §"Start or Continue").
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormContextRequest {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub field: Option<String>,
}

/// The page the question was asked from, as the browser sends it: the route only.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageContextRequest {
    pub route: String,
}

/// The page a request names, read against the Portal's page table; `400` when it is none.
pub(crate) fn page_context(
    asked: Option<&PageContextRequest>,
    project: &str,
) -> Result<Option<crate::agents::page::PageContext>, ApiError> {
    asked
        .map(|asked| crate::agents::page::parse(&asked.route, project))
        .transpose()
        .map_err(ApiError::BadRequest)
}

/// The longest field path a form may name: deeper than any kind has, shorter than a payload.
const MAX_FIELD_PATH: usize = 200;

/// What the request said about the open form, refused when it is not a form the Portal could have.
///
/// The kind must be one the platform knows, the draft name a DNS-1123 label like every manifest
/// name (MF-02), and the field a dotted path of plain segments — `spec.source.query.q`,
/// `spec.pages[0].layers`. A prompt is built from this, so nothing else is let through.
fn form_context(request: &StartConversation) -> Result<oneshot::FormContext, ApiError> {
    let Some(asked) = request.form_context.as_ref() else {
        return Ok(oneshot::FormContext::default());
    };
    if let Some(kind) = &asked.kind {
        if !crate::resource::kinds().any(|known| known.kind == kind) {
            return Err(ApiError::BadRequest(format!(
                "'{kind}' is not a kind this platform has, so no form of it is open"
            )));
        }
    }
    if let Some(name) = &asked.name {
        if !crate::resource::is_dns1123(name) {
            return Err(ApiError::BadRequest(format!(
                "'{name}' is not a draft name (lowercase letters, digits and dashes)"
            )));
        }
    }
    if let Some(field) = &asked.field {
        let shaped = field.len() <= MAX_FIELD_PATH
            && !field.is_empty()
            && field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '[' | ']'));
        if !shaped {
            return Err(ApiError::BadRequest(
                "field must be a dotted path of at most 200 characters".into(),
            ));
        }
    }
    Ok(oneshot::FormContext {
        kind: asked.kind.clone(),
        name: asked.name.clone(),
        field: asked.field.clone(),
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/assistant/conversations",
    summary = "Start A Conversation",
    description = "Starts a conversation with the assistant on the caller's first message and answers the queued run. Needs `propose` on any kind in the project; a path also needs `propose` on the kind it ends by proposing; the assistant reads and drafts as the caller and proposes nothing on its own.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("X-JC-Run-Origin" = Option<String>, Header, description = "`journey` marks a run of the Portal's own live journeys (AG-93); refused with 403 for anyone but the configured journey users, and beside a bearer token"),
    ),
    request_body(
        content = StartConversation,
        example = json!({ "message": "Which endpoints publish air quality?" })
    ),
    responses(
        (status = 202, description = "The conversation run, queued", body = CreatedRun),
        (status = 400, description = "Invalid request or invalid continuation", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No role grants proposing anything here, none grants the kind the path proposes, or `X-JC-Run-Origin` from anyone but the Portal's own live journeys", body = ProblemDetails),
        (status = 503, description = "No agent runner, or no such profile", body = ProblemDetails)
    )
)]
pub async fn start_conversation(
    user: CurrentUser,
    origin: crate::agents::run::RunOrigin,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: axum::http::HeaderMap,
    Json(request): Json<StartConversation>,
) -> Result<(StatusCode, Json<CreatedRun>), ApiError> {
    let settings = agent_settings(&state)?;

    let grants = crate::permissions::for_request(&state, &user.0.identity, &project);
    grants.check_converses(&project)?;
    if let Some(hidden) = request.path.and_then(|path| path.hidden_by(&state.config)) {
        return Err(ApiError::BadRequest(hidden));
    }
    // A path ends by proposing its kind; a person who may not cannot take it (AG-87, UI-44).
    if let Some(kind) = request.path.and_then(crate::agents::paths::Path::proposes) {
        grants.check(kind, Verb::Propose, None)?;
    }
    if let Some(access) = &request.access {
        access.check().map_err(ApiError::BadRequest)?;
        if let Some(path) = request.path.filter(|path| !access.allows_path(*path)) {
            return Err(ApiError::Denied(access.path_refusal(path)));
        }
    }

    if request.message.trim().is_empty() && request.path.is_none() {
        return Err(ApiError::BadRequest(
            "message must not be empty unless a path is given".into(),
        ));
    }
    if request.message.chars().count() > MAX_PROMPT_CHARS {
        return Err(ApiError::BadRequest(format!(
            "message is longer than {MAX_PROMPT_CHARS} characters"
        )));
    }
    // A conversation is a run like any other and counts against the same day (PF-74, T-1403).
    crate::api::agent_runs::within_runs_per_day(&state, &project).await?;

    let form = form_context(&request)?;
    let page = page_context(request.page_context.as_ref(), &project)?;

    let profile_name = request
        .profile
        .clone()
        .unwrap_or_else(|| conversation_profile(&state.mirror));
    let profile = Profile::load(&state.mirror, &profile_name)?;

    let mut run_endpoints = if request.endpoint_names.is_empty() {
        Vec::new()
    } else {
        let names = crate::agents::endpoints::requested(None, &request.endpoint_names)
            .map_err(ApiError::BadRequest)?;
        crate::agents::endpoints::resolve_readable(&state.mirror, &project, &names)?
    };
    if let Some(parent_id) = &request.continues {
        let parent = state
            .agents
            .get_run(parent_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| ApiError::BadRequest(format!("run '{parent_id}' does not exist")))?;
        if parent.project != project {
            return Err(ApiError::BadRequest(format!(
                "run '{parent_id}' does not belong to project '{project}'"
            )));
        }
        if parent.kind != "conversation" {
            return Err(ApiError::BadRequest(format!(
                "run '{parent_id}' is not a conversation run"
            )));
        }
        let is_approver =
            crate::api::changes::may_approve_anything(&state, &user.0.identity, &project).is_ok();
        if !is_approver && parent.created_by != user.0.identity.username {
            return Err(ApiError::BadRequest(format!(
                "run '{parent_id}' is not visible to you"
            )));
        }
        // A continuation that names no endpoints keeps the ones it continues, resolved again
        // for this person and held to this profile like named ones (AG-70, JC1-24).
        if run_endpoints.is_empty() {
            let names: Vec<String> = crate::agents::endpoints::of_run(&parent)
                .into_iter()
                .map(|endpoint| endpoint.name)
                .collect();
            if !names.is_empty() {
                run_endpoints =
                    crate::agents::endpoints::resolve_readable(&state.mirror, &project, &names)?;
            }
        }
    }
    // AG-70: a profile that lists endpoints lets the assistant read only those.
    if let Some(refused) = run_endpoints
        .iter()
        .find(|endpoint| !profile.access.grants_endpoint(&endpoint.name, false))
    {
        return Err(ApiError::Denied(format!(
            "agent profile '{}' does not grant reading endpoint '{}' (AG-70)",
            profile.name, refused.name
        )));
    }

    let id = mint_run_id();
    let (ticket, ticket_hash) = mint_ticket();
    let created_at = now_rfc3339();
    let expires_at = expiry(settings.run_ttl_secs);

    let run = AgentRun {
        id: id.clone(),
        project: project.clone(),
        app_name: String::new(),
        title: None,
        endpoint_name: run_endpoints
            .first()
            .map(|e| e.name.clone())
            .unwrap_or_default(),
        endpoint_slug: run_endpoints
            .first()
            .map(|e| e.slug.clone())
            .unwrap_or_default(),
        endpoints: serde_json::to_value(&run_endpoints).unwrap_or_else(|_| serde_json::json!([])),
        profile: profile.name.clone(),
        kind: "conversation".to_owned(),
        unattended: false,
        continues: request.continues.clone(),
        app_class: "ui".to_owned(),
        visibility: "private".to_owned(),
        prompt: request.message.clone(),
        prompt_digest: digest_prompt(&request.message),
        data_needs: serde_json::json!([]),
        allows_write: false,
        branch: String::new(),
        path_prefix: String::new(),
        status: AgentRunStatus::Queued.as_str().to_owned(),
        ticket_hash,
        workspace: None,
        merge_request: None,
        change_id: None,
        source_url: None,
        mirror_url: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: serde_json::json!({}),
        steps: 0,
        tokens_used: 0,
        created_by: user.0.identity.username.clone(),
        origin: origin.as_str().to_owned(),
        starter: serde_json::to_value(&user.0.identity).unwrap_or(serde_json::Value::Null),
        created_at,
        started_at: None,
        finished_at: None,
        expires_at,
        error: None,
    };

    state.agents.create_run(&run).await.map_err(unavailable)?;
    // The person's identity goes to the proxy before anything the run does can read through it
    // (ADR-N-038 §3.1, AG-94); the Portal keeps no copy.
    crate::agents::identity::hand_over(
        state.oidc.as_deref(),
        &settings.proxy_base,
        &id,
        crate::agents::identity::persons_token(&headers, state.config.trust_edge_token),
    )
    .await;
    publish_event(
        &state,
        &id,
        "status",
        status_payload(AgentRunStatus::Queued),
    )
    .await?;
    // The first question is a line of the chat like every later one, so the panel shows it and
    // a continuation reads it back (AG-68). The driver subscribes after this, so it answers the
    // prompt once, not this event as well. A path picked without words has no line to show.
    if !run.prompt.trim().is_empty() {
        publish_event(
            &state,
            &id,
            "message",
            serde_json::json!({ "text": run.prompt, "sentBy": run.created_by }),
        )
        .await?;
    }

    oneshot::spawn(
        state.clone(),
        &run,
        &user.0.identity,
        &ticket,
        &profile,
        settings,
        oneshot::Opening {
            form,
            path: request.path,
            page,
            access: request.access,
        },
    );

    Ok((StatusCode::ACCEPTED, Json(CreatedRun { run, ticket: None })))
}

/// One operation of the registry as a run the caller starts would meet it (AG-70, UI-56).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationAccess {
    pub name: String,
    pub read_only: bool,
    /// The profile's half: its access block, or the read-only default without one.
    pub profile: bool,
    /// The caller's own permission in the project (PF-50).
    pub person: bool,
    /// The half that refuses; `null` when neither does.
    pub reason: Option<String>,
}

/// An `AgentProfile` and what it lets a run of the caller do.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAccess {
    pub name: String,
    pub title: Option<String>,
    pub role: String,
    /// `spec.access` as written; `null` when the profile has none.
    #[schema(value_type = Option<Object>)]
    pub access: Option<Value>,
    pub egress_hosts: Vec<String>,
    pub operations: Vec<OperationAccess>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentAccessList {
    pub items: Vec<ProfileAccess>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/assistant/access",
    summary = "Read Assistant Access",
    description = "Every agent profile of the organization with what it lets the assistant reach here, intersected with the caller's own grants. It changes nothing.",
    tag = "agents",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "Every agent profile with the caller's effective access", body = AgentAccessList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not a project name", body = ProblemDetails)
    )
)]
pub async fn get_access(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<AgentAccessList>, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let identity = &user.0.identity;
    // The person's side of each operation is the same for every profile: asked once.
    let permitted: Vec<Result<(), String>> = crate::ops::registry()
        .iter()
        .map(|op| crate::ops::permitted(op, identity, &state, &project).map_err(|e| e.to_string()))
        .collect();
    let mut items: Vec<ProfileAccess> = state
        .mirror
        .list(
            crate::api::blueprints::ORG_NAMESPACE,
            "AgentProfile",
            &ListOptions::default(),
        )
        .items
        .iter()
        .map(|env| {
            let access = access::Access::from_spec(&env.spec);
            let operations = crate::ops::registry()
                .iter()
                .zip(&permitted)
                .map(|(op, person)| {
                    let profile = access.names(op);
                    let reason = if profile {
                        person.as_ref().err().cloned()
                    } else {
                        Some(refusal(op.name))
                    };
                    OperationAccess {
                        name: op.name.to_owned(),
                        read_only: op.annotations.read_only_hint,
                        profile,
                        person: person.is_ok(),
                        reason,
                    }
                })
                .collect();
            ProfileAccess {
                name: env.metadata.name.clone(),
                title: title_of(env),
                role: env.spec["role"].as_str().unwrap_or_default().to_owned(),
                access: env.spec.get("access").filter(|a| a.is_object()).cloned(),
                egress_hosts: env
                    .spec
                    .pointer("/egress/allowedHosts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                operations,
            }
        })
        .collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(AgentAccessList { items }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/assistant/access", get(get_access))
        .route(
            "/projects/{project}/assistant/propose-endpoint",
            post(propose_endpoint),
        )
        .route(
            "/projects/{project}/assistant/conversations",
            post(start_conversation),
        )
}

/// The `Group` names the repository declares for a project, so a share never proposes one twice.
pub(crate) fn declared_groups(state: &AppState, project: &str) -> Vec<String> {
    state
        .mirror
        .list(project, "Group", &crate::store::ListOptions::default())
        .items
        .into_iter()
        .map(|group| group.metadata.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        endpoint_access, field_words, form_context, org_domain, score, words, FormContextRequest,
        StartConversation,
    };
    use serde_json::json;

    /// T-0964: `field_words` and `ops::feed_shape::split_camel` look alike and are not the same
    /// function. This pins what this one does, so a later de-slop that merges them has to see
    /// what it would change: the assistant's search index is built from these words.
    #[test]
    fn a_field_splits_at_punctuation_and_at_camel_case() {
        assert_eq!(
            field_words("BikeHireDockingStation"),
            vec!["bike", "hire", "docking", "station"]
        );
        assert_eq!(
            field_words("available_bike-number"),
            vec!["available", "bike", "number"]
        );
        assert_eq!(
            field_words("pm10"),
            vec!["pm10"],
            "a digit continues the word"
        );
        assert_eq!(field_words("PM10Sensor"), vec!["pm10", "sensor"]);
        // A one-letter word is kept here and dropped from a question (`words`): a field that is
        // one letter is still the field's name.
        assert_eq!(field_words("A sensor"), vec!["a", "sensor"]);
        assert_eq!(field_words("Kvalita ovzdušia"), vec!["kvalita", "ovzdusia"]);
        assert_eq!(field_words(""), Vec::<String>::new());
        assert_eq!(field_words("---"), Vec::<String>::new());
    }

    #[test]
    fn the_org_domain_is_the_installations_before_the_project_name() {
        let mut config = crate::config::Config::for_tests();
        assert_eq!(
            org_domain(
                &crate::state::AppState::new(config.clone(), None),
                "helsinki"
            ),
            "helsinki"
        );
        config.app_settings = Some(crate::apps::reconciler::Settings {
            host: "portal.example.org".into(),
            apex: "example.org".into(),
            gateway_url: Some("http://context-gateway.jc.svc.cluster.local:8080".into()),
            service_url: Some("http://portal.joinedcontext.svc.cluster.local:8080".into()),
            namespace: "apps".into(),
            org_domain: "hel.fi".into(),
            apisix_namespace: "apisix".into(),
            image_repository: None,
            pull_secret: None,
            basemap_base: None,
            release: None,
            service_account: None,
        });
        assert_eq!(
            org_domain(&crate::state::AppState::new(config, None), "helsinki"),
            "hel.fi"
        );
    }

    /// T-6435: the gateway binds Policy URNs on the installation Organization, so the first by
    /// name must not stand in for it.
    #[test]
    fn the_org_domain_is_the_installation_organizations_before_the_first_by_name() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let organization = |name: &str, spec: serde_json::Value| {
            serde_json::from_value(json!({
                "apiVersion": crate::resource::API_VERSION,
                "kind": "Organization",
                "metadata": { "name": name, "namespace": crate::api::blueprints::ORG_NAMESPACE },
                "spec": spec,
            }))
            .expect("envelope")
        };
        state.mirror.upsert(organization(
            "a-other",
            json!({ "domain": "barcelona.cat" }),
        ));
        state
            .mirror
            .upsert(organization("city", json!({ "domain": "hel.fi" })));
        assert_eq!(
            org_domain(&state, ""),
            "barcelona.cat",
            "no claim: as before"
        );
        state.mirror.upsert(organization(
            "city",
            json!({ "domain": "hel.fi", "installation": true }),
        ));
        assert_eq!(org_domain(&state, ""), "hel.fi");
    }

    /// T-2763: a follow-up that names nothing is no search: "and right now??" searched the catalog.
    #[test]
    fn a_turn_that_names_nothing_has_no_words() {
        for said in [
            "and right now??",
            "what is on the page??",
            "ok, thanks",
            "yes please, here",
        ] {
            assert!(words(said).is_empty(), "{said}: {:?}", words(said));
        }
        assert_eq!(words("and the bikes right now?"), vec!["bikes"]);
    }

    fn envelope(
        kind: &str,
        project: &str,
        name: &str,
        spec: serde_json::Value,
    ) -> crate::resource::ResourceEnvelope {
        let mut meta =
            serde_json::to_value(crate::resource::ObjectMeta::new(name, project)).expect("meta");
        meta["title"] = json!({ "sk": format!("{name} title") });
        crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: kind.into(),
            metadata: serde_json::from_value(meta).expect("meta back"),
            spec,
            status: None,
        }
    }

    fn person(groups: &[&str]) -> crate::auth::session::Identity {
        crate::auth::session::Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
            second_factor: false,
        }
    }

    /// S-37 (T-6530): an Endpoint over a type the space's data model lacks is refused before any
    /// draft, naming the type and the model's classes; a type the model has is drafted.
    #[tokio::test]
    async fn an_endpoint_over_a_class_the_space_model_lacks_is_refused_naming_it() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(envelope(
            "ContextSpace",
            "city",
            "doprava",
            json!({ "dataModelRef": { "kind": "DataModel", "name": "bike-counter" } }),
        ));
        state.mirror.upsert(envelope(
            "DataModel",
            "city",
            "bike-counter",
            json!({ "classes": ["BikeCounter"] }),
        ));
        let ask = |types: serde_json::Value| -> crate::agents::share::ProposeEndpoint {
            serde_json::from_value(json!({
                "contextSpace": "doprava", "name": "city-events", "audience": "public",
                "entityTypes": types
            }))
            .expect("params")
        };
        let steward = person(&[state.config.bootstrap_admins.as_str()]);
        let refused =
            super::execute_propose_endpoint(&steward, &state, "city", ask(json!(["Event"])))
                .await
                .expect_err("refused");
        let text = refused.to_string();
        assert!(
            text.contains("'Event' is no class of the data model of space 'doprava'")
                && text.contains("its classes are BikeCounter"),
            "{text}"
        );
        let other =
            super::execute_propose_endpoint(&steward, &state, "city", ask(json!(["BikeCounter"])))
                .await;
        assert!(
            !matches!(&other, Err(crate::error::ApiError::BadRequest(_))),
            "{:?}",
            other.err()
        );
        // A person who may not propose an Endpoint here learns nothing about the model.
        let outsider = super::execute_propose_endpoint(
            &crate::auth::session::Identity {
                subject: "f:1:outsider".into(),
                username: "outsider".into(),
                ..person(&[])
            },
            &state,
            "city",
            ask(json!(["Event"])),
        )
        .await;
        assert!(
            matches!(&outsider, Err(crate::error::ApiError::Denied(_))),
            "{:?}",
            outsider.err()
        );
    }

    /// Helsinki references two Banská Bystrica Endpoints (one admits Helsinki, one names only
    /// Zvolen); a third Endpoint of Banská Bystrica is referenced by nobody.
    fn shared_world() -> crate::state::AppState {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let endpoint = |name: &str, slug: &str, audience: serde_json::Value| {
            let mut spec = json!({ "contextSpaceRef": "banskabystrica-verejne", "slug": slug });
            spec.as_object_mut()
                .expect("object")
                .extend(audience.as_object().expect("object").clone());
            envelope("Endpoint", "banskabystrica", name, spec)
        };
        state.mirror.upsert(endpoint(
            "app-banskabystrica-ovzdusie",
            "ovzdusieslug000000000000000000",
            json!({ "audience": "organization" }),
        ));
        state.mirror.upsert(endpoint(
            "banskabystrica-ovzdusie-zvolen",
            "zvolenslug00000000000000000000",
            json!({ "audience": "project-list", "allowedProjects": ["zvolen"] }),
        ));
        state.mirror.upsert(endpoint(
            "banskabystrica-ovzdusie-private",
            "privateslug0000000000000000000",
            json!({ "audience": "organization" }),
        ));
        state.mirror.upsert(envelope(
            "SharedSpaceReference",
            "helsinki",
            "bb-air",
            json!({ "endpointRef": { "project": "banskabystrica", "name": "app-banskabystrica-ovzdusie" }, "alias": "bb-air" }),
        ));
        state.mirror.upsert(envelope(
            "SharedSpaceReference",
            "helsinki",
            "bb-zvolen",
            json!({ "endpointSlug": "zvolenslug00000000000000000000", "alias": "bb-zvolen" }),
        ));
        state
    }

    /// T-6104: "air quality in Banská Bystrica" from Helsinki found nothing although Helsinki
    /// references the city's air Endpoint. The search folds diacritics and finds a word inside
    /// a compound; an Endpoint whose audience shuts Helsinki out, or that no reference names,
    /// stays out of the answer however wide the caller's rights.
    #[tokio::test]
    async fn the_search_finds_a_shared_endpoint_the_audience_admits_and_nothing_else() {
        let state = shared_world();
        let everything = crate::permissions::effective(
            &state.mirror,
            "boot",
            &person(&["boot"]),
            "helsinki",
            chrono::Utc::now(),
        );
        let found = super::search(
            &state,
            "helsinki",
            &everything,
            "aká je kvalita ovzdušia v Banskej Bystrici? Banská Bystrica",
            None,
        )
        .await;
        let names: Vec<_> = found.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["app-banskabystrica-ovzdusie"]);
        let item = &found.items[0];
        assert_eq!(item.owner, "banskabystrica");
        assert!(item.access.is_allowed());
        assert!(item
            .match_reason
            .iter()
            .any(|r| r.contains("shared with this project")));

        // A person with no grant in Helsinki does not read its references, so nothing opens.
        let nobody = crate::permissions::effective(
            &state.mirror,
            "boot",
            &person(&[]),
            "helsinki",
            chrono::Utc::now(),
        );
        assert!(
            super::search(&state, "helsinki", &nobody, "banska bystrica", None)
                .await
                .items
                .is_empty()
        );

        // A conversation opens the shared Endpoint by name; the shut-out one is not found.
        let resolved = crate::agents::endpoints::resolve_readable(
            &state.mirror,
            "helsinki",
            &["app-banskabystrica-ovzdusie".to_owned()],
        )
        .expect("shared endpoint");
        assert_eq!(resolved[0].slug, "ovzdusieslug000000000000000000");
        for name in [
            "banskabystrica-ovzdusie-zvolen",
            "banskabystrica-ovzdusie-private",
        ] {
            assert!(crate::agents::endpoints::resolve_readable(
                &state.mirror,
                "helsinki",
                &[name.to_owned()]
            )
            .is_err());
        }
        // A build keeps to the project's own Endpoints.
        assert!(crate::agents::endpoints::resolve(
            &state.mirror,
            "helsinki",
            &["app-banskabystrica-ovzdusie".to_owned()]
        )
        .is_err());
    }

    #[test]
    fn diacritics_fold_and_a_long_word_matches_inside_a_compound() {
        let fields = vec![("name", "app-banskabystrica-ovzdusie".to_owned())];
        assert_eq!(score(&fields, &words("Banská Bystrica ovzdušie")).0, 3);
        // A short word matches only at the start of a field word.
        assert_eq!(score(&fields, &words("app air")).0, 1);
    }

    /// T-6104: a title of only spaces is no title; the name stands in.
    #[test]
    fn an_empty_title_is_none() {
        let mut env = envelope("Endpoint", "helsinki", "air", json!({}));
        env.metadata = serde_json::from_value(
            json!({ "name": "air", "namespace": "helsinki", "title": { "en": " " } }),
        )
        .expect("meta");
        assert_eq!(super::title_of(&env), None);
    }

    #[test]
    fn words_are_short_lower_and_unique() {
        assert_eq!(
            words("Where is the Bike  availability, bike?"),
            vec!["availability", "bike"]
        );
        assert!(words("a , !").is_empty());
    }

    #[test]
    fn a_word_matches_the_start_of_a_field_word_and_filler_matches_nothing() {
        let fields = vec![
            ("name", "helsinki-weather".to_owned()),
            ("title", "Helsinki indicators and news".to_owned()),
            (
                "classes",
                "https://smartdatamodels.org/BikeHireDockingStation".to_owned(),
            ),
        ];
        assert_eq!(score(&fields, &words("change the color to black")).0, 0);
        assert_eq!(score(&fields, &words("create a new one")).0, 0);
        assert_eq!(score(&fields, &words("docking weather")).0, 2);
        assert_eq!(score(&fields, &words("news")).1, vec!["title"]);
    }

    #[test]
    fn a_score_counts_words_and_names_fields() {
        let fields = vec![
            ("name", "helsinki-bikes".to_owned()),
            ("title", "City bikes".to_owned()),
            ("description", "Bike stations and availability".to_owned()),
        ];
        let (hits, reasons) = score(&fields, &words("bike availability"));
        assert_eq!(hits, 2);
        assert_eq!(reasons, vec!["name", "title", "description"]);
        assert_eq!(score(&fields, &words("parking")).0, 0);
    }

    #[test]
    fn the_audience_decides_the_verdict() {
        assert!(endpoint_access(&json!({ "audience": "public" }), "helsinki").is_allowed());
        assert!(endpoint_access(&json!({ "audience": "organization" }), "helsinki").is_allowed());
        assert!(endpoint_access(
            &json!({ "audience": "project-list", "allowedProjects": ["helsinki"] }),
            "helsinki"
        )
        .is_allowed());
        let shut = endpoint_access(
            &json!({ "audience": "project-list", "allowedProjects": ["espoo"] }),
            "helsinki",
        );
        assert!(!shut.is_allowed());
        assert!(shut.reason.contains("does not name helsinki"));
        assert!(!endpoint_access(&json!({}), "helsinki").is_allowed());
    }

    /// T-1611, UI-61: a prompt is built from what the request says about the open form, so only a
    /// form the Portal could actually have is let through — and never a value.
    #[test]
    fn a_form_context_is_a_kind_a_draft_and_a_field_path_or_it_is_refused() {
        let asked =
            |kind: Option<&str>, name: Option<&str>, field: Option<&str>| StartConversation {
                path: None,
                message: "what goes here?".to_owned(),
                profile: None,
                continues: None,
                endpoint_names: Vec::new(),
                access: None,
                page_context: None,
                form_context: Some(FormContextRequest {
                    kind: kind.map(str::to_owned),
                    name: name.map(str::to_owned),
                    field: field.map(str::to_owned),
                }),
            };

        let taken = form_context(&asked(
            Some("Pipeline"),
            Some("citybikes-gbfs"),
            Some("spec.source.query.q"),
        ))
        .expect("a pipeline form is a form");
        assert_eq!(taken.kind.as_deref(), Some("Pipeline"));
        assert_eq!(taken.name.as_deref(), Some("citybikes-gbfs"));
        assert_eq!(taken.field.as_deref(), Some("spec.source.query.q"));

        // An index into a list is a path a form really has.
        assert!(form_context(&asked(
            Some("Dashboard"),
            None,
            Some("spec.pages[0].layers")
        ))
        .is_ok());

        // A kind nobody has, a name that is not a manifest name, a field carrying anything else.
        for wrong in [
            asked(Some("Spaceship"), None, None),
            asked(Some("Pipeline"), Some("Citybikes GBFS"), None),
            asked(Some("Pipeline"), None, Some("spec.q; DROP TABLE drafts")),
            asked(Some("Pipeline"), None, Some(&"a".repeat(201))),
            asked(Some("Pipeline"), None, Some("")),
        ] {
            assert!(
                form_context(&wrong).is_err(),
                "a form context the Portal cannot have was accepted"
            );
        }

        // No form named itself: nothing to say, and no error either.
        assert!(form_context(&StartConversation {
            path: None,
            message: "how is the air?".to_owned(),
            profile: None,
            continues: None,
            endpoint_names: Vec::new(),
            access: None,
            form_context: None,
            page_context: None,
        })
        .expect("a question from a page is fine")
        .is_empty());
    }
}
