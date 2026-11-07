//! Deploying approved DataSource pipelines as Bento streams on the project runner (T-0646, PL-47).
//!
//! Approved pipelines reading external feeds via `kind: DataSource` are rendered into native Bento
//! stream definitions and applied over the runner's streams REST API. Live state reflects the
//! runner's actual response, never Git alone.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jc_core::kinds::data_source::DataSourceSpec;
use jc_core::kinds::pipeline::{ComputeKind, PipelineSpec, Step};
use jc_core::Condition;
use serde_json::Value;

mod identity;
mod render;
pub use identity::*;
pub use render::*;

use super::run_queue::{self, Clock, Planned, RunQueue, RunState, Runner};
use crate::store::Mirror;

/// Timeout for requests sent to the runner's streams API.
const RUNNER_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for a PUT, POST or DELETE of one stream. Bento stops the old stream under the
/// request's own context, which has no deadline: it waits for the writes in flight, and a
/// gateway write may take its 30 s timeout four times over with Bento's default retries. A client
/// that hangs up first cancels that context, the forced stop then fails at once, and the runner
/// logs "Streams CRUD Error: context canceled" with the replace undone (T-4060).
/// ponytail: one fixed bound; a slower stop still ends in the backoff below.
const REPLACE_TIMEOUT: Duration = Duration::from_secs(180);

/// How many streams one pass sends to the runner at once. A render that changed every stream
/// (a new sink, T-4036) replaced 102 streams one after another, each waiting for its old
/// stream's writes, and the runner took half an hour to converge (T-4060).
const SENDS_AT_ONCE: usize = 8;

/// What the runner holds for one project, asked at most once per pass and only when a stream
/// needs it.
type Running = tokio::sync::OnceCell<Option<HashMap<String, bool>>>;

/// The first wait after a PUT the runner did not answer, doubled on each further miss up to
/// [`BACKOFF_CEILING`] (T-2891).
const BACKOFF_START: Duration = Duration::from_secs(60);
const BACKOFF_CEILING: Duration = Duration::from_secs(15 * 60);

/// The last PUT of one stream that the runner did not answer: the render it carried, when the
/// next attempt is due, the wait that led there, and what the transport said.
#[derive(Debug, Clone)]
struct Backoff {
    hash: u64,
    until: Instant,
    wait: Duration,
    reason: String,
}

/// Why a pipeline stream cannot be rendered from the manifest specifications.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The pipeline has no compute block or its compute kind is not bloblang.
    #[error("pipeline compute is missing or not bloblang")]
    MissingCompute,
    /// The pipeline class does not fit the data source (PL-04, PL-50).
    #[error("pipeline class does not fit data source: {0}")]
    Class(String),
    /// The author's `bento.yaml` is not YAML the runner would read.
    #[error("bento.yaml: {0}")]
    Bento(String),
    /// The author wrote an input and the pipeline names a DataSource too (PL-39).
    #[error("bento.yaml already declares an input, and spec.source.dataSourceRef names another one; keep one of the two")]
    BentoInput,
    /// Serialization between norway and json values failed.
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    /// A custom configuration error.
    #[error("{0}")]
    Custom(String),
}

/// The `bento.yaml` beside each Pipeline manifest, by `(project, pipeline)`: the author's
/// mapping the loader leaves alone (PL-03), read from the staged tree by the sync.
pub type Bentos = HashMap<(String, String), String>;

/// The result of attempting to apply one pipeline stream to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The runner accepted the stream and is executing it.
    Live,
    /// The pipeline is not eligible for streams mode (e.g. disabled or not a DataSource).
    Skipped(&'static str),
    /// The runner refused the stream, or an error occurred during render or network transport.
    Error(String),
    /// A pipeline that does not wait for pushes, run from the queue and deleted after each run
    /// (PL-78): the condition's reason (`Queued`, `Running`, `Waiting`) and what it says.
    Scheduled(&'static str, String),
}

/// Deploys and retires streams on project-specific pipeline runners.
pub struct StreamDeployer {
    runner_url: String,
    http: reqwest::Client,
    deployed: Mutex<HashSet<(String, String)>>,
    /// The hash of the config each live stream was last PUT with: an unchanged render is not
    /// sent again, because the runner restarts a stream on every PUT and a periodic pipeline
    /// then emits on every pass instead of every period (PL-45, T-0659).
    rendered: Mutex<HashMap<(String, String), u64>>,
    /// Streams whose last PUT the runner did not answer. Bento restarts a stream on PUT and
    /// waits for it to stop, and a stream blocked delivering data never stops, so every such
    /// PUT costs a whole [`RUNNER_TIMEOUT`]; sending the same render again on every pass kept
    /// a leader tick busy for minutes (T-2891). A changed render is sent at once.
    backoff: Mutex<HashMap<(String, String), Backoff>>,
    /// Where a refused record is posted (the Portal's internal listener, as the runner reaches
    /// it) and the compiled model of each space: with both, every stream into a space that
    /// names a model carries the validation stage (PL-60, PL-61).
    validation: Option<(String, Arc<crate::pipeline_validation::ModelSchemas>)>,
    /// The projects whose streams under the bare pipeline name were removed from the runner
    /// since this Portal started (T-3002): once per project, then never asked again.
    unqualified_retired: Mutex<HashSet<String>>,
    /// Whether each stream presents its own pipeline's identity through the runner's token
    /// sidecar rather than its project's `pipelines` account (PL-19, Architecture/12 §3).
    pipeline_identity: bool,
    /// [`REPLACE_TIMEOUT`], shorter in tests.
    replace_timeout: Duration,
    /// The pipelines that do not wait for pushes, each loaded only for its run (PL-78).
    queue: Mutex<RunQueue>,
    /// Where each ended run is told, for the snapshot pipelines' source deletes (T-4891).
    run_ends: Option<tokio::sync::mpsc::UnboundedSender<crate::pipeline_source_deletes::RunEnd>>,
    /// Each running run's dropped outcome reports at its start, the base its end is counted
    /// from (F-POR3-006).
    report_drops: Mutex<HashMap<run_queue::Key, u64>>,
}

/// How often the run queue looks at the runner.
const QUEUE_TICK: Duration = Duration::from_secs(5);

/// The runner's id of one project's stream. One runner may serve every project, and two
/// projects may each have a pipeline of the same name: under the bare name the last PUT won and
/// the other pipeline read Live while it did not run (T-3002). Project and pipeline names are
/// DNS labels, so the dot cannot come from either of them.
pub fn stream_id(project: &str, name: &str) -> String {
    format!("{project}.{name}")
}

/// The spaces a stream of `spec` in project `ns` writes into and reads out of, as its mapping
/// reads them: `env("JC_SPACE")`, `JC_SPACE_2`, … per output (PL-57, PF-84) and
/// `env("JC_SOURCE_SPACE")` per source Endpoint. The live stream and the test harness inject the
/// same values, so a mapping minting ids from them passes the test as it passes live (T-6567).
pub(crate) struct StreamSpaces {
    /// Each output Endpoint's Context Space, "" where the mirror lacks it.
    pub spaces: Vec<String>,
    segments: Vec<String>,
    /// A pipeline that computes from another space records where its figures came from, and
    /// that provenance is a URN the broker parses: with nothing injected the mapping rendered
    /// `urn:ngsi-ld:Endpoint:hel.fi:<nil>:<nil>`, every indicator was refused, and the indicator
    /// space stayed empty (T-2445). A source that names a DataSource contributes nothing here.
    sources: Vec<String>,
}

impl StreamSpaces {
    pub(crate) fn of(mirror: &Mirror, ns: &str, spec: &PipelineSpec) -> Self {
        let space_of = |owner: &str, endpoint: &str| {
            mirror
                .get(owner, "Endpoint", endpoint)
                .and_then(|ep| {
                    ep.spec.get("contextSpaceRef").and_then(|r| {
                        r.as_str()
                            .or_else(|| r.get("name").and_then(Value::as_str))
                            .map(str::to_owned)
                    })
                })
                .unwrap_or_default()
        };
        let spaces: Vec<String> = spec
            .outputs()
            .iter()
            .map(|output| space_of(ns, output.target_endpoint.local_id()))
            .collect();
        let segments = spaces
            .iter()
            .map(|space| crate::spaces::segment(mirror, ns, space))
            .collect();
        let sources = spec
            .sources()
            .iter()
            .filter_map(|source| source.endpoint_ref.as_ref())
            .map(|reference| {
                let owner = reference.namespace().unwrap_or(ns);
                crate::spaces::segment(mirror, owner, &space_of(owner, reference.name()))
            })
            .collect();
        Self {
            spaces,
            segments,
            sources,
        }
    }

    /// Writes the spaces into every string of `stream` that reads them.
    pub(crate) fn inject(&self, stream: &mut Value) {
        jcctl::bento::inject_space(stream, &self.segments);
        inject_source_space(stream, &self.sources);
    }
}

/// Every text of the pipeline the runner runs as Bloblang or as a processor passes the check it
/// passed at write time (PL-16, PL-52): no read of the runner's environment but the pipeline's
/// own variables, no refused processor. A manifest pushed to the forge past the Portal is
/// loaded with only a warning (daemon.rs), and the runner holds every project's credentials,
/// so the check runs again before anything is rendered. `bento.yaml` has its own in the render.
fn reads_only_its_own(spec: &PipelineSpec) -> Result<(), String> {
    let own = spec.own_variables();
    let check = |field: &'static str, processor: Value| {
        jc_core::kinds::pipeline::validate_processor_reading(field, &processor, &own)
            .map_err(|err| err.to_string())
    };
    let mut steps = spec.steps();
    steps.extend(spec.routes.iter().flat_map(|route| route.steps.clone()));
    for step in &steps {
        match step {
            Step::Processor(step) => {
                for (name, config) in &step.processor {
                    check("spec.steps.processor", serde_json::json!({ name: config }))?;
                }
            }
            Step::Compute(compute) => {
                if let Some(text) = compute.mapping().map_err(|err| err.to_string())? {
                    check(
                        "spec.compute.bloblang",
                        serde_json::json!({ "mapping": text }),
                    )?;
                }
            }
        }
    }
    for route in &spec.routes {
        check(
            "spec.routes.when",
            serde_json::json!({ "mapping": route.when }),
        )?;
    }
    if let Some(marker) = spec
        .source_deletes
        .as_ref()
        .and_then(|rule| rule.deleted_when.as_deref())
    {
        check(
            "spec.sourceDeletes.deletedWhen",
            serde_json::json!({ "mapping": marker }),
        )?;
    }
    Ok(())
}

/// The rendered config as one number, so two passes can tell an unchanged stream apart.
fn config_hash(config: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config.to_string().hash(&mut hasher);
    hasher.finish()
}

/// Whether the runner's answer holds `rendered`. Bento answers a stream's config with every
/// `${VAR}` replaced from the runner's environment (checked against the pinned 1.21.1, T-3006), so
/// an exact comparison never held for a stream carrying its credential or the gateway's address,
/// and every restarted Portal sent every stream again, which restarts it and fires its first tick
/// out of schedule. Here a `${VAR}` or `${VAR:default}` matches whatever the runner filled in, and
/// every other character, key and item must be the same; `${! … }` is Bento's per-message
/// interpolation, which the runner keeps as written.
///
/// ponytail: a render that changed only the name of a variable reads as held; the runner is rolled
/// when its environment changes (`joinedcontext.com/pipeline-secrets`), which re-sends it anyway.
fn held_as_rendered(rendered: &Value, held: &Value) -> bool {
    match (rendered, held) {
        (Value::String(rendered), Value::String(held)) => interpolated(rendered, held),
        (Value::Array(rendered), Value::Array(held)) => {
            rendered.len() == held.len()
                && rendered
                    .iter()
                    .zip(held)
                    .all(|(rendered, held)| held_as_rendered(rendered, held))
        }
        (Value::Object(rendered), Value::Object(held)) => {
            rendered.len() == held.len()
                && rendered.iter().all(|(key, rendered)| {
                    held.get(key)
                        .is_some_and(|held| held_as_rendered(rendered, held))
                })
        }
        (rendered, held) => rendered == held,
    }
}

/// A fingerprint of the project's spaces, data models and policies: what decides whether its
/// pipelines' writes are taken (T-5687). Statuses are left out, they change on every pass.
fn rules_fingerprint(mirror: &Mirror, ns: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for kind in ["ContextSpace", "DataModel", "Policy"] {
        for envelope in mirror
            .list(ns, kind, &crate::store::ListOptions::default())
            .items
        {
            (kind, &envelope.metadata.name, envelope.spec.to_string()).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Whether `held` is `rendered` with each environment reference replaced by some value.
fn interpolated(rendered: &str, held: &str) -> bool {
    static REFERENCE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\$\{[A-Za-z_][A-Za-z0-9_]*(?::[^}]*)?\}").expect("a valid pattern")
    });
    if !REFERENCE.is_match(rendered) {
        return rendered == held;
    }
    let mut pattern = String::from("^");
    let mut from = 0;
    for reference in REFERENCE.find_iter(rendered) {
        pattern.push_str(&regex::escape(&rendered[from..reference.start()]));
        pattern.push_str("(?s:.*)");
        from = reference.end();
    }
    pattern.push_str(&regex::escape(&rendered[from..]));
    pattern.push('$');
    regex::Regex::new(&pattern).is_ok_and(|pattern| pattern.is_match(held))
}

impl StreamDeployer {
    /// Creates a new deployer targeting the specified runner URL template (may contain `{project}`).
    pub fn new(runner_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(RUNNER_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            runner_url: runner_url.into(),
            http,
            deployed: Mutex::new(HashSet::new()),
            rendered: Mutex::new(HashMap::new()),
            backoff: Mutex::new(HashMap::new()),
            validation: None,
            unqualified_retired: Mutex::new(HashSet::new()),
            pipeline_identity: false,
            replace_timeout: REPLACE_TIMEOUT,
            queue: Mutex::new(RunQueue::new(1, 1)),
            run_ends: None,
            report_drops: Mutex::new(HashMap::new()),
        }
    }

    /// Tells `ends` of every run the queue saw end (T-4891).
    pub fn with_run_ends(
        mut self,
        ends: tokio::sync::mpsc::UnboundedSender<crate::pipeline_source_deletes::RunEnd>,
    ) -> Self {
        self.run_ends = Some(ends);
        self
    }

    /// Runs at most `batch` batch-lane and `short` short-lane pipelines at once (PL-78).
    pub fn with_concurrency(mut self, batch: usize, short: usize) -> Self {
        self.queue = Mutex::new(RunQueue::new(batch, short));
        self
    }

