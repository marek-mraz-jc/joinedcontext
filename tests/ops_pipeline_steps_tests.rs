//! The workbench's steps as operations (T-2711, ADR-N-034 §3.5, PL-63): the sample of a source,
//! the mapping tried on it and the records checked against the target space's model, each through
//! `POST /api/v1/projects/{project}/ops/{name}`, the door the workbench, MCP and the assistant
//! share. The mapping step answers what the pipeline test answers for the same input; sampling and
//! trying need propose on Pipeline, validating needs read, and nobody else is let through.
mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn op(project: &str, name: &str) -> String {
    format!("/api/v1/projects/{project}/ops/{name}")
}

/// `dev` proposes pipelines, `jana` reads them, `mikko` holds nothing; `project` has one space
/// that names its model, an Endpoint into it and two DataSources.
fn mirror(project: &str) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for (role, verb, who) in [("author", "propose", "dev"), ("reader", "read", "jana")] {
        mirror.upsert(envelope(
            "Role",
            &format!("pipeline-{role}"),
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Pipeline", "DataSource"], "verbs": [verb] }] }),
        ));
        mirror.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{role}"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{who}@hel.fi") }],
                "role": format!("pipeline-{role}"),
                "scope": { "project": project },
            }),
        ));
    }
    mirror.upsert(envelope(
        "ContextSpace",
        "bikes",
        project,
        json!({ "urnSegment": "bikes", "dataModelRef": { "kind": "DataModel", "name": "bikes" } }),
    ));
    mirror.upsert(envelope(
        "ContextSpace",
        "loose",
        project,
        json!({ "urnSegment": "loose" }),
    ));
    mirror.upsert(envelope(
        "Endpoint",
        "bikes-write",
        project,
        json!({ "contextSpaceRef": "bikes", "slug": "k7r2m4xq9vbn3tdw6hcy5pajfe" }),
    ));
    mirror.upsert(envelope(
        "DataSource",
        "stations-csv",
        project,
        json!({ "type": "http", "http": { "url": "https://feeds.example/stations.csv?day=1", "format": "csv" } }),
    ));
    mirror.upsert(envelope(
        "DataSource",
        "locked",
        project,
        json!({ "type": "http", "http": {
            "url": "https://feeds.example/locked.json",
            "authorization": { "headerRef": { "name": "feed-key", "key": "token" } }
        } }),
    ));
    mirror.upsert(envelope(
        "DataSource",
        "broker",
        project,
        json!({ "type": "mqtt", "mqtt": {
            "urls": ["tcp://mqtt.example:1883"],
            "topics": ["$share/live/hfp/v2/journey/ongoing/vp/#"],
            "username": "reader",
            "passwordRef": { "name": "hfp-broker", "key": "password" }
        } }),
    ));
    // Another project's source: nobody here reads it by name.
    mirror.upsert(envelope(
        "DataSource",
        "elsewhere",
        "other-project",
        json!({ "type": "mqtt", "mqtt": { "urls": ["tcp://mqtt.example:1883"], "topics": ["x"] } }),
    ));
    mirror
}

fn config(runner: Option<&MockServer>) -> Config {
    let mut config = Config::for_tests();
    config.org_domain = Some("hel.fi".into());
    config.pipeline_runner_url = runner.map(|r| format!("{}/{{project}}", r.uri()));
    config.pipeline_test_capture_url = Some("http://portal-internal:9090".into());
    config.oidc = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("a realm")
    .oidc;
    config.pipeline_runner_client_id = Some(common::PIPELINE_RUNNER_CLIENT.into());
    config
}

/// A Portal of `project`, its runner mocked when given, the `bikes` space's model compiled.
fn state(project: &str, runner: Option<&MockServer>) -> AppState {
    let state = AppState::new(config(runner), None).with_mirror(mirror(project));
    state.model_schemas.replace(HashMap::from([(
        (project.to_owned(), "bikes".to_owned()),
        Arc::new(ModelSchema::compile(
            "bikes",
            "1.2.0",
            &json!({ "definitions": { "BikeStation": {
                "properties": { "id": { "type": "string" }, "capacity": { "type": "integer" } }
            }}}),
            &["BikeStation".to_owned()],
            false,
        )),
    )]));
    state
}

