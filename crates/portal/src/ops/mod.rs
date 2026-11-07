//! One operation registry behind the UI, the API, the assistant, and MCP (AG-59, ADR-N-021).
//!
//! Every action the Portal offers is an operation: a name, input/output schemas,
//! behaviour annotations, required roles/verbs, and an interaction lane (CC-70).
//! REST and MCP surfaces act as thin adapters over these same functions (CC-48).
//!
//! Note on drafts (AG-61), verdict gating (AG-62) and elicitation (AG-63): these are
//! later increments (T-0638/T-0639). Proposal operations currently return the planned Change
//! envelope directly since human review on the merge request serves as the gate.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

pub mod access_tools;
pub mod admin;
pub(crate) mod bounds;
pub mod changes;
pub mod compute;
pub mod data_query;
pub mod data_views;
pub mod data_write;
pub mod drafts;
pub mod endpoint_reach;
pub mod feed_shape;
pub mod knowledge;
pub mod model_catalog;
pub mod mqtt;
pub mod people;
pub mod pipeline_runs;
pub mod pipeline_steps;
pub mod pipeline_templates;
pub mod plans;
pub mod policy_names;
pub mod previews;
pub mod proposals;
pub mod replace_check;
pub mod resources;
pub mod runs;
pub mod schemas;
pub mod space_complete;
pub mod space_reads;
pub mod sync_sources;
pub mod verdict;
pub mod views;
pub mod workspaces;

pub use bounds::PAGE_BYTES;
pub(crate) use bounds::{annotations, empty_input_schema, parse_input, verdict_schema};
pub(crate) use changes::change_schema;
pub use drafts::*;
pub(crate) use proposals::datasource_verdict;
pub(crate) use proposals::dry_run_output_schema;
pub(crate) use proposals::manifest_input_schema;
pub(crate) use proposals::ManifestInput;
pub(crate) use proposals::{check_for_kind, propose_with_optional_draft};
pub use proposals::{
    forget_check, forget_import_check, record_check, record_import_check, record_refused_check,
    verdict_for_import, verdict_for_manifest, verdict_refusal,
};
pub use verdict::*;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::change::Lane;
use crate::error::ApiError;
use crate::state::AppState;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Session,
    Bearer,
    Mcp,
    /// An assistant or application run acting for the person who started it (AG-70).
    Agent,
}

impl Via {
    /// Who last touched a draft, as the draft records it (AG-61).
    pub fn touched_kind(self) -> &'static str {
        match self {
            // A program with its own Keycloak token is a person's, like the session (T-4616).
            Via::Session | Via::Bearer => "person",
            Via::Mcp => "mcp",
            Via::Agent => "run",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Caller {
    pub identity: Identity,
    pub via: Via,
    /// What the run's `AgentProfile` grants, when this call belongs to an agent run (AG-70). The
    /// identity is always the person who started the run: an agent has no ambient access (AG-03).
    ///
    /// `None` is a person at a keyboard or a program with their own token: their bindings alone
    /// decide. A profile is the second half and only ever narrows — an operation the person may
    /// not run stays refused whether the profile names it or not.
    pub access: Option<crate::agents::access::Access>,
    /// The access token the call arrived with, at the MCP doors: what a data read is exchanged
    /// from, so the gateway decides as the person (AG-146). `None` for a session or a run.
    pub bearer: Option<Bearer>,
}

/// An access token a caller presented; its `Debug` never prints it.
#[derive(Clone)]
pub struct Bearer(pub String);

impl std::fmt::Debug for Bearer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Bearer(..)")
    }
}

impl Caller {
    /// A person at the Portal's own form, which mints a new Endpoint's slug in the browser and
    /// shows its address before the change is proposed (EP-75); every other caller leaves the
    /// slug to the platform.
    pub fn brings_its_own_slug(&self) -> bool {
        self.via == Via::Session && self.identity.client.is_none() && self.access.is_none()
    }

    pub fn new(identity: Identity, via: Via) -> Self {
        Self {
            identity,
            via,
            access: None,
            bearer: None,
        }
    }

    /// A call made inside an agent run: the person who started it, narrowed by its profile.
    pub fn for_run(identity: Identity, access: crate::agents::access::Access) -> Self {
        Self {
            identity,
            via: Via::Agent,
            access: Some(access),
            bearer: None,
        }
    }

    /// The profile's half of a call, where a caller has one (AG-70).
    pub fn grants(&self, op: &Operation) -> Result<(), OpError> {
        match &self.access {
            Some(access) if !access.names(op) => Err(OpError::Api(ApiError::Denied(
                crate::agents::access::refusal(op.name),
            ))),
            _ => Ok(()),
        }
    }

    /// The profile's half for this call's input (T-6366): a kind-`*` operation reaches only the
    /// kinds the profile grants, with the verb the call needs.
    pub fn grants_input(&self, op: &Operation, input: &Value) -> Result<(), OpError> {
        match self
            .access
            .as_ref()
            .and_then(|access| access.refused_kind(op, input))
        {
            Some(kind) => Err(OpError::Api(ApiError::Denied(format!(
                "the agent profile does not grant {} on the kind {} (AG-70)",
                if op.annotations.read_only_hint {
                    "read"
                } else {
                    "propose"
                },
                kind
            )))),
            None => Ok(()),
        }
    }

    /// Whether this run may read objects of `kind`, by its profile; any other caller may, and
    /// the person's own check still applies (T-6366).
    pub fn reads_kind(&self, kind: &str) -> bool {
        self.access.as_ref().is_none_or(|access| access.reads(kind))
    }

