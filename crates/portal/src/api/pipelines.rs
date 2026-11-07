//! Runtime numbers of one Bento stream, read from the project's pipeline runner.
//!
//! The runner serves Prometheus text on its own port (PL-24). The Portal scrapes it for the
//! browser so the UI never talks to a workload directly and the runner needs no ingress; the
//! response carries counters only, never logs, configuration or secret values (PL-17).

use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// How long the runner has to answer. A metrics port that is slow is a runner that is busy
/// ingesting; the view would rather show the pipeline without numbers than hang on it.
const SCRAPE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Default, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineMetrics {
    pub pipeline: String,
    /// When the Portal read the runner, RFC 3339. The counters are as old as this instant.
    pub scraped_at: String,
    /// Cumulative since the runner started, exactly as it reports them: a rate is the view's
    /// job, history is Prometheus' job (OPS-16). An absent field is not zero, it is a counter
    /// this runner does not export.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<u64>,
    /// Records the validation stage refused since the runner started (PL-61): counted apart
    /// from `errors`, because a refused record is the stage working, not the stream failing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_depth: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_p99_ms: Option<f64>,
    /// The same counters per component of the stream, by the Bento `label` the reconciler
    /// wrote (T-1125): the studio paints a node with the numbers of its own label. Empty for a
    /// stream deployed before the labels, whose samples carry none.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub nodes: std::collections::BTreeMap<String, NodeCounters>,
}

/// One sample line of the Prometheus text exposition format.
pub(crate) struct Sample<'a> {
    pub name: &'a str,
    pub labels: &'a str,
    pub value: f64,
}

pub(crate) fn parse_line(line: &str) -> Option<Sample<'_>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (name, labels, rest) = match line.split_once('{') {
        Some((name, tail)) => {
            let (labels, rest) = tail.split_once('}')?;
            (name, labels, rest)
        }
        None => {
            let (name, rest) = line.split_once(' ')?;
            (name, "", rest)
        }
    };
    let value = rest.split_whitespace().next()?.parse().ok()?;
    Some(Sample {
        name: name.trim(),
        labels,
        value,
    })
}

/// Reads one label. Bento's label values are stream names, metric paths and quantiles, so the
/// escaping rules of the exposition format never come up; a value with an escaped quote in it
/// would simply not match.
pub(crate) fn label<'a>(labels: &'a str, key: &str) -> Option<&'a str> {
    labels.split(',').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"'))
    })
}

/// The counter a metric name stands for. Bento is a Benthos fork: the names keep the
/// `input_received` / `output_sent` shape and a deployment may prefix them (`bento_`) or
/// suffix a counter with `_total`, so a family is matched by suffix, never by equality.
fn family(name: &str) -> Option<Family> {
    let name = name.strip_suffix("_total").unwrap_or(name);
    if name.ends_with("input_received") {
        Some(Family::Received)
    } else if name.ends_with("output_sent") {
        Some(Family::Sent)
    } else if name.ends_with("output_error") || name.ends_with("processor_error") {
        Some(Family::Errors)
    } else if name.ends_with("buffer_backlog") {
        Some(Family::BufferDepth)
    } else if name.ends_with("output_latency_ns") || name.ends_with("processor_latency_ns") {
        Some(Family::LatencyNs)
    } else if name.ends_with("output_latency_ns_bucket")
        || name.ends_with("processor_latency_ns_bucket")
    {
        Some(Family::LatencyBucket)
    } else {
        None
    }
}

enum Family {
    Received,
    Sent,
    Errors,
    BufferDepth,
    LatencyNs,
    /// The same timing as a histogram (`use_histogram_timing`, T-3625): cumulative counts per
    /// `le` bound, in seconds, because Bento converts its nanoseconds for the buckets.
    LatencyBucket,
}

/// The 0.99 quantile of cumulative `(le, count)` buckets, in milliseconds, interpolated inside
/// the bucket that crosses the rank as Prometheus' `histogram_quantile` does. None without a
/// finite bound or without a single observation.
fn histogram_p99_ms(mut buckets: Vec<(f64, f64)>) -> Option<f64> {
    buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = buckets.last()?.1;
    if total.is_nan() || total <= 0.0 || !buckets.iter().any(|(le, _)| le.is_finite()) {
        return None;
    }
    let rank = 0.99 * total;
    let (mut lower, mut below) = (0.0, 0.0);
    for (le, count) in buckets {
        if count >= rank {
            if !le.is_finite() {
                // Past the last finite bound nothing says how far: that bound is the answer.
                return Some(lower * 1000.0);
            }
            let inside = count - below;
            let seconds = if inside > 0.0 {
                lower + (le - lower) * (rank - below) / inside
            } else {
                le
            };
            return Some(seconds * 1000.0);
        }
        (lower, below) = (le, count);
    }
    None
}

/// What one component of a stream counted, as its own label reports it (T-1125).
#[derive(Debug, Default, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeCounters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<u64>,
}

/// How many outcome reports the runner dropped for one stream, cumulative over the stream id's
/// runs since the runner started (F-POR3-006): the sends of the sink's `outcome_drop` fallback.
/// `None` when the scrape has no such series, which Bento exports, at 0, once the stream is built.
pub(crate) fn dropped_reports(body: &str, stream: &str) -> Option<u64> {
    body.lines()
        .filter_map(parse_line)
        .filter(|sample| {
            matches!(family(sample.name), Some(Family::Sent))
                && label(sample.labels, "stream") == Some(stream)
                && label(sample.labels, "label") == Some(crate::pipeline_log::SINK_LABELS[6])
        })
        .map(|sample| sample.value as u64)
        .reduce(|a, b| a + b)
}

/// Folds every series of one stream into the counters the view shows. Series of the same family
/// are summed (a stream may have several inputs or outputs); the latency takes the slowest
/// output rather than a sum, which would mean nothing. Since the upsert is a processor
/// (T-4056) the latency is its writes' ([`crate::reconciler::streams::is_write_label`]); a
/// stream rendered before that still reports it on its output. `stream` is the runner's id of the
/// stream, [`crate::reconciler::streams::stream_id`]: the project-qualified id is the `stream`
/// label to select on (T-3002).
pub(crate) fn scrape(body: &str, stream: &str, scraped_at: String) -> PipelineMetrics {
    let mut metrics = PipelineMetrics {
        pipeline: stream.to_string(),
        scraped_at,
        ..PipelineMetrics::default()
    };

    // Summed over labels per bound: the stream's one distribution. Index 0 is the output's,
    // 1 the writes'; the writes win whenever they report.
    let mut buckets: [Vec<(f64, f64)>; 2] = [Vec::new(), Vec::new()];
    let mut p99: [Option<f64>; 2] = [None, None];
    for sample in body.lines().filter_map(parse_line) {
        if label(sample.labels, "stream") != Some(stream) {
            continue;
        }
        let Some(family) = family(sample.name) else {
            continue;
        };
        // The outcome sink and the pass report say what the stream did; their own sends and
        // errors are not the stream's (PL-62, T-3001).
        if label(sample.labels, "label").is_some_and(|node| {
            crate::pipeline_log::SINK_LABELS.contains(&node)
                || crate::pipeline_log::PASS_LABELS.contains(&node)
                || node.starts_with(crate::pipeline_samples::TAP_PREFIX)
        }) {
            continue;
        }
        let add = |slot: &mut Option<u64>| *slot = Some(slot.unwrap_or(0) + sample.value as u64);
        // The component's own counters, kept beside the stream's total so the studio can paint
        // the node where a message stopped rather than only the pipeline that stopped.
        if let Some(node) = label(sample.labels, "label").filter(|one| !one.is_empty()) {
            let counters = metrics.nodes.entry(node.to_owned()).or_default();
            let slot = match family {
                Family::Received => Some(&mut counters.received),
                Family::Sent => Some(&mut counters.sent),
                Family::Errors => Some(&mut counters.errors),
                _ => None,
            };
            if let Some(slot) = slot {
                *slot = Some(slot.unwrap_or(0) + sample.value as u64);
            }
        }
        let staged = label(sample.labels, "label")
            .is_some_and(|node| crate::pipeline_validation::STAGE_LABELS.contains(&node));
        match family {
            Family::Received => add(&mut metrics.received),
            Family::Sent => add(&mut metrics.sent),
            Family::Errors if staged => add(&mut metrics.rejected),
            Family::Errors => add(&mut metrics.errors),
            Family::BufferDepth => add(&mut metrics.buffer_depth),
            Family::LatencyNs | Family::LatencyBucket
                if sample.name.contains("processor_latency")
                    && !label(sample.labels, "label")
                        .is_some_and(crate::reconciler::streams::is_write_label) => {}
            Family::LatencyNs => {
                if label(sample.labels, "quantile") == Some("0.99") {
                    let ms = sample.value / 1_000_000.0;
                    let slot = &mut p99[usize::from(sample.name.contains("processor_latency"))];
                    *slot = Some(slot.map_or(ms, |seen| seen.max(ms)));
                }
            }
            Family::LatencyBucket => {
                let buckets = &mut buckets[usize::from(sample.name.contains("processor_latency"))];
                let bound = match label(sample.labels, "le") {
                    Some("+Inf") => Some(f64::INFINITY),
                    Some(le) => le.parse::<f64>().ok().filter(|le| le.is_finite()),
                    None => None,
                };
                if let Some(bound) = bound {
                    match buckets.iter_mut().find(|(le, _)| *le == bound) {
                        Some((_, count)) => *count += sample.value,
                        None => buckets.push((bound, sample.value)),
                    }
                }
            }
        }
    }
    // The writes first, then the output; the summary before the buckets, so the Portal reads
    // the runner before and after it switches.
    let [output_buckets, write_buckets] = buckets;
    metrics.latency_p99_ms = p99[1]
        .or_else(|| histogram_p99_ms(write_buckets))
        .or(p99[0])
        .or_else(|| histogram_p99_ms(output_buckets));

    metrics
}

