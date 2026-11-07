//! Running a Pipeline and reading its runs through the registry (T-6028, PL-62, PL-78).
//!
//! The run list, a run's log and run-now had routes and no operations, so an MCP connector could
//! test a mapping but never start the pipeline or ask why last night's run wrote nothing. Each
//! operation calls the route's own function; the grants and the refusals are the route's. A run
//! answers its id at once and never waits for the run; the log comes back a page at a time, cut
//! at [`LOG_BYTES`] with the cursor of the next page.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, NAME};
use super::runs::as_user;
use super::{parse_input, OpError, Operation, OperationAnnotations};
use crate::api::pipelines::{LogQuery, RunsQuery};
use crate::change::Lane;
use crate::pipeline_log::LogLine;

/// The most a log page carries, in bytes of its lines' JSON: well under a connector's answer
/// (`connectors::DEFAULT_LIMITS.result_bytes`), so a page always reaches the client whole.
pub const LOG_BYTES: usize = super::bounds::PAGE_BYTES;
/// The run list and log operation's name.
pub const LOGS: &str = "jc_pipeline_logs";
/// Lines read from the store per page, before the byte cut.
const LOG_LINES: usize = 200;
/// Runs listed when the call names no limit, and the most it may name.
const RUNS: usize = 10;
const MAX_RUNS: usize = 50;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunInput {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunsInput {
    pub name: String,
    /// The run whose log to read; the newest when absent.
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// The `next` of the previous page.
    #[serde(default)]
    pub cursor: Option<i64>,
    /// Only the lines of this step of `spec.steps` (T-6082).
    #[serde(default)]
    pub step: Option<i32>,
}

fn run_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "name": text("The Pipeline's name, e.g. counters-hourly", NAME) },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn run_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "run": { "type": "string", "description": "The run's id, as jc_pipeline_logs names it once it wrote" },
            "state": { "type": "string", "enum": ["queued"], "description": "The run waits its turn in the project's lane" }
        },
        "required": ["run"]
    })
}

fn runs_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("The Pipeline's name, e.g. counters-hourly", NAME),
            "run": text("The run, as runs[].run names it; the newest when absent", 64),
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RUNS, "description": "Runs listed (default 10)" },
            "cursor": { "type": "integer", "description": "The previous answer's next" },
            "step": { "type": "integer", "description": "This step only" }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn runs_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "runs": { "type": "array", "description": "The newest runs, each with the records it sent, the model rejected and a step failed" },
            "run": { "type": ["string", "null"], "description": "The run whose log lines follow; null when the pipeline never ran" },
            "lines": { "type": "array", "description": "The run's log, newest first: each record's id, the step it failed at, its outcome and what happened" },
            "next": { "type": "integer", "description": "Present when the log goes on: send it as cursor" }
        },
        "required": ["runs", "run", "lines"]
    })
}

/// `lines` (one store page of `asked` lines) cut to [`LOG_BYTES`], and the cursor of what follows:
/// the id of the last line kept, when lines were cut or the store may hold more.
fn page(mut lines: Vec<LogLine>, asked: usize) -> (Vec<LogLine>, Option<i64>) {
    let fetched = lines.len();
    let kept = super::bounds::fitting(&lines, LOG_BYTES);
    lines.truncate(kept);
    let more = kept < fetched || fetched == asked;
    let next = more.then(|| lines.last().map(|line| line.id)).flatten();
    (lines, next)
}

/// A store page of lines cut as [`page`] cuts it, only those of `step` when one is named. A page
/// the filter thinned still names the cursor past every line it read, so the next page goes on.
fn of_step(lines: Vec<LogLine>, step: Option<i32>) -> (Vec<LogLine>, Option<i64>) {
    let Some(step) = step else {
        return page(lines, LOG_LINES);
    };
    let read_on = (lines.len() == LOG_LINES)
        .then(|| lines.last().map(|line| line.id))
        .flatten();
    let kept = lines.into_iter().filter(|line| line.step == Some(step));
    let (kept, next) = page(kept.collect(), LOG_LINES);
    (kept, next.or(read_on))
}

async fn runs(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: RunsInput,
) -> Result<Value, OpError> {
    let path = (project.to_owned(), input.name.clone());
    let Json(list) = crate::api::pipelines::get_runs(
        as_user(caller),
        State(state.clone()),
        Path(path.clone()),
        Query(RunsQuery {
            limit: Some(input.limit.unwrap_or(RUNS).clamp(1, MAX_RUNS)),
        }),
    )
    .await?;
    let run = input
        .run
        .or_else(|| list.items.first().map(|r| r.run.clone()));
    let (lines, next) = match &run {
        Some(run) => {
            let Json(log) = crate::api::pipelines::get_run_log(
                as_user(caller),
                State(state.clone()),
                Path((path.0, path.1, run.clone())),
                Query(LogQuery {
                    limit: Some(LOG_LINES),
                    before: input.cursor,
                }),
            )
            .await?;
            of_step(log.items, input.step)
        }
        None => (Vec::new(), None),
    };
    let mut answer = json!({ "runs": list.items, "run": run, "lines": lines });
    if let Some(next) = next {
        answer["next"] = json!(next);
    }
    Ok(answer)
}

/// `jc_pipeline_inspect`: what the pipeline page shows of a running pipeline (JC2-32).
pub const INSPECT: &str = "jc_pipeline_inspect";
/// Rejected records one page lists when the call names no limit; the route takes up to 100.
const REJECTED: usize = 20;

/// Which of the pipeline page's panels to read.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum View {
    Samples,
    Stats,
    Rejected,
    SourceDeletes,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InspectInput {
    pub name: String,
    pub view: View,
    /// Rejected records per page.
    #[serde(default)]
    pub limit: Option<usize>,
    /// The `next` of the previous rejected page.
    #[serde(default)]
    pub cursor: Option<i64>,
}