async fn runner() -> MockServer {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/[a-z-]+/streams/pipeline-test-[a-z0-9-]+$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/[a-z-]+/streams/pipeline-test-[a-z0-9-]+$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    runner
}

/// The runner's side of the `nth` harness it receives: answers `messages` on its capture route.
async fn play_runner(
    runner: &MockServer,
    state: &AppState,
    nth: usize,
    messages: &[Value],
) -> Value {
    for _ in 0..200 {
        let received = common::stream_requests(runner).await;
        if let Some(create) = received.iter().filter(|r| r.method == "POST").nth(nth) {
            let harness: Value = serde_json::from_slice(&create.body).expect("a JSON harness");
            let capture = harness["output"]["http_client"]["url"]
                .as_str()
                .expect("the capture url");
            let path = &capture[capture.find("/internal/").expect("internal path")..];
            for message in messages {
                let status = server::internal_app(state.clone())
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(path)
                            .header(
                                axum::http::header::AUTHORIZATION,
                                format!(
                                    "Bearer {}",
                                    common::REALM.workload(common::PIPELINE_RUNNER_CLIENT)
                                ),
                            )
                            .body(Body::from(message.to_string()))
                            .expect("request"),
                    )
                    .await
                    .expect("response")
                    .status();
                assert_eq!(status, StatusCode::NO_CONTENT);
            }
            return harness;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the runner never received harness {nth}");
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_sample_is_the_sources_first_records_with_their_fields() {
    let runner = runner().await;
    let state = state("sample", Some(&runner));
    let rows = [
        json!({ "input": "page", "output": { "station": "01", "capacity": "12" }, "error": null }),
        json!({ "input": "page", "output": { "station": "02", "free": "3" }, "error": null }),
    ];
    let uri = op("sample", "jc_pipeline_sample_source");
    let (answer, harness) = tokio::join!(
        send(
            &state,
            person("dev"),
            "POST",
            &uri,
            Some(json!({ "dataSource": "stations-csv" })),
        ),
        play_runner(&runner, &state, 0, &rows),
    );
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body = json_of(&answer.text);
    assert_eq!(body["count"], 2);
    assert_eq!(body["truncated"], false);
    assert_eq!(body["fields"], json!(["capacity", "free", "station"]));
    assert_eq!(body["records"][1]["free"], "3");
    // The feed is fetched once, split as CSV because the DataSource says `format: csv` (PL-114),
    // with no step of anyone's.
    let text = harness.to_string();
    assert!(
        text.contains("https://feeds.example/stations.csv?day=1"),
        "{text}"
    );
    assert!(text.contains("parse_csv"), "{text}");
    assert!(!text.contains("secretRef"));
}

#[tokio::test]
async fn a_source_with_a_credential_a_stream_or_no_name_says_what_to_give_instead() {
    let state = state("refusals", None);
    for (source, status, words) in [
        ("locked", StatusCode::BAD_REQUEST, "declares a credential"),
        ("nobody", StatusCode::NOT_FOUND, "'nobody' not found"),
        ("elsewhere", StatusCode::NOT_FOUND, "'elsewhere' not found"),
    ] {
        let answer = send(
            &state,
            person("dev"),
            "POST",
            &op("refusals", "jc_pipeline_sample_source"),
            Some(json!({ "dataSource": source })),
        )
        .await;
        assert_eq!(answer.status, status, "{source}: {}", answer.text);
        assert!(answer.text.contains(words), "{source}: {}", answer.text);
        if status == StatusCode::BAD_REQUEST {
            assert!(
                answer.text.contains("give the sample as text"),
                "{}",
                answer.text
            );
        }
    }
    let both = send(
        &state,
        person("dev"),
        "POST",
        &op("refusals", "jc_pipeline_sample_source"),
        Some(json!({ "dataSource": "stations-csv", "sample": { "text": "a,b\n1,2\n", "format": "csv" } })),
    )
    .await;
    assert_eq!(
        both.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        both.text
    );
}

#[tokio::test]
async fn trying_a_mapping_answers_what_the_pipeline_test_answers() {
    let runner = runner().await;
    let state = state("mapping", Some(&runner));
    let mapping = "root.id = \"urn:ngsi-ld:BikeStation:hel.fi:bikes:\" + this.station\nroot.type = \"BikeStation\"\nroot.capacity = this.capacity.number()";
    let pipeline = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "stations" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "stations-csv" } },
            "compute": { "kind": "bloblang", "bloblang": mapping },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-write"
        }
    });
    let sample = json!({ "text": "station,capacity\n01,12\n02,x\n", "format": "csv" });
    let captured = [
        json!({ "input": "{\"station\":\"01\",\"capacity\":\"12\"}", "output": { "id": "urn:ngsi-ld:BikeStation:hel.fi:bikes:01", "type": "BikeStation", "capacity": 12 }, "error": null }),
        json!({ "input": "{\"station\":\"02\",\"capacity\":\"x\"}", "output": null, "error": "failed assignment (line 3): strconv.ParseFloat: parsing \"x\": invalid syntax", "step": "0" }),
    ];
    let body = json!({ "pipeline": pipeline, "sample": sample });

    let uri = op("mapping", "jc_pipeline_try_mapping");
    let (tried, _) = tokio::join!(
        send(&state, person("dev"), "POST", &uri, Some(body.clone())),
        play_runner(&runner, &state, 0, &captured),
    );
    let (tested, _) = tokio::join!(
        send(
            &state,
            person("dev"),
            "POST",
            "/api/v1/projects/mapping/pipelines/test",
            Some(body)
        ),
        play_runner(&runner, &state, 1, &captured),
    );
    assert_eq!(tried.status, StatusCode::OK, "{}", tried.text);
    assert_eq!(tested.status, StatusCode::OK, "{}", tested.text);
    let (tried, tested) = (json_of(&tried.text), json_of(&tested.text));
    assert_eq!(tried["records"], tested["mapping"]);
    assert_eq!(tried["errors"], tested["errors"]);
    assert_eq!(tried["input"], tested["input"]);
    assert_eq!(tried["errors"][0]["line"], 3);
    assert_eq!(tried["errors"][0]["step"], 0);
}

