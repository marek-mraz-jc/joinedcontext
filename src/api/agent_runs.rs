//! Builder runs: the routes a person drives an agent through, and the two the credential proxy
//! calls back on (AG-43…AG-46, AG-52, AP-44, AP-46, AP-51, AP-55).
//!
//! The Portal holds everything a run is: what was asked for, what the agent may reach, how much
//! it may spend, and every line of the conversation. The workspace holds a ticket and nothing
//! else (ADR-N-020), so these routes are the whole of what a run can do to the platform.
//!
//! Two surfaces, one module. The project routes are the Portal's public API and carry a session
//! or a bearer. The `internal/` pair is served on a separate listener the edge does not route
//! (`JC_INTERNAL_BIND`) and is authenticated by the proxy's own token: `runId` on an event comes
//! from the run whose ticket the proxy verified, which is what makes AG-46 hold.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use utoipa::{IntoParams, ToSchema};

use crate::agents::kube;
use crate::agents::needs::{declared_roles, validate_data_needs};
use crate::agents::profile::Profile;
use crate::agents::run::{
    digest_prompt, mint_run_id, mint_ticket, AgentRun, AgentRunEvent, AgentRunStatus, RunOrigin,
};
use crate::agents::store::{now_rfc3339, StatusChangeError, StoreError};
use crate::agents::{kit, oneshot, preview, repository, transpile};
use crate::auth::session::Front;
use crate::auth::CurrentUser;
use crate::config::AgentSettings;
use crate::error::{ApiError, ProblemDetails};
use crate::git::GitError;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// How many runs one list answer carries at most.
const MAX_LIST_LIMIT: i64 = 100;
const DEFAULT_LIST_LIMIT: i64 = 20;
/// Longest prompt a run is started from. A prompt is a paragraph, not a corpus; the brief the
/// agent works from is the manifests and the model, not this field.
pub(crate) const MAX_PROMPT_CHARS: usize = 4_000;
/// Longest instruction a person may send into a live run. Same ceiling as the prompt: it is a
/// sentence of steering, and a workspace reads it with the same budget as everything else.
const MAX_MESSAGE_CHARS: usize = 4_000;
/// Longest runtime error a preview frame may report, and the longest file name it may name.
const MAX_PREVIEW_ERROR_CHARS: usize = 2_000;
const MAX_PREVIEW_FILE_CHARS: usize = 256;
/// The bounds of a preview observation (API/04 §5, SDK-27).
const MAX_OBSERVED_PAGES: usize = 20;
const MAX_PAGE_LABEL_CHARS: usize = 120;
const MAX_PAGE_TEXT_CHARS: usize = 20_000;
const MAX_OBSERVED_ENTRIES: usize = 50;
/// The most level-1 headings one observed page may count.
const MAX_OBSERVED_H1: u32 = 50;
/// How long the workspace's inbox call waits for something new before answering empty. Short
/// enough to sit inside every proxy's read timeout, long enough that an idle agent is not a
/// request per second.
pub(crate) const INBOX_WAIT_SECS: u64 = 25;
/// The kinds a workspace reads from its inbox: what the person answered, and what they said.
pub(crate) const INBOX_KINDS: [&str; 2] = ["answer", "message"];
/// The run one application came out of, on the `App` manifest a publish opens (AP-51).
const AGENT_RUN_ANNOTATION: &str = "joinedcontext.com/agent-run";
/// The digest of the prompt that run was started from, under jc-core's name (MF-08).
use jc_core::envelope::annotations::PROMPT_DIGEST as PROMPT_DIGEST_ANNOTATION;
/// The toolchains a generated application is built with (AP-11), the same pins
/// `Architecture/16` writes into its worked examples.
const RUST_TOOLCHAIN: &str = "1.90";
const NODE_TOOLCHAIN: &str = "22";
/// The SQL quota of a generated `wasm` App's own schema, as the reference server Apps keep (AP-148).
const WASM_SQL_QUOTA_MIB: u32 = 100;

/// The `: keep-alive` comment interval of the event stream, short enough for an edge's read
/// timeout to never see an idle stream.
const KEEP_ALIVE_SECS: u64 = 15;

/// What a person asks for when they start a run (AP-51).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateRunRequest {
    /// Name of the application to build; becomes the `App` manifest's name.
    pub app_name: String,
    /// The one `Endpoint` the application reads through; `endpointNames` for several. Nothing
    /// else is reachable.
    #[serde(default)]
    pub endpoint_name: Option<String>,
    /// The `Endpoint`s the application reads, one to five, the first the primary (AP-44).
    #[serde(default)]
    pub endpoint_names: Vec<String>,
    /// Which `AgentProfile` runs. Defaults to the builder profile the platform ships.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// `ui`, `ui-rust` or `wasm`, as the `App` kind spells them (AP-124, T-3575); `static` and `fullstack` are
    /// read as them for one release, and `ui-node` is refused until it is built.
    pub app_class: String,
    /// Who may reach the published application. `public` is refused (AP-42).
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// What kind of run to execute: application, dashboard, analysis.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Whether the run executes unattended without interactive questions.
    #[serde(default)]
    pub unattended: bool,
    /// What the application should do, in the person's own words.
    pub prompt: String,
    /// The types, attributes and operations the application needs. Checked against what the
    /// endpoint publishes before anything is scheduled (AP-44).
    pub data_needs: Vec<serde_json::Value>,
}

pub(crate) fn default_profile() -> String {
    "app-builder".to_owned()
}

/// The organization's profile for conversations (T-2770): a fast model without reasoning and a
/// run pool of its own, so a build or a test never slows or queues a person's chat.
pub(crate) const CHAT_PROFILE: &str = "chat";

/// The profile a conversation runs on when the request names none: the `chat` profile where the
/// organization has one, else the app-builder's, as every conversation ran before it existed.
pub(crate) fn conversation_profile(mirror: &crate::store::Mirror) -> String {
    match mirror.get(
        crate::api::blueprints::ORG_NAMESPACE,
        "AgentProfile",
        CHAT_PROFILE,
    ) {
        Some(_) => CHAT_PROFILE.to_owned(),
        None => default_profile(),
    }
}

fn default_visibility() -> String {
    "project".to_owned()
}

fn default_kind() -> String {
    "application".to_owned()
}

/// The header the Portal's own live journeys start their runs with (AG-93).
pub const RUN_ORIGIN_HEADER: header::HeaderName =
    header::HeaderName::from_static("x-jc-run-origin");

/// The origin a request asks its run to carry (AG-93). Only the Portal's own live journeys may
/// say `journey`: a browser session of one of the configured journey users. Anyone else who
/// sends the header, a bearer caller (a service, a script, an agent) included, is refused with
/// 403, never quietly counted as a person, so marking a run cannot hide a person's activity.
/// Any value but `journey` is a 400.
pub fn run_origin(
    headers: &HeaderMap,
    front: Front,
    username: &str,
    journey_users: &[String],
) -> Result<RunOrigin, ApiError> {
    let Some(value) = headers.get(&RUN_ORIGIN_HEADER) else {
        return Ok(RunOrigin::Person);
    };
    if value.to_str().ok() != Some("journey") {
        return Err(ApiError::BadRequest(
            "X-JC-Run-Origin must be journey".into(),
        ));
    }
    if front == Front::Bearer || !journey_users.iter().any(|name| name == username) {
        return Err(ApiError::Denied(
            "X-JC-Run-Origin is sent by the Portal's own live journeys only; leave the header out"
                .into(),
        ));
    }
    Ok(RunOrigin::Journey)
}

impl axum::extract::FromRequestParts<AppState> for RunOrigin {
    type Rejection = ApiError;

    /// Who is signed in is read only when the header is there: a request without it is a
    /// person's, and its handler authenticates it as always.
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if !parts.headers.contains_key(&RUN_ORIGIN_HEADER) {
            return Ok(RunOrigin::Person);
        }
        let user = CurrentUser::from_request_parts(parts, state).await?;
        run_origin(
            &parts.headers,
            Front::of(&parts.headers, state.config.trust_edge_token),
            &user.0.identity.username,
            &state.config.journey_users,
        )
    }
}

/// Which origins a list shows (AG-93): the one asked for, else people's runs, and every origin
/// to a journey, so it finds the runs it started.
pub fn listed_origin(asked: Option<&str>, caller: RunOrigin) -> Result<Option<String>, ApiError> {
    match asked {
        None if caller == RunOrigin::Journey => Ok(None),
        None | Some("person") => Ok(Some(RunOrigin::Person.as_str().to_owned())),
        Some("journey") => Ok(Some(RunOrigin::Journey.as_str().to_owned())),
        Some("all") => Ok(None),
        Some(_) => Err(ApiError::BadRequest(
            "origin must be person, journey or all".into(),
        )),
    }
}

/// Query parameters for listing runs.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListRunsQuery {
    pub limit: Option<i64>,
    pub app: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub mine: Option<bool>,
    /// `person` (the default), `journey` or `all` (AG-93).
    pub origin: Option<String>,
}

/// One page of runs, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct RunList {
    pub items: Vec<AgentRun>,
}

/// An answer to one question the agent asked (AG-45).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AnswerRequest {
    pub question_id: String,
    pub answers: serde_json::Value,
}

/// What the proxy relays on behalf of a workspace it has already authenticated.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RelayedEvent {
    pub run_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// What a person says to a run that is already going (AG-45).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageRequest {
    pub text: String,
    /// On a conversation: the endpoints the assistant may query from this message on (AG-75).
    #[serde(default, rename = "endpointNames")]
    pub endpoint_names: Option<Vec<String>>,
    /// On a conversation: the page the person is on as they send it (T-2763).
    #[serde(default, rename = "pageContext")]
    pub page_context: Option<crate::api::assistant::PageContextRequest>,
    /// On a conversation: the capabilities from this message on (AG-92, T-2718).
    #[serde(default)]
    pub access: Option<crate::agents::capabilities::Capabilities>,
    /// On an application run: the `src` of the preview's `jc-select`, `{file}:{line}`; the
    /// editing agent changes that file only for this message (SDK-46).
    #[serde(default)]
    pub scope: Option<String>,
}

/// A runtime error the preview frame posted as `jc-error`, relayed by the page that frames it.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewErrorRequest {
    pub message: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
}

/// What the preview frame saw of one version, page by page, relayed by the page that frames it
/// (SDK-27). Every field is text the frame wrote.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewObservationRequest {
    /// The `v` of the preview URL the frame loaded.
    pub version: u32,
    pub pages: Vec<ObservedPage>,
    #[serde(default)]
    pub failed_requests: Vec<FailedRequest>,
}

/// One page of the application as the frame rendered it.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedPage {
    pub label: String,
    /// The page's visible text.
    pub text: String,
    /// The row counts of its tables.
    #[serde(default)]
    pub rows: Vec<u32>,
    /// How many level-1 headings the page shows (SDK-28, T-3060); an SDK older than T-3060
    /// counts none and sends nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h1: Option<u32>,
}

/// A request of the frame the bridge answered with an error status.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FailedRequest {
    pub path: String,
    pub status: u16,
}

/// Where the workspace reads from, how far it has read, and how long it will wait.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxQuery {
    #[serde(default)]
    pub after: i64,
    /// Seconds to hold the call open when there is nothing new, clamped to `INBOX_WAIT_SECS`.
    /// `0` answers at once, which is what an agent between two steps of its own work wants.
    pub wait: Option<u64>,
}

/// The answers and instructions after `after`, oldest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct Inbox {
    pub items: Vec<RelayedInbound>,
}

/// One line of the inbox. The payload is the person's own words, so a workspace treats it as
/// data: it is an instruction to the agent, never a grant, and nothing here widens `dataNeeds`.
#[derive(Debug, Serialize, ToSchema)]
pub struct RelayedInbound {
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// The sequence number a relayed event was given.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventReceipt {
    pub seq: i64,
}

/// Everything the proxy needs to decide one request, and nothing a workspace may see.
///
/// This is the one place the ticket hash leaves the Portal, and it leaves on the internal
/// listener alone. The budgets are read from the profile at answer time rather than copied into
/// the run row, so lowering a profile's ceiling takes effect on the runs already in flight.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunContext {
    pub id: String,
    pub project: String,
    pub app_name: String,
    pub endpoint_slug: String,
    /// Every endpoint slug of the run, the primary first: what `/v1/data/endpoints/{slug}/…`
    /// may address (Architecture/19 §4).
    pub endpoint_slugs: Vec<String>,
    pub allows_write: bool,
    pub branch: String,
    /// The folder the run writes, as its repository spells it.
    pub path_prefix: String,
    /// The project's own repository in layout 2, the one repository the proxy's forge route
    /// reaches for this run (CC-87, AG-86); absent, the configuration repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub status: String,
    /// The run's kind: whose daily model budget its calls count against (AG-97).
    pub kind: String,
    pub ticket_hash: String,
    pub max_tokens: u64,
    pub allowed_hosts: Vec<String>,
    pub requests_per_minute: u32,
    /// The profile's `limits.stepsPerRun`: the proxy counts one model call per step and refuses
    /// the call past it, so a run stops at the limit however its driver loops (AG-25, AG-51).
    pub steps_per_run: u32,
    pub max_response_bytes: u64,
    /// The profile's `egress.maxBytesPerRun`: what the run may read from the allow-listed hosts
    /// in total, counted by the proxy's fetch route. Zero for a profile that names no host, and
    /// a run with zero reaches nothing (AG-50, AG-65).
    pub max_egress_bytes_per_run: u64,
    pub created_by: String,
    pub model_name: String,
    /// The profile's `model.reasoningEffort`: the proxy adds it to every model call (AG-72).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// The ticket, handed to the workspace and to nobody else. It is in the create answer because
