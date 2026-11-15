//! The workbench's whole-source check (G-21, T-7085): the saved pipeline over every page of its
//! source, as a dry run. The source's own read walks its pages (G-07), the pipeline's steps run
//! on each record as an ephemeral stream on the project's runner, and every record the steps make
//! comes back to the capture route instead of the gateway. The Portal checks each one as the
//! rejected route checks a record the stage refused, so the counts per rule are the ones a live
//! run's rejected list would hold. Nothing is written; the run holds the project's test slot,
//! presents the harness principal, and ends at 60 s or 50 000 records.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::data_source::{DataSourceSpec, DataSourceType};
use jc_core::kinds::PipelineSpec;
use jcctl::pipeline_test::{Captured, Sample, SampleFormat, TestError, FETCH_FAILED};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use utoipa::ToSchema;

use crate::api::internal::pipeline_rejected::{problems_of, reason_of};
use crate::api::pipeline_test::{
    harnessed, outcome_of, run_stream_gathering, with_lifetime, Gathered,
};
use crate::api::pipelines::{group_views, readable_pipeline, RejectedGroupView};
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::pipeline_outcomes::{mask, now, Reason, Rejected, RejectedGroup, EXAMPLES, KEPT};
use crate::pipeline_validation::Problem;
use crate::state::AppState;

/// The longest a check runs, from the stream's create.
const DEADLINE: Duration = Duration::from_secs(60);
/// The records a check reads at most; the one past them ends it.
pub(crate) const RECORDS: usize = 50_000;
/// Silence after the last record that ends the check: the source's walk hands its pages on
/// together, so a gap this long means the stream has nothing left.
const QUIET: Duration = Duration::from_secs(5);
/// Records the runner posts back at once: one at a time, 50 000 would outlast the deadline.
const IN_FLIGHT: u32 = 16;

/// What the whole-source check found (G-21).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceCheck {
    /// Records the steps made, each checked: `accepted` plus `rejected`.
    pub records: u64,
    /// Records the space's model takes.
    pub accepted: u64,
    /// Records a live run would put on the rejected list.
    pub rejected: u64,
    /// Whether the whole source was read: `false` when 60 s or 50 000 records ended the check
    /// first, or the source's read failed, so the counts cover part of it.
    pub complete: bool,
    /// The rejected records counted by rule and attribute, the largest first, each with its
    /// first three records, where the fix belongs and, for an enum, the values the model allows.
    pub groups: Vec<RejectedGroupView>,
    /// The first 1000 rejected records, secrets masked; `id` is the record's place in the run.
    pub rows: Vec<Rejected>,
    /// What stopped the source or the runner, in words a person can act on.
    pub errors: Vec<String>,
}

/// The counts while the records arrive.
#[derive(Debug, Default)]
struct Tally {
    accepted: u64,
    rejected: u64,
    /// Each group with the place it first appeared, so equal counts keep that order.
    groups: HashMap<(String, String), (usize, RejectedGroup)>,
    rows: Vec<Rejected>,
    errors: Vec<String>,
    /// A limit ended the check before the stream went quiet.
    limited: bool,
}

impl Gathered for Tally {
    fn refused(errors: Vec<TestError>) -> Self {
        let mut tally = Tally::default();
        for error in errors {
            tally.runner_error(error);
        }
        tally
    }

    fn runner_error(&mut self, error: TestError) {
        let words = match error.step {
            Some(step) => format!("step {}: {}", step + 1, error.message),
            None => error.message,
        };
        self.error(words);
    }
}

impl Tally {
    fn records(&self) -> u64 {
        self.accepted + self.rejected
    }

    fn error(&mut self, words: String) {
        if !self.errors.contains(&words) {
            self.errors.push(words);
        }
    }