    /// Drives the run queue against the runner every `QUEUE_TICK`. The queue only holds what
    /// [`Self::converge`] planned, so a Portal that does not lead starts nothing.
    pub fn spawn_queue(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut every = tokio::time::interval(QUEUE_TICK);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                self.tick_queue(chrono::Utc::now()).await;
            }
        })
    }

    /// One step of the run queue at `now`: what [`Self::spawn_queue`] does every few seconds.
    pub async fn tick_queue(&self, now: chrono::DateTime<chrono::Utc>) {
        run_queue::tick(&self.queue, self, now).await;
    }

    /// Tells the run queue which pipelines have ever run and when their newest run last wrote, so
    /// a scheduled one that never did runs on its first plan instead of at its next clock
    /// (T-4421), and one that missed its due time while no Portal held it runs under that time
    /// (T-5778).
    pub fn ran(
        &self,
        ran: std::collections::HashMap<(String, String), chrono::DateTime<chrono::Utc>>,
    ) {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ran(ran);
    }

    /// Tells the run queue the stream hash of each pipeline's newest logged run, so a replace
    /// pipeline whose stream changed while no Portal held it runs at once (T-5825).
    pub fn streams(&self, streams: std::collections::HashMap<(String, String), String>) {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .streams(streams);
    }

    /// Tells the run queue which pipelines' newest run failed for a cause outside their source,
    /// with that run and the cause, so each is retried with backoff (T-5687).
    pub fn failed(&self, failed: std::collections::BTreeMap<(String, String), (String, String)>) {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .failed(failed);
    }

    /// Tells the run queue which workload accounts (`project/name`) have no Keycloak client
    /// after this reconcile, so their pipelines are not started into a 401 loop (T-4426).
    pub fn unbound(&self, accounts: HashSet<String>) {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unbound(accounts);
    }

    /// Queues one run of a run-and-delete pipeline now (T-4349) and returns its id, the one its
    /// log lists it under.
    pub fn run_now(&self, project: &str, name: &str) -> Result<String, run_queue::RunNow> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .run_now(&(project.to_owned(), name.to_owned()), chrono::Utc::now())
    }

    /// Renders every stream with its own pipeline's identity (PL-19): set once the runner carries
    /// the token sidecar and the federated mechanism is seen working.
    pub fn with_pipeline_identity(mut self, on: bool) -> Self {
        self.pipeline_identity = on;
        self
    }

    fn identified(&self, stream: &mut Value, project: &str, pipeline: &str) {
        if self.pipeline_identity {
            with_pipeline_identity(stream, project, pipeline);
        }
    }

    /// Validates every record against its space's model before the write (PL-60): `internal`
    /// is the base URL of the Portal's internal listener as the runner reaches it.
    pub fn with_validation(
        mut self,
        internal: impl Into<String>,
        schemas: Arc<crate::pipeline_validation::ModelSchemas>,
    ) -> Self {
        self.validation = Some((internal.into(), schemas));
        self
    }

    /// The compiled models this deployer renders stages from, when it validates.
    pub fn model_schemas(&self) -> Option<&Arc<crate::pipeline_validation::ModelSchemas>> {
        self.validation.as_ref().map(|(_, schemas)| schemas)
    }

    /// The stream's sinks to the Portal's internal listener: the validation stage's rejected
    /// sink, the outcome sink and the samples taps, each presenting the stream's own project's
    /// `pipelines` client (T-4038). The listener admits only a client of the pipeline's own
    /// project (T-3193); the runner-wide `${JC_CLIENT_ID}` is one project's, so every other
    /// project's reports answered 401 and were dropped.
    fn reporting(&self, stream: &mut Value, project: &str, pipeline: &str, spaces: &[String]) {
        self.validated(stream, project, pipeline, spaces);
        self.logged(stream, project, pipeline);
        own_credential(stream, project);
    }

    /// Puts the validation stage into one rendered stream when every output writes into the
    /// same space and that space names a model (PL-60). A stream that fans out into spaces of
    /// different models is left without it and says so.
    /// ponytail: one stage per stream; per-output stages are the upgrade when a fan-out needs them.
    fn validated(&self, stream: &mut Value, project: &str, pipeline: &str, spaces: &[String]) {
        let Some((internal, schemas)) = &self.validation else {
            return;
        };
        let Some(space) = spaces.first() else {
            return;
        };
        if spaces.iter().any(|other| other != space) {
            tracing::warn!(
                project,
                pipeline,
                "the outputs write into several spaces; the stream is not validated"
            );
            return;
        }
        let Some(schema) = schemas.get(project, space) else {
            return;
        };
        let sink = format!(
            "{}/internal/pipelines/{project}/{pipeline}/rejected",
            internal.trim_end_matches('/')
        );
        if !crate::pipeline_validation::insert_stage(stream, &schema, &sink) {
            tracing::warn!(
                project,
                pipeline,
                "the stream has no batch split; the stage is not rendered"
            );
        }
    }

    /// Sends the outcome of every batch the stream's write took to the pipeline's log (PL-62).
    fn logged(&self, stream: &mut Value, project: &str, pipeline: &str) {
        let Some((internal, _)) = &self.validation else {
            return;
        };
        let Some(output) = stream.get_mut("output") else {
            return;
        };
        let url = format!(
            "{}/internal/pipelines/{project}/{pipeline}/outcomes",
            internal.trim_end_matches('/')
        );
        *output = crate::pipeline_log::with_outcomes(output.take(), &url);
        with_source_events(
            stream,
            &format!(
                "{}/internal/pipelines/{project}/{pipeline}/source-events",
                internal.trim_end_matches('/')
            ),
        );
        crate::pipeline_log::with_passes(stream, &url);
        with_source_failures(stream, &url);
        crate::pipeline_log::with_problems(stream, KPI_PROBLEM_LABEL, &url);
        // What last crossed each node, for the studio's drawer (T-3974).
        let samples = format!(
            "{}/internal/pipelines/{project}/{pipeline}/samples",
            internal.trim_end_matches('/')
        );
        crate::pipeline_samples::with_taps(stream, &samples, &stream_id(project, pipeline));
    }

    /// The runner's Prometheus text for one project, or `None` when it does not answer.
    ///
    /// A stream can be Live and still read nothing — a source that refuses the runner's token,
    /// a mapping that throws on every message — and the counters are the only place that shows
    /// (T-0914).
    pub async fn metrics(&self, project: &str) -> Option<String> {
        let runner = self.runner_for(project)?;
        let url = format!("{runner}/metrics");
        let response = self.http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.text().await.ok()
    }

    /// The stream's dropped outcome reports on its runner, cumulative over its runs (F-POR3-006);
    /// `Err` when the runner's metrics did not answer.
    async fn dropped_reports(&self, key: &run_queue::Key) -> Result<Option<u64>, &'static str> {
        let body = self
            .metrics(&key.0)
            .await
            .ok_or("the runner's metrics did not answer")?;
        Ok(crate::api::pipelines::dropped_reports(
            &body,
            &stream_id(&key.0, &key.1),
        ))
    }

    /// The runner's base URL for one project, or nothing when the name is not a project name.
    ///
    /// The template may carry `{project}` in its authority — `http://bento-{project}.streams.svc`
    /// is the natural shape of a runner service per project — so an unchecked name moves the
    /// request to another host, and one carrying `/` moves it outside the project's own path
    /// (T-2298). The check is here as well as at the mirror's door, because either one alone
    /// leaves the other caller open.
    fn runner_for(&self, project: &str) -> Option<String> {
        if !crate::resource::is_dns1123(project) {
            tracing::warn!(project = %project, "the runner is not asked about a name that is not a project");
            return None;
        }
        Some(
            self.runner_url
                .replace("{project}", project)
                .trim_end_matches('/')
                .to_owned(),
        )
    }

    /// Renders and PUTs every eligible Pipeline of `mirror`; returns (namespace, name, outcome).
    pub async fn converge(
        &self,
        mirror: &Mirror,
        bentos: &Bentos,
        refused: &std::collections::BTreeMap<(String, String), String>,
    ) -> Vec<(String, String, StreamOutcome)> {
        let mut outcomes = Vec::new();
        let mut current_live = HashSet::new();
        // The streams to send, each with its place in `outcomes`: sent after the loop, a few at
        // once (T-4060).
        let mut sends: Vec<(usize, String, String, Value)> = Vec::new();
        // The pipelines that do not wait for pushes, for the run queue (PL-78).
        let mut queued: std::collections::BTreeMap<(String, String), Planned> =
            std::collections::BTreeMap::new();

        for ns in mirror.namespaces() {
            // What the project may keep resident (PF-74): the ones beyond its quota are not
            // scheduled at all, and the condition on each says why.
            let beyond = crate::quotas::beyond(mirror, &ns, "residentPipelines");
            let page = mirror.list(&ns, "Pipeline", &crate::store::ListOptions::default());
            self.retire_unqualified(&ns, page.items.iter().map(|e| e.metadata.name.as_str()))
                .await;
            for envelope in page.items {
                let name = envelope.metadata.name.clone();
                // A pipeline whose credential did not resolve is not started: a stream that
                // cannot connect fails in the runner's log, where nobody is looking (T-0927).
                if let Some(reason) = refused.get(&(ns.clone(), name.clone())) {
                    outcomes.push((ns.clone(), name, StreamOutcome::Error(reason.clone())));
                    continue;
                }
                if beyond.contains(&name) {
                    outcomes.push((
                        ns.clone(),
                        name,
                        StreamOutcome::Skipped(
                            "the project's resident pipeline quota is used up, so this one is \
                             not scheduled (PF-73, PF-74)",
                        ),
                    ));
                    continue;
                }
                let spec = serde_json::from_value::<PipelineSpec>(envelope.spec.clone())
                    .map_err(|err| err.to_string())
                    .and_then(|spec| reads_only_its_own(&spec).map(|()| spec));
                let spec = match spec {
                    Ok(s) => s,
                    Err(err) => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(format!("invalid pipeline spec: {err}")),
                        ));
                        continue;
                    }
                };

                let first = spec.sources().into_iter().next();
                let ds_ref_name = first
                    .as_ref()
                    .and_then(|s| s.data_source_ref.as_ref())
                    .map(|r| r.name().to_string());

                let ep_ref = first.as_ref().and_then(|s| s.endpoint_ref.clone());
                let ep_ref_name = ep_ref.as_ref().map(|r| r.name().to_string());

                if ds_ref_name.is_none() && ep_ref_name.is_none() {
                    outcomes.push((
                        ns.clone(),
                        name,
                        StreamOutcome::Skipped("not a supported stream pipeline"),
                    ));
                    continue;
                }

                if !eligible(&spec) {
                    let reason = if !spec.enabled {
                        "pipeline is disabled"
                    } else if let Some(compute) = first_compute(&spec) {
                        if !compute.kind.is_mapping() {
                            "compute is not bloblang"
                        } else {
                            "endpoint pipeline requires bloblang mapping"
                        }
                    } else {
                        "not eligible for streams mode"
                    };
                    outcomes.push((ns.clone(), name, StreamOutcome::Skipped(reason)));
                    continue;
                }

                let stream_spaces = StreamSpaces::of(mirror, &ns, &spec);
                let spaces = &stream_spaces.spaces;
                // Every output's Endpoint, from the mirror (PL-52, PL-55).
                let slugs: Result<Vec<String>, String> = spec
                    .outputs()
                    .iter()
                    .map(|output| {
                        let ep_name = output.target_endpoint.local_id().to_string();
                        let ep_env = mirror.get(&ns, "Endpoint", &ep_name).ok_or_else(|| {
                            format!("target endpoint {ep_name} is not in the mirror")
                        })?;
                        ep_env
                            .spec
                            .get("slug")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .ok_or_else(|| format!("target endpoint {ep_name} has no slug"))
                    })
                    .collect();
                let slugs = match slugs {
                    Ok(slugs) if !slugs.is_empty() => slugs,
                    Ok(_) => {
                        outcomes.push((
                            ns.clone(),
                            name,
                            StreamOutcome::Error(
                                "the pipeline names no target endpoint".to_owned(),
                            ),
                        ));
                        continue;
                    }
                    Err(reason) => {
                        outcomes.push((ns.clone(), name, StreamOutcome::Error(reason)));
                        continue;
                    }
                };

                if is_merged(&spec) {
                    let resolved: Result<Vec<ResolvedSource>, String> = spec
                        .sources()
                        .into_iter()
                        .map(|source| resolve_source(mirror, &ns, source))
                        .collect();
                    let resident = resolved.as_ref().is_ok_and(|sources| {
                        sources.iter().any(|source| match source {
                            ResolvedSource::Data { spec, .. } => {
                                run_queue::is_resident(&spec.source_type)
                            }
                            ResolvedSource::Endpoint { source, .. } => source.trigger.is_some(),
                        })
                    });
                    let rendered = resolved.and_then(|sources| {
                        render_merged(&spec, &name, &ns, &sources, &slugs)
                            .map_err(|err| err.to_string())
                    });
                    match rendered {
                        Ok(mut stream_json) => {
                            self.reporting(&mut stream_json, &ns, &name, spaces);
                            self.identified(&mut stream_json, &ns, &name);
                            stream_spaces.inject(&mut stream_json);
                            place(
                                (&ns, name, stream_json),
                                resident,
                                &spec,
                                &mut sends,
                                &mut outcomes,
                                &mut queued,
                            );
                        }
                        Err(reason) => {
                            outcomes.push((ns.clone(), name, StreamOutcome::Error(reason)))
                        }
                    }
                    continue;
                }
                let target_slug = slugs[0].clone();
                // An Endpoint read with a change trigger watches its source; one on a clock polls.
                let mut resident = first
                    .as_ref()
                    .is_some_and(|source| source.trigger.is_some());

                let stream_json = if let Some(ds_name) = ds_ref_name {
                    let ds_env = match mirror.get(&ns, "DataSource", &ds_name) {
                        Some(e) => e,
                        None => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(format!(
                                    "data source {ds_name} is not in the mirror"
                                )),
                            ));
                            continue;
                        }
                    };

                    let ds_spec: DataSourceSpec = match serde_json::from_value(ds_env.spec.clone())
                    {
                        Ok(s) => s,
                        Err(err) => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(format!("invalid data source spec: {err}")),
                            ));
                            continue;
                        }
                    };

                    resident = run_queue::is_resident(&ds_spec.source_type);
                    let bento = bentos.get(&(ns.clone(), name.clone())).map(String::as_str);
                    match render_stream(&spec, &name, &ns, &ds_spec, &ds_name, &target_slug, bento)
                    {
                        Ok(val) => val,
                        Err(err) => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(err.to_string()),
                            ));
                            continue;
                        }
                    }
                } else if let Some(reference) = ep_ref {
                    let source_ep_name = reference.name().to_owned();
                    let source_ep_env = match source_endpoint(mirror, &ns, &reference) {
                        Ok(e) => e,
                        Err(reason) => {
                            outcomes.push((ns.clone(), name, StreamOutcome::Error(reason)));
                            continue;
                        }
                    };

                    let source_slug = match source_ep_env.spec.get("slug").and_then(Value::as_str) {
                        Some(s) => s,
                        None => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(format!(
                                    "source endpoint {source_ep_name} has no slug"
                                )),
                            ));
                            continue;
                        }
                    };

                    let source_is_public = source_ep_env
                        .spec
                        .get("audience")
                        .and_then(Value::as_str)
                        .is_none_or(|audience| audience == "public");

                    match render_endpoint_stream(
                        &spec,
                        &ns,
                        &format!("{ns}/{name}"),
                        source_slug,
                        source_is_public,
                        &target_slug,
                    ) {
                        Ok(val) => val,
                        Err(err) => {
                            outcomes.push((
                                ns.clone(),
                                name,
                                StreamOutcome::Error(err.to_string()),
                            ));
                            continue;
                        }
                    }
                } else {
                    unreachable!();
                };

                let mut stream_json = stream_json;
                self.reporting(&mut stream_json, &ns, &name, spaces);
                self.identified(&mut stream_json, &ns, &name);
                stream_spaces.inject(&mut stream_json);
                place(
                    (&ns, name, stream_json),
                    resident,
                    &spec,
                    &mut sends,
                    &mut outcomes,
                    &mut queued,
                );
            }
        }
        self.send_all(sends, &mut outcomes, &mut current_live).await;
        let queued_keys: HashSet<(String, String)> = queued.keys().cloned().collect();
        {
            let rules = mirror
                .namespaces()
                .into_iter()
                .map(|ns| {
                    let fingerprint = rules_fingerprint(mirror, &ns);
                    (ns, fingerprint)
                })
                .collect();
            let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
            queue.rules(rules);
            queue.plan(queued, chrono::Utc::now());
            for (ns, name, outcome) in outcomes.iter_mut() {
                let key = (ns.clone(), name.clone());
                if queued_keys.contains(&key) {
                    if let Some(said) = queue.outcome(&key) {
                        *outcome = said;
                    }
                }
            }
        }
        let mut sends: Vec<(usize, String, String, Value)> = Vec::new();

        // The sweeps of the pipelines that remove stale entities (PL-64), after every pipeline's
        // own stream, so a sweep's outcome lands on its pipeline after the pipeline's own.
        for ns in mirror.namespaces() {
            let page = mirror.list(&ns, "Pipeline", &crate::store::ListOptions::default());
            for envelope in page.items {
                let name = envelope.metadata.name;
                let Ok(spec) = serde_json::from_value::<PipelineSpec>(envelope.spec) else {
                    continue;
                };
                let Some(expiry) = spec.expiry.as_ref() else {
                    continue;
                };
                if !spec.enabled || refused.contains_key(&(ns.clone(), name.clone())) {
                    continue;
                }
                let sweep = crate::pipeline_expiry::sweep_name(&name);
                let slug = spec.outputs().first().and_then(|output| {
                    mirror
                        .get(&ns, "Endpoint", output.target_endpoint.local_id())
                        .and_then(|ep| {
                            ep.spec
                                .get("slug")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                });
                let stream = slug
                    .as_deref()
                    .map(|slug| crate::pipeline_expiry::render_sweep(&ns, slug, expiry));
                let outcome = match stream {
                    Some(Some(mut stream)) => {
                        // The sweep deletes as the pipeline it sweeps for (PL-20).
                        self.identified(&mut stream, &ns, &name);
                        sends.push((outcomes.len(), ns.clone(), sweep.clone(), stream));
                        StreamOutcome::Live
                    }
                    // Never read as a window of zero, which deletes everything (PL-64).
                    Some(None) => StreamOutcome::Error(format!(
                        "expiry `{}` over {:?} is not a window of 1h to 365d over one to {} \
                         distinct entity types, so nothing is swept",
                        expiry.after,
                        expiry.types,
                        jc_core::kinds::pipeline::EXPIRY_MAX_TYPES
                    )),
                    None => StreamOutcome::Error(
                        "the target endpoint is not in the mirror or has no slug, so nothing \
                         is swept"
                            .to_owned(),
                    ),
                };
                outcomes.push((ns.clone(), sweep, outcome));
            }
        }
        self.send_all(sends, &mut outcomes, &mut current_live).await;

        // Retire streams that were deployed previously but are no longer active or eligible.
        let to_retire: Vec<(String, String)> = {
            let deployed = self.deployed.lock().unwrap_or_else(|p| p.into_inner());
            // A queued pipeline's stream is its run's, which the queue deletes itself.
            deployed
                .difference(&current_live)
                .filter(|key| !queued_keys.contains(*key))
                .cloned()
                .collect()
        };
        let mut by_project: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (ns, name) in to_retire {
            by_project.entry(ns).or_default().push(name);
        }
        for (project, names) in by_project {
            self.retire(&project, &names).await;
        }

        if let Ok(mut deployed) = self.deployed.lock() {
            *deployed = current_live;
        }

        outcomes
    }

    /// Applies each stream, [`SENDS_AT_ONCE`] at a time, writing each outcome into its place:
    /// one stream whose old copy is slow to stop no longer holds every stream after it (T-4060).
    async fn send_all(
        &self,
        sends: Vec<(usize, String, String, Value)>,
        outcomes: &mut [(String, String, StreamOutcome)],
        current_live: &mut HashSet<(String, String)>,
    ) {
        use futures_util::StreamExt;
        let running: HashMap<String, Running> = sends
            .iter()
            .map(|(_, ns, _, _)| (ns.clone(), Running::new()))
            .collect();
        let running = &running;
        let mut applied = futures_util::stream::iter(sends)
            .map(|(at, ns, name, stream)| async move {
                let outcome = match running.get(&ns) {
                    Some(running) => self.apply(&ns, &name, stream, running).await,
                    None => StreamOutcome::Error("no runner list for the project".to_owned()),
                };
                (at, ns, name, outcome)
            })
            .buffer_unordered(SENDS_AT_ONCE);
        while let Some((at, ns, name, outcome)) = applied.next().await {
            if outcome == StreamOutcome::Live {
                current_live.insert((ns, name));
            }
            if let Some(slot) = outcomes.get_mut(at) {
                slot.2 = outcome;
            }
        }
    }

    /// Sends one rendered stream to the runner unless it already runs this exact render, and
    /// records it as live when the runner has it.
    async fn apply(
        &self,
        ns: &str,
        name: &str,
        mut stream_json: Value,
        running: &Running,
    ) -> StreamOutcome {
        own_credential(&mut stream_json, ns);
        let key = (ns.to_owned(), name.to_owned());
        let hash = config_hash(&stream_json);
        let stored = self
            .rendered
            .lock()
            .map(|rendered| rendered.get(&key).copied())
            .unwrap_or(None);
        let mut unchanged = stored == Some(hash);
        // Why the stream is sent, when it is: a PUT restarts it and fires a clock's first tick
        // out of schedule, so every one is logged with its reason (T-3007).
        let mut why = if stored.is_some() {
            "its render changed"
        } else {
            "this Portal has not sent it and the runner does not run it as rendered"
        };
        // A runner that restarted holds no streams, so an unchanged render it no longer runs is
        // sent again; a runner that does not answer the list keeps the hash's word. A stream the
        // runner holds but reports inactive is sent again too, unless its input is one that ends
        // by itself: a clock, a broker and a socket never finish, so inactive means that stream
        // died — a source it cannot reach, or an output that refuses its credential — and it is
        // the reconciler counting it as live that left the city's `voda` stream dead across two
        // applies while its render was unchanged (T-2336, PL-18, OPS-27). Sending it again
        // restarts it on the runner, and its output fetches a fresh token as it starts.
        if unchanged {
            unchanged = match running.get_or_init(|| self.running(ns)).await {
                None => true,
                Some(streams) => match streams.get(&stream_id(ns, name)) {
                    None => {
                        why = "the runner does not hold it";
                        false
                    }
                    Some(true) => true,
                    Some(false) => {
                        why = "the runner holds it but it is not running";
                        let finished = input_ends_by_itself(&stream_json["input"]);
                        if !finished {
                            tracing::warn!(
                                project = %ns,
                                pipeline = %name,
                                "the runner holds this stream but it is not running; sending it \
                                 again"
                            );
                        }
                        finished
                    }
                },
            };
        } else if stored.is_none() {
            // No hash means this Portal has not sent the stream since it started. The runner
            // answers a stream's config as it was sent, its environment filled in, so a stream it
            // runs as rendered is adopted rather than PUT again: every restart used to PUT every
            // stream, a PUT restarts the stream and fires its first tick (T-3006), and a stream
            // the runner cannot stop made each of those PUTs wait out the timeout (T-2891).
            unchanged = self.holds_as_rendered(ns, name, &stream_json).await;
        }
        let outcome = if unchanged {
            StreamOutcome::Live
        } else if let Some(waiting) = self.waiting(&key, hash) {
            waiting
        } else {
            tracing::info!(
                project = %ns,
                pipeline = %name,
                render = format_args!("{hash:016x}"),
                "sending the stream to the runner, which restarts it: {why}"
            );
            match self.deploy_stream(ns, name, &stream_json).await {
                Ok(outcome) => outcome,
                Err(reason) => self.back_off(key.clone(), hash, reason),
            }
        };
        if outcome == StreamOutcome::Live {
            if let Ok(mut rendered) = self.rendered.lock() {
                rendered.insert(key.clone(), hash);
            }
            if let Ok(mut backoff) = self.backoff.lock() {
                backoff.remove(&key);
            }
        }
        outcome
    }

    /// Whether the runner holds `name` with exactly this config and it still runs (or its input
    /// ends by itself, as in [`Self::apply`]). Anything else, the runner not answering included,
    /// is "no": the stream is then sent as before.
    async fn holds_as_rendered(&self, project: &str, name: &str, stream_json: &Value) -> bool {
        let Some(runner) = self.runner_for(project) else {
            return false;
        };
        let Ok(response) = self
            .http
            .get(format!("{runner}/streams/{}", stream_id(project, name)))
            .send()
            .await
        else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        let Ok(held) = response.json::<Value>().await else {
            return false;
        };
        let alive = held.get("active").and_then(Value::as_bool).unwrap_or(true)
            || input_ends_by_itself(&stream_json["input"]);
        let same = held
            .get("config")
            .is_some_and(|config| held_as_rendered(stream_json, config));
        if same && alive {
            tracing::info!(
                project = %project,
                pipeline = %name,
                "the runner already runs this stream as rendered; not sent again"
            );
        }
        same && alive
    }

    /// The outcome of a stream whose last PUT of this same render the runner did not answer,
    /// while its wait lasts; `None` when it is due, or when the render changed since.
    fn waiting(&self, key: &(String, String), hash: u64) -> Option<StreamOutcome> {
        let backoff = self.backoff.lock().ok()?;
        let last = backoff.get(key)?;
        let left = last.until.checked_duration_since(Instant::now())?;
        (last.hash == hash).then(|| {
            StreamOutcome::Error(format!(
                "the pipeline runner did not answer the last change of this stream ({}); the \
                 Portal sends it again in {} s",
                last.reason,
                left.as_secs().max(1)
            ))
        })
    }

    /// Records a PUT the runner did not answer and says when the next one goes out: the wait
    /// doubles while the same render keeps missing, and starts over for a new one.
    fn back_off(&self, key: (String, String), hash: u64, reason: String) -> StreamOutcome {
        let wait = self
            .backoff
            .lock()
            .ok()
            .and_then(|backoff| {
                backoff
                    .get(&key)
                    .filter(|last| last.hash == hash)
                    .map(|last| last.wait)
            })
            .map_or(BACKOFF_START, |last| (last * 2).min(BACKOFF_CEILING));
        tracing::warn!(
            project = %key.0,
            pipeline = %key.1,
            wait_secs = wait.as_secs(),
            %reason,
            "the pipeline runner did not answer the stream's PUT; waiting before the next one"
        );
        let said = format!(
            "the pipeline runner did not answer: {reason}; the Portal sends it again in {} s",
            wait.as_secs()
        );
        if let Ok(mut backoff) = self.backoff.lock() {
            backoff.insert(
                key,
                Backoff {
                    hash,
                    until: Instant::now() + wait,
                    wait,
                    reason,
                },
            );
        }
        StreamOutcome::Error(said)
    }

    /// DELETE {runner}/streams/{project}.{name} for pipelines that were deployed last run and are gone or disabled now.
    pub async fn retire(&self, project: &str, names: &[String]) {
        let Some(runner) = self.runner_for(project) else {
            return;
        };
        for name in names {
            self.remove(
                &runner,
                project,
                name,
                "it was live on the last pass and is not now",
            )
            .await;
        }
    }

    /// DELETE one stream, a stream that is not there included, and forget it. False when the
    /// runner gave no answer within [`REPLACE_TIMEOUT`]: a stream whose step never returns
    /// never stops, and the hang-up leaves it held, stopped in name only (T-5370).
    async fn remove(&self, runner: &str, project: &str, name: &str, why: &str) -> bool {
        {
            let url = format!("{runner}/streams/{}", stream_id(project, name));
            tracing::info!(
                project = %project,
                pipeline = %name,
                "removing the stream from the runner: {why}"
            );
            let answered = match self
                .http
                .delete(&url)
                .timeout(self.replace_timeout)
                .send()
                .await
            {
                Ok(resp) => {
                    if !resp.status().is_success()
                        && resp.status() != reqwest::StatusCode::NOT_FOUND
                    {
                        tracing::warn!(
                            project = %project,
                            pipeline = %name,
                            status = %resp.status(),
                            "failed to delete stream from runner"
                        );
                    }
                    true
                }
                Err(err) => {
                    tracing::warn!(
                        project = %project,
                        pipeline = %name,
                        error = %err,
                        "failed to delete stream from runner"
                    );
                    !err.is_timeout()
                }
            };
            let key = (project.to_string(), name.to_string());
            if let Ok(mut deployed) = self.deployed.lock() {
                deployed.remove(&key);
            }
            if let Ok(mut rendered) = self.rendered.lock() {
                rendered.remove(&key);
            }
            if let Ok(mut backoff) = self.backoff.lock() {
                backoff.remove(&key);
            }
            answered
        }
    }

    /// Removes the streams a Portal before T-3002 put on the runner under the bare pipeline name
    /// (and `{name}.expiry` for its sweep), once per project: left there, each would go on
    /// writing beside the stream under its new id. A runner that gives no list is asked again on
    /// the next pass. On a runner shared by several projects the bare name may be another
    /// project's old stream; that one is re-sent under its own id in the same pass.
    async fn retire_unqualified<'a>(&self, project: &str, names: impl Iterator<Item = &'a str>) {
        let done = self
            .unqualified_retired
            .lock()
            .map(|done| done.contains(project))
            .unwrap_or(true);
        if done {
            return;
        }
        let (Some(runner), Some(held)) = (self.runner_for(project), self.running(project).await)
        else {
            return;
        };
        for name in names {
            for old in [name.to_owned(), crate::pipeline_expiry::sweep_name(name)] {
                if !held.contains_key(&old) {
                    continue;
                }
                match self
                    .http
                    .delete(format!("{runner}/streams/{old}"))
                    .timeout(self.replace_timeout)
                    .send()
                    .await
                {
                    Ok(resp)
                        if resp.status().is_success()
                            || resp.status() == reqwest::StatusCode::NOT_FOUND =>
                    {
                        tracing::info!(project = %project, stream = %old, "removed the stream kept under its bare pipeline name");
                    }
                    Ok(resp) => {
                        tracing::warn!(project = %project, stream = %old, status = %resp.status(), "the runner kept the stream under its bare pipeline name");
                        return;
                    }
                    Err(err) => {
                        tracing::warn!(project = %project, stream = %old, error = %err, "the runner kept the stream under its bare pipeline name");
                        return;
                    }
                }
            }
        }
        if let Ok(mut done) = self.unqualified_retired.lock() {
            done.insert(project.to_owned());
        }
    }

    /// What the runner holds for one project, each with whether it is still running, or `None`
    /// when it gave no list.
    ///
    /// A body without the `active` flag counts as running, because a runner whose answer this does
    /// not understand must not have all of its streams restarted on every pass.
    async fn running(&self, project: &str) -> Option<HashMap<String, bool>> {
        let runner = self.runner_for(project)?;
        let url = format!("{runner}/streams");
        let response = self.http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let streams: HashMap<String, Value> = response.json().await.ok()?;
        Some(
            streams
                .into_iter()
                .map(|(name, state)| {
                    let alive = state.get("active").and_then(Value::as_bool).unwrap_or(true);
                    (name, alive)
                })
                .collect(),
        )
    }

    /// PUTs (or, for a stream the runner does not hold, POSTs) one stream: the runner's answer,
    /// or `Err` with what the transport said when the runner did not answer at all.
    async fn deploy_stream(
        &self,
        project: &str,
        name: &str,
        stream_json: &Value,
    ) -> Result<StreamOutcome, String> {
        let Some(runner) = self.runner_for(project) else {
            return Ok(StreamOutcome::Error(format!(
                "`{project}` is not a project name"
            )));
        };
        // The project's write budget first: the stream names it, and the runner refuses a
        // stream naming a rate limit it does not hold (T-5803).
        if let Some(refused) = declare_egress(&self.http, &runner, project).await? {
            return Ok(StreamOutcome::Error(refused));
        }
        let url = format!("{runner}/streams/{}", stream_id(project, name));

        let mut response = self
            .http
            .put(&url)
            .timeout(self.replace_timeout)
            .json(stream_json)
            .send()
            .await
            .map_err(|err| err.to_string())?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            response = self
                .http
                .post(&url)
                .timeout(self.replace_timeout)
                .json(stream_json)
                .send()
                .await
                .map_err(|err| err.to_string())?;
        }

        let status = response.status();
        if status.is_success() {
            Ok(StreamOutcome::Live)
        } else {
            let body = response.text().await.unwrap_or_default();
            let truncated = truncate_body(&body, 500);
            Ok(StreamOutcome::Error(format!(
                "runner answered {status}: {truncated}"
            )))
        }
    }
}

/// Puts one rendered pipeline where it runs (PL-78): a resident one is sent to the runner on
/// this pass, any other joins the run queue as one run on its clock.
fn place(
    (ns, name, mut stream): (&str, String, Value),
    resident: bool,
    spec: &PipelineSpec,
    sends: &mut Vec<(usize, String, String, Value)>,
    outcomes: &mut Vec<(String, String, StreamOutcome)>,
    queued: &mut std::collections::BTreeMap<(String, String), Planned>,
) {
    if resident {
        sends.push((outcomes.len(), ns.to_owned(), name.clone(), stream));
        outcomes.push((ns.to_owned(), name, StreamOutcome::Live));
        return;
    }
    match Clock::of(spec.period.as_deref(), spec.schedule.as_deref()) {
        Ok(clock) => {
            own_credential(&mut stream, ns);
            let stream = run_queue::one_run(&stream);
            let replace = identity::replaces_any(spec);
            queued.insert(
                (ns.to_owned(), name.clone()),
                Planned {
                    stream,
                    clock,
                    replace,
                },
            );
            outcomes.push((
                ns.to_owned(),
                name,
                StreamOutcome::Scheduled("Waiting", String::new()),
            ));
        }
        Err(reason) => outcomes.push((ns.to_owned(), name, StreamOutcome::Error(reason))),
    }
}

