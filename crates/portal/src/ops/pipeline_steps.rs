//! The workbench's steps as operations of the registry (ADR-N-034 §3.5, PL-63, AG-89): the
//! source's sample, the mapping tried on it and the records checked against the target space's
//! model. The workbench calls them through `POST /api/v1/projects/{project}/ops/{name}`, MCP and
//! the assistant through the registry, so a person, an agent and the assistant see the same
//! output for the same input. The fourth step, the save, is `jc_resource_propose`.
//!
//! Nothing here writes: the sample and the mapping run as the throwaway harness stream of a
//! pipeline test (PL-43, MF-38) and resolve no `secretRef`; a streaming source's sample is its
//! saved input, whose credential only the runner fills (T-4841). The check is the one the
//! runner's validation stage renders (PL-60).

use super::bounds;
use super::drafts::DraftRef;
use super::parse_input;
use super::{OpError, Operation, OperationAnnotations};
use crate::api::{datasource_test, pipeline_test};
use crate::change::Lane;
use crate::error::ApiError;
use crate::pipeline_validation::Problem;
use crate::state::AppState;
use jc_core::kinds::data_source::DataSourceSpec;
use jc_core::kinds::pipeline::Step;
use jc_core::kinds::{PipelineSpec, Verb};
use jcctl::pipeline_test::{Sample, TestError, TestTrace, MAX_MESSAGES, STAGE_RECORDS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The most records one validation takes: a page of the workbench's mapped output.
pub const MAX_RECORDS: usize = 100;

/// The bytes the per-step records of one answer carry together (T-6082): a step past them keeps
/// its counts, its errors and its records' field names, and says `truncated`.
pub const STEP_BYTES: usize = 32 * 1024;

/// How long a streaming source is listened to for its sample, at most (T-4841).
pub const STREAM_SECONDS: u64 = 5;
/// The bytes of every message one streamed sample keeps, together (T-4841).
pub const STREAM_BYTES: usize = 256 * 1024;
/// Streamed samples one person starts in a minute: each holds a broker connection for seconds.
pub const STREAM_SAMPLES_PER_MINUTE: usize = 6;

/// How a streaming source is listened to: no quiet ends the wait, a topic may send every second.
const STREAM_WINDOW: pipeline_test::Window = pipeline_test::Window {
    deadline: std::time::Duration::from_secs(STREAM_SECONDS),
    quiet: None,
    bytes: STREAM_BYTES,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SampleSourceInput {
    #[serde(default)]
    data_source: Option<String>,
    /// With `dataSource`: the open plan (jc_change_plan) that holds it, sampled before approval.
    #[serde(default)]
    plan_id: Option<String>,
    #[serde(default)]
    sample: Option<Sample>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TryMappingInput {
    #[serde(default)]
    pipeline: Option<Value>,
    #[serde(default)]
    sample: Option<Sample>,
    /// The messages a streaming source gave its sample, each tried alone (T-4841).
    #[serde(default)]
    messages: Option<Vec<Value>>,
    #[serde(default)]
    draft: Option<DraftRef>,
    /// Also what each step made of the sample (T-6082).
    #[serde(default)]
    steps: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateInput {
    #[serde(default)]
    pipeline: Option<Value>,
    #[serde(default)]
    draft: Option<DraftRef>,
    #[serde(default)]
    space: Option<String>,
    /// A type of the space's model: the answer lists which of its attributes the pipeline may
    /// write through its Endpoint (T-4842).
    #[serde(default, rename = "type")]
    entity_type: Option<String>,
    records: Vec<Value>,
}

/// One record's verdict against the model.
#[derive(Debug, Serialize)]
struct RecordVerdict {
    index: usize,
    ok: bool,
    problems: Vec<Problem>,
    /// Where its write is judged (T-4840): the Endpoint, each finding from the model, the
    /// Endpoint and the lifetime, and the ends the record would be written with.
    #[serde(skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    findings: Option<Vec<crate::record_check::Finding>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires: Option<crate::record_check::Expires>,
}

fn sample_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "description": "The sample",
        "properties": {
            "text": text("The sample inline", jcctl::pipeline_test::MAX_SAMPLE_BYTES as u64),
            "url": text("Or an http(s) URL the runner fetches", URL),
            "format": {
                "type": "string",
                "description": "How the sample is split into records",
                "enum": ["csv", "json", "text"]
            }
        },
        "additionalProperties": false
    })
}

fn sample_source_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "dataSource": text("The DataSource to read; or sample", NAME),
            "planId": text("The jc_change_plan planId adding dataSource, sampled before approval", TERM),
            "sample": sample_schema()
        },
        "additionalProperties": false
    })
}

fn sample_source_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "streamed": { "type": "boolean", "description": "Whether the source streams: its records are the messages that arrived while it was listened to, each one message, to be tried as messages" },
            "seconds": { "type": "integer", "description": "How long a streaming source was listened to, at most" },
            "records": { "type": "array", "description": "The first records the source answered, at most 20" },
            "fields": { "type": "array", "items": { "type": "string" }, "description": "Every field of those records, once each, in name order" },
            "count": { "type": "integer" },
            "truncated": { "type": "boolean", "description": "Whether the source answered more than the records shown" },
            "bytes": { "type": "integer", "description": "When truncated by size: the bytes the records held before they were cut to 16 KiB" },
            "cut": { "type": "string", "description": "When one record held a long array: its JSON pointer and how many of its items are shown" },
            "errors": { "type": "array", "description": "What the fetch or the parse said, when it failed" },
            "withheld": withheld_schema()
        }
    })
}

fn try_mapping_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "pipeline": manifest("The Pipeline manifest to try; or draft"),
            "sample": sample_schema(),
            "messages": {
                "type": "array",
                "description": "Instead of sample: a streamed sample's records, one message each",
                "maxItems": MAX_MESSAGES
            },
            "draft": draft_ref(),
            "steps": steps_flag()
        },
        "additionalProperties": false
    })
}

/// The `steps` flag of jc_pipeline_try_mapping and jc_pipeline_test (T-6082).
pub(super) fn steps_flag() -> Value {
    json!({ "type": "boolean", "description": "Per-step records" })
}

