//! A pipeline's runs and the log of each (PL-62, ADR-N-034).
//!
//! The runner sends one outcome line per record through a sink that drops rather than retries:
//! `sent` once the gateway took the record's batch, `rejected` when the validation stage refused
//! it, `failed` when one of the author's steps threw. A run is one tick of the pipeline's clock,
//! or one UTC hour for a source that never ends; the runner names it.
//!
//! Bounded per pipeline: the newest [`LINES_KEPT`] lines and the newest [`RUNS_KEPT`] runs, the
//! database trimmed per [`TRIM_EVERY_LINES`] lines or [`TRIM_EVERY`] rather than per batch. A
//! run's counts are kept apart from its lines, so a run of 4000 records still says 4000 after
//! most of its lines have aged out. Record ids and messages are masked before they are stored:
//! an id is whatever the mapping produced, and a message can quote the source.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

/// The labels of the sink and each of its parts; an unlabelled part would count its reports as
/// the stream's own writes.
pub const SINK_LABELS: [&str; 8] = [
    "outcome",
    "outcome_post",
    "outcome_runs",
    "outcome_join",
    "outcome_chunks",
    "outcome_split",
    "outcome_drop",
    "outcome_report",
];

/// How many writes' reports the sink gathers before it sends them, at most: each carries the
/// ids and hashes of one write, never its records.
pub const REPORTS_GATHERED: usize = 100;

/// The most ids one outcome report carries: the outcome route's own limit.
pub const BATCH_LINES: usize = 1000;

/// The start of the metadata key each write leaves its refused items in (T-4056): a JSON array
/// of `{id, message}`, one key per target, so a record refused by any target is logged `failed`.
pub const REFUSED_PREFIX: &str = "jc_refused_";

/// The metadata key that says the gateway refused a write's whole batch (`400`, `413`, `422`),
/// in its own words (T-4056).
pub const FAILED_KEY: &str = "jc_failed";

/// The labels of the pass report and its request (T-3001), and of a run's report that its source
/// refused the request (T-4428), which the metrics leave out like the sink: a report the Portal did not take says nothing about the stream's own reads and writes.
pub const PASS_LABELS: [&str; 4] = [
    "pass",
    "pass_report",
    "source_failed",
    "source_failed_report",
];

/// How many log lines one pipeline keeps.
pub const LINES_KEPT: usize = 5000;
/// How many runs one pipeline keeps the counts of.
pub const RUNS_KEPT: usize = 200;
/// The longest record id and message a line carries.
const TEXT_CHARS: usize = 500;

/// What became of one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = PipelineOutcome)]
pub enum Outcome {
    /// The gateway took the batch it was in.
    Sent,
    /// The validation stage refused it; it is on the rejected list.
    Rejected,
    /// One of the author's steps threw on it.
    Failed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Sent => "sent",
            Outcome::Rejected => "rejected",
            Outcome::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "sent" => Outcome::Sent,
            "rejected" => Outcome::Rejected,
            _ => Outcome::Failed,
        }
    }
}

/// One line as the runner or the rejected route reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLine {
    pub record_id: String,
    pub step: Option<i32>,
    pub outcome: Outcome,
    pub message: String,
}

/// One line of a run's log.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineLogLine)]
pub struct LogLine {
    /// Its place in the log; the `before` of the next page.
    pub id: i64,
    /// When the Portal took the line, RFC 3339.
    pub at: String,
    /// The run it belongs to.
    pub run: String,
    /// The record's `id`, as the mapping produced it; empty when it had none.
    pub record_id: String,
    /// The step of `spec.steps` it failed at, when it failed in a step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<i32>,
    pub outcome: Outcome,
    /// What happened, in words; empty for a record that was sent.
    pub message: String,
}

/// One run with its counts.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineRun)]
pub struct Run {
    /// Its name: the tick's time, or the UTC hour of a source that never ends.
    pub run: String,
    /// The first and the last line the Portal took for it, RFC 3339.
    pub first_at: String,
    pub last_at: String,
    pub sent: u64,
    pub rejected: u64,
    pub failed: u64,
    /// What the written records did (T-3304): an id the pipeline never wrote before, a record
    /// whose hash changed, one whose hash did not. `null` when the run was reported without hashes.
    pub created: Option<u64>,
    pub updated: Option<u64>,
    pub unchanged: Option<u64>,
}

/// What the written records of one report did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changes {
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
}

type Key = (String, String);

#[derive(Default)]
struct Memory {
    next: i64,
    lines: Vec<LogLine>,
    runs: Vec<Run>,
    /// The last hash written per entity id.
    hashes: HashMap<String, String>,
    /// The stream hash each run ran, by run (T-5825).
    streams: HashMap<String, String>,
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<HashMap<Key, Memory>>),
}

/// The words of a log line whose cause lies outside the pipeline's source and may clear by itself
/// (T-5687): the space's model refused the record (DM-61), a policy refused the write, or the
/// write failed past its retries on 401, 403, 429 or a 5xx. One pattern for Postgres (`~`) and
/// the `regex` crate.
pub const OUTSIDE_CAUSE: &str = r"\(DM-61\)|Access Denied by Policy|^the write failed: .*unexpected response code \((401|403|429|5[0-9][0-9])\)";

static OUTSIDE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(OUTSIDE_CAUSE).expect("a valid pattern"));

/// Lines a pipeline writes after its last trim before the database trims it again. Each trim
/// walks the newest [`LINES_KEPT`] ids; per batch it took 1.1-1.8 s on dev (T-5292).
pub const TRIM_EVERY_LINES: usize = 500;
/// The longest a pipeline's log goes untrimmed while it writes.
pub const TRIM_EVERY: Duration = Duration::from_secs(60);

/// When each pipeline's log was last trimmed and the lines written since.
#[derive(Default)]
struct Trims(HashMap<Key, (Instant, usize)>);

impl Trims {
    /// Whether writing `lines` more for `key` at `now` trims its log: the first write of this
    /// process does, then every [`TRIM_EVERY_LINES`] lines or [`TRIM_EVERY`].
    fn due(&mut self, key: &Key, lines: usize, now: Instant) -> bool {
        let Some((at, since)) = self.0.get_mut(key) else {
            self.0.insert(key.clone(), (now, 0));
            return true;
        };
        *since = since.saturating_add(lines);
        if *since < TRIM_EVERY_LINES && now.saturating_duration_since(*at) < TRIM_EVERY {
            return false;
        }
        *at = now;
        *since = 0;
        true
    }
}

/// The runs and logs of every pipeline.
pub struct LogStore {
    inner: Inner,
    trims: Mutex<Trims>,
}

/// A run name the runner sent, or the current UTC hour when it sent none a person could read:
/// the name is shown and filtered on, so it is short and plain.
pub fn run_name(sent: Option<&str>) -> String {
    match sent.map(str::trim) {
        Some(name)
            if !name.is_empty()
                && name.len() <= 40
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '.')) =>
        {
            name.to_owned()
        }
        _ => {
            let now = time::OffsetDateTime::now_utc();
            format!(
                "{:04}-{:02}-{:02}T{:02}:00Z",
                now.year(),
                u8::from(now.month()),
                now.day(),
                now.hour()
            )
        }
    }
}

/// The Bloblang that names a message's run in the runner: the tick its clock stamped, or the
/// UTC hour for a source without a clock.
pub const RUN: &str = r#"meta("jc_run").or(now().ts_format("2006-01-02T15:00Z", "UTC"))"#;

/// The Bloblang a clocked input stamps its tick with, so every record read on it is one run.
pub const TICK: &str = r#"meta jc_run = now().ts_format("2006-01-02T15:04:05Z", "UTC")"#;

