//! The workbench's whole-source check (G-21, T-7085): the saved pipeline over every page of its
//! http DataSource as a dry run on a stub runner. It counts per rule what a live run's rejected
//! list holds for the same records, writes nothing, and answers a caller without read on the
//! pipeline that it is not there.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const CHECK: &str = "/api/v1/projects/helsinki/pipelines/stations/source-check";
const GROUPS: &str = "/api/v1/projects/helsinki/pipelines/stations/rejected/groups";

/// The Portal of a runner at `runner`: `jana` reads the pipeline, which reads a paged http
/// DataSource and writes into a space with a compiled model.
fn world(runner: &MockServer) -> AppState {
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(format!("{}/{{project}}", runner.uri()));
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
    let state = AppState::new(config, None);
    for resource in [
        envelope(
            "Role",
            "pipeline-reader",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
        ),
        envelope(
            "RoleBinding",
            "jana-pipeline-reader",
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": "jana@hel.fi" }],
                "role": "pipeline-reader",
                "scope": { "project": "helsinki" },
            }),
        ),
        envelope(
            "ContextSpace",
            "helsinki",
            "helsinki",
            json!({ "urnSegment": "helsinki", "dataModelRef": { "kind": "DataModel", "name": "bikes" } }),
        ),
        envelope(
            "Endpoint",
            "bikes",
            "helsinki",
            json!({ "contextSpaceRef": "helsinki", "slug": "k7r2m4xq9vbn3tdw6hcy5pajfe" }),
        ),
        envelope(
            "DataSource",
            "stations-feed",
            "helsinki",
            json!({
                "type": "http",
                "http": {
                    "url": "https://data.hel.fi/api/stations",
                    "records": "items",
                    "paging": { "mode": "page", "param": "page" }
                }
            }),
        ),
        envelope(
            "Pipeline",
            "stations",
            "helsinki",
            json!({
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "stations-feed" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes"
            }),
        ),
    ] {
        state.mirror.upsert(resource);
    }
    state.model_schemas.replace(HashMap::from([(
        ("helsinki".to_owned(), "helsinki".to_owned()),
        Arc::new(ModelSchema::compile(
            "bikes",
            "1.0.0",
            &json!({ "definitions": { "BikeStation": {
                "properties": { "id": { "type": "string" }, "capacity": { "type": "integer" } }
            }}}),
            &["BikeStation".to_owned()],
            false,
        )),
    )]));
    state
}

fn station(n: u32, capacity: Value) -> Value {
    json!({
        "id": format!("urn:ngsi-ld:BikeStation:s-{n}"),
        "type": "BikeStation",
        "capacity": { "type": "Property", "value": capacity }
    })
}

/// The fixture: what the steps made of every record of every page, and the one record a step
/// failed on.
fn fixture() -> (Vec<Value>, Value) {
    let records = vec![
        station(1, json!(12)),
        station(2, json!("twelve")),
        json!({ "id": "urn:ngsi-ld:Car:c-1", "type": "Car" }),
        station(3, json!(7)),
        station(4, json!("many")),
        station(5, json!(3)),
    ];
    (records, json!({ "raw": "s-6" }))
}