fn try_mapping_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "input": { "type": "object", "description": "What the runner read: events, bytes and the first record" },
            "records": { "type": "array", "description": "Each record as the pipeline would write it" },
            "errors": { "type": "array", "description": "Each failure with its stage, the step and the line of the mapping" },
            "withheld": withheld_schema()
        }
    })
}

/// The `withheld` member of a sample answer (AG-158).
fn withheld_schema() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Each personal field of the source left out, as DataSource/{name}.{field}"
    })
}

/// Whether the caller's answers go to a model outside the Portal: an MCP client or an agent
/// run, which do not see the fields a DataSource lists personal (AG-158).
fn hides_source_fields(caller: &super::Caller) -> bool {
    matches!(caller.via, super::Via::Mcp | super::Via::Agent)
}

/// `answer` without the personal fields of each DataSource in `sources`, for a caller that
/// does not see them.
fn without_source_fields<'a>(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    sources: impl IntoIterator<Item = &'a str>,
    mut answer: Value,
) -> Value {
    if hides_source_fields(caller) {
        for source in sources {
            let fields = crate::personal_data::source_fields(state, project, source);
            crate::personal_data::withhold_source(&mut answer, source, &fields);
        }
    }
    answer
}

/// `answer` without the personal fields the planned DataSource `name` of `spec` lists, for a
/// caller that does not see them: the mirror does not hold it before its approval (T-6269).
fn without_planned_fields(
    caller: &super::Caller,
    name: &str,
    spec: &Value,
    mut answer: Value,
) -> Value {
    if hides_source_fields(caller) {
        let fields: std::collections::BTreeSet<String> = spec
            .get("personalFields")
            .cloned()
            .and_then(|fields| serde_json::from_value(fields).ok())
            .unwrap_or_default();
        crate::personal_data::withhold_source(&mut answer, name, &fields);
    }
    answer
}

/// The DataSources a Pipeline manifest reads: `spec.source.dataSourceRef` and each
/// `spec.sources[].dataSourceRef`, by name or as a reference.
fn data_sources_of(pipeline: &Value) -> Vec<String> {
    let spec = &pipeline["spec"];
    std::iter::once(&spec["source"])
        .chain(spec["sources"].as_array().into_iter().flatten())
        .filter_map(|source| {
            let reference = &source["dataSourceRef"];
            reference
                .as_str()
                .or_else(|| reference["name"].as_str())
                .map(str::to_owned)
        })
        .collect()
}

fn validate_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "pipeline": manifest("The Pipeline whose target model checks the records; or draft or space"),
            "draft": draft_ref(),
            "space": text("The ContextSpace whose model checks the records", NAME),
            "type": text("With a pipeline or draft: a type of the space's model; writable answers which of its attributes the pipeline may write", NAME),
            "records": {
                "type": "array",
                "description": "The records jc_pipeline_try_mapping answered",
                "maxItems": MAX_RECORDS,
                "items": { "type": "object", "maxProperties": RECORD_MEMBERS }
            }
        },
        "required": ["records"],
        "additionalProperties": false
    })
}

fn validate_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": { "type": "string" },
            "model": { "type": "string" },
            "version": { "type": "string" },
            "valid": { "type": "integer" },
            "rejected": { "type": "integer" },
            "writable": {
                "type": "array",
                "description": "With type: each attribute of the type, whether the pipeline may write it through its Endpoint and, when not, why (T-4842)",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "writable": { "type": "boolean" },
                        "reason": { "type": "string" }
                    }
                }
            },
            "verdicts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "index": { "type": "integer" },
                        "ok": { "type": "boolean" },
                        "problems": { "type": "array", "description": "Each rule broken: its SHACL component, the attribute and why" },
                        "endpoint": { "type": "string", "description": "The Endpoint the record is written through" },
                        "findings": { "type": "array", "description": "Why the record would not be written: source (model, endpoint, lifetime), rule, field, reason" },
                        "expires": { "type": "object", "description": "The expiresAt of the entity and each attribute" }
                    }
                }
            }
        }
    })
}

/// Messages a streamed sample could have given: at most [`MAX_MESSAGES`], each at most the bytes
/// a sample keeps of one message, [`STREAM_BYTES`] together.
fn messages_bounded(messages: &[Value]) -> Result<(), OpError> {
    let size = |message: &Value| match message {
        Value::String(text) => text.len(),
        other => other.to_string().len(),
    };
    let refused = |message: String| OpError::InvalidInput {
        path: "/messages".into(),
        message,
    };
    if messages.is_empty() || messages.len() > MAX_MESSAGES {
        return Err(refused(format!("give 1 to {MAX_MESSAGES} messages")));
    }
    if let Some(at) = messages
        .iter()
        .position(|m| size(m) > datasource_test::RECORD_BYTES)
    {
        return Err(OpError::InvalidInput {
            path: format!("/messages/{at}"),
            message: format!(
                "a message is at most {} bytes",
                datasource_test::RECORD_BYTES
            ),
        });
    }
    if messages.iter().map(size).sum::<usize>() > STREAM_BYTES {
        return Err(refused(format!(
            "the messages are at most {STREAM_BYTES} bytes together"
        )));
    }
    Ok(())
}

/// A spec with no steps: the harness then answers the source's records as it read them.
fn no_steps() -> Result<PipelineSpec, OpError> {
    serde_json::from_value(json!({
        "class": "auto",
        "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
    }))
    .map_err(|e| OpError::Api(ApiError::Internal(format!("the sampling spec: {e}"))))
}

