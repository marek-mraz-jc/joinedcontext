//! Testing a publish to an MQTT broker before a device sends it, and watching for the real one
//! (T-5534, API/01 §44a).
//!
//! `POST .../mqttbrokers/{name}/test-publish` dry-runs a sample message of one ServiceAccount on
//! one topic through what would take it: the broker's ACL, its size and rate limits, the
//! DataSource and Pipeline bound to the topic, the pipeline's mapping on the project's runner and
//! the record check of the Endpoint it writes through. Nothing is published, no MQTT connection
//! is made and nothing is written. `GET .../mqttbrokers/{name}/watch` answers what the gateway
//! refused for the broker since a time (its CONNECT check keeps the newest 50, without token or
//! client id) and what the broker measures for the account now.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::data_source::DataSourceSpec;
use jc_core::kinds::mqtt_broker::{release_name, MAX_MESSAGE_BYTES};
use jc_core::kinds::{MqttBrokerSpec, PipelineSpec, Verb};
use jc_time::TimestampTz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::api::mqtt_bindings::pipeline_visible;
use crate::api::pipeline_test;
use crate::auth::session::{CurrentUser, Identity};
use crate::error::{ApiError, ProblemDetails};
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;

/// The port of the gateway's internal listener, beside its API port on the same Service
/// (deployment `components/context-gateway`, `extraPorts: internal`).
const GATEWAY_INTERNAL_PORT: u16 = 8082;
/// The longest topic a test names; MQTT allows 65535 bytes, no device topic comes near.
const MAX_TOPIC_BYTES: usize = 1024;

/// One message a ServiceAccount would publish (API/01 §44a).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TestPublish {
    /// A ServiceAccount the broker's roles name, e.g. `cam-west`.
    pub account: String,
    /// The topic the device publishes on, no wildcard, e.g. `cameras/helsinki-cam-west/frame`.
    pub topic: String,
    /// The message as the device sends it, text or JSON.
    pub payload: String,
}

/// How one step judged the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    Refused,
    /// The step could not be run; `reason` says why and what to do.
    Unchecked,
}

/// One step of the dry run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Step {
    /// `acl`, `size`, `rate`, `binding`, `mapping` or `endpoint`.
    pub step: &'static str,
    pub outcome: Outcome,
    /// In words, never a value of the payload or a token.
    pub reason: String,
}

impl Step {
    fn new(step: &'static str, outcome: Outcome, reason: impl Into<String>) -> Self {
        Self {
            step,
            outcome,
            reason: reason.into(),
        }
    }
}

/// The answer of the dry run.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DryRun {
    /// The client id and username the device connects with: its Keycloak client.
    pub client_id: String,
    /// Always `false`: the test publishes nothing.
    pub published: bool,
    pub steps: Vec<Step>,
}

/// Whether the MQTT topic filter `filter` matches `topic` (MQTT 3.1.1 §4.7).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let mut levels = topic.split('/');
    for want in filter.split('/') {
        if want == "#" {
            return true;
        }
        match levels.next() {
            Some(level) if want == "+" || want == level => {}
            _ => return false,
        }
    }
    levels.next().is_none()
}

/// The roles' accounts, sorted, once each.
fn accounts(spec: &MqttBrokerSpec) -> Vec<&str> {
    let mut names: Vec<&str> = spec
        .roles
        .iter()
        .flat_map(|role| role.service_accounts.iter().map(String::as_str))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The body checked against the broker: 422 naming each member at fault.
pub fn validate(spec: &MqttBrokerSpec, body: &TestPublish) -> Result<(), ApiError> {
    let mut errors = Vec::new();
    let names = accounts(spec);
    if !names.contains(&body.account.as_str()) {
        errors.push(format!(
            "account: '{}' is no ServiceAccount of this broker's roles; pick one of: {}",
            body.account.chars().take(64).collect::<String>(),
            names.join(", ")
        ));
    }
    let topic = &body.topic;
    if topic.is_empty() || topic.len() > MAX_TOPIC_BYTES {
        errors.push(format!(
            "topic: give the topic the device publishes on, 1 to {MAX_TOPIC_BYTES} bytes, e.g. sensors/{{client id}}/reading"
        ));
    } else if topic.contains(['+', '#', '\0']) || topic.starts_with('$') {
        errors.push(
            "topic: a device publishes on one topic: no + or # wildcard, no $ at the start".into(),
        );
    }
    if body.payload.len() > MAX_MESSAGE_BYTES as usize {
        errors.push(format!(
            "payload: {} bytes is above the {MAX_MESSAGE_BYTES} bytes any broker accepts",
            body.payload.len()
        ));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Unprocessable {
            detail: "the test message does not fit this broker".into(),
            errors,
        })
    }
}