    /// Whether this caller may run the operation at all — asked before anything is asked of a
    /// person (AG-63).
    ///
    /// A refusal that arrives after the confirmation has the person answer a question whose
    /// answer changes nothing, and it teaches an agent that approving is something it does and
    /// then fails at. The profile's half (AG-70) and the refusal no profile can lift (AG-11,
    /// AG-82) are both knowable from the caller and the operation alone, so they are decided
    /// here — and because [`listing`] filters on this function, an operation refused here is one
    /// `GET …/ops` and `tools/list` do not offer either.
    pub fn may_run(&self, op: &Operation) -> Result<(), OpError> {
        // AG-11 before AG-70: a profile that names an approval is an author's mistake, and being
        // told the profile does not grant what it plainly lists explains nothing. The true reason
        // is that no profile can grant it.
        if op.kind == "Change" && op.verb.is_some() {
            resources::refuse_agent_decision(self)?;
        }
        if op.name == "jc_workspace_propose" {
            workspaces::refuse_agent_bring_back(self)?;
        }
        if op.name == "jc_workspace_discard" {
            workspaces::refuse_agent_discard(self)?;
        }
        // The rest of what no profile can grant: another run and the answer to a run's question
        // (AG-11). Their refusal used to live in the body alone, which left `listing` — and so
        // `GET …/ops` and `tools/list` — offering a run an operation it would always be denied.
        runs::refuse_agent(self, op.name)?;
        self.grants(op)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub name: String,
    pub title: String,
    pub description: String,
    #[schema(value_type = Object)]
    pub input_schema: Value,
    #[schema(value_type = Object)]
    pub output_schema: Value,
    pub annotations: OperationAnnotations,
    pub lane: Lane,
    pub requires: Requires,
}

/// What a call of an operation needs of the caller, as [`permitted`] and [`may_try`] decide it
/// (T-5948): the verb, the kind it is held on, and who decides. Published in `GET …/ops`, in
/// `tools/list` and on the operation's OpenAPI entry, so a client sees the rule the door applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Requires {
    /// A verb of the platform's set, e.g. `propose`.
    pub verb: String,
    /// The kind the verb is held on; `*` is the kind the input names, or any kind.
    pub kind: String,
    /// `registry` (the verb above), `route` (the REST route's own check), `organization` (a right
    /// over the organization's people or projects) or `project` (a grant in the project).
    pub checked_by: String,
}

/// The verb a verbless write needs, as [`missing_right`] asks it: a delete deletes, anything else
/// proposes.
fn inferred_verb(name: &str) -> Verb {
    if name.ends_with("_delete") {
        Verb::Delete
    } else {
        Verb::Propose
    }
}

/// The rule [`permitted`] and [`may_try`] apply to `op`, in words a client reads.
pub fn requires(op: &Operation) -> Requires {
    let rule = |verb: Verb, kind: &str, by: &str| Requires {
        verb: crate::permissions::verb_name(verb).to_owned(),
        kind: kind.to_owned(),
        checked_by: by.to_owned(),
    };
    let verb_of_write = || {
        if op.annotations.read_only_hint {
            Verb::Read
        } else {
            inferred_verb(op.name)
        }
    };
    match (op.kind, op.verb) {
        (kind, Some(verb)) => rule(verb, kind, "registry"),
        _ if ORGANIZATION_OPERATIONS.contains(&op.name) => {
            rule(verb_of_write(), op.kind, "organization")
        }
        _ if resources::CHECKED_BY_THE_ROUTE.contains(&op.name) => {
            rule(verb_of_write(), op.kind, "route")
        }
        _ => rule(verb_of_write(), op.kind, "project"),
    }
}

/// The operations [`organization_right`] decides.
const ORGANIZATION_OPERATIONS: &[&str] = &[
    "jc_person_create",
    "jc_person_edit",
    "jc_person_disable",
    "jc_person_enable",
    "jc_person_sign_out",
    "jc_project_create",
];

/// A request a client can send first: the schema's own example, else each required member at
/// its first enum value, its default, or the smallest value of its type (MF-40).
pub fn example_of(schema: &Value) -> Value {
    if let Some(example) = schema["examples"].get(0).or_else(|| schema.get("example")) {
        return example.clone();
    }
    if let Some(value) = schema.get("const").or_else(|| schema.get("default")) {
        return value.clone();
    }
    if let Some(first) = schema["enum"].get(0) {
        return first.clone();
    }
    if let Some(first) = schema["oneOf"].get(0).or_else(|| schema["anyOf"].get(0)) {
        return example_of(first);
    }
    let kind = match &schema["type"] {
        Value::Array(types) => types.first().cloned().unwrap_or(Value::Null),
        other => other.clone(),
    };
    match kind.as_str() {
        Some("string") => json!("example"),
        Some("integer") | Some("number") => schema.get("minimum").cloned().unwrap_or(json!(1)),
        Some("boolean") => json!(false),
        Some("array") => {
            let least = schema["minItems"].as_u64().unwrap_or(0);
            Value::Array((0..least).map(|_| example_of(&schema["items"])).collect())
        }
        Some("null") => Value::Null,
        _ => {
            let mut object = serde_json::Map::new();
            for name in schema["required"].as_array().into_iter().flatten() {
                if let Some(name) = name.as_str() {
                    object.insert(name.to_owned(), example_of(&schema["properties"][name]));
                }
            }
            Value::Object(object)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("forbidden: missing role {0}")]
    Forbidden(String),
    #[error("invalid input at {path}: {message}")]
    InvalidInput { path: String, message: String },
    #[error("conflict: {0}")]
    Conflict(Value),
    #[error(transparent)]
    Api(#[from] ApiError),
}

impl From<serde_json::Error> for OpError {
    fn from(err: serde_json::Error) -> Self {
        OpError::Api(ApiError::Internal(err.to_string()))
    }
}

impl IntoResponse for OpError {
    fn into_response(self) -> Response {
        match self {
            // The verdict gate's and the change's own documents travel whole (T-0956).
            OpError::Conflict(val) => {
                let slug = if val.get("error") == Some(&json!("verdict_required")) {
                    "verdict-required"
                } else {
                    "conflict"
                };
                crate::error::problem_response(val, StatusCode::CONFLICT, slug)
            }
            other => other.into_api_error().into_response(),
        }
    }
}

impl OpError {
    /// The refusal as the Portal's problem document (API/09): a missing role is `forbidden`
    /// naming it, an input the schema refuses is `unprocessable` with the member's path in
    /// `errors[]`, so a client corrects that member and calls again (AG-146, T-5949).
    pub fn into_api_error(self) -> ApiError {
        match self {
            OpError::Forbidden(role) => ApiError::Denied(format!("missing role {role}")),
            OpError::InvalidInput { path, message } => ApiError::Unprocessable {
                detail: format!("the input breaks the operation's schema: {message}"),
                errors: vec![violation(&path, message)],
            },
            other => ApiError::from(other),
        }
    }
}

/// `path: message` with the JSON pointer read as the dotted path `errors[]` names (`/spec/name`
/// is `spec.name`); the message alone when it is about the whole input.
fn violation(pointer: &str, message: String) -> String {
    let path = pointer.trim_start_matches('/').replace('/', ".");
    if path.is_empty() {
        message
    } else {
        format!("{path}: {message}")
    }
}

impl From<OpError> for ApiError {
    fn from(err: OpError) -> Self {
        match err {
            OpError::Forbidden(msg) => ApiError::Denied(msg),
            OpError::InvalidInput { path, message } => ApiError::Invalid {
                detail: format!("the input breaks the operation's schema: {message}"),
                errors: vec![violation(&path, message)],
            },
            // The verdict gate's document travels whole, so a REST door answers what the
            // operation answers (T-0956).
            OpError::Conflict(val) if val.get("error") == Some(&json!("verdict_required")) => {
                ApiError::VerdictRequired(val)
            }
            OpError::Conflict(val) => ApiError::Conflict(
                val.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("conflict")
                    .to_owned(),
            ),
            OpError::Api(api) => api,
        }
    }
}

pub struct Operation {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input: fn() -> Value,
    pub output: fn() -> Value,
    pub annotations: OperationAnnotations,
    pub kind: &'static str,
    pub verb: Option<Verb>,
    pub lane: Lane,
    pub validate: fn(&Value) -> Result<(), OpError>,
    pub run: RunFn,
}

/// One operation's body: the caller, the state, the project and the validated input.
pub type RunFn =
    for<'a> fn(&'a Caller, &'a AppState, &'a str, Value) -> BoxFuture<'a, Result<Value, OpError>>;

static REGISTRY: OnceLock<Vec<Operation>> = OnceLock::new();

pub fn registry() -> &'static [Operation] {
    REGISTRY.get_or_init(init_registry)
}