/// The sample a DataSource of this project gives: its feed fetched once, as a DataSource Check
/// fetches it (MF-39). A source with a credential has no sample a dry run may read, and the
/// answer says what to give instead; one that streams is listened to by [`streamed_sample`].
fn sample_of_data_source(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(Sample, PipelineSpec), OpError> {
    let source = readable_source(caller, state, project, name)?;
    sample_of_spec(name, &source.spec)
}

/// How a fetched DataSource of `spec` is sampled, or why it is not.
fn sample_of_spec(name: &str, spec: &Value) -> Result<(Sample, PipelineSpec), OpError> {
    let instead = "give the sample as text (a file of the feed) or as a URL instead";
    match pipeline_test::probe_plan(spec) {
        Some(Ok(url)) => pipeline_test::sampling(spec, url)
            .map_err(|e| OpError::Api(ApiError::Internal(format!("the sampling spec: {e}")))),
        Some(Err(skipped)) => Err(OpError::Api(ApiError::BadRequest(format!(
            "DataSource '{name}' cannot be sampled: {}; {instead}",
            skipped.skipped.unwrap_or_default()
        )))),
        None => Err(OpError::Api(ApiError::BadRequest(format!(
            "DataSource '{name}' is of type {}, which streams rather than answers a request; {instead}",
            spec["type"].as_str().unwrap_or("unknown")
        )))),
    }
}

/// The sample of DataSource `name` as the caller's open plan `id` adds it (T-6269): sampled before
/// an approver merges it, so a mapping is built on its records. Nobody approved its address yet,
/// so it is held to the public internet before the runner fetches it inside the cluster (T-6028);
/// a streaming source is listened to only once approved.
async fn sample_of_planned(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    id: &str,
    name: &str,
) -> Result<(Value, (Sample, PipelineSpec)), OpError> {
    let items = state
        .plans
        .peek(id, &caller.identity.subject, project, chrono::Utc::now())
        .ok_or_else(|| {
            OpError::from(ApiError::NotFound(format!(
                "no open plan {id} of yours in project '{project}'; plan the DataSource with jc_change_plan first"
            )))
        })?;
    let spec = items
        .iter()
        .find(|item| item.kind == "DataSource" && item.name == name)
        .and_then(|item| item.manifest.as_ref())
        .map(|manifest| manifest["spec"].clone())
        .ok_or_else(|| {
            OpError::from(ApiError::NotFound(format!(
                "plan {id} adds or changes no DataSource '{name}'"
            )))
        })?;
    if spec["type"] != "http" {
        return Err(ApiError::BadRequest(format!(
            "DataSource '{name}' of the plan is of type {}: a planned source is sampled only over http; give the sample as text, or sample it once approved",
            spec["type"].as_str().unwrap_or("unknown")
        ))
        .into());
    }
    if let Some(url) = spec.pointer("/http/url").and_then(Value::as_str) {
        public_address(url, "/dataSource").await?;
    }
    let sampled = sample_of_spec(name, &spec)?;
    Ok((spec, sampled))
}

/// Refuses a planned DataSource's address that is not on the public internet (T-6269), or whose
/// host does not resolve: [`sample_address`], and a name that resolves to nothing is refused too.
async fn public_address(url: &str, path: &str) -> Result<(), OpError> {
    match resolved_address(url, path).await? {
        Some((host, addresses)) => {
            resolved_public(&host, &addresses).map_err(|message| OpError::InvalidInput {
                path: path.into(),
                message,
            })
        }
        None => Ok(()),
    }
}

/// Refuses a sample address a caller typed that is not on the public internet: written into the
/// URL, or what its name resolves to now (AG-65, JC1-27), answered at the input member `path`. A
/// name that resolves to nothing passes: the runner cannot fetch it either and says so. The
/// runner resolves it again; its egress policy stays the last guard.
pub(crate) async fn sample_address(url: &str, path: &str) -> Result<(), OpError> {
    let refused = |message: String| OpError::InvalidInput {
        path: path.into(),
        message,
    };
    // A name the Portal cannot resolve may still resolve in the runner's pod, whose DNS search
    // list names its own namespace: a single-label or cluster-internal name is refused as written.
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(url::Host::Domain(host)) = parsed.host() {
            if cluster_name(host) {
                return Err(refused(format!(
                    "'{host}' is a name inside the cluster, not a public host; outbound calls reach public hosts only (AG-65)"
                )));
            }
        }
    }
    match resolved_address(url, path).await? {
        Some((host, addresses)) if !addresses.is_empty() => resolved_public(&host, &addresses)
            .map_err(|message| OpError::InvalidInput {
                path: path.into(),
                message,
            }),
        _ => Ok(()),
    }
}

/// The host an address names and what it resolves to now, after the written URL itself was held
/// to a public host; `None` when the URL names an address, not a name.
async fn resolved_address(
    url: &str,
    path: &str,
) -> Result<Option<(String, Vec<std::net::IpAddr>)>, OpError> {
    let refused = |message: String| OpError::InvalidInput {
        path: path.into(),
        message,
    };
    let parsed =
        url::Url::parse(url).map_err(|e| refused(format!("the address does not parse: {e}")))?;
    jc_core::outbound::check(&parsed).map_err(|r| refused(r.to_string()))?;
    let Some(url::Host::Domain(host)) = parsed.host() else {
        return Ok(None);
    };
    let port = parsed.port_or_known_default().unwrap_or(443);
    let resolved = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::lookup_host((host, port)),
    )
    .await;
    let addresses: Vec<std::net::IpAddr> = match resolved {
        Ok(Ok(found)) => found.map(|a| a.ip()).collect(),
        _ => Vec::new(),
    };
    Ok(Some((host.to_owned(), addresses)))
}

/// Whether `host` names something inside the cluster or the pod's own network rather than the
/// public internet: one label (`gitea`), or a cluster or local suffix.
fn cluster_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    !host.contains('.')
        || [".local", ".svc", ".internal", ".localdomain", ".home.arpa"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

/// Why a host that resolved to `addresses` is not called: none at all, or one that is not public.
fn resolved_public(host: &str, addresses: &[std::net::IpAddr]) -> Result<(), String> {
    match addresses.iter().find(|ip| !jc_core::outbound::is_public(**ip)) {
        Some(private) => Err(format!(
            "'{host}' resolves to {private}, a private, loopback or link-local address; outbound calls reach public hosts only (AG-65)"
        )),
        None if addresses.is_empty() => Err(format!(
            "the host '{host}' does not resolve"
        )),
        None => Ok(()),
    }
}

/// The DataSource `name` of this project when the caller reads it. Sampling a source, fetched
/// or listened to, reads what it carries: a caller who may not read it is told nothing, the
/// same 404 as a source that does not exist, before anything is fetched (T-5698).
fn readable_source(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<crate::resource::ResourceEnvelope, OpError> {
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    state
        .mirror
        .get(project, "DataSource", name)
        .filter(|source| effective.may_read_envelope("DataSource", source))
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "DataSource '{name}' not found in project '{project}'"
            ))
            .into()
        })
}