/// The client every call to the pipeline runner goes through.
///
/// It keeps no idle connection: a pooled connection belongs to the runtime that opened it, so one
/// reused after that runtime stopped fails with "runtime dropped the dispatch task". That is every
/// test of this process, each on its own runtime, once the OS hands a later test's runner the
/// port of an earlier one's (T-3195). A call to the runner is an occasional request inside the
/// cluster, so opening its connection each time costs nothing a person sees.
pub(crate) fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(SCRAPE_TIMEOUT)
            .pool_max_idle_per_host(0)
            .build()
            .unwrap_or_default()
    })
}

/// What last crossed each node of a running stream (T-3974, API/01 §7b).
#[derive(Debug, Serialize, ToSchema)]
pub struct PipelineSamples {
    pub pipeline: String,
    /// The newest samples per node label (`input`, `step_{j}`, `compute`, `output`), newest first.
    pub nodes: std::collections::BTreeMap<String, Vec<crate::pipeline_samples::Sample>>,
    /// True when a target Endpoint hides attributes (or cannot be read): the nodes before the
    /// mapping carry the source's own field names, so only `output` is served.
    pub withheld: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/samples",
    summary = "Read the Last Messages per Step",
    description = "The newest messages the runner's taps sent from each node of a running pipeline, without what its target Endpoint hides and with every secret-list key masked. When a target hides attributes only the mapped output node is served, and `withheld` says so (T-3974).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 200, description = "The samples per node; empty before the stream ran", body = PipelineSamples),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
    )
)]
pub async fn get_samples(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<PipelineSamples>, ApiError> {
    let pipeline = readable_pipeline(&state, &user, &project, &name)?;
    // A sample shows no more than the pipeline's targets serve: what any of them hides is out.
    // `None` when a target cannot be read, which withholds as much as a target that hides.
    let hidden: Option<Vec<String>> =
        serde_json::from_value::<jc_core::kinds::pipeline::PipelineSpec>(pipeline.spec.clone())
            .ok()
            .and_then(|spec| {
                spec.outputs()
                    .into_iter()
                    .map(|output| {
                        state
                            .mirror
                            .get(&project, "Endpoint", output.target_endpoint.local_id())
                            .map(|endpoint| {
                                endpoint.spec["projection"]["hiddenAttributes"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|name| name.as_str().map(str::to_owned))
                                    .collect::<Vec<_>>()
                            })
                    })
                    .collect::<Option<Vec<_>>>()
                    .map(|lists| lists.concat())
            });
    // Before the mapping a record carries the source's own field names, so a hidden attribute
    // is not recognisable there: with anything hidden only the mapped `output` is served.
    let withheld = hidden.as_ref().is_none_or(|names| !names.is_empty());
    let hidden = hidden.unwrap_or_default();
    let nodes = state
        .pipeline_samples
        .read(&project, &name, chrono::Utc::now())
        .into_iter()
        .filter(|(node, _)| !withheld || node == "output")
        .map(|(node, samples)| {
            let samples = samples
                .into_iter()
                .map(|sample| crate::pipeline_samples::Sample {
                    at: sample.at,
                    record: crate::pipeline_samples::redacted(sample.record, &hidden),
                })
                .collect();
            (node, samples)
        })
        .collect();
    Ok(Json(PipelineSamples {
        pipeline: name,
        nodes,
        withheld,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/bento",
    summary = "Read Pipeline Mapping File",
    description = "The `bento.yaml` beside a Pipeline's manifest, as the repository holds it: the processors the runner runs when the manifest carries no compute step (PL-03, PL-41). Credentials in it are `${NAME}` references the runner fills (PL-16), never values.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 200, description = "The file in YAML", content_type = "text/yaml", body = String),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found, or it has no bento.yaml", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn get_bento(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    // Asked of the forge only once the caller may read the pipeline (T-4818).
    let pipeline = readable_pipeline(&state, &user, &project, &name)?;
    let text = read_bento(&state, &project, &pipeline).await?;
    let yaml = [(axum::http::header::CONTENT_TYPE, "text/yaml; charset=utf-8")];
    Ok((yaml, text).into_response())
}

/// The `bento.yaml` in the folder of the pipeline's manifest, on the default branch.
pub(crate) async fn read_bento(
    state: &AppState,
    project: &str,
    pipeline: &crate::resource::ResourceEnvelope,
) -> Result<String, ApiError> {
    let info = crate::resource::by_kind("Pipeline")
        .ok_or_else(|| ApiError::Internal("kind Pipeline is not registered".into()))?;
    let manifest = crate::api::mutate::resolve_repo_path(pipeline, info, project)?;
    let path = match manifest.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/bento.yaml"),
        None => "bento.yaml".to_owned(),
    };
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let branch = gitea.default_branch().await?;
    let file = gitea.get_file(&path, &branch).await?.ok_or_else(|| {
        let name = &pipeline.metadata.name;
        ApiError::NotFound(format!(
            "pipeline '{name}' has no bento.yaml beside its manifest"
        ))
    })?;
    Ok(file.content)
}
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/metrics",
    summary = "Read Pipeline Counters",
    description = "What the runner has counted for one pipeline: state, messages, errors and latency.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 200, description = "Runtime counters of the stream", body = PipelineMetrics),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 503, description = "No runner configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn get_metrics(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<PipelineMetrics>, ApiError> {
    // PF-59, T-0983: counters say a pipeline of this name runs here and how it is doing, so a
    // caller who may not read the project's pipelines is answered as if it were not there
    // rather than handed its throughput. `metrics_for` itself stays open: the catalog search
    // calls it behind its own access filter (AG-58).
    readable_pipeline(&state, &user, &project, &name)?;
    Ok(Json(metrics_for(&state, &project, &name).await?))
}

/// The runner's counters for one pipeline; the read behind the metrics route and the
/// assistant's diagnostics door (AG-57).
pub async fn metrics_for(
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<PipelineMetrics, ApiError> {
    let not_found = || ApiError::NotFound(format!("pipeline '{name}' not found in '{project}'"));
    // The names go into the runner URL, so they are checked before anything is built from them,
    // and a pipeline that is not mirrored is not disclosed as existing elsewhere (R20).
    if !is_dns1123(project) || !is_dns1123(name) {
        return Err(not_found());
    }
    state
        .mirror
        .get(project, "Pipeline", name)
        .ok_or_else(not_found)?;

    let template = state
        .config
        .pipeline_runner_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no pipeline runner is configured".into()))?;
    let url = format!(
        "{}/metrics",
        template.replace("{project}", project).trim_end_matches('/')
    );

    let response = http().get(&url).send().await.map_err(|err| {
        // The URL names an internal service; the reason is logged, the caller learns only that
        // the runner is unreachable.
        tracing::warn!(project = %project, error = %err, "pipeline runner metrics unreachable");
        ApiError::Unavailable("the pipeline runner did not answer".into())
    })?;
    if !response.status().is_success() {
        tracing::warn!(project = %project, status = %response.status(), "pipeline runner metrics refused");
        return Err(ApiError::Unavailable(
            "the pipeline runner did not answer".into(),
        ));
    }
    let body = response.text().await.map_err(|err| {
        tracing::warn!(project = %project, error = %err, "pipeline runner metrics unreadable");
        ApiError::Unavailable("the pipeline runner did not answer".into())
    })?;

    // The runner knows the stream by its project-qualified id; the answer names the pipeline.
    let mut metrics = scrape(
        &body,
        &crate::reconciler::streams::stream_id(project, name),
        now_rfc3339(),
    );
    metrics.pipeline = name.to_owned();
    Ok(metrics)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/stats",
    summary = "Read Pipeline Rates",
    description = "What each node of the pipeline received, sent and failed over the last 1, 5 and 15 minutes, from the runner's counters read at most every two seconds and kept 15 minutes (PL-68).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 200, description = "Counts per node and window", body = crate::pipeline_stats::PipelineStats),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 503, description = "No runner configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn get_stats(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<crate::pipeline_stats::PipelineStats>, ApiError> {
    readable_pipeline(&state, &user, &project, &name)?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64;
    if let Some(stats) = state.pipeline_stats.fresh(&project, &name, now) {
        return Ok(Json(stats));
    }
    // A runner that is down is 503 from here, never a body of zeros.
    let metrics = metrics_for(&state, &project, &name).await?;
    Ok(Json(state.pipeline_stats.record(
        &project,
        &name,
        now,
        metrics.scraped_at,
        metrics.nodes,
    )))
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// One page of a pipeline's rejected records.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RejectedPage {
    pub items: Vec<crate::pipeline_outcomes::Rejected>,
    /// How many the pipeline holds, at most 1000 (PL-61).
    pub total: u64,
    /// The `before` of the next page, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<i64>,
}

#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct RejectedQuery {
    /// Records per page, 1…100 (default 50).
    pub limit: Option<usize>,
    /// Only records older than this id: the `next` of the previous page.
    pub before: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/rejected",
    summary = "List Rejected Records",
    description = "The records the pipeline's validation stage refused and did not write, newest first, each with the rule it broke; secrets masked (PL-61).",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
        RejectedQuery,
    ),
    responses(
        (status = 200, description = "One page of rejected records", body = RejectedPage),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 503, description = "The list could not be read", body = ProblemDetails)
    )
)]
pub async fn get_rejected(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<RejectedQuery>,
) -> Result<Json<RejectedPage>, ApiError> {
    readable_pipeline(&state, &user, &project, &name)?;
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let unreadable = |error: sqlx::Error| {
        tracing::warn!(%project, pipeline = %name, %error, "the rejected list could not be read");
        ApiError::Unavailable("the rejected list could not be read; try again".into())
    };
    let items = state
        .rejected
        .list(&project, &name, limit, query.before)
        .await
        .map_err(unreadable)?;
    let total = state
        .rejected
        .count(&project, &name)
        .await
        .map_err(unreadable)?;
    let next = (items.len() == limit)
        .then(|| items.last().map(|r| r.id))
        .flatten();
    Ok(Json(RejectedPage { items, total, next }))
}

/// Read on the pipeline, and a caller without it is told the pipeline is not there (PF-59):
/// the pipeline when the caller may read it.
pub(crate) fn readable_pipeline(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    name: &str,
) -> Result<crate::resource::ResourceEnvelope, ApiError> {
    readable_pipeline_for(state, &user.0.identity, project, name)
}

/// [`readable_pipeline`] for a caller known by its identity, as the operation registry asks it
/// before a run is turned into a question (AG-63, T-6280).
pub(crate) fn readable_pipeline_for(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    name: &str,
) -> Result<crate::resource::ResourceEnvelope, ApiError> {
    let not_found = || {
        ApiError::NotFound(format!(
            "pipeline '{name}' not found in project '{project}'"
        ))
    };
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() || !is_dns1123(project) || !is_dns1123(name) {
        return Err(not_found());
    }
    state
        .mirror
        .get(project, "Pipeline", name)
        .filter(|p| {
            may_pipeline(
                &effective,
                state,
                project,
                &p.spec,
                jc_core::kinds::Verb::Read,
            )
        })
        .ok_or_else(not_found)
}

/// A pipeline's latest runs with their counts (PL-62).
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = PipelineRunList)]
pub struct RunList {
    /// The run with the latest line first, at most 200.
    pub items: Vec<crate::pipeline_log::Run>,
}

#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct LogQuery {
    /// Lines per page, 1…500 (default 100).
    pub limit: Option<usize>,
    /// Only lines older than this id: the `next` of the previous page.
    pub before: Option<i64>,
}

#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct RunsQuery {
    /// Runs per answer, 1…200 (default 200): the pipeline list asks for the latest one (T-4817).
    pub limit: Option<usize>,
}