fn inspect_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("The Pipeline's name, e.g. counters-hourly", NAME),
            "view": {
                "type": "string",
                "enum": ["samples", "stats", "rejected", "sourceDeletes"],
                "description": "samples: last message per node; stats: per-node rates over 1/5/15 min; rejected: records validation refused, with the rule; sourceDeletes: a snapshot pipeline's source ledger and delete dry run"
            },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "rejected per page (default 20)" },
            "cursor": { "type": "integer", "description": "rejected: the previous answer's next" }
        },
        "required": ["name", "view"],
        "additionalProperties": false
    })
}

fn inspect_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The panel as the pipeline page reads it; rejected pages carry next while older records follow"
    })
}

async fn inspect(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: InspectInput,
) -> Result<Value, OpError> {
    let user = || as_user(caller);
    let at = || Path((project.to_owned(), input.name.clone()));
    let api = || State(state.clone());
    use crate::api::pipelines as route;
    Ok(match input.view {
        View::Samples => serde_json::to_value(route::get_samples(user(), api(), at()).await?.0)?,
        View::Stats => serde_json::to_value(route::get_stats(user(), api(), at()).await?.0)?,
        View::SourceDeletes => {
            serde_json::to_value(route::get_source_deletes(user(), api(), at()).await?.0)?
        }
        View::Rejected => {
            let query = route::RejectedQuery {
                limit: Some(input.limit.unwrap_or(REJECTED).clamp(1, 100)),
                before: input.cursor,
            };
            serde_json::to_value(
                route::get_rejected(user(), api(), at(), Query(query))
                    .await?
                    .0,
            )?
        }
    })
}

/// This module's operations in the registry (`super::init_registry`).
pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_pipeline_run",
            title: "Run a Pipeline Now",
            description: "Queues one run of a queued pipeline now, outside its schedule, and answers its id; it waits its turn in its lane. 409 while its run is in the runner or for a pipeline fed by pushes. Needs propose and approve on Pipeline (a steward); each run is an activity event `pipeline.run`",
            input: run_input_schema,
            output: run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "Pipeline",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<RunInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: RunInput = parse_input(val)?;
                    let (_, Json(started)) = crate::api::pipelines::post_run(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.name)),
                    )
                    .await?;
                    // Queued, never awaited: jc_pipeline_logs reads it once it wrote.
                    Ok(json!({ "run": started.run, "state": "queued" }))
                })
            },
        },
        Operation {
            name: LOGS,
            title: "Read Pipeline Runs and Logs",
            description: "A pipeline's newest runs with what each sent, rejected and failed, and one run's log a page at a time: each record's outcome and why, by step if asked",
            input: runs_input_schema,
            output: runs_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<RunsInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { runs(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: INSPECT,
            title: "Inspect A Running Pipeline",
            description: "One panel of the pipeline page: the last message per node, per-node rates, the records validation refused, or a snapshot pipeline's source-delete dry run",
            input: inspect_input_schema,
            output: inspect_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<InspectInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { inspect(caller, state, project, parse_input(val)?).await })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline_log::Outcome;

    fn line(id: i64, message: &str) -> LogLine {
        LogLine {
            id,
            at: "2026-10-29T03:00:00Z".into(),
            run: "2026-10-29T03:00:00Z".into(),
            record_id: format!("c{id}"),
            step: None,
            outcome: Outcome::Failed,
            message: message.into(),
        }
    }

    #[test]
    fn a_log_over_the_byte_cap_is_cut_with_the_cursor_of_the_rest() {
        let long = "x".repeat(400);
        let lines: Vec<LogLine> = (0..120).rev().map(|id| line(id, &long)).collect();
        let (kept, next) = page(lines, LOG_LINES);
        let bytes: usize = kept
            .iter()
            .map(|l| serde_json::to_vec(l).map_or(0, |b| b.len()))
            .sum();
        assert!(bytes <= LOG_BYTES, "{bytes}");
        assert!(kept.len() < 120, "{}", kept.len());
        assert_eq!(next, kept.last().map(|l| l.id));
    }

    /// T-6082: a step filter keeps that step's lines and still pages past the lines it skipped.
    #[test]
    fn a_step_filter_keeps_its_steps_lines_and_reads_on_past_the_others() {
        let at = |id: i64, step: Option<i32>| LogLine {
            step,
            ..line(id, "m")
        };
        let lines = vec![at(3, Some(1)), at(2, Some(0)), at(1, None)];
        let (kept, next) = of_step(lines.clone(), Some(1));
        assert_eq!(kept.iter().map(|l| l.id).collect::<Vec<_>>(), [3]);
        assert_eq!(next, None, "a short store page has no more");
        assert_eq!(of_step(lines, None).0.len(), 3);
        let full: Vec<LogLine> = (0..LOG_LINES as i64)
            .rev()
            .map(|id| at(id, Some(0)))
            .collect();
        assert_eq!(
            of_step(full, Some(1)),
            (Vec::new(), Some(0)),
            "read on past a page of other steps"
        );
    }

    #[test]
    fn a_short_last_page_has_no_cursor_and_a_full_one_has() {
        let (kept, next) = page(vec![line(2, "a"), line(1, "b")], LOG_LINES);
        assert_eq!((kept.len(), next), (2, None));
        let (_, next) = page(vec![line(2, "a"), line(1, "b")], 2);
        assert_eq!(next, Some(1));
        assert_eq!(page(Vec::new(), LOG_LINES), (Vec::new(), None));
    }
}
