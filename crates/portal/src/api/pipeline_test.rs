//! Testing a candidate pipeline before it is proposed (T-0591, PL-43, MF-38, API/01 §7a).
//!
//! The mapping runs where it will run: as an ephemeral stream on the project's runner, through
//! its streams API. The Portal renders the harness with jcctl, creates the stream, waits at
//! most three seconds for what the harness posts back to the capture route on the internal
//! listener, deletes the stream whatever happened, and answers the trace. Nothing is written
//! and no `secretRef` is resolved: the harness carries the sample and the mapping, nothing else.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::{PipelineSpec, Verb};
use jcctl::pipeline_test::{
    harness, lint_errors, trace, Captured, Sample, SampleFormat, TestError, TestTrace, EMPTY_BODY,
    FETCH_FAILED, MAX_MESSAGES, MAX_SAMPLE_BYTES, STREAM_PREFIX,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use utoipa::ToSchema;

use crate::agents::share;
use crate::api::dry_run::Probe;
use crate::api::pipelines::http;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// How long the runner has to produce the trace (Architecture/08 §7).
const DEADLINE: Duration = Duration::from_secs(3);
/// The age past which the project's test stream is a left-over of a failed delete, not a test.
const LEFT_OVER: Duration = Duration::from_secs(60);
/// Silence after the last captured message that ends the wait early.
const QUIET: Duration = Duration::from_millis(300);
/// The request body: the sample's five mebibytes plus the manifest and the JSON around them.
pub(crate) const BODY_LIMIT: usize = MAX_SAMPLE_BYTES + 1024 * 1024;

/// The request of API/01 §7a.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TestRequest {
    /// The candidate Pipeline manifest, unsaved.
    pub pipeline: Value,
    /// What the harness reads, read through `SampleRequest`, whose shape the document publishes.
    #[serde(deserialize_with = "sample_of_request")]
    #[schema(value_type = SampleRequest)]
    pub sample: Sample,
}

fn sample_of_request<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Sample, D::Error> {
    SampleRequest::deserialize(deserializer).map(Sample::from)
}

/// The sample of one test as the route reads it and the OpenAPI document publishes it (UI-07).
/// jcctl's `Sample` carries no schema; this is its shape, turned into it by `From` below, so a
/// field jcctl gains or loses fails to compile here instead of drifting from the document.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PipelineTestSample)]
pub struct SampleRequest {
    /// The sample inline, at most five mebibytes.
    #[serde(default)]
    pub text: Option<String>,
    /// An http(s) URL the runner fetches the sample from instead.
    #[serde(default)]
    pub url: Option<String>,
    /// How the sample is split into messages.
    #[serde(default)]
    pub format: SampleFormatRequest,
}

/// How a sample is split: one message, a CSV row each, or a JSON array's elements.
#[derive(Debug, Clone, Copy, Default, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = PipelineTestSampleFormat)]
pub enum SampleFormatRequest {
    #[default]
    Text,
    Csv,
    Json,
}

impl From<SampleRequest> for Sample {
    fn from(request: SampleRequest) -> Self {
        let SampleRequest { text, url, format } = request;
        Sample {
            text,
            url,
            format: match format {
                SampleFormatRequest::Text => SampleFormat::Text,
                SampleFormatRequest::Csv => SampleFormat::Csv,
                SampleFormatRequest::Json => SampleFormat::Json,
            },
            records: None,
        }
    }
}

/// A test in flight: which project it belongs to and where its captured messages go.
pub(crate) struct Running {
    pub(crate) project: String,
    pub(crate) sender: mpsc::UnboundedSender<Captured>,
}

/// One test per project at a time, keyed by the test id the capture route is called with.
pub(crate) static RUNNING: LazyLock<Mutex<HashMap<String, Running>>> =
    LazyLock::new(Mutex::default);

/// Holds the project's slot while the test runs and frees it however the test ends.
struct Slot(String);