#[tokio::test]
async fn sampling_and_trying_need_propose_and_a_reader_is_told_so() {
    let state = state("gate", None);
    let sample = json!({ "text": "a\n1\n", "format": "csv" });
    for (name, body) in [
        ("jc_pipeline_sample_source", json!({ "sample": sample })),
        (
            "jc_pipeline_try_mapping",
            json!({ "draft": { "kind": "Pipeline", "name": "x" }, "sample": sample }),
        ),
    ] {
        for who in ["jana", "mikko"] {
            let answer = send(
                &state,
                person(who),
                "POST",
                &op("gate", name),
                Some(body.clone()),
            )
            .await;
            assert_eq!(
                answer.status,
                StatusCode::FORBIDDEN,
                "{name} as {who}: {}",
                answer.text
            );
        }
    }
}

#[tokio::test]
async fn validating_gives_a_verdict_per_record_with_the_rule_each_broke() {
    let state = state("check", None);
    let pipeline = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "stations" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "stations-csv" } },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-write"
        }
    });
    let records = json!([
        { "id": "urn:ngsi-ld:BikeStation:hel.fi:bikes:01", "type": "BikeStation", "capacity": { "type": "Property", "value": 12 } },
        { "id": "urn:ngsi-ld:BikeStation:hel.fi:bikes:02", "type": "BikeStation", "capacity": { "type": "Property", "value": "x" } },
        // An id must be an NGSI-LD URN of the record's type (PF-43); another organization's prefix
        // would be an id like any other since ADR-N-041.
        { "id": "urn:ngsi-ld:Tram:hel.fi:bikes:03", "type": "BikeStation" },
        { "id": "urn:ngsi-ld:Tram:hel.fi:bikes:04", "type": "Tram" }
    ]);
    // A reader may check: the verdict writes nothing and names only the model's rules.
    let answer = send(
        &state,
        person("jana"),
        "POST",
        &op("check", "jc_pipeline_validate"),
        Some(json!({ "pipeline": pipeline, "records": records })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body = json_of(&answer.text);
    assert_eq!(body["space"], "bikes");
    assert_eq!(body["model"], "bikes");
    assert_eq!(body["version"], "1.2.0");
    assert_eq!(
        (body["valid"].as_u64(), body["rejected"].as_u64()),
        (Some(1), Some(3))
    );
    let verdicts = body["verdicts"].as_array().expect("verdicts");
    assert_eq!(verdicts[0]["ok"], true);
    assert_eq!(verdicts[1]["problems"][0]["rule"], "sh:datatype");
    assert_eq!(verdicts[1]["problems"][0]["path"], "capacity");
    assert!(verdicts[2]["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .any(|p| p["rule"] == "id"));
    assert_eq!(verdicts[3]["problems"][0]["rule"], "type");
    assert!(
        !answer.text.contains("\"x\""),
        "a problem never quotes the value: {}",
        answer.text
    );

    // With the pipeline each record also names its Endpoint and its findings (T-4840): here the
    // model's rules, each one a problem.
    assert_eq!(verdicts[0]["endpoint"], "bikes-write");
    assert_eq!(verdicts[1]["findings"][0]["source"], "model");
    assert_eq!(verdicts[1]["findings"][0]["field"], "capacity");

    // With a type, the answer lists each of its attributes as the pipeline may write it through
    // its Endpoint (T-4842); this Portal writes no broker views, so the grant alone decides.
    let judged = send(
        &state,
        person("jana"),
        "POST",
        &op("check", "jc_pipeline_validate"),
        Some(json!({ "pipeline": pipeline, "records": [], "type": "BikeStation" })),
    )
    .await;
    assert_eq!(judged.status, StatusCode::OK, "{}", judged.text);
    let judged = json_of(&judged.text);
    let names: Vec<&str> = judged["writable"]
        .as_array()
        .expect("writable")
        .iter()
        .filter_map(|w| w["name"].as_str())
        .collect();
    assert!(names.contains(&"capacity"), "{judged}");
    assert!(judged["writable"]
        .as_array()
        .is_some_and(|all| all.iter().all(|w| w["writable"].is_boolean())));
    let unknown = send(
        &state,
        person("jana"),
        "POST",
        &op("check", "jc_pipeline_validate"),
        Some(json!({ "pipeline": pipeline, "records": [], "type": "Ferry" })),
    )
    .await;
    assert_eq!(
        unknown.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        unknown.text
    );
    assert!(
        unknown.text.contains("Ferry is not a class"),
        "{}",
        unknown.text
    );

    // The space named directly gives the same verdicts by its model.
    let direct = send(
        &state,
        person("jana"),
        "POST",
        &op("check", "jc_pipeline_validate"),
        Some(json!({ "space": "bikes", "records": records })),
    )
    .await;
    let direct = json_of(&direct.text);
    for (at, verdict) in verdicts.iter().enumerate() {
        assert_eq!(direct["verdicts"][at]["ok"], verdict["ok"]);
        assert_eq!(direct["verdicts"][at]["problems"], verdict["problems"]);
        assert!(direct["verdicts"][at].get("findings").is_none());
    }
}

#[tokio::test]
async fn validating_refuses_a_space_without_a_model_too_many_records_and_a_stranger() {
    let state = state("edges", None);
    let one = json!([{ "id": "urn:ngsi-ld:BikeStation:hel.fi:loose:1", "type": "BikeStation" }]);
    let loose = send(
        &state,
        person("jana"),
        "POST",
        &op("edges", "jc_pipeline_validate"),
        Some(json!({ "space": "loose", "records": one })),
    )
    .await;
    assert_eq!(loose.status, StatusCode::CONFLICT, "{}", loose.text);
    assert!(loose.text.contains("spec.dataModelRef"), "{}", loose.text);

    let missing = send(
        &state,
        person("jana"),
        "POST",
        &op("edges", "jc_pipeline_validate"),
        Some(json!({ "space": "nowhere", "records": one })),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text);

    let nothing = send(
        &state,
        person("jana"),
        "POST",
        &op("edges", "jc_pipeline_validate"),
        Some(json!({ "records": one })),
    )
    .await;
    assert_eq!(
        nothing.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        nothing.text
    );
    assert!(
        nothing.text.contains("give pipeline, draft or space"),
        "{}",
        nothing.text
    );

    let many: Vec<Value> = (0..101).map(|n| json!({ "id": n })).collect();
    let too_many = send(
        &state,
        person("jana"),
        "POST",
        &op("edges", "jc_pipeline_validate"),
        Some(json!({ "space": "bikes", "records": many })),
    )
    .await;
    assert_eq!(
        too_many.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        too_many.text
    );

    let stranger = send(
        &state,
        person("mikko"),
        "POST",
        &op("edges", "jc_pipeline_validate"),
        Some(json!({ "space": "bikes", "records": one })),
    )
    .await;
    assert_eq!(stranger.status, StatusCode::FORBIDDEN, "{}", stranger.text);
}

/// What the runner posts for one streamed message, as the sample's envelope maps it.
fn streamed(message: Value) -> Value {
    json!({ "input": message.to_string(), "output": message, "error": null })
}

#[tokio::test]
async fn a_streaming_source_is_listened_to_and_each_message_is_one_record() {
    let runner = runner().await;
    let state = state("listen", Some(&runner));
    let messages: Vec<Value> = (0..20)
        .map(|n| streamed(json!({ "VP": { "veh": n, "lat": 60.17 } })))
        .collect();
    let uri = op("listen", "jc_pipeline_sample_source");
    let (answer, harness) = tokio::join!(
        send(
            &state,
            person("dev"),
            "POST",
            &uri,
            Some(json!({ "dataSource": "broker" })),
        ),
        play_runner(&runner, &state, 0, &messages),
    );
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body = json_of(&answer.text);
    assert_eq!(body["streamed"], true);
    assert_eq!(body["count"], 20);
    assert_eq!(
        body["truncated"], true,
        "20 messages is the most a sample takes"
    );
    assert_eq!(body["fields"], json!(["VP"]));
    assert_eq!(body["records"][3]["VP"]["veh"], 3);

    // The saved source's broker and topic, read beside the live pipelines: no shared
    // subscription, a session the broker drops, a client id no Pipeline can have.
    let mqtt = &harness["input"]["mqtt"];
    assert_eq!(mqtt["urls"], json!(["tcp://mqtt.example:1883"]));
    assert_eq!(mqtt["topics"], json!(["hfp/v2/journey/ongoing/vp/#"]));
    assert_eq!(mqtt["clean_session"], true);
    assert!(
        mqtt["client_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("jc-listen-workbench_sample_")
                && harness["output"]["http_client"]["url"]
                    .as_str()
                    .is_some_and(|url| id.contains(url.rsplit('/').next().unwrap_or("?")))),
        "{mqtt}"
    );
    // The credential is the runner's own variable, never a value and never a secretRef name.
    let password = mqtt["password"].as_str().expect("a password variable");
    assert!(
        password.starts_with("${") && password.ends_with('}'),
        "{password}"
    );
    assert!(!answer.text.contains(password), "{}", answer.text);
    // It reads and publishes nothing: its one output is the capture route.
    let output = harness["output"].as_object().expect("an output");
    assert!(output.contains_key("http_client"), "{output:?}");
    assert!(output.get("mqtt").is_none(), "{output:?}");
}

#[tokio::test]
async fn a_quiet_topic_answers_no_records_after_five_seconds_and_an_oversized_message_is_named() {
    let runner = runner().await;
    let state = state("quiet", Some(&runner));
    let uri = op("quiet", "jc_pipeline_sample_source");
    let started = std::time::Instant::now();
    let (answer, _) = tokio::join!(
        send(
            &state,
            person("dev"),
            "POST",
            &uri,
            Some(json!({ "dataSource": "broker" }))
        ),
        play_runner(&runner, &state, 0, &[]),
    );
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(
        started.elapsed() >= Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let body = json_of(&answer.text);
    assert_eq!(
        (body["count"].clone(), body["truncated"].clone()),
        (json!(0), json!(false))
    );
    assert_eq!(body["records"], json!([]));

    let oversized = json!({
        "input": "",
        "error": "a message of 70000 bytes is over the 65536 bytes a sample keeps of one message"
    });
    let posted = [streamed(json!({ "VP": { "veh": 1 } })), oversized];
    let (answer, _) = tokio::join!(
        send(
            &state,
            person("dev"),
            "POST",
            &uri,
            Some(json!({ "dataSource": "broker" }))
        ),
        play_runner(&runner, &state, 1, &posted),
    );
    let body = json_of(&answer.text);
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(body["errors"][0]["stage"], "source");
    assert!(body["errors"][0]["message"]
        .as_str()
        .is_some_and(|m| m.contains("70000 bytes")));
}

#[tokio::test]
async fn another_projects_source_is_not_listened_to_and_listening_is_rate_limited() {
    let state = state("bounds", None);
    // A caller with no grant in the source's own project is refused there.
    let answer = send(
        &state,
        person("dev"),
        "POST",
        &op("other-project", "jc_pipeline_sample_source"),
        Some(json!({ "dataSource": "elsewhere" })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);

    // kim proposes here; with no runner each listen is 503, and the seventh in a minute is 429.
    state.mirror.upsert(envelope(
        "RoleBinding",
        "kim-author",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "kim@hel.fi" }],
            "role": "pipeline-author",
            "scope": { "project": "bounds" },
        }),
    ));
    let uri = op("bounds", "jc_pipeline_sample_source");
    for n in 0..6 {
        let answer = send(
            &state,
            person("kim"),
            "POST",
            &uri,
            Some(json!({ "dataSource": "broker" })),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{n}: {}",
            answer.text
        );
    }
    let answer = send(
        &state,
        person("kim"),
        "POST",
        &uri,
        Some(json!({ "dataSource": "broker" })),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        answer.text
    );
    // The limit is the person's: the request path of another source is not held by it.
    let answer = send(
        &state,
        person("kim"),
        "POST",
        &uri,
        Some(json!({ "sample": { "text": "a\n1\n", "format": "csv" } })),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
}

#[tokio::test]
async fn streamed_messages_are_tried_one_each_and_bounded() {
    let runner = runner().await;
    let state = state("messages", Some(&runner));
    let pipeline = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "hfp" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "broker" } },
            "compute": { "kind": "bloblang", "bloblang": "root.id = this.VP.veh.string()" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-write"
        }
    });
    let uri = op("messages", "jc_pipeline_try_mapping");
    let captured =
        [json!({ "input": "{\"VP\":{\"veh\":1}}", "output": { "id": "1" }, "error": null })];
    let (tried, harness) = tokio::join!(
        send(
            &state,
            person("dev"),
            "POST",
            &uri,
            Some(json!({ "pipeline": pipeline, "messages": [{ "VP": { "veh": 1 } }, "plain"] })),
        ),
        play_runner(&runner, &state, 0, &captured),
    );
    assert_eq!(tried.status, StatusCode::OK, "{}", tried.text);
    assert_eq!(json_of(&tried.text)["records"][0]["id"], "1");
    assert_eq!(
        harness["pipeline"]["processors"][0]["unarchive"]["format"], "json_array",
        "each message reaches the mapping alone: {harness}"
    );

    let many: Vec<Value> = (0..21).map(|n| json!({ "n": n })).collect();
    let huge = "x".repeat(70_000);
    for body in [
        json!({ "pipeline": pipeline, "messages": many }),
        json!({ "pipeline": pipeline, "messages": [huge] }),
        json!({ "pipeline": pipeline, "messages": [] }),
        json!({ "pipeline": pipeline, "messages": [1], "sample": { "text": "a", "format": "text" } }),
        json!({ "pipeline": pipeline }),
    ] {
        let answer = send(&state, person("dev"), "POST", &uri, Some(body.clone())).await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            answer.text
        );
    }
}

