//! Testing a saved DataSource's connection (T-4819, MF-39): the source's own input, rendered as
//! the reconciler renders it for a pipeline, runs as an ephemeral stream on the project's runner
//! until its first records or three seconds, and the answer is those records or why there were
//! none. The connection is the stored manifest's, never one the caller sends, so the runner fills
//! no `${VAR}` a live pipeline of this source would not fill too; the answer carries the records
//! and the error's last clause, never the rendered configuration.

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::data_source::{refused_target, DataSourceSpec, DataSourceType};
use jc_core::kinds::{PipelineSpec, Verb};
use jcctl::bento::{input_of, prepended_processors, InputContext};
use jcctl::pipeline_test::{harness, Sample, SampleFormat, FETCH_FAILED};
use serde_json::{json, Value};

use crate::api::dry_run::Probe;
use crate::api::pipeline_test::{answer_of, run_stream};
use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The bytes of one record the answer keeps; a larger message is cut, and shows as text.
pub(crate) const RECORD_BYTES: usize = 64 * 1024;

/// The runner configuration that reads `source` once and posts each record to `capture`.
///
/// An `http` source is one fetch by the `http` processor, as the reconciler polls it: Bento's
/// `http_client` input polls without pause (T-1192). Every other input runs as it would live;
/// the stream is deleted once the wait ends, with its shared reader state taken off (`isolated`).
pub(crate) fn connection_test(
    name: &str,
    project: &str,
    source: &DataSourceSpec,
    capture: &str,
) -> Result<Value, ApiError> {
    // The envelope the capture route reads: the record as text, or the read's error, worded as
    // the probe words a failed fetch.
    let envelope = format!(
        concat!(
            "root = {{}}\n",
            "root.input = if errored() {{ \"\" }} else {{ content().string().slice(0, {}) }}\n",
            "root.error = if errored() {{ \"{}\" + error() }} else {{ null }}"
        ),
        RECORD_BYTES, FETCH_FAILED
    );
    reading(name, project, "connection-test", source, capture, envelope)
}

/// The client name of a workbench sample's read, before the test's own id: a Pipeline name is
/// DNS-1123, so no live pipeline of the source shares the MQTT client id the read connects with,
/// and the id keeps two reads apart, since a broker drops one of two sessions of one client id.
const SAMPLE_READER: &str = "workbench_sample";

/// The runner configuration that reads a streaming `source` for the workbench's sample step
/// (T-4841): the source's input as a live pipeline reads it, beside the live pipelines, each
/// message posted whole as the sample's record (JSON parsed, anything else as text), and a
/// message past `RECORD_BYTES` (64 KiB) posted as an error with its size instead of its bytes.
pub fn stream_sample(
    name: &str,
    project: &str,
    source: &DataSourceSpec,
    capture: &str,
) -> Result<Value, ApiError> {
    let envelope = format!(
        concat!(
            "let size = if errored() {{ 0 }} else {{ content().length() }}\n",
            "let kept = !errored() && $size <= {max}\n",
            "root = {{}}\n",
            "root.input = if $kept {{ content().string() }} else {{ \"\" }}\n",
            "root.output = if $kept {{ content().string().parse_json().catch(content().string()) }} else {{ deleted() }}\n",
            "root.error = if errored() {{ \"{fetch}\" + error() }} else if !$kept {{ \"a message of %d bytes is over the {max} bytes a sample keeps of one message\".format($size) }} else {{ null }}"
        ),
        max = RECORD_BYTES,
        fetch = FETCH_FAILED
    );
    let run = capture.rsplit('/').next().unwrap_or_default();
    let reader = format!("{SAMPLE_READER}_{run}");
    reading(name, project, &reader, source, capture, envelope)
}