impl Slot {
    fn take(
        project: &str,
        id: &str,
        sender: mpsc::UnboundedSender<Captured>,
    ) -> Result<Self, ApiError> {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running.values().any(|r| r.project == project) {
            return Err(ApiError::Conflict(format!(
                "a pipeline test is already running in '{project}'; wait for it"
            )));
        }
        running.insert(
            id.to_owned(),
            Running {
                project: project.to_owned(),
                sender,
            },
        );
        Ok(Self(id.to_owned()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// The manifest's spec, once the kind has accepted the whole manifest (MF-37).
fn spec_of(pipeline: &Value, project: &str) -> Result<PipelineSpec, ApiError> {
    if pipeline["kind"] != "Pipeline" {
        return Err(ApiError::BadRequest(
            "pipeline.kind must be 'Pipeline'".into(),
        ));
    }
    // A candidate belongs to the project of the URL: the studio's draft names no namespace,
    // and one naming another project would be tested on the wrong runner.
    let mut pipeline = pipeline.clone();
    match pipeline["metadata"]["namespace"].as_str() {
        None => {
            if let Some(metadata) = pipeline["metadata"].as_object_mut() {
                metadata.insert("namespace".into(), Value::String(project.to_owned()));
            }
        }
        Some(namespace) if namespace != project => {
            return Err(ApiError::BadRequest(format!(
                "pipeline.metadata.namespace '{namespace}' is not the project '{project}'"
            )));
        }
        Some(_) => {}
    }
    let text = serde_json::to_string(&pipeline).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Some(checked) = jc_core::registry::validate_yaml("Pipeline", &text) {
        checked.map_err(|e| ApiError::BadRequest(format!("spec is not a valid Pipeline: {e}")))?;
    }
    serde_json::from_value(pipeline["spec"].clone())
        .map_err(|e| ApiError::BadRequest(format!("spec is not a valid Pipeline: {e}")))
}

/// How long a test waits for what the harness posts, and how much it keeps.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Window {
    /// The longest wait, from the stream's create.
    pub(crate) deadline: Duration,
    /// Silence after a message that ends the wait early; `None` waits out the deadline.
    pub(crate) quiet: Option<Duration>,
    /// The input bytes kept over every message; the message past them ends the wait unkept.
    pub(crate) bytes: usize,
}

/// A source that answers a request: one answer, read within three seconds.
pub(crate) const REQUEST: Window = Window {
    deadline: DEADLINE,
    quiet: Some(QUIET),
    bytes: usize::MAX,
};

/// Waits for what the harness posts: until the runner has been quiet after its first message,
/// until enough messages or bytes, or until the deadline. `true` beside the messages when a limit
/// ended the wait, so the source had more to give.
async fn collect(
    receiver: &mut mpsc::UnboundedReceiver<Captured>,
    window: Window,
) -> (Vec<Captured>, bool) {
    let mut captured = Vec::new();
    let mut bytes = 0;
    let deadline = tokio::time::Instant::now() + window.deadline;
    while captured.len() < MAX_MESSAGES {
        let wait = match window.quiet {
            Some(quiet) if !captured.is_empty() => {
                (tokio::time::Instant::now() + quiet).min(deadline)
            }
            _ => deadline,
        };
        match tokio::time::timeout_at(wait, receiver.recv()).await {
            Ok(Some(message)) => {
                bytes += message.input.as_ref().map_or(0, String::len);
                if bytes > window.bytes {
                    return (captured, true);
                }
                captured.push(message);
            }
            _ => return (captured, false),
        }
    }
    (captured, true)
}

/// Executes a candidate pipeline test and returns the trace (PL-43, MF-38).
pub async fn execute_test_pipeline(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    request: TestRequest,
) -> Result<TestTrace, ApiError> {
    let spec = tested_spec(identity, state, project, &request.pipeline)?;
    // A sample URL the caller typed is fetched by the runner inside the cluster before anybody
    // approved it: held to the public internet first (AG-65, JC1-27).
    if let Some(url) = request.sample.url.as_deref() {
        crate::ops::pipeline_steps::sample_address(url, "/sample/url")
            .await
            .map_err(crate::ops::OpError::into_api_error)?;
    }
    run_harness(state, project, &spec, &request.sample).await
}

/// The spec of a candidate this caller may test: a Pipeline of this project, with a source, that
/// the caller may propose (PL-43).
pub(crate) fn tested_spec(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    pipeline: &Value,
) -> Result<PipelineSpec, ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let spec = spec_of(pipeline, project)?;
    let has_source = spec
        .sources()
        .iter()
        .any(|s| s.data_source_ref.is_some() || s.endpoint_ref.is_some());
    if !has_source {
        return Err(ApiError::BadRequest(
            "pipeline.spec.source must declare dataSourceRef or endpointRef".into(),
        ));
    }
    crate::permissions::for_request(state, identity, project).check(
        "Pipeline",
        Verb::Propose,
        Some(pipeline),
    )?;
    Ok(spec)
}

/// `POST /api/v1/projects/{project}/pipelines/test` (PL-43, MF-38).
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/pipelines/test",
    summary = "Pipeline Test",
    description = "Tests candidate pipeline mapping and validation on runner without writing.",
    tag = "pipelines",
    params(("project" = String, Path, description = "Project slug")),
    request_body(
        content = TestRequest,
        description = "`pipeline`: the candidate manifest, unsaved. `sample`: `text` or `url`, \
                       and a `format` (`csv`, `json`, `text`). API/01 §7a.",
        content_type = "application/json",
        example = json!({
            "pipeline": {
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Pipeline",
                "metadata": { "name": "shmu-air-quality" },
                "spec": {
                    "class": "resident",
                    "source": { "dataSourceRef": { "kind": "DataSource", "name": "shmu-csv" } },
                    "compute": { "kind": "bloblang", "bloblang": "root.pm10 = this.pm10.number()" },
                    "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:air:helsinki-air"
                }
            },
            "sample": { "text": "station_id,pm10\n01,18.2\n", "format": "csv" }
        })
    ),
    responses(
        (status = 200, description = "The trace: what the harness read, what each step made of \
                                      it, and the messages it would have written; `check` is \
                                      each record's verdict from the space's model, the target \
                                      Endpoint and the lifetime (T-4840)", body = Object),
        (status = 400, description = "A manifest that is not a Pipeline, a source that declares \
                                      neither `dataSourceRef` nor `endpointRef`, or a sample \
                                      past the byte limit", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No grant proposes a Pipeline in this project", body = ProblemDetails),
        (status = 409, description = "A test of this project is already running", body = ProblemDetails),
        (status = 413, description = "Body larger than the limit", body = ProblemDetails),
        (status = 503, description = "No pipeline runner is configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn test_pipeline(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<TestRequest>,
) -> Result<Json<Value>, ApiError> {
    let pipeline = request.pipeline.clone();
    let trace = execute_test_pipeline(&user.0.identity, &state, &project, request).await?;
    let check = crate::record_check::check_trace(&state, &project, &pipeline, &trace.mapping)?;
    let mut answer = serde_json::to_value(&trace).map_err(|e| ApiError::Internal(e.to_string()))?;
    answer["check"] =
        serde_json::to_value(&check).map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(answer))
}

/// One run of `spec` over `sample` on the project's runner: the harness (PL-43) as an
/// ephemeral stream, its captured messages as the trace, the stream deleted whatever happened.
pub(crate) async fn run_harness(
    state: &AppState,
    project: &str,
    spec: &PipelineSpec,
    sample: &Sample,
) -> Result<TestTrace, ApiError> {
    run_stream(state, project, |capture| {
        let mut config = harnessed(&state.mirror, project, spec, sample, capture)?;
        with_lifetime(&mut config, spec)?;
        Ok(config)
    })
    .await
}

/// The harness of `spec` with the spaces its mapping reads (`env("JC_SPACE")`, …) written in as
/// the live stream has them: without them a mapping minting ids from its space made
/// `…:<nil>:…` in the test and every record was refused, while the same mapping ran live (T-6567).
fn harnessed(
    mirror: &crate::store::Mirror,
    project: &str,
    spec: &PipelineSpec,
    sample: &Sample,
    capture: &str,
) -> Result<Value, ApiError> {
    let mut config =
        harness(spec, sample, capture).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    crate::reconciler::streams::StreamSpaces::of(mirror, project, spec).inject(&mut config);
    Ok(config)
}

/// The harness with the pipeline's history and lifetime steps after its own steps, where a live
/// stream runs them (PL-82, PL-107, T-4840, T-4845), so the trace's records carry the `observedAt`
/// and `expiresAt` they would be written with. A message that already failed skips them: its
/// envelope keeps the error it failed with.
fn with_lifetime(config: &mut Value, spec: &PipelineSpec) -> Result<(), ApiError> {
    let history = crate::reconciler::streams::history_step(spec)
        .map_err(|e| ApiError::BadRequest(format!("spec.history: {e}")))?;
    let lifetime = crate::reconciler::streams::lifetime_step(spec)
        .map_err(|e| ApiError::BadRequest(format!("spec.lifetime: {e}")))?;
    let steps: Vec<Value> = history.into_iter().chain(lifetime).collect();
    if steps.is_empty() {
        return Ok(());
    }
    let Some(processors) = config
        .pointer_mut("/pipeline/processors")
        .and_then(Value::as_array_mut)
    else {
        return Err(ApiError::Internal("the harness has no processors".into()));
    };
    let envelope = processors
        .iter()
        .position(|p| {
            p["mapping"]
                .as_str()
                .is_some_and(|m| m.starts_with("let failed = errored()"))
        })
        .ok_or_else(|| ApiError::Internal("the harness has no envelope step".into()))?;
    processors.insert(
        envelope,
        serde_json::json!({ "switch": [{ "check": "!errored()", "processors": steps }] }),
    );
    Ok(())
}

/// `spec` run over the messages a streaming source gave (T-4841): each message reaches the steps
/// alone, as the live input hands it over, and not as one page of them.
pub(crate) async fn run_messages(
    state: &AppState,
    project: &str,
    spec: &PipelineSpec,
    messages: &[Value],
) -> Result<TestTrace, ApiError> {
    let sample = messages_sample(messages);
    run_stream(state, project, |capture| {
        let mut config = harnessed(&state.mirror, project, spec, &sample, capture)?;
        split_messages(&mut config);
        with_lifetime(&mut config, spec)?;
        Ok(config)
    })
    .await
}

/// The messages as one text sample: a JSON array of each message's bytes in base64, a text
/// message as it is and any other as its JSON.
fn messages_sample(messages: &[Value]) -> Sample {
    use base64::Engine;
    let encoded: Vec<Value> = messages
        .iter()
        .map(|message| {
            let text = match message {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            Value::String(base64::engine::general_purpose::STANDARD.encode(text))
        })
        .collect();
    Sample {
        text: Some(Value::Array(encoded).to_string()),
        url: None,
        format: SampleFormat::Text,
        records: None,
    }
}

/// The harness of [`messages_sample`] reading its array one message each, each message's own
/// bytes again, before anything else runs.
fn split_messages(config: &mut Value) {
    if let Some(processors) = config["pipeline"]["processors"].as_array_mut() {
        processors.splice(
            0..0,
            [
                serde_json::json!({ "unarchive": { "format": "json_array" } }),
                serde_json::json!({ "mapping": "root = this.decode(\"base64\")" }),
            ],
        );
    }
}

/// One ephemeral stream on the project's runner: `config` builds it from its capture URL, the
/// captured messages are the trace, the stream is deleted whatever happened.
pub(crate) async fn run_stream(
    state: &AppState,
    project: &str,
    config: impl FnOnce(&str) -> Result<Value, ApiError>,
) -> Result<TestTrace, ApiError> {
    run_stream_within(state, project, REQUEST, config)
        .await
        .map(|(trace, _)| trace)
}

/// `run_stream` waiting as `window` says; `true` beside the trace when a limit ended the wait.
pub(crate) async fn run_stream_within(
    state: &AppState,
    project: &str,
    window: Window,
    config: impl FnOnce(&str) -> Result<Value, ApiError>,
) -> Result<(TestTrace, bool), ApiError> {
    let runner = state
        .config
        .pipeline_runner_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no pipeline runner is configured".into()))?
        .replace("{project}", project)
        .trim_end_matches('/')
        .to_owned();
    let capture = state
        .config
        .pipeline_test_capture_url
        .as_deref()
        .ok_or_else(|| {
            ApiError::Unavailable("no capture route is configured for pipeline tests".into())
        })?;

    let id = share::slug();
    let config = config(&format!("{capture}/internal/pipeline-tests/{id}"))?;
    // The project's write budget a step's `rate_limit: pipeline_egress` names (T-5803).
    if let Some(why) = crate::reconciler::streams::declare_egress(http(), &runner, project)
        .await
        .unwrap_or_else(Some)
    {
        tracing::warn!(project = %project, %why, "pipeline runner refused a test's write budget");
        return Err(ApiError::Unavailable(
            "the pipeline runner did not answer".into(),
        ));
    }
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let _slot = Slot::take(project, &id, sender)?;

    // One stream name per project, never per test (T-4936): Bento keeps a stream name's metric
    // series for its whole life after the stream is deleted (90 series, about 160 KB each in
    // 1.22.0), so a fresh name per test grew the dev runner past 2 GiB in three days. The slot
    // above keeps this Portal to one test per project; another replica's test answers 409.
    let stream = format!("{runner}/streams/{STREAM_PREFIX}{project}");
    let identity = state.config.pipeline_identity;
    let outcome =
        create_and_trace(project, &stream, &config, identity, window, &mut receiver).await;
    // Another replica's test in flight: its stream is not this test's to delete.
    if matches!(outcome, Err(ApiError::Conflict(_))) {
        return outcome;
    }

    // Gone whatever happened: a runner that timed out or answered 5xx may still have created the
    // stream, and one it keeps answers 409 on the next test's create. A delete it refuses (anything
    // but 2xx, or 404 for a stream it never made) is a log line and a runner error in the trace.
    let deleted = match http().delete(&stream).send().await {
        Ok(answer) if answer.status().is_success() || answer.status() == StatusCode::NOT_FOUND => {
            Ok(())
        }
        Ok(answer) => Err(answer.status().to_string()),
        Err(err) => Err(err.to_string()),
    };
    let (mut result, limited) = outcome?;
    if let Err(err) = deleted {
        tracing::warn!(project = %project, error = %err, "pipeline test stream not deleted");
        result.errors.push(TestError {
            stage: "runner".into(),
            // The stream, not a step: nothing the author wrote failed here.
            step: None,
            line: None,
            message: "the test stream could not be deleted; the runner keeps it until it is".into(),
        });
    }
    Ok((result, limited))
}

/// Creates the harness stream and reads its trace: what the harness posted back, or the lint
/// errors of a create the runner refused (4xx). A create that did not answer, or answered 5xx,
/// is the caller's 503 with the runner's words kept in the log. The project's stream still on the
/// runner: a left-over one is removed and the create sent again, a live one (another Portal
/// replica's test) is the caller's 409.
async fn create_and_trace(
    project: &str,
    stream: &str,
    config: &Value,
    identity: bool,
    window: Window,
    receiver: &mut mpsc::UnboundedReceiver<Captured>,
) -> Result<(TestTrace, bool), ApiError> {
    let config = credentialed(config, project, identity);
    let (mut status, mut refusal) = create(project, stream, &config).await?;
    if held(status, &refusal) && left_over_removed(stream).await {
        (status, refusal) = create(project, stream, &config).await?;
    }
    if held(status, &refusal) {
        return Err(ApiError::Conflict(format!(
            "a pipeline test is already running in '{project}'; wait for it"
        )));
    }

    if status.is_success() {
        let (captured, limited) = collect(receiver, window).await;
        let mut result = trace(&captured);
        // A request source answers at least one message; an empty trace with no error read as
        // "the mapping made nothing and nothing failed" while the runner never answered (T-6275).
        // A streamed window (no quiet) may hear a silent topic, which is no failure.
        if captured.is_empty() && window.quiet.is_some() {
            result.errors.push(silent_runner(window));
        }
        Ok((result, limited))
    } else if status.is_client_error() {
        let refused = TestTrace {
            errors: lint_errors(&refusal),
            ..TestTrace::default()
        };
        Ok((refused, false))
    } else {
        tracing::warn!(project = %project, status = %status, "pipeline runner refused a test stream");
        Err(ApiError::Unavailable(
            "the pipeline runner did not answer".into(),
        ))
    }
}

/// The runner error of a request-window run that captured nothing.
fn silent_runner(window: Window) -> TestError {
    TestError {
        stage: "runner".into(),
        step: None,
        line: None,
        message: format!(
            "the runner sent nothing back within {} s: the sample did not reach the steps, or a step dropped every message; try again, and if it repeats ask an operator to check the project's pipeline runner",
            window.deadline.as_secs()
        ),
    }
}

/// The runner's answer to the create: its status and its words.
async fn create(
    project: &str,
    stream: &str,
    config: &Value,
) -> Result<(StatusCode, String), ApiError> {
    let created = http().post(stream).json(config).send().await.map_err(|err| {
        tracing::warn!(project = %project, error = %err, "pipeline runner unreachable for a test");
        ApiError::Unavailable("the pipeline runner did not answer".into())
    })?;
    let status = created.status();
    Ok((status, created.text().await.unwrap_or_default()))
}

/// Bento 1.22.0's refusal of a create whose stream name it already runs: 400 and these words.
fn held(status: StatusCode, refusal: &str) -> bool {
    status == StatusCode::BAD_REQUEST && refusal.trim() == "Stream already exists"
}

/// A test stream older than any test lives (the longest window is seconds) is one whose delete
/// failed: removed here, `true` when it is gone. A younger one, or one the runner will not
/// describe, is left alone.
async fn left_over_removed(stream: &str) -> bool {
    let uptime = match http().get(stream).send().await {
        Ok(answer) if answer.status().is_success() => answer
            .json::<Value>()
            .await
            .ok()
            .and_then(|held| held["uptime"].as_f64()),
        _ => None,
    };
    if uptime.is_none_or(|seconds| seconds < LEFT_OVER.as_secs_f64()) {
        return false;
    }
    matches!(http().delete(stream).send().await, Ok(answer) if answer.status().is_success())
}

/// The harness as the runner gets it: the project's `pipelines` client, or with the pipelines'
/// own identities on, the project's harness principal through the token service, which names no
/// client secret for the runner's streams API to serve back (PL-43, T-4050).
fn credentialed(config: &Value, project: &str, identity: bool) -> Value {
    let mut config = config.clone();
    crate::reconciler::streams::own_credential(&mut config, project);
    if identity {
        crate::reconciler::streams::with_pipeline_identity(
            &mut config,
            project,
            jc_core::kinds::pipeline_identity::HARNESS,
        );
    }
    config
}

/// The URL an `http` DataSource is probed at, or why it is not (MF-39): `None` for every other
/// type, a skipped probe for a source that declares a credential (a dry run resolves none,
/// MF-38) or names no URL.
pub(crate) fn probe_plan(spec: &Value) -> Option<Result<String, Probe>> {
    if spec["type"] != "http" {
        return None;
    }
    let http = &spec["http"];
    if !http["authorization"].is_null() {
        return Some(Err(Probe::skipped(
            "the source declares a credential; a dry run resolves none (MF-38)",
        )));
    }
    match http["url"].as_str() {
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
            Some(Ok(url.to_owned()))
        }
        _ => Some(Err(Probe::skipped("the source names no http(s) URL"))),
    }
}

/// Whether an `http` DataSource answers XML: its `Accept` header asks for XML, or its path ends
/// in `.xml`. FMI's WFS is one (T-4818).
fn answers_xml(spec: &Value, url: &str) -> bool {
    let accepts_xml = spec["http"]["headers"].as_object().is_some_and(|headers| {
        headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("accept")
                && value
                    .as_str()
                    .is_some_and(|v| v.to_ascii_lowercase().contains("xml"))
        })
    });
    let path = url.split(['?', '#']).next().unwrap_or(url);
    accepts_xml || path.to_ascii_lowercase().ends_with(".xml")
}

/// How one fetch of a DataSource at `url` is read, shared by the workbench's sample and the
/// DataSource Check (MF-39, PL-43): a CSV split into rows when the connection says `format: csv`
/// (PL-114) or into the elements of its `records` array, and kept whole otherwise, as the runner hands it over, an XML feed taken as it is and turned
/// into JSON by the same `xml` step a pipeline reading it starts with, anything else parsed as
/// JSON. Reading XML as JSON failed on its first `<` and the page said the source answered a
/// web page (T-4818). The spec's target is the minimum a PipelineSpec needs to exist.
pub(crate) fn sampling(
    spec: &Value,
    url: String,
) -> Result<(Sample, PipelineSpec), serde_json::Error> {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(&url)
        .to_ascii_lowercase();
    let xml = answers_xml(spec, &url);
    // The runner splits an `http` body into rows only when the DataSource says `format: csv`
    // (PL-114); any other CSV reaches the steps whole, so the sample does too (T-5350).
    let rows = spec.pointer("/http/format").and_then(Value::as_str) == Some("csv");
    let format = if xml {
        SampleFormat::Text
    } else if rows {
        SampleFormat::Csv
    } else if path.ends_with(".csv") {
        SampleFormat::Text
    } else {
        SampleFormat::Json
    };
    let target = "urn:ngsi-ld:Endpoint:probe.local:probe:probe";
    let plan = if xml {
        serde_json::json!({
            "class": "auto",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "probe" } }],
            "steps": [{ "processor": { "xml": { "operator": "to_json", "cast": false } } }],
            "outputs": [{ "targetEndpoint": target }],
        })
    } else {
        serde_json::json!({ "class": "auto", "targetEndpoint": target })
    };
    // A JSON body the source names a `records` array in reaches the steps one element per
    // message, split before them as the runner splits it (PL-114, T-6572).
    let records = spec
        .pointer("/http/records")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let sample = Sample {
        text: None,
        url: Some(url),
        format,
        records,
    };
    Ok((sample, serde_json::from_value(plan)?))
}

