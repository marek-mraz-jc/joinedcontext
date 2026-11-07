//! The internal listener's rejected route (PL-61, ADR-N-034): a record the runner's validation
//! stage refused, posted once by the project's pipeline runner and kept on the pipeline's
//! rejected list.
//!
//! The runner says that a record failed; the Portal says why. The record is checked again against
//! the space's compiled model, the same check the workbench runs, so the rule on the list is the
//! one a person sees in the workbench and not the runner's wording. A record that passes that
//! check failed in one of the author's steps, and is kept with the step it failed at.
//!
//! Beside it, the outcome route: the lines the runner's outcome sink sends for a batch the
//! gateway took (PL-62). Each refused record also becomes a line of its run's log, `rejected` or
//! `failed`, so a run's log is the whole story of its records.

use crate::pipeline_log::{run_name, NewLine, Outcome};
use crate::pipeline_validation::Problem;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

/// A refused record is one entity; anything larger is not one the gateway would have taken.
const BODY_LIMIT: usize = 1024 * 1024;

/// How much of the runner's own error a kept record carries.
const ERROR_CHARS: usize = 500;

/// What the stage's catch posts.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refused {
    pub record: Value,
    #[serde(default)]
    pub error: Option<String>,
    /// The step the harness stamped, as text: Bento metadata is text.
    #[serde(default)]
    pub step: Option<Value>,
    /// The run the record belongs to (PL-62).
    #[serde(default)]
    pub run: Option<String>,
}

/// What the outcome sink posts for one batch the gateway took.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sent {
    #[serde(default)]
    pub run: Option<String>,
    /// The `id` of each record of the batch; a record without one is an empty string.
    pub sent: Vec<Value>,
    /// The hash of each record, beside its id (T-3304); empty from a stream rendered before.
    #[serde(default)]
    pub hashes: Vec<Value>,
    /// The records of the batch the gateway refused: an item of a `207`, or every record of a
    /// batch it answered `400`, `413` or `422` (T-4056).
    #[serde(default)]
    pub failed: Vec<Failed>,
}

/// One record the gateway refused, with its words for why.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failed {
    pub id: Value,
    #[serde(default)]
    pub message: String,
}

/// The largest batch the gateway takes, and so the most lines one report carries.
const BATCH: usize = 1000;

/// `POST /internal/pipelines/{project}/{name}/rejected`.
pub async fn rejected(
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if crate::auth::internal::authenticate_pipeline_of(&state, &headers, &project, &name)
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(refused) = serde_json::from_slice::<Refused>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    keep(&state, &project, &name, refused).await
}