/// The output a stream writes through, followed by the outcome sink once it has taken a batch
/// (PL-62): the sink only runs after the write succeeded, and a sink that fails falls through to
/// `drop`, so the log never holds back or repeats a write.
///
/// The fall-through is a `fallback`, not `drop_on`: Bento's `drop_on` waits for each answer
/// before it takes the next message, which held a stream of single positions to one outcome
/// at a time, about fifty a second; the HSL feed sends three times that, backed up into its
/// MQTT input and lost the connection every 45 seconds (T-4036). `fallback` keeps the sink's
/// own in-flight window.
///
/// The sink reports in batches, not one POST per write: one report per write held a
/// connection-pool slot and the run's row of `pipeline_runs` for each HFP position, the route
/// answered in 1.4 s on dev, and 64 reports in flight capped the stream at 45 writes a second
/// (T-4036, again). Each write is first reduced to its report (run, ids, hashes); the sink
/// gathers those for half a second, merges them per run and sends at most [`BATCH_LINES`] ids
/// per report. Gathering the records themselves held every body of the last half second, and
/// the runner was OOMKilled (T-4062).
///
/// Every part carries a label from [`SINK_LABELS`], so the stream's counters leave the sink out.
///
/// The broker fails fast (T-2983): a write that fails after its own retries goes back to the
/// input as a nack, which replays it. Plain `fan_out_sequential` retried it inside the broker
/// without end and without heeding a stop, so a stream whose target failed could be neither
/// replaced nor deleted, and only a runner restart (all its streams) freed it.
pub fn with_outcomes(output: serde_json::Value, url: &str) -> serde_json::Value {
    let sink = serde_json::json!({
        "label": SINK_LABELS[0],
        "fallback": [
            { "label": SINK_LABELS[1], "http_client": {
                "url": url,
                "verb": "POST",
                "headers": { "Content-Type": "application/json" },
                "timeout": "5s",
                "retries": 0,
                "oauth2": {
                    "enabled": true,
                    "client_key": "${JC_CLIENT_ID}",
                    "client_secret": "${JC_CLIENT_SECRET}",
                    "token_url": "${JC_TOKEN_URL}",
                },
                "batching": {
                    "count": REPORTS_GATHERED,
                    "period": "500ms",
                    "processors": [
                        { "label": SINK_LABELS[2], "group_by_value": { "value": "${! json(\"run\") }" } },
                        { "label": SINK_LABELS[3], "archive": { "format": "json_array" } },
                        { "label": SINK_LABELS[4], "mapping": format!(
                            "let sent = this.map_each(report -> report.sent).flatten()\n\
                             let hashes = this.map_each(report -> report.hashes).flatten()\n\
                             let failed = this.map_each(report -> report.failed.or([])).flatten()\n\
                             let run = this.index(0).run\n\
                             let n = $sent.length()\n\
                             let f = $failed.length()\n\
                             root = range(0, [(($n + {last}) / {BATCH_LINES}).floor(), 1].max()).map_each(i -> {{ \
                             \"run\": $run, \
                             \"sent\": $sent.slice(i * {BATCH_LINES}, [i * {BATCH_LINES} + {BATCH_LINES}, $n].min()), \
                             \"hashes\": $hashes.slice(i * {BATCH_LINES}, [i * {BATCH_LINES} + {BATCH_LINES}, $n].min()) }}).concat(\
                             if $f == 0 {{ [] }} else {{ \
                             range(0, (($f + {last}) / {BATCH_LINES}).floor()).map_each(i -> {{ \
                             \"run\": $run, \"sent\": [], \"hashes\": [], \
                             \"failed\": $failed.slice(i * {BATCH_LINES}, [i * {BATCH_LINES} + {BATCH_LINES}, $f].min()) }}) }})",
                            last = BATCH_LINES - 1,
                        ) },
                        { "label": SINK_LABELS[5], "unarchive": { "format": "json_array" } },
                    ],
                },
            }},
            { "label": SINK_LABELS[6], "drop": {} },
        ],
        // A write's report: the records it wrote, and apart from them those its targets refused
        // (T-4056): every record when the batch was refused whole, else each write's items.
        "processors": [{ "label": SINK_LABELS[7], "mapping": format!(
            "let records = if this.type() == \"array\" {{ this }} else {{ [this] }}\n\
             let refused = if @{FAILED_KEY} != null {{ $records.map_each(record -> {{ \"id\": record.id.or(\"\"), \"message\": @{FAILED_KEY} }}) }} \
             else {{ @.keys().filter(key -> key.has_prefix(\"{REFUSED_PREFIX}\")).map_each(key -> @.get(key).parse_json().catch([])).flatten() }}\n\
             let gone = $refused.map_each(item -> item.id)\n\
             let written = $records.filter(record -> !$gone.contains(record.id.or(\"\")))\n\
             root = {{ \"run\": {RUN}, \"sent\": $written.map_each(record -> record.id.or(\"\")), \
             \"hashes\": $written.map_each(record -> record.format_json().hash(\"xxhash64\").string()), \
             \"failed\": $refused }}"
        ) }],
    });
    serde_json::json!({ "broker": {
        "pattern": "fan_out_sequential_fail_fast",
        "outputs": [output, sink],
    }})
}

/// Reports every pass of a clocked stream to its run's log before the pass reads (T-3001, PL-62):
/// a report with no records, which the outcome route keeps as a run with its counts at zero. A
/// pass whose writes all went elsewhere (a reaper deleting through its own step) or that found
/// nothing new is then still seen to have run.
///
/// Only a stream on a `generate` clock has passes; a resident source is one run per hour and its
/// records say that it runs. The report is a branch whose answer is thrown away, and the `catch`
/// after it clears a failed report, so the Portal being away never costs the tick its read.
pub fn with_passes(stream: &mut serde_json::Value, url: &str) {
    if stream.pointer("/input/generate").is_none() {
        return;
    }
    let report = serde_json::json!({ "label": PASS_LABELS[0], "branch": {
        "request_map": format!("root = {{ \"run\": {RUN}, \"sent\": [] }}"),
        "processors": [{ "label": PASS_LABELS[1], "http": pass_http(url) }],
    }});
    let Some(pipeline) = stream.as_object_mut().map(|stream| {
        stream
            .entry("pipeline")
            .or_insert_with(|| serde_json::json!({}))
    }) else {
        return;
    };
    let Some(pipeline) = pipeline.as_object_mut() else {
        return;
    };
    let processors = pipeline
        .entry("processors")
        .or_insert_with(|| serde_json::json!([]));
    if let Some(processors) = processors.as_array_mut() {
        processors.splice(0..0, [report, serde_json::json!({ "catch": [] })]);
    }
}

/// The `http` processor a pass is reported with (T-4269). The report carries no lines, so a
/// repeat adds nothing to the run's counts and is safe to retry; without it one refused or slow
/// POST at a stream's first tick (the Portal restarting, the token endpoint slow) left a 24 h
/// clock without a run for a day. Three tries 2 s apart hold the tick's read at most ~21 s.
fn pass_http(url: &str) -> serde_json::Value {
    let mut http = report_http(url);
    http["retries"] = serde_json::json!(3);
    http["retry_period"] = serde_json::json!("2s");
    http
}

/// The `http` processor a stream posts a report to its run's log with.
fn report_http(url: &str) -> serde_json::Value {
    serde_json::json!({
        "url": url,
        "verb": "POST",
        "headers": { "Content-Type": "application/json" },
        "timeout": "5s",
        "retries": 0,
        "oauth2": {
            "enabled": true,
            "client_key": "${JC_CLIENT_ID}",
            "client_secret": "${JC_CLIENT_SECRET}",
            "token_url": "${JC_TOKEN_URL}",
        },
    })
}