/// Whether a role of `account` may publish on `topic`, `${clientid}` bound to `client_id`.
fn acl_step(spec: &MqttBrokerSpec, account: &str, client_id: &str, topic: &str) -> Step {
    let filters: Vec<String> = spec
        .roles
        .iter()
        .filter(|role| role.service_accounts.iter().any(|name| name == account))
        .flat_map(|role| &role.publish)
        .map(|filter| filter.replace("${clientid}", client_id))
        .collect();
    if filters.is_empty() {
        return Step::new(
            "acl",
            Outcome::Refused,
            format!("no role of '{account}' may publish; add a publish filter to one of its roles"),
        );
    }
    match filters.iter().find(|filter| topic_matches(filter, topic)) {
        Some(filter) => Step::new("acl", Outcome::Ok, format!("allowed by {filter}")),
        None => Step::new(
            "acl",
            Outcome::Refused,
            format!(
                "'{account}' may publish only on {}; the broker drops this message",
                filters.join(", ")
            ),
        ),
    }
}

fn size_step(spec: &MqttBrokerSpec, bytes: usize) -> Step {
    let limit = spec.limits.max_message_bytes as usize;
    if bytes > limit {
        Step::new(
            "size",
            Outcome::Refused,
            format!("{bytes} bytes is above the broker's limit of {limit} (spec.limits.maxMessageBytes); the broker drops it"),
        )
    } else {
        Step::new("size", Outcome::Ok, format!("{bytes} of {limit} bytes"))
    }
}

fn rate_step(spec: &MqttBrokerSpec) -> Step {
    Step::new(
        "rate",
        Outcome::Ok,
        format!(
            "one message fits; a client may publish {} per second (spec.limits.messagesPerSecond), the broker drops the rest",
            spec.limits.messages_per_second
        ),
    )
}

/// The project's DataSources that read `topic` from broker `name`, each with the Pipelines that
/// take it as their source, in name order.
fn bound(
    mirror: &crate::store::Mirror,
    project: &str,
    name: &str,
    topic: &str,
) -> Vec<(String, Vec<ResourceEnvelope>)> {
    let mut sources: Vec<String> = mirror
        .list(project, "DataSource", &ListOptions::default())
        .items
        .into_iter()
        .filter(|envelope| {
            serde_json::from_value::<DataSourceSpec>(envelope.spec.clone())
                .ok()
                .and_then(|spec| spec.mqtt)
                .is_some_and(|mqtt| {
                    mqtt.broker_ref.is_some_and(|broker| broker.name == name)
                        && mqtt
                            .topics
                            .iter()
                            .any(|filter| topic_matches(filter, topic))
                })
        })
        .map(|envelope| envelope.metadata.name)
        .collect();
    sources.sort();
    let pipelines = mirror
        .list(project, "Pipeline", &ListOptions::default())
        .items;
    sources
        .into_iter()
        .map(|source| {
            let mut taking: Vec<ResourceEnvelope> = pipelines
                .iter()
                .filter(|pipeline| reads(pipeline, &source))
                .cloned()
                .collect();
            taking.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
            (source, taking)
        })
        .collect()
}

fn reads(pipeline: &ResourceEnvelope, source: &str) -> bool {
    serde_json::from_value::<PipelineSpec>(pipeline.spec.clone()).is_ok_and(|spec| {
        spec.sources().iter().any(|input| {
            input
                .data_source_ref
                .as_ref()
                .is_some_and(|reference| reference.name() == source)
        })
    })
}

/// The binding step, naming only the DataSource and Pipeline the caller may read (PF-59, as
/// the bindings route does); the mapping runs only on a Pipeline the caller may read.
fn binding_step<'a>(
    bound: &'a [(String, Vec<ResourceEnvelope>)],
    may_read_sources: bool,
    may_read: &dyn Fn(&ResourceEnvelope) -> bool,
) -> (Step, Option<&'a ResourceEnvelope>) {
    let Some((source, pipelines)) = bound.first() else {
        return (
            Step::new(
                "binding",
                Outcome::Refused,
                "no DataSource of this project reads this topic from this broker: the message would arrive and nobody takes it; add the topic to a DataSource's mqtt.topics",
            ),
            None,
        );
    };
    if !may_read_sources {
        return (
            Step::new(
                "binding",
                Outcome::Ok,
                "a DataSource you may not read takes this topic; ask a steward of the project for its mapping",
            ),
            None,
        );
    }
    if pipelines.is_empty() {
        return (
            Step::new(
                "binding",
                Outcome::Refused,
                format!("DataSource '{source}' reads this topic, but no Pipeline takes it as its source; create one from the DataSource"),
            ),
            None,
        );
    }
    let Some(pipeline) = pipelines.iter().find(|pipeline| may_read(pipeline)) else {
        return (
            Step::new(
                "binding",
                Outcome::Ok,
                format!("DataSource '{source}' reads it, a Pipeline you may not read takes it"),
            ),
            None,
        );
    };
    let more = bound.iter().map(|(_, p)| p.len()).sum::<usize>() - 1;
    let others = if more > 0 {
        format!(
            "; {more} more Pipeline(s) also take it, tested here: '{}'",
            pipeline.metadata.name
        )
    } else {
        String::new()
    };
    (
        Step::new(
            "binding",
            Outcome::Ok,
            format!(
                "DataSource '{source}' reads it, Pipeline '{}' takes it{others}",
                pipeline.metadata.name
            ),
        ),
        Some(pipeline),
    )
}