/// the caller is the Portal's own UI in the one deployment that has no cluster to schedule in;
/// in a cluster the Job env is the only carrier and this field is absent.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedRun {
    #[serde(flatten)]
    pub run: AgentRun,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs",
    summary = "Start Unattended Work",
    description = "Starts an application, dashboard or analysis run, which waits for a person's approval at the end.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("X-JC-Run-Origin" = Option<String>, Header, description = "`journey` marks a run of the Portal's own live journeys (AG-93); refused with 403 for anyone but the configured journey users, and beside a bearer token"),
    ),
    request_body(
        content = CreateRunRequest,
        example = json!({
            "appName": "city-bikes-overview",
            "endpointName": "helsinki-bikes",
            "appClass": "ui-rust",
            "visibility": "project",
            "prompt": "A live bike availability dashboard with station filtering",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "mobility" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location"],
                "operations": ["queryEntity", "retrieveEntity"]
            }]
        })
    ),
    responses(
        (status = 202, description = "The run, queued", body = CreatedRun),
        (status = 400, description = "A request the endpoint or the policy does not allow", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No role grants proposing an App here, or `X-JC-Run-Origin` from anyone but the Portal's own live journeys", body = ProblemDetails),
        (status = 404, description = "No such endpoint in this project", body = ProblemDetails),
        (status = 503, description = "No agent runner, or no such builder profile", body = ProblemDetails)
    )
)]
pub async fn create_run(
    user: CurrentUser,
    origin: RunOrigin,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<CreatedRun>), ApiError> {
    let settings = agent_settings(&state)?;

    // Proposing the App this run will write is the permission a run needs; the merge request at
    // the end is reviewed like any other (AP-10, PF-50).
    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "App",
        jc_core::kinds::Verb::Propose,
        None,
    )?;

    within_runs_per_day(&state, &project).await?;

    if !is_dns1123(&request.app_name) {
        return Err(ApiError::BadRequest(format!(
            "appName '{}' is not a DNS-1123 label",
            request.app_name
        )));
    }
    if request.prompt.trim().is_empty() {
        return Err(ApiError::BadRequest("prompt must not be empty".into()));
    }
    if request.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(ApiError::BadRequest(format!(
            "prompt is longer than {MAX_PROMPT_CHARS} characters"
        )));
    }
    // The two enumerations are jc-core's, not a second copy of them: what the `App` kind
    // accepts is what a run may be started for, in jc-core's words for what it refuses (AP-124).
    let app_class = jc_core::kinds::AppClass::parse(&request.app_class)
        .map_err(|refusal| ApiError::BadRequest(format!("appClass: {refusal}")))?;
    serde_json::from_value::<jc_core::kinds::AppVisibility>(serde_json::json!(request.visibility))
        .map_err(|_| {
            ApiError::BadRequest(format!(
                "visibility '{}' is not one of private, project, organization, public",
                request.visibility
            ))
        })?;

    let profile = Profile::load(&state.mirror, &request.profile)?;
    if !profile.is_builder() {
        return Err(ApiError::BadRequest(format!(
            "agent profile '{}' has role '{}' and builds no application (AG-26)",
            profile.name, profile.role
        )));
    }

    if request.kind == "conversation" {
        return Err(ApiError::BadRequest(
            "a conversation starts at /assistant/conversations".into(),
        ));
    }
    if !crate::agents::run::RUN_KINDS.contains(&request.kind.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "kind '{}' is not one of {}",
            request.kind,
            crate::agents::run::RUN_KINDS.join(", ")
        )));
    }
    let unattended = if request.kind == "dashboard" || request.kind == "analysis" {
        true
    } else {
        request.unattended
    };
    // A static application is its own repository on the forge (AP-75): a name the forge would
    // refuse is refused here, before anything is recorded.
    let own_repository = repository::owns_repository(&app_class.to_string(), &request.kind);
    if own_repository {
        if let Some(refusal) = repository::name_refusal(&project, &request.app_name) {
            return Err(ApiError::BadRequest(refusal));
        }
    }

    // The endpoints first: every name the project has, with a slug, at most five (AP-44).
    let names = crate::agents::endpoints::requested(
        request.endpoint_name.as_deref(),
        &request.endpoint_names,
    )
    .map_err(ApiError::BadRequest)?;
    let run_endpoints = crate::agents::endpoints::resolve(&state.mirror, &project, &names)?;

    // AP-42 and AP-44 in one place: public is refused, and every violation of what the
    // endpoints publish is named at once.
    let allows_write = validate_data_needs(
        &state.mirror,
        &project,
        &run_endpoints,
        &request.visibility,
        &request.data_needs,
    )?;
    // T-3159, T-3160: a dashboard and an analysis only read; one that writes is an application.
    if (request.kind == "dashboard" || request.kind == "analysis") && allows_write {
        return Err(ApiError::BadRequest(format!(
            "a{} {} only reads, and these data needs ask for a write: build an application \
             to write (AP-56)",
            if request.kind == "analysis" { "n" } else { "" },
            request.kind
        )));
    }
    // AP-132, PF-70: never wider than what the caller holds, as the gateway says it.
    crate::agents::held::check(
        crate::agents::held::client(),
        state.config.gateway_url.as_deref(),
        caller_token(&state, &headers).as_deref(),
        &run_endpoints,
        &request.data_needs,
        &crate::agents::held::portal_reads(&state, &user.0.identity, &project),
    )
    .await?;

    // AG-70: a profile that lists endpoints builds only on the ones it grants, with write for a
    // run that writes.
    if let Some(refused) = run_endpoints
        .iter()
        .find(|endpoint| !profile.access.grants_endpoint(&endpoint.name, allows_write))
    {
        return Err(ApiError::Denied(format!(
            "agent profile '{}' does not grant {} on endpoint '{}' (AG-70)",
            profile.name,
            if allows_write {
                "read and write"
            } else {
                "read"
            },
            refused.name
        )));
    }

    let endpoint_slug = run_endpoints[0].slug.clone();

    if request.kind != "conversation" {
        if let Some(live) = state
            .agents
            .live_run_for_app(&project, &request.app_name)
            .await
            .map_err(unavailable)?
        {
            return Err(ApiError::Conflict(format!(
                "application '{}' already has a live run: {}",
                request.app_name, live.id
            )));
        }
    }

    let id = mint_run_id();
    let (ticket, ticket_hash) = mint_ticket();
    let created_at = now_rfc3339();
    let expires_at = expiry(settings.run_ttl_secs);
    let run = AgentRun {
        id: id.clone(),
        project: project.clone(),
        app_name: request.app_name.clone(),
        title: None,
        endpoint_name: run_endpoints[0].name.clone(),
        endpoint_slug,
        endpoints: serde_json::to_value(&run_endpoints).unwrap_or_else(|_| serde_json::json!([])),
        profile: profile.name.clone(),
        kind: request.kind.clone(),
        unattended,
        continues: None,
        // Recorded in the new name, whichever the caller wrote (AP-124).
        app_class: app_class.to_string(),
        visibility: request.visibility.clone(),
        prompt: request.prompt.clone(),
        prompt_digest: digest_prompt(&request.prompt),
        data_needs: serde_json::Value::Array(request.data_needs.clone()),
        allows_write,
        branch: format!("agent/app-{}/{}", request.app_name, id),
        // The application's own repository holds it at its root; a workspace run writes its
        // folder of the configuration repository (AP-75, Architecture/19).
        path_prefix: if own_repository {
            String::new()
        } else {
            format!("projects/{project}/apps/{}/", request.app_name)
        },
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
        files: serde_json::Value::Object(serde_json::Map::new()),
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

    if run.kind == "conversation" {
        state.agents.create_run(&run).await.map_err(unavailable)?;
    } else if !state
        .agents
        .create_run_alone(&run)
        .await
        .map_err(unavailable)?
    {
        return Err(ApiError::Conflict(format!(
            "application '{}' already has a live run",
            request.app_name
        )));
    }
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

    // A `ui` application, and a `wasm` one with its server half (T-3575), is driven by the Portal
    // itself, in this process (code on the SDK), and the ticket stays with the driver (AP-56,
    // AG-54). The workspace Job is what `ui-rust` gets.
    if matches!(
        app_class,
        jc_core::kinds::AppClass::Ui | jc_core::kinds::AppClass::Wasm
    ) {
        oneshot::spawn(
            state.clone(),
            &run,
            &user.0.identity,
            &ticket,
            &profile,
            settings,
            crate::agents::oneshot::Opening::default(),
        );
        return Ok((StatusCode::ACCEPTED, Json(CreatedRun { run, ticket: None })));
    }

    // The workspace is where the ticket goes. Scheduling it is the last step, so a run that
    // could not be recorded never has a pod: the pod is what spends money.
    match kube::schedule_workspace_job(
        state.kube.as_deref(),
        &settings.namespace,
        &settings.portal_namespace,
        &run,
        &ticket,
        &settings.proxy_base,
        &profile,
        settings.run_ttl_secs,
    )
    .await
    {
        Ok(scheduled) => {
            if scheduled {
                let _ = state
                    .agents
                    .set_status(&id, AgentRunStatus::Starting, None)
                    .await;
                publish_event(
                    &state,
                    &id,
                    "status",
                    status_payload(AgentRunStatus::Starting),
                )
                .await?;
            }
            let run = state
                .agents
                .get_run(&id)
                .await
                .map_err(unavailable)?
                .unwrap_or(run);
            Ok((
                StatusCode::ACCEPTED,
                Json(CreatedRun {
                    run,
                    // Nowhere to schedule means the caller drives the workspace itself, and
                    // then it needs the ticket; a scheduled run's ticket stays in the Job.
                    ticket: (!scheduled).then_some(ticket),
                }),
            ))
        }
        Err(message) => {
            let _ = state
                .agents
                .set_status(&id, AgentRunStatus::Failed, Some(&message))
                .await;
            publish_event(
                &state,
                &id,
                "status",
                serde_json::json!({
                    "status": AgentRunStatus::Failed.as_str(),
                    "error": message,
                }),
            )
            .await?;
            Err(ApiError::Unavailable(format!(
                "the workspace could not be scheduled: {message}"
            )))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs",
    summary = "List Runs",
    description = "The project's agent runs, newest first, narrowed by app, kind or status.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("X-JC-Run-Origin" = Option<String>, Header, description = "`journey` lists every origin for the Portal's own live journeys (AG-93); refused with 403 for anyone but the configured journey users, and beside a bearer token"),
        ListRunsQuery,
    ),
    responses(
        (status = 200, description = "The project's runs, newest first", body = RunList),
        (status = 400, description = "An `origin` that is none of person, journey and all, or an `X-JC-Run-Origin` other than journey", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "`X-JC-Run-Origin` from anyone but the Portal's own live journeys", body = ProblemDetails),
        (status = 503, description = "The run store did not answer", body = ProblemDetails)
    )
)]
pub async fn list_runs(
    user: CurrentUser,
    origin: RunOrigin,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ListRunsQuery>,
) -> Result<Json<RunList>, ApiError> {
    // The runs of a project the caller may not read are the runs of no project (PF-59, R20,
    // T-1368), not an empty list that says the project is there.
    if !crate::permissions::for_request(&state, &user.0.identity, &project).may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let limit = match query.limit {
        Some(n) if n <= 0 => return Err(ApiError::BadRequest("limit must be positive".into())),
        Some(n) => n.min(MAX_LIST_LIMIT),
        None => DEFAULT_LIST_LIMIT,
    };
    let is_approver =
        crate::api::changes::may_approve_anything(&state, &user.0.identity, &project).is_ok();
    let created_by = if !is_approver || query.mine == Some(true) {
        Some(user.0.identity.username.clone())
    } else {
        None
    };
    let filter = crate::agents::store::RunFilter {
        app: query.app,
        kind: query.kind,
        status: query.status,
        created_by,
        origin: listed_origin(query.origin.as_deref(), origin)?,
    };
    let items = state
        .agents
        .list_runs_filtered(&project, &filter, limit)
        .await
        .map_err(unavailable)?;
    let items = items
        .into_iter()
        .map(|run| with_links(&state, run))
        .collect();
    Ok(Json(RunList { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs/{id}",
    summary = "Read One Run",
    description = "One of this caller's runs: what it is building, what it asks and where it stands.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    responses(
        (status = 200, description = "The run", body = AgentRun),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails)
    )
)]
pub async fn get_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<AgentRun>, ApiError> {
    Ok(Json(with_links(
        &state,
        own_run_of(&state, &user, &project, &id).await?,
    )))
}