/// The processors a run puts after its source's request (T-4428): a request the source refused
/// is reported to the run's log as a `failed` line with the source's words, then dropped, so a
/// run whose upstream answered 4xx reads as failed, not as a run that found nothing. The words
/// lose every URL query (where a key could ride); the route masks secret-shaped text as well.
pub fn source_failure(url: &str) -> serde_json::Value {
    let report = format!(
        r#"root = {{ "run": {RUN}, "sent": [], "failed": [{{ "id": "", "message": error().or("the source refused the request").re_replace_all("""\?[^'"\s]*""", "") }}] }}"#
    );
    serde_json::json!({ "switch": [{
        "check": "errored()",
        "processors": [
            { "label": PASS_LABELS[2], "branch": {
                "request_map": report,
                "processors": [{ "label": PASS_LABELS[3], "http": report_http(url) }],
            }},
            { "mapping": "root = deleted()" },
        ],
    }]})
}

/// Puts the run-log report in place of each processor labelled `label` (PL-104): a run whose
/// step found it must write nothing reports why as a `failed` line, then drops the message.
pub fn with_problems(stream: &mut serde_json::Value, label: &str, url: &str) {
    let Some(processors) = stream
        .pointer_mut("/pipeline/processors")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for processor in processors.iter_mut() {
        if processor.get("label").and_then(serde_json::Value::as_str) == Some(label) {
            let mut report = source_failure(url);
            report["label"] = serde_json::json!(label);
            *processor = report;
        }
    }
}

fn clean(text: &str) -> String {
    let short: String = text.chars().take(TEXT_CHARS).collect();
    crate::pipeline_outcomes::mask_text(&short)
}

pub(crate) fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

type LineRow = (
    i64,
    time::OffsetDateTime,
    String,
    String,
    Option<i32>,
    String,
    String,
);

/// A `pipeline_runs` row as a [`Run`].
fn run_of((run, first, last, sent, rejected, failed, created, updated, unchanged): RunRow) -> Run {
    Run {
        run,
        first_at: rfc3339(first),
        last_at: rfc3339(last),
        sent: u64::try_from(sent).unwrap_or_default(),
        rejected: u64::try_from(rejected).unwrap_or_default(),
        failed: u64::try_from(failed).unwrap_or_default(),
        created: created.and_then(|n| u64::try_from(n).ok()),
        updated: updated.and_then(|n| u64::try_from(n).ok()),
        unchanged: unchanged.and_then(|n| u64::try_from(n).ok()),
    }
}