/// `dev`'s plan adding DataSource `name` at `url`: its answer.
async fn plan(state: &AppState, project: &str, name: &str, url: &str) -> common::Answer {
    send(
        state,
        person("dev"),
        "POST",
        &op(project, "jc_change_plan"),
        Some(
            json!({ "changes": [{ "op": "create", "kind": "DataSource", "name": name,
            "manifest": { "apiVersion": "joinedcontext.com/v1alpha1", "kind": "DataSource",
                "metadata": { "name": name, "namespace": project },
                "spec": { "type": "http", "http": { "url": url, "format": "csv" } } } }] }),
        ),
    )
    .await
}

/// The planId of `dev`'s plan adding DataSource `name` at `url`.
async fn planned(state: &AppState, project: &str, name: &str, url: &str) -> String {
    let answer = plan(state, project, name, url).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    json_of(&answer.text)["planId"]
        .as_str()
        .expect("a planId")
        .to_owned()
}
/// T-6269: an open-data import samples the DataSource it plans before an approver merges it.
#[tokio::test]
async fn a_planned_data_source_is_sampled_before_its_approval() {
    let runner = runner().await;
    let state = state("planned", Some(&runner));
    let url = "https://93.184.215.14/parkoviska.csv";
    let rows =
        [json!({ "input": "page", "output": { "lot": "P1", "spaces": "40" }, "error": null })];
    let (id, _) = tokio::join!(
        planned(&state, "planned", "parkoviska", url),
        play_runner(&runner, &state, 0, &rows),
    );
    let uri = op("planned", "jc_pipeline_sample_source");
    let (answer, harness) = tokio::join!(
        send(
            &state,
            person("dev"),
            "POST",
            &uri,
            Some(json!({ "dataSource": "parkoviska", "planId": id })),
        ),
        play_runner(&runner, &state, 1, &rows),
    );
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body = json_of(&answer.text);
    assert_eq!(body["fields"], json!(["lot", "spaces"]));
    assert!(harness.to_string().contains(url), "{harness}");
}