impl Runner for StreamDeployer {
    async fn start(&self, key: &run_queue::Key, stream: &Value) -> Result<(), String> {
        tracing::info!(project = %key.0, pipeline = %key.1, "loading the pipeline's run into the runner (PL-78)");
        if self.run_ends.is_some() {
            // A stream the runner never built has no series yet: none of its reports dropped.
            let base = self
                .dropped_reports(key)
                .await
                .map(Option::unwrap_or_default);
            let mut drops = self.report_drops.lock().unwrap_or_else(|p| p.into_inner());
            match base {
                Ok(base) => drops.insert(key.clone(), base),
                Err(_) => drops.remove(key),
            };
        }
        match self.deploy_stream(&key.0, &key.1, stream).await? {
            StreamOutcome::Live => Ok(()),
            StreamOutcome::Error(reason) => Err(reason),
            other => Err(format!("{other:?}")),
        }
    }

    async fn state(&self, key: &run_queue::Key) -> RunState {
        let Some(runner) = self.runner_for(&key.0) else {
            return RunState::Gone;
        };
        let url = format!("{runner}/streams/{}", stream_id(&key.0, &key.1));
        match self.http.get(&url).send().await {
            Ok(answer) if answer.status() == reqwest::StatusCode::NOT_FOUND => RunState::Gone,
            Ok(answer) if answer.status().is_success() => match answer.json::<Value>().await {
                Ok(held) if held.get("active").and_then(Value::as_bool) == Some(false) => {
                    RunState::Finished
                }
                _ => RunState::Running,
            },
            // A runner that does not answer keeps the run until its timeout.
            _ => RunState::Running,
        }
    }

    async fn ended(&self, key: &run_queue::Key, run: &str, finished: bool, stream: &str) {
        if let Some(ends) = &self.run_ends {
            // Bento keeps a deleted stream's series until it restarts, so the count outlives the
            // stream this run ran in.
            let end = self.dropped_reports(key).await;
            let base = self
                .report_drops
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(key);
            let _ = ends.send(crate::pipeline_source_deletes::RunEnd {
                project: key.0.clone(),
                pipeline: key.1.clone(),
                run: run.to_owned(),
                finished,
                reports: report_count(base, end),
                stream: stream.to_owned(),
            });
        }
    }

    async fn delete(&self, key: &run_queue::Key) -> bool {
        let Some(runner) = self.runner_for(&key.0) else {
            return true;
        };
        self.remove(
            &runner,
            &key.0,
            &key.1,
            "its run ended or it is new to this Portal (PL-78)",
        )
        .await
    }
}

/// A run's lost outcome reports from the runner's counter at its start (`base`) and at its end.
fn report_count(
    base: Option<u64>,
    end: Result<Option<u64>, &'static str>,
) -> crate::pipeline_source_deletes::ReportCount {
    use crate::pipeline_source_deletes::ReportCount;
    match (base, end) {
        (_, Err(why)) => ReportCount::Unknown(why),
        (_, Ok(None)) => ReportCount::Unknown("the runner does not export the outcome_drop counter"),
        (None, _) => ReportCount::Unknown(
            "no count at the run's start: the runner's metrics did not answer or this Portal restarted",
        ),
        (Some(base), Ok(Some(end))) if end < base => {
            ReportCount::Unknown("the runner restarted during the run")
        }
        (Some(base), Ok(Some(end))) if end == base => ReportCount::Complete,
        (Some(base), Ok(Some(end))) => ReportCount::Lost(end - base),
    }
}

/// Checks whether a pipeline is eligible for deployment into the runner as a resident stream.
pub fn eligible(spec: &PipelineSpec) -> bool {
    if !spec.enabled {
        return false;
    }
    let mut steps = spec.steps();
    steps.extend(spec.routes.iter().flat_map(|route| route.steps.clone()));
    // A stream runs Bloblang and runner processors, a route's steps too (PL-73); `mapping`, `wasm` and `container` compute
    // run elsewhere (PL-33).
    if !steps.iter().all(|step| match step {
        Step::Processor(_) => true,
        Step::Compute(c) => c.kind.is_mapping() || c.kind == ComputeKind::Kpi,
    }) {
        return false;
    }
    let maps = steps.iter().any(|step| match step {
        Step::Processor(_) => true,
        Step::Compute(c) => {
            c.kpi.is_some() || c.bloblang.as_ref().is_some_and(|b| !b.trim().is_empty())
        }
    });
    let sources = spec.sources();
    !sources.is_empty()
        && sources.iter().all(|source| {
            source.data_source_ref.is_some()
                || (source.endpoint_ref.is_some() && source.query.is_some() && maps)
        })
}

/// Returns whether `spec` is configured as a stream pipeline (DataSource or Endpoint-sourced).
pub fn is_stream_pipeline(spec: &PipelineSpec) -> bool {
    spec.sources().iter().any(|source| {
        source.data_source_ref.is_some()
            || (source.endpoint_ref.is_some() && source.query.is_some())
    })
}

/// Whether the runner keeps `spec` loaded between runs (PL-78): a DataSource that waits for
/// pushes, or an Endpoint read with a change trigger. A scheduled poll is created for its run and
/// deleted after it, so it holds no resident slot (PF-73, T-4952). A DataSource not in the mirror
/// counts nothing here: its pipeline is not rendered until it is, and is counted then.
pub fn stays_resident(mirror: &Mirror, ns: &str, spec: &PipelineSpec) -> bool {
    spec.sources().into_iter().any(|source| {
        if source.data_source_ref.is_none() {
            return source.endpoint_ref.is_some() && source.trigger.is_some();
        }
        matches!(
            resolve_source(mirror, ns, source),
            Ok(ResolvedSource::Data { spec, .. }) if run_queue::is_resident(&spec.source_type)
        )
    })
}

/// One `StreamDeployed` condition with the runner's word on why.
pub fn make_condition(
    condition_type: &str,
    status: &str,
    reason: &str,
    message: &str,
) -> Condition {
    Condition {
        r#type: condition_type.to_string(),
        status: status.to_string(),
        reason: Some(reason.to_string()),
        message: Some(message.to_string()),
        last_transition_time: Some(chrono::Utc::now()),
    }
}

fn truncate_body(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl StreamDeployer {
        /// The same deployer with other timeouts, so a test need not wait out the real ones.
        fn with_timeouts(mut self, request: Duration, replace: Duration) -> Self {
            self.http = reqwest::Client::builder()
                .timeout(request)
                .build()
                .unwrap_or_default();
            self.replace_timeout = replace;
            self
        }
    }

    /// The upsert of the write labelled `label` (T-4056): the `http` inside its branch.
    fn upsert_at<'a>(stream: &'a Value, label: &str) -> &'a Value {
        fn find<'a>(value: &'a Value, label: &str) -> Option<&'a Value> {
            match value {
                Value::Object(map) if map.get("label").and_then(Value::as_str) == Some(label) => {
                    value.pointer("/branch/processors/0/http")
                }
                Value::Object(map) => map.values().find_map(|v| find(v, label)),
                Value::Array(items) => items.iter().find_map(|v| find(v, label)),
                _ => None,
            }
        }
        find(stream, label).unwrap_or(&Value::Null)
    }
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use jc_core::kinds::pipeline::PipelineSource;

    fn helsinki_pipeline_spec() -> PipelineSpec {
        serde_json::from_value(serde_json::json!({
            "class": "auto",
            "period": "60s",
            "source": {
                "dataSourceRef": {
                    "kind": "DataSource",
                    "name": "hsl-citybikes-free"
                }
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this.data.bikes"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
        }))
        .expect("valid PipelineSpec")
    }

    fn helsinki_datasource_spec() -> DataSourceSpec {
        serde_json::from_value(serde_json::json!({
            "type": "http",
            "http": {
                "url": "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_status.json",
                "timeout": "15s"
            }
        }))
        .expect("valid DataSourceSpec")
    }

    /// A resident pipeline (a socket, PL-78), so the runner keeps its stream between passes.
    fn helsinki_test_mirror() -> Mirror {
        let mirror = Mirror::new();
        let ds = serde_json::json!({
            "type": "websocket",
            "webSocket": { "url": "wss://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_status" }
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "DataSource".to_string(),
            metadata: ObjectMeta {
                name: "hsl-citybikes-free".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: ds,
            status: None,
        });

        let ep = serde_json::json!({
            "contextSpaceRef": "helsinki",
            "slug": "abc123456789012345678901234",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Endpoint".to_string(),
            metadata: ObjectMeta {
                name: "helsinki-all".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: ep,
            status: None,
        });

        let pipe = serde_json::json!({
            "class": "resident",
            "source": {
                "dataSourceRef": {
                    "kind": "DataSource",
                    "name": "hsl-citybikes-free"
                }
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this.data.bikes"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Pipeline".to_string(),
            metadata: ObjectMeta {
                name: "citybikes-free".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: pipe,
            status: None,
        });

        mirror
    }

    /// PF-46, AG-52, T-2384: a stream presents its own project's ServiceAccount, on the write
    /// and on the read of a private source, and never another project's.
    ///
    /// One runner-wide client stood here before, so every Banská Bystrica write carried
    /// Helsinki's token and the gateway refused it with `token is not bound to this resource`.
    /// The client id is an `azp` and is written out; the secret never is — the stream names the
    /// environment variable the deployment resolves from that client's Secret.
    #[test]
    fn a_stream_carries_its_own_projects_client_and_no_other() {
        for (project, client, secret) in [
            (
                "helsinki",
                "helsinki-pipelines",
                "${JC_CLIENT_SECRET_HELSINKI}",
            ),
            (
                "banskabystrica",
                "banskabystrica-pipelines",
                "${JC_CLIENT_SECRET_BANSKABYSTRICA}",
            ),
            ("bbsk", "bbsk-pipelines", "${JC_CLIENT_SECRET_BBSK}"),
            (
                "helsinki-mobility",
                "helsinki-mobility-pipelines",
                "${JC_CLIENT_SECRET_HELSINKI_MOBILITY}",
            ),
        ] {
            // The write of a stream that reads a DataSource.
            let rendered = render_stream(
                &helsinki_pipeline_spec(),
                "citybikes-free",
                project,
                &helsinki_datasource_spec(),
                "hsl-citybikes-free",
                "target_slug_456",
                None,
            )
            .expect("rendered stream");
            let oauth2 = &upsert_at(&rendered, "write_0")["oauth2"];
            assert_eq!(oauth2["client_key"], client, "{project} write");
            assert_eq!(oauth2["client_secret"], secret, "{project} write");
            assert_eq!(oauth2["enabled"], true, "{project} write");

            // The read of a private source endpoint, and the write beside it.
            let rendered = render_endpoint_stream(
                &endpoint_pipeline_spec(Some("AirQualityObserved"), Vec::new(), Some("15m"), None),
                project,
                &format!("{project}/kpi"),
                "source_slug_123",
                false,
                "target_slug_456",
            )
            .expect("rendered endpoint stream");
            let read = &rendered["pipeline"]["processors"][0]["try"][0]["http"]["oauth2"];
            assert_eq!(read["client_key"], client, "{project} read");
            assert_eq!(read["client_secret"], secret, "{project} read");
            assert_eq!(
                upsert_at(&rendered, "write_0")["oauth2"]["client_key"],
                client,
                "{project} write beside the read"
            );

            // No other project's client and no secret value anywhere in the rendered stream:
            // the runner's own API serves this document back (T-2384).
            let whole = serde_json::to_string(&rendered).expect("serialisable");
            for other in [
                "helsinki-pipelines",
                "banskabystrica-pipelines",
                "bbsk-pipelines",
            ] {
                assert_eq!(
                    other == client,
                    whole.contains(other),
                    "{project} rendered {other}"
                );
            }
            assert!(!whole.contains("JC_CLIENT_ID"), "{whole}");
        }
    }

    #[test]
    fn renders_the_static_stream_shape() {
        let pipeline = helsinki_pipeline_spec();
        let ds = helsinki_datasource_spec();

        let rendered = render_stream(
            &pipeline,
            "citybikes-free",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "abc123",
            None,
        )
        .expect("rendered stream");

        assert_eq!(rendered["input"]["generate"]["interval"], "60s");
        assert_eq!(
            rendered["pipeline"]["processors"][0]["try"][0]["http"]["url"],
            "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_status.json"
        );

        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors array");
        assert!(processors
            .iter()
            .any(|p| p.get("mapping").and_then(Value::as_str) == Some("root = this.data.bikes")));
        assert!(processors.iter().any(|p| {
            p.get("mapping").and_then(Value::as_str)
                == Some("root = if this.type() == \"array\" { this } else { [this] }")
        }));
        assert!(processors.iter().any(|p| {
            p.get("unarchive")
                .and_then(|u| u.get("format"))
                .and_then(Value::as_str)
                == Some("json_array")
        }));
        // The batches, then the write and its refusals (T-4056).
        let archive = processors.len() - 3;
        assert!(processors[archive].get("archive").is_some());
        // A batch operation at the gateway takes 1000 entities at most.
        assert_eq!(
            processors[archive - 1]["split"]["size"],
            1000,
            "the batch is split before it is archived"
        );
        // And at 1 MiB, so wide or large entities never reach the broker's 4 MiB bound (T-4753).
        assert_eq!(processors[archive - 1]["split"]["byte_size"], 1024 * 1024);
        assert_eq!(processors[archive + 1]["label"], "write_0");
        assert_eq!(processors[archive + 2]["label"], "write_refused");

        assert_eq!(
            upsert_at(&rendered, "write_0")["url"],
            "${JC_GATEWAY_URL}/api/endpoint/abc123/ngsi-ld/v1/entityOperations/upsert?options=update"
        );
        assert_eq!(
            upsert_at(&rendered, "write_0")["oauth2"]["client_secret"],
            "${JC_CLIENT_SECRET_HELSINKI}"
        );

        let json_str = serde_json::to_string(&rendered).unwrap();
        assert!(!json_str.contains("Authorization"));
    }

    #[test]
    fn a_disabled_or_foreign_pipeline_is_not_eligible() {
        let mut spec = helsinki_pipeline_spec();
        assert!(eligible(&spec));

        spec.enabled = false;
        assert!(!eligible(&spec));

        spec.enabled = true;
        spec.source = None;
        assert!(!eligible(&spec));

        let mut spec2 = helsinki_pipeline_spec();
        spec2.compute.as_mut().unwrap().kind = ComputeKind::Wasm;
        assert!(!eligible(&spec2));
    }

    fn endpoint_pipeline_spec(
        type_name: Option<&str>,
        ids: Vec<jc_core::urn::Urn>,
        period: Option<&str>,
        schedule: Option<&str>,
    ) -> PipelineSpec {
        let mut source_json = serde_json::json!({
            "endpointRef": {
                "kind": "Endpoint",
                "name": "helsinki-all"
            },
            "query": {
                "attrs": ["availableBikeNumber"]
            }
        });
        if let Some(t) = type_name {
            source_json["query"]["type"] = serde_json::json!(t);
        }
        if !ids.is_empty() {
            source_json["query"]["ids"] = serde_json::json!(ids);
        }

        let mut pipe_json = serde_json::json!({
            "class": "scheduled",
            "source": source_json,
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki-kpi:kpi-writer"
        });
        if let Some(p) = period {
            pipe_json["period"] = serde_json::json!(p);
        }
        if let Some(s) = schedule {
            pipe_json["schedule"] = serde_json::json!(s);
        }
        serde_json::from_value(pipe_json).expect("valid endpoint PipelineSpec")
    }

    /// T-3132, PL-31: the first page states the total, every further page is read from the same
    /// source with the same credential, and the pages reach the compute as one list.
    #[test]
    fn an_endpoint_source_is_read_page_by_page_into_one_list() {
        let spec =
            endpoint_pipeline_spec(Some("StatisticalObservation"), vec![], Some("168h"), None);
        let rendered =
            render_endpoint_stream(&spec, "bbsk", "bbsk/kpi", "kraj_slug", false, "kpi_slug")
                .expect("rendered");
        let read = rendered["pipeline"]["processors"][0]["try"]
            .as_array()
            .expect("the read");
        let first = &read[0]["http"];
        assert!(
            first["url"]
                .as_str()
                .is_some_and(|url| url.ends_with("&limit=1000&count=true")),
            "{first}"
        );
        assert_eq!(
            first["extract_headers"]["include_patterns"][0],
            "(?i)^ngsild-results-count$"
        );
        let pages = read[1]["mapping"].as_str().expect("the pages mapping");
        assert!(
            pages.contains("50000") && pages.contains("did not say how many"),
            "{pages}"
        );
        assert_eq!(read[2]["unarchive"]["format"], "json_array");
        let further = &read[3]["switch"][0]["processors"][1]["http"];
        assert_eq!(
            further["url"],
            "${JC_GATEWAY_URL}/api/endpoint/kraj_slug/ngsi-ld/v1/entities?type=StatisticalObservation&attrs=availableBikeNumber&limit=1000&offset=${! meta(\"jc_page_offset\") }"
        );
        // A private source: every page with the runner's token, never only the first.
        assert_eq!(further["oauth2"], first["oauth2"]);
        assert!(!first["oauth2"].is_null());
        assert_eq!(read[4]["archive"]["format"], "json_array");
        assert_eq!(read[5]["mapping"], "root = this.flatten()");
    }

    #[test]
    fn endpoint_source_renders_get_url_with_type_and_attrs() {
        let spec =
            endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], Some("15m"), None);
        let rendered = render_endpoint_stream(
            &spec,
            "helsinki",
            "helsinki/kpi",
            "source_slug_123",
            true,
            "target_slug_456",
        )
        .expect("rendered endpoint stream");

        assert_eq!(rendered["input"]["generate"]["interval"], "15m");
        // Each tick is its own run, as on a DataSource's clock; without the stamp every pass of
        // an hour lands in that hour's one run (PL-62, T-3002).
        assert_eq!(
            rendered["input"]["generate"]["mapping"],
            format!("root = \"\"\n{}", crate::pipeline_log::TICK)
        );
        let http = &rendered["pipeline"]["processors"][0]["try"][0]["http"];
        assert_eq!(
            http["url"],
            "${JC_GATEWAY_URL}/api/endpoint/source_slug_123/ngsi-ld/v1/entities?type=BikeHireDockingStation&attrs=availableBikeNumber&limit=1000&count=true"
        );
        assert_eq!(http["verb"], "GET");
        assert_eq!(http["headers"]["Accept"], "application/json");
        // A public source is read the way anyone reads it: no token, because the runner's
        // token names the endpoints its ServiceAccount is bound to and the gateway answers a
        // token for another slug 401 (T-0914).
        assert!(http["oauth2"].is_null(), "{http}");
        // The write always carries the project's own identity: nothing is written anonymously,
        // and nothing is written with another project's client (T-2384).
        assert_eq!(
            upsert_at(&rendered, "write_0")["oauth2"]["client_key"],
            "helsinki-pipelines"
        );
        assert_eq!(
            upsert_at(&rendered, "write_0")["url"],
            "${JC_GATEWAY_URL}/api/endpoint/target_slug_456/ngsi-ld/v1/entityOperations/upsert?options=update"
        );
    }

    #[test]
    fn a_source_that_is_not_public_is_read_with_the_runners_own_token() {
        let spec =
            endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], Some("15m"), None);
        let rendered = render_endpoint_stream(
            &spec,
            "helsinki",
            "helsinki/kpi",
            "source_slug_123",
            false,
            "target_slug_456",
        )
        .expect("rendered endpoint stream");

        let http = &rendered["pipeline"]["processors"][0]["try"][0]["http"];
        assert_eq!(http["oauth2"]["client_key"], "helsinki-pipelines");
        assert_eq!(http["oauth2"]["token_url"], "${JC_TOKEN_URL}");
    }

    #[test]
    fn a_schedule_is_the_clock_and_no_change_gate_is_rendered() {
        let spec = endpoint_pipeline_spec(
            Some("BikeHireDockingStation"),
            vec![],
            None,
            Some("*/15 * * * *"),
        );
        let rendered =
            render_endpoint_stream(&spec, "helsinki", "helsinki/kpi", "src", true, "dst")
                .expect("render");
        assert_eq!(rendered["input"]["generate"]["interval"], "*/15 * * * *");
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        // No change gate: the only branches are the write's own (T-4056).
        assert!(processors
            .iter()
            .filter(|p| !p["label"].as_str().is_some_and(|l| l.starts_with("write")))
            .all(|p| p.get("cache").is_none() && p.get("branch").is_none()));
    }

    #[test]
    fn an_on_change_trigger_renders_the_change_gate_before_the_compute() {
        let mut spec = endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], None, None);
        spec.source.as_mut().expect("source").trigger = Some(
            serde_json::from_value(serde_json::json!({
                "subscription": { "type": "BikeHireDockingStation", "watchedAttributes": ["availableBikeNumber", "status"] }
            }))
            .expect("trigger"),
        );
        let rendered =
            render_endpoint_stream(&spec, "helsinki", "helsinki/bikes-kpi", "src", true, "dst")
                .expect("render");

        // No period: the source is looked at every ten seconds, with the watched attributes.
        assert_eq!(rendered["input"]["generate"]["interval"], "10s");
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert!(processors[0]["try"][0]["http"]["url"]
            .as_str()
            .expect("url")
            .contains("attrs=availableBikeNumber,status&"));
        let hash = processors[2]["mutation"].as_str().expect("the hash");
        assert!(
            hash.contains(r#"e.get("availableBikeNumber.value"), e.get("status.value")"#),
            "{hash}"
        );
        assert_eq!(
            processors[3]["branch"]["processors"][0]["cache"],
            serde_json::json!({ "resource": CHANGE_CACHE, "operator": "get", "key": "helsinki/bikes-kpi" })
        );
        assert_eq!(processors[4], serde_json::json!({ "catch": [] }));
        assert_eq!(
            processors[5]["mutation"],
            "root = if @jc_last == @jc_change { deleted() }"
        );
        assert_eq!(processors[6]["cache"]["operator"], "set");
        assert_eq!(processors[6]["cache"]["key"], "helsinki/bikes-kpi");
        // The compute comes after the gate.
        assert_eq!(processors[7]["mapping"], "root = this");
    }

    #[test]
    fn an_http_source_runs_on_its_schedule_and_a_period_still_wins() {
        let pipe = |extra: serde_json::Value| -> PipelineSpec {
            let mut v = serde_json::json!({
                "class": "scheduled",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "src" } },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
            });
            v.as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            serde_json::from_value(v).expect("valid PipelineSpec")
        };
        let source = helsinki_datasource_spec();
        for (extra, interval) in [
            (
                serde_json::json!({ "schedule": "49 1 * * *" }),
                "49 1 * * *",
            ),
            (serde_json::json!({ "schedule": " " }), "60s"),
            (
                serde_json::json!({ "period": "5m", "schedule": "49 1 * * *" }),
                "5m",
            ),
            (serde_json::json!({}), "60s"),
        ] {
            let spec = pipe(extra.clone());
            let (input, _) =
                datasource_input(&spec, "k", "helsinki", &source, "src").expect("render");
            assert_eq!(input["generate"]["interval"], interval, "{extra}");
            // Each tick is one run of the pipeline's log (PL-62).
            assert!(
                input["generate"]["mapping"]
                    .as_str()
                    .is_some_and(|mapping| mapping.contains("meta jc_run")),
                "{input}"
            );
        }
    }

    #[test]
    fn an_on_change_period_under_five_seconds_is_raised_and_a_longer_one_kept() {
        let trigger: jc_core::kinds::pipeline::Trigger = serde_json::from_value(
            serde_json::json!({ "subscription": { "type": "BikeHireDockingStation" } }),
        )
        .expect("trigger");
        for (period, interval) in [("1s", "5s"), ("500ms", "5s"), ("30s", "30s"), ("2m", "2m")] {
            let mut spec =
                endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], Some(period), None);
            spec.source.as_mut().expect("source").trigger = Some(trigger.clone());
            let rendered =
                render_endpoint_stream(&spec, "helsinki", "k", "src", true, "dst").expect("render");
            assert_eq!(
                rendered["input"]["generate"]["interval"], interval,
                "{period}"
            );
            // No watched attributes named: the query's are watched.
            assert!(rendered["pipeline"]["processors"][2]["mutation"]
                .as_str()
                .expect("hash")
                .contains(r#"e.get("availableBikeNumber.value")"#));
        }
    }

    fn source_query(v: serde_json::Value) -> jc_core::kinds::pipeline::SourceQuery {
        serde_json::from_value(v).expect("source query")
    }

    /// The query parameters of a rendered source URL, decoded the way the gateway reads them.
    fn source_parameters(query: serde_json::Value) -> Vec<(String, String)> {
        let rendered = endpoint_source_url("slug", &source_query(query)).expect("rendered");
        let url = url::Url::parse(&rendered.replace("${JC_GATEWAY_URL}", "http://gw"))
            .expect("the rendered URL parses");
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    // T-2542, PL-31: a filter value is one parameter value, whatever characters it holds.
    #[test]
    fn endpoint_source_query_values_arrive_as_written() {
        let q = "name==\"a&b=c #d+e%f?g\";refPlace==\"Töölö\"";
        let params = source_parameters(serde_json::json!({
            "type": "Bike",
            "q": q,
            "scopeQ": "/hel+sinki/#",
            "geoQ": "georel=near;maxDistance==100&x=1",
        }));
        assert_eq!(
            params,
            vec![
                ("type".to_string(), "Bike".to_string()),
                ("limit".to_string(), "1000".to_string()),
                ("q".to_string(), q.to_string()),
                ("scopeQ".to_string(), "/hel+sinki/#".to_string()),
                (
                    "geoQ".to_string(),
                    "georel=near;maxDistance==100&x=1".to_string()
                ),
            ]
        );
    }

    // T-2542: the runner reads `${...}` in a URL as its own interpolation, so a value that holds
    // one must leave the renderer as plain escaped text.
    #[test]
    fn endpoint_source_values_carry_no_runner_interpolation() {
        let rendered = endpoint_source_url(
            "slug",
            &source_query(serde_json::json!({
                "type": "Bike",
                "attrs": ["a${!env(\"X\")}"],
                "q": "name==\"${JC_TOKEN}\"",
            })),
        )
        .expect("rendered");
        let tail = rendered.trim_start_matches("${JC_GATEWAY_URL}");
        assert!(!tail.contains("${"), "{rendered}");
        assert!(!tail.contains('!'), "{rendered}");
    }

    // T-2542: every attribute name is encoded on its own; the comma between them stays the
    // separator the gateway splits on.
    #[test]
    fn endpoint_source_attrs_are_encoded_one_by_one() {
        let params = source_parameters(serde_json::json!({
            "type": "Bike",
            "attrs": ["speed&limit=1", "https://example.org/ns#weight"],
        }));
        assert_eq!(
            params[1],
            (
                "attrs".to_string(),
                "speed&limit=1,https://example.org/ns#weight".to_string()
            )
        );
        assert_eq!(params.iter().filter(|(k, _)| k == "limit").count(), 1);
    }

    // T-2542: a comma inside one attribute name would be read as two names, so it is refused.
    #[test]
    fn endpoint_source_refuses_a_comma_inside_an_attribute_name() {
        let err = endpoint_source_url(
            "slug",
            &source_query(serde_json::json!({"type": "Bike", "attrs": ["a,b"]})),
        )
        .expect_err("a comma in a name is refused");
        assert!(err.to_string().contains("\"a,b\""), "{err}");
    }

    #[test]
    fn endpoint_source_ids_renders_id_param() {
        let urn: jc_core::urn::Urn = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:station-1"
            .parse()
            .expect("urn");
        let spec = endpoint_pipeline_spec(None, vec![urn], None, Some("*/15 * * * *"));
        let rendered = render_endpoint_stream(
            &spec,
            "helsinki",
            "helsinki/kpi",
            "source_slug_123",
            true,
            "target_slug_456",
        )
        .expect("rendered endpoint stream");

        assert_eq!(rendered["input"]["generate"]["interval"], "*/15 * * * *");
        let http = &rendered["pipeline"]["processors"][0]["try"][0]["http"];
        assert_eq!(
            http["url"],
            "${JC_GATEWAY_URL}/api/endpoint/source_slug_123/ngsi-ld/v1/entities?id=urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:station-1&attrs=availableBikeNumber&limit=1000&count=true"
        );
        assert!(!http["url"].as_str().unwrap().contains("type="));
    }

    #[test]
    fn datasource_without_compute_renders_without_error() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let rendered = render_stream(
            &spec,
            "citybikes-free",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "abc123",
            None,
        )
        .expect("renders without compute");
        let processors = rendered["pipeline"]["processors"].as_array().unwrap();
        assert!(!processors
            .iter()
            .any(|p| p.get("mapping").and_then(Value::as_str) == Some("root = this.data.bikes")));
    }

    const BENTO: &str = r#"