type RunRow = (
    String,
    time::OffsetDateTime,
    time::OffsetDateTime,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// A [`RunRow`] with its pipeline's name last.
type LatestRow = (
    String,
    time::OffsetDateTime,
    time::OffsetDateTime,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    String,
);

impl LogStore {
    /// Durable with a database, in memory without one.
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::new(HashMap::new())),
            },
            trims: Mutex::new(Trims::default()),
        }
    }

    /// Adds the lines of one report to the run's log and its counts, then drops what is beyond
    /// the bounds (in the database only when `Trims::due`). A report with no lines is a pass that wrote nothing: the run is kept with its
    /// counts at zero, so "ran, nothing new" reads apart from "did not run" (T-3001).
    pub async fn append(
        &self,
        project: &str,
        pipeline: &str,
        run: &str,
        lines: &[NewLine],
    ) -> Result<(), sqlx::Error> {
        let count = |outcome: Outcome| lines.iter().filter(|l| l.outcome == outcome).count() as i64;
        let (sent, rejected, failed) = (
            count(Outcome::Sent),
            count(Outcome::Rejected),
            count(Outcome::Failed),
        );
        let ids: Vec<String> = lines.iter().map(|l| clean(&l.record_id)).collect();
        let messages: Vec<String> = lines.iter().map(|l| clean(&l.message)).collect();
        match &self.inner {
            Inner::Postgres(pool) => {
                let steps: Vec<Option<i32>> = lines.iter().map(|l| l.step).collect();
                let outcomes: Vec<&str> = lines.iter().map(|l| l.outcome.as_str()).collect();
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "INSERT INTO pipeline_log (project, pipeline, run, record_id, step, outcome, message) \
                     SELECT $1, $2, $3, r, s, o, m FROM UNNEST($4::text[], $5::int[], $6::text[], $7::text[]) \
                     AS t(r, s, o, m)",
                )
                .bind(project)
                .bind(pipeline)
                .bind(run)
                .bind(&ids)
                .bind(&steps)
                .bind(&outcomes)
                .bind(&messages)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO pipeline_runs (project, pipeline, run, sent, rejected, failed) \
                     VALUES ($1, $2, $3, $4, $5, $6) \
                     ON CONFLICT (project, pipeline, run) DO UPDATE SET last_at = now(), \
                     sent = pipeline_runs.sent + $4, rejected = pipeline_runs.rejected + $5, \
                     failed = pipeline_runs.failed + $6",
                )
                .bind(project)
                .bind(pipeline)
                .bind(run)
                .bind(sent)
                .bind(rejected)
                .bind(failed)
                .execute(&mut *tx)
                .await?;
                let key = (project.to_owned(), pipeline.to_owned());
                let due = self.trims.lock().unwrap_or_else(|e| e.into_inner()).due(
                    &key,
                    lines.len(),
                    Instant::now(),
                );
                if !due {
                    return tx.commit().await;
                }
                sqlx::query(
                    "DELETE FROM pipeline_log WHERE project = $1 AND pipeline = $2 AND id < \
                     (SELECT coalesce(min(id), 0) FROM (SELECT id FROM pipeline_log \
                      WHERE project = $1 AND pipeline = $2 ORDER BY id DESC LIMIT $3) AS kept)",
                )
                .bind(project)
                .bind(pipeline)
                .bind(LINES_KEPT as i64)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "DELETE FROM pipeline_runs WHERE project = $1 AND pipeline = $2 AND run NOT IN \
                     (SELECT run FROM pipeline_runs WHERE project = $1 AND pipeline = $2 \
                      ORDER BY last_at DESC LIMIT $3)",
                )
                .bind(project)
                .bind(pipeline)
                .bind(RUNS_KEPT as i64)
                .execute(&mut *tx)
                .await?;
                tx.commit().await
            }
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                let memory = map
                    .entry((project.to_owned(), pipeline.to_owned()))
                    .or_default();
                let at = rfc3339(time::OffsetDateTime::now_utc());
                for ((line, record_id), message) in lines.iter().zip(ids).zip(messages) {
                    memory.next += 1;
                    memory.lines.push(LogLine {
                        id: memory.next,
                        at: at.clone(),
                        run: run.to_owned(),
                        record_id,
                        step: line.step,
                        outcome: line.outcome,
                        message,
                    });
                }
                let over = memory.lines.len().saturating_sub(LINES_KEPT);
                memory.lines.drain(..over);
                match memory.runs.iter().position(|r| r.run == run) {
                    Some(at_) => {
                        let mut kept = memory.runs.remove(at_);
                        kept.last_at = at;
                        kept.sent += sent as u64;
                        kept.rejected += rejected as u64;
                        kept.failed += failed as u64;
                        memory.runs.push(kept);
                    }
                    None => memory.runs.push(Run {
                        run: run.to_owned(),
                        first_at: at.clone(),
                        last_at: at,
                        sent: sent as u64,
                        rejected: rejected as u64,
                        failed: failed as u64,
                        created: None,
                        updated: None,
                        unchanged: None,
                    }),
                }
                let over = memory.runs.len().saturating_sub(RUNS_KEPT);
                memory.runs.drain(..over);
                let runs = &memory.runs;
                memory
                    .streams
                    .retain(|run, _| runs.iter().any(|kept| kept.run == *run));
                Ok(())
            }
        }
    }

    /// Splits what one report wrote into created, updated and unchanged by the hash of each record
    /// against the last one kept for its id, keeps the new hashes, and adds the split to the run
    /// `append` just counted (T-3304). An id written twice in one report is compared with itself.
    pub async fn record_changes(
        &self,
        project: &str,
        pipeline: &str,
        run: &str,
        written: &[(String, String)],
    ) -> Result<Changes, sqlx::Error> {
        let written: Vec<(String, String)> = written
            .iter()
            .filter(|(id, _)| !id.is_empty())
            .map(|(id, hash)| (clean(id), hash.chars().take(64).collect()))
            .collect();
        let tally = |known: &mut HashMap<String, String>| {
            let mut changes = Changes::default();
            for (id, hash) in &written {
                match known.insert(id.clone(), hash.clone()) {
                    None => changes.created += 1,
                    Some(before) if &before == hash => changes.unchanged += 1,
                    Some(_) => changes.updated += 1,
                }
            }
            changes
        };
        match &self.inner {
            Inner::Postgres(pool) => {
                let ids: Vec<&str> = written.iter().map(|(id, _)| id.as_str()).collect();
                let mut tx = pool.begin().await?;
                let rows: Vec<(String, String)> = sqlx::query_as(
                    "SELECT record, hash FROM pipeline_record_hashes \
                     WHERE project = $1 AND pipeline = $2 AND record = ANY($3::text[])",
                )
                .bind(project)
                .bind(pipeline)
                .bind(&ids)
                .fetch_all(&mut *tx)
                .await?;
                let mut known: HashMap<String, String> = rows.into_iter().collect();
                let changes = tally(&mut known);
                let (records, hashes): (Vec<&str>, Vec<&str>) = known
                    .iter()
                    .filter(|(id, _)| ids.contains(&id.as_str()))
                    .map(|(id, hash)| (id.as_str(), hash.as_str()))
                    .unzip();
                sqlx::query(
                    "INSERT INTO pipeline_record_hashes (project, pipeline, record, hash) \
                     SELECT $1, $2, r, h FROM UNNEST($3::text[], $4::text[]) AS t(r, h) \
                     ON CONFLICT (project, pipeline, record) DO UPDATE SET hash = EXCLUDED.hash",
                )
                .bind(project)
                .bind(pipeline)
                .bind(&records)
                .bind(&hashes)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE pipeline_runs SET created = coalesce(created, 0) + $4, \
                     updated = coalesce(updated, 0) + $5, unchanged = coalesce(unchanged, 0) + $6 \
                     WHERE project = $1 AND pipeline = $2 AND run = $3",
                )
                .bind(project)
                .bind(pipeline)
                .bind(run)
                .bind(changes.created as i64)
                .bind(changes.updated as i64)
                .bind(changes.unchanged as i64)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(changes)
            }
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                let memory = map
                    .entry((project.to_owned(), pipeline.to_owned()))
                    .or_default();
                let changes = tally(&mut memory.hashes);
                if let Some(kept) = memory.runs.iter_mut().find(|r| r.run == run) {
                    kept.created = Some(kept.created.unwrap_or_default() + changes.created);
                    kept.updated = Some(kept.updated.unwrap_or_default() + changes.updated);
                    kept.unchanged = Some(kept.unchanged.unwrap_or_default() + changes.unchanged);
                }
                Ok(changes)
            }
        }
    }

    /// Drops the runs and the log of every pipeline a project `loaded` no longer holds, as
    /// [`crate::pipeline_outcomes::RejectedStore::forget_gone`] does with the refused records.
    pub async fn forget_gone(
        &self,
        loaded: &BTreeMap<String, HashSet<String>>,
    ) -> Result<(), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let scope: Vec<&str> = loaded.keys().map(String::as_str).collect();
                let (projects, pipelines): (Vec<&str>, Vec<&str>) = loaded
                    .iter()
                    .flat_map(|(project, names)| {
                        names.iter().map(move |n| (project.as_str(), n.as_str()))
                    })
                    .unzip();
                let mut tx = pool.begin().await?;
                for statement in [
                    "DELETE FROM pipeline_log r WHERE r.project = ANY($1::text[]) AND NOT EXISTS (SELECT 1 FROM \
                     UNNEST($2::text[], $3::text[]) AS k(project, pipeline) \
                     WHERE k.project = r.project AND k.pipeline = r.pipeline)",
                    "DELETE FROM pipeline_runs r WHERE r.project = ANY($1::text[]) AND NOT EXISTS (SELECT 1 FROM \
                     UNNEST($2::text[], $3::text[]) AS k(project, pipeline) \
                     WHERE k.project = r.project AND k.pipeline = r.pipeline)",
                    "DELETE FROM pipeline_record_hashes r WHERE r.project = ANY($1::text[]) AND NOT EXISTS (SELECT 1 FROM \
                     UNNEST($2::text[], $3::text[]) AS k(project, pipeline) \
                     WHERE k.project = r.project AND k.pipeline = r.pipeline)",
                ] {
                    sqlx::query(statement)
                        .bind(&scope)
                        .bind(&projects)
                        .bind(&pipelines)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await
            }
            Inner::Memory(map) => {
                map.write()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|(project, pipeline), _| {
                        loaded
                            .get(project)
                            .is_none_or(|names| names.contains(pipeline))
                    });
                Ok(())
            }
        }
    }

    /// Every (project, pipeline) with at least one run kept (T-4421), with the time of its newest
    /// run's last line: a pipeline whose next due time after it has passed missed a run while no
    /// Portal held it (T-5778).
    pub async fn ran(&self) -> Result<HashMap<Key, chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query_as::<_, (String, String, time::OffsetDateTime)>(
                "SELECT project, pipeline, max(last_at) FROM pipeline_runs GROUP BY project, pipeline",
            )
            .fetch_all(pool)
            .await?
            .into_iter()
            .filter_map(|(project, pipeline, last)| {
                let last = chrono::DateTime::from_timestamp(last.unix_timestamp(), 0)?;
                Some(((project, pipeline), last))
            })
            .collect()),
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(|(key, memory)| {
                    let last = memory
                        .runs
                        .iter()
                        .filter_map(|run| chrono::DateTime::parse_from_rfc3339(&run.last_at).ok())
                        .max()?;
                    Some((key.clone(), last.with_timezone(&chrono::Utc)))
                })
                .collect()),
        }
    }

    /// Files the [`stream_hash`](crate::reconciler::run_queue::stream_hash) of the stream the run
    /// `run` ran with its row (T-5825). A run that logged no line has no row and keeps none: a
    /// restarted Portal then plans the pipeline as before.
    pub async fn ran_stream(
        &self,
        project: &str,
        pipeline: &str,
        run: &str,
        stream: &str,
    ) -> Result<(), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                sqlx::query(
                    "UPDATE pipeline_runs SET stream_hash = $4 \
                     WHERE project = $1 AND pipeline = $2 AND run = $3",
                )
                .bind(project)
                .bind(pipeline)
                .bind(run)
                .bind(stream)
                .execute(pool)
                .await?;
                Ok(())
            }
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                if let Some(memory) = map.get_mut(&(project.to_owned(), pipeline.to_owned())) {
                    if memory.runs.iter().any(|logged| logged.run == run) {
                        memory.streams.insert(run.to_owned(), stream.to_owned());
                    }
                }
                Ok(())
            }
        }
    }

    /// Every (project, pipeline) whose newest run has a stream hash, with that hash (T-5825):
    /// a replace pipeline whose stream differs from it changed while no Portal held it.
    pub async fn ran_streams(&self) -> Result<HashMap<Key, String>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query_as::<_, (String, String, Option<String>)>(
                "SELECT DISTINCT ON (project, pipeline) project, pipeline, stream_hash \
                 FROM pipeline_runs ORDER BY project, pipeline, last_at DESC",
            )
            .fetch_all(pool)
            .await?
            .into_iter()
            .filter_map(|(project, pipeline, stream)| Some(((project, pipeline), stream?)))
            .collect()),
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(|(key, memory)| {
                    // The newest run is the last: each report moves its run to the end.
                    let newest = memory.runs.last()?;
                    Some((key.clone(), memory.streams.get(&newest.run)?.clone()))
                })
                .collect()),
        }
    }

    /// The pipeline's runs, the one with the latest line first, at most `limit`.
    pub async fn runs(
        &self,
        project: &str,
        pipeline: &str,
        limit: usize,
    ) -> Result<Vec<Run>, sqlx::Error> {
        let limit = limit.clamp(1, RUNS_KEPT);
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<RunRow> = sqlx::query_as(
                    "SELECT run, first_at, last_at, sent, rejected, failed, created, updated, unchanged FROM pipeline_runs \
                     WHERE project = $1 AND pipeline = $2 ORDER BY last_at DESC LIMIT $3",
                )
                .bind(project)
                .bind(pipeline)
                .bind(limit as i64)
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(run_of).collect())
            }
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(project.to_owned(), pipeline.to_owned()))
                .map(|memory| memory.runs.iter().rev().take(limit).cloned().collect())
                .unwrap_or_default()),
        }
    }

    /// The newest run of each of `pipelines` that ran, by name, in one read (T-5818).
    pub async fn latest_runs(
        &self,
        project: &str,
        pipelines: &[String],
    ) -> Result<BTreeMap<String, Run>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<LatestRow> = sqlx::query_as(
                    "SELECT DISTINCT ON (pipeline) run, first_at, last_at, sent, rejected, failed, created, updated, unchanged, pipeline \
                     FROM pipeline_runs WHERE project = $1 AND pipeline = ANY($2) ORDER BY pipeline, last_at DESC",
                )
                .bind(project)
                .bind(pipelines)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(
                            run,
                            first,
                            last,
                            sent,
                            rejected,
                            failed,
                            created,
                            updated,
                            unchanged,
                            pipeline,
                        )| {
                            (
                                pipeline,
                                run_of((
                                    run, first, last, sent, rejected, failed, created, updated,
                                    unchanged,
                                )),
                            )
                        },
                    )
                    .collect())
            }
            Inner::Memory(map) => {
                let map = map.read().unwrap_or_else(|e| e.into_inner());
                Ok(pipelines
                    .iter()
                    .filter_map(|name| {
                        let memory = map.get(&(project.to_owned(), name.clone()))?;
                        Some((name.clone(), memory.runs.last()?.clone()))
                    })
                    .collect())
            }
        }
    }

    /// The pipelines whose newest `streak` runs the gateway each refused for an ended grant
    /// (PL-93), with the newest run's words: a `failed` line [`crate::access_requests::is_refusal`]
    /// reads as one.
    pub async fn refused_streaks(
        &self,
        streak: usize,
    ) -> Result<BTreeMap<Key, String>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query_as::<_, (String, String, String)>(
                "WITH newest AS (SELECT project, pipeline, run, row_number() OVER                    (PARTITION BY project, pipeline ORDER BY last_at DESC) AS n FROM pipeline_runs)                  SELECT r.project, r.pipeline, (array_agg(m.message ORDER BY r.n))[1] FROM newest r                  JOIN LATERAL (SELECT message FROM pipeline_log g WHERE g.project = r.project                    AND g.pipeline = r.pipeline AND g.run = r.run AND g.outcome = 'failed'                    AND (g.message LIKE 'access revoked by %' OR g.message LIKE 'access ended on %')                    ORDER BY g.id DESC LIMIT 1) m ON true                  WHERE r.n <= $1 GROUP BY r.project, r.pipeline HAVING count(*) = $1",
            )
            .bind(streak as i64)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(project, pipeline, message)| ((project, pipeline), message))
            .collect()),
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(|(key, memory)| {
                    let newest: Vec<&Run> = memory.runs.iter().rev().take(streak).collect();
                    let refusals: Vec<&str> = newest
                        .iter()
                        .filter_map(|run| {
                            memory
                                .lines
                                .iter()
                                .rev()
                                .find(|l| {
                                    l.run == run.run
                                        && l.outcome == Outcome::Failed
                                        && crate::access_requests::is_refusal(&l.message)
                                })
                                .map(|l| l.message.as_str())
                        })
                        .collect();
                    (newest.len() == streak && refusals.len() == streak)
                        .then(|| (key.clone(), refusals[0].to_owned()))
                })
                .collect()),
        }
    }

    /// The pipelines whose newest run has a failed or rejected line for a cause outside its source
    /// ([`OUTSIDE_CAUSE`]), with that run and the newest such line's words (T-5687): the run queue
    /// retries them with backoff.
    pub async fn outside_failures(&self) -> Result<BTreeMap<Key, (String, String)>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query_as::<_, (String, String, String, String)>(
                "WITH newest AS (SELECT DISTINCT ON (project, pipeline) project, pipeline, run, \
                 rejected + failed AS problems FROM pipeline_runs ORDER BY project, pipeline, last_at DESC) \
                 SELECT n.project, n.pipeline, n.run, m.message FROM newest n \
                 JOIN LATERAL (SELECT message FROM pipeline_log g WHERE g.project = n.project \
                 AND g.pipeline = n.pipeline AND g.run = n.run AND g.outcome <> 'sent' \
                 AND g.message ~ $1 ORDER BY g.id DESC LIMIT 1) m ON true WHERE n.problems > 0",
            )
            .bind(OUTSIDE_CAUSE)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(project, pipeline, run, message)| ((project, pipeline), (run, message)))
            .collect()),
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(|(key, memory)| {
                    let run = memory.runs.iter().max_by(|a, b| a.last_at.cmp(&b.last_at))?;
                    let line = memory.lines.iter().rev().find(|l| {
                        l.run == run.run
                            && l.outcome != Outcome::Sent
                            && OUTSIDE.is_match(&l.message)
                    })?;
                    Some((key.clone(), (run.run.clone(), line.message.clone())))
                })
                .collect()),
        }
    }

    /// The newest failed or rejected line of the pipeline's log, with when it was taken (T-5048).
    pub async fn last_problem(
        &self,
        project: &str,
        pipeline: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query_as::<_, (String, time::OffsetDateTime)>(
                "SELECT message, at FROM pipeline_log WHERE project = $1 AND pipeline = $2 \
                 AND outcome <> 'sent' ORDER BY id DESC LIMIT 1",
            )
            .bind(project)
            .bind(pipeline)
            .fetch_optional(pool)
            .await?
            .map(|(message, at)| (message, rfc3339(at)))),
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(project.to_owned(), pipeline.to_owned()))
                .and_then(|memory| {
                    memory
                        .lines
                        .iter()
                        .rev()
                        .find(|l| l.outcome != Outcome::Sent)
                        .map(|l| (l.message.clone(), l.at.clone()))
                })),
        }
    }

    /// One run's log lines, newest first, at most `limit`, older than `before` when given.
    pub async fn lines(
        &self,
        project: &str,
        pipeline: &str,
        run: &str,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<LogLine>, sqlx::Error> {
        let limit = limit.clamp(1, LINES_KEPT);
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<LineRow> = sqlx::query_as(
                    "SELECT id, at, run, record_id, step, outcome, message FROM pipeline_log \
                     WHERE project = $1 AND pipeline = $2 AND run = $3 \
                     AND ($4::bigint IS NULL OR id < $4) ORDER BY id DESC LIMIT $5",
                )
                .bind(project)
                .bind(pipeline)
                .bind(run)
                .bind(before)
                .bind(limit as i64)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|(id, at, run, record_id, step, outcome, message)| LogLine {
                        id,
                        at: rfc3339(at),
                        run,
                        record_id,
                        step,
                        outcome: Outcome::parse(&outcome),
                        message,
                    })
                    .collect())
            }
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(project.to_owned(), pipeline.to_owned()))
                .map(|memory| {
                    memory
                        .lines
                        .iter()
                        .rev()
                        .filter(|l| l.run == run && before.is_none_or(|b| l.id < b))
                        .take(limit)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, outcome: Outcome, message: &str) -> NewLine {
        NewLine {
            record_id: id.to_owned(),
            step: None,
            outcome,
            message: message.to_owned(),
        }
    }

    /// T-5648: the outcome sink sends what a run refused in its body; message metadata (the
    /// `jc_refused_*` items, a source's response headers) never becomes a request header, so
    /// an outcome carrying a long refusal still fits the Portal hop's header limit.
    #[test]
    fn the_outcome_sink_sends_no_message_metadata_as_headers() {
        let rendered = with_outcomes(serde_json::json!({ "drop": {} }), "http://portal/outcomes");
        let client = &rendered["broker"]["outputs"][1]["fallback"][0]["http_client"];
        assert_eq!(client["url"], "http://portal/outcomes");
        assert_eq!(
            client["headers"],
            serde_json::json!({ "Content-Type": "application/json" })
        );
        assert_eq!(client.get("metadata"), None);
    }

    /// T-5687: a refusal by the model, a policy, the gateway or the broker may clear by itself; a
    /// source that refused, a malformed record or a step that threw does not.
    #[test]
    fn only_causes_outside_the_source_count_as_outside() {
        for outside in [
            "entity type `Tree` is not a class of the space's data model `praha-mesto` (DM-61)",
            "Access Denied by Policy",
            "the write failed: HTTP request returned unexpected response code (403): 403 Forbidden",
            "the write failed: HTTP request returned unexpected response code (503): 503 Service Unavailable",
            "the write failed: HTTP request returned unexpected response code (429): 429 Too Many Requests",
        ] {
            assert!(OUTSIDE.is_match(outside), "{outside}");
        }
        for source in [
            "HTTP request returned unexpected response code (404): 404 Not Found",
            "HTTP request returned unexpected response code (503): 503 Service Unavailable",
            "the write failed: HTTP request returned unexpected response code (400): 400 Bad Request",
            "attribute `height` is not a number",
            "access revoked by city.example on 2026-10-20",
        ] {
            assert!(!OUTSIDE.is_match(source), "{source}");
        }
    }

    /// Only the newest run counts: a pipeline whose later run wrote clean, or failed at its
    /// source, is not listed.
    #[tokio::test]
    async fn outside_failures_lists_the_newest_run_refused_for_an_outside_cause() {
        let store = LogStore::new(None);
        let dm61 = "entity type `Tree` is not a class of the space's data model `m` (DM-61)";
        for (pipeline, runs) in [
            (
                "trees",
                vec![("2026-10-25T10:00Z", "ok"), ("2026-10-25T11:00Z", dm61)],
            ),
            (
                "healed",
                vec![("2026-10-25T10:00Z", dm61), ("2026-10-25T11:00Z", "ok")],
            ),
            (
                "source",
                vec![(
                    "2026-10-25T11:00Z",
                    "HTTP request returned unexpected response code (404)",
                )],
            ),
        ] {
            for (run, said) in runs {
                let outcome = if said == "ok" {
                    Outcome::Sent
                } else {
                    Outcome::Failed
                };
                let said = if said == "ok" { "" } else { said };
                store
                    .append("praha", pipeline, run, &[line("urn:a", outcome, said)])
                    .await
                    .expect("append");
            }
        }
        let failed = store.outside_failures().await.expect("read");
        assert_eq!(
            failed,
            [(
                ("praha".to_owned(), "trees".to_owned()),
                ("2026-10-25T11:00Z".to_owned(), dm61.to_owned())
            )]
            .into()
        );
    }

    /// T-3001, PL-62: a clocked stream reports each pass before it reads, and a report that
    /// fails never drops the tick it was made for.
    #[test]
    fn a_clocked_stream_reports_every_pass_and_a_resident_one_is_left_alone() {
        let url = "http://portal-internal:8081/internal/pipelines/helsinki/reaper/outcomes";
        let mut clocked = serde_json::json!({
            "input": { "generate": { "interval": "60s", "mapping": TICK } },
            "pipeline": { "processors": [{ "try": [{ "http": {} }] }] },
            "output": { "drop": {} },
        });
        with_passes(&mut clocked, url);
        let processors = clocked["pipeline"]["processors"]
            .as_array()
            .expect("processors");
        assert_eq!(processors.len(), 3, "{processors:?}");
        let branch = &processors[0]["branch"];
        assert_eq!(branch["processors"][0]["http"]["url"], url);
        assert_eq!(branch["processors"][0]["http"]["verb"], "POST");
        let request = branch["request_map"].as_str().expect("a request map");
        assert!(
            request.contains(RUN) && request.contains("\"sent\": []"),
            "{request}"
        );
        assert!(
            branch.get("result_map").is_none(),
            "the tick goes on as it was"
        );
        assert_eq!(processors[1], serde_json::json!({ "catch": [] }));
        assert!(
            processors[2].get("try").is_some(),
            "the read follows the report"
        );

        let mut resident = serde_json::json!({
            "input": { "nats": { "urls": ["nats://demo-nats:4222"], "subject": "x" } },
            "pipeline": { "processors": [{ "mapping": "root = this" }] },
            "output": { "drop": {} },
        });
        let before = resident.clone();
        with_passes(&mut resident, url);
        assert_eq!(
            resident, before,
            "a stream without a clock has no pass to report"
        );
    }

    /// T-4269: a pass report is retried, so one refused or slow POST at a stream's first tick
    /// does not leave a 24 h clock without a run for a day; a report that carries lines adds to
    /// the run's counts and is never repeated.
    #[test]
    fn a_pass_report_is_retried_and_a_counting_report_is_sent_once() {
        let url = "http://portal/outcomes";
        let mut clocked = serde_json::json!({
            "input": { "generate": { "interval": "24h", "mapping": TICK } },
            "output": { "drop": {} },
        });
        with_passes(&mut clocked, url);
        let pass = &clocked["pipeline"]["processors"][0]["branch"]["processors"][0]["http"];
        assert!(pass["retries"].as_u64().is_some_and(|n| n >= 1), "{pass}");
        assert!(pass["retry_period"].is_string(), "{pass}");

        let failure = source_failure(url);
        let report = &failure["switch"][0]["processors"][0]["branch"]["processors"][0]["http"];
        assert_eq!(report["retries"], 0, "a failed line counts once: {report}");
        let sink = with_outcomes(serde_json::json!({ "drop": {} }), url);
        let outcome = &sink["broker"]["outputs"][1]["fallback"][0]["http_client"];
        assert_eq!(outcome["retries"], 0, "sent ids count once: {outcome}");
    }

    /// T-4036: the sink keeps its own in-flight window. `drop_on` sends one message and waits
    /// for its answer before the next, which capped a stream of single records at one outcome
    /// per round trip; a failed outcome still ends in `drop`, never back at the write.
    #[test]
    fn a_failed_outcome_falls_through_to_drop_without_a_one_at_a_time_wrapper() {
        let stream = with_outcomes(serde_json::json!({ "drop": {} }), "http://portal/outcomes");
        let sink = &stream["broker"]["outputs"][1];
        assert!(sink.get("drop_on").is_none(), "{sink}");
        assert_eq!(
            sink["fallback"][0]["http_client"]["url"],
            "http://portal/outcomes"
        );
        assert!(sink["fallback"][0]["http_client"]
            .get("max_in_flight")
            .is_none_or(|n| n.as_u64() > Some(1)));
        assert!(sink["fallback"][1].get("drop").is_some(), "{sink}");
        assert_eq!(sink["fallback"].as_array().map(Vec::len), Some(2));
    }

    /// T-4036: one report per write held the Portal's route to 1.4 s and the stream to 45 writes
    /// a second; the sink gathers reports, merges them per run and cuts them at the route's
    /// limit. T-4062: what it gathers is each write's report, never the write's records.
    #[test]
    fn the_sink_gathers_each_write_s_report_and_merges_them_per_run() {
        let stream = with_outcomes(serde_json::json!({ "drop": {} }), "http://portal/outcomes");
        let sink = &stream["broker"]["outputs"][1];
        let before = sink["processors"].as_array().expect("the sink's own steps");
        assert_eq!(
            before.len(),
            1,
            "one report per write, nothing split: {sink}"
        );
        assert!(before[0]["mapping"]
            .as_str()
            .is_some_and(|m| m.contains(r#""sent": $written.map_each"#)));
        let batching = &sink["fallback"][0]["http_client"]["batching"];
        assert_eq!(batching["count"], REPORTS_GATHERED);
        assert_eq!(batching["period"], "500ms");
        let steps: Vec<&str> = batching["processors"]
            .as_array()
            .expect("the batch's steps")
            .iter()
            .filter_map(|step| step.as_object()?.keys().find(|key| *key != "label"))
            .map(String::as_str)
            .collect();
        assert_eq!(steps, ["group_by_value", "archive", "mapping", "unarchive"]);
        assert_eq!(
            batching["processors"][0]["group_by_value"]["value"],
            r#"${! json("run") }"#
        );
        let chunks = batching["processors"][2]["mapping"]
            .as_str()
            .expect("the merge");
        assert!(
            chunks.contains(&format!("$sent.slice(i * {BATCH_LINES}")),
            "{chunks}"
        );
        // Bento's `range` with a step drops a last partial step (1.22.0: range(0, 5, 2) is
        // [0, 2]), so the chunks are counted with a step of one.
        assert!(!chunks.contains(&format!(", {BATCH_LINES})")), "{chunks}");
        // Bento refuses an empty `range(0, 0)`, so a run with no refused record, the usual one,
        // threw here and reached the Portal with no report at all (dev, 2026-10-16).
        assert!(chunks.contains("if $f == 0 { [] } else {"), "{chunks}");
    }

    /// T-4036: every part of the sink is labelled, so none of its counters is the stream's.
    #[test]
    fn every_part_of_the_sink_carries_one_of_its_labels() {
        let stream = with_outcomes(serde_json::json!({ "drop": {} }), "http://portal/outcomes");
        let sink = &stream["broker"]["outputs"][1];
        let parts: Vec<&serde_json::Value> = std::iter::once(sink)
            .chain(sink["fallback"].as_array().into_iter().flatten())
            .chain(sink["processors"].as_array().into_iter().flatten())
            .chain(
                sink.pointer("/fallback/0/http_client/batching/processors")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .collect();
        let mut labels: Vec<&str> = parts
            .iter()
            .filter_map(|part| part["label"].as_str())
            .collect();
        assert_eq!(labels.len(), parts.len(), "{sink}");
        let mut expected = SINK_LABELS.to_vec();
        expected.sort_unstable();
        labels.sort_unstable();
        assert_eq!(labels, expected);
    }

    /// T-3304: the sink sends the hash of each record beside its id, so the Portal can tell a
    /// record that changed from one that did not; the mapping is the one Bento runs.
    #[test]
    fn the_outcome_sink_sends_a_hash_beside_each_id() {
        let stream = with_outcomes(serde_json::json!({ "drop": {} }), "http://portal/outcomes");
        let mapping = stream
            .pointer("/broker/outputs/1/processors/0/mapping")
            .and_then(serde_json::Value::as_str)
            .expect("the sink's mapping");
        println!("{mapping}");
        assert!(
            mapping.contains(r#""sent": $written.map_each(record -> record.id.or(""))"#),
            "{mapping}"
        );
        assert!(
            mapping.contains(r#""hashes": $written.map_each(record -> record.format_json().hash("xxhash64").string())"#),
            "{mapping}"
        );
    }

    #[tokio::test]
    async fn a_run_counts_its_outcomes_and_its_log_is_its_own_pipelines_only() {
        let store = LogStore::new(None);
        store
            .append(
                "helsinki",
                "stations",
                "2026-09-25T08:00Z",
                &[
                    line("urn:a", Outcome::Sent, ""),
                    line("urn:b", Outcome::Sent, ""),
                    line("urn:c", Outcome::Rejected, "capacity is not an integer"),
                ],
            )
            .await
            .expect("kept");
        store
            .append(
                "helsinki",
                "stations",
                "2026-09-25T08:00Z",
                &[line("urn:d", Outcome::Failed, "step 2 threw")],
            )
            .await
            .expect("kept");
        store
            .append(
                "helsinki",
                "other",
                "2026-09-25T08:00Z",
                &[line("urn:other", Outcome::Sent, "")],
            )
            .await
            .expect("kept");

        let runs = store.runs("helsinki", "stations", 10).await.expect("runs");
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].sent, runs[0].rejected, runs[0].failed), (2, 1, 1));
        let lines = store
            .lines("helsinki", "stations", "2026-09-25T08:00Z", 10, None)
            .await
            .expect("lines");
        let ids: Vec<&str> = lines.iter().map(|l| l.record_id.as_str()).collect();
        assert_eq!(ids, ["urn:d", "urn:c", "urn:b", "urn:a"], "newest first");
        assert!(store
            .lines("other", "stations", "2026-09-25T08:00Z", 10, None)
            .await
            .expect("lines")
            .is_empty());
    }

    /// T-4421: a pass that wrote nothing still counts as a run, so the queue does not run the
    /// pipeline again as if it were new.
    #[tokio::test]
    async fn ran_lists_every_pipeline_with_a_run_and_only_those() {
        let store = LogStore::new(None);
        assert!(store.ran().await.expect("read").is_empty());
        store
            .append("slovensko", "susr-as", "2026-10-17T17:00Z", &[])
            .await
            .expect("append");
        let ran = store.ran().await.expect("read");
        let keys: Vec<&Key> = ran.keys().collect();
        assert_eq!(keys, [&("slovensko".to_owned(), "susr-as".to_owned())]);
        // T-5778: with the time of its last line, which the queue reads for a missed due time.
        let last = ran[&("slovensko".to_owned(), "susr-as".to_owned())];
        assert!((chrono::Utc::now() - last).num_seconds().abs() < 60);
    }

    /// T-5825: a run's stream hash is kept only for a run the log holds, and the queue reads the
    /// newest run's: a newer run logged without one hides the older hash.
    #[tokio::test]
    async fn ran_streams_name_the_newest_runs_hash_and_only_for_a_logged_run() {
        let store = LogStore::new(None);
        let key = ("helsinki".to_owned(), "trees".to_owned());
        store
            .ran_stream("helsinki", "trees", "r0", "never-logged")
            .await
            .expect("keep");
        assert!(store.ran_streams().await.expect("read").is_empty());
        store
            .append("helsinki", "trees", "r1", &[])
            .await
            .expect("append");
        store
            .ran_stream("helsinki", "trees", "r1", "h1")
            .await
            .expect("keep");
        assert_eq!(store.ran_streams().await.expect("read")[&key], "h1");
        store
            .append("helsinki", "trees", "r2", &[])
            .await
            .expect("append");
        assert!(store.ran_streams().await.expect("read").is_empty());
    }

    /// T-5048: the health tab shows the newest failed or rejected line, never a sent one.
    #[tokio::test]
    async fn last_problem_is_the_newest_line_that_did_not_send() {
        let store = LogStore::new(None);
        assert_eq!(store.last_problem("p", "x").await.expect("read"), None);
        store
            .append(
                "p",
                "x",
                "r1",
                &[
                    line("urn:1", Outcome::Failed, "timeout"),
                    line("urn:2", Outcome::Rejected, "schema"),
                    line("urn:3", Outcome::Sent, "ok"),
                ],
            )
            .await
            .expect("append");
        let (message, at) = store
            .last_problem("p", "x")
            .await
            .expect("read")
            .expect("a problem");
        assert_eq!(message, "schema");
        assert!(!at.is_empty());
        assert_eq!(store.last_problem("p", "other").await.expect("read"), None);
    }

    /// PL-93 (T-4882): three runs in a row refused for an ended grant make a streak; a run between
    /// them that was not refused breaks it.
    #[tokio::test]
    async fn three_refused_runs_in_a_row_are_a_streak() {
        let store = LogStore::new(None);
        let refused = "access revoked by anna on 2026-10-21; the gateway answered: 403";
        for (pipeline, runs) in [("cut", ["r1", "r2", "r3"]), ("broken", ["r1", "r2", "r3"])] {
            for run in runs {
                let message = if pipeline == "broken" && run == "r2" {
                    "timeout"
                } else {
                    refused
                };
                store
                    .append("p", pipeline, run, &[line("", Outcome::Failed, message)])
                    .await
                    .expect("append");
            }
        }
        store
            .append("p", "short", "r1", &[line("", Outcome::Failed, refused)])
            .await
            .expect("append");
        let streaks = store.refused_streaks(3).await.expect("read");
        assert_eq!(streaks.len(), 1, "{streaks:?}");
        assert_eq!(
            streaks
                .get(&("p".to_owned(), "cut".to_owned()))
                .map(String::as_str),
            Some(refused)
        );
    }

    #[tokio::test]
    async fn lines_are_paged_bounded_and_the_counts_outlive_them() {
        let store = LogStore::new(None);
        let many: Vec<NewLine> = (0..LINES_KEPT + 10)
            .map(|n| line(&format!("urn:{n}"), Outcome::Sent, ""))
            .collect();
        store.append("p", "x", "r1", &many).await.expect("kept");
        let first = store.lines("p", "x", "r1", 3, None).await.expect("page");
        assert_eq!(first.len(), 3);
        let next = store
            .lines("p", "x", "r1", 3, Some(first[2].id))
            .await
            .expect("page");
        assert!(next[0].id < first[2].id);
        let all = store
            .lines("p", "x", "r1", LINES_KEPT, None)
            .await
            .expect("all");
        assert_eq!(all.len(), LINES_KEPT);
        assert_eq!(
            store.runs("p", "x", 1).await.expect("runs")[0].sent,
            (LINES_KEPT + 10) as u64,
            "the count is the run's, not the kept lines'"
        );

        for n in 0..RUNS_KEPT + 5 {
            store
                .append(
                    "p",
                    "x",
                    &format!("r{n}"),
                    &[line("urn", Outcome::Sent, "")],
                )
                .await
                .expect("kept");
        }
        assert_eq!(
            store.runs("p", "x", RUNS_KEPT).await.expect("runs").len(),
            RUNS_KEPT
        );
    }

    #[tokio::test]
    async fn a_credential_in_an_id_or_a_message_is_masked() {
        let jwt = [
            "eyJhbGciOiJIUzI1NiJ9",
            "eyJzdWIiOiJ4In0",
            "c2lnbmF0dXJlLXBhcnQ",
        ]
        .join(".");
        let store = LogStore::new(None);
        store
            .append(
                "p",
                "x",
                "r",
                &[line(
                    &jwt,
                    Outcome::Failed,
                    &format!("the source answered 401 to Bearer {}", "abc123def456"),
                )],
            )
            .await
            .expect("kept");
        let kept = &store.lines("p", "x", "r", 1, None).await.expect("line")[0];
        assert_eq!(kept.record_id, crate::pipeline_outcomes::MASK);
        assert!(!kept.message.contains("abc123def456"), "{}", kept.message);
    }

    #[tokio::test]
    async fn a_pipeline_that_is_gone_takes_its_runs_and_log_with_it() {
        let store = LogStore::new(None);
        for (project, pipeline) in [("p", "kept"), ("p", "deleted"), ("q", "unread")] {
            store
                .append(project, pipeline, "r", &[line("urn", Outcome::Sent, "")])
                .await
                .expect("kept");
        }
        // `q` did not stage this sync: none of its pipelines is in the mirror, and it keeps them.
        let loaded = BTreeMap::from([("p".to_owned(), HashSet::from(["kept".to_owned()]))]);
        store.forget_gone(&loaded).await.expect("forgotten");
        assert_eq!(store.runs("q", "unread", 10).await.expect("runs").len(), 1);
        assert_eq!(store.runs("p", "kept", 10).await.expect("runs").len(), 1);
        assert!(store
            .runs("p", "deleted", 10)
            .await
            .expect("runs")
            .is_empty());
        assert!(store
            .lines("p", "deleted", "r", 10, None)
            .await
            .expect("lines")
            .is_empty());
    }

    /// The database trims a pipeline's log on its first write, then once a minute or per
    /// [`TRIM_EVERY_LINES`] lines, each pipeline on its own clock (T-5292).
    #[test]
    fn the_trim_runs_at_most_once_a_minute_or_per_500_lines() {
        let mut trims = Trims::default();
        let (x, y) = (
            ("p".to_owned(), "x".to_owned()),
            ("p".to_owned(), "y".to_owned()),
        );
        let start = Instant::now();
        assert!(
            trims.due(&x, 10, start),
            "a restart trims on its first write"
        );
        let mut trimmed = 0;
        for second in 1..60 {
            trimmed += usize::from(trims.due(&x, 1, start + Duration::from_secs(second)));
        }
        assert_eq!(trimmed, 0, "59 batches of one line within the minute");
        assert!(trims.due(&x, 1, start + TRIM_EVERY));
        assert!(trims.due(&y, 1, start + TRIM_EVERY), "y has its own clock");
        assert!(!trims.due(&x, TRIM_EVERY_LINES - 1, start + TRIM_EVERY));
        assert!(
            trims.due(&x, 1, start + TRIM_EVERY),
            "500 lines since the last trim"
        );
        // 3,000 batches of 10 lines, 20 ms apart, trim per 500 lines: 60 times, not 3,000.
        let mut trims = Trims::default();
        let trims_run = (0..3000u64)
            .filter(|n| trims.due(&x, 10, start + Duration::from_millis(n * 20)))
            .count();
        assert_eq!(trims_run, 60);
    }

    #[test]
    fn a_run_name_is_short_and_plain_or_the_current_hour() {
        assert_eq!(
            run_name(Some("2026-09-25T08:15:00Z")),
            "2026-09-25T08:15:00Z"
        );
        for bad in [None, Some(""), Some("<script>"), Some(&"9".repeat(41)[..])] {
            let hour = run_name(bad);
            assert!(hour.ends_with(":00Z") && hour.len() == 17, "{hour}");
        }
    }
}