/// The run with the links a person follows from it: its source in Git and the Change that
/// publishes it (AP-71). The forge's public address, never a cluster-internal name.
pub(crate) fn with_links(state: &AppState, mut run: AgentRun) -> AgentRun {
    run.change_id = run
        .merge_request
        .and_then(|number| u64::try_from(number).ok())
        .map(|number| crate::change::ChangeMeta::from_merge_request(number, "").name);
    if let Some(gitea) = state.gitea.as_deref() {
        if !run.branch.is_empty() {
            run.source_url = Some(if run.in_own_repository() {
                gitea
                    .for_application(repository::name(&run.project, &run.app_name))
                    .browse_url("", &run.branch)
            } else {
                // The project's own repository in layout 2 (CC-87).
                state.forge_for(&run.project).map_or_else(
                    || gitea.browse_url(&run.path_prefix, &run.branch),
                    |forge| forge.browse_url(&run.path_prefix, &run.branch),
                )
            });
            if run.in_own_repository() {
                run.mirror_url = state.github_mirror.as_deref().map(|mirror| {
                    mirror.web_url(&repository::name(&run.project, &run.app_name), &run.branch)
                });
            }
        }
    }
    run
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs/{id}/events",
    summary = "Follow A Run",
    description = "One run's events as Server-Sent Events: its status, what it said, the tools it used and the questions it asks. The caller's own runs, or any run for an approver of the project.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
        ("Last-Event-ID" = Option<i64>, Header, description = "Resume after this sequence number"),
    ),
    responses(
        (status = 200, description = "The run's event stream, text/event-stream"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails)
    )
)]
pub async fn stream_events(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    own_run_of(&state, &user, &project, &id).await?;
    let after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);

    // Subscribe before reading the backlog: an event published between the two is then in the
    // live half rather than in neither, and the sequence number filter drops the overlap.
    let live = state.agent_events.subscribe(&id).await;
    let replay = state
        .agents
        .events_since(&id, after)
        .await
        .map_err(unavailable)?;
    let replayed_through = replay.last().map(|event| event.seq).unwrap_or(after);

    let backlog = tokio_stream::iter(
        replay
            .into_iter()
            .map(|event| Ok::<Event, std::convert::Infallible>(sse(&event))),
    );
    let tail = BroadcastStream::new(live).filter_map(
        move |item| -> Option<Result<Event, std::convert::Infallible>> {
            match item {
                Ok(event) if event.seq > replayed_through => Some(Ok(sse(&event))),
                // Already in the backlog this stream just sent.
                Ok(_) => None,
                // A browser that fell behind is told so rather than handed a stream with a hole in it;
                // its reconnect carries `Last-Event-ID` and the backlog fills the gap.
                Err(BroadcastStreamRecvError::Lagged(missed)) => Some(Ok(Event::default()
                    .event("lag")
                    .data(serde_json::json!({ "missed": missed }).to_string()))),
            }
        },
    );

    let stream = Sse::new(backlog.chain(tail)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(KEEP_ALIVE_SECS))
            .text("keep-alive"),
    );

    let mut response = stream.into_response();
    // The edge buffers a proxied response by default, which would hold every event until the
    // run ended (AG-45).
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        header::HeaderValue::from_static("no"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/answers",
    summary = "Answer A Run's Question",
    description = "Answers one question a run asked, so it carries on; a person answers, never another run.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body(
        content = AnswerRequest,
        example = json!({ "questionId": "layout", "answers": ["A map beside the table"] })
    ),
    responses(
        (status = 204, description = "The answer is on the run's log"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project, or the run asked no question with that id", body = ProblemDetails),
        (status = 409, description = "The run is over", body = ProblemDetails),
        (status = 400, description = "The answer is not one of what the question offered", body = ProblemDetails)
    )
)]
pub async fn answer_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<AnswerRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_to_write(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and asks nothing",
            run.status
        )));
    }
    let asked = state
        .agents
        .events_since(&id, 0)
        .await
        .map_err(unavailable)?
        .into_iter()
        .rev()
        .find(|event| {
            event.kind == "question"
                && event.payload.get("questionId").and_then(|q| q.as_str())
                    == Some(request.question_id.as_str())
        });
    // An answer only ever answers a question the run asked: an unknown id would carry files, URLs
    // and JSON of any shape past offered_answer's bounds into the log and the model (T-5701).
    let asked = asked.ok_or_else(|| {
        ApiError::NotFound(format!(
            "run '{id}' asked no question '{}'",
            request.question_id
        ))
    })?;
    offered_answer(&asked.payload, &request.answers).map_err(ApiError::BadRequest)?;
    hand_over_on_turn(&state, &headers, &id).await;
    publish_event(
        &state,
        &id,
        "answer",
        serde_json::json!({
            "questionId": request.question_id,
            "answers": request.answers,
            "answeredBy": user.0.identity.username,
        }),
    )
    .await?;
    record_answer(&state, &project, &id, &user, &request.question_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// The person's turn on a run they own: their token goes to the proxy again before the run acts
/// on the turn, so a grant the start could not leave, or a proxy restart took, is back
/// (ADR-N-038 §3.1, T-6483). No token, no agent runner: nothing to hand over, and the run keeps
/// the grant it holds.
async fn hand_over_on_turn(state: &AppState, headers: &HeaderMap, id: &str) {
    let Some(token) =
        crate::agents::identity::persons_token(headers, state.config.trust_edge_token)
    else {
        return;
    };
    let Ok(settings) = agent_settings(state) else {
        return;
    };
    crate::agents::identity::hand_over(
        state.oidc.as_deref(),
        &settings.proxy_base,
        id,
        Some(token),
    )
    .await;
}

/// An answer is what the question offered, never what a browser made up (AG-83, UI-73, T-2694):
/// an option that was not offered or is disabled, a file or an address the question did not ask
/// for or that breaks its limits, a repeat or a count outside `min`/`max` is refused and the
/// question stays open. A question of the model's own options with one answer keeps "Something
/// else…" (UI-57).
fn offered_answer(question: &serde_json::Value, answers: &serde_json::Value) -> Result<(), String> {
    use serde_json::Value;
    let given = ["answer", "file", "url"]
        .iter()
        .filter(|key| answers.get(**key).is_some())
        .count();
    if given > 1 {
        return Err("an answer is one option, one file or one address".to_owned());
    }
    let input = question.get("input").filter(|input| !input.is_null());
    if let Some(file) = answers.get("file") {
        let asked = input
            .and_then(|input| input.get("file"))
            .ok_or("the question asks for no file")?;
        return offered_file(asked, file);
    }
    if let Some(url) = answers.get("url") {
        if input.and_then(|input| input.get("url")) != Some(&Value::Bool(true)) {
            return Err("the question asks for no address".to_owned());
        }
        return offered_url(url);
    }
    let options = question
        .get("options")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let chosen_of = |answer: &Value| -> Vec<String> {
        match answer {
            Value::String(one) => vec![one.clone()],
            Value::Array(many) => many
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    };
    let chosen = answers.get("answer").map(chosen_of).unwrap_or_default();
    for one in &chosen {
        let reason = options
            .iter()
            .find(|option| option.get("value").and_then(Value::as_str) == Some(one.as_str()))
            .and_then(|option| option.get("disabledReason"))
            .and_then(Value::as_str);
        if let Some(reason) = reason {
            return Err(format!("'{one}' cannot be chosen: {reason}"));
        }
    }
    let picked = question.get("pick").is_some_and(|pick| !pick.is_null());
    let multiple = question.get("multiple").and_then(Value::as_bool) == Some(true);
    if !picked && !multiple {
        return Ok(());
    }
    let offered: Vec<&str> = options
        .iter()
        .filter_map(|option| option.get("value").and_then(Value::as_str))
        .collect();
    let answer = answers.get("answer");
    let chosen: Vec<&str> = if multiple {
        let many = answer
            .and_then(Value::as_array)
            .ok_or("the question takes a list of answers")?;
        many.iter()
            .map(|one| one.as_str().ok_or("every answer is one of the options"))
            .collect::<Result<_, _>>()?
    } else {
        vec![answer
            .and_then(Value::as_str)
            .ok_or("the question takes one of its options")?]
    };
    if let Some(stranger) = chosen.iter().find(|one| !offered.contains(one)) {
        return Err(format!("'{stranger}' is not one of the options offered"));
    }
    let mut unique = chosen.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != chosen.len() {
        return Err("an option is chosen twice".to_owned());
    }
    let count = |key: &str| question.get(key).and_then(Value::as_u64);
    if let Some(min) = count("min") {
        if (chosen.len() as u64) < min {
            return Err(format!("choose at least {min}"));
        }
    }
    if let Some(max) = count("max") {
        if (chosen.len() as u64) > max {
            return Err(format!("choose at most {max}"));
        }
    }
    Ok(())
}

/// A file handed to a question that asked for one: `{name, format, text}` and nothing else, a
/// format it accepts, text within its size, no NUL, and JSON that parses (API/04 section 5).
fn offered_file(asked: &serde_json::Value, file: &serde_json::Value) -> Result<(), String> {
    use serde_json::Value;
    let fields = file.as_object().ok_or("a file is {name, format, text}")?;
    if let Some(stranger) = fields
        .keys()
        .find(|key| !["name", "format", "text"].contains(&key.as_str()))
    {
        return Err(format!("a file carries no '{stranger}'"));
    }
    let field = |key: &str| {
        fields
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("a file needs its {key}"))
    };
    let (name, format, text) = (field("name")?, field("format")?, field("text")?);
    if name.trim().is_empty() || name.chars().count() > 255 || name.contains(['/', '\\']) {
        return Err("a file's name is up to 255 characters, without a path".to_owned());
    }
    let accepted = asked
        .get("accept")
        .and_then(Value::as_array)
        .is_some_and(|accept| accept.iter().any(|one| one.as_str() == Some(format)));
    if !accepted {
        return Err(format!("the question does not take a {format} file"));
    }
    // A workbook is offered so the browser takes one; it answers with the CSV of the sheet the
    // person chose, since the run reads text (JC1-07).
    if format == "xlsx" {
        return Err("an Excel workbook is answered as the CSV of one of its sheets".to_owned());
    }
    let limit = asked.get("maxBytes").and_then(Value::as_u64).unwrap_or(0);
    if text.is_empty() {
        return Err("the file is empty".to_owned());
    }
    if text.len() as u64 > limit {
        return Err(format!(
            "the file is {} bytes; the question takes up to {limit}",
            text.len()
        ));
    }
    if text.contains('\0') {
        return Err("the file is not text".to_owned());
    }
    if format == "json" && serde_json::from_str::<Value>(text).is_err() {
        return Err("the file is not JSON".to_owned());
    }
    Ok(())
}

/// A feed's address handed to a question that asked for one: absolute `http(s)`, at most 2048
/// characters. Where it points is the fetch guard's to judge when the run reads it.
fn offered_url(url: &serde_json::Value) -> Result<(), String> {
    let url = url.as_str().ok_or("an address is a string")?;
    if url.len() > 2048 {
        return Err("the address is longer than 2048 characters".to_owned());
    }
    match url::Url::parse(url) {
        Ok(parsed) if ["http", "https"].contains(&parsed.scheme()) && parsed.host().is_some() => {
            Ok(())
        }
        _ => Err("the address is not an absolute http or https URL".to_owned()),
    }
}

/// A person answered a run's question, on the project's activity (AG-80, OPS-48).
///
/// What they typed stays on the run's timeline, where the driver redacts it; the activity feed
/// says only that the question was answered, by whom, so a typed password never reaches a
/// projection built for reading. Failing to record it does not fail the answer: the timeline and
/// the audit log already hold it, and the activity store is derived from them (OPS-49).
async fn record_answer(
    state: &AppState,
    project: &str,
    run_id: &str,
    user: &CurrentUser,
    question_id: &str,
) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "agent.answer".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} answered a question of run {run_id}",
            user.0.identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: serde_json::json!({
            "object": format!("agent-runs/{run_id}"),
            "runId": run_id,
            "questionId": question_id,
            "subject": user.0.identity.subject,
        }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(run = run_id, error = %err, "the answer is not on the activity feed");
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/messages",
    summary = "Send A Run A Message",
    description = "Sends text to one of this caller's running conversations or applications.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body(
        content = MessageRequest,
        example = json!({ "text": "Make this title red", "scope": "src/pages/Stations.tsx:14" })
    ),
    responses(
        (status = 204, description = "The instruction is on the run's log and in its inbox"),
        (status = 400, description = "An empty or over-long instruction", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is over and reads nothing", body = ProblemDetails)
    )
)]
pub async fn post_message(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<MessageRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_to_write(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and reads nothing",
            run.status
        )));
    }
    let text = request.text.trim();
    if text.is_empty() {
        return Err(ApiError::BadRequest("text must not be empty".into()));
    }
    if text.chars().count() > MAX_MESSAGE_CHARS {
        return Err(ApiError::BadRequest(format!(
            "text is longer than {MAX_MESSAGE_CHARS} characters"
        )));
    }
    if request.page_context.is_some() && run.kind != "conversation" {
        return Err(ApiError::BadRequest(
            "pageContext is the page of a conversation only".into(),
        ));
    }
    let page = crate::api::assistant::page_context(request.page_context.as_ref(), &project)?;
    if let Some(scope) = &request.scope {
        if run.kind == "conversation" {
            return Err(ApiError::BadRequest(
                "scope is a file of an application run only".into(),
            ));
        }
        if crate::agents::code::scope(scope).is_none() {
            return Err(ApiError::BadRequest(
                "scope is {file}:{line}, an interface file under src/ ending in .tsx (SDK-46)"
                    .into(),
            ));
        }
    }
    if let Some(access) = &request.access {
        if run.kind != "conversation" {
            return Err(ApiError::BadRequest(
                "access is the capabilities of a conversation only".into(),
            ));
        }
        access.check().map_err(ApiError::BadRequest)?;
    }
    if let Some(names) = &request.endpoint_names {
        if run.kind != "conversation" {
            return Err(ApiError::BadRequest(
                "endpointNames changes the endpoints of a conversation only".into(),
            ));
        }
        let endpoints = if names.is_empty() {
            Vec::new()
        } else {
            let names =
                crate::agents::endpoints::requested(None, names).map_err(ApiError::BadRequest)?;
            crate::agents::endpoints::resolve_readable(&state.mirror, &project, &names)?
        };
        let profile = crate::agents::profile::Profile::load(&state.mirror, &run.profile)?;
        if let Some(refused) = endpoints
            .iter()
            .find(|endpoint| !profile.access.grants_endpoint(&endpoint.name, false))
        {
            return Err(ApiError::Denied(format!(
                "agent profile '{}' does not grant reading endpoint '{}' (AG-70)",
                profile.name, refused.name
            )));
        }
        let changed = crate::agents::endpoints::of_run(&run) != endpoints;
        if changed {
            state
                .agents
                .set_endpoints(&id, &endpoints)
                .await
                .map_err(unavailable)?;
            publish_event(
                &state,
                &id,
                "endpoints",
                serde_json::json!({
                    "names": endpoints.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
                }),
            )
            .await?;
        }
    }
    hand_over_on_turn(&state, &headers, &id).await;
    publish_event(
        &state,
        &id,
        "message",
        serde_json::json!({
            "text": text,
            "sentBy": user.0.identity.username,
            "page": page,
            "access": request.access,
            "scope": request.scope,
        }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/preview-errors",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body(
        content = PreviewErrorRequest,
        example = json!({ "message": "TypeError: rows is undefined", "file": "src/App.tsx", "line": 42 })
    ),
    responses(
        (status = 204, description = "The error is on the run's log as a preview_error event"),
        (status = 400, description = "A blank or over-long message, an over-long file, or line 0", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is over", body = ProblemDetails)
    )
)]
/// A runtime error of the preview, for the first run's repair and the editing agent's
/// `preview_errors` tool (SDK-14, SDK-20). The frame wrote every field: it is stored as text.
pub async fn post_preview_error(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    Json(request): Json<PreviewErrorRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_to_write(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and repairs nothing",
            run.status
        )));
    }
    let message = request.message.trim();
    if message.is_empty() || message.chars().count() > MAX_PREVIEW_ERROR_CHARS {
        return Err(ApiError::BadRequest(format!(
            "message must be 1 to {MAX_PREVIEW_ERROR_CHARS} characters"
        )));
    }
    if request
        .file
        .as_ref()
        .is_some_and(|file| file.chars().count() > MAX_PREVIEW_FILE_CHARS)
    {
        return Err(ApiError::BadRequest(format!(
            "file is longer than {MAX_PREVIEW_FILE_CHARS} characters"
        )));
    }
    if request.line == Some(0) {
        return Err(ApiError::BadRequest("line starts at 1".into()));
    }
    publish_event(
        &state,
        &id,
        "preview_error",
        serde_json::json!({
            "message": message,
            "file": request.file,
            "line": request.line,
            "reportedBy": user.0.identity.username,
        }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/preview-observations",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body(
        content = PreviewObservationRequest,
        example = json!({
            "version": 2,
            "pages": [{ "label": "Overview", "text": "12 stations, 3 without a bike", "rows": [12] }],
            "failedRequests": [{ "path": "/ngsi-ld/v1/entities", "status": 403 }]
        })
    ),
    responses(
        (status = 204, description = "The observation is on the run's log as a preview_observation event"),
        (status = 400, description = "A value outside the bounds of API/04 §5", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is over, or this version was observed already", body = ProblemDetails)
    )
)]
/// What the preview frame saw of one version, for the run's verification (SDK-27, SDK-28). The
/// first observation of a version counts; a second one is refused, so a reload does not start a
/// second check of the same files.
pub async fn post_preview_observation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    Json(request): Json<PreviewObservationRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_to_write(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and verifies nothing",
            run.status
        )));
    }
    if let Some(problem) = observation_out_of_bounds(&request) {
        return Err(ApiError::BadRequest(problem));
    }
    let observed = state
        .agents
        .events_since(&id, 0)
        .await
        .map_err(unavailable)?
        .iter()
        .any(|event| {
            event.kind == "preview_observation"
                && event
                    .payload
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(request.version))
        });
    if observed {
        return Err(ApiError::Conflict(format!(
            "version {} of run '{id}' was observed already",
            request.version
        )));
    }
    let mut payload = serde_json::to_value(&request)
        .map_err(|err| ApiError::Internal(format!("the observation did not serialise: {err}")))?;
    payload["reportedBy"] = serde_json::json!(user.0.identity.username);
    publish_event(&state, &id, "preview_observation", payload).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The bounds of an observation (API/04 §5), or `None` when it keeps them.