pipeline:
  processors:
    - mapping: root = this.data.stations
    - unarchive:
        format: json_array
output:
  http_client:
    url: https://somewhere.example/authored
"#;

    /// PL-19 (T-1508): with the switch on, every credential of the project's `pipelines` account
    /// in a stream, its write and its source read, becomes its own pipeline's at the runner's
    /// sidecar, and no secret variable is named; a `${JC_CLIENT_ID}` still in the stream is not
    /// the project's client and stays (the sinks lose theirs before, T-4038).
    #[test]
    fn a_stream_presents_its_own_pipeline_and_names_no_secret() {
        let mut stream = serde_json::json!({
            "input": { "http_client": { "url": "src", "oauth2": pipeline_oauth2("zilina") } },
            "pipeline": { "processors": [{ "http": { "url": "x", "oauth2": {
                "enabled": true, "client_key": "${JC_CLIENT_ID}",
                "client_secret": "${JC_CLIENT_SECRET}", "token_url": "${JC_TOKEN_URL}" } } }] },
            "output": gateway_output("zilina", "slug")
        });
        let before = stream.to_string();
        assert!(before.contains("JC_CLIENT_SECRET_ZILINA"), "{before}");

        with_pipeline_identity(&mut stream, "zilina", "drepo");
        let after = stream.to_string();
        assert!(
            !after.contains("JC_CLIENT_SECRET_ZILINA") && !after.contains("zilina-pipelines"),
            "{after}"
        );
        for oauth2 in [
            &stream["input"]["http_client"]["oauth2"],
            &stream["output"]["http_client"]["oauth2"],
        ] {
            assert_eq!(oauth2["client_key"], "zilina/drepo");
            assert_eq!(oauth2["token_url"], "${JC_PIPELINE_TOKEN_URL}");
            assert!(oauth2.get("client_secret").is_none(), "{oauth2}");
        }
        assert_eq!(
            stream["pipeline"]["processors"][0]["http"]["oauth2"]["client_key"], "${JC_CLIENT_ID}",
            "a credential that is not the project's client is not a pipeline's"
        );

        // Another project's credential in the stream is not this pipeline's to take.
        let mut other = serde_json::json!({ "output": gateway_output("bbsk", "slug") });
        with_pipeline_identity(&mut other, "zilina", "drepo");
        assert_eq!(
            other["output"]["http_client"]["oauth2"]["client_key"],
            "bbsk-pipelines"
        );
    }

    /// T-4038: every sink a stream reports to the Portal's internal listener through (the
    /// outcome sink, the pass report, the samples taps) presents a client the listener admits for
    /// that pipeline (T-3193): its project's `pipelines` client, or with pipeline identity its own.
    /// The runner-wide `${JC_CLIENT_ID}` is one project's, and zilina's reports answered 401.
    #[test]
    fn a_streams_reports_present_a_client_of_its_own_pipeline() {
        fn sinks(value: &Value, out: &mut Vec<Value>) {
            match value {
                Value::Object(map) => {
                    let internal = map
                        .get("url")
                        .and_then(Value::as_str)
                        .is_some_and(|url| url.contains("/internal/pipelines/zilina/drepo/"));
                    if let (true, Some(oauth2)) = (internal, map.get("oauth2")) {
                        out.push(oauth2.clone());
                    }
                    map.values().for_each(|v| sinks(v, out));
                }
                Value::Array(items) => items.iter().for_each(|v| sinks(v, out)),
                _ => {}
            }
        }
        let deployer = StreamDeployer::new("http://runner").with_validation(
            "http://portal-internal:8081/",
            Arc::new(crate::pipeline_validation::ModelSchemas::default()),
        );
        let mut stream = serde_json::json!({
            "input": { "label": "input", "generate": { "interval": "1s", "mapping": "root = {}" } },
            "pipeline": { "processors": [{ "label": "step", "mapping": "root = this" }] },
            "output": gateway_output("zilina", "slug")
        });
        deployer.reporting(&mut stream, "zilina", "drepo", &["air".to_owned()]);
        let mut found = Vec::new();
        sinks(&stream, &mut found);
        // The outcome sink, the pass report and at least one tap.
        assert!(found.len() >= 3, "{stream}");
        let admitted = crate::auth::internal::pipeline_clients("zilina", "drepo");
        for oauth2 in &found {
            assert_eq!(oauth2, &pipeline_oauth2("zilina"), "{stream}");
            assert!(admitted.iter().any(|c| oauth2["client_key"] == c.as_str()));
        }
        let text = stream.to_string();
        assert!(
            !text.contains("${JC_CLIENT_ID}") && !text.contains("${JC_CLIENT_SECRET}"),
            "{text}"
        );

        // With pipeline identity the reports are the pipeline's own, as its write is.
        with_pipeline_identity(&mut stream, "zilina", "drepo");
        let mut found = Vec::new();
        sinks(&stream, &mut found);
        for oauth2 in &found {
            assert_eq!(oauth2["client_key"], sidecar_client("zilina", "drepo"));
            assert_eq!(oauth2["token_url"], "${JC_PIPELINE_TOKEN_URL}");
        }
    }

    /// T-4069: a polled stream holds at most a few pages on a slow gateway. T-5636: a resident
    /// source read in batches (HSL's HFP over MQTT) writes one batch at a time, so two batches of
    /// the same vehicles never wait on each other's row locks; an unbatched resident one keeps
    /// Bento's window of single-record writes.
    #[test]
    fn a_stream_bounds_its_writes_in_flight_by_how_its_source_arrives() {
        let pipeline = helsinki_pipeline_spec();
        let polled = render_stream(
            &pipeline,
            "citybikes-free",
            "helsinki",
            &helsinki_datasource_spec(),
            "hsl-citybikes-free",
            "abc123",
            None,
        )
        .expect("polled");
        assert_eq!(polled["pipeline"]["threads"], POLLED_IN_FLIGHT);

        let mqtt: DataSourceSpec = serde_json::from_value(serde_json::json!({
            "type": "mqtt",
            "mqtt": { "urls": ["tcp://mqtt.hsl.fi:1883"], "topics": ["/hfp/v2/journey/ongoing/vp/#"] }
        }))
        .expect("mqtt");
        let hfp: PipelineSpec = serde_json::from_value(serde_json::json!({
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "hfp" } },
            "compute": { "kind": "bloblang", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
        }))
        .expect("resident pipeline");
        let resident =
            render_stream(&hfp, "hfp", "helsinki", &mqtt, "hfp", "abc123", None).expect("resident");
        assert_eq!(resident["pipeline"]["threads"], 1);
        assert_eq!(resident["pipeline"]["threads"], BATCHES_IN_FLIGHT);

        let nats: DataSourceSpec = serde_json::from_value(serde_json::json!({
            "type": "nats",
            "input": { "urls": ["nats://nats.helsinki.fi:4222"], "subject": "city.bikes.updates" }
        }))
        .expect("nats");
        let unbatched =
            render_stream(&hfp, "hfp", "helsinki", &nats, "nats", "abc123", None).expect("nats");
        assert_eq!(unbatched["pipeline"]["threads"], WRITES_IN_FLIGHT);
        // Several sources: an unbatched resident one wins, then a batched one, then the clock.
        assert_eq!(in_flight([None, Some(&mqtt)]), BATCHES_IN_FLIGHT);
        assert_eq!(in_flight([Some(&mqtt), Some(&nats)]), WRITES_IN_FLIGHT);
        assert_eq!(in_flight([None]), POLLED_IN_FLIGHT);

        let spec = endpoint_pipeline_spec(Some("BikeHireDockingStation"), vec![], None, None);
        let endpoint =
            render_endpoint_stream(&spec, "helsinki", "helsinki/kpi", "src", true, "dst")
                .expect("endpoint");
        assert_eq!(endpoint["pipeline"]["threads"], POLLED_IN_FLIGHT);
    }

    /// T-5046: one HSL frame was one upsert, 64 of them waiting on the gateway per stream. A
    /// resident source is read in batches that become one write each; the source keeps the
    /// `input` label its counters are read by, and a polled one is not wrapped.
    #[test]
    fn a_resident_source_is_read_in_batches_and_keeps_its_input_label() {
        let resident = |source: serde_json::Value| {
            let spec: DataSourceSpec = serde_json::from_value(source).expect("source");
            let hfp: PipelineSpec = serde_json::from_value(serde_json::json!({
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "hfp" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
            }))
            .expect("resident pipeline");
            render_stream(&hfp, "hfp", "helsinki", &spec, "hfp", "abc123", None).expect("resident")
        };
        for (kind, stream) in [
            (
                "mqtt",
                resident(serde_json::json!({ "type": "mqtt", "mqtt": {
                    "urls": ["wss://mqtt.hsl.fi:443/"], "topics": ["/hfp/v2/journey/ongoing/vp/#"] } })),
            ),
            (
                "websocket",
                resident(
                    serde_json::json!({ "type": "websocket", "webSocket": { "url": "wss://feed.example/live" } }),
                ),
            ),
        ] {
            let input = &stream["input"];
            assert_eq!(input["label"], "input_batches", "{input}");
            assert_eq!(
                input["broker"]["batching"],
                serde_json::json!({ "count": 1000, "period": "1s" })
            );
            let inner = &input["broker"]["inputs"];
            assert_eq!(inner.as_array().map(Vec::len), Some(1), "{inner}");
            assert_eq!(inner[0]["label"], "input");
            assert!(inner[0].get(kind).is_some(), "{inner}");
            assert!(!input_ends_by_itself(input));
            // The batch becomes one array, cut at the gateway's limits, before the write.
            let steps = stream["pipeline"]["processors"]
                .as_array()
                .expect("processors");
            assert!(steps
                .iter()
                .any(|p| p.get("archive") == Some(&serde_json::json!({ "format": "json_array" }))));
        }
        let polled = render_stream(
            &helsinki_pipeline_spec(),
            "citybikes-free",
            "helsinki",
            &helsinki_datasource_spec(),
            "hsl-citybikes-free",
            "abc123",
            None,
        )
        .expect("polled");
        assert!(
            polled["input"].get("broker").is_none(),
            "{}",
            polled["input"]
        );
    }

    #[test]
    fn a_write_no_retry_can_fix_is_dropped_and_one_that_can_recover_is_retried() {
        let output = gateway_output("helsinki", "abc");
        let dropped = output["http_client"]["drop_on"]
            .as_array()
            .expect("drop_on");
        assert_eq!(
            dropped,
            &vec![
                serde_json::json!(400),
                serde_json::json!(413),
                serde_json::json!(422)
            ]
        );
        for recoverable in [401, 403, 429, 500, 502, 503] {
            assert!(
                !dropped.contains(&serde_json::json!(recoverable)),
                "{recoverable} is retried"
            );
        }
    }

    #[test]
    fn datasource_with_a_bento_mapping_renders_its_processors_and_the_endpoint_output() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let rendered = render_stream(
            &spec,
            "citybikes-gbfs",
            "helsinki",
            &ds,
            "hsl-citybikes-gbfs",
            "abc123",
            Some(BENTO),
        )
        .expect("renders from bento.yaml");
        let processors = rendered["pipeline"]["processors"].as_array().unwrap();
        // The HTTP poll and its error guard first, the body bound (PL-113), the author's two,
        // then the shared tail.
        assert!(processors[3]["catch"].is_array());
        assert_eq!(processors[4]["mapping"], "root = this.data.stations");
        assert_eq!(processors[5]["unarchive"]["format"], "json_array");
        assert_eq!(processors.len(), 12, "the shared tail and the write");
        let url = upsert_at(&rendered, "write_0")["url"].as_str().unwrap();
        assert!(url.contains("/api/endpoint/abc123/"), "{url}");
        assert!(!url.contains("authored"));
    }

    #[test]
    fn a_bento_with_its_own_input_is_refused_and_broken_yaml_names_itself() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let render =
            |bento: &str| render_stream(&spec, "p", "helsinki", &ds, "src", "abc123", Some(bento));
        assert!(matches!(
            render("input:\n  generate: {}\n").unwrap_err(),
            RenderError::BentoInput
        ));
        assert!(matches!(
            render("pipeline: [\n").unwrap_err(),
            RenderError::Bento(_)
        ));
        // An empty file is an author who has not written the mapping yet: the stream still renders.
        assert!(render("").is_ok());
    }

    /// T-3167 (PL-16, PL-07): a `bento.yaml` step reads its own pipeline's declared variable and
    /// renders; one naming a variable it did not declare, another project's credential or the
    /// runner's own pod is refused before a stream reaches the runner.
    #[test]
    fn a_bento_step_reads_its_own_declared_variable_and_nothing_else_of_the_runner() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        spec.secret_refs = vec![jc_core::envelope::SecretRef {
            name: "city-api".to_owned(),
            key: Some("key".to_owned()),
            env_var: Some("CITY_API_KEY".to_owned()),
        }];
        let ds = helsinki_datasource_spec();
        let render = |spec: &PipelineSpec, bento: &str| {
            render_stream(spec, "p", "helsinki", &ds, "src", "abc123", Some(bento))
        };
        let reading = |var: &str| {
            format!(
                "pipeline:\n  processors:\n    - http:\n        url: https://api.example/x\n        headers: {{ X-Key: '${{{var}}}' }}\n"
            )
        };
        render(&spec, &reading("CITY_API_KEY")).expect("its own declared variable");

        let mut undeclared = spec.clone();
        undeclared.secret_refs.clear();
        for (spec, bento) in [
            (&undeclared, reading("CITY_API_KEY")),
            (&spec, reading("JC_CLIENT_SECRET_BBSK")),
            (&spec, reading("DS_OTHER_PASSWORD")),
            (
                &spec,
                "pipeline:\n  processors:\n    - http: { url: 'http://127.0.0.1:4195/streams/x', verb: PUT }\n"
                    .to_owned(),
            ),
        ] {
            let refused = render(spec, &bento).expect_err(&bento);
            assert!(matches!(refused, RenderError::Bento(_)), "{bento}: {refused:?}");
            let said = refused.to_string();
            assert!(said.contains("PL-16") || said.contains("PL-07"), "{said}");
        }
    }

    /// PL-16, PL-50, PL-52 (T-2557): the author's `bento.yaml` processors pass the same check as a
    /// manifest's steps: a processor the platform does not run, at the top or nested inside
    /// another, or a Bloblang read of the runner's environment, and no stream is rendered.
    #[test]
    fn a_bento_processor_the_platform_refuses_does_not_render() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let render =
            |bento: &str| render_stream(&spec, "p", "helsinki", &ds, "src", "abc123", Some(bento));
        for bento in [
            "pipeline:\n  processors:\n    - command: { name: sh }\n",
            "pipeline:\n  processors:\n    - subprocess: { name: sh }\n",
            "pipeline:\n  processors:\n    - try:\n        - file: { path: /etc/passwd }\n",
            "pipeline:\n  processors:\n    - switch:\n        - processors:\n            - wasm: { module_path: /tmp/x.wasm }\n",
            "pipeline:\n  processors:\n    - no_such_processor: {}\n",
            "pipeline:\n  processors:\n    - mapping: 'root.s = env(\"JC_CLIENT_SECRET\")'\n",
            "pipeline:\n  processors:\n    - branch:\n        processors:\n          - mapping: 'root = env(\"JC_CLIENT_SECRET\")'\n",
        ] {
            let refused = render(bento).expect_err(bento);
            assert!(matches!(refused, RenderError::Bento(_)), "{bento}: {refused:?}");
        }
        // The same names as configuration of an allowed processor are not processors.
        assert!(render("pipeline:\n  processors:\n    - redis: { url: redis://cache:6379, command: get, args_mapping: 'root = [this.id]' }\n").is_ok());
        assert!(render(BENTO).is_ok());
    }

    /// PL-90, PL-55 (T-4882): a typed `endpointRef` reads the Endpoint of the project it names, and
    /// only when that Endpoint admits the pipeline's project; a bare name stays in its own.
    #[test]
    fn a_cross_project_source_resolves_where_its_endpoint_admits_the_pipeline() {
        let mirror = Mirror::new();
        let endpoint = |name: &str, spec: Value| ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Endpoint".to_string(),
            metadata: ObjectMeta {
                name: name.to_string(),
                namespace: Some("espoo".to_string()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        mirror.upsert(endpoint(
            "roads",
            serde_json::json!({ "contextSpaceRef": "roads", "slug": "roadsslug", "audience": "project-list", "allowedProjects": ["helsinki"] }),
        ));
        mirror.upsert(endpoint(
            "closed",
            serde_json::json!({ "contextSpaceRef": "roads", "slug": "closedslug", "audience": "project-list", "allowedProjects": ["vantaa"] }),
        ));
        let typed = |name: &str| -> jc_core::Ref {
            serde_json::from_value(
                serde_json::json!({ "kind": "Endpoint", "name": name, "namespace": "espoo" }),
            )
            .expect("a typed ref")
        };
        let read = source_endpoint(&mirror, "helsinki", &typed("roads")).expect("admitted");
        assert_eq!(read.spec["slug"], "roadsslug");
        let refused =
            source_endpoint(&mirror, "helsinki", &typed("closed")).expect_err("not listed");
        assert!(
            refused.contains("espoo/closed does not admit project helsinki"),
            "{refused}"
        );
        mirror.upsert(endpoint(
            "bare",
            serde_json::json!({ "contextSpaceRef": "roads", "slug": "bareslug" }),
        ));
        assert!(source_endpoint(&mirror, "helsinki", &typed("bare")).is_err());
        let gone = source_endpoint(&mirror, "helsinki", &typed("gone")).expect_err("absent");
        assert!(
            gone.contains("source endpoint espoo/gone is not in the mirror"),
            "{gone}"
        );
        // A bare name is the pipeline's own project's, where `roads` does not exist.
        let bare = jc_core::Ref::Name("roads".to_owned());
        let own = source_endpoint(&mirror, "helsinki", &bare).expect_err("own project");
        assert!(
            own.contains("source endpoint roads is not in the mirror"),
            "{own}"
        );
        // The merged path resolves through the same rule.
        let source: PipelineSource = serde_json::from_value(serde_json::json!({
            "endpointRef": { "kind": "Endpoint", "name": "roads", "namespace": "espoo" },
            "query": { "type": "Road" }
        }))
        .expect("a source");
        match resolve_source(&mirror, "helsinki", source) {
            Ok(ResolvedSource::Endpoint { slug, .. }) => assert_eq!(slug, "roadsslug"),
            _ => panic!("the cross-project source did not resolve"),
        }
    }

    #[tokio::test]
    async fn missing_source_endpoint_is_error() {
        let mirror = Mirror::new();
        let target_ep = serde_json::json!({
            "contextSpaceRef": "helsinki-kpi",
            "slug": "targetslug1234567890123456",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Endpoint".to_string(),
            metadata: ObjectMeta {
                name: "kpi-writer".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: target_ep,
            status: None,
        });

        let pipe = serde_json::json!({
            "class": "scheduled",
            "source": {
                "endpointRef": {
                    "kind": "Endpoint",
                    "name": "nonexistent-endpoint"
                },
                "query": {
                    "type": "BikeHireDockingStation",
                    "attrs": ["availableBikeNumber"]
                }
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = this"
            },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki-kpi:kpi-writer"
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Pipeline".to_string(),
            metadata: ObjectMeta {
                name: "kpi-pipe".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: pipe,
            status: None,
        });

        let deployer = StreamDeployer::new("http://dummy-runner:4195");
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0].2 {
            StreamOutcome::Error(msg) => {
                assert!(msg.contains("source endpoint nonexistent-endpoint is not in the mirror"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// T-3163: the reaper's `${JC_CLIENT_ID}`/`${JC_CLIENT_SECRET}` become the stream's own
    /// project's client, never the runner's plain variable, which is one project's; a name that
    /// only starts the same is left alone, and the platform's own write is unchanged.
    #[test]
    fn an_authors_credential_alias_becomes_the_streams_own_project() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let bento = "pipeline:\n  processors:\n    - http:\n        url: ${JC_GATEWAY_URL}/api/endpoint/x/ngsi-ld/v1/entityOperations/delete\n        oauth2: { enabled: true, client_key: '${JC_CLIENT_ID}', client_secret: '${JC_CLIENT_SECRET}', token_url: '${JC_TOKEN_URL}' }\n        headers: { X-Run: 'reaper' }\n";
        let mut rendered =
            render_stream(&spec, "p", "bbsk", &ds, "src", "abc123", Some(bento)).expect("renders");
        let before_output = rendered["output"].clone();
        own_credential(&mut rendered, "bbsk");
        let whole = rendered.to_string();
        assert!(!whole.contains("${JC_CLIENT_SECRET}"), "{whole}");
        assert!(!whole.contains("${JC_CLIENT_ID}"), "{whole}");
        let step = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .find(|p| {
                p.get("http")
                    .is_some_and(|h| h.get("headers").is_some_and(|h| h.get("X-Run").is_some()))
            })
            .expect("the author's step")
            .clone();
        assert_eq!(step["http"]["oauth2"], pipeline_oauth2("bbsk"));
        // jc-core refuses another project's variable in a step (T-3163), so the rewrite leaving a
        // name that only starts the same alone is shown on a value of its own.
        let mut other = serde_json::json!({ "x": "${JC_CLIENT_SECRET_HELSINKI}" });
        own_credential(&mut other, "bbsk");
        assert_eq!(other["x"], "${JC_CLIENT_SECRET_HELSINKI}");
        assert_eq!(rendered["output"], before_output);
    }

    /// T-5337: every cache a stream names, an author's nested ones and the platform's own change
    /// cache alike, becomes its project's `<project>__<name>`, so one project's step never reads
    /// or overwrites what another's cached in the runner they share; other fields are left alone.
    #[test]
    fn own_credential_keeps_every_cache_to_the_project() {
        let mut stream = serde_json::json!({
            "pipeline": { "processors": [
                { "cache": { "resource": "gtfs", "operator": "get", "key": "k" } },
                { "branch": { "processors": [{ "cached": { "cache": "gtfs", "key": "k" } }] } },
                { "dedupe": { "cache": "seen", "key": "k" } },
                { "cache": { "resource": CHANGE_CACHE, "operator": "set", "key": "a/b" } },
                { "rate_limit": { "resource": "hfp_vehicles" } },
                { "mapping": "root.cache = \"gtfs\"" }
            ] }
        });
        own_credential(&mut stream, "banska-bystrica");
        let p = &stream["pipeline"]["processors"];
        assert_eq!(p[0]["cache"]["resource"], "banska_bystrica__gtfs");
        assert_eq!(p[0]["cache"]["key"], "k");
        assert_eq!(
            p[1]["branch"]["processors"][0]["cached"]["cache"],
            "banska_bystrica__gtfs"
        );
        assert_eq!(p[2]["dedupe"]["cache"], "banska_bystrica__seen");
        assert_eq!(
            p[3]["cache"]["resource"],
            "banska_bystrica__pipeline_changes"
        );
        assert_eq!(p[4]["rate_limit"]["resource"], "hfp_vehicles");
        assert_eq!(p[5]["mapping"], "root.cache = \"gtfs\"");
        assert_eq!(cache_label("helsinki", "gtfs"), "helsinki__gtfs");
        // The reconciler owns a stream twice (reporting, then before the send): still one prefix.
        own_credential(&mut stream, "banska-bystrica");
        let p = &stream["pipeline"]["processors"];
        assert_eq!(p[0]["cache"]["resource"], "banska_bystrica__gtfs");
        assert_eq!(p[2]["dedupe"]["cache"], "banska_bystrica__seen");
        assert_eq!(
            p[3]["cache"]["resource"],
            "banska_bystrica__pipeline_changes"
        );
    }

    /// A runner that takes every project's write budget, declared before its streams (T-5803);
    /// a test mounts the streams it reads on top.
    async fn runner() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex("^/resources/rate_limit/"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        server
    }

    /// F-BNT3-002 (T-5803): two projects' streams wait on two write budgets. The platform's
    /// upsert, an author's `rate_limit: pipeline_egress` and a step naming another project's
    /// bucket all become the stream's own project's; a source host's pace stays shared.
    #[test]
    fn each_project_writes_through_its_own_egress_budget() {
        let render = |project: &str| {
            let mut stream = serde_json::json!({
                "pipeline": { "processors": [
                    { "http": { "url": "https://example.org", "rate_limit": "pipeline_egress" } },
                    { "rate_limit": { "resource": "pipeline_egress" } },
                    { "http": { "url": "https://example.org", "rate_limit": "helsinki__pipeline_egress" } },
                    { "rate_limit": { "resource": "zilina_opendata" } },
                    { "http": upsert(project, "air-quality-endpoint-slug-abcdef", false) },
                    { "http": { "url": "https://example.org", "rate_limit": "${JC_SPACE_9:helsinki__pipeline_egress}" } },
                    { "rate_limit": { "resource": "${JC_SOURCE_SPACE:zilina_opendata}" } },
                    { "http": { "url": "https://example.org", "rate_limit": "helsinki__pipeline_egress_x" } }
                ] }
            });
            own_credential(&mut stream, project);
            own_credential(&mut stream, project);
            stream
        };
        let praha = render("praha");
        let helsinki = render("helsinki");
        let budgets = |stream: &Value| -> Vec<Value> {
            let p = &stream["pipeline"]["processors"];
            vec![
                p[0]["http"]["rate_limit"].clone(),
                p[1]["rate_limit"]["resource"].clone(),
                p[2]["http"]["rate_limit"].clone(),
                p[4]["http"]["rate_limit"].clone(),
                p[5]["http"]["rate_limit"].clone(),
                p[6]["rate_limit"]["resource"].clone(),
                p[7]["http"]["rate_limit"].clone(),
            ]
        };
        assert_eq!(
            budgets(&praha),
            vec![Value::from("praha__pipeline_egress"); 7]
        );
        assert_eq!(
            budgets(&helsinki),
            vec![Value::from("helsinki__pipeline_egress"); 7]
        );
        assert_eq!(
            praha["pipeline"]["processors"][3]["rate_limit"]["resource"],
            "zilina_opendata"
        );
        assert_eq!(
            egress_label("banska-bystrica"),
            "banska_bystrica__pipeline_egress"
        );
    }

    /// F-BNT3-002 (T-5803): before a project's stream reaches the runner, the runner holds that
    /// project's own budget, so a project the resources file never named still runs; a runner
    /// that refuses the budget gets no stream naming it.
    /// A project name that is no DNS-1123 label never reaches the runner's URL: the refusal
    /// comes back before any request (the runner here would answer none).
    #[tokio::test]
    async fn a_project_name_off_dns1123_gets_no_budget_request() {
        let http = reqwest::Client::new();
        for project in ["../streams", "a/b", "Praha", ""] {
            let refused = declare_egress(&http, "http://localhost:9", project).await;
            assert!(matches!(refused, Ok(Some(_))), "{project}: {refused:?}");
        }
    }

    #[tokio::test]
    async fn the_runner_gets_each_projects_budget_before_its_stream() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex("^/resources/rate_limit/"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let deployer = StreamDeployer::new(server.uri());
        let stream = serde_json::json!({ "pipeline": { "processors": [] } });
        for project in ["helsinki", "praha"] {
            let sent = deployer.deploy_stream(project, "feed", &stream).await;
            assert_eq!(sent, Ok(StreamOutcome::Live));
        }
        let paths: Vec<(String, String)> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .map(|r| (r.method.to_string(), r.url.path().to_owned()))
            .collect();
        let pair = |m: &str, p: &str| (m.to_owned(), p.to_owned());
        assert_eq!(
            paths,
            vec![
                pair("POST", "/resources/rate_limit/helsinki__pipeline_egress"),
                pair("PUT", "/streams/helsinki.feed"),
                pair("POST", "/resources/rate_limit/praha__pipeline_egress"),
                pair("PUT", "/streams/praha.feed"),
            ]
        );
        let body: Value =
            serde_json::from_slice(&server.received_requests().await.expect("recorded")[0].body)
                .expect("json");
        assert_eq!(
            body,
            serde_json::json!({ "local": { "count": 500, "interval": "1s" } })
        );

        let refusing = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(502))
            .mount(&refusing)
            .await;
        let deployer = StreamDeployer::new(refusing.uri());
        match deployer.deploy_stream("praha", "feed", &stream).await {
            Ok(StreamOutcome::Error(said)) => {
                assert!(said.contains("praha__pipeline_egress"), "{said}")
            }
            other => panic!("a refused budget was not said: {other:?}"),
        }
        let sent = refusing.received_requests().await.expect("recorded");
        assert_eq!(sent.len(), 1, "a stream followed a refused budget");
    }

    /// PL-06: a `bento.yaml` cannot turn a pipeline into a request/response service: an input
    /// that serves HTTP is refused, and an output that answers the caller is replaced by the
    /// endpoint write, whatever the author wrote.
    #[test]
    fn a_bento_cannot_serve_or_answer_http_requests() {
        let mut spec = helsinki_pipeline_spec();
        spec.compute = None;
        let ds = helsinki_datasource_spec();
        let render =
            |bento: &str| render_stream(&spec, "p", "helsinki", &ds, "src", "abc123", Some(bento));
        assert!(matches!(
            render("input:\n  http_server:\n    path: /api\n").unwrap_err(),
            RenderError::BentoInput
        ));
        for output in [
            "output:\n  sync_response: {}\n",
            "output:\n  http_server:\n    path: /answer\n",
        ] {
            let rendered = render(&format!(
                "pipeline:\n  processors:\n    - mapping: root = this\n{output}"
            ))
            .expect("renders");
            assert_eq!(
                rendered["output"],
                labelled(written(), "output"),
                "{output}"
            );
            let url = upsert_at(&rendered, "write_0")["url"]
                .as_str()
                .unwrap_or_default();
            assert!(url.contains("/api/endpoint/abc123/"), "{url}");
            assert!(!rendered.to_string().contains("sync_response"), "{output}");
            assert!(!rendered.to_string().contains("/answer"), "{output}");
        }
    }

    /// F-POR3-006: a run's lost reports are the runner's drop count at its end less the count at
    /// its start; anything that leaves either end uncounted is no count, never zero.
    #[test]
    fn the_lost_reports_are_counted_from_the_runs_start() {
        use crate::pipeline_source_deletes::ReportCount;
        assert_eq!(report_count(Some(3), Ok(Some(3))), ReportCount::Complete);
        assert_eq!(report_count(Some(3), Ok(Some(5))), ReportCount::Lost(2));
        for (base, end) in [
            (None, Ok(Some(0))),
            (Some(0), Ok(None)),
            (Some(0), Err("down")),
            (Some(4), Ok(Some(1))),
        ] {
            assert!(
                matches!(report_count(base, end), ReportCount::Unknown(_)),
                "{base:?} {end:?}"
            );
        }
    }

    /// F-POR3-006: the deployer reads the stream's drop counter on the runner as the run starts
    /// and again once it ended, and tells the run's end the difference.
    #[tokio::test]
    async fn a_run_end_carries_the_reports_the_runner_dropped_during_the_run() {
        let server = runner().await;
        let key = ("helsinki".to_owned(), "registry".to_owned());
        let line = |n: u64| {
            format!(
                "output_sent{{label=\"outcome_drop\",stream=\"{}\"}} {n}\n",
                stream_id(&key.0, &key.1)
            )
        };
        for (body, times) in [(line(4), Some(1)), (line(6), None)] {
            let mock = wiremock::Mock::given(wiremock::matchers::method("GET"))
                .and(wiremock::matchers::path("/metrics"))
                .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body));
            match times {
                Some(n) => mock.up_to_n_times(n).mount(&server).await,
                None => mock.mount(&server).await,
            }
        }
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let (ends, mut heard) = tokio::sync::mpsc::unbounded_channel();
        let deployer = StreamDeployer::new(server.uri()).with_run_ends(ends);
        let stream =
            serde_json::json!({ "input": { "generate": { "count": 1, "mapping": "root = {}" } } });
        let _ = Runner::start(&deployer, &key, &stream).await;
        Runner::ended(&deployer, &key, "r1", true, "h1").await;
        let end = heard.try_recv().expect("the run's end");
        assert_eq!(
            end.stream, "h1",
            "the stream hash reaches the run log (T-5825)"
        );
        assert_eq!(
            end.reports,
            crate::pipeline_source_deletes::ReportCount::Lost(2)
        );
        // A second end with no start of its own has no base to count from.
        Runner::ended(&deployer, &key, "r2", true, "h2").await;
        assert!(matches!(
            heard.try_recv().expect("the run's end").reports,
            crate::pipeline_source_deletes::ReportCount::Unknown(_)
        ));
    }

    /// T-4044: with pipeline identity nothing the reconciler sends the runner names a client
    /// secret, not the pipeline's stream, its reports, an author's own credential step or its
    /// sweep: the runner's `GET /streams/{id}` serves a stream back with its variables resolved.
    #[tokio::test]
    async fn with_pipeline_identity_no_stream_sent_to_the_runner_names_a_client_secret() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let deployer = StreamDeployer::new(server.uri())
            .with_pipeline_identity(true)
            .with_validation(
                "http://portal-internal:8081/",
                Arc::new(crate::pipeline_validation::ModelSchemas::default()),
            );
        let mirror = helsinki_test_mirror();
        let mut pipeline = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("pipeline");
        pipeline.spec["expiry"] = serde_json::json!({ "after": "14d", "types": ["Vehicle"] });
        mirror.upsert(pipeline);
        let mut bentos = Bentos::new();
        bentos.insert(
            ("helsinki".to_owned(), "citybikes-free".to_owned()),
            "pipeline:\n  processors:\n    - http:\n        url: ${JC_GATEWAY_URL}/api/endpoint/x/ngsi-ld/v1/entityOperations/delete\n        oauth2: { enabled: true, client_key: '${JC_CLIENT_ID}', client_secret: '${JC_CLIENT_SECRET}', token_url: '${JC_TOKEN_URL}' }\n".to_owned(),
        );

        let outcomes = deployer
            .converge(&mirror, &bentos, &Default::default())
            .await;
        assert!(
            outcomes.iter().all(|(_, _, o)| *o == StreamOutcome::Live),
            "{outcomes:?}"
        );
        let sent: Vec<Value> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .filter(|r| r.method.as_str() == "PUT")
            .map(|r| serde_json::from_slice(&r.body).expect("json"))
            .collect();
        assert_eq!(sent.len(), 2, "the stream and its sweep");
        for body in &sent {
            let text = body.to_string();
            assert!(
                !text.contains("client_secret") && !text.contains("JC_CLIENT_SECRET"),
                "{text}"
            );
            assert!(text.contains("\"helsinki/citybikes-free\""), "{text}");
        }
    }

    /// PL-64: an expiry the door would refuse never reaches the runner as a sweep; it is said as
    /// the sweep's error, and the pipeline itself still runs.
    #[tokio::test]
    async fn an_expiry_that_does_not_validate_sends_no_sweep() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let mut pipeline = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("pipeline");
        pipeline.spec["expiry"] = serde_json::json!({ "after": "30m", "types": ["Vehicle"] });
        mirror.upsert(pipeline);

        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(
            outcomes[0],
            (
                "helsinki".to_owned(),
                "citybikes-free".to_owned(),
                StreamOutcome::Live
            )
        );
        match &outcomes[1] {
            (_, name, StreamOutcome::Error(said)) => {
                assert_eq!(name, "citybikes-free.expiry");
                assert!(said.contains("`30m`"), "{said}");
            }
            other => panic!("the sweep was not refused: {other:?}"),
        }
        let sent = server.received_requests().await.expect("recorded");
        assert!(
            sent.iter().all(|r| !r.url.path().ends_with(".expiry")),
            "the runner was sent a sweep"
        );
    }

    /// PL-64: a pipeline with expiry gets its sweep beside its own stream; a pipeline without it,
    /// or disabled, gets none, and a sweep whose expiry was removed is retired.
    #[tokio::test]
    async fn an_expiring_pipeline_runs_its_sweep_and_loses_it_with_the_expiry() {
        let server = runner().await;
        for path in [
            "/streams/helsinki.citybikes-free",
            "/streams/helsinki.citybikes-free.expiry",
        ] {
            wiremock::Mock::given(wiremock::matchers::method("PUT"))
                .and(wiremock::matchers::path(path))
                .respond_with(wiremock::ResponseTemplate::new(200))
                .mount(&server)
                .await;
        }
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path(
                "/streams/helsinki.citybikes-free.expiry",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let mut pipeline = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("pipeline");
        let plain = pipeline.clone();
        pipeline.spec["expiry"] = serde_json::json!({ "after": "14d", "types": ["Vehicle"] });
        mirror.upsert(pipeline.clone());

        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        let names: Vec<&str> = outcomes.iter().map(|(_, name, _)| name.as_str()).collect();
        assert_eq!(names, ["citybikes-free", "citybikes-free.expiry"]);
        assert!(outcomes.iter().all(|(_, _, o)| *o == StreamOutcome::Live));
        let sent = server.received_requests().await.expect("recorded");
        let sweep = sent
            .iter()
            .find(|r| {
                r.method.as_str() == "PUT"
                    && r.url.path() == "/streams/helsinki.citybikes-free.expiry"
            })
            .expect("the sweep was sent");
        let body: Value = serde_json::from_slice(&sweep.body).expect("json");
        assert!(body
            .to_string()
            .contains("/api/endpoint/abc123456789012345678901234/"));

        // Disabled: neither the stream nor its sweep.
        let mut disabled = pipeline.clone();
        disabled.spec["enabled"] = serde_json::json!(false);
        mirror.upsert(disabled);
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert!(outcomes
            .iter()
            .all(|(_, name, _)| !name.ends_with(".expiry")));

        // Expiry removed: the sweep is retired (the DELETE expectation above).
        mirror.upsert(plain);
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert!(outcomes
            .iter()
            .all(|(_, name, _)| !name.ends_with(".expiry")));
    }

    #[tokio::test]
    async fn a_pipeline_failing_its_kinds_checks_never_reaches_the_runner() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let mirror = helsinki_test_mirror();
        let mut pipeline = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("the pipeline");
        pipeline.spec["compute"]["bloblang"] =
            serde_json::json!("root = env(\"JC_CLIENT_SECRET_OTHER\")");
        mirror.upsert(pipeline);
        let outcomes = StreamDeployer::new(server.uri())
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes.len(), 1);
        assert!(
            matches!(&outcomes[0].2, StreamOutcome::Error(said) if said.contains("JC_CLIENT_SECRET_OTHER")),
            "{outcomes:?}"
        );
    }

    #[tokio::test]
    async fn put_success_is_live_and_refusal_is_error() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].2, StreamOutcome::Live);

        let server_err = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(
                wiremock::ResponseTemplate::new(400).set_body_string("lint error in line 3"),
            )
            .mount(&server_err)
            .await;

        let deployer_err = StreamDeployer::new(server_err.uri());
        let outcomes_err = deployer_err
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes_err.len(), 1);
        match &outcomes_err[0].2 {
            StreamOutcome::Error(err) => {
                assert!(err.contains("400") && err.contains("lint error"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_runner_receives_the_space_segment_where_the_mapping_reads_jc_space() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let mut pipeline = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("the pipeline");
        pipeline.spec["compute"]["bloblang"] = serde_json::json!(
            "root.id = \"urn:ngsi-ld:X:\" + env(\"JC_ORG_DOMAIN\") + \":\" + env(\"JC_SPACE\") + \":\" + this.id"
        );
        mirror.upsert(pipeline);
        let bentos = Bentos::new();
        let outcomes = deployer
            .converge(&mirror, &bentos, &Default::default())
            .await;
        assert_eq!(outcomes[0].2, StreamOutcome::Live, "{outcomes:?}");
        let sent = server.received_requests().await.expect("recorded");
        // The first request asks whether the runner already holds the stream (T-2891).
        let put = sent
            .iter()
            .find(|request| request.method.as_str() == "PUT")
            .expect("the stream was PUT");
        let body = String::from_utf8_lossy(&put.body).to_string();
        // The space `helsinki` has no manifest here, so it renders `helsinki-helsinki` (PF-84);
        // the domain stays the runner's own variable.
        assert!(body.contains(r#"+ \"helsinki-helsinki\" +"#), "{body}");
        assert!(!body.contains("JC_SPACE"), "{body}");
        assert!(body.contains("JC_ORG_DOMAIN"), "{body}");

        // A pin on the space is what the runner gets instead.
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "ContextSpace".to_owned(),
            metadata: ObjectMeta {
                name: "helsinki".to_owned(),
                namespace: Some("helsinki".to_owned()),
                ..Default::default()
            },
            spec: serde_json::json!({ "urnSegment": "helsinki" }),
            status: None,
        });
        let deployer = StreamDeployer::new(server.uri());
        deployer
            .converge(&mirror, &bentos, &Default::default())
            .await;
        let sent = server.received_requests().await.expect("recorded");
        let body = String::from_utf8_lossy(&sent.last().expect("a second PUT").body).to_string();
        assert!(body.contains(r#"+ \"helsinki\" +"#), "{body}");
    }

    fn air_model(version: &str) -> Arc<crate::pipeline_validation::ModelSchema> {
        Arc::new(crate::pipeline_validation::ModelSchema::compile(
            "helsinki",
            version,
            &serde_json::json!({ "definitions": { "BikeHireDockingStation": {
                "additionalProperties": false,
                "properties": { "id": { "type": "string" }, "name": { "type": "string", "x-ngsi-ld-kind": "Property" } }
            }}}),
            &["BikeHireDockingStation".to_owned()],
            false,
        ))
    }

    /// PL-60, PL-61: a stream into a space that names a model carries the stage and the
    /// rejected sink right before the batch split, with the space written into the id rule; a
    /// new model version is a new render, and a space with no model gets no stage.
    #[tokio::test]
    async fn a_stream_into_a_modelled_space_validates_before_it_splits_into_batches() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let schemas = Arc::new(crate::pipeline_validation::ModelSchemas::default());
        let deployer = StreamDeployer::new(server.uri())
            .with_validation("http://portal-internal:8081/", Arc::clone(&schemas));
        let mirror = helsinki_test_mirror();
        let bentos = Bentos::new();
        let last = |requests: Vec<wiremock::Request>| -> Value {
            serde_json::from_slice(&requests.last().expect("a PUT").body).expect("json")
        };

        // No model: the stream is what it was.
        deployer
            .converge(&mirror, &bentos, &Default::default())
            .await;
        let plain = last(server.received_requests().await.expect("recorded"));
        assert!(!plain.to_string().contains("json_schema"), "{plain}");

        schemas.replace(HashMap::from([(
            ("helsinki".to_owned(), "helsinki".to_owned()),
            air_model("1.0.0"),
        )]));
        deployer
            .converge(&mirror, &bentos, &Default::default())
            .await;
        let staged = last(server.received_requests().await.expect("recorded"));
        let processors = staged["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let at = |label: &str| processors.iter().position(|p| p["label"] == label);
        let split = processors
            .iter()
            .position(|p| p.get("split").is_some())
            .expect("the split");
        let (validation, id_rule, rejected) = (
            at("validation").expect("the stage"),
            at("validation_id").expect("the id rule"),
            at("rejected").expect("the sink"),
        );
        assert!(
            validation < id_rule && id_rule < rejected && rejected < split,
            "{processors:?}"
        );
        let text = staged.to_string();
        assert!(
            text.contains(
                "http://portal-internal:8081/internal/pipelines/helsinki/citybikes-free/rejected"
            ),
            "{text}"
        );
        assert!(
            !text.contains("env(\\\"JC_SPACE\\\")"),
            "the space is written in: {text}"
        );
        // The write is unchanged and first; the outcome sink follows it only once it took the
        // batch, and drops rather than retries (PL-62). A failed write goes back to the input
        // rather than round the broker forever, so the stream can always stop (T-2983).
        let outputs = &staged["output"]["broker"]["outputs"];
        assert_eq!(
            staged["output"]["broker"]["pattern"],
            "fan_out_sequential_fail_fast"
        );
        assert_eq!(
            upsert_at(&staged, "write_0")["verb"],
            "POST",
            "the write is unchanged"
        );
        assert_eq!(outputs[0]["switch"]["cases"][0]["check"], "errored()");
        assert_eq!(outputs[0]["label"], "output");
        assert_eq!(outputs[1]["label"], crate::pipeline_log::SINK_LABELS[0]);
        assert_eq!(outputs[1]["fallback"][0]["http_client"]["retries"], 0);
        assert!(outputs[1]["fallback"][1].get("drop").is_some());
        assert_eq!(
            outputs[1]["fallback"][0]["http_client"]["url"],
            "http://portal-internal:8081/internal/pipelines/helsinki/citybikes-free/outcomes"
        );

        // A changed model is a changed render: the stage the runner gets is the new one.
        let mut changed = (*air_model("1.1.0")).clone();
        changed
            .classes
            .insert("Alert".into(), serde_json::json!({}));
        schemas.replace(HashMap::from([(
            ("helsinki".to_owned(), "helsinki".to_owned()),
            Arc::new(changed),
        )]));
        deployer
            .converge(&mirror, &bentos, &Default::default())
            .await;
        let restaged = last(server.received_requests().await.expect("recorded"));
        assert_ne!(restaged, staged);
        assert!(
            restaged.to_string().contains("this.type == \\\"Alert\\\""),
            "{restaged}"
        );
    }

    #[tokio::test]
    async fn the_runner_receives_the_source_segment_where_the_mapping_reads_jc_source_space() {
        // An indicator pipeline records where its figures came from, as the source Endpoint's
        // URN. Nothing injected `JC_SOURCE_SPACE`, so the mapping reached the runner with the
        // variable still in it, the runner resolved it to nothing, and every indicator was
        // refused `attribute derivedFrom: object must be a URI` one by one while the stream
        // reported a 2xx for the batch. The space it writes stayed empty (T-2445).
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.kpi-pipe"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mirror = Mirror::new();
        for (name, space) in [
            ("kpi-writer", "helsinki-kpi"),
            ("helsinki-all", "helsinki-raw"),
        ] {
            mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_string(),
                kind: "Endpoint".to_string(),
                metadata: ObjectMeta {
                    name: name.to_string(),
                    namespace: Some("helsinki".to_string()),
                    ..Default::default()
                },
                spec: serde_json::json!({
                    "contextSpaceRef": space,
                    "slug": "targetslug1234567890123456",
                    "audience": "public",
                    "enabledRepresentations": ["ngsi-ld"]
                }),
                status: None,
            });
        }
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "ContextSpace".to_string(),
            metadata: ObjectMeta {
                name: "helsinki-raw".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: serde_json::json!({ "urnSegment": "helsinki-raw" }),
            status: None,
        });
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Pipeline".to_string(),
            metadata: ObjectMeta {
                name: "kpi-pipe".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: serde_json::json!({
                "class": "scheduled",
                "source": {
                    "endpointRef": { "kind": "Endpoint", "name": "helsinki-all" },
                    "query": { "type": "BikeHireDockingStation" }
                },
                "compute": {
                    "kind": "bloblang",
                    "bloblang": "root.derivedFrom = \"urn:ngsi-ld:Endpoint:x:\" + env(\"JC_SOURCE_SPACE\") + \":all\""
                },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki-kpi:kpi-writer"
            }),
            status: None,
        });

        // An Endpoint read on no clock runs once from the queue (PL-78): the run is PUT on the
        // queue's next tick, not by the pass.
        let deployer = StreamDeployer::new(server.uri());
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert!(
            matches!(
                &outcomes[0].2,
                StreamOutcome::Scheduled("Queued" | "Waiting", _)
            ),
            "{outcomes:?}"
        );
        run_queue::tick(&deployer.queue, &deployer, chrono::Utc::now()).await;
        let sent = server.received_requests().await.expect("recorded");
        let put = sent
            .iter()
            .find(|request| request.method.as_str() == "PUT")
            .expect("the stream was PUT");
        let body = String::from_utf8_lossy(&put.body).to_string();
        assert!(body.contains(r#"+ \"helsinki-raw\" +"#), "{body}");
        assert!(!body.contains("JC_SOURCE_SPACE"), "{body}");
    }

    #[tokio::test]
    async fn put_404_falls_back_to_post() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].2, StreamOutcome::Live);
    }

    #[tokio::test]
    async fn a_runner_that_restarted_gets_its_unchanged_streams_again() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;
        // The runner lists the stream after the first PUT, then restarts and lists nothing. The
        // first pass reads the list once more, for the streams under a bare name (T-3002).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "helsinki.citybikes-free": { "active": true } }),
            ))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        // PUT, then left running, then sent again to the empty runner: two PUTs in three passes.
        for _ in 0..3 {
            let outcomes = deployer
                .converge(&mirror, &Bentos::new(), &Default::default())
                .await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live);
        }
    }

    /// A stream the runner holds but no longer runs is dead, not live: it is sent again, which
    /// restarts it and makes its output fetch a fresh token (T-2336, PL-18).
    #[tokio::test]
    async fn a_stream_the_runner_reports_inactive_is_sent_again() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(3)
            .mount(&server)
            .await;
        // The runner holds it and says it is not running: its output was refused, over and over.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "helsinki.citybikes-free": { "active": false, "uptime": 1800.0 } }),
            ))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        // Three passes over an unchanged manifest: three PUTs, because it is dead on every one.
        for _ in 0..3 {
            let outcomes = deployer
                .converge(&mirror, &Bentos::new(), &Default::default())
                .await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live);
        }
    }

    /// `rendered` as Bento answers it: every `${VAR}` filled in from the runner's environment,
    /// `${! … }` kept, the way the pinned 1.21.1 answers `GET /streams/{id}` (T-3006).
    fn as_the_runner_answers(rendered: &serde_json::Value) -> serde_json::Value {
        let reference =
            regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::[^}]*)?\}").expect("a regex");
        let text = serde_json::to_string(rendered).expect("JSON");
        serde_json::from_str(&reference.replace_all(&text, "value-of-$1")).expect("JSON")
    }

    /// T-3006: the runner's answer holds a render when only its environment references differ.
    #[test]
    fn a_config_is_held_as_rendered_up_to_the_runners_environment() {
        let rendered = serde_json::json!({
            "url": "${JC_GATEWAY_URL}/api/endpoint/abc/upsert",
            "oauth2": { "client_secret": "${JC_CLIENT_SECRET}", "scopes": ["${SCOPE:openid}"] },
            "mapping": "meta x = \"${! error() }\"",
            "drop_on": [400, 413],
        });
        let held = serde_json::json!({
            "url": "http://gateway.test/api/endpoint/abc/upsert",
            "oauth2": { "client_secret": "s3cr3t\nwith a line", "scopes": [""] },
            "mapping": "meta x = \"${! error() }\"",
            "drop_on": [400, 413],
        });
        assert!(held_as_rendered(&rendered, &held));
        assert!(held_as_rendered(&rendered, &rendered));

        for (pointer, other) in [
            // Another endpoint behind the same variable, a per-message interpolation the runner
            // would have kept, another item, one more key.
            (
                "/url",
                serde_json::json!("http://gateway.test/api/endpoint/xyz/upsert"),
            ),
            ("/mapping", serde_json::json!("meta x = \"boom\"")),
            ("/drop_on", serde_json::json!([400, 422])),
            ("/drop_on", serde_json::json!([400, 413, 422])),
            (
                "/oauth2",
                serde_json::json!({ "client_secret": "s", "scopes": [""], "extra": 1 }),
            ),
            ("/oauth2/scopes", serde_json::json!("openid")),
        ] {
            let mut changed = held.clone();
            *changed.pointer_mut(pointer).expect("a member") = other;
            assert!(
                !held_as_rendered(&rendered, &changed),
                "{pointer}: {changed}"
            );
        }
        // The regex metacharacters of a literal are literal.
        assert!(!held_as_rendered(
            &serde_json::json!("a.c${V}"),
            &serde_json::json!("abcx")
        ));
        assert!(held_as_rendered(
            &serde_json::json!("a.c${V}"),
            &serde_json::json!("a.cx")
        ));
    }

    /// What one converge PUT to a runner that accepts everything: the rendered stream as sent.
    async fn rendered_citybikes() -> serde_json::Value {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        StreamDeployer::new(server.uri())
            .converge(&helsinki_test_mirror(), &Bentos::new(), &Default::default())
            .await;
        let sent = server.received_requests().await.expect("recorded");
        let put = sent
            .iter()
            .find(|request| request.method.as_str() == "PUT")
            .expect("the stream was PUT");
        serde_json::from_slice(&put.body).expect("a JSON stream")
    }

    /// A run's DELETE the runner does not answer in time is a stop that never came (a looping
    /// step holds it, T-5370): the queue hears false; a stop that answers, 404 included, is true.
    #[tokio::test]
    async fn a_delete_the_runner_does_not_answer_in_time_says_the_run_is_held() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path("/streams/helsinki.loop"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_delay(Duration::from_millis(1500)),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path("/streams/helsinki.done"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let deployer = StreamDeployer::new(server.uri())
            .with_timeouts(Duration::from_secs(5), Duration::from_millis(300));
        let key = |name: &str| ("helsinki".to_owned(), name.to_owned());
        assert!(!Runner::delete(&deployer, &key("loop")).await);
        assert!(Runner::delete(&deployer, &key("done")).await);
    }

    /// A runner whose `GET /streams/citybikes-free` answers `held`, and which counts every PUT.
    async fn runner_holding(held: serde_json::Value, puts: u64) -> wiremock::MockServer {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(held))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "helsinki.citybikes-free": { "active": true } }),
            ))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(puts)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        server
    }

    /// T-2891: a Portal that restarted has no hashes, and the runner answers each stream's config
    /// as it was sent, its environment filled in (T-3006). A stream it runs as rendered is adopted, not PUT again, on the first
    /// pass and on the passes after it; one it holds with another config, or holds stopped, is sent.
    #[tokio::test]
    async fn a_restarted_portal_adopts_the_stream_the_runner_runs_as_rendered() {
        let rendered = rendered_citybikes().await;
        let mirror = helsinki_test_mirror();

        // The runner answers with its environment filled in (T-3006); the render as sent is
        // adopted too.
        assert!(rendered.to_string().contains("${JC_GATEWAY_URL}"));
        for config in [as_the_runner_answers(&rendered), rendered.clone()] {
            let same =
                runner_holding(serde_json::json!({ "active": true, "config": config }), 0).await;
            let restarted = StreamDeployer::new(same.uri());
            for _ in 0..2 {
                let outcomes = restarted
                    .converge(&mirror, &Bentos::new(), &Default::default())
                    .await;
                assert_eq!(outcomes[0].2, StreamOutcome::Live, "{outcomes:?}");
            }
        }

        let mut changed = rendered.clone();
        changed["pipeline"]["processors"] = serde_json::json!([{ "mapping": "root = this" }]);
        for held in [
            serde_json::json!({ "active": true, "config": changed }),
            serde_json::json!({ "active": false, "config": rendered }),
            serde_json::json!({ "active": true }),
        ] {
            let runner = runner_holding(held.clone(), 1).await;
            let outcomes = StreamDeployer::new(runner.uri())
                .converge(&mirror, &Bentos::new(), &Default::default())
                .await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live, "{held}: {outcomes:?}");
        }
    }

    /// T-2891: a PUT the runner does not answer is not sent again on the next pass: the pipeline
    /// says when it will be, and nothing but the adoption check reaches the runner meanwhile. A
    /// changed render is sent at once. The runner here takes each connection and drops it, the
    /// transport failure a PUT that outlives the timeout ends in.
    #[tokio::test]
    async fn a_put_the_runner_does_not_answer_waits_before_it_is_sent_again() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let address = listener.local_addr().expect("an address");
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 16];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).to_string();
                let verb = head.split(' ').next().unwrap_or_default().to_owned();
                log.lock().expect("the log").push(verb);
                drop(socket);
            }
        });
        let verbs = || seen.lock().expect("the log").clone();
        // Each send starts with the project's write budget (T-5803), the first request a
        // runner that drops every connection sees of it.
        let puts = || {
            verbs()
                .iter()
                .filter(|verb| verb.as_str() == "POST")
                .count()
        };

        let deployer = StreamDeployer::new(format!("http://{address}"));
        let mirror = helsinki_test_mirror();
        let first = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        let StreamOutcome::Error(said) = &first[0].2 else {
            panic!("an unanswered PUT is not live: {first:?}");
        };
        assert!(
            said.contains("did not answer") && said.contains("again in 60 s"),
            "{said}"
        );
        assert_eq!(puts(), 1, "{:?}", verbs());

        let second = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        let StreamOutcome::Error(said) = &second[0].2 else {
            panic!("a stream waiting to be sent is not live: {second:?}");
        };
        assert!(said.contains("the last change of this stream"), "{said}");
        assert_eq!(puts(), 1, "the waiting stream was PUT again: {:?}", verbs());

        let mut pipeline = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("the pipeline");
        pipeline.spec["compute"]["bloblang"] = serde_json::json!("root = this.data.stations");
        mirror.upsert(pipeline);
        let third = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert!(
            matches!(&third[0].2, StreamOutcome::Error(said) if said.contains("again in 60 s")),
            "{third:?}"
        );
        assert_eq!(
            puts(),
            2,
            "a changed render waits for nothing: {:?}",
            verbs()
        );
    }

    /// A runner whose PUT answers after `delay`, holding none of the streams.
    async fn slow_runner(delay: Duration) -> wiremock::MockServer {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_delay(delay))
            .mount(&server)
            .await;
        server
    }

    /// T-4060: Bento stops the old stream under the PUT's own context, so a replace answers only
    /// once the old stream's writes are done. A replace that outlasts the timeout of every other
    /// request is still waited for and is Live, where it used to be cancelled into an error.
    #[tokio::test]
    async fn a_replace_slower_than_a_plain_request_is_waited_for() {
        let runner = slow_runner(Duration::from_millis(1500)).await;
        let outcomes = StreamDeployer::new(runner.uri())
            .with_timeouts(Duration::from_secs(1), Duration::from_secs(10))
            .converge(&helsinki_test_mirror(), &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes[0].2, StreamOutcome::Live, "{outcomes:?}");

        let impatient = StreamDeployer::new(runner.uri())
            .with_timeouts(Duration::from_secs(1), Duration::from_secs(1))
            .converge(&helsinki_test_mirror(), &Bentos::new(), &Default::default())
            .await;
        assert!(
            matches!(&impatient[0].2, StreamOutcome::Error(said) if said.contains("did not answer")),
            "a replace past its own timeout still backs off: {impatient:?}"
        );
    }

    /// T-4060: a render that changes every stream sends them a few at once, each in its own
    /// place among the outcomes, so one slow stop does not hold every stream after it.
    #[tokio::test]
    async fn a_pass_sends_its_streams_a_few_at_once_and_keeps_their_order() {
        let mirror = helsinki_test_mirror();
        let pipeline = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("the pipeline");
        let names: Vec<String> = (1..=SENDS_AT_ONCE).map(|n| format!("copy-{n}")).collect();
        for name in &names {
            let mut copy = pipeline.clone();
            copy.metadata.name = name.clone();
            mirror.upsert(copy);
        }
        let runner = slow_runner(Duration::from_millis(800)).await;
        let started = Instant::now();
        let outcomes = StreamDeployer::new(runner.uri())
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        let took = started.elapsed();
        assert_eq!(outcomes.len(), SENDS_AT_ONCE + 1, "{outcomes:?}");
        assert!(
            outcomes.iter().all(|o| o.2 == StreamOutcome::Live),
            "{outcomes:?}"
        );
        let mut listed: Vec<&str> = outcomes.iter().map(|o| o.1.as_str()).collect();
        let order = listed.clone();
        listed.sort_unstable();
        assert_eq!(order, listed, "the outcomes keep the mirror's order");
        assert!(
            took < Duration::from_millis(800 * 3),
            "{} streams of 0.8 s each took {took:?}",
            SENDS_AT_ONCE + 1
        );
    }

    /// Which inputs end on their own, because that is what tells a finished stream from a dead one
    /// when the runner reports it inactive (T-2336, PL-50).
    #[test]
    fn an_endless_input_is_told_apart_from_one_that_finishes() {
        for endless in [
            serde_json::json!({ "generate": { "interval": "60s", "mapping": "root = \"\"" } }),
            serde_json::json!({ "generate": { "interval": "60s", "count": 0 } }),
            serde_json::json!({ "mqtt": { "urls": ["tcp://mqtt:1883"] } }),
            serde_json::json!({ "nats": { "urls": ["nats://nats:4222"] } }),
            serde_json::json!({ "kafka": { "addresses": ["kafka:9092"] } }),
            serde_json::json!({ "http_server": {} }),
            serde_json::json!({ "broker": { "inputs": [{ "file": {} }, { "mqtt": {} }] } }),
            // Nothing recognisable is not a licence to let a dead stream lie.
            serde_json::json!({}),
            serde_json::json!({ "label": "input_0" }),
            serde_json::json!({ "broker": { "inputs": [] } }),
            serde_json::json!("not an input"),
        ] {
            assert!(
                !input_ends_by_itself(&endless),
                "{endless} does not end by itself"
            );
        }
        for finite in [
            serde_json::json!({ "file": { "paths": ["/data/stations.json"] } }),
            serde_json::json!({ "csv": { "paths": ["/data/stations.csv"] } }),
            serde_json::json!({ "parquet": { "paths": ["/data/stations.parquet"] } }),
            serde_json::json!({ "aws_s3": { "bucket": "open-data" } }),
            serde_json::json!({ "generate": { "interval": "1s", "count": 3 } }),
            serde_json::json!({ "label": "input_0", "file": { "paths": ["/data/x.json"] } }),
            serde_json::json!({ "broker": { "inputs": [{ "file": {} }, { "csv": {} }] } }),
        ] {
            assert!(input_ends_by_itself(&finite), "{finite} finishes");
        }
    }

    /// The other half of the same rule: a stream that is running is left alone, so a healthy
    /// periodic pipeline is not restarted on every pass and does not emit on every pass (T-0659).
    #[tokio::test]
    async fn a_stream_the_runner_reports_active_is_left_alone() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "helsinki.citybikes-free": { "active": true, "uptime": 1320.0 } }),
            ))
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        for _ in 0..3 {
            let outcomes = deployer
                .converge(&mirror, &Bentos::new(), &Default::default())
                .await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live);
        }
    }

    /// A runner whose answer carries no `active` flag is taken at its word: an unknown body shape
    /// must not restart every stream of a project on every pass.
    #[tokio::test]
    async fn a_listing_without_the_active_flag_is_taken_at_its_word() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        for body in [
            serde_json::json!({ "helsinki.citybikes-free": {} }),
            serde_json::json!({ "helsinki.citybikes-free": { "uptime": 1320.0 } }),
            serde_json::json!({ "helsinki.citybikes-free": { "active": "yes" } }),
        ] {
            wiremock::Mock::given(wiremock::matchers::method("GET"))
                .and(wiremock::matchers::path("/streams"))
                .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
                .up_to_n_times(1)
                .mount(&server)
                .await;
        }

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        for _ in 0..4 {
            let outcomes = deployer
                .converge(&mirror, &Bentos::new(), &Default::default())
                .await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live);
        }
    }

    #[test]
    fn an_http_feed_keeps_a_body_that_is_not_json() {
        let rendered = render_stream(
            &helsinki_pipeline_spec(),
            "p",
            "helsinki",
            &helsinki_datasource_spec(),
            "src",
            "abc123",
            None,
        )
        .expect("renders");
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        // `this` would parse an RSS or CSV body as JSON and fail on every fetch. The label is
        // the reconciler's own, so the counters of this processor are attributable (T-1125).
        assert_eq!(
            processors[1],
            serde_json::json!({
                "label": "processor_1",
                "mutation": "root = if errored() { deleted() }"
            })
        );
    }

    /// T-5721: a read that timed out or was refused is a failed line in the run's log, not a run
    /// of sent 0 and failed 0 that reads as an empty source.
    #[test]
    fn a_failed_source_read_is_reported_to_the_run_log_in_place_of_its_drop() {
        let mut rendered = render_stream(
            &helsinki_pipeline_spec(),
            "p",
            "helsinki",
            &helsinki_datasource_spec(),
            "src",
            "abc123",
            None,
        )
        .expect("renders");
        StreamDeployer::new("http://runner")
            .with_validation(
                "http://portal/",
                Arc::new(crate::pipeline_validation::ModelSchemas::default()),
            )
            .reporting(&mut rendered, "helsinki", "p", &["helsinki".to_owned()]);
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert!(processors.iter().all(|p| p["mutation"] != DROP_ERRORED));
        let report = processors
            .iter()
            .find(|p| p["label"] == "processor_1")
            .expect("the node keeps its name");
        assert_eq!(report["switch"][0]["check"], "errored()");
        let text = report.to_string();
        assert!(
            text.contains("http://portal/internal/pipelines/helsinki/p/outcomes"),
            "{text}"
        );
        assert!(text.contains("\\\"failed\\\": [{"), "{text}");
        assert_eq!(
            report["switch"][0]["processors"][1]["mapping"],
            "root = deleted()"
        );
    }

    /// T-5721: reads inside the input (a merged source, a broker of several) give one report at
    /// the head of the stream's processors, so no Bento label repeats.
    #[test]
    fn failed_reads_inside_the_input_share_one_report_at_the_head() {
        let read = |url: &str| {
            serde_json::json!({ "generate": { "interval": "1h", "mapping": "root = \"\"" },
                "processors": [{ "try": [{ "http": { "url": url } }] }, { "mutation": DROP_ERRORED }] })
        };
        let mut stream = serde_json::json!({
            "input": { "broker": { "inputs": [read("https://a.example"), read("https://b.example")] } },
            "pipeline": { "processors": [{ "label": "pass", "branch": {} }, { "catch": [] }, { "mapping": "root = this" }] },
        });
        with_source_failures(&mut stream, "http://portal/outcomes");
        assert!(!stream.to_string().contains(DROP_ERRORED), "{stream}");
        let processors = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert_eq!(processors.len(), 4);
        assert_eq!(processors[0]["switch"][0]["check"], "errored()");
        assert_eq!(processors[1]["label"], "pass");
        assert_eq!(stream.to_string().matches("\"source_failed\"").count(), 1);

        // An http_client input is reported by the run queue's one tick: nothing is moved.
        let mut resident = serde_json::json!({
            "input": { "broker": { "inputs": [read("https://a.example"), { "http_client": { "url": "https://rt.example" } }] } },
            "pipeline": { "processors": [{ "mapping": "root = this" }] },
        });
        let before = resident.clone();
        with_source_failures(&mut resident, "http://portal/outcomes");
        assert_eq!(resident, before);
    }

    #[tokio::test]
    async fn an_unchanged_render_is_not_put_again_and_a_change_is() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        // Two passes over the same manifests: one PUT.
        for _ in 0..2 {
            let outcomes = deployer
                .converge(&mirror, &Bentos::new(), &Default::default())
                .await;
            assert_eq!(outcomes[0].2, StreamOutcome::Live);
        }
        // A changed mapping renders differently: the second PUT.
        let mut changed = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("the test pipeline");
        changed.spec["compute"]["bloblang"] = serde_json::json!("root = this.data");
        mirror.upsert(changed);
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes[0].2, StreamOutcome::Live);
    }

    /// PL-78 (T-4348): a socket, a broker and a server stay loaded; a poll is never sent by the
    /// pass, only planned, and its resident stream from an older Portal is not retired by it.
    #[tokio::test]
    async fn push_sources_stay_resident_and_a_poll_is_only_queued() {
        let server = runner().await;
        for name in ["citybikes-free", "hfp", "webhook"] {
            wiremock::Mock::given(wiremock::matchers::method("PUT"))
                .and(wiremock::matchers::path(format!(
                    "/streams/helsinki.{name}"
                )))
                .respond_with(wiremock::ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;
        }
        wiremock::Mock::given(wiremock::matchers::path("/streams/helsinki.harvest"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let mirror = helsinki_test_mirror();
        let template = mirror
            .get("helsinki", "Pipeline", "citybikes-free")
            .expect("the test pipeline");
        for (name, class, extra, source) in [
            (
                "hfp",
                "resident",
                serde_json::json!({}),
                serde_json::json!({ "type": "mqtt", "mqtt": { "urls": ["tcp://mqtt.hsl.fi:1883"], "topics": ["/hfp/#"] } }),
            ),
            (
                "webhook",
                "resident",
                serde_json::json!({}),
                serde_json::json!({ "type": "http_server", "input": { "path": "/hook" } }),
            ),
            (
                "harvest",
                "scheduled",
                serde_json::json!({ "schedule": "0 5 * * *" }),
                serde_json::json!({ "type": "http", "http": { "url": "https://example.org/data.json" } }),
            ),
        ] {
            let mut source_env = mirror
                .get("helsinki", "DataSource", "hsl-citybikes-free")
                .expect("the test source");
            source_env.metadata.name = name.to_owned();
            source_env.spec = source;
            mirror.upsert(source_env);
            let mut pipe = template.clone();
            pipe.metadata.name = name.to_owned();
            pipe.spec["class"] = serde_json::json!(class);
            pipe.spec["source"]["dataSourceRef"]["name"] = serde_json::json!(name);
            pipe.spec
                .as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            mirror.upsert(pipe);
        }
        let deployer = StreamDeployer::new(server.uri());
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        let by_name: HashMap<&str, &StreamOutcome> = outcomes
            .iter()
            .map(|(_, name, outcome)| (name.as_str(), outcome))
            .collect();
        for resident in ["citybikes-free", "hfp", "webhook"] {
            assert_eq!(by_name[resident], &StreamOutcome::Live, "{outcomes:?}");
        }
        assert!(
            matches!(by_name["harvest"], StreamOutcome::Scheduled("Waiting", said) if said.starts_with("next run ")),
            "{outcomes:?}"
        );
        // A second pass leaves the resident ones alone (one PUT each) and still sends no poll.
        deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
    }

    #[tokio::test]
    async fn retire_deletes_what_disappeared() {
        let server = runner().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path("/streams/helsinki.citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let deployer = StreamDeployer::new(server.uri());
        let mirror = helsinki_test_mirror();
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        assert_eq!(outcomes[0].2, StreamOutcome::Live);

        let empty_mirror = Mirror::new();
        let outcomes_empty = deployer
            .converge(&empty_mirror, &Bentos::new(), &Default::default())
            .await;
        assert!(outcomes_empty.is_empty());
    }

    /// T-3002: one runner serves every project, and helsinki/air-quality and praha/air-quality
    /// shared the stream `air-quality`: the last PUT won and the other read Live without running.
    /// Each project's pipeline is its own stream now, and the stream an older Portal left under
    /// the bare name is removed once, not on every pass.
    #[tokio::test]
    async fn two_projects_with_one_pipeline_name_run_two_streams() {
        let server = runner().await;
        for project in ["helsinki", "praha"] {
            wiremock::Mock::given(wiremock::matchers::method("PUT"))
                .and(wiremock::matchers::path(format!(
                    "/streams/{project}.citybikes-free"
                )))
                .respond_with(wiremock::ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;
        }
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/streams"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "citybikes-free": { "active": true },
                    "helsinki.citybikes-free": { "active": true },
                    "praha.citybikes-free": { "active": true },
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path("/streams/citybikes-free"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;

        let mirror = helsinki_test_mirror();
        for kind in ["DataSource", "Endpoint", "Pipeline"] {
            for mut envelope in mirror
                .list("helsinki", kind, &crate::store::ListOptions::default())
                .items
            {
                envelope.metadata.namespace = Some("praha".to_owned());
                mirror.upsert(envelope);
            }
        }
        let deployer = StreamDeployer::new(server.uri());
        for _ in 0..2 {
            let outcomes = deployer
                .converge(&mirror, &Bentos::new(), &Default::default())
                .await;
            let live: Vec<_> = outcomes
                .iter()
                .filter(|(_, _, outcome)| *outcome == StreamOutcome::Live)
                .map(|(ns, name, _)| format!("{ns}/{name}"))
                .collect();
            assert_eq!(live, ["helsinki/citybikes-free", "praha/citybikes-free"]);
        }
        // Once per project on a shared runner: the second project's DELETE answers what the
        // first already removed, and neither is asked again on the next pass.
    }

    #[test]
    fn stream_ids_carry_the_project() {
        assert_eq!(stream_id("helsinki", "air-quality"), "helsinki.air-quality");
        assert_ne!(
            stream_id("helsinki", "air-quality"),
            stream_id("praha", "air-quality")
        );
        assert_eq!(
            stream_id("helsinki", &crate::pipeline_expiry::sweep_name("reaper")),
            "helsinki.reaper.expiry"
        );
    }

    /// T-3952, PL-72: an `http` DataSource with an archive reads its directory by range, then
    /// each named member, after the clock's poll and before the author's steps.
    #[test]
    fn an_archive_datasource_reads_its_directory_then_its_members() {
        let ds: DataSourceSpec = serde_json::from_value(serde_json::json!({
            "type": "http",
            "http": {
                "url": "https://infopalvelut.storage.hsldev.com/gtfs/hsl.zip",
                "archive": { "format": "zip", "members": ["stops.txt"], "maxBytes": 4194304 }
            }
        }))
        .expect("valid archive DataSourceSpec");
        let rendered = render_stream(
            &helsinki_pipeline_spec(),
            "stops",
            "helsinki",
            &ds,
            "hsl-gtfs",
            "abc123456789",
            None,
        )
        .expect("renders an archive stream");
        let processors = rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert!(rendered["input"]["generate"].is_object());
        assert_eq!(
            processors[0]["try"][0]["http"]["headers"]["Range"],
            "bytes=-65557"
        );
        assert_eq!(
            processors[1]["mutation"],
            "root = if errored() { deleted() }"
        );
        let keys = |steps: &serde_json::Value| -> Vec<String> {
            steps
                .as_array()
                .expect("steps")
                .iter()
                .filter_map(|step| step.as_object()?.keys().next().cloned())
                .collect()
        };
        // The body bound (PL-113) refuses an oversized directory read before it is parsed.
        assert!(processors[3]["catch"].is_array());
        assert_eq!(keys(&processors[4]["try"]), ["mapping", "unarchive"]);
        assert!(processors[5]["catch"].is_array());
        // Each member is a batch of its own and inflates through `unarchive: zip`, not the
        // wasm module Bento never frees (T-4752, PL-72).
        let each = &processors[6]["for_each"];
        assert_eq!(keys(each), ["try", "catch"]);
        let read = &each[0]["try"];
        assert_eq!(
            keys(read),
            ["mapping", "http", "mapping", "unarchive", "mapping"]
        );
        assert_eq!(read[1]["http"]["headers"]["Range"], "${! @range }");
        assert_eq!(read[3]["unarchive"]["format"], "zip");
        assert!(!rendered.to_string().contains("inflate.wasm"));
        assert_eq!(processors[7]["label"], COMPUTE_LABEL);
    }

    /// T-6451: a feed split by a second mapping step. Both mappings kept one label and the runner
    /// refused the stream by lint, after jc_pipeline_test had passed it.
    #[test]
    fn two_compute_steps_each_keep_a_label_of_their_own() {
        let spec: PipelineSpec = serde_json::from_value(serde_json::json!({
            "class": "scheduled",
            "schedule": "0 * * * *",
            "sources": [{ "dataSourceRef": "gbfs" }],
            "steps": [
                { "kind": "bloblang", "bloblang": "root = this.data.stations" },
                { "processor": { "unarchive": { "format": "json_array" } } },
                { "kind": "bloblang", "bloblang": "root.id = this.station_id" }
            ],
            "outputs": [{ "mode": "upsert", "type": "Station",
                          "targetEndpoint": "urn:ngsi-ld:Endpoint:stations" }]
        }))
        .expect("spec");
        assert!(is_merged(&spec));
        let sources = vec![ResolvedSource::Data {
            spec: Box::new(helsinki_datasource_spec()),
            name: "gbfs".to_owned(),
        }];
        let mut stream =
            render_merged(&spec, "p", "helsinki", &sources, &["slug".to_owned()]).expect("renders");
        crate::pipeline_samples::with_taps(&mut stream, "http://portal", "p");
        let labels: Vec<&str> = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .filter_map(|processor| processor["label"].as_str())
            .collect();
        for label in [
            "compute",
            "tap_compute",
            "step_0",
            "compute_1",
            "tap_compute_1",
        ] {
            assert_eq!(
                labels.iter().filter(|l| **l == label).count(),
                1,
                "{label}: {labels:?}"
            );
        }
    }

    #[test]
    fn runner_nats_source_renders_verbatim_and_refuses_scheduled_class() {
        let mut pipe = helsinki_pipeline_spec();
        pipe.class = jc_core::kinds::PipelineClass::Resident;
        pipe.period = None;

        let ds: DataSourceSpec = serde_json::from_value(serde_json::json!({
            "type": "nats",
            "input": {
                "urls": ["nats://nats.helsinki.fi:4222"],
                "subject": "city.bikes.updates"
            }
        }))
        .expect("valid runner DataSourceSpec");

        let rendered = render_stream(
            &pipe,
            "bikes-stream",
            "helsinki",
            &ds,
            "city-nats",
            "abc123456789",
            None,
        )
        .expect("renders nats stream");

        assert_eq!(
            rendered["input"]["nats"]["urls"][0],
            "nats://nats.helsinki.fi:4222"
        );
        assert_eq!(rendered["input"]["nats"]["subject"], "city.bikes.updates");
        assert_eq!(
            upsert_at(&rendered, "write_0")["url"],
            "${JC_GATEWAY_URL}/api/endpoint/abc123456789/ngsi-ld/v1/entityOperations/upsert?options=update"
        );

        let mut sched_pipe = pipe;
        sched_pipe.class = jc_core::kinds::PipelineClass::Scheduled;
        sched_pipe.schedule = Some("*/10 * * * *".to_string());

        let err = render_stream(
            &sched_pipe,
            "bikes-stream",
            "helsinki",
            &ds,
            "city-nats",
            "abc123456789",
            None,
        )
        .unwrap_err();

        assert!(
            matches!(err, RenderError::Class(_)),
            "expected RenderError::Class, got: {err:?}"
        );
    }

    /// T-1125, PL-24: every component of a rendered stream carries a `label`, because Bento
    /// reports its counters under one and an unlabelled stream can only be read as a total.
    #[test]
    fn every_component_of_a_rendered_stream_is_labelled() {
        let rendered = render_stream(
            &helsinki_pipeline_spec(),
            "p",
            "helsinki",
            &helsinki_datasource_spec(),
            "src",
            "abc123",
            None,
        )
        .expect("renders");
        assert_eq!(rendered["input"]["label"], serde_json::json!("input"));
        assert_eq!(rendered["output"]["label"], serde_json::json!("output"));
        // The platform's own processors by their place in the stream; the compute step by
        // its place in the manifest, the studio's `compute` node (PL-66).
        for (at, processor) in rendered["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .enumerate()
        {
            let compute = first_compute(&helsinki_pipeline_spec()).and_then(|c| c.bloblang);
            let expected = if processor["mapping"].as_str() == compute.as_deref() {
                COMPUTE_LABEL.to_owned()
            } else if processor.get("branch").is_some() {
                "write_0".to_owned()
            } else if processor.get("switch").is_some() {
                "write_refused".to_owned()
            } else {
                format!("processor_{at}")
            };
            assert_eq!(
                processor["label"],
                serde_json::json!(expected),
                "processor {at}: {processor}"
            );
        }
    }

    /// A label the author wrote is theirs: the metrics they read elsewhere are named by it.
    #[test]
    fn a_label_the_author_wrote_is_kept() {
        let mine = serde_json::json!({ "label": "mine", "mutation": "root = this" });
        assert_eq!(labelled(mine.clone(), "processor_0"), mine);
        // Anything that cannot carry the key comes back as it came.
        assert_eq!(
            labelled(serde_json::json!("plain"), "input"),
            serde_json::json!("plain")
        );
    }

    /// A `resource:` reference gets no label: Bento's lint refuses one there and the runner
    /// refuses the stream (dev 2026-10-26, every events pipeline naming `events_tail`).
    #[test]
    fn a_resource_reference_stays_unlabelled() {
        let tail = serde_json::json!({ "resource": "events_tail" });
        assert_eq!(labelled(tail.clone(), "processor_3"), tail);
    }
    // --- v1alpha2: sources, steps, outputs (PL-52…PL-54, T-1468) ---

    fn second_shape(spec: serde_json::Value) -> PipelineSpec {
        serde_json::from_value(spec).expect("valid v1alpha2 PipelineSpec")
    }

    #[test]
    fn a_second_version_pipeline_of_one_source_step_and_output_renders_the_first_versions_bytes() {
        let first = helsinki_pipeline_spec();
        let second = second_shape(serde_json::json!({
            "class": "auto",
            "period": "60s",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } }],
            "steps": [{ "kind": "bloblang", "bloblang": "root = this.data.bikes" }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all" }]
        }));
        assert!(
            !is_merged(&second),
            "one of each takes the first version's renderer"
        );
        let ds = helsinki_datasource_spec();
        let render = |spec: &PipelineSpec| {
            serde_json::to_string(
                &render_stream(
                    spec,
                    "citybikes-free",
                    "helsinki",
                    &ds,
                    "hsl-citybikes-free",
                    "slug1",
                    None,
                )
                .expect("renders"),
            )
            .expect("serializes")
        };
        assert_eq!(render(&first), render(&second));
    }

    #[test]
    fn a_datacube_step_renders_the_stream_its_copied_bloblang_ran() {
        let datacube = serde_json::json!({
            "wholeArea": "SK0",
            "datasetPage": "https://opendata.example.org/dataset/susr-{cube}",
            "cubes": { "vh3001rr": {
                "source": "https://data.statistics.sk/api/v2/dataset/vh3001rr/all/all/all?lang=sk&type=json",
                "update": "2025-10-31",
                "period": "vh3001rr_rok",
                "area": "nuts14",
                "indicator": "vh3001rr_ukaz"
            } }
        });
        let step: jc_core::datacube::DatacubeStep =
            serde_json::from_value(datacube.clone()).expect("valid datacube step");
        let copied = jc_core::datacube::bloblang(&step).expect("renders");
        let shaped = |step: serde_json::Value| {
            second_shape(serde_json::json!({
                "class": "auto",
                "period": "24h",
                "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } }],
                "steps": [step],
                "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all" }]
            }))
        };
        let connector = shaped(serde_json::json!({ "kind": "datacube", "datacube": datacube }));
        let bloblang = shaped(serde_json::json!({ "kind": "bloblang", "bloblang": copied }));
        assert!(eligible(&connector), "a datacube step runs in the stream");
        let ds = helsinki_datasource_spec();
        let render = |spec: &PipelineSpec| {
            serde_json::to_string(
                &render_stream(
                    spec,
                    "susr-vh",
                    "slovensko",
                    &ds,
                    "hsl-citybikes-free",
                    "slug1",
                    None,
                )
                .expect("renders"),
            )
            .expect("serializes")
        };
        assert_eq!(render(&connector), render(&bloblang));
        assert!(render(&connector).contains("KeyPerformanceIndicator"));
    }

    #[test]
    fn a_datacube_step_whose_parameters_do_not_check_is_not_rendered() {
        let spec = second_shape(serde_json::json!({
            "class": "auto",
            "period": "24h",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } }],
            "steps": [{ "kind": "datacube", "datacube": {
                "wholeArea": "SK0\" } else { env(\"JC_TOKEN\")",
                "datasetPage": "https://opendata.example.org/dataset/susr-{cube}",
                "cubes": { "vh3001rr": { "source": "https://data.statistics.sk/x", "period": "vh3001rr_rok" } }
            } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all" }]
        }));
        let ds = helsinki_datasource_spec();
        let err = render_stream(
            &spec,
            "susr-vh",
            "slovensko",
            &ds,
            "hsl-citybikes-free",
            "slug1",
            None,
        )
        .expect_err("refused");
        assert!(err.to_string().contains("wholeArea"), "{err}");
    }

    fn routed() -> PipelineSpec {
        second_shape(serde_json::json!({
            "class": "resident",
            "period": "30s",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "gbfs" } }],
            "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
            "routes": [
                { "name": "hot", "when": "this.t > 30",
                  "steps": [{ "kind": "bloblang", "bloblang": "root = this.merge({\"alert\": true})" }],
                  "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:alerts:hot" }] },
                { "name": "frost-warning", "when": "this.t < 0 # below zero",
                  "outputs": [
                      { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:alerts:cold" },
                      { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:kpi:cold-kpi" }
                  ] }
            ],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:ops" }]
        }))
    }

    fn render_routed(spec: &PipelineSpec) -> Value {
        let sources = vec![ResolvedSource::Data {
            spec: Box::new(helsinki_datasource_spec()),
            name: "gbfs".to_owned(),
        }];
        let slugs: Vec<String> = ["opsslug", "hotslug", "coldslug", "coldkpislug"]
            .map(str::to_owned)
            .into();
        render_merged(spec, "air-routed", "helsinki", &sources, &slugs).expect("renders")
    }

    /// Every label of a rendered stream, at any depth.
    fn labels(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                if let Some(label) = fields.get("label").and_then(Value::as_str) {
                    into.push(label.to_owned());
                }
                fields.values().for_each(|field| labels(field, into));
            }
            Value::Array(items) => items.iter().for_each(|item| labels(item, into)),
            _ => {}
        }
    }

    /// PL-79: each output is written in its own mode: `replace` posts the upsert without
    /// options, so the entity is exactly the record; `upsert` and `update-attrs` merge.
    #[test]
    fn each_output_writes_in_its_own_mode() {
        let spec = second_shape(serde_json::json!({
            "class": "resident",
            "period": "30s",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "gbfs" } }],
            "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
            "routes": [
                { "name": "hot", "when": "this.t > 30",
                  "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:alerts:hot" }] },
                { "name": "frost-warning", "when": "this.t < 0",
                  "outputs": [
                      { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:alerts:cold", "mode": "replace" },
                      { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:kpi:cold-kpi", "mode": "update-attrs" }
                  ] }
            ],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:ops", "mode": "replace" }]
        }));
        let stream = render_routed(&spec);
        let url = |label: &str| {
            upsert_at(&stream, label)["url"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_default()
        };
        let merged = "/ngsi-ld/v1/entityOperations/upsert?options=update";
        let replaced = "/ngsi-ld/v1/entityOperations/upsert";
        assert!(
            url("write_0").ends_with(&format!("/opsslug{replaced}")),
            "{}",
            url("write_0")
        );
        assert!(url("route_0_write_0").ends_with(&format!("/hotslug{merged}")));
        assert!(url("route_1_write_0").ends_with(&format!("/coldslug{replaced}")));
        assert!(url("route_1_write_1").ends_with(&format!("/coldkpislug{merged}")));
        // A v1alpha1 pipeline with no mode keeps the merge it always had.
        let plain = endpoint_pipeline_spec(Some("T"), vec![], Some("15m"), None);
        let rendered =
            render_endpoint_stream(&plain, "helsinki", "helsinki/kpi", "src", true, "dst")
                .expect("renders");
        assert!(upsert_at(&rendered, "write_0")["url"]
            .as_str()
            .is_some_and(|u| u.ends_with(&format!("/dst{merged}"))));
        // T-5824: the run queue runs a changed stream at once only when an output replaces.
        assert!(identity::replaces_any(&spec));
        assert!(!identity::replaces_any(&plain));
    }

    /// PL-73: each record takes the first route whose `when` holds, runs that route's steps, and
    /// is written through that route's outputs; the rest go through the default lane.
    #[test]
    fn a_write_label_is_told_from_the_refusals_and_other_labels() {
        for label in ["write_0", "write_12", "route_3_write_1"] {
            assert!(is_write_label(label), "{label}");
        }
        for label in [
            "write_refused",
            "route_0_write_refused",
            "write_",
            "rewrite_0",
            "mapping_0",
        ] {
            assert!(!is_write_label(label), "{label}");
        }
        assert_eq!(write_label("route_2_", 1), "route_2_write_1");
        assert!(is_write_label(&write_label("route_2_", 1)));
    }

    #[test]
    fn routes_pick_a_route_per_record_and_write_through_its_own_outputs() {
        let spec = routed();
        assert!(is_merged(&spec) && eligible(&spec));
        let stream = render_routed(&spec);
        let processors = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let at = |label: &str| {
            processors
                .iter()
                .position(|p| p["label"] == label)
                .unwrap_or_else(|| panic!("no {label} in {processors:#?}"))
        };
        let pick = processors[at("route_pick")]["mutation"]
            .as_str()
            .expect("the pick");
        assert!(pick.starts_with("meta jc_route = if (\nthis.t > 30\n) { \"hot\" } else if ("));
        assert!(pick.contains("this.t < 0 # below zero\n) { \"frost-warning\" }"));
        assert!(pick.ends_with("{ \"\" }"));
        let steps = processors[at("route_steps")]["switch"]
            .as_array()
            .expect("the route steps");
        assert_eq!(steps.len(), 1, "a route without steps has no case");
        assert_eq!(steps[0]["check"], "meta(\"jc_route\") == \"hot\"");
        assert_eq!(steps[0]["processors"][0]["label"], "route_0_0");
        // One record per message before the pick, one route per batch before the split.
        assert!(processors[at("route_pick") - 1].get("unarchive").is_some());
        let group = at("route_steps") + 1;
        assert!(processors[group].get("group_by_value").is_some());
        let split = processors
            .iter()
            .rposition(|p| p.get("split").is_some())
            .expect("the batch split");
        assert!(group < split);

        // Each route's writes run in a `switch` after the batch split (T-4056).
        let routes = &processors[at(ROUTES_LABEL)];
        assert!(at(ROUTES_LABEL) > split);
        let cases = routes["switch"].as_array().expect("cases");
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0]["processors"][0]["label"], "route_0_write_0");
        assert!(upsert_at(&stream, "route_0_write_0")["url"]
            .as_str()
            .is_some_and(|url| url.contains("/hotslug/")));
        assert_eq!(cases[1]["check"], "meta(\"jc_route\") == \"frost-warning\"");
        assert_eq!(
            cases[1]["processors"].as_array().map(Vec::len),
            Some(3),
            "two writes and their refusals"
        );
        assert!(
            cases[2].get("check").is_none(),
            "the default lane takes the rest"
        );
        assert_eq!(cases[2]["processors"][0]["label"], "write_0");
        assert!(upsert_at(&stream, "write_0")["url"]
            .as_str()
            .is_some_and(|url| url.contains("/opsslug/")));
        assert_eq!(stream["output"], labelled(written(), "output"));

        // Bento refuses a repeated label and one outside [a-z0-9_].
        let mut all = Vec::new();
        labels(&stream, &mut all);
        let unique: std::collections::BTreeSet<&String> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "{all:?}");
        assert!(all.iter().all(|label| label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')));
    }

    #[test]
    fn a_pipeline_without_routes_renders_as_before_and_a_route_step_of_wasm_is_not_a_stream() {
        let mut spec = routed();
        spec.routes.clear();
        let stream = render_routed(&spec);
        assert_eq!(stream["output"], labelled(written(), "output"));
        assert!(!stream["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .any(|p| p["label"] == ROUTES_LABEL));
        assert!(!stream["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .any(|p| p.get("group_by_value").is_some()));

        let mut wasm = routed();
        wasm.routes[0].steps = vec![Step::Compute(jc_core::kinds::Compute {
            kind: ComputeKind::Wasm,
            module: Some("m".to_owned()),
            function: None,
            mapping_ref: None,
            bloblang: None,
            kpi: None,
            datacube: None,
        })];
        assert!(!eligible(&wasm));
    }

    fn kpi_spec(class: &str) -> PipelineSpec {
        second_shape(serde_json::json!({
            "class": class,
            "schedule": "0 * * * *",
            "sources": [
                { "name": "parking", "endpointRef": { "kind": "Endpoint", "name": "helsinki-parking" },
                  "query": { "type": "ParkingSite", "attrs": ["occupiedSpotNumber", "district"] } },
                { "name": "stops", "endpointRef": { "kind": "Endpoint", "name": "helsinki-transit" },
                  "query": { "type": "TransitStop", "attrs": ["district"] } }
            ],
            "steps": [{ "kind": "kpi", "kpi": {
                "name": "parking-per-stop",
                "formula": "sum(parking.occupiedSpotNumber) / count(stops)",
                "unitCode": "C62",
                "join": { "by": "area", "keys": { "parking": "district", "stops": "district" } }
            } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki-kpi:kpi" }]
        }))
    }

    fn kpi_sources(spec: &PipelineSpec) -> Vec<ResolvedSource> {
        spec.sources()
            .into_iter()
            .map(|source| ResolvedSource::Endpoint {
                source: Box::new(source),
                slug: "slug".to_owned(),
                public: false,
            })
            .collect()
    }

    /// PL-117: a change over one source is the general renderer's: a second input reads the
    /// Temporal API with the pipeline's token, its page named apart, no rows meaning no history,
    /// and the batch waits for both pages.
    #[test]
    fn a_read_back_is_one_more_input_on_the_temporal_api() {
        let mut spec: Value = serde_json::to_value(kpi_spec("scheduled")).expect("a value");
        spec["sources"].as_array_mut().expect("sources").truncate(1);
        spec["sources"][0]["query"]["q"] = serde_json::json!("occupiedSpotNumber>0");
        spec["steps"][0]["kpi"]["formula"] = serde_json::json!(
            "sum(parking.occupiedSpotNumber) - prev(sum(parking.occupiedSpotNumber), P7D)"
        );
        spec["steps"][0]["kpi"]["join"] = serde_json::json!({ "by": "time" });
        let spec: PipelineSpec = serde_json::from_value(spec).expect("a spec");
        assert!(is_merged(&spec) && eligible(&spec));
        let stream = render_merged(
            &spec,
            "parking-change",
            "helsinki",
            &kpi_sources(&spec),
            &["kpislug".to_owned()],
        )
        .expect("renders");
        let broker = &stream["input"]["broker"];
        assert_eq!(broker["batching"]["count"], 2);
        assert_eq!(broker["inputs"].as_array().expect("inputs").len(), 2);
        let earlier = broker["inputs"][1].to_string();
        assert!(earlier.contains("/api/endpoint/slug/ngsi-ld/v1/temporal/entities?type=ParkingSite&attrs=occupiedSpotNumber"), "{earlier}");
        assert!(earlier.contains("q=occupiedSpotNumber%3E0"), "{earlier}");
        assert!(
            earlier.contains("&timerel=before&lastN=1&timeAt=${! now().ts_sub_iso8601("),
            "{earlier}"
        );
        assert!(earlier.contains("oauth2"), "{earlier}");
        assert!(
            earlier.contains("it has no history 7 days back"),
            "{earlier}"
        );
        assert!(earlier.contains("parking@P7D"), "{earlier}");
        let current = broker["inputs"][0].to_string();
        assert!(!current.contains("temporal"), "{current}");
        assert!(stream["pipeline"].to_string().contains("was_parking_p7d"));
    }

    /// PL-104 (T-4864): each source keeps its read's error and is wrapped with its name, the
    /// broker folds one page per source into one message, jc-core's mapping computes, and a
    /// problem is reported to the run log instead of written.
    #[test]
    fn a_kpi_step_gathers_one_page_per_source_and_reports_a_problem_instead_of_writing() {
        let spec = kpi_spec("scheduled");
        assert!(is_merged(&spec));
        assert!(eligible(&spec));
        let mut stream = render_merged(
            &spec,
            "parking-per-stop",
            "helsinki",
            &kpi_sources(&spec),
            &["kpislug".to_owned()],
        )
        .expect("renders");

        let broker = &stream["input"]["broker"];
        assert_eq!(broker["batching"]["count"], 2);
        assert_eq!(
            broker["batching"]["processors"][0]["archive"]["format"],
            "json_array"
        );
        for (input, name) in broker["inputs"]
            .as_array()
            .expect("inputs")
            .iter()
            .zip(["parking", "stops"])
        {
            let processors = input["processors"].as_array().expect("processors");
            assert!(
                processors.iter().all(|p| p["mutation"] != DROP_ERRORED),
                "a failed read is kept as the run's problem, not dropped"
            );
            assert!(processors.iter().any(|p| p["catch"][0]["mapping"]
                .as_str()
                .is_some_and(|m| m.contains("jc_kpi_error"))));
            assert!(processors.iter().any(|p| p["mapping"]
                .as_str()
                .is_some_and(|m| m.contains(render::EMPTY_KPI_SOURCE))));
            let wrap = processors.last().expect("the wrap")["mapping"]
                .as_str()
                .expect("mapping");
            assert!(wrap.contains(&format!("\"input\": \"{name}\"")), "{wrap}");
        }

        let processors = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let compute = processors
            .iter()
            .find(|p| p["label"] == COMPUTE_LABEL)
            .expect("the compute step");
        let mapping = compute["mapping"].as_str().expect("mapping");
        assert!(
            mapping.contains("division by zero: count(stops) is 0"),
            "{mapping}"
        );
        assert!(
            mapping.contains("urn:ngsi-ld:Pipeline:\" + $domain + \":helsinki:parking-per-stop")
        );
        let at = |label: &str| processors.iter().position(|p| p["label"] == label);
        assert_eq!(at(KPI_PROBLEM_LABEL), at(COMPUTE_LABEL).map(|c| c + 1));

        crate::pipeline_log::with_problems(
            &mut stream,
            KPI_PROBLEM_LABEL,
            "http://portal/outcomes",
        );
        let problem = stream["pipeline"]["processors"]
            .as_array()
            .and_then(|p| p.iter().find(|p| p["label"] == KPI_PROBLEM_LABEL))
            .expect("still labelled");
        assert_eq!(problem["switch"][0]["check"], "errored()");
        assert!(problem.to_string().contains("http://portal/outcomes"));
    }

    #[test]
    fn a_kpi_step_runs_only_on_its_schedule_and_over_endpoints() {
        let resident = kpi_spec("resident");
        let refused = render_merged(
            &resident,
            "p",
            "helsinki",
            &kpi_sources(&resident),
            &["s".to_owned()],
        );
        assert!(refused.is_err_and(|e| e.to_string().contains("runs on its schedule")));

        let spec = kpi_spec("scheduled");
        let mut sources = kpi_sources(&spec);
        sources[1] = ResolvedSource::Data {
            spec: Box::new(helsinki_datasource_spec()),
            name: "gbfs".to_owned(),
        };
        let refused = render_merged(&spec, "p", "helsinki", &sources, &["s".to_owned()]);
        assert!(refused.is_err_and(|e| e.to_string().contains("reads Endpoints")));
    }

    #[test]
    fn two_sources_merge_through_a_broker_and_two_outputs_fan_out() {
        let spec = second_shape(serde_json::json!({
            "class": "resident",
            "period": "30s",
            "sources": [
                { "dataSourceRef": { "kind": "DataSource", "name": "gbfs" } },
                { "endpointRef": { "kind": "Endpoint", "name": "legacy" },
                  "query": { "type": "BikeHireDockingStation" } }
            ],
            "steps": [
                { "kind": "bloblang", "bloblang": "root = this" },
                { "processor": { "dedupe": { "cache": "pipeline_changes", "key": "${! json(\"id\") }" } } }
            ],
            "outputs": [
                { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:ops" },
                { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki-kpi:kpi" }
            ]
        }));
        assert!(is_merged(&spec));
        assert!(eligible(&spec));
        let sources = vec![
            ResolvedSource::Data {
                spec: Box::new(helsinki_datasource_spec()),
                name: "gbfs".to_owned(),
            },
            ResolvedSource::Endpoint {
                source: Box::new(spec.sources()[1].clone()),
                slug: "legacyslug".to_owned(),
                public: false,
            },
        ];
        let stream = render_merged(
            &spec,
            "bikes-merged",
            "helsinki",
            &sources,
            &["opsslug".to_owned(), "kpislug".to_owned()],
        )
        .expect("renders");

        let inputs = stream["input"]["broker"]["inputs"]
            .as_array()
            .expect("a broker input");
        assert_eq!(inputs.len(), 2);
        // Each input carries the processors its own input needs, the http poll among them.
        assert!(inputs[0]["generate"].is_object());
        assert!(inputs[0]["processors"][0]["try"][0]["http"]["url"]
            .as_str()
            .is_some_and(|url| url.contains("gbfs")));
        let read = &inputs[1]["processors"][0]["try"][0]["http"];
        assert!(read["url"]
            .as_str()
            .is_some_and(|url| url.contains("/api/endpoint/legacyslug/")));
        assert_eq!(
            read["oauth2"]["enabled"], true,
            "a non-public source is read with the runner's token"
        );

        let processors = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert_eq!(processors[0]["mapping"], "root = this");
        assert_eq!(processors[0]["label"], COMPUTE_LABEL);
        assert_eq!(processors[1]["dedupe"]["cache"], "pipeline_changes");
        assert_eq!(
            processors[1]["label"], "step_0",
            "a processor step is labelled by its place among the manifest's processor steps (PL-66)"
        );
        assert!(processors[2]["mapping"]
            .as_str()
            .is_some_and(|m| m.contains("array")));

        // Both targets are written in turn; a failed write goes back to the input rather than
        // round the broker forever (T-2983, T-4056).
        assert_eq!(
            stream["output"]["switch"]["cases"][0]["output"]["reject"],
            "${! error() }"
        );
        assert!(upsert_at(&stream, "write_0")["url"]
            .as_str()
            .is_some_and(|url| url.contains("/api/endpoint/opsslug/")));
        assert!(upsert_at(&stream, "write_1")["url"]
            .as_str()
            .is_some_and(|url| url.contains("/api/endpoint/kpislug/")));
    }

    #[test]
    fn a_processor_step_alone_renders_without_a_broker() {
        let spec = second_shape(serde_json::json!({
            "class": "resident",
            "period": "30s",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "gbfs" } }],
            "steps": [{ "processor": { "log": { "message": "seen" } } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:ops" }]
        }));
        assert!(
            is_merged(&spec),
            "a processor step takes the general renderer"
        );
        let stream = render_merged(
            &spec,
            "logged",
            "helsinki",
            &[ResolvedSource::Data {
                spec: Box::new(helsinki_datasource_spec()),
                name: "gbfs".to_owned(),
            }],
            &["opsslug".to_owned()],
        )
        .expect("renders");
        assert!(stream["input"]["broker"].is_null());
        assert!(stream["input"]["generate"].is_object());
        assert!(upsert_at(&stream, "write_0").is_object());
        assert_eq!(
            stream["pipeline"]["processors"][0]["log"]["message"],
            "seen"
        );
    }

    #[test]
    fn a_merged_pipeline_with_a_compute_the_stream_cannot_run_is_refused_and_not_eligible() {
        let spec = second_shape(serde_json::json!({
            "class": "resident",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "gbfs" } }],
            "steps": [
                { "kind": "mapping", "mappingRef": { "kind": "Mapping", "name": "m" } },
                { "processor": { "log": { "message": "x" } } }
            ],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:ops" }]
        }));
        assert!(!eligible(&spec));
        let refused = render_merged(
            &spec,
            "p",
            "helsinki",
            &[ResolvedSource::Data {
                spec: Box::new(helsinki_datasource_spec()),
                name: "gbfs".to_owned(),
            }],
            &["opsslug".to_owned()],
        );
        assert!(matches!(refused, Err(RenderError::MissingCompute)));
        let none = render_merged(&spec, "p", "helsinki", &[], &["opsslug".to_owned()]);
        assert!(matches!(none, Err(RenderError::Custom(_))));
    }

    #[tokio::test]
    async fn the_reconciler_resolves_every_source_and_output_from_the_mirror() {
        let mirror = helsinki_test_mirror();
        let pipe = serde_json::json!({
            "class": "auto",
            "period": "60s",
            "sources": [
                { "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } },
                { "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } }
            ],
            "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
            "outputs": [
                { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all" },
                { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:not-there" }
            ]
        });
        mirror.upsert(ResourceEnvelope {
            api_version: jc_core::API_VERSION_V1ALPHA2.to_string(),
            kind: "Pipeline".to_string(),
            metadata: ObjectMeta {
                name: "merged".to_string(),
                namespace: Some("helsinki".to_string()),
                ..Default::default()
            },
            spec: pipe,
            status: None,
        });
        let deployer = StreamDeployer::new("http://dummy-runner:4195");
        let outcomes = deployer
            .converge(&mirror, &Bentos::new(), &Default::default())
            .await;
        let merged = outcomes
            .iter()
            .find(|(_, name, _)| name == "merged")
            .expect("the merged pipeline is reconciled");
        match &merged.2 {
            StreamOutcome::Error(msg) => assert!(
                msg.contains("target endpoint not-there is not in the mirror"),
                "{msg}"
            ),
            other => panic!("expected the missing output named, got {other:?}"),
        }
    }

    /// The four shapes of `spec.lifetime` (PL-80) on one Bus pipeline, by golden file name.
    fn lifetimes() -> [(&'static str, Value); 4] {
        [
            (
                "entity-from",
                serde_json::json!([{ "type": "Bus", "entity": { "from": "observedAt", "plus": "PT10M" } }]),
            ),
            (
                "entity-rolling",
                serde_json::json!([{ "type": "Bus", "entity": { "rolling": "PT10M" } }]),
            ),
            (
                "entity-at",
                serde_json::json!([{ "type": "Bus", "entity": { "at": "2030-01-01T00:00:00Z" } }]),
            ),
            (
                "attributes",
                serde_json::json!([{ "type": "Bus", "attributes": [{ "names": ["position", "speed"], "rolling": "PT5M" }] }]),
            ),
        ]
    }

    fn with_lifetime(spec: &PipelineSpec, lifetime: &Value) -> PipelineSpec {
        let mut value = serde_json::to_value(spec).expect("serializes");
        value["lifetime"] = lifetime.clone();
        serde_json::from_value(value).expect("a PipelineSpec with a lifetime")
    }

    /// The processor labels of a stream's pipeline, in order.
    fn processor_labels(stream: &Value) -> Vec<String> {
        stream["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .map(|p| p["label"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// Where the lifetime step sits: right after the step before it, before the batching's
    /// first mapping; `None` when the stream has none.
    fn lifetime_at(stream: &Value) -> Option<usize> {
        stream["pipeline"]["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .position(|p| p["label"] == LIFETIME_LABEL)
    }

    #[test]
    fn each_lifetime_mode_renders_its_golden_stream() {
        let ds = helsinki_datasource_spec();
        for (mode, lifetime) in lifetimes() {
            let spec = with_lifetime(&helsinki_pipeline_spec(), &lifetime);
            let stream = render_stream(
                &spec,
                "buses",
                "helsinki",
                &ds,
                "hsl-citybikes-free",
                "busslug",
                None,
            )
            .expect("renders");
            let yaml = serde_norway::to_string(&stream).expect("yaml");
            let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/lifetime/{mode}.yaml"));
            if std::env::var_os("JC_UPDATE_GOLDEN").is_some() {
                std::fs::create_dir_all(golden.parent().expect("dir")).expect("dir");
                std::fs::write(&golden, &yaml).expect("golden written");
            }
            let expected = std::fs::read_to_string(&golden).unwrap_or_else(|_| {
                panic!("{} missing: run with JC_UPDATE_GOLDEN=1", golden.display())
            });
            assert_eq!(
                yaml, expected,
                "{mode}: the rendered stream moved from its golden file"
            );
            // The generated step itself, right after the author's compute step.
            let labels = processor_labels(&stream);
            let at = lifetime_at(&stream).expect("a lifetime step");
            assert_eq!(labels[at - 1], COMPUTE_LABEL, "{mode}: {labels:?}");
            assert_eq!(
                stream["pipeline"]["processors"][at]["mapping"].as_str(),
                jcctl::lifetime::step(&spec).expect("compiles").as_deref(),
            );
        }
    }

    #[test]
    fn no_lifetime_renders_no_step_and_the_same_bytes_as_before() {
        let ds = helsinki_datasource_spec();
        let plain = helsinki_pipeline_spec();
        let stream = render_stream(
            &plain,
            "buses",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "busslug",
            None,
        )
        .expect("renders");
        assert_eq!(lifetime_at(&stream), None);
        let empty = with_lifetime(&plain, &serde_json::json!([]));
        let again = render_stream(
            &empty,
            "buses",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "busslug",
            None,
        )
        .expect("renders");
        assert_eq!(stream, again);
    }

    #[test]
    fn the_validation_stage_checks_what_the_lifetime_step_wrote() {
        let ds = helsinki_datasource_spec();
        let spec = with_lifetime(&helsinki_pipeline_spec(), &lifetimes()[1].1);
        let mut stream = render_stream(
            &spec,
            "buses",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "busslug",
            None,
        )
        .expect("renders");
        let schema = crate::pipeline_validation::ModelSchema::compile(
            "transport",
            "1.0.0",
            &serde_json::json!({}),
            &["Bus".to_owned()],
            false,
        );
        assert!(crate::pipeline_validation::insert_stage(
            &mut stream,
            &schema,
            "http://portal/rejected"
        ));
        let labels = processor_labels(&stream);
        let lifetime = lifetime_at(&stream).expect("a lifetime step");
        let stage = labels
            .iter()
            .position(|l| l == "validation")
            .expect("the stage");
        assert!(lifetime < stage, "{labels:?}");
    }

    #[test]
    fn an_endpoint_and_a_routed_multi_space_stream_run_the_lifetime_after_every_author_step() {
        let lifetime = &lifetimes()[0].1;
        let spec = with_lifetime(
            &endpoint_pipeline_spec(Some("Bus"), vec![], None, None),
            lifetime,
        );
        let stream = render_endpoint_stream(&spec, "helsinki", "helsinki/kpi", "src", true, "dst")
            .expect("renders");
        let processors = stream["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        let at = lifetime_at(&stream).expect("a lifetime step");
        assert_eq!(processors[at - 1]["label"], COMPUTE_LABEL);
        assert_eq!(processors[at + 1], batching()[0]);

        // Routes write four Endpoints in three spaces: the step follows the route steps, so a
        // record a route rewrote still gets its end, and precedes every route's writes.
        let routed = with_lifetime(&routed(), lifetime);
        let stream = render_routed(&routed);
        let labels = processor_labels(&stream);
        let at = lifetime_at(&stream).expect("a lifetime step");
        let steps = labels
            .iter()
            .position(|l| l == "route_steps")
            .expect("route steps");
        let writes = labels
            .iter()
            .position(|l| l == ROUTES_LABEL)
            .expect("the routed writes");
        assert!(steps < at && at < writes, "{labels:?}");
    }

    fn with_events(spec: &PipelineSpec, deleted_when: &str) -> PipelineSpec {
        let mut value = serde_json::to_value(spec).expect("serializes");
        value["sourceDeletes"] =
            serde_json::json!({ "mode": "events", "deletedWhen": deleted_when });
        serde_json::from_value(value).expect("a PipelineSpec in events mode")
    }

    /// PL-98 (T-5360): `deletedWhen` runs after the author's steps and before the lifetime step,
    /// and the deployer posts its marks right after it; the stream matches its golden file.
    #[test]
    fn an_events_pipeline_renders_its_golden_stream_with_the_marks_before_the_lifetime() {
        let ds = helsinki_datasource_spec();
        let spec = with_events(
            &with_lifetime(&helsinki_pipeline_spec(), &lifetimes()[0].1),
            "this.status.or(\"\") == \"withdrawn\"",
        );
        let mut stream = render_stream(
            &spec,
            "buses",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "busslug",
            None,
        )
        .expect("renders");
        with_source_events(
            &mut stream,
            "http://portal-internal:8081/internal/pipelines/helsinki/buses/source-events",
        );
        let labels = processor_labels(&stream);
        let at = labels
            .iter()
            .position(|l| l == SOURCE_EVENTS_LABEL)
            .expect("the events step");
        assert_eq!(labels[at - 1], COMPUTE_LABEL, "{labels:?}");
        assert_eq!(labels[at + 1], "source_events_post", "{labels:?}");
        assert_eq!(labels[at + 2], "source_events_kept", "{labels:?}");
        assert_eq!(lifetime_at(&stream), Some(at + 3), "{labels:?}");
        let yaml = serde_norway::to_string(&stream).expect("yaml");
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/source-deletes/events.yaml");
        if std::env::var_os("JC_UPDATE_GOLDEN").is_some() {
            std::fs::write(&golden, &yaml).expect("golden written");
        }
        let expected = std::fs::read_to_string(&golden).unwrap_or_else(|_| {
            panic!("{} missing: run with JC_UPDATE_GOLDEN=1", golden.display())
        });
        assert_eq!(
            yaml, expected,
            "the events stream moved from its golden file"
        );
    }

    #[test]
    fn only_events_mode_renders_the_step_and_a_stream_without_it_gets_no_post() {
        let ds = helsinki_datasource_spec();
        let mut value = serde_json::to_value(helsinki_pipeline_spec()).expect("serializes");
        value["sourceDeletes"] = serde_json::json!({ "mode": "never" });
        let never: PipelineSpec = serde_json::from_value(value).expect("never");
        let mut stream = render_stream(
            &never,
            "buses",
            "helsinki",
            &ds,
            "hsl-citybikes-free",
            "busslug",
            None,
        )
        .expect("renders");
        let before = stream.clone();
        with_source_events(&mut stream, "http://portal/source-events");
        assert_eq!(stream, before);
        assert!(!processor_labels(&stream)
            .iter()
            .any(|l| l.starts_with("source_events")));
    }

    #[test]
    fn a_routed_events_stream_marks_after_the_route_steps_and_before_every_write() {
        let spec = with_events(&routed(), "this.gone.or(false)");
        let stream = render_routed(&spec);
        let labels = processor_labels(&stream);
        let at = labels
            .iter()
            .position(|l| l == SOURCE_EVENTS_LABEL)
            .expect("the events step");
        let steps = labels
            .iter()
            .position(|l| l == "route_steps")
            .expect("route steps");
        let writes = labels
            .iter()
            .position(|l| l == ROUTES_LABEL)
            .expect("the routed writes");
        assert!(steps < at && at < writes, "{labels:?}");
    }
}