/// One fetch of an `http` DataSource on the project's runner, beside the dry run's plan
/// (MF-39). Nothing here fails the dry run: a runner that is missing or busy, or a feed that
/// does not answer, is a skipped probe with the reason.
pub(crate) async fn probe_source(state: &AppState, project: &str, spec: &Value) -> Option<Probe> {
    let url = match probe_plan(spec)? {
        Ok(url) => url,
        Err(skipped) => return Some(skipped),
    };
    let (sample, probe_spec) = match sampling(spec, url) {
        Ok(plan) => plan,
        Err(err) => return Some(Probe::skipped(format!("probe spec: {err}"))),
    };
    Some(
        match run_harness(state, project, &probe_spec, &sample).await {
            Ok(trace) => answer_of(trace),
            Err(err) => Probe::skipped(err.to_string()),
        },
    )
}

/// The probe's answer from its trace: the records, or what the fetch or the parse said. A test
/// stream the runner kept is already logged by `run_harness`, beside the answer, never in its place.
pub(crate) fn answer_of(trace: TestTrace) -> Probe {
    match trace.errors.iter().find(|error| error.stage != "runner") {
        None if trace.input.events > 0 => Probe {
            records: Some(trace.input.events),
            bytes: Some(trace.input.bytes),
            sample: trace.input.sample,
            skipped: None,
        },
        None => Probe::skipped("the feed answered nothing within the test's three seconds"),
        Some(error) => Probe::skipped(outcome_of(&error.message)),
    }
}