pub fn find(name: &str) -> Option<&'static Operation> {
    registry().iter().find(|op| op.name == name)
}

/// The lane one call of `op` runs in. The registry names one lane per operation, which is the
/// answer for every operation but a blueprint flow: that one runs in the flow's own lane, the
/// stricter of what the blueprint declares and what it renders, so a green sandbox blueprint is
/// not asked about as yellow and one that renders a Red kind is asked about as red (AG-14,
/// CC-63). An input that does not plan keeps the registered lane; the call then refuses it.
pub fn lane_for(
    op: &Operation,
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    input: &Value,
) -> Lane {
    match op.name {
        "jc_flow_start" => {
            crate::api::blueprints::flow_lane(state, identity, project, input).unwrap_or(op.lane)
        }
        // A dry run checks the source and proposes nothing: nothing to ask the person (JC2-22).
        "jc_model_source_put" if input.get("dryRun").and_then(Value::as_bool) == Some(true) => {
            Lane::Green
        }
        _ => op.lane,
    }
}

pub async fn call(
    op: &Operation,
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: Value,
) -> Result<Value, OpError> {
    // A credential in any argument is refused before anything reads it: it would sit in the
    // model's context, the plan and the audit row (MF-24, T-5971).
    let pasted = crate::secret_scan::find(&input);
    if !pasted.is_empty() {
        return Err(OpError::Api(ApiError::SecretInArgument(pasted)));
    }
    caller.may_run(op)?;
    caller.grants_input(op, &input)?;
    permitted(op, &caller.identity, state, project)?;
    within_schema(op, &input)?;
    (op.validate)(&input)?;
    let answer = (op.run)(caller, state, project, input).await?;
    Ok(shown(caller.via, answer))
}

/// What a caller reads of an answer: a connector or an agent run puts it in a model's context,
/// so a credential a log line or a record carries is masked there; a person's own session and
/// token read it whole (T-5971).
fn shown(via: Via, answer: Value) -> Value {
    match via {
        Via::Session | Via::Bearer => answer,
        Via::Mcp | Via::Agent => crate::secret_scan::masked(&answer),
    }
}

/// The input against the schema the operation publishes (T-1659): what `tools/list` promises an
/// MCP client is what the door enforces, so an unbounded string or a member nobody named is
/// refused here, with the path of the first thing wrong, before the operation parses anything.
fn within_schema(op: &Operation, input: &Value) -> Result<(), OpError> {
    // ponytail: compiled per call; the schemas are a few dozen lines. Cache per operation when
    // a profile shows this on the hot path.
    let validator = jsonschema::validator_for(&(op.input)()).map_err(|e| {
        OpError::Api(ApiError::Internal(format!(
            "the input schema of {} does not compile: {e}",
            op.name
        )))
    })?;
    let first = validator.iter_errors(input).next().map(|error| {
        let mut path = error.instance_path().to_string();
        // A member nobody named is reported at its parent: name the member itself, so the
        // caller removes that one (AG-146, T-5949).
        if let jsonschema::error::ValidationErrorKind::AdditionalProperties { unexpected } =
            error.kind()
        {
            if let [member] = unexpected.as_slice() {
                path = format!("{path}/{member}");
            }
        }
        (path, error.to_string())
    });
    match first {
        None => Ok(()),
        Some((path, message)) => Err(OpError::InvalidInput {
            path: if path.is_empty() {
                "/".to_owned()
            } else {
                path
            },
            message,
        }),
    }
}

/// The person's half of a call (PF-50), as the REST route of the same action decides it: the
/// operation's verb on its kind; for a decision on a change, the verb on any kind, the change's own
/// kind being checked when the change is read; for a resource operation, the check its route
/// function makes (AG-77); for any other verbless operation, a grant in the project.
pub fn permitted(
    op: &Operation,
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
) -> Result<(), OpError> {
    let effective = crate::permissions::for_request(state, identity, project);
    match (op.kind, op.verb) {
        ("Change", Some(_)) => crate::api::changes::may_approve_anything(state, identity, project)?,
        (kind, Some(verb)) => effective.check(kind, verb, None)?,
        (_, None) if resources::CHECKED_BY_THE_ROUTE.contains(&op.name) => {}
        // An Endpoint shared with the caller's Organization or a group of theirs is read without
        // a right in its project (ADR-N-067 §2.4, T-6413): the gateway decides that read on the
        // caller's own token, and the operation answers a refused one as a missing Endpoint.
        (_, None) if op.name == data_query::NAME => {}
        // A read of a project the caller may not read is the answer of a project that is not
        // there (PF-59, R20); a write keeps its 403, which names what is missing (PF-50).
        (_, None) if !effective.may_read_project() && op.annotations.read_only_hint => {
            return Err(OpError::Api(ApiError::NotFound(format!(
                "project '{project}' not found"
            ))));
        }
        (_, None) if !effective.may_read_project() => {
            return Err(OpError::Api(ApiError::Denied(format!(
                "no role grants access in project {project} (PF-50)"
            ))));
        }
        (_, None) => {}
    }
    Ok(())
}