/// One page of a run's log.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = PipelineLogPage)]
pub struct LogPage {
    pub items: Vec<crate::pipeline_log::LogLine>,
    /// The `before` of the next page, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/runs",
    summary = "List Pipeline Runs",
    description = "The pipeline's latest runs, each with how many records it sent, how many the model rejected and how many failed in a step (PL-62). A run is one tick of the pipeline's clock, or one UTC hour for a source that never ends.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
        RunsQuery,
    ),
    responses(
        (status = 200, description = "The runs, latest first", body = RunList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 503, description = "The runs could not be read", body = ProblemDetails)
    )
)]
pub async fn get_runs(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<RunsQuery>,
) -> Result<Json<RunList>, ApiError> {
    readable_pipeline(&state, &user, &project, &name)?;
    let limit = query.limit.unwrap_or(crate::pipeline_log::RUNS_KEPT);
    let items = state
        .pipeline_log
        .runs(&project, &name, limit)
        .await
        .map_err(|error| {
            tracing::warn!(%project, pipeline = %name, %error, "the runs could not be read");
            ApiError::Unavailable("the pipeline's runs could not be read; try again".into())
        })?;
    Ok(Json(RunList { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/runs/{run}/log",
    summary = "Read a Run's Log",
    description = "One line per record of the run, newest first: the record's id, the step it failed at, its outcome (sent, rejected, failed) and what happened (PL-62). Record ids and messages are masked where they look like a credential.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
        ("run" = String, Path, description = "The run, as the run list names it"),
        LogQuery,
    ),
    responses(
        (status = 200, description = "One page of the run's log", body = LogPage),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 503, description = "The log could not be read", body = ProblemDetails)
    )
)]
pub async fn get_run_log(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name, run)): Path<(String, String, String)>,
    axum::extract::Query(query): axum::extract::Query<LogQuery>,
) -> Result<Json<LogPage>, ApiError> {
    readable_pipeline(&state, &user, &project, &name)?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let items = state
        .pipeline_log
        .lines(&project, &name, &run, limit, query.before)
        .await
        .map_err(|error| {
            tracing::warn!(%project, pipeline = %name, %error, "a run's log could not be read");
            ApiError::Unavailable("the run's log could not be read; try again".into())
        })?;
    let next = (items.len() == limit)
        .then(|| items.last().map(|line| line.id))
        .flatten();
    Ok(Json(LogPage { items, next }))
}

/// The newest run of each pipeline the caller may read (T-5818).
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = PipelineLatestRunList)]
pub struct LatestRunList {
    /// One per pipeline that ran, by name; a pipeline that never ran is absent.
    pub items: Vec<LatestRun>,
}