/// The message as the runner hands it over: JSON when it parses, its text otherwise.
fn message_of(payload: &str) -> Value {
    serde_json::from_str(payload).unwrap_or_else(|_| Value::String(payload.to_owned()))
}

/// The pipeline's mapping over the payload and the record check of where it writes; both
/// `unchecked` when the caller may not test the project's pipelines or the runner cannot run it.
async fn mapping_steps(
    identity: &Identity,
    state: &AppState,
    project: &str,
    pipeline: &ResourceEnvelope,
    payload: &str,
) -> Vec<Step> {
    let mut manifest = match serde_json::to_value(pipeline) {
        Ok(manifest) => manifest,
        Err(error) => return unchecked_both(error.to_string()),
    };
    if let Some(object) = manifest.as_object_mut() {
        object.remove("status");
    }
    let spec = match pipeline_test::tested_spec(identity, state, project, &manifest) {
        Ok(spec) => spec,
        Err(ApiError::Forbidden | ApiError::Denied(_)) => {
            return unchecked_both(
                "only a person who may propose this project's Pipelines runs its mapping; the steps above still hold",
            )
        }
        Err(error) => return unchecked_both(why(&error)),
    };
    let trace = match pipeline_test::run_messages(
        state,
        project,
        &spec,
        &pipeline_test::manifest_name(&manifest),
        &[message_of(payload)],
    )
    .await
    {
        Ok(trace) => trace,
        Err(error) => return unchecked_both(why(&error)),
    };
    let mapping = match mapping_step(&trace) {
        Ok(mapping) => mapping,
        Err(steps) => return steps,
    };
    let endpoint = endpoint_of(crate::record_check::check_trace(
        state,
        project,
        &manifest,
        &trace.mapping,
    ));
    vec![mapping, endpoint]
}

/// The endpoint step of a record check: what it found, or why it could not run.
fn endpoint_of(checked: Result<crate::record_check::Checked, ApiError>) -> Step {
    match checked {
        Ok(checked) => endpoint_step(&checked),
        Err(error) => Step::new("endpoint", Outcome::Unchecked, why(&error)),
    }
}

/// The mapping step of a run, or both steps when the run left no record to check: a failure
/// names the runner's error, a run with no record and no error (a runner that captured nothing
/// answers its silence as an error today, `pipeline_test::run_stream_within`) says so.
fn mapping_step(trace: &jcctl::pipeline_test::TestTrace) -> Result<Step, Vec<Step>> {
    let refused = |why: String| {
        vec![
            Step::new("mapping", Outcome::Refused, why),
            Step::new("endpoint", Outcome::Unchecked, "no record to check"),
        ]
    };
    if let Some(error) = trace.errors.first() {
        return Err(refused(format!("the mapping failed: {}", error.message)));
    }
    if trace.mapping.is_empty() {
        return Err(refused(
            "the mapping made no record of this message".to_owned(),
        ));
    }
    Ok(Step::new(
        "mapping",
        Outcome::Ok,
        format!("{} record(s)", trace.mapping.len()),
    ))
}

fn endpoint_step(checked: &crate::record_check::Checked) -> Step {
    let through = checked
        .records
        .iter()
        .find_map(|record| record.endpoint.clone())
        .unwrap_or_else(|| "its Endpoint".to_owned());
    if checked.ok {
        return Step::new(
            "endpoint",
            Outcome::Ok,
            format!("every attribute is written through {through}"),
        );
    }
    let findings: Vec<String> = checked
        .records
        .iter()
        .flat_map(|record| &record.findings)
        .take(3)
        .map(|finding| {
            if finding.field.is_empty() {
                finding.reason.clone()
            } else {
                format!("{}: {}", finding.field, finding.reason)
            }
        })
        .collect();
    Step::new(
        "endpoint",
        Outcome::Refused,
        format!("{through} refuses: {}", findings.join("; ")),
    )
}

fn unchecked_both(reason: impl Into<String>) -> Vec<Step> {
    let reason = reason.into();
    vec![
        Step::new("mapping", Outcome::Unchecked, reason.clone()),
        Step::new("endpoint", Outcome::Unchecked, reason),
    ]
}

/// The sentence of an error the person can act on, never its debug form.
fn why(error: &ApiError) -> String {
    match error {
        ApiError::Unavailable(text)
        | ApiError::BadRequest(text)
        | ApiError::Conflict(text)
        | ApiError::NotFound(text) => text.clone(),
        _ => "the runner could not test the mapping; try again in a minute".into(),
    }
}