fn observation_out_of_bounds(request: &PreviewObservationRequest) -> Option<String> {
    if request.version == 0 {
        return Some("version starts at 1".into());
    }
    if request.pages.is_empty() || request.pages.len() > MAX_OBSERVED_PAGES {
        return Some(format!("pages must hold 1 to {MAX_OBSERVED_PAGES} pages"));
    }
    for (index, page) in request.pages.iter().enumerate() {
        if page.label.chars().count() > MAX_PAGE_LABEL_CHARS {
            return Some(format!(
                "pages[{index}].label is longer than {MAX_PAGE_LABEL_CHARS} characters"
            ));
        }
        if page.text.chars().count() > MAX_PAGE_TEXT_CHARS {
            return Some(format!(
                "pages[{index}].text is longer than {MAX_PAGE_TEXT_CHARS} characters"
            ));
        }
        if page.rows.len() > MAX_OBSERVED_ENTRIES {
            return Some(format!(
                "pages[{index}].rows holds more than {MAX_OBSERVED_ENTRIES} counts"
            ));
        }
        if page.h1.is_some_and(|h1| h1 > MAX_OBSERVED_H1) {
            return Some(format!("pages[{index}].h1 is more than {MAX_OBSERVED_H1}"));
        }
    }
    if request.failed_requests.len() > MAX_OBSERVED_ENTRIES {
        return Some(format!(
            "failedRequests holds more than {MAX_OBSERVED_ENTRIES} entries"
        ));
    }
    for (index, failed) in request.failed_requests.iter().enumerate() {
        if failed.path.chars().count() > MAX_PREVIEW_FILE_CHARS {
            return Some(format!(
                "failedRequests[{index}].path is longer than {MAX_PREVIEW_FILE_CHARS} characters"
            ));
        }
        if !(400..=599).contains(&failed.status) {
            return Some(format!(
                "failedRequests[{index}].status must be between 400 and 599"
            ));
        }
    }
    None
}

/// How long the Portal waits for `jc-functions`, whose own limit is 5 s of script.
const FUNCTION_TIMEOUT: Duration = Duration::from_secs(20);

/// How many runs this project may start in a day (PF-73, PF-74), asked by every door that
/// starts one: an application run and an assistant conversation (T-1403). The store answers
/// newest first, so the newest `max` runs are enough to know whether today is full.
pub(crate) async fn within_runs_per_day(state: &AppState, project: &str) -> Result<(), ApiError> {
    let Some(max) = crate::quotas::effective(&state.mirror, project).agent_runs_per_day else {
        return Ok(());
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let recent = state
        .agents
        .list_runs(project, i64::from(max))
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    let started_today = recent
        .iter()
        .filter(|run| run.created_at.starts_with(&today))
        .count() as u32;
    if started_today >= max {
        return Err(crate::quotas::over(
            "agentRunsPerDay",
            started_today + 1,
            max,
            project,
        ));
    }
    Ok(())
}

/// `[a-z][a-z0-9-]{0,39}`: the name of a function file (Architecture/20 §3).
pub(crate) fn is_function_name(name: &str) -> bool {
    name.len() <= 40
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The access token the request was authenticated with: a bearer or the edge's. A Portal cookie
/// session keeps none, so its function calls reach the endpoint anonymously (API/04 §5).
/// Read as [`crate::agents::identity::persons_token`] reads it (F-R2POR1-015).
pub(crate) fn caller_token(state: &AppState, headers: &HeaderMap) -> Option<String> {
    crate::agents::identity::persons_token(headers, state.config.trust_edge_token)
        .map(str::to_owned)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/functions/{fn}",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
        ("fn" = String, Path, description = "The function: `functions/{fn}.ts` of the run"),
    ),
    request_body(
        content = serde_json::Value,
        description = "The function's JSON body; an empty body is null",
        content_type = "application/json",
        example = json!({ "station": "001" })
    ),
    responses(
        (status = 200, description = "The function's own status and JSON body, whatever status it returned", body = serde_json::Value),
        (status = 400, description = "A body that is not JSON, or function files that do not build: every problem with file and line", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project, or no `functions/{fn}.ts` among its files", body = ProblemDetails),
        (status = 409, description = "The run is over", body = ProblemDetails),
        (status = 429, description = "The functions runtime is running all the calls it takes", body = ProblemDetails),
        (status = 500, description = "The function threw, ran out of memory or time, or returned more than 1 MiB: `{error: {message, file, line}}`", body = serde_json::Value),
        (status = 503, description = "No `JC_FUNCTIONS_URL`, no Keycloak client, a Portal built without the SDK server module, or a runtime that did not answer", body = ProblemDetails)
    )
)]
/// One call of a run's function in `jc-functions`: the run's current functions, the request and
/// the caller's own token, sent with the Portal's audience-bound token (SDK-18, SDK-23).
pub async fn call_function(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id, name)): Path<(String, String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let run = own_run_to_write(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and runs no function",
            run.status
        )));
    }
    let tool = format!("function:{name}");
    let input: serde_json::Value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| ApiError::BadRequest(format!("the body is not JSON: {err}")))?
    };
    let invoked = invoke_function(
        &state,
        &run,
        &name,
        input.clone(),
        &query,
        &user.0.identity,
        Caller::Person(caller_token(&state, &headers)),
    )
    .await;
    let Invocation {
        outcome,
        duration_ms,
    } = match invoked {
        Ok(invocation) => invocation,
        Err(InvokeError::NoFunction) => {
            return Err(ApiError::NotFound(format!(
                "run '{id}' has no function '{name}'"
            )))
        }
        Err(InvokeError::DoesNotBuild(problems)) => {
            return Err(ApiError::Invalid {
                detail: format!(
                    "the run's functions do not build: {} problem(s)",
                    problems.len()
                ),
                errors: problems,
            })
        }
        Err(InvokeError::Unavailable(reason)) => return Err(ApiError::Unavailable(reason)),
        Err(InvokeError::Refused {
            reason,
            duration_ms,
            status,
        }) => {
            publish_event(
                &state,
                &id,
                "tool",
                serde_json::json!({
                    "tool": tool,
                    "status": "failed",
                    "durationMs": duration_ms,
                    "input": input,
                    "error": reason,
                }),
            )
            .await?;
            return Err(match status {
                RefusedStatus::NoAnswer => {
                    ApiError::Unavailable("jc-functions did not answer".into())
                }
                RefusedStatus::Full => ApiError::TooManyRequests(
                    "jc-functions is running all the calls it takes; try again".into(),
                ),
                RefusedStatus::TooLarge => ApiError::BadRequest(
                    "the function's body is larger than jc-functions takes (256 KiB)".into(),
                ),
                RefusedStatus::Other => ApiError::Unavailable(reason),
            });
        }
    };
    let status = outcome["status"]
        .as_u64()
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let logs = outcome
        .get("logs")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(error) = outcome.get("error").filter(|error| !error.is_null()) {
        // The runtime names files by their import name; the run knows them by path.
        let mut error = error.clone();
        if let Some(file) = error["file"].as_str() {
            error["file"] = serde_json::json!(file.strip_prefix(transpile::APP).unwrap_or(file));
        }
        // A data call the gateway refused for want of a credential is the caller's to fix
        // (sign in again), not a fault of the function (T-3134).
        let status = if unauthenticated(&error) {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        publish_event(
            &state,
            &id,
            "tool",
            serde_json::json!({
                "tool": tool,
                "status": "failed",
                "durationMs": duration_ms,
                "input": input,
                "output": { "status": status.as_u16(), "logs": logs },
                "error": error,
            }),
        )
        .await?;
        let slug = if status == StatusCode::UNAUTHORIZED {
            "unauthorized"
        } else {
            "internal-error"
        };
        let mut body = serde_json::json!({ "error": error });
        if let Some(message) = error["message"].as_str() {
            body["detail"] = serde_json::json!(message);
        }
        return Ok(crate::error::problem_response(body, status, slug));
    }
    publish_event(
        &state,
        &id,
        "tool",
        serde_json::json!({
            "tool": tool,
            "status": "ok",
            "durationMs": duration_ms,
            "input": input,
            "output": { "status": status.as_u16(), "logs": logs },
        }),
    )
    .await?;
    let body = outcome
        .get("body")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok((status, Json(body)).into_response())
}