/// A DataSource of this project the caller may read, when it streams rather than answers a
/// request: `None` for a source the request path samples.
fn streamed_source(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Option<DataSourceSpec>, OpError> {
    let source = readable_source(caller, state, project, name)?;
    if pipeline_test::probe_plan(&source.spec).is_some() {
        return Ok(None);
    }
    serde_json::from_value(source.spec.clone())
        .map(Some)
        .map_err(|e| {
            ApiError::BadRequest(format!("DataSource '{name}' does not parse: {e}")).into()
        })
}

/// The sample of a streaming source (T-4841): its input, as a live pipeline reads it, listened to
/// beside the live pipelines for [`STREAM_SECONDS`], up to [`MAX_MESSAGES`] messages and
/// [`STREAM_BYTES`], then disconnected. The broker is the saved source's and its credential stays
/// a `${VAR}` only the runner fills; the read subscribes and publishes nothing.
async fn streamed_sample(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    name: &str,
    source: &DataSourceSpec,
) -> Result<Value, OpError> {
    static STARTED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<std::time::Instant>>>,
    > = std::sync::OnceLock::new();
    let minute = std::time::Duration::from_secs(60);
    let subject = &caller.identity.subject;
    if crate::api::knowledge::counted(&STARTED, subject, std::time::Instant::now(), minute)
        > STREAM_SAMPLES_PER_MINUTE
    {
        return Err(ApiError::TooManyRequests(format!(
            "{STREAM_SAMPLES_PER_MINUTE} samples of a streaming source in a minute already; try again in a minute"
        ))
        .into());
    }
    let (trace, limited) =
        pipeline_test::run_stream_within(state, project, STREAM_WINDOW, |capture| {
            datasource_test::stream_sample(name, project, source, capture)
        })
        .await?;
    let errors: Vec<TestError> = trace
        .errors
        .into_iter()
        .map(|error| TestError {
            // The source's message, not a step of anyone's.
            stage: "source".into(),
            step: None,
            line: None,
            ..error
        })
        .collect();
    Ok(json!({
        "streamed": true,
        "seconds": STREAM_SECONDS,
        "count": trace.mapping.len(),
        "truncated": limited,
        "fields": fields_of(&trace.mapping),
        "records": trace.mapping,
        "errors": errors,
    }))
}

/// Every field of the records, once each, in name order.
fn fields_of(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .filter_map(Value::as_object)
        .flat_map(Map::keys)
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The model of the space the records land in, with the space's segment for the id rule.
fn target(
    state: &AppState,
    project: &str,
    space: Option<String>,
    pipeline: Option<&Value>,
) -> Result<
    (
        String,
        std::sync::Arc<crate::pipeline_validation::ModelSchema>,
    ),
    OpError,
> {
    let space = match (space, pipeline) {
        (Some(space), _) => space,
        (None, Some(pipeline)) => {
            crate::api::internal::pipeline_rejected::target_space(state, project, &pipeline["spec"])
                .ok_or_else(|| {
                    ApiError::BadRequest(
                "the pipeline writes through no Endpoint of this project, so it has no target \
                 space; name its targetEndpoint or give space"
                    .into(),
            )
                })?
        }
        (None, None) => {
            return Err(OpError::InvalidInput {
                path: "/space".into(),
                message: "give pipeline, draft or space: the records are checked against the \
                          model of the space they land in"
                    .into(),
            })
        }
    };
    state
        .mirror
        .get(project, "ContextSpace", &space)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "ContextSpace '{space}' not found in project '{project}'"
            ))
        })?;
    let schema = state.model_schemas.get(project, &space).ok_or_else(|| {
        OpError::Conflict(json!({
            "error": "no_model",
            "space": space,
            "detail": format!(
                "space '{space}' names no data model, so nothing checks what lands in it; name \
                 its model in spec.dataModelRef (DM-61)"
            ),
        }))
    })?;
    Ok((space, schema))
}

/// `jc_pipeline_try_mapping`'s input in bounds: a candidate, and one of sample and messages.
fn try_mapping_valid(val: &Value) -> Result<(), OpError> {
    let input: TryMappingInput = parse_input(val.clone())?;
    if input.pipeline.is_none() && input.draft.is_none() {
        return Err(OpError::InvalidInput {
            path: "/pipeline".into(),
            message: "either pipeline or draft is required".into(),
        });
    }
    match (&input.sample, &input.messages) {
        (Some(_), None) => Ok(()),
        (None, Some(messages)) => messages_bounded(messages),
        _ => Err(OpError::InvalidInput {
            path: "/sample".into(),
            message: "give exactly one of sample and messages".into(),
        }),
    }
}

/// `jc_pipeline_try_mapping`: the candidate run over the sample, or over a streamed sample's
/// messages one each.
async fn try_mapping(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    val: Value,
) -> Result<Value, OpError> {
    try_mapping_valid(&val)?;
    let input: TryMappingInput = parse_input(val)?;
    let (pipeline, draft) =
        candidate(caller, state, project, input.pipeline, input.draft.as_ref()).await?;
    let trace = match (input.sample, input.messages) {
        (Some(sample), _) => {
            pipeline_test::execute_test_pipeline(
                &caller.identity,
                state,
                project,
                pipeline_test::TestRequest {
                    pipeline: pipeline.clone(),
                    sample,
                },
            )
            .await?
        }
        (None, messages) => {
            let messages = messages.unwrap_or_default();
            let spec = pipeline_test::tested_spec(&caller.identity, state, project, &pipeline)?;
            pipeline_test::run_messages(state, project, &spec, &messages).await?
        }
    };
    // The records the mapping made, judged where their write is: the record check a proposal
    // needs (T-4840), kept beside the draft's own verdict.
    if let Some(draft) = draft {
        let checked = crate::record_check::check_trace(state, project, &pipeline, &trace.mapping)?;
        let shown = &trace.mapping[..trace.mapping.len().min(crate::record_check::MAX_RECORDS)];
        let mut records = checked.verdict(&pipeline, shown);
        records.ok &= trace.errors.is_empty();
        crate::ops::proposals::record_records(caller, state, project, &draft, &pipeline, records)
            .await;
    }
    let mut answer = json!({
        "input": trace.input,
        "records": trace.mapping,
        "errors": trace.errors,
    });
    if input.steps {
        answer["steps"] = json!(step_trace(&pipeline, &trace));
    }
    let sources = data_sources_of(&pipeline);
    Ok(without_source_fields(
        caller,
        state,
        project,
        sources.iter().map(String::as_str),
        answer,
    ))
}