    /// One message the harness posted: a record the steps made, checked by `check`, or one that
    /// failed in a step, listed as the rejected route lists it, or the source's own failure.
    fn add(&mut self, message: Captured, check: &impl Fn(&Value) -> Vec<Problem>) {
        let step: Option<i64> = message.step.as_deref().and_then(|at| at.parse().ok());
        match (message.error.as_deref(), message.output.as_ref()) {
            (Some(error), _) if error.starts_with(FETCH_FAILED) => self.error(outcome_of(error)),
            (Some(error), _) => {
                let record = failed_record(&message, step);
                let (reason, _) = reason_of(&check(&record), Some(error), step);
                self.reject(record, reason);
            }
            (None, None | Some(Value::Null)) => {}
            (None, Some(record)) => match check(record).as_slice() {
                [] => self.accepted += 1,
                problems => {
                    let (reason, _) = reason_of(problems, None, None);
                    self.reject(record.clone(), reason);
                }
            },
        }
    }

    fn reject(&mut self, record: Value, reason: Reason) {
        self.rejected += 1;
        let row = Rejected {
            id: i64::try_from(self.records()).unwrap_or(i64::MAX),
            at: now(),
            record: mask(&record),
            rule: reason.rule,
            path: reason.path,
            message: reason.message,
            step: reason.step,
            run: None,
        };
        let order = self.groups.len();
        let (_, group) = self
            .groups
            .entry((row.rule.clone(), row.path.clone()))
            .or_insert_with(|| {
                (
                    order,
                    RejectedGroup {
                        rule: row.rule.clone(),
                        path: row.path.clone(),
                        count: 0,
                        examples: Vec::new(),
                    },
                )
            });
        group.count += 1;
        if group.examples.len() < EXAMPLES {
            group.examples.push(row.clone());
        }
        if self.rows.len() < KEPT {
            self.rows.push(row);
        }
    }

    /// The groups, the largest first, equal ones in the order they appeared.
    fn groups(&mut self) -> Vec<RejectedGroup> {
        let mut groups: Vec<_> = self.groups.drain().map(|(_, group)| group).collect();
        groups.sort_by(|(a, x), (b, y)| y.count.cmp(&x.count).then(a.cmp(b)));
        groups.into_iter().map(|(_, group)| group).collect()
    }
}

/// The record a step failed on, as the stage's catch posts it live: what the step before made of
/// it, else what the steps were given.
/// ponytail: a step's snapshot is cut at 4 KiB, so a larger record falls back to its input.
fn failed_record(message: &Captured, step: Option<i64>) -> Value {
    let parsed = |text: &str| serde_json::from_str(text).ok();
    let before = step
        .and_then(|at| usize::try_from(at).ok()?.checked_sub(1))
        .and_then(|at| message.steps.as_ref()?.get(at)?.as_deref().and_then(parsed));
    before
        .or_else(|| message.input.as_deref().and_then(parsed))
        .unwrap_or_else(|| Value::String(message.input.clone().unwrap_or_default()))
}