/// The gateway's answer to a data call that carries no valid credential, as the SDK's
/// `ProblemError` brings its title out of the function (`@joinedcontext/sdk/server`).
const UNAUTHENTICATED: &str = "Authentication Required";

/// Whether a function failed because its data call was refused as unauthenticated, which is a
/// `401` the caller can act on, never the function's own error (T-3134).
pub(crate) fn unauthenticated(error: &serde_json::Value) -> bool {
    error["message"].as_str() == Some(UNAUTHENTICATED)
}

/// What one invocation of a run's function returned: the runtime's outcome and how long it took.
pub(crate) struct Invocation {
    pub outcome: serde_json::Value,
    pub duration_ms: u64,
}

/// Why the runtime was not asked, or did not take the call.
pub(crate) enum InvokeError {
    NoFunction,
    DoesNotBuild(Vec<String>),
    /// This Portal cannot reach a runtime at all: no address, no client, no server module.
    Unavailable(String),
    /// The runtime did not take the call; `reason` is what the run's log says.
    Refused {
        reason: String,
        duration_ms: u64,
        status: RefusedStatus,
    },
}

pub(crate) enum RefusedStatus {
    NoAnswer,
    Full,
    TooLarge,
    Other,
}

/// One call of a run's function in `jc-functions`, as `identity` (SDK-18, SDK-23): the route
/// and the editing agent's `call_function` tool share it (SDK-20). `caller_token` is the
/// person's own edge token when the call comes through the edge, nothing otherwise.
pub(crate) async fn invoke_function(
    state: &AppState,
    run: &AgentRun,
    name: &str,
    input: serde_json::Value,
    query: &BTreeMap<String, String>,
    identity: &crate::auth::session::Identity,
    caller: Caller,
) -> Result<Invocation, InvokeError> {
    let files: BTreeMap<String, String> = run
        .files
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(path, _)| path.starts_with("functions/"))
        .filter_map(|(path, text)| Some((path.clone(), text.as_str()?.to_owned())))
        .collect();
    let entry = format!("functions/{name}.ts");
    if !is_function_name(name) || !files.contains_key(&entry) {
        return Err(InvokeError::NoFunction);
    }
    let built = transpile::transpile(&files);
    let problems: Vec<String> = built
        .problems
        .iter()
        .filter(|problem| !problem.file.contains(".test."))
        .map(ToString::to_string)
        .collect();
    if !problems.is_empty() {
        return Err(InvokeError::DoesNotBuild(problems));
    }
    let space = state
        .mirror
        .get(&run.project, "Endpoint", &run.endpoint_name)
        .and_then(|env| crate::api::assistant::ref_name(&env.spec["contextSpaceRef"]))
        .unwrap_or_else(|| run.project.clone());
    let request = serde_json::json!({
        "method": "POST",
        "query": query,
        "body": input,
        "user": {
            "id": identity.subject,
            "name": identity.name.clone().unwrap_or_else(|| identity.username.clone()),
            "email": identity.email,
            "roles": identity.roles,
        },
    });
    let mut config = serde_json::json!({
        "slug": run.endpoint_slug,
        "orgDomain": crate::api::assistant::org_domain(state, &run.project),
        "space": space,
    });
    let run_endpoints = crate::agents::endpoints::of_run(run);
    if run_endpoints.len() > 1 {
        config["endpoints"] = crate::agents::endpoints::config(&run_endpoints, &run.data_needs);
    }
    // The published route's entry, so a preview answers a refused data call as that route does.
    let mut modules = built.functions;
    let module = format!("{}{entry}", transpile::APP);
    let wrapper = format!("{}entry.js", transpile::APP);
    modules.insert(
        wrapper.clone(),
        crate::apps::functions::entry(&module, "default"),
    );
    invoke(state, modules, wrapper, request, config, caller).await
}

/// The document `jc-functions` runs: the files, the entry, the request, the SDK's configuration
/// and the caller's credential; a run's own call also names the proxy route (ADR-N-038 decision 6).
fn invocation_body(
    modules: BTreeMap<String, String>,
    entry: String,
    request: serde_json::Value,
    config: serde_json::Value,
    caller: Caller,
) -> serde_json::Value {
    let (token, via_proxy) = match caller {
        Caller::Person(token) => (token, false),
        Caller::Run(credential) => (Some(credential), true),
    };
    let mut invocation = serde_json::json!({
        "files": modules,
        "entry": entry,
        "request": request,
        "config": config,
        "token": token,
    });
    if via_proxy {
        invocation["via"] = serde_json::json!("proxy");
    }
    invocation
}

/// Whose credential a function's data calls carry.
pub(crate) enum Caller {
    /// The person's own access token, or none for a session that carries none: the function then
    /// calls its endpoint anonymously.
    Person(Option<String>),
    /// The run's data credential from the agent proxy: the editing agent's own call, which reads
    /// through the proxy as the run (ADR-N-038 decision 6).
    Run(String),
}

/// Sends one invocation to `jc-functions` with the Portal's audience-bound token: `modules` by
/// import name, the `entry` whose default export runs, the request, the SDK's configuration and
/// the caller's own token, which is the only credential the function's data calls carry
/// (SDK-18, SDK-23, AP-84). The preview and the published route share it; the editing agent's
/// own call sends the run's data credential through the agent proxy instead ([`Caller::Run`]).
pub(crate) async fn invoke(
    state: &AppState,
    mut modules: BTreeMap<String, String>,
    entry: String,
    request: serde_json::Value,
    config: serde_json::Value,
    caller: Caller,
) -> Result<Invocation, InvokeError> {
    let runtime = state.config.functions_url.as_deref().ok_or_else(|| {
        InvokeError::Unavailable(
            "this Portal has no jc-functions address (JC_FUNCTIONS_URL)".into(),
        )
    })?;
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        InvokeError::Unavailable(
            "this Portal has no Keycloak client to authenticate to jc-functions with".into(),
        )
    })?;
    let server = kit::functions_server().ok_or_else(|| {
        InvokeError::Unavailable(
            "this Portal was built without the SDK server module (sdk/dist/functions-server.js)"
                .into(),
        )
    })?;
    let service = oidc
        .service_token()
        .await
        .map_err(|err| InvokeError::Unavailable(format!("no token for jc-functions: {err}")))?;
    modules.insert("@joinedcontext/sdk/server".to_owned(), server);
    let invocation = invocation_body(modules, entry, request, config, caller);

    let started = std::time::Instant::now();
    let answer = functions_http()
        .post(format!("{runtime}/invoke"))
        .bearer_auth(service)
        .json(&invocation)
        .send()
        .await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let refused = |reason: String, status: RefusedStatus| InvokeError::Refused {
        reason,
        duration_ms,
        status,
    };
    let response = match answer {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "jc-functions did not answer");
            return Err(refused(
                "jc-functions did not answer".into(),
                RefusedStatus::NoAnswer,
            ));
        }
    };
    match response.status() {
        StatusCode::OK => {}
        StatusCode::TOO_MANY_REQUESTS => {
            return Err(refused("jc-functions is full".into(), RefusedStatus::Full))
        }
        StatusCode::PAYLOAD_TOO_LARGE => {
            return Err(refused(
                "the request is too large".into(),
                RefusedStatus::TooLarge,
            ))
        }
        status => {
            tracing::warn!(%status, "jc-functions refused an invocation");
            return Err(refused(
                format!("jc-functions answered {status}"),
                RefusedStatus::Other,
            ));
        }
    }
    let outcome: serde_json::Value = response.json().await.map_err(|err| {
        refused(
            format!("jc-functions answered no outcome: {err}"),
            RefusedStatus::Other,
        )
    })?;
    Ok(Invocation {
        outcome,
        duration_ms,
    })
}

/// One client for every invocation, without redirects: the runtime's address is configuration.
fn functions_http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(FUNCTION_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

pub(crate) async fn end_run(
    state: &AppState,
    run: &AgentRun,
    status: AgentRunStatus,
    reason: &str,
) -> Result<AgentRun, ApiError> {
    // The ticket first, then the status, then the pod: from the first call on, the workspace
    // is refused by the proxy (AG-46). Clearing a ticket is safe to repeat, so an end that
    // failed after it is finished by the next one; with the status first, a store error between
    // the two left a run that says it is over with a live ticket, and every later end a 409
    // (T-2560).
    let ticket_was_live = !run.ticket_hash.is_empty();
    state
        .agents
        .invalidate_ticket(&run.id)
        .await
        .map_err(unavailable)?;
    let ended = state.agents.set_status(&run.id, status, Some(reason)).await;
    // A run whose ticket this call killed also loses its pod, even when its status could not
    // move: the pod has nothing left to work with, and nothing else would delete it.
    if ended.is_ok() || ticket_was_live {
        if let Some(settings) = state.config.agent_settings.as_ref() {
            kube::delete_workspace_job(state.kube.as_deref(), &settings.namespace, &run.id).await;
        }
    }
    let ended = ended.map_err(status_error)?;
    // Who or what ended it travels with the status, so the stream says it and not only the
    // run's record (T-2772): "cancelled by jana.kovacova", a lease that ran out.
    let mut payload = status_payload(status);
    payload["reason"] = serde_json::json!(reason);
    publish_event(state, &run.id, "status", payload).await?;
    Ok(ended)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/cancel",
    summary = "Cancel Run",
    description = "Stops one of this caller's runs; what it had not finished is not published.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    responses(
        (status = 200, description = "The cancelled run", body = AgentRun),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is already over", body = ProblemDetails)
    )
)]
pub async fn cancel_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<AgentRun>, ApiError> {
    let run = own_run_to_write(&state, &user, &project, &id).await?;

    let reason = format!("cancelled by {}", user.0.identity.username);
    let cancelled = end_run(&state, &run, AgentRunStatus::Cancelled, &reason).await?;
    Ok(Json(cancelled))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/publish",
    summary = "Publish What A Run Built",
    description = "Proposes the application a finished run built; a person approves the change.",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    responses(
        (status = 202, description = "The Change that publishes the application", body = crate::change::Change),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run has nothing to publish yet", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails)
    )
)]
pub async fn publish_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let run = own_run_to_write(&state, &user, &project, &id).await?;
    if run.kind == "dashboard" {
        return Err(ApiError::Conflict(
            "publishing a dashboard run is not available yet".into(),
        ));
    }
    if run.kind == "analysis" {
        return Err(ApiError::Conflict("an analysis is never published".into()));
    }
    let status = AgentRunStatus::parse(&run.status);
    // An unattended run ends waiting for approval with its preview built, so it is published
    // from there (AG-69).
    let waiting = run.unattended && status == Some(AgentRunStatus::AwaitingApproval);
    if status != Some(AgentRunStatus::Previewing) && !waiting {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}'; only a run that has a preview is published (AP-46)",
            run.status
        )));
    }

    let events = state
        .agents
        .events_since(&id, 0)
        .await
        .map_err(unavailable)?;
    if let Some(held) = tests_hold(run.preview_url.as_deref(), &events) {
        return Err(ApiError::Conflict(held));
    }

    // The application is published the way every other manifest is: one merge request, the
    // project's own approval rules, no path around review (AP-10, AP-55).
    let manifest = app_manifest(&run, publish_source(&state, &run).await?);
    let operation = match state.mirror.get(&project, "App", &run.app_name) {
        Some(_) => crate::change::Operation::Update,
        None => crate::change::Operation::Create,
    };
    let outcome = crate::api::mutate::propose_with_identity(
        &user.0.identity,
        &state,
        &project,
        "apps",
        Some(&run.app_name),
        operation,
        false,
        manifest,
    )
    .await?;
    let crate::api::mutate::ProposeOutcome::Change(change) = outcome else {
        return Err(ApiError::Internal(
            "publishing proposed a dry run instead of a Change".into(),
        ));
    };
    // The run remembers its Change, so its page shows it and approves it in place (AP-71).
    if let Ok(number) = crate::api::changes::parse_change_id(&change.metadata.name) {
        if let Ok(number) = i32::try_from(number) {
            state
                .agents
                .set_merge_request(&id, number)
                .await
                .map_err(unavailable)?;
        }
    }
    let response = (StatusCode::ACCEPTED, Json(change)).into_response();

    if !waiting {
        state
            .agents
            .set_status(&id, AgentRunStatus::AwaitingApproval, None)
            .await
            .map_err(status_error)?;
        lease_for_approval(&state, &id).await;
        publish_event(
            &state,
            &id,
            "status",
            status_payload(AgentRunStatus::AwaitingApproval),
        )
        .await?;
    }
    Ok(response)
}