/// The reason a probe that ran gives for a feed that failed it, as opposed to a probe that did not
/// run at all (a credential, no runner, a test already running): every answer `answer_of` words
/// about the feed itself names the feed first.
pub(crate) fn feed_failed(probe: &Probe) -> Option<&str> {
    probe
        .skipped
        .as_deref()
        .filter(|reason| reason.starts_with("the feed "))
}

/// A failed fetch or parse in plain words: the HTTP status, a timeout, an empty body, or the
/// last clause of the transport or JSON error (`connection refused`, `no such host`).
fn outcome_of(error: &str) -> String {
    let last = |text: &str| text.rsplit(": ").next().unwrap_or(text).to_owned();
    let Some(fetch) = error.strip_prefix(FETCH_FAILED) else {
        return format!("the feed is not JSON: {}", last(error));
    };
    let status = fetch
        .split_once("unexpected response code (")
        .and_then(|(_, rest)| rest.split_once("): "))
        .map(|(_, status)| status.split(", Error:").next().unwrap_or(status));
    match status {
        Some(status) => format!("the feed answered {status}"),
        None if fetch.contains("Client.Timeout") || fetch.contains("deadline exceeded") => {
            "the feed did not answer within the test's three seconds".to_owned()
        }
        None if fetch.ends_with(EMPTY_BODY) => EMPTY_BODY.to_owned(),
        None => format!("the feed could not be reached: {}", last(fetch)),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/pipelines/test", post(test_pipeline))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::internal::pipeline_tests::captured;
    use axum::body::Bytes;
    use axum::http::StatusCode;
    use serde_json::json;

    /// UI-07: the request the document publishes is the one the route reads, and it reaches the
    /// harness as jcctl's own `Sample`, field for field.
    #[test]
    fn the_published_sample_is_the_one_jcctl_reads() {
        let request: TestRequest = serde_json::from_value(json!({
            "pipeline": { "kind": "Pipeline" },
            "sample": { "url": "https://example.org/a.csv", "format": "csv" }
        }))
        .expect("a sample by url");
        assert_eq!(
            request.sample.url.as_deref(),
            Some("https://example.org/a.csv")
        );
        assert_eq!(request.sample.text, None);
        assert_eq!(request.sample.format, SampleFormat::Csv);

        let inline: TestRequest = serde_json::from_value(json!({
            "pipeline": {}, "sample": { "text": "a" }
        }))
        .expect("an inline sample");
        assert_eq!(
            inline.sample.format,
            SampleFormat::Text,
            "text when no format is named"
        );
    }

    #[test]
    fn a_sample_field_nobody_reads_is_refused() {
        for sample in [
            json!({ "text": "a", "encoding": "latin1" }),
            json!({ "text": "a", "format": "xml" }),
        ] {
            let refused =
                serde_json::from_value::<TestRequest>(json!({ "pipeline": {}, "sample": sample }));
            assert!(refused.is_err(), "{sample} was accepted");
        }
    }

    /// T-6567: the test reads the target space where the live stream does, so a mapping minting
    /// ids from `env("JC_SPACE")` gets the space's segment in the test, never `<nil>`.
    #[test]
    fn the_harness_writes_the_target_space_a_mapping_reads() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let mirror = crate::store::Mirror::new();
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta {
                name: "bikes-in".to_owned(),
                namespace: Some("helsinki".to_owned()),
                ..Default::default()
            },
            spec: json!({ "contextSpaceRef": "bikes" }),
            status: None,
        });
        let spec: PipelineSpec = serde_json::from_value(json!({
            "class": "auto",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
            "compute": { "kind": "bloblang", "bloblang":
                "root.id = \"urn:ngsi-ld:Station:\" + env(\"JC_ORG_DOMAIN\") + \":\" + env(\"JC_SPACE\") + \":\" + this.id" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-in"
        }))
        .expect("spec");
        let sample = Sample {
            text: Some("{\"id\":\"1\"}".into()),
            url: None,
            format: SampleFormat::Json,
            records: None,
        };
        let config = harnessed(
            &mirror,
            "helsinki",
            &spec,
            &sample,
            "http://portal-internal:9090/x",
        )
        .expect("harness");
        let text = config.to_string();
        assert!(!text.contains("JC_SPACE"), "{text}");
        assert!(text.contains(r#"\"helsinki-bikes\""#), "{text}");
        assert!(
            text.contains("JC_ORG_DOMAIN"),
            "the runner's own variable stays: {text}"
        );
    }

    fn harness_for(sample: Sample) -> Value {
        let spec: PipelineSpec = serde_json::from_value(json!({
            "class": "auto",
            "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
        }))
        .expect("spec");
        harness(&spec, &sample, "http://portal-internal:9090/x").expect("harness")
    }

    /// T-4840: the test runs the lifetime step where a live stream does, after the author's
    /// steps and before the envelope, and only on a message that did not fail.
    #[test]
    fn the_harness_runs_the_lifetime_step_before_the_envelope() {
        let spec: PipelineSpec = serde_json::from_value(json!({
            "class": "auto",
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:vehicles:helsinki-all", "type": "Bus" }],
            "lifetime": [{ "type": "Bus", "entity": { "rolling": "PT10M" } }]
        }))
        .expect("spec");
        let sample = Sample {
            text: Some("{}".into()),
            url: None,
            format: SampleFormat::Json,
            records: None,
        };
        let mut config = harness(&spec, &sample, "http://portal-internal:9090/x").expect("harness");
        let before = config["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .len();
        with_lifetime(&mut config, &spec).expect("the step");
        let processors = config["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert_eq!(processors.len(), before + 1);
        let at = processors
            .iter()
            .position(|p| p.get("switch").is_some())
            .expect("the lifetime step");
        assert_eq!(processors[at]["switch"][0]["check"], "!errored()");
        let step = &processors[at]["switch"][0]["processors"][0];
        assert_eq!(step["label"], "lifetime");
        assert!(step["mapping"]
            .as_str()
            .is_some_and(|m| m.contains("expiresAt")));
        assert!(processors[at + 1]["mapping"]
            .as_str()
            .is_some_and(|m| m.starts_with("let failed = errored()")));

        // No lifetime, the harness jcctl rendered.
        let plain = harness_for(sample);
        let mut unchanged = plain.clone();
        let none: PipelineSpec = serde_json::from_value(json!({
            "class": "auto",
            "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
        }))
        .expect("spec");
        with_lifetime(&mut unchanged, &none).expect("nothing to add");
        assert_eq!(unchanged, plain);
    }

    #[test]
    fn a_url_sample_is_fetched_once_by_a_processor_and_a_text_sample_is_left_alone() {
        let url = harness_for(Sample {
            text: None,
            url: Some("https://feed.example/x.json".into()),
            format: SampleFormat::Json,
            records: None,
        });
        assert_eq!(url["input"]["generate"]["count"], 1);
        assert!(url["input"].get("http_client").is_none());
        let processors = url["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert_eq!(processors[0]["http"]["url"], "https://feed.example/x.json");
        assert_eq!(processors[0]["http"]["timeout"], "3s");
        assert!(processors[1]["mapping"]
            .as_str()
            .unwrap()
            .contains(EMPTY_BODY));
        assert!(processors[2]["catch"][0]["mapping"]
            .as_str()
            .unwrap()
            .starts_with("meta jc_fetch_error"));

        let text = harness_for(Sample {
            text: Some("[1]".into()),
            url: None,
            format: SampleFormat::Json,
            records: None,
        });
        assert!(text["input"]["generate"]["mapping"]
            .as_str()
            .unwrap()
            .contains("decode"));
        assert!(!text.to_string().contains("meta jc_fetch_error ="));
        assert!(text["pipeline"]["processors"][0].get("http").is_none());
    }

    /// T-4268: a live stream reads a CSV file through Bento's csv input, one message per row, so
    /// the pinned harness splits a CSV sample before the mapping; a per-row mapping that passed
    /// Live failed the test on one array of rows. Red here while the pin is behind that change.
    #[test]
    fn a_csv_sample_reaches_the_mapping_one_row_each() {
        let csv = harness_for(Sample {
            text: Some("zone_id,name\n1,Kallio\n2,Töölö\n3,Pasila\n".into()),
            url: None,
            format: SampleFormat::Csv,
            records: None,
        });
        let processors = csv["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let split = processors
            .iter()
            .position(|p| p["unarchive"]["format"] == "json_array")
            .expect("a split");
        let input = processors
            .iter()
            .position(|p| {
                p["mapping"]
                    .as_str()
                    .is_some_and(|m| m.contains("meta jc_input"))
            })
            .expect("the input's record");
        assert!(
            split < input,
            "the rows are split before the mapping: {processors:?}"
        );
    }

    /// The capture route refuses a call with no bearer (T-2271), so the harness the Portal sends
    /// must carry the runner's own credential. `harness` comes from jcctl through a **pinned tag**,
    /// and a pin behind that change is a Portal that demands a token from a harness which sends
    /// none: on dev every "Test mapping" answered 401 while both repositories' suites were green,
    /// because nothing crossed the repository boundary (T-2274). This is that check. It reads the
    /// dependency as pinned, so it goes red here the moment the two sides disagree again.
    #[test]
    fn the_harness_this_portal_sends_carries_the_runners_own_credential() {
        let config = harness_for(Sample {
            text: Some("[1]".into()),
            url: None,
            format: SampleFormat::Json,
            records: None,
        });
        let oauth2 = &config["output"]["http_client"]["oauth2"];
        assert_eq!(oauth2["enabled"], true, "{config}");
        // The runner pod's own variables, resolved by Bento where the stream runs: the Portal never
        // holds the secret and never writes one into a harness it stores.
        assert_eq!(oauth2["client_key"], "${JC_CLIENT_ID}");
        assert_eq!(oauth2["client_secret"], "${JC_CLIENT_SECRET}");
        assert_eq!(oauth2["token_url"], "${JC_TOKEN_URL}");
        assert!(
            !config.to_string().contains("Bearer "),
            "no bearer belongs in a harness: {config}"
        );
    }

    #[test]
    fn a_message_that_failed_reaches_the_capture_route_as_an_envelope() {
        let config = harness_for(Sample {
            text: Some("<html>no</html>".into()),
            url: None,
            format: SampleFormat::Json,
            records: None,
        });
        let envelope = config["pipeline"]["processors"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["mapping"].as_str())
            .find(|m| m.starts_with("let failed = errored()"))
            .expect("jcctl's envelope");
        // If jcctl rewrites its envelope, the patch no longer applies and this says so.
        assert!(!envelope.contains("let out = this\n"), "{envelope}");
        assert!(envelope.contains("let out = if $failed { null } else { this }"));
        assert!(envelope.contains("meta(\"jc_fetch_error\").or(error())"));
    }

    fn trace_with(events: usize, errors: &[(&str, &str)]) -> TestTrace {
        TestTrace {
            input: jcctl::pipeline_test::InputStage {
                events,
                bytes: events * 10,
                sample: Some(json!({ "id": 1 })),
            },
            errors: errors
                .iter()
                .map(|(stage, message)| TestError {
                    stage: (*stage).into(),
                    step: None,
                    line: None,
                    message: (*message).into(),
                })
                .collect(),
            ..TestTrace::default()
        }
    }

    #[test]
    fn the_probe_names_what_the_fetch_answered_and_a_kept_stream_never_replaces_it() {
        let kept = (
            "runner",
            "the test stream could not be deleted; the runner keeps it until it is",
        );
        let records = answer_of(trace_with(2, &[kept]));
        assert_eq!((records.records, records.bytes), (Some(2), Some(20)));
        assert!(records.skipped.is_none());

        // What Bento 1.21.1 posted for each outcome, run against a local feed.
        for (error, said) in [
            ("fetch: http://127.0.0.1:42019/x: Get \"http://127.0.0.1:42019/x\": dial tcp 127.0.0.1:42019: connect: connection refused", "the feed could not be reached: connection refused"),
            ("fetch: http://127.0.0.1:42013/missing: HTTP request returned unexpected response code (404): 404 Not Found, Error: not here", "the feed answered 404 Not Found"),
            ("fetch: http://127.0.0.1:42013/slow: Get \"http://127.0.0.1:42013/slow\": context deadline exceeded (Client.Timeout exceeded while awaiting headers)", "the feed did not answer within the test's three seconds"),
            ("fetch: https://127.0.0.1:42013/tls: Get \"https://127.0.0.1:42013/tls\": tls: first record does not look like a TLS handshake", "the feed could not be reached: first record does not look like a TLS handshake"),
            ("fetch: http://no-such-host.invalid/x: Get \"http://no-such-host.invalid/x\": dial tcp: lookup no-such-host.invalid on 192.168.65.7:53: no such host", "the feed could not be reached: no such host"),
            ("fetch: failed assignment (line 1): the feed answered an empty body", EMPTY_BODY),
            ("failed to parse message into JSON array: invalid character '<' looking for beginning of value", "the feed is not JSON: invalid character '<' looking for beginning of value"),
        ] {
            let answer = answer_of(trace_with(1, &[("mapping", error), kept]));
            assert_eq!(answer.skipped.as_deref(), Some(said));
            assert_eq!(feed_failed(&answer), Some(said));
        }
        assert_eq!(
            answer_of(trace_with(0, &[kept])).skipped.as_deref(),
            Some("the feed answered nothing within the test's three seconds")
        );
        assert!(feed_failed(&answer_of(trace_with(0, &[kept]))).is_some());
        assert!(feed_failed(&records).is_none());
        assert!(feed_failed(&Probe::skipped(
            "the source declares a credential; a dry run resolves none (MF-38)"
        ))
        .is_none());
    }

    #[test]
    fn a_manifest_of_another_kind_or_a_broken_spec_is_400() {
        assert!(spec_of(&json!({ "kind": "Endpoint", "spec": {} }), "helsinki").is_err());
        assert!(spec_of(&json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "x" },
            "spec": { "class": "scheduled", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:all" }
        }), "helsinki")
        .is_err(), "scheduled without a schedule is what the kind refuses (MF-37)");
        // The draft names no namespace: the project of the URL is filled in. Another project's
        // namespace is refused.
        let draft = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "x" },
            "spec": { "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:all" }
        });
        let spec = spec_of(&draft, "helsinki").expect("a valid pipeline");
        assert!(spec.compute.is_none());
        let mut foreign = draft.clone();
        foreign["metadata"]["namespace"] = json!("espoo");
        assert!(spec_of(&foreign, "helsinki").is_err());
        assert!(spec_of(&foreign, "espoo").is_ok());
    }

    /// T-4818: FMI's WFS answers XML, which the sample and the Check read as JSON and failed on
    /// the first `<` ("the source answered a web page"). An XML source is fetched as it is and
    /// turned into JSON by the `xml` step its pipeline starts with; JSON and CSV stay as they were.
    #[test]
    fn an_xml_source_is_sampled_through_the_xml_step_and_never_parsed_as_json() {
        let fmi = json!({ "type": "http", "http": {
            "url": "https://opendata.fmi.fi/wfs?service=WFS&request=getFeature",
            "headers": { "accept": "application/xml" }
        }});
        let url = probe_plan(&fmi).expect("http").expect("probed");
        let (sample, spec) = sampling(&fmi, url).expect("plan");
        assert_eq!(sample.format, SampleFormat::Text);
        let config = harness(&spec, &sample, "http://capture.local/c").expect("harness");
        let steps = config["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let xml = json!({ "xml": { "operator": "to_json", "cast": false } });
        assert!(steps.contains(&xml), "{steps:?}");
        assert!(!config.to_string().contains("parse_json"), "{config}");

        let by_path = json!({ "type": "http", "http": { "url": "https://x.example/feed.XML" } });
        let (sample, _) = sampling(&by_path, "https://x.example/feed.XML".into()).expect("plan");
        assert_eq!(sample.format, SampleFormat::Text);
        // T-5350, PL-114: rows only when the connection says `format: csv`, as the runner sends
        // them; a `.csv` URL without it reaches the steps whole, so its sample is one message.
        for (http, format) in [
            (
                json!({ "url": "https://x.example/bikes.json" }),
                SampleFormat::Json,
            ),
            (
                json!({ "url": "https://x.example/stops.csv?x=1" }),
                SampleFormat::Text,
            ),
            (
                json!({ "url": "https://x.example/stops.csv?x=1", "format": "csv" }),
                SampleFormat::Csv,
            ),
            (
                json!({ "url": "https://x.example/export", "format": "csv" }),
                SampleFormat::Csv,
            ),
        ] {
            let url = http["url"].as_str().expect("url").to_owned();
            let spec = json!({ "type": "http", "http": http });
            let (sample, plan) = sampling(&spec, url.clone()).expect("plan");
            assert_eq!(sample.format, format, "{http}");
            assert!(plan.steps().is_empty(), "{url}");
        }
    }

    /// T-6572: a GBFS feed keeps its ~460 stations in `data.stations`, ~100 KB in one body, over
    /// the 64 KiB a sample keeps of one message. With `records: data.stations` the sample splits
    /// the body right after the fetch, as the runner does, so each station is one message well
    /// under the bound and the steps see stations, never the page.
    #[test]
    fn a_json_source_with_records_samples_one_message_per_element() {
        let stations: Vec<Value> = (0..460)
            .map(|n| json!({ "station_id": format!("{n:03}"), "name": format!("Asema {n} with a longer name"), "lat": 60.1, "lon": 24.9, "capacity": 20, "rental_uris": { "android": "https://example.invalid/app" } }))
            .collect();
        let body = json!({ "last_updated": 1, "ttl": 0, "data": { "stations": stations } });
        assert!(
            body.to_string().len() > 64 * 1024,
            "the page alone is over the bound"
        );
        assert!(stations
            .iter()
            .all(|one| one.to_string().len() < crate::api::datasource_test::RECORD_BYTES));

        let spec = json!({ "type": "http", "http": {
            "url": "https://feeds.example/gbfs/station_information.json",
            "records": "data.stations"
        }});
        let url = probe_plan(&spec).expect("http").expect("probed");
        let (sample, plan) = sampling(&spec, url).expect("plan");
        assert_eq!(sample.records.as_deref(), Some("data.stations"));
        let config = harness(&plan, &sample, "http://capture.local/c").expect("harness");
        let processors = config["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let take = processors
            .iter()
            .position(|p| {
                p["mapping"]
                    .as_str()
                    .is_some_and(|m| m.starts_with("let records = this.\"data\".\"stations\""))
            })
            .expect("the records split");
        let input = processors
            .iter()
            .position(|p| {
                p["mapping"]
                    .as_str()
                    .is_some_and(|m| m.contains("meta jc_input"))
            })
            .expect("the input stamp");
        assert!(take < input, "split before the input is recorded");
        assert_eq!(
            processors[take + 1]["switch"][0]["processors"][1]["unarchive"]["format"],
            "json_array"
        );

        let whole = json!({ "type": "http", "http": { "url": "https://feeds.example/gbfs.json" } });
        let (sample, _) = sampling(&whole, "https://feeds.example/gbfs.json".into()).expect("plan");
        assert_eq!(sample.records, None, "without records the body stays whole");
    }

    #[test]
    fn only_an_http_source_without_a_credential_is_probed() {
        assert!(probe_plan(&json!({ "type": "mqtt", "mqtt": {} })).is_none());
        let with_credential = json!({ "type": "http", "http": {
            "url": "https://feeds.example/bikes.json",
            "authorization": { "scheme": "bearer", "headerRef": { "name": "feed", "key": "token" } }
        }});
        let skipped = probe_plan(&with_credential)
            .expect("http")
            .expect_err("skipped");
        assert!(skipped
            .skipped
            .as_deref()
            .is_some_and(|r| r.contains("MF-38")));
        assert!(!format!("{skipped:?}").contains("token\": \"")); // the ref's key, never a value
        assert_eq!(
            probe_plan(
                &json!({ "type": "http", "http": { "url": "https://feeds.example/bikes.json" } })
            ),
            Some(Ok("https://feeds.example/bikes.json".to_owned()))
        );
        assert!(
            probe_plan(&json!({ "type": "http", "http": { "url": "ftp://x" } }))
                .expect("http")
                .is_err()
        );
    }

    #[tokio::test]
    async fn one_test_per_project_and_the_slot_is_freed_when_it_ends() {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let first = Slot::take("slot-project", "id-1", sender.clone()).expect("free");
        let second = Slot::take("slot-project", "id-2", sender.clone());
        assert!(matches!(second, Err(ApiError::Conflict(_))));
        assert!(Slot::take("other-project", "id-3", sender.clone()).is_ok());
        drop(first);
        assert!(Slot::take("slot-project", "id-4", sender).is_ok());
    }

    #[tokio::test]
    async fn a_message_for_no_running_test_is_404_and_a_captured_one_reaches_the_test() {
        let body = Bytes::from(r#"{"input":"a","output":{"id":"x"},"error":null}"#);
        assert_eq!(captured("unknown", &body), StatusCode::NOT_FOUND);
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let _slot = Slot::take("capture-project", "id-c", sender).expect("free");
        assert_eq!(captured("id-c", &body), StatusCode::NO_CONTENT);
        assert_eq!(
            captured("id-c", &Bytes::from("not json")),
            StatusCode::BAD_REQUEST
        );
        let (messages, limited) = collect(&mut receiver, REQUEST).await;
        assert_eq!(
            (messages.len(), limited),
            (1, false),
            "one message, then the quiet period ends the wait"
        );
    }

    #[test]
    fn streamed_messages_reach_the_steps_one_each_before_anything_reads_them() {
        let messages = [
            json!({ "VP": { "veh": 12 } }),
            json!("plain text, \"quoted\""),
        ];
        let sample = messages_sample(&messages);
        let spec: PipelineSpec = serde_json::from_value(json!({
            "class": "auto",
            "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
        }))
        .expect("spec");
        let mut config = harness(&spec, &sample, "http://portal/x").expect("harness");
        split_messages(&mut config);
        let processors = config["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert_eq!(processors[0]["unarchive"]["format"], "json_array");
        assert_eq!(processors[1]["mapping"], "root = this.decode(\"base64\")");
        assert!(
            processors[2]["mapping"]
                .as_str()
                .is_some_and(|m| m.contains("jc_input")),
            "the input is read after the split: {processors:?}"
        );
        use base64::Engine;
        let array: Vec<String> =
            serde_json::from_str(sample.text.as_deref().expect("text")).expect("an array");
        let decoded: Vec<String> = array
            .iter()
            .map(|m| {
                String::from_utf8(
                    base64::engine::general_purpose::STANDARD
                        .decode(m)
                        .expect("base64"),
                )
                .expect("utf-8")
            })
            .collect();
        assert_eq!(decoded, [r#"{"VP":{"veh":12}}"#, r#"plain text, "quoted""#]);
    }

    fn message(input: &str) -> Captured {
        Captured {
            input: Some(input.to_owned()),
            ..Captured::default()
        }
    }

    #[tokio::test]
    async fn a_window_without_quiet_waits_out_slow_messages_and_stops_at_its_bytes() {
        let window = Window {
            deadline: Duration::from_millis(400),
            quiet: None,
            bytes: 10,
        };
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let late = sender.clone();
        sender.send(message("1234")).expect("sent");
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            _ = late.send(message("5678"));
        });
        let (kept, limited) = collect(&mut receiver, window).await;
        assert_eq!(
            (kept.len(), limited),
            (2, false),
            "a quiet gap does not end it"
        );

        sender.send(message("123456")).expect("sent");
        sender.send(message("7890ab")).expect("sent");
        let (kept, limited) = collect(&mut receiver, window).await;
        assert_eq!(
            (kept.len(), limited),
            (1, true),
            "the message past the bytes is not kept, and the wait says it was cut"
        );

        for n in 0..=MAX_MESSAGES {
            sender.send(message(&n.to_string())).expect("sent");
        }
        let window = Window {
            bytes: usize::MAX,
            ..window
        };
        let (kept, limited) = collect(&mut receiver, window).await;
        assert_eq!((kept.len(), limited), (MAX_MESSAGES, true));
    }
}