/// The broker `name` of `project` when the caller may read it; 404 otherwise (PF-59).
fn readable_broker(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
) -> Result<MqttBrokerSpec, ApiError> {
    let not_found = || {
        ApiError::NotFound(format!(
            "MQTT broker '{name}' not found in project '{project}'"
        ))
    };
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() || !effective.may_read("MqttBroker") {
        return Err(not_found());
    }
    let manifest = state
        .mirror
        .get(project, "MqttBroker", name)
        .ok_or_else(not_found)?;
    serde_json::from_value(manifest.spec).map_err(|_| not_found())
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/test-publish",
    summary = "Test an MQTT Publish",
    description = "Dry-runs one message of a ServiceAccount on a topic: the broker's ACL, size and rate limits, the DataSource and Pipeline bound to the topic, the mapping on the project's runner and the Endpoint's record check (API/01 §44a, T-5534). Nothing is published or written. Whoever may read the project's MqttBrokers; the mapping step needs propose on Pipeline.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
    ),
    request_body(
        content = TestPublish,
        content_type = "application/json",
        example = json!({"account": "cam-west", "topic": "cameras/helsinki-cam-west/frame", "payload": "{\"temperature\": 21.5}"})
    ),
    responses(
        (status = 200, description = "Each step's outcome", body = DryRun),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such broker, or not readable by the caller", body = ProblemDetails),
        (status = 422, description = "An account the broker's roles do not name, a topic with a wildcard, or a payload above 1 MiB", body = ProblemDetails),
    )
)]
pub async fn test_publish(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(body): Json<TestPublish>,
) -> Result<Json<DryRun>, ApiError> {
    dry_run(&state, &user.0.identity, &project, &name, body)
        .await
        .map(Json)
}

/// The dry run of `body` on broker `name`, once the caller may read the broker.
async fn dry_run(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
    body: TestPublish,
) -> Result<DryRun, ApiError> {
    let spec = readable_broker(state, identity, project, name)?;
    validate(&spec, &body)?;
    let client_id = format!("{project}-{}", body.account);
    let mut steps = vec![
        acl_step(&spec, &body.account, &client_id, &body.topic),
        size_step(&spec, body.payload.len()),
        rate_step(&spec),
    ];
    let bound = bound(&state.mirror, project, name, &body.topic);
    let effective = crate::permissions::for_request(state, identity, project);
    let may = |kind: &str, space: Option<&str>| effective.may_in(kind, Verb::Read, space);
    let may_read = |pipeline: &ResourceEnvelope| {
        serde_json::from_value::<PipelineSpec>(pipeline.spec.clone())
            .is_ok_and(|spec| pipeline_visible(&state.mirror, project, &spec, &may))
    };
    let (binding, pipeline) = binding_step(&bound, may("DataSource", None), &may_read);
    steps.push(binding);
    match pipeline {
        Some(pipeline) => {
            steps.extend(mapping_steps(identity, state, project, pipeline, &body.payload).await)
        }
        None => steps.extend(unchecked_both("no Pipeline you may read takes this topic")),
    }
    Ok(DryRun {
        client_id,
        published: false,
        steps,
    })
}

/// What the watch asks for.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct WatchQuery {
    /// Only this ServiceAccount's refusals and connections, e.g. `cam-west`.
    pub account: Option<String>,
    /// Only refusals after this time (RFC 3339), e.g. `2026-10-24T10:00:00Z`.
    #[param(value_type = Option<String>, format = DateTime)]
    pub since: Option<TimestampTz>,
}

/// One CONNECT the gateway refused, as its record keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Refusal {
    #[schema(value_type = String, format = DateTime)]
    pub at: TimestampTz,
    /// `no_token`, `expired`, `wrong_audience`, `invalid_token`, `wrong_client`,
    /// `unknown_account`, `other_project`, `no_role`, `no_subscription`, `no_realm`,
    /// `source_address`, `token_not_allowed` or `certificate`.
    pub reason: String,
    pub account: Option<String>,
}

#[derive(Deserialize)]
struct Kept {
    refusals: Vec<Refusal>,
}

/// What the watch answers.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Watched {
    #[schema(value_type = String, format = DateTime)]
    pub observed_at: TimestampTz,
    /// Newest first; empty when `refusalsRead` is false.
    pub refusals: Vec<Refusal>,
    pub refusals_read: bool,
    /// The account's connected clients now; `null` when the broker is not measured.
    pub connected: Option<u32>,
    /// The broker's messages per second now; `null` when not measured.
    pub messages_per_second: Option<f64>,
    /// Why refusals or measurements are missing, in words.
    pub reason: Option<String>,
}

/// The gateway's internal listener: its API URL with the internal port.
fn internal_url(gateway: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(gateway).ok()?;
    url.set_port(Some(GATEWAY_INTERNAL_PORT)).ok()?;
    Some(url.as_str().trim_end_matches('/').to_owned())
}