/// Why the version on screen is not published yet (SDK-38): its tests failed, or still run in the
/// sandbox. A version with no `tests` event (the first, quick one), or whose tests were skipped
/// or could not run, is left to the build lane, which runs them again at publication (SDK-24).
fn tests_hold(preview_url: Option<&str>, events: &[AgentRunEvent]) -> Option<String> {
    let version: u64 = preview_url?
        .split_once("?v=")?
        .1
        .split('&')
        .next()?
        .parse()
        .ok()?;
    let tests = events.iter().rev().find(|event| {
        event.kind == "tests"
            && event.payload.get("version").and_then(|v| v.as_u64()) == Some(version)
    })?;
    match tests.payload.get("outcome").and_then(|o| o.as_str()) {
        Some("running") => Some(format!(
            "version {version}'s tests are still running; publish once they pass (SDK-38)"
        )),
        Some("failed") => {
            let names: Vec<String> = tests
                .payload
                .get("failures")
                .and_then(|f| f.as_array())
                .into_iter()
                .flatten()
                .take(5)
                .filter_map(|failure| {
                    let name = failure.get("name")?.as_str()?;
                    let file = failure.get("file").and_then(|f| f.as_str()).unwrap_or("");
                    Some(format!("{file} › {name}"))
                })
                .collect();
            let failed = tests
                .payload
                .get("failed")
                .and_then(|f| f.as_u64())
                .unwrap_or(0);
            Some(format!(
                "version {version} fails {failed} of its tests, so it is not published; ask the \
                 assistant to fix them first (SDK-38): {}",
                names.join("; ")
            ))
        }
        _ => None,
    }
}

/// Where the published application's source is, as its manifest says (AP-02).
///
/// A static application lives in its own repository: the merge request from the run's branch
/// into `main` is opened (or the open one reused), and the manifest names the exact commit the
/// branch points at, so the Change a reviewer approves is one commit and nothing pushed later
/// rides along (AP-77). A workspace run's source sits beside the manifest.
async fn publish_source(state: &AppState, run: &AgentRun) -> Result<serde_json::Value, ApiError> {
    if !run.in_own_repository() {
        return Ok(serde_json::json!({ "path": "./src" }));
    }
    let gitea = state.gitea.as_deref().ok_or_else(|| {
        ApiError::Unavailable(
            "no forge is configured, so the application's repository cannot be published (AP-77)"
                .into(),
        )
    })?;
    let repo = gitea.for_application(repository::name(&run.project, &run.app_name));
    let sha = match repo.branch_head(&run.branch).await {
        Ok(sha) => sha,
        Err(GitError::NotFound) => {
            return Err(ApiError::Conflict(format!(
                "run '{}' has no version in the application's repository yet: publish once a \
                 version has been committed to its branch '{}' (AP-77)",
                run.id, run.branch
            )))
        }
        Err(err) => return Err(err.into()),
    };
    // What the approval merges into `main` is this commit; a tree without the application would
    // leave the repository README-only and the published application serving nothing (T-2603).
    if !repository::holds_application(&repo, &sha).await? {
        return Err(ApiError::Conflict(format!(
            "run '{}' has committed no file of the application to its branch '{}' yet, so \
             publishing it would leave main with the README alone: publish once a version has \
             been committed (AP-77)",
            run.id, run.branch
        )));
    }
    repository::open_merge_request(&repo, run).await?;
    Ok(serde_json::json!({ "git": { "url": repo.clone_url(), "ref": sha } }))
}

/// After the Change that publishes a static application is merged: the application's merge
/// request merges at the commit the approved manifest names, and the run is `published` (AP-77).
///
/// The approval already stands when this runs, so nothing here undoes it: a merge the forge
/// refuses (the branch moved after publish, a conflict with `main`) is said on the run, where
/// the person who published it reads it, and in the log.
pub async fn merge_published_application(
    state: &AppState,
    project: &str,
    manifest: Option<&crate::resource::ResourceEnvelope>,
    approver: &str,
) {
    let Some(manifest) = manifest.filter(|manifest| manifest.kind == "App") else {
        return;
    };
    let Some(run_id) = publishing_run(Some(manifest)) else {
        return;
    };
    let Some(sha) = manifest
        .spec
        .pointer("/source/git/ref")
        .and_then(serde_json::Value::as_str)
    else {
        return;
    };
    let Some(gitea) = state.gitea.as_deref() else {
        return;
    };
    let run = match state.agents.get_run(run_id).await {
        Ok(Some(run)) if run.project == project && run.in_own_repository() => run,
        Ok(_) => return,
        Err(err) => {
            tracing::warn!(run = %run_id, error = %err, "published application's run not read");
            return;
        }
    };
    let repo = gitea.for_application(repository::name(&run.project, &run.app_name));
    match repository::merge_published(&repo, &run, sha, approver).await {
        Ok(()) => {
            if let Err(err) = state
                .agents
                .set_status(&run.id, AgentRunStatus::Published, None)
                .await
            {
                tracing::warn!(run = %run.id, error = %err, "published run not marked published");
                return;
            }
            if let Err(err) = publish_event(
                state,
                &run.id,
                "status",
                status_payload(AgentRunStatus::Published),
            )
            .await
            {
                tracing::warn!(run = %run.id, error = ?err, "published status not streamed");
            }
        }
        Err(err) => {
            tracing::warn!(run = %run.id, error = %err, "application merge request not merged");
            let text = format!(
                "The Change is approved, but the application's repository did not merge branch \
                 '{}' into main: {err}. Open the merge request in the repository, bring the \
                 branch back to the published commit {sha} or merge it there (AP-77).",
                run.branch
            );
            if let Err(err) = publish_event(
                state,
                &run.id,
                "thought",
                serde_json::json!({ "text": text }),
            )
            .await
            {
                tracing::warn!(run = %run.id, error = ?err, "merge refusal not streamed");
            }
        }
    }
}

/// The run an `App` manifest was published by, as its annotation names it; `None` for any other
/// manifest.
pub fn publishing_run(manifest: Option<&crate::resource::ResourceEnvelope>) -> Option<&str> {
    manifest
        .filter(|manifest| manifest.kind == "App")?
        .metadata
        .annotations
        .get(AGENT_RUN_ANNOTATION)
        .map(String::as_str)
}

/// After the Change that publishes an application is rejected, or closed on the forge without
/// being merged: the run that proposed it stops waiting for an approval nothing can give any more,
/// so the catalogue stops showing it as waiting (T-3015). It ends `cancelled` with `reason`, and
/// the status event says why.
///
/// Only the run whose own Change this is: the same project, the Change number the run recorded
/// when it published, and still waiting for approval. Any other run is left as it is, so a
/// Change can never end a run it does not belong to. Returns whether the run ended.
pub async fn end_rejected_publication(
    state: &AppState,
    project: &str,
    run_id: &str,
    change_number: u64,
    reason: &str,
) -> bool {
    let run = match state.agents.get_run(run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return false,
        Err(err) => {
            tracing::warn!(run = %run_id, error = %err, "rejected application's run not read");
            return false;
        }
    };
    let own_change = run
        .merge_request
        .and_then(|number| u64::try_from(number).ok())
        == Some(change_number);
    let waiting = AgentRunStatus::parse(&run.status) == Some(AgentRunStatus::AwaitingApproval);
    if run.project != project || !own_change || !waiting {
        return false;
    }
    match end_run(state, &run, AgentRunStatus::Cancelled, reason).await {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(run = %run.id, error = ?err, "rejected application's run not ended");
            false
        }
    }
}

/// The `App` manifest a published run leaves behind (AP-01, AP-51).
///
/// Built from the run rather than from anything the workspace wrote: the agent's commits are the
/// application's source, and what the platform deploys is derived from the request a person
/// approved. `dataNeeds` is the list that was checked against the endpoint before the run
/// started, so the manifest cannot widen what the application may reach.
fn app_manifest(run: &AgentRun, source: serde_json::Value) -> serde_json::Value {
    let mut manifest = serde_json::json!({
        "apiVersion": crate::resource::API_VERSION,
        "kind": "App",
        "metadata": {
            "name": run.app_name,
            "namespace": run.project,
            "annotations": {
                // What a reviewer of the merge request needs to find the conversation the
                // application came out of, and to see that the prompt has not been edited since.
                AGENT_RUN_ANNOTATION: run.id,
                PROMPT_DIGEST_ANNOTATION: run.prompt_digest,
            },
        },
        "spec": {
            // A run recorded before AP-124 holds the old name; the manifest gets the new one.
            "kind": jc_core::kinds::AppClass::parse(&run.app_class)
                .map_or_else(|_| run.app_class.clone(), |class| class.to_string()),
            // Where the run committed it: the application's repository at the commit it
            // published, or beside the manifest (`path_prefix`).
            "source": source,
            "build": build_toolchains(&run.app_class),
            "visibility": run.visibility,
            "lifecycle": "published",
            "dataNeeds": run.data_needs,
        },
    });
    // A role a need names must be declared (AP-91); its title and members are added on the App
    // page afterwards, each through a Change.
    let roles = declared_roles(
        run.data_needs
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
    );
    // The server half keeps its table in the App's own schema, which the template's migration
    // creates (AP-148, T-3575).
    if jc_core::kinds::AppClass::parse(&run.app_class) == Ok(jc_core::kinds::AppClass::Wasm) {
        manifest["spec"]["storage"] = serde_json::json!({
            "sql": { "migrations": "migrations", "quotaMiB": WASM_SQL_QUOTA_MIB },
        });
    }
    if !roles.is_empty() {
        manifest["spec"]["roles"] = roles
            .into_iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect();
    }
    manifest
}

/// The toolchains CI pins for a generated application (AP-11). A `ui` application is a Vite
/// build and nothing else; a `ui-rust` one carries a Rust binary with the build embedded.
pub(crate) fn build_toolchains(app_class: &str) -> serde_json::Value {
    if jc_core::kinds::AppClass::parse(app_class) == Ok(jc_core::kinds::AppClass::Ui) {
        serde_json::json!({ "node": NODE_TOOLCHAIN })
    } else {
        serde_json::json!({ "rust": RUST_TOOLCHAIN, "node": NODE_TOOLCHAIN })
    }
}

// ---------------------------------------------------------------------------------------------

/// The run, if it is this project's. A run of another project is a 404 rather than a 403: the
/// caller has no business learning that the id exists.
async fn run_of(state: &AppState, project: &str, id: &str) -> Result<AgentRun, ApiError> {
    match state.agents.get_run(id).await.map_err(unavailable)? {
        Some(run) if run.project == project => Ok(run),
        _ => Err(ApiError::NotFound(format!(
            "run '{id}' not found in project '{project}'"
        ))),
    }
}

/// The run of a caller who may see it: the person who started it, or a caller who may approve in
/// the project — the rule `list_runs` already applies to the listing (AG-43, AG-45, PF-50). A run
/// keeps the identity of whoever created it and acts with their grants, so a second person who
/// learns the id (a shared link, a published App's annotation) must not read its conversation or
/// send it an instruction. Refused as `404`, like a run of another project: whether the id exists
/// is not the caller's business (T-0801).
async fn own_run_of(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    id: &str,
) -> Result<AgentRun, ApiError> {
    let run = run_of(state, project, id).await?;
    if run.created_by == user.0.identity.username
        || crate::api::changes::may_approve_anything(state, &user.0.identity, project).is_ok()
    {
        return Ok(run);
    }
    Err(ApiError::NotFound(format!(
        "run '{id}' not found in project '{project}'"
    )))
}