/// What is asked before a call is offered or turned into a question (AG-63): [`permitted`], and
/// for a writing operation without a verb the cheap half of its route's own check, so whoever
/// holds none of what the route asks for is not offered the tool and is refused before the person
/// is asked to confirm. `input` is the call's, when there is one: a kind it names is checked by
/// name. The route keeps its own check; [`call`] leaves the answer to it.
pub fn may_try(
    op: &Operation,
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    input: Option<&Value>,
) -> Result<(), OpError> {
    permitted(op, identity, state, project)?;
    if let Some(input) = input {
        named_subject(op.name, identity, state, project, input)?;
    }
    if op.verb.is_some() || op.annotations.read_only_hint {
        return Ok(());
    }
    if let Some(decided) = organization_right(op.name, state, identity) {
        return Ok(decided?);
    }
    let effective = crate::permissions::for_request(state, identity, project);
    // A project the caller may not read is one that is not there, as its route answers (R20).
    if !effective.may_read_project() {
        return Err(OpError::Api(ApiError::NotFound(format!(
            "project '{project}' not found"
        ))));
    }
    match missing_right(op.name, &effective, input.and_then(named_kind)) {
        None => Ok(()),
        Some(what) => Err(OpError::Api(ApiError::Denied(format!(
            "{} needs a role with {what} in project {project} (PF-50)",
            op.title
        )))),
    }
}

/// For an operation that acts on one object named by the input's `name`, the route's 404 when
/// that object is not there (T-6280): a person is not asked to confirm a run of a Pipeline that
/// is only in an open change, which would then fail. A name that is no string is the input's 422,
/// which the call answers.
fn named_subject(
    name: &str,
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    input: &Value,
) -> Result<(), OpError> {
    let Some(subject) = input.get("name").and_then(Value::as_str) else {
        return Ok(());
    };
    let found = match name {
        "jc_pipeline_run" => {
            crate::api::pipelines::readable_pipeline_for(state, identity, project, subject)
                .map(|_| ())
        }
        "jc_syncsource_sync" | "jc_syncsource_pause" => {
            crate::api::sync_sources::named(state, project, subject).map(|_| ())
        }
        _ => return Ok(()),
    };
    found.map_err(|missing| match missing {
        ApiError::NotFound(why) => OpError::Api(ApiError::NotFound(format!(
            "{why}; one still in an open change exists once that change is approved \
             (jc_change_list lists the open changes)"
        ))),
        other => OpError::Api(other),
    })
}

/// The route's check of an operation on the organization's people or projects (PF-91, PF-65),
/// or `None` for an operation inside one project.
fn organization_right(
    name: &str,
    state: &AppState,
    identity: &crate::auth::session::Identity,
) -> Option<Result<(), ApiError>> {
    use jc_core::kinds::Verb;
    let person = |verb| Some(crate::api::people::allow(state, identity, verb));
    match name {
        "jc_person_create" => person(Verb::Create),
        "jc_person_edit" => person(Verb::Update),
        "jc_person_disable" | "jc_person_enable" | "jc_person_sign_out" => person(Verb::Disable),
        "jc_project_create" => Some(crate::api::projects::may_open(state, identity)),
        _ => None,
    }
}

/// What a writing operation inside a project needs and the caller lacks, as "<verb> on <kind>":
/// a delete deletes, anything else (a draft, a proposal, a blueprint flow, an import, a workspace,
/// which answers only to who opened it with propose, API/01 §22) proposes; on the kind the input
/// names, on Project for the project's own delete, else on some kind.
fn missing_right(
    name: &str,
    effective: &crate::permissions::Effective,
    named: Option<&str>,
) -> Option<String> {
    let verb = inferred_verb(name);
    let kind = if name == "jc_project_delete" {
        Some("Project")
    } else {
        named
    };
    let held = match kind {
        Some(kind) => effective.may(kind, verb),
        None => effective.may_anything(verb),
    };
    let verb = crate::permissions::verb_name(verb);
    (!held).then(|| format!("{verb} on {}", kind.unwrap_or("some kind")))
}

/// Every kind a call's input names (`kind`, `manifest.kind`, `draft.kind`) that this Portal has.
pub(crate) fn named_kinds(input: &Value) -> impl Iterator<Item = &str> {
    ["/kind", "/manifest/kind", "/draft/kind"]
        .into_iter()
        .filter_map(|at| input.pointer(at).and_then(Value::as_str))
        .filter(|kind| crate::resource::by_kind(kind).is_some())
}

/// The kind a call's input names, when this Portal has it: an unknown one is the input's 422,
/// which the call answers.
pub(crate) fn named_kind(input: &Value) -> Option<&str> {
    input
        .get("kind")
        .or_else(|| input.pointer("/manifest/kind"))
        .or_else(|| input.pointer("/draft/kind"))
        .and_then(Value::as_str)
        .filter(|kind| crate::resource::by_kind(kind).is_some())
}

/// The operations this caller may run in the project: the ones [`may_try`] lets through.
pub fn listing(caller: &Caller, state: &AppState, project: &str) -> Vec<OperationSummary> {
    registry()
        .iter()
        .filter(|op| caller.may_run(op).is_ok())
        .filter(|op| may_try(op, &caller.identity, state, project, None).is_ok())
        .map(summary)
        .collect()
}

/// One operation as `GET …/ops`, `tools/list` and the OpenAPI document publish it.
pub fn summary(op: &Operation) -> OperationSummary {
    OperationSummary {
        name: op.name.to_string(),
        title: op.title.to_string(),
        description: op.description.to_string(),
        input_schema: (op.input)(),
        output_schema: (op.output)(),
        annotations: op.annotations,
        lane: op.lane,
        requires: requires(op),
    }
}

// ---------------------------------------------------------------------------
// Input structures with serde(deny_unknown_fields)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