/// Names the rule and keeps the record, once the caller is known.
pub(crate) async fn keep(
    state: &AppState,
    project: &str,
    name: &str,
    refused: Refused,
) -> StatusCode {
    let Some(pipeline) = state.mirror.get(project, "Pipeline", name) else {
        return StatusCode::NOT_FOUND;
    };
    let problems = target_space(state, project, &pipeline.spec)
        .and_then(|space| {
            let schema = state.model_schemas.get(project, &space)?;
            Some(schema.check(&refused.record))
        })
        .unwrap_or_default();
    let step = refused.step.as_ref().and_then(|step| match step {
        Value::Number(n) => n.as_i64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    });
    let (rule, path, message) = match problems.as_slice() {
        [first, ..] => (
            first.rule.clone(),
            first.path.clone(),
            problems
                .iter()
                .map(|Problem { message, .. }| message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ),
        [] => (
            if step.is_some() { "step" } else { "runner" }.to_owned(),
            String::new(),
            runner_error(refused.error.as_deref()),
        ),
    };
    let outcome = if problems.is_empty() {
        Outcome::Failed
    } else {
        Outcome::Rejected
    };
    let run = run_name(refused.run.as_deref());
    let reason = crate::pipeline_outcomes::Reason {
        rule,
        path,
        message,
        step: step.and_then(|s| i32::try_from(s).ok()),
        run: Some(run.clone()),
    };
    if outcome == Outcome::Rejected {
        let id = vec![record_id(&refused.record)];
        crate::pipeline_source_deletes::note_seen(state, project, name, &run, &id, false).await;
    }
    let line = NewLine {
        record_id: record_id(&refused.record),
        step: reason.step,
        outcome,
        message: reason.message.clone(),
    };
    let kept = state
        .rejected
        .reject(project, name, &refused.record, &reason)
        .await;
    let logged = match &kept {
        Ok(()) => {
            state
                .pipeline_log
                .append(project, name, &run, &[line])
                .await
        }
        Err(_) => Ok(()),
    };
    match kept.and(logged) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!(project, pipeline = name, %error, "a rejected record was not kept");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// `POST /internal/pipelines/{project}/{name}/outcomes`: one `sent` line per record of a batch
/// the gateway took.
pub async fn outcomes(
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if crate::auth::internal::authenticate_pipeline_of(&state, &headers, &project, &name)
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(sent) = serde_json::from_slice::<Sent>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    log_sent(&state, &project, &name, sent).await
}

/// Keeps the lines of one report, once the caller is known.
pub(crate) async fn log_sent(
    state: &AppState,
    project: &str,
    name: &str,
    sent: Sent,
) -> StatusCode {
    if state.mirror.get(project, "Pipeline", name).is_none() {
        return StatusCode::NOT_FOUND;
    }
    if sent.sent.len() + sent.failed.len() > BATCH {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    // A hash per id or none: a report whose two lists disagree says nothing of what changed.
    if !sent.hashes.is_empty() && sent.hashes.len() != sent.sent.len() {
        return StatusCode::BAD_REQUEST;
    }
    let refusal = refusal(state, project, name, &sent).await;
    let lines: Vec<NewLine> = sent
        .sent
        .iter()
        .map(|id| NewLine {
            record_id: id.as_str().unwrap_or_default().to_owned(),
            step: None,
            outcome: Outcome::Sent,
            message: String::new(),
        })
        .chain(sent.failed.iter().map(|failed| NewLine {
            record_id: failed.id.as_str().unwrap_or_default().to_owned(),
            step: None,
            outcome: Outcome::Failed,
            message: match &refusal {
                Some(reason) if forbidden(&failed.message) => runner_error(Some(&format!(
                    "{reason}; the gateway answered: {}",
                    failed.message
                ))),
                _ => runner_error(Some(failed.message.as_str()).filter(|m| !m.trim().is_empty())),
            },
        }))
        .collect();
    let run = run_name(sent.run.as_deref());
    let written: Vec<(String, String)> = sent
        .sent
        .iter()
        .zip(&sent.hashes)
        .map(|(id, hash)| {
            let hash = hash
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| hash.to_string());
            (id.as_str().unwrap_or_default().to_owned(), hash)
        })
        .collect();
    let ids: Vec<String> = sent
        .sent
        .iter()
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect();
    crate::pipeline_source_deletes::note_seen(state, project, name, &run, &ids, true).await;
    let kept = match state.pipeline_log.append(project, name, &run, &lines).await {
        Ok(()) if written.is_empty() => Ok(()),
        Ok(()) => state
            .pipeline_log
            .record_changes(project, name, &run, &written)
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    };
    match kept {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!(project, pipeline = name, %error, "a run's log lines were not kept");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// Whether the runner's words carry the gateway's `403`.
fn forbidden(message: &str) -> bool {
    message.contains("403")
}

/// The ended grant a gateway `403` in this report means (PL-93): who revoked it and when, or when
/// it ended, from the pipeline's access requests; `None` when no line was refused `403`, no grant
/// of the pipeline ended, or the Portal keeps no requests.
async fn refusal(state: &AppState, project: &str, name: &str, sent: &Sent) -> Option<String> {
    if !sent.failed.iter().any(|failed| forbidden(&failed.message)) {
        return None;
    }
    let pool = state.db.as_ref()?;
    match crate::access_requests::outgoing(pool, project, name).await {
        Ok(requests) => {
            crate::access_requests::refusal_reason(&requests, time::OffsetDateTime::now_utc())
        }
        Err(error) => {
            tracing::warn!(project, pipeline = name, %error, "the pipeline's access requests were not read for its refused run");
            None
        }
    }
}

/// The record's `id`, or nothing when it has none a line could show.
fn record_id(record: &Value) -> String {
    record
        .get("id")
        .or_else(|| record.get("@id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The local name of the space the pipeline's first output writes into.
pub(crate) fn target_space(state: &AppState, project: &str, spec: &Value) -> Option<String> {
    let spec: jc_core::kinds::pipeline::PipelineSpec = serde_json::from_value(spec.clone()).ok()?;
    let output = spec.outputs().into_iter().next()?;
    let endpoint = state
        .mirror
        .get(project, "Endpoint", output.target_endpoint.local_id())?;
    crate::api::assistant::ref_name(&endpoint.spec["contextSpaceRef"])
}

/// The runner's error, shortened and without anything shaped like a credential: an author's
/// mapping can throw whatever it read.
fn runner_error(error: Option<&str>) -> String {
    let text: String = error
        .unwrap_or("the runner refused the record without a reason")
        .chars()
        .take(ERROR_CHARS)
        .collect();
    crate::pipeline_outcomes::mask_text(&text)
}

/// What a tap posts (T-3974): the node it sits after and the message, or its text.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tapped {
    pub node: String,
    pub record: Value,
}

/// `POST /internal/pipelines/{project}/{name}/samples`: one message a tap sampled, kept among the
/// newest of its node.
pub async fn sampled(
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if crate::auth::internal::authenticate_pipeline_of(&state, &headers, &project, &name)
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(tapped) = serde_json::from_slice::<Tapped>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    if !crate::pipeline_samples::is_node(&tapped.node) {
        return StatusCode::BAD_REQUEST;
    }
    if state.mirror.get(&project, "Pipeline", &name).is_none() {
        return StatusCode::NOT_FOUND;
    }
    state
        .pipeline_samples
        .push(&project, &name, &tapped.node, tapped.record);
    StatusCode::NO_CONTENT
}

/// `POST /internal/pipelines/{project}/{name}/source-deletes`: what one chunk of a delete stream
/// deleted, found gone or had refused (PL-97, T-4891).
pub async fn source_deletes(
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if crate::auth::internal::authenticate_pipeline_of(&state, &headers, &project, &name)
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(report) = serde_json::from_slice::<crate::pipeline_source_deletes::Report>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    if state.mirror.get(&project, "Pipeline", &name).is_none() {
        return StatusCode::NOT_FOUND;
    }
    match crate::pipeline_source_deletes::reported(&state, &project, &name, report).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!(%project, pipeline = %name, %error, "a source-delete report was not kept");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// `POST /internal/pipelines/{project}/{name}/source-events`: the ids an events stream's
/// `deletedWhen` marked in one message, and the records it could not judge (PL-98, T-5360).
pub(crate) async fn source_events(
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if crate::auth::internal::authenticate_pipeline_of(&state, &headers, &project, &name)
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(marks) = serde_json::from_slice::<crate::pipeline_source_deletes::Marks>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    if marks.ids.len() + marks.failed.len() > BATCH {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    let Some(pipeline) = state.mirror.get(&project, "Pipeline", &name) else {
        return StatusCode::NOT_FOUND;
    };
    if crate::pipeline_source_deletes::events_rule(&pipeline.spec).is_none() {
        return StatusCode::CONFLICT;
    }
    match crate::pipeline_source_deletes::marked_at_source(&state, &project, &name, marks).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!(%project, pipeline = %name, %error, "a source's delete marks were not kept");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/internal/pipelines/{project}/{name}/rejected",
            post(rejected),
        )
        .route(
            "/internal/pipelines/{project}/{name}/outcomes",
            post(outcomes),
        )
        .route(
            "/internal/pipelines/{project}/{name}/samples",
            post(sampled),
        )
        .route(
            "/internal/pipelines/{project}/{name}/source-deletes",
            post(source_deletes),
        )
        .route(
            "/internal/pipelines/{project}/{name}/source-events",
            post(source_events),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::pipeline_log::Run;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;
    use std::sync::Arc;

    fn manifest(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, "ovzdusie"),
            spec,
            status: None,
        }
    }

    /// A project with one space that names its model, an Endpoint into it and a pipeline
    /// writing through that Endpoint; the model is compiled the way a sync compiles it.
    fn world() -> AppState {
        let mut config = Config::for_tests();
        config.org_domain = Some("banskabystrica.sk".into());
        let state = AppState::new(config, None);
        state.mirror.upsert(manifest(
            "ContextSpace",
            "ovzdusie",
            json!({ "urnSegment": "ovzdusie", "dataModelRef": { "kind": "DataModel", "name": "air" } }),
        ));
        state.mirror.upsert(manifest(
            "Endpoint",
            "air-write",
            json!({ "contextSpaceRef": "ovzdusie", "slug": "k7r2m4xq9vbn3tdw6hcy5pajfe" }),
        ));
        state.mirror.upsert(manifest(
            "Pipeline",
            "stations",
            json!({
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:air-write"
            }),
        ));
        state.model_schemas.replace(std::collections::HashMap::from([(
            ("ovzdusie".to_owned(), "ovzdusie".to_owned()),
            Arc::new(crate::pipeline_validation::ModelSchema::compile(
                "air",
                "1.0.0",
                &json!({ "definitions": { "AirQualityObserved": {
                    "additionalProperties": false,
                    "properties": { "id": { "type": "string" }, "pm10": { "type": "number", "x-ngsi-ld-kind": "Property" } }
                }}}),
                &["AirQualityObserved".to_owned()],
                false,
            )),
        )]));
        state
    }

    fn refused(record: Value, step: Option<Value>) -> Refused {
        Refused {
            record,
            error: Some("json_schema: pm10: Invalid type".into()),
            step,
            run: Some("2026-09-25T08:00:00Z".into()),
        }
    }

    #[tokio::test]
    async fn a_refused_record_is_kept_with_the_rule_the_workbench_names() {
        let state = world();
        let record = json!({
            "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:s-1",
            "type": "AirQualityObserved",
            "pm10": { "type": "Property", "value": "n/a" },
            "password": "hunter2hunter2"
        });
        assert_eq!(
            keep(&state, "ovzdusie", "stations", refused(record, None)).await,
            StatusCode::NO_CONTENT
        );
        let kept = state
            .rejected
            .list("ovzdusie", "stations", 10, None)
            .await
            .expect("the list");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].rule, "sh:datatype", "{kept:?}");
        assert_eq!(kept[0].path, "pm10");
        assert_eq!(
            kept[0].record["password"],
            crate::pipeline_outcomes::MASK,
            "masked before it is kept"
        );
        assert!(
            !kept[0].message.contains("n/a"),
            "the value is not quoted: {}",
            kept[0].message
        );

        // An id is any NGSI-LD URN of its type (ADR-N-041, PF-43): one naming another type is wrong.
        let wrong_id =
            json!({ "id": "urn:ngsi-ld:Device:hel.fi:x:1", "type": "AirQualityObserved" });
        keep(&state, "ovzdusie", "stations", refused(wrong_id, None)).await;
        let undeclared =
            json!({ "id": "urn:ngsi-ld:Device:banskabystrica.sk:ovzdusie:1", "type": "Device" });
        keep(&state, "ovzdusie", "stations", refused(undeclared, None)).await;
        let kept = state
            .rejected
            .list("ovzdusie", "stations", 10, None)
            .await
            .expect("the list");
        let rules: Vec<&str> = kept.iter().map(|r| r.rule.as_str()).collect();
        assert_eq!(rules, ["type", "id", "sh:datatype"], "newest first");
    }

    #[tokio::test]
    async fn a_record_that_failed_in_a_step_is_kept_with_its_step_and_an_unknown_pipeline_is_404() {
        let state = world();
        let valid = json!({ "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:s-1", "type": "AirQualityObserved" });
        keep(
            &state,
            "ovzdusie",
            "stations",
            refused(valid, Some(json!("2"))),
        )
        .await;
        let kept = state
            .rejected
            .list("ovzdusie", "stations", 10, None)
            .await
            .expect("the list");
        assert_eq!((kept[0].rule.as_str(), kept[0].step), ("step", Some(2)));

        assert_eq!(
            keep(&state, "ovzdusie", "nobody", refused(json!({}), None)).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            state
                .rejected
                .count("ovzdusie", "nobody")
                .await
                .expect("count"),
            0
        );
    }

    #[tokio::test]
    async fn a_call_without_the_runners_token_is_401() {
        use tower::ServiceExt;
        let state = world();
        let response = router()
            .with_state(state.clone())
            .oneshot(
                axum::http::Request::post("/internal/pipelines/ovzdusie/stations/rejected")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"record":{}}"#))
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            state
                .rejected
                .count("ovzdusie", "stations")
                .await
                .expect("count"),
            0
        );
    }

    /// PL-98 (T-5360): only the pipeline's own workload may post delete marks.
    #[tokio::test]
    async fn delete_marks_without_the_pipelines_token_are_401_and_mark_nothing() {
        use tower::ServiceExt;
        let state = world();
        state
            .source_ledger
            .seen("ovzdusie", "stations", "r0", &["urn:a:1".to_owned()], true)
            .await
            .expect("seen");
        let response = router()
            .with_state(state.clone())
            .oneshot(
                axum::http::Request::post("/internal/pipelines/ovzdusie/stations/source-events")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"run":"r1","ids":["urn:a:1"]}"#))
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(state
            .source_ledger
            .marking()
            .await
            .expect("marking")
            .is_empty());
    }

    #[test]
    fn the_runners_error_is_shortened_and_a_credential_in_it_masked() {
        let jwt = [
            "eyJhbGciOiJIUzI1NiJ9",
            "eyJzdWIiOiJ4In0",
            "c2lnbmF0dXJlLXBhcnQ",
        ]
        .join(".");
        let said = runner_error(Some(&format!("failed reading {jwt} from the feed")));
        assert!(!said.contains("eyJ"), "{said}");
        assert!(said.contains(crate::pipeline_outcomes::MASK));
        assert_eq!(runner_error(Some(&"x".repeat(2000))).len(), ERROR_CHARS);
        assert!(runner_error(None).contains("without a reason"));
    }

    #[tokio::test]
    async fn a_refused_record_is_a_line_of_its_run_and_a_sent_batch_is_one_line_per_record() {
        let state = world();
        let bad = json!({ "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:s-1", "type": "AirQualityObserved", "pm10": { "type": "Property", "value": "n/a" } });
        keep(&state, "ovzdusie", "stations", refused(bad, None)).await;
        let valid = json!({ "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:s-2", "type": "AirQualityObserved" });
        keep(
            &state,
            "ovzdusie",
            "stations",
            refused(valid, Some(json!("0"))),
        )
        .await;
        let sent = Sent {
            failed: Vec::new(),
            hashes: Vec::new(),
            run: Some("2026-09-25T08:00:00Z".into()),
            sent: vec![json!("urn:a"), json!("urn:b"), json!(null)],
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "stations", sent).await,
            StatusCode::NO_CONTENT
        );

        let runs = state
            .pipeline_log
            .runs("ovzdusie", "stations", 10)
            .await
            .expect("runs");
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].sent, runs[0].rejected, runs[0].failed), (3, 1, 1));
        let lines = state
            .pipeline_log
            .lines("ovzdusie", "stations", "2026-09-25T08:00:00Z", 10, None)
            .await
            .expect("lines");
        let rejected = lines
            .iter()
            .find(|line| line.outcome == Outcome::Rejected)
            .expect("the refused record's line");
        assert!(rejected.record_id.ends_with(":s-1"));
        assert!(!rejected.message.is_empty());
        // T-3252: the kept record names the run whose log holds that line.
        let kept = state
            .rejected
            .list("ovzdusie", "stations", 10, None)
            .await
            .expect("rejected");
        assert!(kept
            .iter()
            .all(|record| record.run.as_deref() == Some("2026-09-25T08:00:00Z")));
        let failed = lines
            .iter()
            .find(|line| line.outcome == Outcome::Failed)
            .expect("the step's line");
        assert_eq!(failed.step, Some(0));
    }

    /// T-3304: what each report wrote splits into created, updated and unchanged by the hash the
    /// sink sends beside each id; a report without hashes leaves the split unknown, and one whose
    /// two lists disagree is refused.
    #[tokio::test]
    async fn a_run_counts_what_its_records_created_updated_and_left_unchanged() {
        let state = world();
        let report = |run: &str, items: &[(&str, &str)]| Sent {
            failed: Vec::new(),
            run: Some(run.into()),
            sent: items.iter().map(|(id, _)| json!(id)).collect(),
            hashes: items.iter().map(|(_, hash)| json!(hash)).collect(),
        };
        let runs = |state: &AppState| {
            let log = Arc::clone(&state.pipeline_log);
            async move { log.runs("ovzdusie", "stations", 10).await.expect("runs") }
        };
        assert_eq!(
            log_sent(
                &state,
                "ovzdusie",
                "stations",
                report("r1", &[("urn:a", "1"), ("urn:b", "2")])
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            log_sent(
                &state,
                "ovzdusie",
                "stations",
                report("r2", &[("urn:a", "1"), ("urn:b", "3"), ("urn:c", "4")])
            )
            .await,
            StatusCode::NO_CONTENT
        );
        let split = |run: &Run| (run.created, run.updated, run.unchanged);
        let all = runs(&state).await;
        let r1 = all.iter().find(|run| run.run == "r1").expect("r1");
        let r2 = all.iter().find(|run| run.run == "r2").expect("r2");
        assert_eq!(split(r1), (Some(2), Some(0), Some(0)));
        assert_eq!(split(r2), (Some(1), Some(1), Some(1)));

        let old = Sent {
            failed: Vec::new(),
            run: Some("r3".into()),
            sent: vec![json!("urn:a")],
            hashes: Vec::new(),
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "stations", old).await,
            StatusCode::NO_CONTENT
        );
        let r3 = runs(&state)
            .await
            .into_iter()
            .find(|run| run.run == "r3")
            .expect("r3");
        assert_eq!((r3.sent, split(&r3)), (1, (None, None, None)));

        let lopsided = Sent {
            failed: Vec::new(),
            run: Some("r4".into()),
            sent: vec![json!("urn:a"), json!("urn:b")],
            hashes: vec![json!("1")],
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "stations", lopsided).await,
            StatusCode::BAD_REQUEST
        );
    }

    /// T-3001, PL-62: a pass that wrote nothing is still a run, with its counts at zero, so a
    /// person can tell "ran, nothing new" from "did not run".
    #[tokio::test]
    async fn a_pass_that_wrote_nothing_is_a_run_with_its_counts_at_zero() {
        let state = world();
        let pass = Sent {
            failed: Vec::new(),
            hashes: Vec::new(),
            run: Some("2026-09-26T01:00:00Z".into()),
            sent: Vec::new(),
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "stations", pass).await,
            StatusCode::NO_CONTENT
        );
        let runs = state
            .pipeline_log
            .runs("ovzdusie", "stations", 10)
            .await
            .expect("runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run, "2026-09-26T01:00:00Z");
        assert_eq!((runs[0].sent, runs[0].rejected, runs[0].failed), (0, 0, 0));
        assert!(state
            .pipeline_log
            .lines("ovzdusie", "stations", "2026-09-26T01:00:00Z", 10, None)
            .await
            .expect("lines")
            .is_empty());
    }

    #[tokio::test]
    async fn a_report_for_an_unknown_pipeline_or_larger_than_a_batch_is_refused() {
        let state = world();
        let many = Sent {
            failed: Vec::new(),
            hashes: Vec::new(),
            run: None,
            sent: vec![json!("urn:x"); BATCH + 1],
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "stations", many).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let unknown = Sent {
            failed: Vec::new(),
            hashes: Vec::new(),
            run: None,
            sent: vec![json!("urn:x")],
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "nobody", unknown).await,
            StatusCode::NOT_FOUND
        );
        assert!(state
            .pipeline_log
            .runs("ovzdusie", "stations", 10)
            .await
            .expect("runs")
            .is_empty());
    }

    /// T-4056: what the gateway refused in a batch it took is a `failed` line of the run with the
    /// gateway's words, masked; sent and failed together stay within one batch.
    #[tokio::test]
    async fn the_records_the_gateway_refused_are_failed_lines_of_their_run() {
        let state = world();
        let report = Sent {
            failed: vec![
                Failed {
                    id: json!("urn:c"),
                    message: "speed is not a number, Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJl".into(),
                },
                Failed {
                    id: json!("urn:d"),
                    message: "  ".into(),
                },
            ],
            hashes: vec![json!("1"), json!("2")],
            run: Some("2026-10-13T02:00:00Z".into()),
            sent: vec![json!("urn:a"), json!("urn:b")],
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "stations", report).await,
            StatusCode::NO_CONTENT
        );
        let runs = state
            .pipeline_log
            .runs("ovzdusie", "stations", 10)
            .await
            .expect("runs");
        assert_eq!((runs[0].sent, runs[0].rejected, runs[0].failed), (2, 0, 2));
        let lines = state
            .pipeline_log
            .lines("ovzdusie", "stations", "2026-10-13T02:00:00Z", 10, None)
            .await
            .expect("lines");
        let failed: Vec<_> = lines
            .iter()
            .filter(|line| line.outcome == Outcome::Failed)
            .collect();
        let speed = failed
            .iter()
            .find(|line| line.record_id == "urn:c")
            .expect("urn:c");
        assert!(
            speed.message.starts_with("speed is not a number"),
            "{}",
            speed.message
        );
        assert!(
            !speed.message.contains("eyJhbGciOiJIUzI1NiJ9"),
            "{}",
            speed.message
        );
        let bare = failed
            .iter()
            .find(|line| line.record_id == "urn:d")
            .expect("urn:d");
        assert!(
            bare.message.contains("without a reason"),
            "{}",
            bare.message
        );

        let over = Sent {
            failed: (0..2)
                .map(|_| Failed {
                    id: json!("urn:x"),
                    message: String::new(),
                })
                .collect(),
            hashes: Vec::new(),
            run: None,
            sent: vec![json!("urn:y"); BATCH - 1],
        };
        assert_eq!(
            log_sent(&state, "ovzdusie", "stations", over).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let unknown = serde_json::from_value::<Sent>(json!({
            "sent": [], "failed": [{ "id": "urn:x", "message": "m", "extra": 1 }]
        }));
        assert!(unknown.is_err(), "a failed item takes no other member");
    }
}