/// One pipeline's newest run.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = PipelineLatestRun)]
pub struct LatestRun {
    /// The pipeline's name.
    pub pipeline: String,
    pub run: crate::pipeline_log::Run,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipeline-runs",
    summary = "List Latest Pipeline Runs",
    description = "The newest run of each pipeline of the project the caller may read, in one read for the pipeline list (T-5818, PL-62): its counts as in List Pipeline Runs. A pipeline that never ran is absent.",
    tag = "pipelines",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The newest run per pipeline, by name", body = LatestRunList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller reads no pipeline of the project", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
        (status = 503, description = "The runs could not be read", body = ProblemDetails)
    )
)]
pub(crate) async fn get_latest_runs(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<LatestRunList>, ApiError> {
    let names = pipelines_with(
        &state,
        &user,
        &project,
        jc_core::kinds::Verb::Read,
        "reading pipeline runs needs read on Pipeline in project",
    )?;
    let runs = state
        .pipeline_log
        .latest_runs(&project, &names)
        .await
        .map_err(|error| {
            tracing::warn!(%project, %error, "the latest runs could not be read");
            ApiError::Unavailable("the pipelines' runs could not be read; try again".into())
        })?;
    Ok(Json(LatestRunList {
        items: runs
            .into_iter()
            .map(|(pipeline, run)| LatestRun { pipeline, run })
            .collect(),
    }))
}

/// The health of every pipeline a steward looks after (T-5048).
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = PipelineHealthList)]
pub struct HealthList {
    /// One per pipeline, by name.
    pub items: Vec<crate::pipeline_health::Health>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipeline-health",
    summary = "List Pipeline Health",
    description = "Each pipeline the caller approves, with its health from its kept runs (T-5048, PL-62): when it last sent records, how many its newest run sent against the median of the 7 days before, how many runs failed in a row, and the newest failed or rejected line of its log with every URL's query cut off. `alert` is true after 3 failed runs in a row or a drop of more than 80 %, the same test that opens a failure or zero-output alert. `links` is the newest weekly link check of the `url` and `organizer.url` its entities carry (T-5133): links asked, answered, skipped (robots.txt or a host that is no source of the project) and the first 5 of those that failed 2 checks in a row. Needs approve on Pipeline (a steward).",
    tag = "pipelines",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The pipelines' health, by name", body = HealthList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller approves no pipeline of the project", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
        (status = 503, description = "The runs could not be read", body = ProblemDetails)
    )
)]
pub(crate) async fn get_pipeline_health(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<HealthList>, ApiError> {
    let names = pipelines_with(
        &state,
        &user,
        &project,
        jc_core::kinds::Verb::Approve,
        "the pipelines' health is for stewards: it needs approve on Pipeline in project",
    )?;
    let unavailable = |error: sqlx::Error| {
        tracing::warn!(%project, %error, "the pipelines' health could not be read");
        ApiError::Unavailable("the pipelines' runs could not be read; try again".into())
    };
    let mut items = crate::pipeline_health::of_pipelines(&state.pipeline_log, &project, &names)
        .await
        .map_err(unavailable)?;
    for item in &mut items {
        item.links = state
            .link_check
            .get(&project, &item.pipeline)
            .await
            .map_err(unavailable)?;
    }
    Ok(Json(HealthList { items }))
}

/// The pipelines of `project` the caller holds `verb` on, by name; 404 for a project it cannot
/// read, 403 (`denied` and the project) when it holds `verb` on none.
fn pipelines_with(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    verb: jc_core::kinds::Verb,
    denied: &str,
) -> Result<Vec<String>, ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if !effective.may_read_project() || !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let mut names: Vec<String> = state
        .mirror
        .matching(|env| {
            env.kind == "Pipeline" && env.metadata.namespace.as_deref() == Some(project)
        })
        .into_iter()
        .filter(|p| may_pipeline(&effective, state, project, &p.spec, verb))
        .map(|p| p.metadata.name)
        .collect();
    if names.is_empty() && !effective.may("Pipeline", verb) {
        return Err(ApiError::Denied(format!("{denied} {project}")));
    }
    names.sort();
    Ok(names)
}

/// The run a run-now queued (T-4349).
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = PipelineRunStarted)]
pub struct RunStarted {
    /// The run's id, as the run list names it once it wrote: its due time.
    pub run: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/pipelines/{name}/runs",
    summary = "Run a Pipeline Now",
    description = "Queues one run of a queued pipeline now, outside its schedule, and answers its id; it waits its turn in its lane. 409 while its run is in the runner or for a pipeline fed by pushes. Needs propose and approve on Pipeline (a steward); each run is an activity event `pipeline.run`.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 202, description = "The run is queued", body = RunStarted),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not run the pipeline", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 409, description = "A run is in progress, or the pipeline always runs", body = ProblemDetails),
        (status = 503, description = "No pipeline runner is configured", body = ProblemDetails)
    )
)]
pub async fn post_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<(axum::http::StatusCode, Json<RunStarted>), ApiError> {
    readable_pipeline(&state, &user, &project, &name)?;
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    let spec = state
        .mirror
        .get(&project, "Pipeline", &name)
        .map(|p| p.spec)
        .unwrap_or_default();
    let may = |verb| may_pipeline(&effective, &state, &project, &spec, verb);
    if !may(jc_core::kinds::Verb::Propose) || !may(jc_core::kinds::Verb::Approve) {
        return Err(ApiError::Denied(format!(
            "a run outside the schedule writes through the approved pipeline, which needs propose \
             and approve on Pipeline in project {project}"
        )));
    }
    let deployer = state
        .syncer
        .as_ref()
        .and_then(|syncer| syncer.streams())
        .ok_or_else(|| ApiError::Unavailable("no pipeline runner is configured".into()))?;
    let run = deployer
        .run_now(&project, &name)
        .map_err(|refusal| match refusal {
            crate::reconciler::run_queue::RunNow::Running(run) => ApiError::Conflict(format!(
                "pipeline '{name}' is running ({run}); start another once it ended"
            )),
            crate::reconciler::run_queue::RunNow::Held => ApiError::Conflict(format!(
                "pipeline '{name}' is held: its last run ran past its cap and the runner could \
                 not stop it; change the looping step and push again, it runs once its steps \
                 change"
            )),
            crate::reconciler::run_queue::RunNow::NotQueued => ApiError::Conflict(format!(
            "pipeline '{name}' does not run from the queue: it waits for pushes and always runs, \
             or it is not deployed (its status says why)"
        )),
        })?;
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.clone(),
        space: None,
        kind: "pipeline.run".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} started run {run} of pipeline {name}",
            user.0.identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: serde_json::json!({
            "object": format!("pipelines/{name}"),
            "run": run,
            "actor": user.0.identity.username,
        }),
    };
    if let Err(error) = state.activity.append(&[event]).await {
        tracing::warn!(%project, pipeline = %name, %error, "the run is not on the activity feed");
    }
    Ok((axum::http::StatusCode::ACCEPTED, Json(RunStarted { run })))
}

/// Which rejected records to replay.
#[derive(Debug, serde::Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryRequest {
    /// The `id`s of the records, 1…100.
    pub ids: Vec<i64>,
}

/// What a replay sent to the runner, and what it could not send.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetryAnswer {
    /// Records replayed through the pipeline's current stage and write; one that still breaks
    /// the model comes back to the list with its rule.
    pub replayed: usize,
    /// Records left on the list because a value of theirs was masked when they were kept:
    /// replaying the mask would write it. The pipeline's next read of its source brings them.
    pub masked: Vec<i64>,
}