fn init_registry() -> Vec<Operation> {
    let mut operations = compute::operations();
    operations.extend(data_query::operations());
    operations.extend(data_write::operations());
    operations.extend(proposals::operations());
    operations.extend(changes::operations());
    operations.extend(drafts::operations());
    operations.extend(space_complete::operations());
    operations.extend(resources::operations());
    operations.extend(runs::operations());
    operations.extend(views::operations());
    operations.extend(pipeline_steps::operations());
    operations.extend(pipeline_runs::operations());
    operations.extend(admin::operations());
    operations.extend(sync_sources::operations());
    operations.extend(knowledge::operations());
    operations.extend(mqtt::operations());
    operations.extend(workspaces::operations());
    operations.extend(people::operations());
    operations.extend(schemas::operations());
    operations.extend(plans::operations());
    operations.extend(model_catalog::operations());
    operations.extend(access_tools::operations());
    operations.extend(space_reads::operations());
    operations
}

#[cfg(test)]
mod tests {
    use super::bounds::serde_error_path_and_message;
    use super::*;

    /// JC2-22: a dry run of jc_model_source_put proposes nothing, so it is Green and the MCP
    /// door asks the person nothing; the write itself stays Yellow.
    #[test]
    fn a_model_source_dry_run_is_green_and_its_write_is_not() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let identity = Identity {
            client: None,
            subject: "f:1:ana".into(),
            username: "ana".into(),
            email: None,
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        };
        let op = find("jc_model_source_put").expect("op");
        let lane = |input: Value| lane_for(op, &identity, &state, "demo", &input);
        assert_eq!(lane(json!({ "name": "m", "dryRun": true })), Lane::Green);
        assert_eq!(lane(json!({ "name": "m", "dryRun": false })), Lane::Yellow);
        assert_eq!(lane(json!({ "name": "m" })), Lane::Yellow);
    }

    /// What is wrong with a registry entry, empty when nothing is (T-5948). A write needs a verb:
    /// one held on a kind the registry names, the route's or the organization's check, or the verb
    /// inferred on the kind its input names (`kind: "*"`). A write naming a kind without a verb would
    /// be gated on some other kind, and an input schema that is not a closed object is not one the
    /// door can hold to (T-1659).
    fn problems(op: &Operation) -> Vec<String> {
        let mut found = Vec::new();
        let checked_elsewhere = ORGANIZATION_OPERATIONS.contains(&op.name)
            || resources::CHECKED_BY_THE_ROUTE.contains(&op.name);
        if op.verb.is_none()
            && !op.annotations.read_only_hint
            && op.kind != "*"
            && !checked_elsewhere
        {
            found.push(format!("{} writes {} but names no verb", op.name, op.kind));
        }
        // The entity write's grant is the Endpoint Policy's, decided by the gateway per entity;
        // the Portal's half is reading the Endpoint (T-6389).
        if op.verb == Some(Verb::Read)
            && !op.annotations.read_only_hint
            && op.name != data_write::NAME
        {
            found.push(format!("{} writes with only the read verb", op.name));
        }
        let input = (op.input)();
        if input["type"] != "object" || input["additionalProperties"] != json!(false) {
            found.push(format!("{}'s input schema is not a closed object", op.name));
        }
        if let Err(err) = jsonschema::validator_for(&input) {
            found.push(format!(
                "{}'s input schema does not compile: {err}",
                op.name
            ));
        }
        found
    }

    #[test]
    fn every_registry_entry_is_sound() {
        let found: Vec<String> = registry().iter().flat_map(problems).collect();
        assert!(found.is_empty(), "{}", found.join("\n"));
        assert!(find("jc_schema_get").is_some(), "get_schema is registered");
    }

    /// T-5948: the rule a client reads is the gate's: a registry verb, the route's, the
    /// organization's, or the project's with the verb a write's name implies.
    #[test]
    fn each_operation_publishes_the_rule_its_gate_applies() {
        let rule = |name: &str| requires(find(name).expect(name));
        assert_eq!(
            rule("jc_endpoint_propose"),
            Requires {
                verb: "propose".into(),
                kind: "Endpoint".into(),
                checked_by: "registry".into()
            }
        );
        assert_eq!(rule("jc_resource_delete").verb, "delete");
        assert_eq!(rule("jc_resource_delete").checked_by, "route");
        assert_eq!(rule("jc_person_disable").checked_by, "organization");
        assert_eq!(rule("jc_catalog_search").verb, "read");
        assert_eq!(rule("jc_draft_drop").checked_by, "project");
        let found: Vec<String> = registry().iter().flat_map(problems).collect();
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_write_naming_a_kind_without_a_verb_is_a_problem() {
        let base = find("jc_resource_list").expect("registered");
        let write = Operation {
            name: "jc_thing_write",
            title: base.title,
            description: base.description,
            input: base.input,
            output: base.output,
            annotations: annotations(false, false, false),
            kind: "Pipeline",
            verb: None,
            lane: Lane::Yellow,
            validate: base.validate,
            run: base.run,
        };
        assert!(problems(&write)[0].contains("names no verb"));
        let read_write = Operation {
            verb: Some(Verb::Read),
            input: || json!({ "type": "object" }),
            ..write
        };
        let found = problems(&read_write);
        assert!(
            found.iter().any(|p| p.contains("only the read verb")),
            "{found:?}"
        );
        assert!(
            found.iter().any(|p| p.contains("closed object")),
            "{found:?}"
        );
    }

    #[test]
    fn an_example_is_made_from_the_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["Pipeline", "Endpoint"] },
                "name": { "type": "string" },
                "limit": { "type": "integer", "minimum": 5 },
                "dry": { "type": "boolean", "default": true },
                "needs": { "type": "array", "minItems": 2, "items": { "type": "string" } },
                "either": { "oneOf": [{ "type": "null" }, { "type": "string" }] },
                "optional": { "type": "string" }
            },
            "required": ["kind", "name", "limit", "dry", "needs", "either"]
        });
        assert_eq!(
            example_of(&schema),
            json!({
                "kind": "Pipeline", "name": "example", "limit": 5, "dry": true,
                "needs": ["example", "example"], "either": null
            })
        );
        assert_eq!(
            example_of(&json!({ "examples": [{ "q": "air" }] })),
            json!({ "q": "air" })
        );
    }

    /// T-1659, AG-64: the door enforces the schema it publishes. A bounded string past its
    /// bound and a member the schema does not name are refused with the path to them, before
    /// the operation's own parse; an input inside the schema goes through.
    #[test]
    fn an_input_outside_the_published_schema_is_refused_at_the_door() {
        let op = find("jc_catalog_search").expect("the operation is registered");
        assert!(within_schema(op, &json!({ "q": "air quality" })).is_ok());

        let long = "x".repeat(bounds::QUERY as usize + 1);
        match within_schema(op, &json!({ "q": long })) {
            Err(OpError::InvalidInput { path, .. }) => assert_eq!(path, "/q"),
            other => panic!("an over-long query went through: {other:?}"),
        }
        match within_schema(op, &json!({ "q": "a", "sql": "drop table" })) {
            Err(OpError::InvalidInput { path, message }) => {
                assert_eq!(path, "/sql", "the member itself is named");
                assert!(message.contains("sql"), "{message}");
            }
            other => panic!("an unnamed member went through: {other:?}"),
        }
    }

    /// Every published input schema compiles, or every call of that operation would fail.
    #[test]
    fn every_input_schema_compiles() {
        for op in registry() {
            assert!(
                jsonschema::validator_for(&(op.input)()).is_ok(),
                "{} publishes a schema that does not compile",
                op.name
            );
        }
    }

    #[test]
    fn registry_lists_all_operations() {
        // No count: a number every new operation edits tests nothing (T-1499). What must hold is
        // that a name means one operation, and that these are still there.
        let mut names: Vec<&str> = registry().iter().map(|op| op.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two operations share a name");
        for name in [
            "jc_catalog_search",
            "jc_endpoint_propose",
            "jc_kpi_compute",
            "jc_manifest_dry_run",
            "jc_pipeline_test",
            "jc_change_list",
            "jc_change_approve",
            "jc_datasource_check",
            "jc_model_infer",
            "jc_space_complete",
            "jc_draft_put",
            "jc_draft_get",
            "jc_draft_list",
            "jc_draft_drop",
            "jc_resource_list",
            "jc_resource_get",
            "jc_resource_propose",
            "jc_resource_delete",
            "jc_change_reject",
            "jc_pipeline_metrics",
            "jc_activity_list",
            "jc_federation_graph",
            "jc_model_source_get",
            "jc_run_create",
            "jc_run_cancel",
            "jc_run_publish",
            "jc_project_create",
            "jc_project_delete",
            "jc_project_export",
            "jc_project_import",
            "jc_model_source_put",
            "jc_change_get",
            "jc_run_list",
            "jc_run_get",
            "jc_ckan_status",
            "jc_endpoint_list_all",
            "jc_project_get",
            "jc_project_revisions",
            "jc_run_answer",
            "jc_run_message",
            "jc_flow_start",
            "jc_syncsource_status",
            "jc_syncsource_sync",
            "jc_syncsource_pause",
            "jc_syncsource_detach",
            "jc_knowledge_status",
            "jc_knowledge_search",
        ] {
            assert!(find(name).is_some(), "missing operation {name}");
        }
    }

    #[test]
    fn validate_detects_unknown_fields_and_reports_path() {
        let op = find("jc_catalog_search").expect("jc_catalog_search");
        let valid = json!({ "q": "traffic" });
        assert!((op.validate)(&valid).is_ok());

        let invalid = json!({ "q": "traffic", "extraField": 123 });
        let err = (op.validate)(&invalid).expect_err("should reject unknown field");
        match err {
            OpError::InvalidInput { path, message } => {
                assert_eq!(path, "/extraField");
                assert!(message.contains("unknown field `extraField`"));
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    /// T-5971: a key pasted into any argument is refused by path, before a permission or a
    /// schema reads it, and never echoed; a connector reads a credential in an answer masked.
    #[tokio::test]
    async fn a_pasted_secret_is_refused_at_the_door_and_an_answer_to_a_model_is_masked() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let caller = Caller::new(
            Identity {
                client: Some("mcp-local".into()),
                subject: "f:1:nobody".into(),
                username: "nobody".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            Via::Mcp,
        );
        let op = find("jc_change_plan").expect("registered");
        let input = serde_json::json!({ "changes": [{ "op": "create", "kind": "DataSource",
            "name": "feed", "manifest": { "spec": { "http": {
                "headers": { "X-Api-Key": "pasted-key-0123456789" } } } } }] });
        let refused = call(op, &caller, &state, "p", input)
            .await
            .expect_err("refused")
            .into_api_error();
        let (status, document) = refused.into_document();
        assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            document["type"],
            "https://joinedcontext.com/errors/secret-in-argument"
        );
        assert_eq!(
            document["errors"],
            serde_json::json!([{
                "path": "/changes/0/manifest/spec/http/headers/X-Api-Key",
                "message": "reads like a credential"
            }])
        );
        assert!(!document.to_string().contains("pasted-key-0123456789"));

        let log = serde_json::json!({ "lines": [
            "GET https://feed.example/x failed: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJl",
            "fetched 12 rows"
        ] });
        let masked = shown(Via::Mcp, log.clone());
        assert!(!masked.to_string().contains("eyJhbGci"));
        assert_eq!(masked["lines"][1], "fetched 12 rows");
        assert_eq!(shown(Via::Agent, log.clone()), masked);
        assert_eq!(shown(Via::Session, log.clone()), log);
    }

    #[test]
    fn listing_filters_by_permission() {
        use crate::config::Config;
        use crate::permissions::ORG_NAMESPACE;
        use crate::resource::ResourceEnvelope;

        let config = Config::for_tests();
        let state = AppState::new(config, None);

        // Principal without bindings
        let viewer_id = Identity {
            client: None,
            subject: "f:1:viewer".into(),
            username: "viewer".into(),
            email: Some("viewer@example.sk".into()),
            name: None,
            roles: vec!["portal-viewer".into()],
            groups: vec![],
            second_factor: false,
        };
        let viewer_caller = Caller::new(viewer_id, Via::Session);

        // Without a binding: the data read, which the gateway decides on an Endpoint shared with
        // the caller (ADR-N-067, T-6413), and the reads among the resource operations, which check
        // the caller as their routes do; a write their route would refuse is not offered (AG-63,
        // T-5608).
        let list = listing(&viewer_caller, &state, "ovzdusie");
        let names: Vec<_> = list.iter().map(|o| o.name.as_str()).collect();
        let reads: Vec<_> = std::iter::once(data_query::NAME)
            .chain(
                resources::CHECKED_BY_THE_ROUTE
                    .iter()
                    .copied()
                    .filter(|name| find(name).is_some_and(|op| op.annotations.read_only_hint)),
            )
            .collect();
        assert_eq!(names, reads);

        // Add role & binding for pipeline-developer
        let role = ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Role".into(),
            metadata: crate::resource::ObjectMeta::new("developer", ORG_NAMESPACE),
            spec: json!({
                "rules": [
                    { "kinds": ["Pipeline", "DataSource"], "verbs": ["propose"] }
                ]
            }),
            status: None,
        };
        let binding = ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "RoleBinding".into(),
            metadata: crate::resource::ObjectMeta::new("dev-binding", ORG_NAMESPACE),
            spec: json!({
                "role": "developer",
                "subjects": [{ "group": "devs" }],
                "scope": { "project": "ovzdusie" }
            }),
            status: None,
        };
        state.mirror.upsert(role);
        state.mirror.upsert(binding);

        let dev_id = Identity {
            client: None,
            subject: "f:1:dev".into(),
            username: "dev".into(),
            email: Some("dev@example.sk".into()),
            name: None,
            roles: vec![],
            groups: vec!["devs".into()],
            second_factor: false,
        };
        let dev_caller = Caller::new(dev_id, Via::Session);

        let dev_list = listing(&dev_caller, &state, "ovzdusie");
        let names: Vec<_> = dev_list.iter().map(|o| o.name.as_str()).collect();

        // Should include read-only ops and the one propose, which names Pipeline or DataSource
        assert!(names.contains(&"jc_catalog_search"));
        assert!(names.contains(&"jc_resource_propose"));
        // Should NOT include propose ops for kinds not granted
        assert!(!names.contains(&"jc_endpoint_propose"));
        assert!(!names.contains(&"jc_change_approve"));
    }

    /// The writing operations without a verb of one family, as `listing` offers them.
    fn offered(caller: &Caller, state: &AppState, family: &[&str]) -> Vec<String> {
        listing(caller, state, "ovzdusie")
            .into_iter()
            .map(|op| op.name)
            .filter(|name| family.contains(&name.as_str()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// T-5608, AG-63: each writing operation without a verb is offered, and its call let through
    /// to the question, only to a caller holding what its route checks: a viewer gets none, an
    /// editor the ones that write what they propose, an organization administrator all of them.
    #[test]
    fn a_writing_operation_without_a_verb_is_offered_by_what_its_route_checks() {
        use crate::config::Config;
        use crate::permissions::ORG_NAMESPACE;
        use crate::resource::ResourceEnvelope;

        const PROPOSING: &[&str] = &[
            "jc_draft_put",
            "jc_draft_drop",
            "jc_resource_propose",
            "jc_flow_start",
            "jc_project_import",
            "jc_workspace_open",
            "jc_workspace_update_from_main",
            "jc_workspace_preview_start",
            "jc_workspace_preview_stop",
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_change_apply",
        ];
        const DELETING: &[&str] = &["jc_resource_delete", "jc_project_delete"];
        const ORGANIZATION: &[&str] = &[
            "jc_project_create",
            "jc_person_create",
            "jc_person_edit",
            "jc_person_disable",
            "jc_person_enable",
            "jc_person_sign_out",
        ];
        let all: Vec<&str> = [PROPOSING, DELETING, ORGANIZATION].concat();
        // Every writing operation without a verb is in one family, so a new one gets a test.
        let unlisted: Vec<&str> = registry()
            .iter()
            .filter(|op| op.verb.is_none() && !op.annotations.read_only_hint)
            .map(|op| op.name)
            .filter(|name| !all.contains(name))
            .filter(|name| !["jc_workspace_propose", "jc_workspace_discard"].contains(name))
            .collect();
        assert_eq!(unlisted, Vec::<&str>::new());

        let state = AppState::new(Config::for_tests(), None);
        let role = |name: &str, rules: Value| ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Role".into(),
            metadata: crate::resource::ObjectMeta::new(name, ORG_NAMESPACE),
            spec: json!({ "rules": rules }),
            status: None,
        };
        let binding = |name: &str, group: &str, scope: Value| ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "RoleBinding".into(),
            metadata: crate::resource::ObjectMeta::new(name, ORG_NAMESPACE),
            spec: json!({ "role": name, "subjects": [{ "group": group }], "scope": scope }),
            status: None,
        };
        state.mirror.upsert(role(
            "viewer",
            json!([{ "kinds": ["*"], "verbs": ["read"] }]),
        ));
        state.mirror.upsert(binding(
            "viewer",
            "viewers",
            json!({ "project": "ovzdusie" }),
        ));
        state.mirror.upsert(role(
            "editor",
            json!([{ "kinds": ["Endpoint", "Pipeline"], "verbs": ["propose"] }]),
        ));
        state.mirror.upsert(binding(
            "editor",
            "editors",
            json!({ "project": "ovzdusie" }),
        ));
        let caller = |group: &str| {
            Caller::new(
                Identity {
                    client: None,
                    subject: format!("f:1:{group}"),
                    username: group.into(),
                    email: None,
                    name: None,
                    roles: vec![],
                    groups: vec![group.into()],
                    second_factor: true,
                },
                Via::Session,
            )
        };
        let viewer = caller("viewers");
        let editor = caller("editors");
        let admin = caller(&state.config.bootstrap_admins);

        assert_eq!(offered(&viewer, &state, &all), Vec::<String>::new());
        let mut proposing = PROPOSING.to_vec();
        proposing.sort_unstable();
        assert_eq!(offered(&editor, &state, &all), proposing);
        assert_eq!(offered(&admin, &state, &all).len(), all.len());

        // The call is refused before the question, with the right it misses.
        let refusal = |name: &str| {
            let op = find(name).expect("registered");
            may_try(op, &viewer.identity, &state, "ovzdusie", None)
                .expect_err("refused")
                .to_string()
        };
        assert!(refusal("jc_project_delete").contains("delete on Project"));
        assert!(refusal("jc_resource_delete").contains("delete"));
        assert!(refusal("jc_draft_put").contains("propose"));
        assert!(refusal("jc_person_create").contains("Person"));
        assert!(refusal("jc_project_create").contains("Project"));

        // A kind the call names is checked by name: the editor proposes Endpoints, not Spaces,
        // and a stranger to the project is refused there.
        let named = |caller: &Caller, kind: &str, project: &str| {
            let op = find("jc_draft_put").expect("registered");
            let input = json!({ "kind": kind, "name": "air", "manifest": {} });
            may_try(op, &caller.identity, &state, project, Some(&input)).map_err(|e| e.to_string())
        };
        assert_eq!(named(&editor, "Endpoint", "ovzdusie"), Ok(()));
        let space = named(&editor, "ContextSpace", "ovzdusie").expect_err("refused");
        assert!(space.contains("propose on ContextSpace"), "{space}");
        let stranger = named(&editor, "Endpoint", "doprava").expect_err("refused");
        assert!(stranger.contains("doprava"), "{stranger}");
        // JC2-15: a kind named only by the draft is checked by name too.
        let op = find("jc_resource_propose").expect("registered");
        let by_draft = |kind: &str| {
            let input = json!({ "draft": { "kind": kind, "name": "air" } });
            may_try(op, &editor.identity, &state, "ovzdusie", Some(&input))
                .map_err(|e| e.to_string())
        };
        assert_eq!(by_draft("Endpoint"), Ok(()));
        let space = by_draft("ContextSpace").expect_err("refused");
        assert!(space.contains("propose on ContextSpace"), "{space}");
        // The call itself leaves the answer to the route (T-5608 keeps PF-50's own sentences).
        let op = find("jc_draft_put").expect("registered");
        assert!(permitted(op, &viewer.identity, &state, "ovzdusie").is_ok());
    }

    // ---------------------------------------------------------------------------------------------
    // `serde_error_path_and_message` (T-2116; PF-51, AG-59)
    //
    // The pair a caller reads in a 422: `path` points at the field that is wrong and `message` is
    // serde's own sentence. Its contract is narrow and worth pinning, because it is the one place
    // where text a caller chose (a field name they invented) is put into an answer: the path must
    // stay one field reference, and the pair must stay something a JSON body can carry. Neither
    // half is ever logged — `parse_input` (line 387) is the only builder of `InvalidInput`, and no
    // call site logs it — so an input echoed back reaches nobody but the caller who sent it.
    // ---------------------------------------------------------------------------------------------

    /// Deserialise into a struct with `deny_unknown_fields`, the way every operation's input is.
    fn input_error(input: Value) -> serde_json::Error {
        #[derive(Debug, serde::Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        #[allow(dead_code)]
        struct Wanted {
            project: String,
            limit: u32,
        }
        serde_json::from_value::<Wanted>(input).expect_err("the input is not valid")
    }

    #[test]
    fn a_field_the_schema_does_not_have_is_pointed_at_by_name() {
        for (input, path) in [
            (
                json!({ "project": "ovzdusie", "limit": 1, "extra": 1 }),
                "/extra",
            ),
            (
                json!({ "project": "ovzdusie", "limit": 1, "Limit": 1 }),
                "/Limit",
            ),
            (
                json!({ "project": "ovzdusie", "limit": 1, "l\u{00ed}mit": 1 }),
                "/l\u{ed}mit",
            ),
        ] {
            let (found, message) = serde_error_path_and_message(&input_error(input));
            assert_eq!(found, path);
            assert!(
                message.starts_with("unknown field"),
                "the message stopped saying what was wrong: {message}",
            );
        }
    }

    #[test]
    fn a_field_the_input_left_out_is_pointed_at_by_name() {
        let (path, message) = serde_error_path_and_message(&input_error(json!({ "limit": 1 })));
        assert_eq!(path, "/project");
        assert!(message.starts_with("missing field"), "{message}");
    }

    #[test]
    fn anything_else_points_at_the_whole_input() {
        for input in [
            json!({ "project": 7, "limit": 1 }),           // the wrong type
            json!({ "project": "ovzdusie", "limit": -1 }), // out of range
            json!({ "project": "ovzdusie", "limit": 4_294_967_296_i64 }), // the bound plus one
            json!([]),                                     // not an object at all
            Value::Null,
            json!("ovzdusie"),
        ] {
            let (path, message) = serde_error_path_and_message(&input_error(input.clone()));
            assert_eq!(path, "/", "{input}");
            assert!(!message.is_empty(), "{input} produced no message");
        }
    }

    #[test]
    fn a_field_name_the_caller_invented_cannot_break_out_of_the_answer() {
        for forged in [
            "a\nb",
            "a\r\nSet-Cookie: x=y",
            "a\"b",
            "a`b",
            "a/b",
            "a~b",
            "../../etc/passwd",
            "\u{0}",
            "\u{202e}drowssap",
        ] {
            let mut input = json!({ "project": "ovzdusie", "limit": 1 });
            input[forged] = json!(1);
            let (path, message) = serde_error_path_and_message(&input_error(input));
            assert!(path.starts_with('/'), "{forged:?} produced path {path:?}");
            // The pair travels as two JSON strings, so whatever the name was it is escaped on the
            // way out and no answer can grow a header or a second field of its own.
            let body = serde_json::to_string(&json!({ "path": path, "message": message }))
                .expect("a body");
            assert!(
                !body.contains('\n') && !body.contains('\r') && !body.contains('\0'),
                "{forged:?} reached the body raw: {body}",
            );
            assert!(
                serde_json::from_str::<Value>(&body).is_ok(),
                "{forged:?} produced a body that is not JSON: {body}",
            );
            // The path is a field reference for a person to read, not an escaped JSON Pointer: a
            // name holding `/` or `~` is passed through as written. Nothing resolves it, so this is
            // the shape the UI is told to expect — see /workspace/chyby.md.
            if forged == "a/b" {
                assert_eq!(path, "/a/b");
            }
        }
    }

    #[test]
    fn the_message_carries_the_callers_own_input_and_nothing_of_the_installation() {
        // A value typed into a field of the wrong type is quoted back by serde. It is the caller's
        // own text, it reaches nobody else (no call site logs `InvalidInput`), and nothing of the
        // state — no path, no token, no project the caller cannot see — is added to it.
        let (path, message) =
            serde_error_path_and_message(&input_error(json!({ "project": "ovzdusie",
                                                              "limit": "hunter2" })));
        assert_eq!(path, "/");
        assert!(message.contains("hunter2"), "{message}");
        for never in ["ovzdusie", "/workspace", "Bearer", "postgres", "keycloak"] {
            assert!(
                !message.contains(never),
                "the message carried {never:?}: {message}",
            );
        }
    }
}