/// What each step of `pipeline` made of the sample (T-6082): its name, up to [`STAGE_RECORDS`]
/// distinct records masked as a run's log is, how many messages reached it whole and failed at
/// it, and their errors. Past [`STEP_BYTES`] a step's records give way to their field names.
///
/// ponytail: a step that deletes a message (`root = deleted()`) leaves no capture, so it counts
/// in neither `reached` nor `dropped`; a per-step deletion count needs a harness branch per step.
pub(super) fn step_trace(pipeline: &Value, trace: &TestTrace) -> Vec<Value> {
    let names: Vec<String> = serde_json::from_value::<PipelineSpec>(pipeline["spec"].clone())
        .map(|spec| spec.steps().iter().map(step_name).collect())
        .unwrap_or_default();
    let mut room = STEP_BYTES;
    trace
        .stages
        .iter()
        .map(|stage| {
            let records: Vec<Value> = stage
                .records
                .iter()
                .map(crate::pipeline_outcomes::mask)
                .collect();
            let errors: Vec<Value> = trace
                .errors
                .iter()
                .filter(|error| error.step == Some(stage.step))
                .take(STAGE_RECORDS)
                .map(|error| {
                    json!({
                        "line": error.line,
                        "message": crate::pipeline_outcomes::mask_text(&error.message),
                    })
                })
                .collect();
            let size = serde_json::to_vec(&records).map_or(STEP_BYTES + 1, |b| b.len());
            let mut step = json!({
                "step": stage.step,
                "name": names.get(stage.step),
                "reached": stage.reached,
                "dropped": stage.dropped,
                "errors": errors,
            });
            if size <= room {
                room -= size;
                step["records"] = json!(records);
            } else {
                step["records"] = json!([]);
                step["fields"] = json!(fields_of(&records));
                step["truncated"] = json!(true);
            }
            step
        })
        .collect()
}

/// A step's name as its manifest writes it: the processor's, or the compute kind.
fn step_name(step: &Step) -> String {
    match step {
        Step::Processor(step) => step.processor.keys().next().cloned().unwrap_or_default(),
        Step::Compute(compute) => serde_json::to_value(compute.kind)
            .ok()
            .and_then(|kind| kind.as_str().map(str::to_owned))
            .unwrap_or_default(),
    }
}

/// `jc_pipeline_sample_source` input: a DataSource, maybe of an open plan, or a sample.
fn sample_source_valid(val: &Value) -> Result<(), OpError> {
    let input: SampleSourceInput = parse_input(val.clone())?;
    if input.plan_id.is_some() && input.data_source.is_none() {
        return Err(OpError::InvalidInput {
            path: "/planId".into(),
            message: "planId goes with dataSource, the name of the DataSource the plan adds".into(),
        });
    }
    match (&input.data_source, &input.sample) {
        (Some(_), None) | (None, Some(_)) => Ok(()),
        _ => Err(OpError::InvalidInput {
            path: "/dataSource".into(),
            message: "give exactly one of dataSource and sample".into(),
        }),
    }
}

/// `jc_pipeline_sample_source`: the first records of a DataSource, a planned one or a sample.
async fn sample_source(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    val: Value,
) -> Result<Value, OpError> {
    let input: SampleSourceInput = parse_input(val)?;
    let named = input.data_source.clone();
    // A planned source is not in the mirror yet: its own manifest lists its
    // personal fields (AG-158).
    let mut planned = Value::Null;
    let (sample, plan) = match (input.data_source, input.sample) {
        (Some(name), None) if input.plan_id.is_some() => {
            let id = input.plan_id.unwrap_or_default();
            let (spec, sampled) = sample_of_planned(caller, state, project, &id, &name).await?;
            planned = spec;
            sampled
        }
        (Some(name), None) => {
            if let Some(source) = streamed_source(caller, state, project, &name)? {
                let answer =
                    bounded_sample(streamed_sample(caller, state, project, &name, &source).await?);
                return Ok(without_source_fields(
                    caller,
                    state,
                    project,
                    [name.as_str()],
                    answer,
                ));
            }
            sample_of_data_source(caller, state, project, &name)?
        }
        (None, Some(sample)) => {
            if let Some(url) = sample.url.as_deref() {
                sample_address(url, "/sample/url").await?;
            }
            (sample, no_steps()?)
        }
        _ => {
            return Err(OpError::InvalidInput {
                path: "/dataSource".into(),
                message: "give exactly one of dataSource and sample".into(),
            })
        }
    };
    let trace = pipeline_test::run_harness(state, project, &plan, &sample).await?;
    let fields = fields_of(&trace.mapping);
    let answer = json!({
        "count": trace.mapping.len(),
        "truncated": trace.mapping.len() >= MAX_MESSAGES,
        "fields": fields,
        "records": trace.mapping,
        "errors": trace.errors,
    });
    let answer = bounded_sample(answer);
    let answer = without_source_fields(caller, state, project, named.as_deref(), answer);
    Ok(match named.as_deref() {
        Some(name) => without_planned_fields(caller, name, &planned, answer),
        None => answer,
    })
}

/// The most bytes of records one sample answer carries (T-6393): a client refuses a tool result
/// over its token cap, so a 234 KB feed showed the steward nothing at all.
const MAX_SAMPLE_BYTES: usize = 16 * 1024;