/// The gateway's kept refusals of broker `release`, read on its internal listener at `base`,
/// where only the Portal's mesh identity is answered.
pub async fn refusals(base: &str, release: &str) -> Result<Vec<Refusal>, String> {
    let answer = crate::api::mqtt_measured::http()
        .get(format!("{base}/internal/mqtt/refusals/{release}"))
        .send()
        .await
        .map_err(|_| "the gateway did not answer the refusal read; try again in a minute")?;
    if !answer.status().is_success() {
        return Err(format!(
            "the gateway answered {} to the refusal read; its JC_GATEWAY_MQTT_PORTAL_IDENTITY must name the Portal",
            answer.status().as_u16()
        ));
    }
    answer
        .json::<Kept>()
        .await
        .map(|kept| kept.refusals)
        .map_err(|_| "the gateway answered something other than its refusal record".to_owned())
}

/// The refusals after `since` of `account`, when given.
pub fn filtered(
    refusals: Vec<Refusal>,
    since: Option<TimestampTz>,
    account: Option<&str>,
) -> Vec<Refusal> {
    refusals
        .into_iter()
        .filter(|refusal| since.is_none_or(|since| refusal.at > since))
        .filter(|refusal| account.is_none_or(|account| refusal.account.as_deref() == Some(account)))
        .collect()
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/watch",
    operation_id = "watch_mqtt_broker",
    summary = "Watch an MQTT Broker's Refusals",
    description = "The CONNECTs the gateway refused for this broker since a time, with their reason, and the account's connected clients now (API/01 §44a, T-5534). The record holds no token, client id or address. Whoever may read the project's MqttBrokers.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
        WatchQuery,
    ),
    responses(
        (status = 200, description = "Refusals and the account's connections", body = Watched),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such broker, or not readable by the caller", body = ProblemDetails),
        (status = 422, description = "An account the broker's roles do not name", body = ProblemDetails),
    )
)]
pub async fn watch(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Query(query): Query<WatchQuery>,
) -> Result<Json<Watched>, ApiError> {
    watched(&state, &user.0.identity, &project, &name, query)
        .await
        .map(Json)
}

/// The clients connected now as `account`, or as any account of the broker; `None` unmeasured.
fn connected(measured: &crate::api::mqtt_measured::Measured, account: Option<&str>) -> Option<u32> {
    measured.measured.then(|| {
        measured
            .accounts
            .iter()
            .filter(|seen| account.is_none_or(|account| seen.service_account == account))
            .map(|seen| seen.connected)
            .sum()
    })
}

/// The broker's kept refusals and the account's connections, once the caller may read it.
async fn watched(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
    query: WatchQuery,
) -> Result<Watched, ApiError> {
    let spec = readable_broker(state, identity, project, name)?;
    let account = query.account.as_deref();
    if let Some(account) = account {
        if !accounts(&spec).contains(&account) {
            return Err(ApiError::Unprocessable {
                detail: "the account is not one of this broker's".into(),
                errors: vec![format!(
                    "account: no role of this broker names this ServiceAccount; pick one of: {}",
                    accounts(&spec).join(", ")
                )],
            });
        }
    }
    let release = release_name(project, name);
    let mut reasons = Vec::new();
    let refused = match state.config.gateway_url.as_deref() {
        Some(gateway) => match internal_url(gateway) {
            Some(base) => refusals(&base, &release).await,
            None => Err("the gateway URL (JC_PORTAL_GATEWAY_URL) does not parse".to_owned()),
        },
        None => Err("this instance names no gateway (JC_PORTAL_GATEWAY_URL)".to_owned()),
    };
    let refusals_read = refused.is_ok();
    let refused = refused.unwrap_or_else(|reason| {
        reasons.push(reason);
        Vec::new()
    });
    let measured = crate::api::mqtt_measured::measured_now(state, project, &spec, &release).await;
    Ok(watch_answer(
        refused,
        refusals_read,
        reasons,
        &measured,
        query.since,
        account,
    ))
}