/// [`own_run_of`] for a write (PF-50, T-2486): the caller's live bindings in the project are
/// checked before the run is looked up, as the ops door checks `jc_run_message` and its siblings.
/// Having started the run is not a grant: a person whose binding was removed writes nothing to it,
/// and a stranger hears the same `403` here as there, before any id is looked up. A conversation
/// asks what starting one asked (AG-67); every other run writes an App.
async fn own_run_to_write(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    id: &str,
) -> Result<AgentRun, ApiError> {
    let grants = crate::permissions::for_request(state, &user.0.identity, project);
    grants.check_converses(project)?;
    let run = own_run_of(state, user, project, id).await?;
    if run.kind != "conversation" {
        grants.check("App", jc_core::kinds::Verb::Propose, None)?;
    }
    Ok(run)
}

/// Records one event and hands it to every connected stream.
pub(crate) async fn publish_event(
    state: &AppState,
    run_id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Result<AgentRunEvent, ApiError> {
    if kind == "tool" {
        crate::telemetry::tool_step(&payload);
    }
    let event = state
        .agents
        .append_event(run_id, kind, payload)
        .await
        .map_err(unavailable)?;
    state.agent_events.broadcast(&event).await;
    Ok(event)
}

fn sse(event: &AgentRunEvent) -> Event {
    Event::default()
        .id(event.seq.to_string())
        .event(event.kind.clone())
        .data(frame_data(event).to_string())
}

/// The data of one event's frame: its payload with `seq` and `timestamp`.
fn frame_data(event: &AgentRunEvent) -> serde_json::Value {
    let mut data = event.payload.clone();
    // The sequence number is on the frame as its id and in the body, because the API examples
    // show a client reading `seq` out of the payload without parsing the frame (API/04 §4).
    // Every frame carries the instant the Portal recorded it, so a client reads where a run's
    // seconds went from the stream alone (T-2771); a `status` frame's own is that instant too.
    if let Some(object) = data.as_object_mut() {
        object.insert("seq".to_owned(), serde_json::json!(event.seq));
        object
            .entry("timestamp")
            .or_insert_with(|| serde_json::json!(event.created_at));
    }
    data
}

pub(crate) fn status_payload(status: AgentRunStatus) -> serde_json::Value {
    serde_json::json!({ "status": status.as_str(), "timestamp": now_rfc3339() })
}

pub(crate) fn terminal(run: &AgentRun) -> bool {
    AgentRunStatus::parse(&run.status).is_some_and(|status| status.is_terminal())
}

pub(crate) fn agent_settings(state: &AppState) -> Result<&AgentSettings, ApiError> {
    state.config.agent_settings.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no agent runner configured (AG-33)".into())
    })
}

/// A run that proposed waits for its change's approval on the approval's lease, not on what was
/// left of the build's wall clock (T-2772): the reaper reads `expires_at`, and an approver has
/// days. A store that refuses leaves the old lease, and the run expires as it did before.
pub(crate) async fn lease_for_approval(state: &AppState, run_id: &str) {
    let Some(settings) = state.config.agent_settings.as_ref() else {
        return;
    };
    let until = expiry(settings.approval_ttl_secs);
    if let Err(err) = state.agents.set_expiry(run_id, &until).await {
        tracing::warn!(run = %run_id, error = %err, "the approval lease was not given");
    }
}

pub(crate) fn expiry(ttl_secs: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::seconds(ttl_secs))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

pub(crate) fn unavailable(err: StoreError) -> ApiError {
    ApiError::Unavailable(err.to_string())
}

pub(crate) fn status_error(err: StatusChangeError) -> ApiError {
    match err {
        StatusChangeError::Unknown => ApiError::NotFound("no such run".into()),
        StatusChangeError::Refused { .. } => ApiError::Conflict(err.to_string()),
        StatusChangeError::Store(store) => unavailable(store),
    }
}

/// The project routes, under `/api/v1`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/agent-runs",
            get(list_runs).post(create_run),
        )
        .route("/projects/{project}/agent-runs/{id}", get(get_run))
        .route(
            "/projects/{project}/agent-runs/{id}/events",
            get(stream_events),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/answers",
            post(answer_question),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/messages",
            post(post_message),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/preview-errors",
            post(post_preview_error),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/preview-observations",
            post(post_preview_observation),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/functions/{fn}",
            post(call_function),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/cancel",
            post(cancel_run),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/publish",
            post(publish_run),
        )
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs/{id}/preview",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run identifier"),
    ),
    responses(
        (status = 200, description = "One document: a code run's interface on the SDK runtime", content_type = "text/html"),
        (status = 400, description = "A code run's files do not build: every problem with file and line", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run, or no pass has written files yet", body = ProblemDetails),
        (status = 503, description = "This Portal was built without the SDK runtime", body = ProblemDetails)
    )
)]
/// The preview of a run in one document, because the frame it is shown in has no origin to
/// fetch anything else with (AP-50, AP-60, UI-41): a code run's `src/**` transpiled onto the SDK
/// runtime (SDK-16).
pub async fn preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    let code: BTreeMap<String, String> = run
        .files
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(path, _)| path.starts_with("src/") || path.starts_with("functions/"))
        .filter_map(|(path, text)| Some((path.clone(), text.as_str()?.to_owned())))
        .collect();
    // A run of the retired spec.json kit (T-0681) has no code and so no preview.
    if code.is_empty() {
        return Err(ApiError::NotFound(format!("run '{id}' has no preview yet")));
    }
    code_preview(&state, &run, &code, &headers)
}

/// One rendered preview: its ETag, and the policy and document it answers with.
type RenderedPreview = (String, std::sync::Arc<(String, String)>);

/// Rendered code previews by the digest of what they are made of, newest last: reopening a run
/// serves the document it already rendered instead of transpiling every file again.
static RENDERED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::VecDeque<RenderedPreview>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::VecDeque::new()));

/// How many rendered previews the Portal keeps.
const RENDERED_KEEP: usize = 16;

/// A code run's document: the SDK configured for the bridge on the run's endpoint, the
/// basemap route the one address it may connect to (SDK-16, AP-63, AP-67).
fn code_preview(
    state: &AppState,
    run: &AgentRun,
    files: &BTreeMap<String, String>,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let space = state
        .mirror
        .get(&run.project, "Endpoint", &run.endpoint_name)
        .and_then(|env| crate::api::assistant::ref_name(&env.spec["contextSpaceRef"]))
        .unwrap_or_else(|| run.project.clone());
    let mut config = serde_json::json!({
        "slug": run.endpoint_slug,
        "orgDomain": crate::api::assistant::org_domain(state, &run.project),
        "space": space,
        "transport": "bridge",
        "appName": run.app_name,
        "endpointName": run.endpoint_name,
    });
    let run_endpoints = crate::agents::endpoints::of_run(run);
    if run_endpoints.len() > 1 {
        config["endpoints"] = crate::agents::endpoints::config(&run_endpoints, &run.data_needs);
    }
    if let Some(url) = crate::api::basemap::style_url(&state.config, &run.project) {
        config["basemap"] = serde_json::Value::String(url);
    }
    let route = crate::api::basemap::route_prefix(&state.config, &run.project);
    // The document is a function of the files, the configuration and the route: its digest is
    // the ETag, so a browser reopening an unchanged version gets a 304 and the Portal a hit.
    let etag = {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        for (path, content) in files {
            hash.update(path.as_bytes());
            hash.update([0]);
            hash.update(content.as_bytes());
            hash.update([0]);
        }
        hash.update(config.to_string().as_bytes());
        hash.update(route.as_deref().unwrap_or_default().as_bytes());
        hash.update(env!("CARGO_PKG_VERSION").as_bytes());
        format!("\"{:x}\"", hash.finalize())
    };
    let cached = RENDERED.lock().ok().and_then(|kept| {
        kept.iter()
            .find(|(key, _)| *key == etag)
            .map(|(_, doc)| doc.clone())
    });
    if cached.is_some()
        && headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(',').any(|tag| tag.trim() == etag))
    {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, "private, no-cache".to_owned()),
            ],
        )
            .into_response());
    }
    if let Some(doc) = cached {
        return Ok(preview_response(&etag, &doc.1, doc.0.clone()));
    }
    let document =
        preview::document(files, &run.app_name, &config, route.as_deref()).map_err(|refusal| {
            match refusal {
                preview::Refusal::NoRuntime => ApiError::Unavailable(
                    "this Portal was built without the SDK runtime (sdk/dist/runtime is empty)"
                        .into(),
                ),
                preview::Refusal::Problems(problems) => ApiError::Invalid {
                    detail: format!(
                        "the run's files do not build a preview: {} problem(s)",
                        problems.len()
                    ),
                    errors: problems.iter().map(ToString::to_string).collect(),
                },
            }
        })?;
    if let Ok(mut kept) = RENDERED.lock() {
        kept.push_back((
            etag.clone(),
            std::sync::Arc::new((document.html.clone(), document.csp.clone())),
        ));
        while kept.len() > RENDERED_KEEP {
            kept.pop_front();
        }
    }
    Ok(preview_response(&etag, &document.csp, document.html))
}

/// A rendered preview: the page, its script policy, and a validator instead of `no-store`.
fn preview_response(etag: &str, csp: &str, html: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp.to_owned()),
            (header::X_FRAME_OPTIONS, "SAMEORIGIN".to_owned()),
            (header::CACHE_CONTROL, "private, no-cache".to_owned()),
            (header::ETAG, etag.to_owned()),
        ],
        html,
    )
        .into_response()
}

/// The preview route alone, merged beside the Portal's own routes rather than under them: the
/// document carries its own policy, and the Portal's `script-src 'self'` would override it.
pub fn preview_router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/projects/{project}/agent-runs/{id}/preview",
        get(preview),
    )
}

#[cfg(test)]
mod tests {
    use super::{app_manifest, caller_token, is_function_name, offered_answer};

    /// JC1-07: a question that offers Excel takes a sheet's CSV, never the workbook's bytes.
    #[test]
    fn an_excel_offer_takes_a_sheets_csv_and_refuses_the_workbook() {
        let question = serde_json::json!({
            "input": { "file": { "accept": ["csv", "tsv", "xlsx", "json"], "maxBytes": 1024 } }
        });
        let file = |format: &str, text: &str| serde_json::json!({ "file": { "name": "Stops.csv", "format": format, "text": text } });
        assert_eq!(
            offered_answer(&question, &file("csv", "name\r\nKamppi\r\n")),
            Ok(())
        );
        assert_eq!(
            offered_answer(&question, &file("xlsx", "PK")),
            Err("an Excel workbook is answered as the CSV of one of its sheets".to_owned())
        );
    }

    /// T-3134: the gateway's unauthenticated answer, as the SDK brings it out of a function, and
    /// nothing else, is the caller's 401.
    #[test]
    fn only_the_gateways_unauthenticated_answer_is_a_401() {
        use super::unauthenticated;
        assert!(unauthenticated(&serde_json::json!({
            "message": "Authentication Required", "file": "@joinedcontext/sdk/server", "line": 80
        })));
        assert!(!unauthenticated(&serde_json::json!({ "message": "boom" })));
        assert!(!unauthenticated(
            &serde_json::json!({ "message": "authentication required in my own words" })
        ));
        assert!(!unauthenticated(&serde_json::Value::Null));
    }

    mod tests_hold {
        use super::super::{tests_hold, AgentRunEvent};
        use serde_json::{json, Value};

        fn event(kind: &str, payload: Value) -> AgentRunEvent {
            AgentRunEvent {
                run_id: "r".into(),
                seq: 0,
                kind: kind.into(),
                payload,
                created_at: String::new(),
            }
        }

        const ON_SCREEN: Option<&str> = Some("/api/v1/projects/p/agent-runs/r/preview?v=3");

        #[test]
        fn a_version_whose_tests_fail_or_still_run_is_held_and_the_failures_are_named() {
            let failed = json!({
                "version": 3, "outcome": "failed", "passed": 1, "failed": 7,
                "failures": (0..7).map(|n| json!({"file": "src/app.test.tsx", "name": format!("case {n}"), "message": "x"})).collect::<Vec<_>>(),
            });
            let held = tests_hold(ON_SCREEN, &[event("tests", failed)]).expect("held");
            assert!(held.contains("version 3 fails 7"), "{held}");
            assert!(held.contains("src/app.test.tsx › case 4"), "{held}");
            assert!(!held.contains("case 5"), "at most five named: {held}");

            let running = json!({"version": 3, "outcome": "running"});
            assert!(tests_hold(ON_SCREEN, &[event("tests", running)])
                .expect("held")
                .contains("still running"));
        }

        #[test]
        fn only_the_newest_result_of_the_version_on_screen_counts() {
            let old_failure = event(
                "tests",
                json!({"version": 2, "outcome": "failed", "failed": 1}),
            );
            let rerun = [
                event("tests", json!({"version": 3, "outcome": "running"})),
                event(
                    "tests",
                    json!({"version": 3, "outcome": "passed", "passed": 4, "failed": 0}),
                ),
            ];
            let mut events = vec![old_failure];
            events.extend(rerun);
            assert_eq!(tests_hold(ON_SCREEN, &events), None);
        }