/// Posts `body` to the internal listener at `path` as the project's runner.
async fn internal(state: &AppState, path: &str, body: &Value) -> StatusCode {
    server::internal_app(state.clone())
        .oneshot(
            Request::post(path)
                .header(
                    "authorization",
                    format!(
                        "Bearer {}",
                        common::REALM.workload(common::PIPELINE_RUNNER_CLIENT)
                    ),
                )
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

/// The live run: the stage refuses what the model refuses and catches the step's failure, and
/// posts each to the rejected route, which lists it.
async fn live_run(state: &AppState) {
    let (records, failed) = fixture();
    let path = "/internal/pipelines/helsinki/stations/rejected";
    for record in records {
        let problems = state
            .model_schemas
            .get("helsinki", "helsinki")
            .expect("the model")
            .check(&record);
        if !problems.is_empty() {
            let refused = json!({ "record": record, "error": "the stage refused it" });
            assert_eq!(
                internal(state, path, &refused).await,
                StatusCode::NO_CONTENT
            );
        }
    }
    let refused = json!({ "record": failed, "error": "cannot add string to number", "step": "0" });
    assert_eq!(
        internal(state, path, &refused).await,
        StatusCode::NO_CONTENT
    );
}

/// The stub runner's side of the check: waits for the stream, then posts what the harness
/// would post for each record, and answers the harness it was given.
async fn play_runner(runner: &MockServer, state: &AppState) -> Value {
    let (records, failed) = fixture();
    for _ in 0..200 {
        let received = common::stream_requests(runner).await;
        if let Some(create) = received.iter().find(|r| r.method == "POST") {
            let harness: Value = serde_json::from_slice(&create.body).expect("a JSON harness");
            let capture = harness["output"]["http_client"]["url"]
                .as_str()
                .expect("the capture url");
            let path = &capture[capture.find("/internal/").expect("internal path")..];
            for record in records {
                let message = json!({ "output": record, "error": null });
                assert_eq!(
                    internal(state, path, &message).await,
                    StatusCode::NO_CONTENT
                );
            }
            let message = json!({
                "input": failed.to_string(),
                "error": "cannot add string to number",
                "step": "0",
                "steps": [null]
            });
            assert_eq!(
                internal(state, path, &message).await,
                StatusCode::NO_CONTENT
            );
            return harness;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("the runner never received the harness");
}

fn counts(groups: &Value) -> Vec<(String, String, u64)> {
    let mut counted: Vec<(String, String, u64)> = groups
        .as_array()
        .expect("groups")
        .iter()
        .map(|g| {
            (
                g["rule"].as_str().unwrap_or_default().to_owned(),
                g["path"].as_str().unwrap_or_default().to_owned(),
                g["count"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    counted.sort();
    counted
}

/// G-21 acceptance: the whole-source check reports the same counts per rule as a live run's
/// rejected list on the same fixture, reads every page of the source, writes nothing and holds
/// the project's test slot through the harness stream.
#[tokio::test]
async fn the_whole_source_check_counts_what_a_live_runs_rejected_list_holds() {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/helsinki/streams/pipeline-test-helsinki$"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&runner)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/helsinki/streams/pipeline-test-helsinki$"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&runner)
        .await;
    let state = world(&runner);

    let (check, harness) = tokio::join!(
        send(&state, person("jana"), "POST", CHECK, None),
        play_runner(&runner, &state),
    );
    assert_eq!(check.status, StatusCode::OK, "{}", check.text);
    let check: Value = serde_json::from_str(&check.text).expect("json");
    assert_eq!(
        state
            .rejected
            .count("helsinki", "stations")
            .await
            .expect("count"),
        0,
        "the check writes nothing, not even to the rejected list"
    );

    // Every page of the source: its own read with its paging, records split before the steps.
    let fetch = &harness["pipeline"]["processors"][0]["http"];
    assert_eq!(fetch["url"], "https://data.hel.fi/api/stations");
    assert_eq!(fetch["paginate"]["mode"], "page");
    assert!(harness["output"]["http_client"]["max_in_flight"].as_u64() > Some(1));
    assert!(
        harness.to_string().contains("slice(0, 50000)"),
        "every record of a page is kept"
    );

    live_run(&state).await;
    let live = send(&state, person("jana"), "GET", GROUPS, None).await;
    assert_eq!(live.status, StatusCode::OK, "{}", live.text);
    let live: Value = serde_json::from_str(&live.text).expect("json");

    assert_eq!(counts(&check["groups"]), counts(&live["groups"]), "{check}");
    assert_eq!(check["rejected"], live["total"], "{check}");
    assert_eq!(check["accepted"], 3, "{check}");
    assert_eq!(check["records"], 7, "{check}");
    assert_eq!(check["complete"], true, "{check}");
    assert_eq!(check["rows"].as_array().map(Vec::len), Some(4), "{check}");
    assert!(check["groups"][0]["fix"].is_string(), "{check}");
    runner.verify().await;
}

/// A caller without a session is 401, one without read on the pipeline is told it is not there,
/// and the runner is never asked.
#[tokio::test]
async fn the_check_needs_a_session_and_read_on_the_pipeline() {
    let runner = common::runner().await;
    let state = world(&runner);
    let anonymous = server::app(state.clone())
        .oneshot(
            Request::post(CHECK)
                // The CSRF pair without a session: CSRF is checked first.
                .header(header::COOKIE, format!("{CSRF_COOKIE}={}", common::CSRF))
                .header(CSRF_HEADER, common::CSRF)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let stranger = send(&state, person("mikko"), "POST", CHECK, None).await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
    let missing = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/pipelines/nobody/source-check",
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text);
    assert!(common::stream_requests(&runner).await.is_empty());
}

/// A pipeline whose source streams without a last page is 409 with what to use instead.
#[tokio::test]
async fn a_source_without_a_last_page_is_409_naming_the_sample() {
    let runner = common::runner().await;
    let state = world(&runner);
    state.mirror.upsert(envelope(
        "DataSource",
        "stations-feed",
        "helsinki",
        json!({ "type": "mqtt", "mqtt": { "brokerRef": { "kind": "MqttBroker", "name": "city" }, "topics": ["bikes/#"] } }),
    ));
    let refused = send(&state, person("jana"), "POST", CHECK, None).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(refused.text.contains("sample"), "{}", refused.text);
    assert!(common::stream_requests(&runner).await.is_empty());
}