/// `source`'s input read beside the live pipelines as `reader`, each record posted to `capture`
/// in the shape `envelope` maps it to.
fn reading(
    name: &str,
    project: &str,
    reader: &str,
    source: &DataSourceSpec,
    capture: &str,
    envelope: String,
) -> Result<Value, ApiError> {
    let context = InputContext::new(name, project, reader);
    let input = serde_json::to_value(input_of(source, &context))
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // The allow-list a pipeline step is held to (PL-07), over every address the input names.
    if let Some(reason) = refused_target(&input.to_string()) {
        return Err(ApiError::BadRequest(format!(
            "the source's address: {reason}"
        )));
    }
    let input = isolated(input).map_err(ApiError::BadRequest)?;
    let (input, mut processors) = match (&source.source_type, input) {
        (DataSourceType::Http | DataSourceType::GtfsRt, Value::Object(mut map)) => {
            let mut fetch = map.remove("http_client").unwrap_or_else(|| json!({}));
            fetch["timeout"] = json!("3s");
            fetch["retries"] = json!(0);
            (
                json!({ "generate": { "count": 1, "interval": "", "mapping": "root = \"\"" } }),
                vec![json!({ "http": fetch })],
            )
        }
        (_, input) => (input, Vec::new()),
    };
    for processor in prepended_processors(source, &context) {
        processors
            .push(serde_json::to_value(processor).map_err(|e| ApiError::Internal(e.to_string()))?);
    }
    processors.push(json!({ "mapping": envelope }));
    processors.push(json!({ "catch": [] }));

    // The output, with the runner's own credential for the capture route, is the harness's.
    let probe: PipelineSpec = serde_json::from_value(json!({
        "class": "auto",
        "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
    }))
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let sample = Sample {
        text: Some(String::new()),
        url: None,
        format: SampleFormat::Text,
        records: None,
    };
    let mut config =
        harness(&probe, &sample, capture).map_err(|e| ApiError::Internal(e.to_string()))?;
    config["input"] = input;
    config["pipeline"] = json!({ "processors": processors });
    Ok(config)
}

/// The inputs a test may read, each with the fields through which readers share messages. A
/// consumer group, queue group or durable consumer hands each message to one of its readers and
/// commits it there, so a test that joined it would take messages from the live pipelines; the
/// test reads beside them instead. Any other input is refused: deny by default.
const SHARED_FIELDS: &[(&str, &[&str])] = &[
    ("http_client", &[]),
    ("websocket", &[]),
    ("mqtt", &[]),
    ("nats", &["queue"]),
    ("nats_jetstream", &["queue", "durable"]),
    ("kafka_franz", &["consumer_group"]),
];

/// `input` (`{kind: config}`) reading beside the live pipelines, or why a test may not read it.
fn isolated(mut input: Value) -> Result<Value, String> {
    let Some((kind, config)) = input.as_object_mut().and_then(|map| map.iter_mut().next()) else {
        return Err("the saved source has no input to read".into());
    };
    let refused = |why: &str| format!("Test connection does not read a {kind} source: {why}");
    if config["bind"] == json!(true) {
        return Err(refused(
            "it binds an existing consumer, so a test read would take its messages",
        ));
    }
    match (kind.as_str(), config.as_object_mut()) {
        // An empty group keeps no offsets, so nothing is committed (Bento's kafka input).
        ("kafka", Some(map)) => _ = map.insert("consumer_group".into(), json!("")),
        ("mqtt", Some(map)) => isolated_mqtt(map),
        (kind, Some(map)) => match SHARED_FIELDS.iter().find(|(name, _)| *name == kind) {
            Some((_, fields)) => fields.iter().for_each(|field| _ = map.remove(*field)),
            None => return Err(refused("its messages go to one reader each, so a test read would take them from the pipelines that use it")),
        },
        (_, None) => {}
    }
    Ok(input)
}

/// An MQTT read in a session of its own: no `$share/{group}/` subscription, which hands each
/// message to one subscriber of the group, and a clean session the broker drops on disconnect.
fn isolated_mqtt(map: &mut serde_json::Map<String, Value>) {
    map.insert("clean_session".into(), json!(true));
    if let Some(Value::Array(topics)) = map.get_mut("topics") {
        for topic in topics.iter_mut() {
            let shared = topic.as_str().and_then(|t| t.strip_prefix("$share/"));
            if let Some(filter) = shared.and_then(|rest| rest.split_once('/')).map(|(_, f)| f) {
                *topic = json!(filter);
            }
        }
    }
}

/// The saved DataSource `name` of `project`, for a caller who may propose it: 404 for a caller who
/// may not read the project or a source that is not there, 403 for one who may only read it.
pub(crate) fn saved_source(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
) -> Result<DataSourceSpec, ApiError> {
    let unknown = || ApiError::NotFound(format!("no DataSource {name} in project {project}"));
    if !(is_dns1123(project) && is_dns1123(name)) {
        return Err(unknown());
    }
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(unknown());
    }
    let envelope = state
        .mirror
        .get(project, "DataSource", name)
        .ok_or_else(unknown)?;
    let manifest =
        serde_json::to_value(&envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
    // Running a source on the runner is what proposing one is for: a reader sees it, an author tests it.
    effective.check("DataSource", Verb::Propose, Some(&manifest))?;
    serde_json::from_value(envelope.spec.clone())
        .map_err(|e| ApiError::BadRequest(format!("the saved source does not parse: {e}")))
}