/// Cuts `answer.records` to [`MAX_SAMPLE_BYTES`], saying so in `truncated`, `bytes` and `cut`. A
/// source that is one JSON document keeps its first items of its longest array, the records a
/// mapping splits it into; else the first records that fit; a single record that still does not
/// fit is shown as the start of its text.
fn bounded_sample(mut answer: Value) -> Value {
    let size = |value: &Value| value.to_string().len();
    let bytes = size(&answer["records"]);
    let Some(records) = answer["records"].as_array_mut() else {
        return answer;
    };
    if bytes <= MAX_SAMPLE_BYTES {
        return answer;
    }
    let mut cut = None;
    if let [record] = records.as_mut_slice() {
        if let Some((path, _)) = longest_array(record, "", 0) {
            let whole = size(record);
            if let Some(items) = record.pointer_mut(&path).and_then(Value::as_array_mut) {
                let sizes: Vec<usize> = items.iter().map(|item| size(item) + 1).collect();
                let room = MAX_SAMPLE_BYTES.saturating_sub(whole - sizes.iter().sum::<usize>());
                let kept = fitting(&sizes, room);
                cut = Some(format!(
                    "{path}: the first {kept} of its {} items",
                    items.len()
                ));
                items.truncate(kept);
            }
        }
    }
    if records.len() > 1 {
        let sizes: Vec<usize> = records.iter().map(|record| size(record) + 1).collect();
        records.truncate(fitting(&sizes, MAX_SAMPLE_BYTES).max(1));
    }
    if let [record] = records.as_mut_slice() {
        let text = record.to_string();
        if text.len() > MAX_SAMPLE_BYTES {
            let mut end = MAX_SAMPLE_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            *record = Value::String(format!(
                "{}… (cut at {end} of {} bytes)",
                &text[..end],
                text.len()
            ));
        }
    }
    answer["truncated"] = json!(true);
    answer["bytes"] = json!(bytes);
    if let Some(cut) = cut {
        answer["cut"] = json!(cut);
    }
    answer
}

/// How many of the leading `sizes` fit in `room` bytes.
fn fitting(sizes: &[usize], mut room: usize) -> usize {
    sizes
        .iter()
        .take_while(|n| room.checked_sub(**n).map(|left| room = left).is_some())
        .count()
}

/// The JSON pointer and length of the array with the most items in `value`, at most four
/// object levels down.
fn longest_array(value: &Value, at: &str, depth: usize) -> Option<(String, usize)> {
    let Value::Object(members) = value else {
        return None;
    };
    members
        .iter()
        .filter_map(|(name, child)| {
            let path = format!("{at}/{}", name.replace('~', "~0").replace('/', "~1"));
            match child {
                Value::Array(items) => Some((path, items.len())),
                Value::Object(_) if depth < 4 => longest_array(child, &path, depth + 1),
                _ => None,
            }
        })
        .max_by_key(|(_, len)| *len)
}

/// This module's operations in the registry (`super::init_registry`).
pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_pipeline_sample_source",
            title: "Sample a Pipeline Source",
            description: "Reads the first records of a DataSource, sample file or URL on the project's runner: the records, their fields and the count",
            input: sample_source_input_schema,
            output: sample_source_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: Some(Verb::Propose),
            lane: Lane::Green,
            validate: sample_source_valid,
            run: |caller, state, project, val| Box::pin(sample_source(caller, state, project, val)),
        },
        Operation {
            name: "jc_pipeline_try_mapping",
            title: "Try a Pipeline Mapping",
            description: "Runs a candidate pipeline's steps over a sample on the runner, writing nothing: each record it would write, each failure at its step and line",
            input: try_mapping_input_schema,
            output: try_mapping_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: Some(Verb::Propose),
            lane: Lane::Green,
            validate: try_mapping_valid,
            run: |caller, state, project, val| Box::pin(try_mapping(caller, state, project, val)),
        },
        Operation {
            name: "jc_pipeline_validate",
            title: "Validate Pipeline Records",
            description: "Checks records against the target space's data model, the target Endpoint's Policies and view, and the lifetime: a verdict per record with each finding and its expiresAt",
            input: validate_input_schema,
            output: validate_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: Some(Verb::Read),
            lane: Lane::Green,
            validate: |val| {
                let input: ValidateInput = parse_input(val.clone())?;
                if input.records.len() > MAX_RECORDS {
                    return Err(OpError::InvalidInput {
                        path: "/records".into(),
                        message: format!("at most {MAX_RECORDS} records at a time"),
                    });
                }
                Ok(())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ValidateInput = parse_input(val)?;
                    let pipeline = match (input.pipeline, input.draft.as_ref()) {
                        (None, None) => None,
                        (pipeline, draft) => Some(candidate(caller, state, project, pipeline, draft).await?),
                    };
                    let (space, schema) =
                        target(state, project, input.space, pipeline.as_ref().map(|p| &p.0))?;
                    // With a pipeline, each record is judged where its write is (T-4840); with a
                    // space alone, by the model only. Records a caller types prove nothing about
                    // the mapping, so no verdict is kept here: only the harness run
                    // (jc_pipeline_test) sets the record check a proposal needs.
                    let checked = match &pipeline {
                        Some((manifest, _)) => Some(crate::record_check::check_records(
                            state,
                            project,
                            manifest,
                            &input.records,
                            chrono::Utc::now(),
                        )?),
                        None => None,
                    };
                    let verdicts: Vec<RecordVerdict> = input
                        .records
                        .iter()
                        .enumerate()
                        .map(|(index, record)| {
                            let problems = schema.check(record);
                            let check = checked.as_ref().map(|c| c.records[index].clone());
                            RecordVerdict {
                                index,
                                ok: check.as_ref().map_or(problems.is_empty(), |c| c.ok),
                                problems,
                                endpoint: check.as_ref().and_then(|c| c.endpoint.clone()),
                                expires: check.as_ref().and_then(|c| c.expires.clone()),
                                findings: check.map(|c| c.findings),
                            }
                        })
                        .collect();
                    let valid = verdicts.iter().filter(|v| v.ok).count();
                    let mut answer = json!({
                        "space": space,
                        "model": schema.model,
                        "version": schema.version,
                        "valid": valid,
                        "rejected": verdicts.len() - valid,
                        "verdicts": verdicts,
                    });
                    if let (Some(entity_type), Some((manifest, _))) = (&input.entity_type, &pipeline) {
                        answer["writable"] = json!(writable_of(state, project, manifest, &schema, entity_type)?);
                    }
                    Ok(answer)
                })
            },
        },
    ]
}

/// Each attribute of `entity_type` in the space's model, as the pipeline may write it (T-4842).
fn writable_of(
    state: &AppState,
    project: &str,
    pipeline: &Value,
    schema: &crate::pipeline_validation::ModelSchema,
    entity_type: &str,
) -> Result<Vec<crate::record_check::Writable>, OpError> {
    let class = schema
        .attributes(entity_type)
        .ok_or_else(|| OpError::InvalidInput {
            path: "/type".into(),
            message: format!(
                "{entity_type} is not a class of the space's data model {}",
                schema.model
            ),
        })?;
    let names: Vec<String> = class.attributes.into_iter().map(|a| a.name).collect();
    Ok(crate::record_check::writable(
        state,
        project,
        pipeline,
        entity_type,
        &names,
        chrono::Utc::now(),
    )?)
}