/// T-6269: a planned source nobody approved is held to public addresses, to its own plan and to
/// the caller who planned it.
#[tokio::test]
async fn a_planned_data_source_on_a_private_or_unknown_host_or_of_another_plan_is_refused() {
    let state = state("guarded", None);
    // A plan never holds a loopback, link-local or private address: the runner is never asked.
    for url in [
        "http://127.0.0.1:9000/x.csv",
        "http://169.254.169.254/latest/meta-data",
        "http://localhost:8080/x.csv",
    ] {
        let answer = plan(&state, "guarded", "inside", url).await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{url}: {}",
            answer.text
        );
        assert!(
            answer.text.contains("private address"),
            "{url}: {}",
            answer.text
        );
    }
    // A name the plan admits is judged by what it resolves to when sampled.
    let id = planned(&state, "guarded", "unknown", "https://feed.invalid/x.csv").await;
    let answer = send(
        &state,
        person("dev"),
        "POST",
        &op("guarded", "jc_pipeline_sample_source"),
        Some(json!({ "dataSource": "unknown", "planId": id })),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    assert!(answer.text.contains("does not resolve"), "{}", answer.text);
    let id = planned(&state, "guarded", "fine", "https://93.184.215.14/x.csv").await;
    for (who, source, plan, status) in [
        ("dev", "other", id.as_str(), StatusCode::NOT_FOUND),
        ("dev", "fine", "nope", StatusCode::NOT_FOUND),
        ("jana", "fine", id.as_str(), StatusCode::FORBIDDEN),
    ] {
        let answer = send(
            &state,
            person(who),
            "POST",
            &op("guarded", "jc_pipeline_sample_source"),
            Some(json!({ "dataSource": source, "planId": plan })),
        )
        .await;
        assert_eq!(
            answer.status, status,
            "{who} {source} {plan}: {}",
            answer.text
        );
    }
    let alone = send(
        &state,
        person("dev"),
        "POST",
        &op("guarded", "jc_pipeline_sample_source"),
        Some(json!({ "planId": id, "sample": { "text": "a\n1\n", "format": "csv" } })),
    )
    .await;
    assert_eq!(
        alone.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        alone.text
    );
}