/// The longest a replay stream is left on the runner before it is deleted.
const REPLAY_WINDOW: Duration = Duration::from_secs(15);

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/pipelines/{name}/rejected/retry",
    summary = "Retry Rejected Records",
    description = "Replays the named rejected records once through the pipeline's current validation stage and its own write (PL-61): a record that passes now is written, one that still fails comes back with its rule. Needs propose on Pipeline.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    request_body(content = RetryRequest, example = json!({ "ids": [412, 409] })),
    responses(
        (status = 202, description = "The replay was handed to the runner", body = RetryAnswer),
        (status = 400, description = "No ids, or more than 100", body = ProblemDetails),
        (status = 403, description = "The caller may read the pipeline but not propose it", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 409, description = "The pipeline's space names no model, so there is no stage to replay through", body = ProblemDetails),
        (status = 503, description = "No runner, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn retry_rejected(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(request): Json<RetryRequest>,
) -> Result<(axum::http::StatusCode, Json<RetryAnswer>), ApiError> {
    let pipeline = readable_pipeline(&state, &user, &project, &name)?;
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !may_pipeline(
        &effective,
        &state,
        &project,
        &pipeline.spec,
        jc_core::kinds::Verb::Propose,
    ) {
        return Err(ApiError::Denied(format!(
            "replaying a rejected record writes through the pipeline, which needs propose on \
             Pipeline in project {project}"
        )));
    }
    if request.ids.is_empty() || request.ids.len() > 100 {
        return Err(ApiError::BadRequest(
            "name between 1 and 100 rejected records by their id".into(),
        ));
    }
    let (space, slug) = replay_target(&state, &project, &pipeline.spec).ok_or_else(|| {
        ApiError::Conflict(format!(
            "pipeline '{name}' writes through no Endpoint this project holds"
        ))
    })?;
    let schema = state.model_schemas.get(&project, &space).ok_or_else(|| {
        ApiError::Conflict(format!(
            "space '{space}' names no data model, so there is no stage to replay through; name \
             its model in spec.dataModelRef (DM-61)"
        ))
    })?;
    let runner = state
        .config
        .pipeline_runner_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no pipeline runner is configured".into()))?
        .replace("{project}", &project)
        .trim_end_matches('/')
        .to_owned();
    let internal = state
        .config
        .pipeline_test_capture_url
        .as_deref()
        .ok_or_else(|| {
            ApiError::Unavailable("no internal listener is configured for the runner".into())
        })?;

    let unreadable = |error: sqlx::Error| {
        tracing::warn!(%project, pipeline = %name, %error, "the rejected list could not be read");
        ApiError::Unavailable("the rejected list could not be read; try again".into())
    };
    // Only what carries no mask leaves the list: the rest stays, named in the answer.
    let listed = state
        .rejected
        .list(&project, &name, crate::pipeline_outcomes::KEPT, None)
        .await
        .map_err(unreadable)?;
    let (masked, clean): (Vec<_>, Vec<_>) = listed
        .into_iter()
        .filter(|record| request.ids.contains(&record.id))
        .partition(|record| {
            record
                .record
                .to_string()
                .contains(crate::pipeline_outcomes::MASK)
        });
    let clean_ids: Vec<i64> = clean.iter().map(|record| record.id).collect();
    let records: Vec<Value> = state
        .rejected
        .take(&project, &name, &clean_ids)
        .await
        .map_err(unreadable)?
        .into_iter()
        .map(|record| record.record)
        .collect();
    let answer = RetryAnswer {
        replayed: records.len(),
        masked: masked.iter().map(|record| record.id).collect(),
    };
    if records.is_empty() {
        return Ok((axum::http::StatusCode::ACCEPTED, Json(answer)));
    }

    let sink = format!(
        "{}/internal/pipelines/{project}/{name}/rejected",
        internal.trim_end_matches('/')
    );
    let mut config = replay_stream(&records, &project, &slug);
    crate::pipeline_validation::insert_stage(&mut config, &schema, &sink);
    // What the replay writes is a line of the pipeline's log like any run's (PL-62).
    if let Some(output) = config.get_mut("output") {
        let outcomes = format!(
            "{}/internal/pipelines/{project}/{name}/outcomes",
            internal.trim_end_matches('/')
        );
        *output = crate::pipeline_log::with_outcomes(output.take(), &outcomes);
    }
    // The internal listener admits the project's own client only (T-3193, T-4038), or with
    // pipeline identity the pipeline's own, which the replay then presents everywhere (T-4044).
    crate::reconciler::streams::own_credential(&mut config, &project);
    if state.config.pipeline_identity {
        crate::reconciler::streams::with_pipeline_identity(&mut config, &project, &name);
    }
    jcctl::bento::inject_space(
        &mut config,
        &[crate::spaces::segment(&state.mirror, &project, &space)],
    );
    let stream = format!(
        "{runner}/streams/rejected-replay-{}",
        crate::agents::share::slug()
    );
    // The project's write budget the replay's upsert names, then the stream (T-5803).
    let accepted = match crate::reconciler::streams::declare_egress(http(), &runner, &project).await
    {
        Ok(None) => {
            let created = http().post(&stream).json(&config).send().await;
            matches!(&created, Ok(answer) if answer.status().is_success())
        }
        Ok(Some(why)) | Err(why) => {
            tracing::warn!(%project, pipeline = %name, %why, "the replay's write budget was not declared");
            false
        }
    };
    if !accepted {
        // Nothing was replayed: the records go back on the list as they were.
        for record in records {
            let _ = state
                .rejected
                .reject(
                    &project,
                    &name,
                    &record,
                    &crate::pipeline_outcomes::Reason {
                        rule: "runner".into(),
                        path: String::new(),
                        message: "the replay did not reach the runner; retry it".into(),
                        step: None,
                        run: None,
                    },
                )
                .await;
        }
        tracing::warn!(%project, pipeline = %name, "the runner refused a replay stream");
        return Err(ApiError::Unavailable(
            "the pipeline runner did not answer".into(),
        ));
    }
    // The stream ends after its one message; it is deleted once it has had its window.
    tokio::spawn(async move {
        tokio::time::sleep(REPLAY_WINDOW).await;
        if let Err(error) = http().delete(&stream).send().await {
            tracing::warn!(%error, "a replay stream was not deleted");
        }
    });
    Ok((axum::http::StatusCode::ACCEPTED, Json(answer)))
}

/// The space and the Endpoint slug the pipeline's first output writes through.
fn replay_target(state: &AppState, project: &str, spec: &Value) -> Option<(String, String)> {
    let spec: jc_core::kinds::pipeline::PipelineSpec = serde_json::from_value(spec.clone()).ok()?;
    let output = spec.outputs().into_iter().next()?;
    let endpoint = state
        .mirror
        .get(project, "Endpoint", output.target_endpoint.local_id())?;
    let space = crate::api::assistant::ref_name(&endpoint.spec["contextSpaceRef"])?;
    let slug = endpoint.spec.get("slug")?.as_str()?.to_owned();
    Some((space, slug))
}

/// Whether the caller may `verb` this pipeline: a grant over the project's pipelines, or one
/// bound to the space its target Endpoint writes (T-4593). A steward of one space acts on the
/// pipelines that write it and on no other.
fn may_pipeline(
    effective: &crate::permissions::Effective,
    state: &AppState,
    project: &str,
    spec: &Value,
    verb: jc_core::kinds::Verb,
) -> bool {
    effective.may("Pipeline", verb)
        || replay_target(state, project, spec)
            .is_some_and(|(space, _)| effective.may_in("Pipeline", verb, Some(&space)))
}

/// One message holding the records, split into one per record, and the pipeline's own write
/// in gateway-sized batches: the stage is put in before the split by the caller.
fn replay_stream(records: &[Value], project: &str, slug: &str) -> Value {
    use base64::Engine;
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(Value::from(records.to_vec()).to_string());
    serde_json::json!({
        "input": { "generate": {
            "count": 1,
            "interval": "",
            "mapping": format!("root = \"{encoded}\".decode(\"base64\")"),
        }},
        "pipeline": { "processors": [
            { "unarchive": { "format": "json_array" } },
            { "split": { "size": 1000 } },
            { "archive": { "format": "json_array" } },
        ]},
        "output": crate::reconciler::streams::gateway_output(project, slug),
    })
}

/// The pipeline's snapshot rule, or 409 when it has none (PL-97).
fn snapshot_of(
    pipeline: &crate::resource::ResourceEnvelope,
) -> Result<jc_core::kinds::SourceDeletes, ApiError> {
    crate::pipeline_source_deletes::snapshot_rule(&pipeline.spec).ok_or_else(|| {
        ApiError::Conflict(format!(
            "pipeline '{}' does not delete what its source drops: set spec.sourceDeletes.mode \
             to snapshot",
            pipeline.metadata.name
        ))
    })
}

fn ledger_unavailable(error: sqlx::Error) -> ApiError {
    tracing::warn!(%error, "the source ledger could not be read");
    ApiError::Unavailable("the pipeline's source ledger could not be read; try again".into())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelines/{name}/source-deletes",
    summary = "Preview Source Deletes",
    description = "The source-vs-space report and dry run of a snapshot pipeline (PL-97, T-4892): how many entities its source ledger holds, the last complete run and how many records it read, the orphans (in the space, gone at the source), the records missing from the space (read but refused), the entities it deleted with the reason, the ids one more complete run without them deletes, the ids due now, and the held run if one is held. Ids only; nothing is deleted.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 200, description = "The ledger and the dry run", body = crate::pipeline_source_deletes::Overview),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 409, description = "The pipeline is not a snapshot pipeline", body = ProblemDetails),
        (status = 503, description = "The ledger could not be read", body = ProblemDetails)
    )
)]
pub async fn get_source_deletes(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<crate::pipeline_source_deletes::Overview>, ApiError> {
    let pipeline = readable_pipeline(&state, &user, &project, &name)?;
    let rule = snapshot_of(&pipeline)?;
    let report =
        crate::pipeline_source_deletes::report(&state, &project, &name, &pipeline.spec, &rule)
            .await
            .map_err(ledger_unavailable)?;
    // Ids of a space the caller may not read stay in it; the counts are the pipeline's own.
    let space = report.space.as_deref().unwrap_or_default();
    let identity = &user.0.identity;
    let readable = crate::api::data_views::readable_space(&state, identity, &project, space);
    Ok(Json(if readable.is_ok() {
        report
    } else {
        report.without_ids()
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/source-removals",
    summary = "List Removals At The Source",
    description = "The newest entities snapshot pipelines deleted from this space because their source no longer listed them (T-4892, PL-100): per id the pipeline, the run that decided it, when the gateway confirmed the delete and the reason the tombstone carries. Ids only, at most 200.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context space name"),
    ),
    responses(
        (status = 200, description = "The removals, newest first", body = Vec<crate::pipeline_source_deletes::Removal>),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Space not found", body = ProblemDetails),
        (status = 503, description = "The ledger could not be read", body = ProblemDetails)
    )
)]
pub async fn get_source_removals(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<Vec<crate::pipeline_source_deletes::Removal>>, ApiError> {
    crate::api::data_views::readable_space(&state, &user.0.identity, &project, &space)?;
    let of = crate::pipeline_source_deletes::RemovalsOf::Space(&space);
    state
        .source_ledger
        .removals(&project, of, 200)
        .await
        .map(Json)
        .map_err(ledger_unavailable)
}

/// A person's answer to a held run (PL-97).
#[derive(Debug, serde::Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schema(as = PipelineSourceDecision)]
pub struct SourceDecision {
    /// `delete` deletes the due ids once, past the share; `keep` counts every id as read again.
    pub decision: SourceChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceChoice {
    Delete,
    Keep,
}

/// What a decision did.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = PipelineSourceDecided)]
pub struct SourceDecided {
    /// The ids handed to the runner to delete; 0 on keep.
    pub deleting: usize,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/pipelines/{name}/source-deletes",
    summary = "Release or Keep Source Deletes",
    description = "Answers a snapshot pipeline's held run (PL-97): `delete` hands the due ids to the runner once, deleted through the pipeline's own Endpoint and grant like any of its deletes; `keep` clears every miss, so nothing is deleted until complete runs miss the ids again. Needs approve on Pipeline, and each answer is an activity event `pipeline.source-delete`.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    request_body(content = SourceDecision, example = json!({ "decision": "keep" })),
    responses(
        (status = 202, description = "Done, or the deletes handed to the runner", body = SourceDecided),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not approve the pipeline", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 409, description = "Not a snapshot pipeline, or nothing is due", body = ProblemDetails),
        (status = 503, description = "The ledger or the runner did not answer", body = ProblemDetails)
    )
)]
pub async fn post_source_deletes(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(request): Json<SourceDecision>,
) -> Result<(axum::http::StatusCode, Json<SourceDecided>), ApiError> {
    let deleting = source_decision(&state, &user, &project, &name, request.decision).await?;
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(SourceDecided { deleting }),
    ))
}