/// The candidate pipeline of a call: the inline manifest, or the saved draft it names, and the
/// draft a verdict is recorded on. The pipeline test and the workbench's steps read it the same
/// way (ADR-N-034).
pub(super) async fn candidate(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    pipeline: Option<Value>,
    draft: Option<&DraftRef>,
) -> Result<(Value, Option<DraftRef>), OpError> {
    match (pipeline, draft) {
        (Some(p), Some(d)) => {
            super::proposals::same_draft(&p, d)?;
            Ok((p, Some(d.clone())))
        }
        (None, Some(d)) => {
            let saved = super::drafts::readable_draft(state, &caller.identity, project, d).await?;
            Ok((saved.manifest, Some(d.clone())))
        }
        (Some(p), None) => {
            let own = super::proposals::own_draft(&p);
            Ok((p, own))
        }
        (None, None) => Err(OpError::InvalidInput {
            path: "/pipeline".into(),
            message: "either pipeline or draft is required".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-6393: a 250 KB feed that is one JSON document answers under the cap with its first
    /// stations, the array's pointer and the size it had; a small sample is left alone.
    #[test]
    fn a_one_message_feed_over_the_cap_keeps_the_first_items_of_its_array() {
        let stations: Vec<Value> = (0..1500)
            .map(|n| json!({ "station_id": format!("s{n}"), "name": format!("Station number {n} by the river"), "lat": 60.17, "lon": 24.94, "capacity": 20 }))
            .collect();
        let feed = json!({ "last_updated": 1, "ttl": 0, "data": { "stations": stations } });
        let answer =
            json!({ "count": 1, "truncated": false, "fields": ["data"], "records": [feed] });
        let original = answer["records"].to_string().len();
        assert!(original > 150_000, "{original}");

        let bounded = bounded_sample(answer);
        assert!(bounded["records"].to_string().len() <= MAX_SAMPLE_BYTES);
        assert!(bounded.to_string().len() <= MAX_SAMPLE_BYTES + 512);
        assert_eq!(bounded["truncated"], true);
        assert_eq!(bounded["bytes"], original);
        let kept = bounded["records"][0]["data"]["stations"]
            .as_array()
            .expect("the array");
        assert!(kept.len() >= 2, "{}", kept.len());
        assert_eq!(kept[1]["station_id"], "s1");
        assert_eq!(bounded["records"][0]["last_updated"], 1);
        let cut = bounded["cut"].as_str().expect("cut");
        assert!(cut.starts_with("/data/stations: the first "), "{cut}");
        assert!(cut.ends_with(" of its 1500 items"), "{cut}");

        let small = json!({ "count": 1, "truncated": false, "records": [{ "a": 1 }] });
        assert_eq!(bounded_sample(small.clone()), small);
    }

    /// T-6393: many records keep the first that fit; one record with no array is shown as the
    /// start of its text.
    #[test]
    fn many_records_or_one_long_text_are_cut_to_the_cap() {
        let rows: Vec<Value> = (0..400)
            .map(|n| json!({ "n": n, "text": "x".repeat(200) }))
            .collect();
        let bounded = bounded_sample(json!({ "count": 400, "records": rows }));
        let kept = bounded["records"].as_array().expect("records");
        assert!(!kept.is_empty() && kept.len() < 400, "{}", kept.len());
        assert_eq!(kept[0]["n"], 0);
        assert!(bounded["records"].to_string().len() <= MAX_SAMPLE_BYTES);
        assert!(bounded.get("cut").is_none());

        let long = json!({ "records": [{ "body": "ž".repeat(20_000) }] });
        let bounded = bounded_sample(long);
        let text = bounded["records"][0].as_str().expect("the record's text");
        assert!(text.ends_with(" bytes)"), "{}", &text[text.len() - 40..]);
        assert!(bounded["records"].to_string().len() <= MAX_SAMPLE_BYTES + 64);
    }

    /// T-6269: a planned source's name is judged by what it resolves to: a public name pointing at
    /// loopback, or a cluster Service name, is refused before the runner fetches it.
    #[test]
    fn a_planned_source_name_resolving_to_a_private_address_is_refused() {
        let ip = |text: &str| text.parse::<std::net::IpAddr>().expect("an address");
        assert_eq!(
            resolved_public("feeds.example", &[ip("93.184.215.14")]),
            Ok(())
        );
        for (host, addresses) in [
            ("127.0.0.1.nip.io", vec![ip("127.0.0.1")]),
            ("portal.portal.svc.cluster.local", vec![ip("10.43.0.12")]),
            ("dual.example", vec![ip("93.184.215.14"), ip("::1")]),
        ] {
            let refusal = resolved_public(host, &addresses).expect_err(host);
            assert!(refusal.contains("public hosts only"), "{refusal}");
        }
        assert!(resolved_public("nowhere.invalid", &[])
            .expect_err("no address")
            .contains("does not resolve"));
    }

    /// JC1-27: a name only the cluster's DNS answers is refused as written, whatever the
    /// Portal's own pod resolves; a public name is left to its resolution.
    #[test]
    fn a_cluster_name_is_never_a_public_host() {
        for host in [
            "gitea",
            "portal.portal.svc",
            "portal.portal.svc.cluster.local.",
            "metadata.google.internal",
            "printer.local",
        ] {
            assert!(cluster_name(host), "{host}");
        }
        for host in ["data.example.org", "api.citybik.es", "svc.example.com"] {
            assert!(!cluster_name(host), "{host}");
        }
    }

    /// JC1-27: a typed sample address is refused at the member named, for a written loopback,
    /// private or cluster address; a dotted name that resolves to nothing is left to the runner.
    #[tokio::test]
    async fn a_sample_address_inside_the_cluster_is_refused_at_its_member() {
        for url in [
            "http://127.0.0.1/a.csv",
            "http://10.0.0.5/a.csv",
            "http://gitea:3000/a.csv",
        ] {
            match sample_address(url, "/sample/url").await {
                Err(OpError::InvalidInput { path, .. }) => assert_eq!("/sample/url", path, "{url}"),
                other => panic!("{url}: {other:?}"),
            }
        }
        assert!(sample_address("https://feeds.invalid/a.csv", "/sample/url")
            .await
            .is_ok());
    }

    /// AG-158 (T-6265): an MCP client and an agent run get a sample without the fields the
    /// DataSource lists personal, named; a signed-in session keeps them.
    #[test]
    fn a_sample_leaves_the_listed_source_fields_only_for_a_model_outside_the_portal() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "DataSource".to_owned(),
            metadata: crate::resource::ObjectMeta::new("trips", "helsinki"),
            spec: json!({ "type": "http", "http": { "url": "https://data.example.fi/trips.csv" }, "personalFields": ["rider"] }),
            status: None,
        });
        let sample = json!({ "records": [{ "rider": "Jana", "station": "SNP" }] });
        let caller = |via| super::super::Caller {
            identity: crate::auth::session::Identity {
                client: None,
                subject: "sub-jana".into(),
                username: "jana".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            via,
            access: None,
            bearer: None,
        };
        use super::super::Via;
        for via in [Via::Mcp, Via::Agent] {
            let answer =
                without_source_fields(&caller(via), &state, "helsinki", ["trips"], sample.clone());
            assert_eq!(answer["records"], json!([{ "station": "SNP" }]), "{via:?}");
            assert_eq!(answer["withheld"], json!(["DataSource/trips.rider"]));
        }
        for via in [Via::Session, Via::Bearer] {
            let answer =
                without_source_fields(&caller(via), &state, "helsinki", ["trips"], sample.clone());
            assert_eq!(answer, sample, "{via:?}");
        }
        // A planned source, not in the mirror yet, is held to its own manifest's list (T-6269).
        let spec = json!({ "personalFields": ["rider"] });
        let answer = without_planned_fields(&caller(Via::Mcp), "planned", &spec, sample.clone());
        assert_eq!(answer["records"], json!([{ "station": "SNP" }]));
        assert_eq!(answer["withheld"], json!(["DataSource/planned.rider"]));
        let kept = without_planned_fields(&caller(Via::Session), "planned", &spec, sample.clone());
        assert_eq!(kept, sample);
        // Another project's source of the same name lists nothing here.
        let answer = without_source_fields(
            &caller(Via::Mcp),
            &state,
            "other",
            ["trips"],
            sample.clone(),
        );
        assert_eq!(answer, sample);
    }

    #[test]
    fn a_pipeline_reads_the_data_sources_its_one_and_many_sources_name() {
        let one = json!({ "spec": { "source": { "dataSourceRef": "gbfs" } } });
        let many = json!({ "spec": { "sources": [
            { "dataSourceRef": { "kind": "DataSource", "name": "a" } },
            { "endpointRef": "e" },
            { "dataSourceRef": "b" }
        ] } });
        assert_eq!(data_sources_of(&one), ["gbfs"]);
        assert_eq!(data_sources_of(&many), ["a", "b"]);
        assert!(data_sources_of(&json!({ "spec": {} })).is_empty());
    }

    /// T-6082: each step names itself and shows its records masked as a run's log is, its
    /// counts and errors; a step past the byte budget keeps its field names and says so.
    #[test]
    fn the_step_trace_names_each_step_and_masks_and_caps_its_records() {
        use jcctl::pipeline_test::{Captured, StageSample};
        let pipeline = json!({ "apiVersion": "joinedcontext.com/v1alpha2", "kind": "Pipeline", "spec": {
            "class": "scheduled", "schedule": "0 * * * *",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "counters" } }],
            "steps": [
                { "processor": { "mapping": "root = this" } },
                { "kind": "bloblang", "bloblang": "root.count = this.count.number()" }
            ],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:d:intake" }]
        }});
        let captured = [
            Captured {
                input: Some("a".into()),
                output: Some(json!({})),
                steps: Some(vec![
                    Some(r#"{"count":"4","password":"hunter2"}"#.into()),
                    Some(r#"{"count":4}"#.into()),
                ]),
                ..Captured::default()
            },
            Captured {
                input: Some("b".into()),
                error: Some("failed assignment (line 1): Bearer abc.def.ghi".into()),
                step: Some("1".into()),
                steps: Some(vec![Some(r#"{"count":"x"}"#.into()), None]),
                ..Captured::default()
            },
        ];
        let trace = jcctl::pipeline_test::trace(&captured);
        let steps = step_trace(&pipeline, &trace);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["name"], "mapping");
        assert_eq!(steps[1]["name"], "bloblang");
        assert_eq!(
            steps[0]["records"][0]["password"],
            crate::pipeline_outcomes::MASK
        );
        assert_eq!(steps[0]["records"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            (steps[1]["reached"].as_u64(), steps[1]["dropped"].as_u64()),
            (Some(1), Some(1))
        );
        assert_eq!(steps[1]["errors"][0]["line"], 1);
        assert!(
            !steps[1]["errors"][0]["message"]
                .to_string()
                .contains("abc.def"),
            "{}",
            steps[1]
        );

        // One step over the whole budget: its field names, no records, and the next still shown.
        let big = TestTrace {
            stages: vec![
                StageSample {
                    records: vec![json!({ "blob": "x".repeat(STEP_BYTES) })],
                    ..StageSample::default()
                },
                StageSample {
                    step: 1,
                    records: vec![json!({ "count": 4 })],
                    ..StageSample::default()
                },
            ],
            ..TestTrace::default()
        };
        let steps = step_trace(&pipeline, &big);
        assert_eq!(steps[0]["truncated"], true);
        assert_eq!(steps[0]["fields"], json!(["blob"]));
        assert_eq!(steps[0]["records"], json!([]));
        assert_eq!(steps[1]["records"], json!([{ "count": 4 }]));
    }

    #[test]
    fn fields_are_every_key_once_in_name_order() {
        let records = [
            json!({ "station": "01", "pm10": 18.2 }),
            json!("not an object"),
            json!({ "pm10": 3, "no2": 4 }),
        ];
        assert_eq!(fields_of(&records), ["no2", "pm10", "station"]);
        assert!(fields_of(&[]).is_empty());
    }
}