/// Reads what the harness posts until the stream is quiet, the deadline, or [`RECORDS`].
async fn gather(
    receiver: &mut mpsc::UnboundedReceiver<Captured>,
    check: impl Fn(&Value) -> Vec<Problem>,
) -> Tally {
    let mut tally = Tally::default();
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut heard = false;
    loop {
        let wait = if heard {
            (tokio::time::Instant::now() + QUIET).min(deadline)
        } else {
            deadline
        };
        match tokio::time::timeout_at(wait, receiver.recv()).await {
            Ok(Some(message)) => {
                heard = true;
                tally.add(message, &check);
                if tally.records() >= RECORDS as u64 {
                    tally.limited = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => {
                tally.limited = tokio::time::Instant::now() >= deadline;
                break;
            }
        }
    }
    if !heard {
        tally.error(format!(
            "the source sent nothing within {} s: its pages did not all arrive in time, or a step dropped every record; check the source's paging (maxPages, delay) and try again",
            DEADLINE.as_secs()
        ));
    } else if tally.limited {
        tally.error(format!(
            "the check stopped at {} s or {RECORDS} records, so the counts cover the part of the source read by then",
            DEADLINE.as_secs()
        ));
    }
    tally
}

/// The runner configuration of the check: the pipeline's harness, with the source's own read
/// (its paging walked) in place of a sample, every record kept, and each record posted back
/// without the input and step samples it carries for a test, unless it failed.
fn whole_source(
    state: &AppState,
    project: &str,
    spec: &PipelineSpec,
    source_name: &str,
    source: &DataSourceSpec,
    capture: &str,
) -> Result<Value, ApiError> {
    let sample = Sample {
        text: Some(String::new()),
        url: None,
        format: SampleFormat::Text,
        records: None,
    };
    let mut config = harnessed(&state.mirror, project, spec, &sample, capture, RECORDS)?;
    with_lifetime(&mut config, spec)?;
    let (input, mut read) =
        crate::api::datasource_test::source_read(source_name, project, "source_check", source)?;
    // The read's own error is kept where the envelope reports it first, as a test's fetch.
    read.push(json!({ "catch": [{ "mapping": format!(
        "meta jc_fetch_error = \"{FETCH_FAILED}\" + error()\nroot = \"\""
    ) }] }));
    let processors = config
        .pointer_mut("/pipeline/processors")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| ApiError::Internal("the harness has no processors".into()))?;
    processors.splice(0..0, read);
    processors.push(json!({ "mapping":
        "root = if this.error == null { this.without(\"input\", \"steps\") } else { this }"
    }));
    config["input"] = input;
    config["output"]["http_client"]["max_in_flight"] = json!(IN_FLIGHT);
    Ok(config)
}

/// The saved http DataSource the pipeline reads, or why the check cannot read it.
fn read_source(
    state: &AppState,
    project: &str,
    spec: &PipelineSpec,
) -> Result<(String, DataSourceSpec), ApiError> {
    let name = spec
        .sources()
        .iter()
        .find_map(|source| source.data_source_ref.as_ref())
        .filter(|reference| reference.namespace().is_none_or(|ns| ns == project))
        .map(|reference| reference.name().to_owned())
        .ok_or_else(|| {
            ApiError::Conflict(
                "the pipeline reads no DataSource of this project; the whole-source check reads a \
                 DataSource's pages, so use the workbench's sample for an Endpoint source"
                    .into(),
            )
        })?;
    let envelope = state
        .mirror
        .get(project, "DataSource", &name)
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "DataSource '{name}' is not in project '{project}'; save it first"
            ))
        })?;
    let source: DataSourceSpec = serde_json::from_value(envelope.spec.clone())
        .map_err(|e| ApiError::Conflict(format!("DataSource '{name}' does not parse: {e}")))?;
    if !matches!(source.source_type, DataSourceType::Http) {
        return Err(ApiError::Conflict(format!(
            "DataSource '{name}' streams without a last page; the whole-source check reads an \
             http source, so use the workbench's sample for this one"
        )));
    }
    Ok((name, source))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/pipelines/{name}/source-check",
    summary = "Check Whole Source",
    description = "Runs the saved pipeline over every page of its http DataSource as a dry run on the project's runner and counts what a live run would accept and reject, by rule and attribute, with the first 1000 rejected records (secrets masked). Nothing is written. Ends at 60 s or 50 000 records and holds the project's one pipeline test slot (G-21, G-07). Needs read on the pipeline.",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Pipeline name"),
    ),
    responses(
        (status = 200, description = "The counts", body = SourceCheck),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Pipeline not found", body = ProblemDetails),
        (status = 409, description = "The pipeline reads no saved http DataSource, or a test of this project is already running", body = ProblemDetails),
        (status = 503, description = "No pipeline runner is configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn check_source(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<SourceCheck>, ApiError> {
    let pipeline = readable_pipeline(&state, &user, &project, &name)?;
    let spec: PipelineSpec = serde_json::from_value(pipeline.spec.clone())
        .map_err(|e| ApiError::Conflict(format!("pipeline '{name}' does not parse: {e}")))?;
    let (source_name, source) = read_source(&state, &project, &spec)?;
    let check = |record: &Value| problems_of(&state, &project, &pipeline.spec, record);
    let mut tally = run_stream_gathering(
        &state,
        &project,
        |capture| whole_source(&state, &project, &spec, &source_name, &source, capture),
        async |receiver| gather(receiver, check).await,
    )
    .await?;
    let groups = tally.groups();
    Ok(Json(SourceCheck {
        records: tally.records(),
        accepted: tally.accepted,
        rejected: tally.rejected,
        complete: !tally.limited && tally.errors.is_empty(),
        groups: group_views(&state, &project, &pipeline.spec, groups),
        rows: tally.rows,
        errors: tally.errors,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/pipelines/{name}/source-check",
        post(check_source),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(rule: &str, path: &str) -> Problem {
        Problem {
            rule: rule.into(),
            path: path.into(),
            message: format!("{path} breaks {rule}"),
        }
    }

    fn message(output: Value) -> Captured {
        Captured {
            output: Some(output),
            ..Captured::default()
        }
    }

    /// The model's verdict per record, the first problem naming the group, a step's failure as
    /// `step` and the source's own failure as an error, not a record.
    #[test]
    fn each_record_is_counted_under_the_rule_the_rejected_route_would_list_it_under() {
        let check = |record: &Value| match record["status"].as_str() {
            Some("gone") => vec![problem("sh:in", "status"), problem("sh:closed", "x")],
            Some("bad") => vec![problem("sh:datatype", "capacity")],
            Some(_) => Vec::new(),
            None => Vec::new(),
        };
        let mut tally = Tally::default();
        for status in ["ok", "gone", "bad", "gone", "ok", "ok"] {
            tally.add(
                message(json!({ "status": status, "password": "hunter2hunter2" })),
                &check,
            );
        }
        tally.add(
            Captured {
                input: Some("{\"raw\":1}".into()),
                error: Some("cannot add string to number".into()),
                step: Some("0".into()),
                ..Captured::default()
            },
            &check,
        );
        tally.add(
            Captured {
                error: Some(format!(
                    "{FETCH_FAILED}http: unexpected response code (404): 404 Not Found"
                )),
                ..Captured::default()
            },
            &check,
        );
        assert_eq!((tally.accepted, tally.rejected), (3, 4));
        let groups: Vec<(String, String, u64)> = tally
            .groups()
            .into_iter()
            .map(|g| (g.rule, g.path, g.count))
            .collect();
        assert_eq!(
            groups,
            [
                ("sh:in".to_owned(), "status".to_owned(), 2),
                ("sh:datatype".to_owned(), "capacity".to_owned(), 1),
                ("step".to_owned(), String::new(), 1),
            ]
        );
        assert_eq!(tally.rows.len(), 4);
        assert_eq!(
            tally.rows[3].record,
            json!({ "raw": 1 }),
            "the step's input"
        );
        assert_eq!(tally.rows[3].step, Some(0));
        assert!(!format!("{:?}", tally.rows).contains("hunter2"), "masked");
        assert_eq!(tally.errors, ["the feed answered 404 Not Found"]);
    }

    /// The run ends when the stream goes quiet, at the record cap, or at the deadline, and says
    /// when a limit cut it.
    #[tokio::test(start_paused = true)]
    async fn the_check_ends_quiet_or_at_its_cap_and_says_a_cap_cut_it() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        for _ in 0..3 {
            sender.send(message(json!({ "id": "a" }))).expect("sent");
        }
        let tally = gather(&mut receiver, |_: &Value| Vec::new()).await;
        assert_eq!((tally.accepted, tally.limited), (3, false));
        assert!(tally.errors.is_empty(), "{:?}", tally.errors);

        for _ in 0..=RECORDS {
            sender.send(message(json!({ "id": "a" }))).expect("sent");
        }
        let tally = gather(&mut receiver, |_: &Value| Vec::new()).await;
        assert_eq!((tally.records(), tally.limited), (RECORDS as u64, true));
        assert_eq!(tally.errors.len(), 1, "{:?}", tally.errors);

        let mut silent = mpsc::unbounded_channel::<Captured>().1;
        let tally = gather(&mut silent, |_: &Value| Vec::new()).await;
        assert!(tally.errors[0].starts_with("the source sent nothing within 60 s"));
    }
}