/// `POST /api/v1/projects/{project}/datasources/{name}/test` (T-4819, MF-39).
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/datasources/{name}/test",
    summary = "Data Source Connection Test",
    description = "Reads a saved data source once on the project's runner and answers its first \
                   record, or why there was none. Nothing is written.",
    tag = "datasources",
    params(
        ("project" = String, Path, description = "Project slug"),
        ("name" = String, Path, description = "DataSource name"),
    ),
    responses(
        (status = 200, description = "The records read in three seconds and the first of them, \
                                      or `skipped` with the reason in plain words", body = Probe),
        (status = 400, description = "The saved source's URL is one the runner may not reach, or its input hands each message to one reader", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No grant proposes a DataSource in this project", body = ProblemDetails),
        (status = 404, description = "No such DataSource in this project", body = ProblemDetails),
        (status = 409, description = "A test of this project is already running", body = ProblemDetails),
        (status = 503, description = "No pipeline runner is configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn test_datasource(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Probe>, ApiError> {
    let source = saved_source(&state, &user.0.identity, &project, &name)?;
    let trace = run_stream(&state, &project, |capture| {
        connection_test(&name, &project, &source, capture)
    })
    .await?;
    Ok(Json(answer_of(trace)))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/datasources/{name}/test",
        post(test_datasource),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(value: Value) -> DataSourceSpec {
        serde_json::from_value(value).expect("a DataSource spec")
    }

    #[test]
    fn an_http_source_is_one_fetch_with_a_short_timeout() {
        let source =
            spec(json!({ "type": "http", "http": { "url": "https://feeds.example/aq.json" } }));
        let config = connection_test("aq", "helsinki", &source, "http://portal/x").expect("config");
        assert_eq!(config["input"]["generate"]["count"], 1);
        let fetch = &config["pipeline"]["processors"][0]["http"];
        assert_eq!(fetch["url"], "https://feeds.example/aq.json");
        assert_eq!(fetch["timeout"], "3s");
        assert_eq!(fetch["retries"], 0);
        assert_eq!(config["output"]["http_client"]["url"], "http://portal/x");
    }

    #[test]
    fn a_runner_input_runs_as_written_and_each_record_is_cut() {
        let source = spec(json!({
            "type": "nats",
            "input": { "urls": ["nats://demo-nats:4222"], "subject": "helsinki.demo.counters" }
        }));
        let config = connection_test("demo-counters", "helsinki", &source, "http://portal/x")
            .expect("config");
        assert_eq!(config["input"]["nats"]["subject"], "helsinki.demo.counters");
        let text = config["pipeline"].to_string();
        assert!(
            text.contains(&format!("slice(0, {RECORD_BYTES})")),
            "{text}"
        );
    }

    #[test]
    fn an_address_the_runner_may_not_reach_is_refused_before_anything_runs() {
        for source in [
            json!({ "type": "http", "http": { "url": "http://169.254.169.254/latest" } }),
            json!({ "type": "nats", "input": { "urls": ["nats://127.0.0.1:4222"], "subject": "x" } }),
        ] {
            assert!(
                matches!(
                    connection_test("meta", "helsinki", &spec(source.clone()), "http://portal/x"),
                    Err(ApiError::BadRequest(_))
                ),
                "{source}"
            );
        }
    }

    fn input_of_test(kind: &str, input: Value) -> Value {
        let source = spec(json!({ "type": kind, "input": input }));
        connection_test("live", "helsinki", &source, "http://portal/x").expect("config")["input"]
            [kind]
            .clone()
    }

    #[test]
    fn a_kafka_test_joins_no_consumer_group_and_commits_nothing() {
        let input = json!({ "addresses": ["kafka.example:9092"], "topics": ["t"], "consumer_group": "live" });
        assert_eq!(input_of_test("kafka", input.clone())["consumer_group"], "");
        assert!(input_of_test("kafka_franz", input.clone())
            .get("consumer_group")
            .is_none());
    }

    #[test]
    fn a_nats_test_joins_no_queue_group_or_durable_consumer() {
        let input = json!({ "urls": ["nats://nats.example:4222"], "subject": "s", "queue": "live", "durable": "live" });
        let jetstream = input_of_test("nats_jetstream", input.clone());
        assert!(
            jetstream.get("durable").is_none() && jetstream.get("queue").is_none(),
            "{jetstream}"
        );
        assert!(input_of_test("nats", input).get("queue").is_none());
    }

    #[test]
    fn an_mqtt_test_drops_a_shared_subscription_and_keeps_no_session() {
        let source = spec(json!({ "type": "mqtt", "mqtt": {
            "urls": ["tcp://mqtt.example:1883"], "topics": ["$share/live/hfp/#", "plain/#"], "cleanSession": false
        } }));
        let config =
            connection_test("live", "helsinki", &source, "http://portal/x").expect("config");
        assert_eq!(
            config["input"]["mqtt"]["topics"],
            json!(["hfp/#", "plain/#"])
        );
        assert_eq!(config["input"]["mqtt"]["clean_session"], true);
    }

    #[test]
    fn a_queue_input_or_a_bound_consumer_is_refused_before_anything_runs() {
        for (kind, input) in [
            (
                "amqp_0_9",
                json!({ "urls": ["amqp://rabbit.example:5672"], "queue": "live" }),
            ),
            ("aws_sqs", json!({ "url": "https://sqs.example/live" })),
            (
                "nats_jetstream",
                json!({ "urls": ["nats://nats.example:4222"], "stream": "s", "durable": "live", "bind": true }),
            ),
        ] {
            let source = spec(json!({ "type": kind, "input": input }));
            let refused = connection_test("live", "helsinki", &source, "http://portal/x");
            assert!(
                matches!(&refused, Err(ApiError::BadRequest(why)) if why.contains(kind)),
                "{kind}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn only_a_caller_who_proposes_the_saved_source_gets_it() {
        use crate::config::Config;
        use crate::permissions::ORG_NAMESPACE;
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        use crate::store::Mirror;
        use std::sync::Arc;

        let envelope = |kind: &str, name: &str, namespace: &str, spec: Value| ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        };
        let mirror = Arc::new(Mirror::new());
        for (role, verb, user) in [
            ("author", "propose", "dev@hel.fi"),
            ("reader", "read", "reader@hel.fi"),
        ] {
            mirror.upsert(envelope(
                "Role",
                role,
                ORG_NAMESPACE,
                json!({ "rules": [{ "kinds": ["DataSource"], "verbs": [verb] }] }),
            ));
            mirror.upsert(envelope(
                "RoleBinding",
                role,
                ORG_NAMESPACE,
                json!({ "subjects": [{ "user": user }], "role": role, "scope": { "project": "oulu" } }),
            ));
        }
        mirror.upsert(envelope(
            "DataSource",
            "aq",
            "oulu",
            json!({ "type": "http", "http": { "url": "https://feeds.example/aq.json" } }),
        ));
        let state = AppState::new(Config::for_tests(), None).with_mirror(mirror);
        let who = |email: &str| Identity {
            client: None,
            subject: format!("f:1:{email}"),
            username: email.to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        };
        assert!(saved_source(&state, &who("dev@hel.fi"), "oulu", "aq").is_ok());
        assert!(matches!(
            saved_source(&state, &who("reader@hel.fi"), "oulu", "aq"),
            Err(ApiError::Denied(_) | ApiError::Forbidden)
        ));
        assert!(matches!(
            saved_source(&state, &who("stranger@hel.fi"), "oulu", "aq"),
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            saved_source(&state, &who("dev@hel.fi"), "oulu", "nope"),
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            saved_source(&state, &who("dev@hel.fi"), "Oulu!", "aq"),
            Err(ApiError::NotFound(_))
        ));
        // The handler itself: a reader is refused before anything runs, and an author reaches the
        // runner, which this state does not have.
        let session = |email: &str| crate::auth::session::Session {
            identity: who(email),
            expires_at: 0,
            issued_at: 0,
            signed_in_at: None,
            id_token: String::new(),
            access_expires_at: 0,
            refresh_token: None,
        };
        let path = || Path(("oulu".to_owned(), "aq".to_owned()));
        let refused = test_datasource(
            CurrentUser(session("reader@hel.fi")),
            State(state.clone()),
            path(),
        )
        .await;
        assert!(matches!(
            refused,
            Err(ApiError::Denied(_) | ApiError::Forbidden)
        ));
        let unavailable =
            test_datasource(CurrentUser(session("dev@hel.fi")), State(state), path()).await;
        assert!(matches!(unavailable, Err(ApiError::Unavailable(_))));
    }
}