/// The watch's answer from what was read: the refusals read (or the reasons they were not), and
/// what the broker measures, whose own reason joins the others.
fn watch_answer(
    refused: Vec<Refusal>,
    refusals_read: bool,
    mut reasons: Vec<String>,
    measured: &crate::api::mqtt_measured::Measured,
    since: Option<TimestampTz>,
    account: Option<&str>,
) -> Watched {
    if let Some(reason) = &measured.reason {
        reasons.push(reason.clone());
    }
    Watched {
        observed_at: TimestampTz::now(),
        refusals: filtered(refused, since, account),
        refusals_read,
        connected: connected(measured, account),
        messages_per_second: measured.messages_per_second,
        reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/mqttbrokers/{name}/test-publish",
            post(test_publish),
        )
        .route("/projects/{project}/mqttbrokers/{name}/watch", get(watch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MqttBrokerSpec {
        serde_json::from_value(serde_json::json!({
            "roles": [
                {"name": "camera", "serviceAccounts": ["cam-west"], "publish": ["cameras/${clientid}/#"]},
                {"name": "viewer", "serviceAccounts": ["dashboard"], "subscribe": ["dashboard/${clientid}/#"]},
            ],
            "limits": {"maxMessageBytes": 16}
        }))
        .expect("a broker reads")
    }

    fn body(account: &str, topic: &str, payload: &str) -> TestPublish {
        TestPublish {
            account: account.into(),
            topic: topic.into(),
            payload: payload.into(),
        }
    }

    fn finding(field: &str, reason: &str) -> crate::record_check::Finding {
        crate::record_check::Finding {
            source: crate::record_check::Source::Endpoint,
            rule: "policy".into(),
            field: field.into(),
            reason: reason.into(),
            access: None,
        }
    }

    fn record(
        endpoint: Option<&str>,
        findings: Vec<crate::record_check::Finding>,
    ) -> crate::record_check::RecordCheck {
        crate::record_check::RecordCheck {
            index: 0,
            ok: findings.is_empty(),
            endpoint: endpoint.map(str::to_owned),
            findings,
            expires: None,
        }
    }

    #[test]
    fn the_mapping_step_names_a_failure_a_run_with_no_record_and_the_records_made() {
        let mut trace = jcctl::pipeline_test::TestTrace::default();
        let none = vec![
            Step::new(
                "mapping",
                Outcome::Refused,
                "the mapping made no record of this message",
            ),
            Step::new("endpoint", Outcome::Unchecked, "no record to check"),
        ];
        assert_eq!(mapping_step(&trace), Err(none));
        trace.mapping = vec![serde_json::json!({ "id": "urn:ngsi-ld:Camera:west" }); 2];
        assert_eq!(
            mapping_step(&trace),
            Ok(Step::new("mapping", Outcome::Ok, "2 record(s)"))
        );
        trace.errors.push(jcctl::pipeline_test::TestError {
            stage: "mapping".into(),
            step: Some(1),
            line: None,
            message: "expected object".into(),
        });
        let failed = mapping_step(&trace).expect_err("a failed run");
        assert_eq!(
            failed[0],
            Step::new(
                "mapping",
                Outcome::Refused,
                "the mapping failed: expected object"
            )
        );
        assert_eq!(failed[1].outcome, Outcome::Unchecked);
    }

    #[test]
    fn the_endpoint_step_names_the_endpoint_and_at_most_three_findings() {
        let checked = |ok: bool, records| crate::record_check::Checked {
            ok,
            checked: 1,
            rejected: usize::from(!ok),
            records,
        };
        let ok = endpoint_step(&checked(true, vec![record(Some("air-ep"), vec![])]));
        assert_eq!(
            (ok.outcome, ok.reason.as_str()),
            (Outcome::Ok, "every attribute is written through air-ep")
        );
        let unnamed = endpoint_step(&checked(true, vec![record(None, vec![])]));
        assert_eq!(
            unnamed.reason,
            "every attribute is written through its Endpoint"
        );

        let refused = endpoint_step(&checked(
            false,
            vec![
                record(
                    None,
                    vec![
                        finding("", "the record has no id"),
                        finding("pm10", "not writable"),
                    ],
                ),
                record(
                    Some("air-ep"),
                    vec![
                        finding("no2", "not writable"),
                        finding("o3", "not writable"),
                    ],
                ),
            ],
        ));
        assert_eq!(refused.outcome, Outcome::Refused);
        assert_eq!(
            refused.reason,
            "air-ep refuses: the record has no id; pm10: not writable; no2: not writable"
        );
    }

    #[test]
    fn an_error_reads_as_its_sentence_and_anything_else_as_try_again() {
        for error in [
            ApiError::Unavailable("u".into()),
            ApiError::BadRequest("u".into()),
            ApiError::Conflict("u".into()),
            ApiError::NotFound("u".into()),
        ] {
            assert_eq!(why(&error), "u", "{error:?}");
        }
        assert_eq!(
            why(&ApiError::Internal("a stack trace".into())),
            "the runner could not test the mapping; try again in a minute"
        );
        assert_eq!(message_of("{\"pm10\": 4}"), serde_json::json!({"pm10": 4}));
        assert_eq!(message_of("pm10=4"), serde_json::json!("pm10=4"));
    }

    #[test]
    fn the_connected_count_is_the_accounts_own_or_every_accounts() {
        use crate::api::mqtt_measured::{AccountSeen, Measured};
        let seen = |account: &str, connected: u32| AccountSeen {
            service_account: account.into(),
            connected,
            since: None,
        };
        let mut measured = Measured {
            measured: true,
            observed_at: TimestampTz::now(),
            messages_per_second: None,
            publishers: None,
            reason: None,
            accounts: vec![seen("cam-west", 2), seen("dashboard", 1)],
            rates: Vec::new(),
            refused_publishes: None,
        };
        assert_eq!(connected(&measured, Some("cam-west")), Some(2));
        assert_eq!(connected(&measured, None), Some(3));
        assert_eq!(connected(&measured, Some("nobody")), Some(0));
        measured.measured = false;
        assert_eq!(connected(&measured, None), None);
    }

    #[test]
    fn filters_match_as_mqtt_matches_them() {
        for (filter, topic) in [
            ("a/b", "a/b"),
            ("a/+", "a/b"),
            ("a/#", "a/b/c"),
            ("a/#", "a"),
            ("#", "a/b"),
            ("+/+", "a/b"),
        ] {
            assert!(topic_matches(filter, topic), "{filter} {topic}");
        }
        for (filter, topic) in [
            ("a/b", "a/b/c"),
            ("a/+", "a/b/c"),
            ("a/b", "a"),
            ("a/+", "b/c"),
        ] {
            assert!(!topic_matches(filter, topic), "{filter} {topic}");
        }
    }

    #[test]
    fn the_acl_binds_the_client_id_and_names_the_filters_a_refusal_misses() {
        let spec = spec();
        let ok = acl_step(
            &spec,
            "cam-west",
            "helsinki-cam-west",
            "cameras/helsinki-cam-west/frame",
        );
        assert_eq!(ok.outcome, Outcome::Ok, "{ok:?}");
        let other = acl_step(
            &spec,
            "cam-west",
            "helsinki-cam-west",
            "cameras/helsinki-cam-east/frame",
        );
        assert_eq!(other.outcome, Outcome::Refused);
        assert!(
            other.reason.contains("cameras/helsinki-cam-west/#"),
            "{other:?}"
        );
        let viewer = acl_step(&spec, "dashboard", "helsinki-dashboard", "dashboard/x");
        assert_eq!(viewer.outcome, Outcome::Refused);
        assert!(viewer.reason.contains("no role"), "{viewer:?}");
    }

    #[test]
    fn the_size_step_holds_the_brokers_own_limit() {
        assert_eq!(size_step(&spec(), 16).outcome, Outcome::Ok);
        let over = size_step(&spec(), 17);
        assert_eq!(over.outcome, Outcome::Refused);
        assert!(over.reason.contains("maxMessageBytes"), "{over:?}");
    }

    #[test]
    fn each_member_at_fault_is_named() {
        assert!(validate(&spec(), &body("cam-west", "cameras/x", "{}")).is_ok());
        let fault = |b: TestPublish| {
            let errors = validate(&spec(), &b)
                .expect_err("a 422")
                .field_errors()
                .to_vec();
            assert!(!errors.is_empty(), "a 422 names its fields");
            errors
        };
        assert!(fault(body("cam-north", "cameras/x", ""))[0].starts_with("account: "));
        for topic in ["", "cameras/+", "cameras/#", "$SYS/x"] {
            assert!(
                fault(body("cam-west", topic, ""))[0].starts_with("topic: "),
                "{topic}"
            );
        }
        let huge = "x".repeat(MAX_MESSAGE_BYTES as usize + 1);
        assert!(fault(body("cam-west", "cameras/x", &huge))[0].starts_with("payload: "));
    }

    #[test]
    fn the_watch_keeps_refusals_after_since_of_the_account() {
        let at = |s: &str| TimestampTz(jc_time::parse_rfc3339(s).expect("a time"));
        let refusal = |time: &str, account: Option<&str>| Refusal {
            at: at(time),
            reason: "expired".into(),
            account: account.map(str::to_owned),
        };
        let kept = vec![
            refusal("2026-10-24T10:02:00Z", Some("cam-west")),
            refusal("2026-10-24T10:01:00Z", None),
            refusal("2026-10-24T09:59:00Z", Some("cam-west")),
        ];
        let since = Some(at("2026-10-24T10:00:00Z"));
        assert_eq!(filtered(kept.clone(), since, None).len(), 2);
        assert_eq!(
            filtered(kept.clone(), since, Some("cam-west")),
            [refusal("2026-10-24T10:02:00Z", Some("cam-west"))]
        );
        assert_eq!(filtered(kept, None, Some("cam-west")).len(), 2);
    }

    #[test]
    fn the_internal_listener_is_the_gateways_host_on_its_internal_port() {
        assert_eq!(
            internal_url("http://context-gateway.jc.svc.cluster.local:8080").as_deref(),
            Some("http://context-gateway.jc.svc.cluster.local:8082")
        );
        assert_eq!(internal_url("not a url"), None);
    }

    fn pipeline_of(spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Pipeline".into(),
            metadata: crate::resource::ObjectMeta::new("cams", "helsinki"),
            spec,
            status: None,
        }
    }

    /// T-7917: a Pipeline that cannot be tested here (no source to read) leaves the mapping and
    /// the endpoint `unchecked` with the sentence that says what to fix, not a runner error.
    #[tokio::test]
    async fn a_pipeline_the_tester_refuses_leaves_both_steps_unchecked_with_its_sentence() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let pipeline = pipeline_of(serde_json::json!({
            "class": "resident",
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:cams"
        }));
        let identity = Identity {
            client: None,
            subject: "sub-ana".into(),
            username: "ana".into(),
            email: Some("ana@hel.fi".into()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        };
        let steps = mapping_steps(&identity, &state, "helsinki", &pipeline, "{}").await;
        assert_eq!(steps.len(), 2);
        for step in &steps {
            assert_eq!(step.outcome, Outcome::Unchecked, "{}", step.step);
            assert!(
                step.reason.contains("pipeline.spec.source must declare"),
                "{}",
                step.reason
            );
        }
    }

    /// T-7917: a broker that is measured adds no reason, one that is not adds its own, and the
    /// reasons of a refusal list that was not read stay beside it.
    #[test]
    fn the_watch_names_a_reason_only_for_what_it_could_not_read() {
        let measured = |reason: Option<&str>| crate::api::mqtt_measured::Measured {
            measured: reason.is_none(),
            observed_at: TimestampTz::now(),
            messages_per_second: reason.is_none().then_some(2.5),
            publishers: None,
            reason: reason.map(str::to_owned),
            accounts: Vec::new(),
            rates: Vec::new(),
            refused_publishes: None,
        };
        let whole = watch_answer(Vec::new(), true, Vec::new(), &measured(None), None, None);
        assert_eq!(whole.reason, None);
        assert_eq!(whole.messages_per_second, Some(2.5));
        assert_eq!(whole.connected, Some(0));
        assert!(whole.refusals_read);

        let partial = watch_answer(
            Vec::new(),
            false,
            vec!["this instance names no gateway".into()],
            &measured(Some("the broker did not answer")),
            None,
            None,
        );
        assert_eq!(
            partial.reason.as_deref(),
            Some("this instance names no gateway; the broker did not answer")
        );
        assert_eq!(partial.connected, None);
        assert!(!partial.refusals_read);
    }

    /// T-7917: a record check that could not run leaves the endpoint step `unchecked` with its
    /// sentence.
    #[test]
    fn a_record_check_that_could_not_run_leaves_the_endpoint_unchecked() {
        let step = endpoint_of(Err(ApiError::BadRequest(
            "records: at most 1 at a time".into(),
        )));
        assert_eq!(step.step, "endpoint");
        assert_eq!(step.outcome, Outcome::Unchecked);
        assert!(step.reason.contains("at most 1"), "{}", step.reason);
    }

    fn reader(name: &str) -> Identity {
        Identity {
            client: None,
            subject: format!("sub-{name}"),
            username: name.into(),
            email: Some(format!("{name}@hel.fi")),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        }
    }

    fn broker_world() -> AppState {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let org = crate::permissions::ORG_NAMESPACE;
        let put = |kind: &str, name: &str, namespace: &str, spec: Value| {
            state.mirror.upsert(ResourceEnvelope {
                api_version: crate::resource::API_VERSION.into(),
                kind: kind.into(),
                metadata: crate::resource::ObjectMeta::new(name, namespace),
                spec,
                status: None,
            });
        };
        put("Project", "helsinki", org, serde_json::json!({}));
        put(
            "MqttBroker",
            "cameras",
            "helsinki",
            serde_json::json!({ "roles": [
                { "name": "camera", "serviceAccounts": ["cam-west"], "publish": ["cameras/${clientid}/#"] }
            ]}),
        );
        put(
            "Role",
            "reader",
            org,
            serde_json::json!({ "rules": [{ "kinds": ["MqttBroker", "Project"], "verbs": ["read"] }] }),
        );
        put(
            "RoleBinding",
            "ana-reads",
            org,
            serde_json::json!({ "subjects": [{ "user": "ana@hel.fi" }], "role": "reader",
                                "scope": { "project": "helsinki" } }),
        );
        state
    }

    /// T-7917: the watch of an instance that names no gateway and no brokers namespace answers
    /// with both reasons and no connection count; an account the broker does not name is a 422;
    /// a stranger finds no broker.
    #[tokio::test]
    async fn the_watch_of_an_instance_without_gateway_or_brokers_says_why() {
        let state = broker_world();
        let query = |account: Option<&str>| WatchQuery {
            since: None,
            account: account.map(str::to_owned),
        };
        let watched_all = watched(&state, &reader("ana"), "helsinki", "cameras", query(None))
            .await
            .expect("a watch");
        assert!(!watched_all.refusals_read);
        assert_eq!(watched_all.connected, None);
        let reason = watched_all.reason.expect("a reason");
        assert!(reason.contains("JC_PORTAL_GATEWAY_URL"), "{reason}");
        assert!(reason.contains("JC_PORTAL_MQTT_NAMESPACE"), "{reason}");

        let wrong = watched(
            &state,
            &reader("ana"),
            "helsinki",
            "cameras",
            query(Some("nobody")),
        )
        .await;
        assert!(
            matches!(wrong, Err(ApiError::Unprocessable { .. })),
            "{wrong:?}"
        );
        let stranger = watched(&state, &reader("eve"), "helsinki", "cameras", query(None)).await;
        assert!(
            matches!(stranger, Err(ApiError::NotFound(_))),
            "{stranger:?}"
        );
    }
}