/// A steward's answer to a held run, checked and put on the activity feed.
async fn source_decision(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    name: &str,
    choice: SourceChoice,
) -> Result<usize, ApiError> {
    let pipeline = readable_pipeline(state, user, project, name)?;
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if !may_pipeline(
        &effective,
        state,
        project,
        &pipeline.spec,
        jc_core::kinds::Verb::Approve,
    ) {
        return Err(ApiError::Denied(format!(
            "releasing or keeping held source deletes needs approve on Pipeline in project {project}"
        )));
    }
    let rule = snapshot_of(&pipeline)?;
    let deleting = decide(state, project, name, &rule, choice).await?;
    let actor = &user.0.identity.username;
    let summary = match choice {
        SourceChoice::Delete => {
            format!("{actor} released {deleting} source deletes of pipeline {name}")
        }
        SourceChoice::Keep => {
            format!("{actor} kept the entities pipeline {name} would have deleted")
        }
    };
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "pipeline.source-delete".to_owned(),
        source: "portal".to_owned(),
        summary,
        severity: "info".to_owned(),
        correlation_id: None,
        details: serde_json::json!({ "object": format!("pipelines/{name}"), "actor": actor }),
    };
    if let Err(error) = state.activity.append(&[event]).await {
        tracing::warn!(%project, pipeline = %name, %error, "the decision is not on the activity feed");
    }
    Ok(deleting)
}