        #[test]
        fn skipped_errored_untested_or_unknown_versions_are_left_to_the_build_lane() {
            for outcome in ["skipped", "error", "passed"] {
                let e = event("tests", json!({"version": 3, "outcome": outcome}));
                assert_eq!(tests_hold(ON_SCREEN, &[e]), None, "{outcome}");
            }
            let failed = event("tests", json!({"version": 3, "outcome": "failed"}));
            assert_eq!(tests_hold(ON_SCREEN, &[event("preview", json!({}))]), None);
            assert_eq!(tests_hold(None, std::slice::from_ref(&failed)), None);
            assert_eq!(
                tests_hold(Some("/preview"), std::slice::from_ref(&failed)),
                None
            );
            assert_eq!(
                tests_hold(Some("/preview?v=x"), std::slice::from_ref(&failed)),
                None
            );
            assert!(tests_hold(Some("/preview?v=3&t=1"), &[failed]).is_some());
        }
    }

    mod origin {
        use super::super::{listed_origin, run_origin, RunOrigin, RUN_ORIGIN_HEADER};
        use crate::auth::session::Front;
        use crate::error::ApiError;
        use axum::http::HeaderMap;

        fn headers(value: Option<&str>) -> HeaderMap {
            let mut headers = HeaderMap::new();
            if let Some(value) = value {
                headers.insert(&RUN_ORIGIN_HEADER, value.parse().expect("a header value"));
            }
            headers
        }

        #[test]
        fn only_the_journeys_own_sessions_mark_a_run_as_a_journey_s() {
            let journeys = vec!["demo.journey".to_owned()];
            for front in [Front::Portal, Front::Edge] {
                assert_eq!(
                    run_origin(&headers(Some("journey")), front, "demo.journey", &journeys).ok(),
                    Some(RunOrigin::Journey)
                );
                assert_eq!(
                    run_origin(&headers(None), front, "demo.journey", &journeys).ok(),
                    Some(RunOrigin::Person)
                );
                // A person, an administrator included, cannot hide their runs behind the mark.
                assert!(matches!(
                    run_origin(&headers(Some("journey")), front, "demo.admin", &journeys),
                    Err(ApiError::Denied(message)) if message.contains("live journeys")
                ));
                // No journey users configured: nobody marks a run.
                assert!(matches!(
                    run_origin(&headers(Some("journey")), front, "demo.journey", &[]),
                    Err(ApiError::Denied(_))
                ));
            }
            // A service, a script or an agent is refused, even under a journey user's name.
            assert!(matches!(
                run_origin(
                    &headers(Some("journey")),
                    Front::Bearer,
                    "demo.journey",
                    &journeys
                ),
                Err(ApiError::Denied(_))
            ));
            assert_eq!(
                run_origin(&headers(None), Front::Bearer, "demo.admin", &journeys).ok(),
                Some(RunOrigin::Person)
            );
            for value in ["person", "Journey", "", "journey "] {
                assert!(
                    matches!(
                        run_origin(
                            &headers(Some(value)),
                            Front::Portal,
                            "demo.journey",
                            &journeys
                        ),
                        Err(ApiError::BadRequest(_))
                    ),
                    "{value:?}"
                );
            }
        }

        #[test]
        fn a_list_shows_people_s_runs_unless_asked_and_a_journey_everything() {
            let person = Some("person".to_owned());
            assert_eq!(
                listed_origin(None, RunOrigin::Person).ok(),
                Some(person.clone())
            );
            assert_eq!(listed_origin(None, RunOrigin::Journey).ok(), Some(None));
            assert_eq!(
                listed_origin(Some("person"), RunOrigin::Journey).ok(),
                Some(person)
            );
            assert_eq!(
                listed_origin(Some("journey"), RunOrigin::Person).ok(),
                Some(Some("journey".to_owned()))
            );
            assert_eq!(
                listed_origin(Some("all"), RunOrigin::Person).ok(),
                Some(None)
            );
            assert!(listed_origin(Some("everything"), RunOrigin::Person).is_err());
        }
    }

    fn published(data_needs: serde_json::Value) -> serde_json::Value {
        published_as("static", data_needs)
    }

    fn published_as(class: &str, data_needs: serde_json::Value) -> serde_json::Value {
        let mut run: super::AgentRun = serde_json::from_value(serde_json::json!({
            "id": "e3b0c442", "project": "helsinki", "appName": "alerts-desk",
            "endpointName": "helsinki-alerts", "endpointSlug": "si6epqkx",
            "profile": "app-builder", "kind": "application", "appClass": "static",
            "visibility": "project", "prompt": "a desk", "promptDigest": "sha256:abc",
            "unattended": false, "dataNeeds": [], "allowsWrite": true,
            "branch": "agent/app-alerts-desk/e3b0c442", "pathPrefix": "projects/helsinki/apps/alerts-desk/",
            "ticketHash": "", "steps": 0, "tokensUsed": 0,
            "status": "published", "createdBy": "demo.steward@hel.fi",
            "createdAt": "2026-09-23T05:00:00Z", "expiresAt": "2026-09-23T05:20:00Z",
        }))
        .unwrap_or_else(|e| panic!("run fixture: {e}"));
        run.data_needs = data_needs;
        run.app_class = class.to_owned();
        app_manifest(
            &run,
            serde_json::json!({ "git": { "url": "https://git.example/hel/alerts-desk.git", "ref": "0123456789abcdef0123456789abcdef01234567" } }),
        )
    }

    fn need(roles: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "types": ["Alert"],
            "operations": ["queryEntity", "updateAttrs"],
            "roles": roles,
        })
    }

    /// T-3575, AP-148, AP-151: a generated `wasm` App publishes with both toolchains and its own
    /// schema from the template's migrations, and jc-core accepts it; a `ui` App gets neither.
    #[test]
    fn a_wasm_run_publishes_an_app_with_its_schema_and_both_toolchains() {
        let manifest = published_as("wasm", serde_json::json!([need(&[])]));
        assert_eq!(manifest["spec"]["kind"], "wasm");
        assert_eq!(
            manifest["spec"]["build"],
            serde_json::json!({ "rust": crate::api::agent_runs::RUST_TOOLCHAIN, "node": crate::api::agent_runs::NODE_TOOLCHAIN })
        );
        assert_eq!(
            manifest["spec"]["storage"],
            serde_json::json!({ "sql": { "migrations": "migrations", "quotaMiB": 100 } })
        );
        let checked = jc_core::registry::validate_yaml("App", &manifest.to_string())
            .unwrap_or_else(|| panic!("App has a jc-core type"));
        assert!(checked.is_ok(), "{checked:?}");
        let ui = published_as("ui", serde_json::json!([need(&[])]));
        assert!(ui["spec"].get("storage").is_none(), "{ui}");
    }

    #[test]
    fn a_role_scoped_need_publishes_an_app_that_declares_the_role() {
        // T-2666: before, publish wrote dataNeeds[].roles and no spec.roles, and the App kind
        // refused the manifest (AP-91, AP-96).
        let manifest = published(serde_json::json!([
            need(&["steward"]),
            need(&["steward", "editor"])
        ]));
        assert_eq!(
            manifest["spec"]["roles"],
            serde_json::json!([{ "name": "steward" }, { "name": "editor" }])
        );
        let checked = jc_core::registry::validate_yaml("App", &manifest.to_string())
            .unwrap_or_else(|| panic!("App has a jc-core type"));
        assert!(checked.is_ok(), "{checked:?}");
    }

    #[test]
    fn needs_without_roles_publish_no_roles_field() {
        let manifest = published(serde_json::json!([need(&[])]));
        assert!(manifest["spec"].get("roles").is_none(), "{manifest}");
    }
    use axum::http::{header, HeaderMap, HeaderValue};

    #[test]
    fn a_function_name_is_lowercase_letters_digits_and_dashes() {
        for name in ["summary", "a", "count-by-type-2", &"a".repeat(40)] {
            assert!(is_function_name(name), "{name}");
        }
        for name in [
            "",
            "Summary",
            "2x",
            "-x",
            "a_b",
            "a.b",
            "..",
            &"a".repeat(41),
        ] {
            assert!(!is_function_name(name), "{name}");
        }
    }

    #[test]
    fn the_callers_token_is_the_bearer_or_the_trusted_edge_header_never_a_cookie() {
        let mut config = crate::config::Config::for_tests();
        let edge = |headers: &[(&str, &str)]| {
            let mut map = HeaderMap::new();
            for (name, value) in headers {
                map.insert(
                    header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    HeaderValue::from_str(value).unwrap(),
                );
            }
            map
        };
        let state = crate::state::AppState::new(config.clone(), None);
        assert_eq!(
            caller_token(&state, &edge(&[("authorization", "Bearer abc")])).as_deref(),
            Some("abc")
        );
        assert_eq!(
            caller_token(&state, &edge(&[("x-access-token", "edge")])),
            None
        );
        assert_eq!(
            caller_token(&state, &edge(&[("cookie", "__Host-jc_session=x")])),
            None
        );
        assert_eq!(
            caller_token(&state, &edge(&[("authorization", "Basic abc")])),
            None
        );
        // F-R2POR1-015 (T-6225): read as persons_token reads it, an empty bearer is no token.
        assert_eq!(
            caller_token(&state, &edge(&[("authorization", "Bearer    ")])),
            None
        );
        config.trust_edge_token = true;
        let state = crate::state::AppState::new(config, None);
        assert_eq!(
            caller_token(&state, &edge(&[("x-access-token", "edge")])).as_deref(),
            Some("edge")
        );
    }
}

#[cfg(test)]
mod frame_tests {
    use super::{frame_data, AgentRunEvent};

    fn event(kind: &str, payload: serde_json::Value) -> AgentRunEvent {
        AgentRunEvent {
            run_id: "run-1".into(),
            seq: 7,
            kind: kind.into(),
            payload,
            created_at: "2026-09-25T08:14:03.412Z".into(),
        }
    }

    /// T-2771: every frame carries the instant the Portal recorded it, whatever its kind, and a
    /// `status` frame keeps its own.
    #[test]
    fn every_frame_carries_its_timestamp() {
        for kind in [
            "tool", "thought", "usage", "navigate", "message", "partial", "path",
        ] {
            let data = frame_data(&event(kind, serde_json::json!({ "text": "x" })));
            assert_eq!(data["timestamp"], "2026-09-25T08:14:03.412Z", "{kind}");
            assert_eq!(data["seq"], 7, "{kind}");
            assert_eq!(data["text"], "x", "{kind}");
        }
        let status = frame_data(&event(
            "status",
            serde_json::json!({ "status": "interviewing", "timestamp": "2026-09-25T08:14:03.400Z" }),
        ));
        assert_eq!(status["timestamp"], "2026-09-25T08:14:03.400Z");
    }
}

#[cfg(test)]
mod conversation_profile_tests {
    use super::{conversation_profile, CHAT_PROFILE};
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use crate::store::Mirror;

    /// T-2770: a conversation runs on the organization's chat profile, and on the app-builder's
    /// while the organization has none, so a Portal ahead of its configuration still answers.
    #[test]
    fn a_conversation_runs_on_the_chat_profile_where_the_organization_has_one() {
        let mirror = Mirror::new();
        assert_eq!(conversation_profile(&mirror), "app-builder");
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "AgentProfile".into(),
            metadata: ObjectMeta {
                name: CHAT_PROFILE.into(),
                namespace: Some(crate::api::blueprints::ORG_NAMESPACE.into()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        });
        assert_eq!(conversation_profile(&mirror), "chat");
    }
}

#[cfg(test)]
mod invocation_tests {
    use super::{invocation_body, Caller};
    use std::collections::BTreeMap;

    fn body(caller: Caller) -> serde_json::Value {
        invocation_body(
            BTreeMap::from([(
                "@app/functions/summary.ts".to_owned(),
                "export default () => ({})".to_owned(),
            )]),
            "@app/functions/summary.ts".to_owned(),
            serde_json::json!({ "method": "POST" }),
            serde_json::json!({ "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa" }),
            caller,
        )
    }

    /// ADR-N-038 decision 6: the editing agent's own call carries the run's data credential and
    /// names the proxy route; a person's call carries their own token, or none, and goes to the
    /// gateway as before.
    #[test]
    fn a_runs_own_call_names_the_proxy_and_a_persons_call_does_not() {
        let run = body(Caller::Run("jcd_run-1.secret".to_owned()));
        assert_eq!(run["token"], "jcd_run-1.secret");
        assert_eq!(run["via"], "proxy");

        let person = body(Caller::Person(Some("persons-token".to_owned())));
        assert_eq!(person["token"], "persons-token");
        assert!(person.get("via").is_none());
        let anonymous = body(Caller::Person(None));
        assert!(anonymous["token"].is_null());
        assert!(anonymous.get("via").is_none());
    }
}