/// Keeps every id, or hands the due ids to the runner once and clears the hold.
async fn decide(
    state: &AppState,
    project: &str,
    name: &str,
    rule: &jc_core::kinds::SourceDeletes,
    choice: SourceChoice,
) -> Result<usize, ApiError> {
    let ledger = &state.source_ledger;
    if choice == SourceChoice::Keep {
        ledger
            .keep(project, name)
            .await
            .map_err(ledger_unavailable)?;
        return Ok(0);
    }
    let (rows, hold) = ledger
        .book(project, name)
        .await
        .map_err(ledger_unavailable)?;
    let due = crate::pipeline_source_deletes::doomed(&rows, rule);
    if due.is_empty() {
        return Err(ApiError::Conflict(format!(
            "pipeline '{name}' has no entity due for deletion"
        )));
    }
    let run = hold.map_or_else(|| "released".to_owned(), |hold| hold.run);
    let deleting = crate::pipeline_source_deletes::launch(state, project, name, &run, &due)
        .await
        .map_err(|reason| {
            ApiError::Unavailable(format!("the deletes were not started: {reason}"))
        })?;
    ledger
        .settle(project, name, &[], None)
        .await
        .map_err(ledger_unavailable)?;
    Ok(deleting)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/pipelines/{name}/source-deletes",
            get(get_source_deletes).post(post_source_deletes),
        )
        .route(
            "/projects/{project}/spaces/{space}/source-removals",
            get(get_source_removals),
        )
        .route(
            "/projects/{project}/pipelines/{name}/rejected/retry",
            axum::routing::post(retry_rejected),
        )
        .route(
            "/projects/{project}/pipelines/{name}/runs",
            get(get_runs).post(post_run),
        )
        .route(
            "/projects/{project}/pipelines/{name}/runs/{run}/log",
            get(get_run_log),
        )
        .route(
            "/projects/{project}/pipelines/{name}/metrics",
            get(get_metrics),
        )
        .route(
            "/projects/{project}/pipelines/{name}/samples",
            get(get_samples),
        )
        .route("/projects/{project}/pipelines/{name}/bento", get(get_bento))
        .route(
            "/projects/{project}/pipelines/{name}/rejected",
            get(get_rejected),
        )
        .route("/projects/{project}/pipelines/{name}/stats", get(get_stats))
        .route("/projects/{project}/pipeline-runs", get(get_latest_runs))
        .route(
            "/projects/{project}/pipeline-health",
            get(get_pipeline_health),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"
# HELP input_received Benthos Counter metric
# TYPE input_received counter
input_received{label="mqtt",path="root.input",stream="aq-mqtt-ingest"} 128401
input_received{label="mqtt",path="root.input",stream="other-stream"} 77
output_sent{label="gateway",path="root.output",stream="aq-mqtt-ingest"} 128390
output_error{label="gateway",path="root.output",stream="aq-mqtt-ingest"} 2
processor_error{label="map",path="root.pipeline.processors.0",stream="aq-mqtt-ingest"} 1
buffer_backlog{stream="aq-mqtt-ingest"} 11
output_latency_ns{stream="aq-mqtt-ingest",quantile="0.5"} 1000000
output_latency_ns{stream="aq-mqtt-ingest",quantile="0.99"} 42500000
output_latency_ns_count{stream="aq-mqtt-ingest"} 128390
uptime_seconds 900
"#;

    fn scraped(stream: &str) -> PipelineMetrics {
        scrape(BODY, stream, "2026-09-06T16:20:11Z".into())
    }

    #[test]
    fn folds_the_series_of_one_stream() {
        let metrics = scraped("aq-mqtt-ingest");
        assert_eq!(
            metrics.received,
            Some(128401),
            "the neighbour's 77 is not ours"
        );
        assert_eq!(metrics.sent, Some(128390));
        assert_eq!(
            metrics.errors,
            Some(3),
            "output and processor errors are one number"
        );
        assert_eq!(metrics.buffer_depth, Some(11));
        assert_eq!(
            metrics.latency_p99_ms,
            Some(42.5),
            "nanoseconds are shown as milliseconds"
        );
    }

    /// PL-61: what the validation stage refused is counted as rejected, not as an error.
    #[test]
    fn the_stages_refusals_are_rejected_and_not_errors() {
        let body = concat!(
            "output_sent{label=\"output\",stream=\"aq\"} 90\n",
            "processor_error{label=\"validation\",stream=\"aq\"} 7\n",
            "processor_error{label=\"validation_id\",stream=\"aq\"} 2\n",
            "processor_error{label=\"processor_0\",stream=\"aq\"} 1\n",
            "output_error{label=\"output\",stream=\"aq\"} 3\n",
            // PL-62: the outcome sink's own sends and failures are not the stream's.
            "output_sent{label=\"outcome\",stream=\"aq\"} 90\n",
            "output_error{label=\"outcome\",stream=\"aq\"} 5\n",
            // T-4036: and neither are its parts' (the report, its batching steps, the drop).
            "output_sent{label=\"outcome_post\",stream=\"aq\"} 2\n",
            "output_error{label=\"outcome_post\",stream=\"aq\"} 1\n",
            "processor_error{label=\"outcome_report\",stream=\"aq\"} 1\n",
            "output_sent{label=\"outcome_drop\",stream=\"aq\"} 1\n",
        );
        let metrics = scrape(body, "aq", "2026-09-25T10:00:00Z".into());
        assert_eq!(metrics.sent, Some(90), "written");
        assert!(!metrics.nodes.contains_key("outcome"));
        assert_eq!(metrics.rejected, Some(9));
        assert_eq!(metrics.errors, Some(4), "a step's error and a failed write");
        assert_eq!(
            scraped("aq-mqtt-ingest").rejected,
            None,
            "no stage, no count"
        );
    }

    /// F-POR3-006: lines of the pinned runner image's own scrape (bento 1.22.0, the dev args):
    /// `p-a` dropped 6 reports over two runs, `p-c` built its sink and dropped none.
    const DROPS: &str = concat!(
        "output_error{label=\"outcome_drop\",path=\"root.output.broker.outputs.1.fallback.1\",stream=\"p-c\"} 0\n",
        "output_sent{label=\"outcome_drop\",path=\"root.output.broker.outputs.1.fallback.1\",stream=\"p-a\"} 6\n",
        "output_sent{label=\"outcome_drop\",path=\"root.output.broker.outputs.1.fallback.1\",stream=\"p-c\"} 0\n",
        "output_sent{label=\"outcome_post\",path=\"root.output.broker.outputs.1.fallback.0\",stream=\"p-c\"} 3\n",
        "output_sent{label=\"output\",path=\"root.output.broker.outputs.0\",stream=\"p-c\"} 3\n",
    );

    #[test]
    fn the_dropped_reports_are_the_sinks_drop_sends_of_that_stream_alone() {
        assert_eq!(dropped_reports(DROPS, "p-a"), Some(6));
        assert_eq!(
            dropped_reports(DROPS, "p-c"),
            Some(0),
            "built, none dropped"
        );
        assert_eq!(
            dropped_reports(DROPS, "p-b"),
            None,
            "another stream's lines"
        );
        assert_eq!(
            dropped_reports("output_sent{label=\"output\",stream=\"p-a\"} 9\n", "p-a"),
            None,
            "a runner without the counter has no count"
        );
        assert_eq!(
            dropped_reports(
                "bento_output_sent_total{label=\"outcome_drop\",stream=\"p-a\"} 2\n",
                "p-a"
            ),
            Some(2),
            "a prefixed counter"
        );
    }

    /// T-3001: a pass report the Portal did not take is neither an error of the stream nor a
    /// node of it.
    #[test]
    fn the_pass_reports_own_failures_are_not_the_streams() {
        let body = concat!(
            "input_received{label=\"input\",stream=\"reaper\"} 5\n",
            "processor_error{label=\"pass\",stream=\"reaper\"} 5\n",
            "processor_error{label=\"pass_report\",stream=\"reaper\"} 5\n",
            "processor_error{label=\"processor_0\",stream=\"reaper\"} 1\n",
        );
        let metrics = scrape(body, "reaper", "2026-09-26T01:00:00Z".into());
        assert_eq!(metrics.errors, Some(1), "only the stream's own step");
        assert!(!metrics.nodes.contains_key("pass"));
        assert!(!metrics.nodes.contains_key("pass_report"));
    }

    #[test]
    fn a_stream_without_series_reports_no_numbers() {
        let metrics = scraped("not-running");
        assert_eq!(metrics.received, None, "absent is not zero");
        assert_eq!(metrics.errors, None);
        assert_eq!(metrics.latency_p99_ms, None);
    }

    #[test]
    fn prefixed_and_total_suffixed_names_are_the_same_families() {
        let body = concat!(
            "bento_input_received_total{stream=\"aq\"} 5\n",
            "bento_output_sent_total{stream=\"aq\"} 4\n"
        );
        let metrics = scrape(body, "aq", String::new());
        assert_eq!(metrics.received, Some(5));
        assert_eq!(metrics.sent, Some(4));
    }

    #[test]
    fn several_inputs_of_one_stream_are_summed() {
        let body = concat!(
            "input_received{label=\"a\",stream=\"aq\"} 5\n",
            "input_received{label=\"b\",stream=\"aq\"} 7\n",
            "output_latency_ns{label=\"a\",stream=\"aq\",quantile=\"0.99\"} 2000000\n",
            "output_latency_ns{label=\"b\",stream=\"aq\",quantile=\"0.99\"} 9000000\n"
        );
        let metrics = scrape(body, "aq", String::new());
        assert_eq!(metrics.received, Some(12));
        assert_eq!(
            metrics.latency_p99_ms,
            Some(9.0),
            "the slowest output, not their sum"
        );
    }

    /// Bento with `use_histogram_timing` (T-3625): the same family as cumulative buckets, whose
    /// `le` bounds are seconds.
    const HISTOGRAM: &str = concat!(
        "output_latency_ns_bucket{label=\"a\",stream=\"aq\",le=\"0.01\"} 50\n",
        "output_latency_ns_bucket{label=\"a\",stream=\"aq\",le=\"0.1\"} 90\n",
        "output_latency_ns_bucket{label=\"a\",stream=\"aq\",le=\"1\"} 100\n",
        "output_latency_ns_bucket{label=\"a\",stream=\"aq\",le=\"+Inf\"} 100\n",
        "output_latency_ns_sum{label=\"a\",stream=\"aq\"} 3.5\n",
        "output_latency_ns_count{label=\"a\",stream=\"aq\"} 100\n",
    );

    #[test]
    fn a_histogram_only_scrape_gives_the_interpolated_p99() {
        // 99 of 100 falls in (0.1, 1]: 90 below, 10 inside, 9/10 of the way: 0.91 s.
        let metrics = scrape(HISTOGRAM, "aq", String::new());
        let p99 = metrics.latency_p99_ms.expect("a latency from the buckets");
        assert!((p99 - 910.0).abs() < 1e-6, "{p99}");
    }

    #[test]
    fn two_labels_buckets_are_summed_into_one_distribution() {
        let body = format!(
            "{HISTOGRAM}{}",
            concat!(
                "output_latency_ns_bucket{label=\"b\",stream=\"aq\",le=\"0.01\"} 100\n",
                "output_latency_ns_bucket{label=\"b\",stream=\"aq\",le=\"0.1\"} 100\n",
                "output_latency_ns_bucket{label=\"b\",stream=\"aq\",le=\"1\"} 100\n",
                "output_latency_ns_bucket{label=\"b\",stream=\"aq\",le=\"+Inf\"} 100\n",
                "output_latency_ns_bucket{label=\"x\",stream=\"other\",le=\"1\"} 9\n",
            )
        );
        // 200 in all, rank 198: 190 by 0.1 s, 200 by 1 s, 8/10 of the way: 0.82 s.
        let p99 = scrape(&body, "aq", String::new())
            .latency_p99_ms
            .expect("a latency");
        assert!((p99 - 820.0).abs() < 1e-6, "{p99}");
    }

    #[test]
    fn the_writes_latency_wins_over_the_output_and_the_other_processors() {
        let body = concat!(
            "output_latency_ns{label=\"\",stream=\"aq\",quantile=\"0.99\"} 1000000\n",
            "processor_latency_ns{label=\"write_0\",stream=\"aq\",quantile=\"0.99\"} 420000000\n",
            "processor_latency_ns{label=\"route_1_write_0\",stream=\"aq\",quantile=\"0.99\"} 9000000\n",
            "processor_latency_ns{label=\"write_refused\",stream=\"aq\",quantile=\"0.99\"} 900000000\n",
            "processor_latency_ns{label=\"mapping_0\",stream=\"aq\",quantile=\"0.99\"} 800000000\n",
        );
        let metrics = scrape(body, "aq", String::new());
        assert_eq!(metrics.latency_p99_ms, Some(420.0));

        // A stream rendered before T-4056 has no write processor: its output says it.
        let old = concat!(
            "output_latency_ns{label=\"\",stream=\"aq\",quantile=\"0.99\"} 1000000\n",
            "processor_latency_ns{label=\"mapping_0\",stream=\"aq\",quantile=\"0.99\"} 800000000\n",
        );
        assert_eq!(scrape(old, "aq", String::new()).latency_p99_ms, Some(1.0));

        // Buckets of the writes beat the output's summary as well.
        let buckets = HISTOGRAM
            .replace("output_latency_ns", "processor_latency_ns")
            .replace("label=\"a\"", "label=\"write_0\"");
        let body = format!(
            "{buckets}output_latency_ns{{label=\"\",stream=\"aq\",quantile=\"0.99\"}} 1000000\n"
        );
        let p99 = scrape(&body, "aq", String::new()).latency_p99_ms.unwrap();
        assert!((p99 - 910.0).abs() < 1e-6, "{p99}");
    }

    #[test]
    fn a_rank_in_the_first_bucket_interpolates_from_zero() {
        let body = concat!(
            "output_latency_ns_bucket{stream=\"aq\",le=\"0.02\"} 10\n",
            "output_latency_ns_bucket{stream=\"aq\",le=\"+Inf\"} 10\n",
        );
        let p99 = scrape(body, "aq", String::new())
            .latency_p99_ms
            .expect("a latency");
        assert!((p99 - 19.8).abs() < 1e-9, "{p99}");
    }

    #[test]
    fn a_rank_past_the_last_finite_bound_answers_that_bound() {
        let body = concat!(
            "output_latency_ns_bucket{stream=\"aq\",le=\"0.5\"} 50\n",
            "output_latency_ns_bucket{stream=\"aq\",le=\"+Inf\"} 100\n",
        );
        assert_eq!(
            scrape(body, "aq", String::new()).latency_p99_ms,
            Some(500.0)
        );
    }

    #[test]
    fn inf_only_or_empty_buckets_give_no_latency() {
        for body in [
            "output_latency_ns_bucket{stream=\"aq\",le=\"+Inf\"} 7\n",
            concat!(
                "output_latency_ns_bucket{stream=\"aq\",le=\"0.1\"} 0\n",
                "output_latency_ns_bucket{stream=\"aq\",le=\"+Inf\"} 0\n",
            ),
            "output_latency_ns_bucket{stream=\"aq\",le=\"not-a-bound\"} 3\n",
        ] {
            assert_eq!(
                scrape(body, "aq", String::new()).latency_p99_ms,
                None,
                "{body}"
            );
        }
    }

    #[test]
    fn the_summary_wins_when_both_are_scraped() {
        let body = format!(
            "{HISTOGRAM}output_latency_ns{{label=\"a\",stream=\"aq\",quantile=\"0.99\"}} 42500000\n"
        );
        assert_eq!(
            scrape(&body, "aq", String::new()).latency_p99_ms,
            Some(42.5)
        );
    }

    #[test]
    fn comments_blank_lines_and_unlabelled_samples_are_ignored() {
        assert!(parse_line("# HELP x a counter").is_none());
        assert!(parse_line("   ").is_none());
        let sample = parse_line("uptime_seconds 900").expect("a sample without labels parses");
        assert_eq!(sample.name, "uptime_seconds");
        assert_eq!(sample.labels, "");
        assert_eq!(sample.value, 900.0);
    }

    /// T-1125, PL-24: Bento reports a component's counters under its `label`, so the studio can
    /// say which node a message reached instead of only what the stream totalled. The samples
    /// of another stream never reach either number.
    #[test]
    fn the_counters_of_each_component_are_kept_beside_the_stream_total() {
        let metrics = scraped("aq-mqtt-ingest");

        assert_eq!(metrics.received, Some(128_401), "the stream's own total");
        let input = metrics.nodes.get("mqtt").expect("the input's counters");
        assert_eq!(input.received, Some(128_401));
        assert_eq!(input.sent, None, "an input sends nothing of its own");

        let output = metrics.nodes.get("gateway").expect("the output's counters");
        assert_eq!(output.sent, Some(128_390));
        assert_eq!(output.errors, Some(2));

        // The other stream's samples carry the same label and are not folded in.
        assert_eq!(
            scraped("other-stream").nodes.get("mqtt").unwrap().received,
            Some(77)
        );
    }

    /// A stream deployed before the labels reports none, and the total still answers.
    #[test]
    fn a_stream_without_labels_reports_no_nodes_and_still_totals() {
        let body = "input_received{path=\"root.input\",stream=\"plain\"} 5\n";
        let metrics = scrape(body, "plain", "2026-09-06T16:20:11Z".into());
        assert_eq!(metrics.received, Some(5));
        assert!(metrics.nodes.is_empty());
    }

    #[tokio::test]
    async fn source_deletes_hide_the_ids_of_a_space_the_reader_does_not_read() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        use serde_json::json;
        let envelope = |kind: &str, name: &str, spec: serde_json::Value| ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, crate::permissions::ORG_NAMESPACE),
            spec,
            status: None,
        };
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let pipeline = |name: &str, spec: serde_json::Value| ResourceEnvelope {
            metadata: ObjectMeta::new(name, "helsinki"),
            ..envelope("Pipeline", name, spec)
        };
        state.mirror.upsert(pipeline(
            "feed",
            json!({
                "class": "scheduled",
                "schedule": "0 * * * *",
                "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }],
                "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:ep-vehicles" }],
                "sourceDeletes": { "mode": "snapshot", "misses": 2, "maxShare": 10 },
            }),
        ));
        state.mirror.upsert(pipeline(
            "plain",
            json!({ "class": "auto", "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }] }),
        ));
        state.mirror.upsert(envelope(
            "Role",
            "reader",
            json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            "reader",
            json!({ "subjects": [{ "user": "reader@hel.fi" }], "role": "reader", "scope": { "project": "helsinki" } }),
        ));
        let ids = ["urn:a".to_owned()];
        state
            .source_ledger
            .seen("helsinki", "feed", "r1", &ids, true)
            .await
            .expect("seen");
        let who = |email: &str| {
            CurrentUser(crate::auth::session::Session {
                identity: crate::auth::session::Identity {
                    client: None,
                    subject: format!("f:1:{email}"),
                    username: email.to_owned(),
                    email: Some(email.to_owned()),
                    name: None,
                    roles: Vec::new(),
                    groups: Vec::new(),
                    second_factor: false,
                },
                expires_at: 0,
                issued_at: 0,
                signed_in_at: None,
                id_token: String::new(),
                access_expires_at: 0,
                refresh_token: None,
            })
        };
        let ask = |email: &str, name: &str| {
            get_source_deletes(
                who(email),
                State(state.clone()),
                Path(("helsinki".to_owned(), name.to_owned())),
            )
        };
        let overview = ask("reader@hel.fi", "feed").await.expect("the overview").0;
        assert_eq!(overview.ledger, 1, "the counts are the pipeline's own");
        assert!(overview.ids_hidden && overview.orphan_ids.is_empty());
        assert!(matches!(
            ask("reader@hel.fi", "plain").await,
            Err(ApiError::Conflict(_))
        ));
        assert!(matches!(
            ask("stranger@hel.fi", "feed").await,
            Err(ApiError::NotFound(_))
        ));
    }
}
