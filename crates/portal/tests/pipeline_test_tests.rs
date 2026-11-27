//! A candidate pipeline tested on the project's runner (T-0591, PL-43, MF-38): the harness
//! goes to the runner's streams API, what it posts back is the trace, the stream is deleted
//! whatever happened, and nothing else is touched.
mod common;
use common::csrf_echo::EchoCsrf;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::CSRF_COOKIE;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "test-csrf-token-12345";

fn identity(email: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn cookies(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let s = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        stepped_up_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    parts.push(format!(
        "{CSRF_COOKIE}={}",
        joinedcontext_portal::auth::csrf::Token::bound(&config.cookie_key, &s).as_str()
    ));
    parts.join("; ")
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// A role that proposes pipelines in `project`, bound to the developer.
///
/// Every test that reaches the runner takes a project of its own: one pipeline test runs per
/// project at a time, process-wide (`pipeline_test::RUNNING`), and the tests of this file run in
/// parallel in one process, so two sharing a project refuse each other whenever they overlap —
/// which a slower CI runner makes them do (T-1458).
fn mirror(project: &str) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(org(
        "Role",
        "pipeline-author",
        json!({ "rules": [{ "kinds": ["Pipeline", "DataSource"], "verbs": ["propose"] }] }),
    ));
    mirror.upsert(org(
        "RoleBinding",
        "dev-authors",
        json!({
            "subjects": [{ "user": "dev@hel.fi" }],
            "role": "pipeline-author",
            "scope": { "project": project }
        }),
    ));
    mirror
}

fn request(bloblang: &str) -> Value {
    json!({
        "pipeline": {
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "shmu-air-quality" },
            "spec": {
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "shmu-csv" } },
                "compute": { "kind": "bloblang", "bloblang": bloblang },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all"
            }
        },
        "sample": { "text": "station_id,pm10\n01,18.2\n02,x\n", "format": "csv" }
    })
}

fn config(runner: Option<&MockServer>) -> Config {
    let mut config = Config::for_tests();
    config.pipeline_runner_url = runner.map(|r| format!("{}/{{project}}", r.uri()));
    config.pipeline_test_capture_url = Some("http://portal-internal:9090".into());
    // The capture route takes the runner's own ServiceAccount token (T-2271); without the realm and
    // the client, this Portal refuses every message, which is the point of the route's own test.
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

/// The header the project's pipeline runner posts a capture with.
fn runner_bearer() -> String {
    format!(
        "Bearer {}",
        common::REALM.workload(common::PIPELINE_RUNNER_CLIENT)
    )
}

async fn post(state: &AppState, who: &str, project: &str, body: &Value) -> (StatusCode, Value) {
    send(
        state,
        who,
        &format!("/api/v1/projects/{project}/pipelines/test"),
        body,
    )
    .await
}

async fn send(state: &AppState, who: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookies(&state.config, identity(who)))
                .echo_csrf()
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The runner's side: waits for the harness to arrive, answers for it on the capture route.
async fn play_runner(runner: &MockServer, state: &AppState, messages: &[Value]) -> Value {
    play_runner_as(
        runner,
        state,
        messages,
        &runner_bearer(),
        StatusCode::NO_CONTENT,
    )
    .await
}

/// `play_runner` presenting `bearer` on the capture route, which answers `expect` to each message.
async fn play_runner_as(
    runner: &MockServer,
    state: &AppState,
    messages: &[Value],
    bearer: &str,
    expect: StatusCode,
) -> Value {
    for _ in 0..100 {
        let received = common::stream_requests(runner).await;
        if let Some(create) = received.iter().find(|r| r.method == "POST") {
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
                            .header(axum::http::header::AUTHORIZATION, bearer)
                            .body(Body::from(message.to_string()))
                            .expect("request"),
                    )
                    .await
                    .expect("response")
                    .status();
                assert_eq!(status, expect);
            }
            return harness;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("the runner never received the harness");
}

fn runner_mocks(create: ResponseTemplate) -> (Mock, Mock) {
    (
        Mock::given(method("POST"))
            .and(path_regex(r"^/[a-z-]+/streams/pipeline-test-[a-z0-9-]+$"))
            .respond_with(create)
            .expect(1),
        Mock::given(method("DELETE"))
            .and(path_regex(r"^/[a-z-]+/streams/pipeline-test-[a-z0-9-]+$"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1),
    )
}

#[tokio::test]
async fn the_trace_is_what_the_harness_posted_back_and_the_stream_is_deleted() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("helsinki"));

    let mapping = "root.id = \"urn:ngsi-ld:AirQualityObserved:hel.fi:aq:\" + this.station_id\nroot.type = \"AirQualityObserved\"\nroot.pm10 = this.pm10.number()";
    let messages = [
        json!({ "input": "{\"station_id\":\"01\",\"pm10\":\"18.2\"}", "output": { "id": "urn:ngsi-ld:AirQualityObserved:hel.fi:aq:01", "type": "AirQualityObserved", "pm10": 18.2 }, "error": null }),
        json!({ "input": "{\"station_id\":\"02\",\"pm10\":\"x\"}", "output": null, "error": "failed assignment (line 3): strconv.ParseFloat: parsing \"x\": invalid syntax" }),
    ];
    let body = request(mapping);
    let (answer, harness) = tokio::join!(
        post(&state, "dev@hel.fi", "helsinki", &body),
        play_runner(&runner, &state, &messages),
    );
    let (status, body) = answer;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["input"]["events"], 2);
    assert_eq!(body["input"]["sample"]["station_id"], "01");
    assert_eq!(
        body["mapping"][0]["id"],
        "urn:ngsi-ld:AirQualityObserved:hel.fi:aq:01"
    );
    assert_eq!(body["validation"][0]["ok"], true);
    assert_eq!(body["errors"][0]["stage"], "mapping");
    assert_eq!(body["errors"][0]["line"], 3);
    assert_eq!(
        body["errors"].as_array().map(Vec::len),
        Some(1),
        "a run that captured messages carries no runner error: {body}"
    );

    // The harness: the sample generated once, the author's mapping as its own processor, the
    // output pointed at the capture route; no secret, no target endpoint (MF-38).
    assert_eq!(harness["input"]["generate"]["count"], 1);
    let processors = harness["pipeline"]["processors"]
        .as_array()
        .expect("processors");
    assert!(processors.iter().any(|p| p["mapping"] == mapping));
    let text = harness.to_string();
    assert!(
        !text.contains("helsinki-all"),
        "the target endpoint is never in the harness"
    );
    assert!(!text.contains("secretRef") && !text.contains("resources"));
    runner.verify().await;
}

/// T-7699: an MQTT broker's test publish runs the mapping of the Pipeline its topic reaches on the
/// project's runner. A message the mapping fails and a record the Endpoint check reads each end
/// in their own step; the payload is never in a reason.
#[tokio::test]
async fn a_test_publish_runs_the_bound_pipelines_mapping_and_names_each_outcome() {
    let mirror = mirror("helsinki");
    mirror.upsert(org(
        "Role",
        "broker-steward",
        json!({ "rules": [{ "kinds": ["MqttBroker", "DataSource", "Pipeline"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(org(
        "RoleBinding",
        "dev-broker",
        json!({ "subjects": [{ "user": "dev@hel.fi" }], "role": "broker-steward", "scope": { "project": "helsinki" } }),
    ));
    let placed = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
        metadata: ObjectMeta::new(name, "helsinki"),
        ..org(kind, name, spec)
    };
    mirror.upsert(placed(
        "MqttBroker",
        "cameras",
        json!({ "roles": [{ "name": "camera", "serviceAccounts": ["cam-west"], "publish": ["cameras/${clientid}/#"] }] }),
    ));
    mirror.upsert(placed(
        "DataSource",
        "frames",
        json!({ "type": "mqtt", "mqtt": { "brokerRef": { "kind": "MqttBroker", "name": "cameras" }, "topics": ["cameras/+/frame"] } }),
    ));
    mirror.upsert(placed(
        "Pipeline",
        "frames-in",
        json!({
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "frames" } },
            "compute": { "kind": "bloblang", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:cams:frames"
        }),
    ));
    let publish = json!({ "account": "cam-west", "topic": "cameras/helsinki-cam-west/frame", "payload": "{\"seq\": 7}" });
    let uri = "/api/v1/projects/helsinki/mqttbrokers/cameras/test-publish";
    let failed = json!({ "input": "{\"seq\": 7}", "output": null, "error": "failed assignment (line 1): expected number" });
    let mapped = json!({ "input": "{\"seq\": 7}", "output": { "id": "urn:ngsi-ld:Frame:hel.fi:cams:7", "type": "Frame", "seq": 7 }, "error": null });
    for (message, mapping, reason) in [
        (&failed, "refused", "the mapping failed: "),
        (&mapped, "ok", "1 record(s)"),
    ] {
        let runner = common::runner().await;
        let (create, delete) = runner_mocks(ResponseTemplate::new(200));
        create.mount(&runner).await;
        delete.mount(&runner).await;
        let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror.clone());
        let (answer, _) = tokio::join!(
            send(&state, "dev@hel.fi", uri, &publish),
            play_runner(&runner, &state, std::slice::from_ref(message)),
        );
        let (status, body) = answer;
        assert_eq!(status, StatusCode::OK, "{body}");
        let steps = body["steps"].as_array().expect("steps");
        let step = |name: &str| {
            steps
                .iter()
                .find(|s| s["step"] == name)
                .cloned()
                .expect(name)
        };
        assert_eq!(step("mapping")["outcome"], mapping, "{body}");
        assert!(
            step("mapping")["reason"]
                .as_str()
                .is_some_and(|r| r.starts_with(reason)),
            "{body}"
        );
        let endpoint = step("endpoint");
        match mapping {
            "ok" => assert_ne!(
                endpoint["outcome"], "ok",
                "no Endpoint of that id exists: {body}"
            ),
            _ => assert_eq!(
                (endpoint["outcome"].as_str(), endpoint["reason"].as_str()),
                (Some("unchecked"), Some("no record to check")),
                "{body}"
            ),
        }
        assert!(
            steps.iter().all(|s| !s["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("{\"seq\": 7}")),
            "the payload stays out of every reason: {body}"
        );
    }
}

/// T-4050: with the pipelines' own identities on, the harness runs as the project's harness
/// principal through the token service: the stream the runner gets names no client secret, and
/// the capture route admits that principal of the test's own project.
#[tokio::test]
async fn with_pipeline_identity_a_test_runs_as_the_harness_principal_without_a_secret() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = identified(&runner, "kerava");
    let (one, bearer) = (one(), harness_bearer("kerava"));
    let body = request("root = this");
    let (answer, harness) = tokio::join!(
        post(&state, "dev@hel.fi", "kerava", &body),
        play_runner_as(&runner, &state, &one, &bearer, StatusCode::NO_CONTENT),
    );
    assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);
    assert_eq!(answer.1["input"]["events"], 1, "{}", answer.1);
    let oauth2 = &harness["output"]["http_client"]["oauth2"];
    assert_eq!(oauth2["client_key"], "kerava/pipeline-test", "{harness}");
    assert_eq!(oauth2["token_url"], "${JC_PIPELINE_TOKEN_URL}");
    assert!(!harness.to_string().contains("client_secret"), "{harness}");
    runner.verify().await;
}

/// T-4050: a DataSource probe is a harness too, and runs as the same principal.
#[tokio::test]
async fn with_pipeline_identity_a_probe_runs_as_the_harness_principal_without_a_secret() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = identified(&runner, "vantaa");
    let (one, bearer) = (one(), harness_bearer("vantaa"));
    let source = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "DataSource",
        "metadata": { "name": "bikes" },
        "spec": { "type": "http", "http": { "url": "https://feeds.example/bikes.json" } }
    });
    let (answer, probe) = tokio::join!(
        send(
            &state,
            "dev@hel.fi",
            "/api/v1/projects/vantaa/datasources?dryRun=All",
            &source
        ),
        play_runner_as(&runner, &state, &one, &bearer, StatusCode::NO_CONTENT),
    );
    assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);
    assert_eq!(answer.1["probe"]["records"], 1, "{}", answer.1);
    assert_eq!(
        probe["output"]["http_client"]["oauth2"]["client_key"],
        "vantaa/pipeline-test"
    );
    assert!(!probe.to_string().contains("client_secret"), "{probe}");
    runner.verify().await;
}

/// One captured message.
fn one() -> [Value; 1] {
    [json!({ "input": "{\"a\":1}", "output": { "a": 1 }, "error": null })]
}

/// A Portal whose streams run as their pipelines' own identities.
fn identified(runner: &MockServer, project: &str) -> AppState {
    let mut config = config(Some(runner));
    config.pipeline_identity = true;
    AppState::new(config, None).with_mirror(mirror(project))
}

/// The bearer of `project`'s harness principal.
fn harness_bearer(project: &str) -> String {
    format!(
        "Bearer {}",
        common::REALM.workload(&format!("{project}-pl-pipeline-test"))
    )
}

/// T-4050: another project's harness principal, or a pipeline's own client, is no harness of
/// this test: the capture route refuses it and the trace stays empty.
#[tokio::test]
async fn the_capture_refuses_another_projects_harness_principal() {
    for stranger in ["helsinki-pl-pipeline-test", "lahti-pl-odpad"] {
        let runner = common::runner().await;
        let (create, delete) = runner_mocks(ResponseTemplate::new(200));
        create.mount(&runner).await;
        delete.mount(&runner).await;
        let mut config = config(Some(&runner));
        config.pipeline_identity = true;
        let state = AppState::new(config, None).with_mirror(mirror("lahti"));
        let bearer = format!("Bearer {}", common::REALM.workload(stranger));
        let message = [json!({ "input": "{}", "output": {}, "error": null })];
        let body = request("root = this");
        let (answer, _) = tokio::join!(
            post(&state, "dev@hel.fi", "lahti", &body),
            play_runner_as(&runner, &state, &message, &bearer, StatusCode::UNAUTHORIZED),
        );
        assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);
        assert_eq!(answer.1["input"]["events"], 0, "{stranger}: {}", answer.1);
    }
}

/// A DataSource Check fetches the feed once on the runner (MF-39); a fetch that fails reaches
/// the capture route like any message, and the Check names what the feed answered.
#[tokio::test]
async fn a_data_source_check_names_what_the_feed_answered() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("porvoo"));
    let source = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "DataSource",
        "metadata": { "name": "bikes" },
        "spec": { "type": "http", "http": { "url": "https://feeds.example/bikes.json" } }
    });
    let failed = [json!({
        "input": null,
        "output": null,
        "error": "fetch: https://feeds.example/bikes.json: HTTP request returned unexpected response code (404): 404 Not Found, Error: gone"
    })];
    let (answer, harness) = tokio::join!(
        send(
            &state,
            "dev@hel.fi",
            "/api/v1/projects/porvoo/datasources?dryRun=All",
            &source
        ),
        play_runner(&runner, &state, &failed),
    );
    let (status, body) = answer;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["probe"]["skipped"], "the feed answered 404 Not Found",
        "{body}"
    );
    assert!(body["probe"].get("records").is_none());

    // One fetch by a processor whose failure is kept for the envelope, never a polling input.
    let processors = harness["pipeline"]["processors"]
        .as_array()
        .expect("processors");
    assert_eq!(
        processors[0]["http"]["url"],
        "https://feeds.example/bikes.json"
    );
    assert!(processors[2]["catch"].is_array());
    assert!(harness["input"].get("http_client").is_none());
    runner.verify().await;
}

#[tokio::test]
async fn a_harness_the_runner_refuses_is_a_lint_error_with_its_line() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(
        ResponseTemplate::new(400).set_body_string(
            "stream 'pipeline-test-x' failed to create: failed to parse mapping: line 2 char 9: expected whitespace",
        ),
    );
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("espoo-lint"));

    let (status, body) = post(
        &state,
        "dev@hel.fi",
        "espoo-lint",
        &request("root.id = = 1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["input"]["events"], 0);
    assert_eq!(body["errors"][0]["stage"], "lint");
    assert_eq!(body["errors"][0]["line"], 2);
    runner.verify().await;
}

/// T-1139, PL-43: a runner that answers with its own failure is the platform's problem, not the
/// caller's. It is `503` and no trace, because a trace of nothing would read as a pipeline that
/// produced nothing.
#[tokio::test]
async fn a_runner_that_fails_the_create_is_503_and_no_trace() {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/[a-z-]+/streams/pipeline-test-[a-z0-9-]+$"))
        .respond_with(ResponseTemplate::new(503).set_body_string("no capacity"))
        .expect(1)
        .mount(&runner)
        .await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("kuopio"));

    let (status, body) = post(&state, "dev@hel.fi", "kuopio", &request("root = this")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.get("errors").is_none(), "{body}");
    // The runner's own words stay inside the cluster: the caller learns the platform is at
    // fault and nothing about its capacity.
    assert!(!body.to_string().contains("no capacity"), "{body}");
    runner.verify().await;
}

#[tokio::test]
async fn what_the_kind_refuses_is_400_before_the_runner_is_asked() {
    let runner = common::runner().await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("vantaa"));
    let mut body = request("root = this");
    body["pipeline"]["spec"]["class"] = json!("scheduled");
    let (status, answer) = post(&state, "dev@hel.fi", "vantaa", &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(answer["detail"]
        .as_str()
        .is_some_and(|d| d.contains("Pipeline")));

    let mut body = request("root = this");
    body["sample"] = json!({ "format": "csv" });
    let (status, answer) = post(&state, "dev@hel.fi", "vantaa", &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(runner
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

#[tokio::test]
async fn a_caller_without_the_propose_verb_is_403_and_without_a_runner_it_is_503() {
    let runner = common::runner().await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("turku"));
    let (status, _) = post(&state, "viewer@hel.fi", "turku", &request("root = this")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let state = AppState::new(config(None), None).with_mirror(mirror("oulu"));
    let (status, body) = post(&state, "dev@hel.fi", "oulu", &request("root = this")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
}

/// T-7084: the script test needs a session (401) and `propose` on Script (403): a pipeline author
/// without it is refused before the runner is asked; one with it reaches the runner (503 here).
#[tokio::test]
async fn a_script_test_needs_a_session_and_the_propose_verb_on_script() {
    let body = json!({ "code": "bento.v0_msg_set_string('x')",
                       "tests": [{ "name": "a", "input": {}, "expected": {} }] });
    let uri = "/api/v1/projects/naantali/scripts/test";
    let state = AppState::new(config(None), None).with_mirror(mirror("naantali"));
    let (status, answer) = send(&state, "dev@hel.fi", uri, &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");

    let anonymous = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
                .echo_csrf()
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let scripts = mirror("naantali");
    scripts.upsert(org(
        "Role",
        "pipeline-author",
        json!({ "rules": [{ "kinds": ["Pipeline", "Script"], "verbs": ["propose"] }] }),
    ));
    let state = AppState::new(config(None), None).with_mirror(scripts.clone());
    let (status, answer) = send(&state, "dev@hel.fi", uri, &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");

    // A project name no project can have answers 404 before any right is read (T-7894).
    let (status, answer) = send(
        &state,
        "dev@hel.fi",
        "/api/v1/projects/Not_A_Name/scripts/test",
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");

    // With a runner each test runs once and answers its result, in the order sent.
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(scripts);
    let messages = [json!({ "input": "{}", "output": { "x": 1 }, "error": null })];
    let ((status, answer), _) = tokio::join!(
        send(&state, "dev@hel.fi", uri, &body),
        play_runner(&runner, &state, &messages),
    );
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["results"][0]["name"], "a", "{answer}");
    assert_eq!(answer["results"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn endpoint_sourced_candidate_with_sample_json_array_returns_trace() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("helsinki-kpi-test"));

    let mapping = "root = this";
    let body = json!({
        "pipeline": {
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "kpi-pipeline" },
            "spec": {
                "class": "scheduled",
                "schedule": "*/15 * * * *",
                "source": {
                    "endpointRef": { "kind": "Endpoint", "name": "helsinki-all" },
                    "query": { "type": "BikeHireDockingStation", "attrs": ["availableBikeNumber"] }
                },
                "compute": { "kind": "bloblang", "bloblang": mapping },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki-kpi:kpi-writer"
            }
        },
        "sample": {
            "text": "[{\"id\":\"urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:1\",\"type\":\"BikeHireDockingStation\",\"availableBikeNumber\":5}]",
            "format": "json"
        }
    });

    let messages = [json!({
        "input": "{\"id\":\"urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:1\",\"type\":\"BikeHireDockingStation\",\"availableBikeNumber\":5}",
        "output": {
            "id": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:1",
            "type": "BikeHireDockingStation",
            "availableBikeNumber": 5
        },
        "error": null
    })];

    let (answer, harness) = tokio::join!(
        post(&state, "dev@hel.fi", "helsinki-kpi-test", &body),
        play_runner(&runner, &state, &messages),
    );
    let (status, resp) = answer;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["input"]["events"], 1);
    assert_eq!(resp["mapping"].as_array().unwrap().len(), 1);
    assert_eq!(
        resp["mapping"][0]["id"],
        "urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:1"
    );

    assert_eq!(harness["input"]["generate"]["count"], 1);
    let processors = harness["pipeline"]["processors"]
        .as_array()
        .expect("processors");
    assert!(processors.iter().any(|p| p["mapping"] == mapping));
    runner.verify().await;
}

#[tokio::test]
async fn candidate_with_neither_source_is_400() {
    let runner = common::runner().await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("helsinki"));
    let body = json!({
        "pipeline": {
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "no-source-pipeline" },
            "spec": {
                "class": "resident",
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all"
            }
        },
        "sample": { "text": "foo", "format": "text" }
    });
    let (status, answer) = post(&state, "dev@hel.fi", "helsinki", &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
}

#[tokio::test]
async fn anonymous_is_401_and_a_capture_for_no_test_is_404() {
    let state = AppState::new(config(None), None).with_mirror(mirror("lahti"));
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/lahti/pipelines/test")
                .header(header::CONTENT_TYPE, "application/json")
                // The CSRF pair without a session: CSRF is checked first.
                .header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
                .echo_csrf()
                .body(Body::from(request("root = this").to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = server::internal_app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/pipeline-tests/nosuchtestnosuchtestnosuch")
                .header(axum::http::header::AUTHORIZATION, runner_bearer())
                .body(Body::from(r#"{"input":"x","output":null,"error":null}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // A feed-sized message (three mebibytes, over axum's default limit) is read, not 413.
    let big = format!(
        r#"{{"input":"{}","output":null,"error":null}}"#,
        "x".repeat(3 * 1024 * 1024)
    );
    let response = server::internal_app(AppState::new(config(None), None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/pipeline-tests/nosuchtestnosuchtestnosuch")
                .header(axum::http::header::AUTHORIZATION, runner_bearer())
                .body(Body::from(big))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// T-1027: a data source's Check says what it could not do. When no runner answers, the probe
/// is reported as skipped with a reason a person can read, never left out — a Check with no
/// probe section says nothing at all, and the recording stops on it.
#[tokio::test]
async fn a_data_source_check_with_no_runner_still_names_a_probe() {
    // No runner configured at all, the state the recording hits while the pod restarts.
    let state = AppState::new(config(None), None).with_mirror(mirror("loviisa"));
    let source = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "DataSource",
        "metadata": { "name": "bikes" },
        "spec": { "type": "http", "http": { "url": "https://feeds.example/bikes.json" } }
    });
    let (status, body) = send(
        &state,
        "dev@hel.fi",
        "/api/v1/projects/loviisa/datasources?dryRun=All",
        &source,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let skipped = body["probe"]["skipped"].as_str().unwrap_or_default();
    assert!(
        !skipped.is_empty(),
        "the Check names why the probe did not run: {body}"
    );
    assert!(
        body["probe"].get("records").is_none(),
        "a probe that did not run reports no records: {body}"
    );
    // The runner's absence is not the feed's fault, so the reason does not start like one.
    assert!(
        !skipped.starts_with("the feed "),
        "a runner that did not answer is not blamed on the feed: {skipped}"
    );

    // A runner that is there but refuses (starting up, or out of room) reads the same way.
    let refusing = common::runner().await;
    // Only the create call happens: the stream the delete would remove was never made.
    Mock::given(method("POST"))
        .and(path_regex(r"^/[a-z-]+/streams/pipeline-test-[a-z0-9-]+$"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&refusing)
        .await;
    let state = AppState::new(config(Some(&refusing)), None).with_mirror(mirror("hamina"));
    let (status, body) = send(
        &state,
        "dev@hel.fi",
        "/api/v1/projects/hamina/datasources?dryRun=All",
        &source,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let skipped = body["probe"]["skipped"].as_str().unwrap_or_default();
    assert!(!skipped.is_empty(), "the Check names a reason: {body}");
    assert!(
        !skipped.starts_with("the feed "),
        "a refusing runner is not blamed on the feed: {skipped}"
    );
}

/// How many requests of one method the runner saw.
async fn requests_of(runner: &MockServer, verb: &str) -> usize {
    common::stream_requests(runner)
        .await
        .iter()
        .filter(|r| r.method.as_str() == verb)
        .count()
}

/// A runner that answers the create with `create` and the delete with `delete`.
async fn runner_answering(create: ResponseTemplate, delete: ResponseTemplate) -> MockServer {
    let runner = common::runner().await;
    for (verb, answer) in [("POST", create), ("DELETE", delete)] {
        Mock::given(method(verb))
            .and(path_regex(
                r"^/[a-z0-9-]+/streams/pipeline-test-[a-z0-9-]+$",
            ))
            .respond_with(answer)
            .mount(&runner)
            .await;
    }
    runner
}

/// T-2565, PL-43: a runner that answered the create with 5xx, or too late, may still hold the
/// stream, so it is deleted all the same; the caller still gets 503.
#[tokio::test]
async fn the_stream_is_deleted_whatever_the_create_answered() {
    for (project, create) in [
        ("t2565-a", ResponseTemplate::new(502)),
        (
            "t2565-b",
            ResponseTemplate::new(200).set_delay(Duration::from_secs(5)),
        ),
    ] {
        let runner = runner_answering(create, ResponseTemplate::new(200)).await;
        let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror(project));
        let (status, body) = post(&state, "dev@hel.fi", project, &request("root = this")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{project}: {body}");
        assert_eq!(requests_of(&runner, "DELETE").await, 1, "{project}");
    }
}

/// T-2565, PL-43: a delete the runner refuses leaves the stream behind, and the trace says so
/// as a runner error; a delete of a stream already gone (404) is not a failure.
#[tokio::test]
async fn a_refused_delete_is_said_as_a_runner_error() {
    for (project, delete, said) in [
        ("t2565-c", ResponseTemplate::new(500), true),
        ("t2565-d", ResponseTemplate::new(404), false),
    ] {
        let create = ResponseTemplate::new(400).set_body_string("line 3 char 1: bad mapping");
        let runner = runner_answering(create, delete).await;
        let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror(project));
        let (status, body) = post(&state, "dev@hel.fi", project, &request("root = this")).await;
        assert_eq!(status, StatusCode::OK, "{project}: {body}");
        let runner_errors = body["errors"]
            .as_array()
            .map(|errors| errors.iter().filter(|e| e["stage"] == "runner").count())
            .unwrap_or_default();
        assert_eq!(runner_errors, usize::from(said), "{project}: {body}");
    }
}

/// T-6275: a runner that takes the stream and posts nothing back within the window is a runner
/// error, never an empty trace that reads as "the mapping made nothing and nothing failed".
#[tokio::test]
async fn a_runner_that_posts_nothing_back_is_a_runner_error() {
    let project = "t6275-a";
    let runner = runner_answering(ResponseTemplate::new(200), ResponseTemplate::new(200)).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror(project));
    let (status, body) = post(&state, "dev@hel.fi", project, &request("root = this")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["input"]["events"], 0, "{body}");
    assert_eq!(body["errors"][0]["stage"], "runner", "{body}");
    let message = body["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(message.contains("sent nothing back within 3 s"), "{body}");
}

// ---- T-2520: `run_harness`, one test stream per project, always removed (PL-43) -------------
// The running tests are one table per process, so every case below has a project of its own.

const STREAM: &str = r"^/[a-z0-9-]+/streams/pipeline-test-[a-z0-9-]+$";

/// A runner answering the create with `create` and the delete with `delete`, and a Portal on it.
async fn world_of(
    project: &str,
    create: ResponseTemplate,
    delete: ResponseTemplate,
) -> (MockServer, AppState) {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(STREAM))
        .respond_with(create)
        .mount(&runner)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(STREAM))
        .respond_with(delete)
        .mount(&runner)
        .await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror(project));
    (runner, state)
}

/// The runner's requests of `verb`, as paths.
async fn asked(runner: &MockServer, verb: &str) -> Vec<String> {
    common::stream_requests(runner)
        .await
        .iter()
        .filter(|r| r.method.as_str() == verb)
        .map(|r| r.url.path().to_owned())
        .collect()
}

/// A runner answer that says when its request arrived, before any delay it holds.
struct Arrival(Arc<tokio::sync::Notify>, ResponseTemplate);

impl wiremock::Respond for Arrival {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        self.0.notify_one();
        self.1.clone()
    }
}

/// T-2520, PL-43: one test at a time per project; the second is refused before it reaches the
/// runner, and the first finishes and removes its stream.
#[tokio::test]
async fn a_second_test_of_the_same_project_while_one_is_running_is_409() {
    // The runner holds its answer, so the first test is still running when the second arrives;
    // an instant answer let the first finish and free the slot first on a slow CI host (T-2673).
    // The second is sent the moment the runner has the first's create, which the first asks only
    // once it holds the project's slot. wiremock lists a request among the received ones only
    // after it answered, so waiting on that list let the first finish before the second was sent
    // on a busy CI host (T-3195); the responder below says so as the request arrives.
    let arrived = Arc::new(tokio::sync::Notify::new());
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(STREAM))
        .respond_with(Arrival(
            arrived.clone(),
            // Long enough for the second's refusal, which needs no network, and well under the
            // Portal's 2 s runner timeout, past which the first would be a 503.
            ResponseTemplate::new(200).set_delay(Duration::from_secs(1)),
        ))
        .mount(&runner)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(STREAM))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror("t2520-one"));
    let body = request("root = this");
    let first = {
        let (state, body) = (state.clone(), body.clone());
        tokio::spawn(async move { post(&state, "dev@hel.fi", "t2520-one", &body).await })
    };
    tokio::time::timeout(Duration::from_secs(20), arrived.notified())
        .await
        .expect("the first test never reached the runner");
    let (status, answer) = post(&state, "dev@hel.fi", "t2520-one", &body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(
        asked(&runner, "POST").await.len(),
        1,
        "the second reached the runner"
    );

    let (status, answer) = first.await.expect("the first test");
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(asked(&runner, "DELETE").await, asked(&runner, "POST").await);
}

// ---- T-4936: one runner stream name per project, so the runner's metrics stay bounded ---------
// Bento keeps every stream name's metric series after the stream is deleted: a name per test grew
// the dev runner by about 160 KB a test, past 2 GiB in three days.

/// Two tests of a project reuse one stream name: the runner holds no new series for the second.
#[tokio::test]
async fn every_test_of_a_project_runs_under_the_one_stream_name_of_the_project() {
    let (runner, state) = world_of(
        "t4936-name",
        ResponseTemplate::new(200),
        ResponseTemplate::new(200),
    )
    .await;
    let body = request("root = this");
    for _ in 0..2 {
        let (status, answer) = post(&state, "dev@hel.fi", "t4936-name", &body).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
    }
    let stream = "/t4936-name/streams/pipeline-test-t4936-name".to_owned();
    assert_eq!(
        asked(&runner, "POST").await,
        vec![stream.clone(), stream.clone()]
    );
    assert_eq!(asked(&runner, "DELETE").await, vec![stream.clone(), stream]);
}

/// The runner's answer to a create of a name it already runs (Bento 1.22.0), and its description
/// of that stream `uptime` seconds after its create.
async fn runner_holding(project: &str, uptime: f64) -> (MockServer, AppState) {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(STREAM))
        .respond_with(ResponseTemplate::new(400).set_body_string("Stream already exists"))
        .up_to_n_times(1)
        .mount(&runner)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(STREAM))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(STREAM))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "active": true, "uptime": uptime })),
        )
        .mount(&runner)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(STREAM))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror(project));
    (runner, state)
}

/// Another Portal replica's test of the project is running: 409, and its stream stays.
#[tokio::test]
async fn a_test_stream_another_replica_is_running_is_409_and_kept() {
    let (runner, state) = runner_holding("t4936-live", 2.0).await;
    let (status, answer) = post(&state, "dev@hel.fi", "t4936-live", &request("root = this")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(asked(&runner, "POST").await.len(), 1);
    assert!(
        asked(&runner, "DELETE").await.is_empty(),
        "a live test's stream was deleted"
    );
}

/// A stream left by a test whose delete failed is removed, and the new test runs.
#[tokio::test]
async fn a_left_over_test_stream_is_removed_and_the_test_runs() {
    let (runner, state) = runner_holding("t4936-old", 3600.0).await;
    let (status, answer) = post(&state, "dev@hel.fi", "t4936-old", &request("root = this")).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(asked(&runner, "POST").await.len(), 2);
    // The left-over's delete, then the new test's own.
    assert_eq!(asked(&runner, "DELETE").await.len(), 2);
}

/// T-2520, PL-43: another project's test runs beside it.
#[tokio::test]
async fn two_different_projects_test_concurrently_without_conflicting() {
    let (runner_a, state_a) = world_of(
        "t2520-a",
        ResponseTemplate::new(200),
        ResponseTemplate::new(200),
    )
    .await;
    let (runner_b, state_b) = world_of(
        "t2520-b",
        ResponseTemplate::new(200),
        ResponseTemplate::new(200),
    )
    .await;
    let body = request("root = this");
    let ((a, _), (b, _)) = tokio::join!(post(&state_a, "dev@hel.fi", "t2520-a", &body), async {
        // B starts once A's stream exists, so the two overlap.
        for _ in 0..100 {
            if !asked(&runner_a, "POST").await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        post(&state_b, "dev@hel.fi", "t2520-b", &body).await
    },);
    assert_eq!((a, b), (StatusCode::OK, StatusCode::OK));
    assert_eq!(asked(&runner_a, "DELETE").await.len(), 1);
    assert_eq!(asked(&runner_b, "DELETE").await.len(), 1);
}

/// T-2520, PL-43: a failed test gives its project's slot back, so the next one runs.
#[tokio::test]
async fn the_slot_is_released_after_a_failed_run_so_a_later_test_of_the_same_project_succeeds() {
    let (runner, state) = world_of(
        "t2520-again",
        ResponseTemplate::new(400).set_body_string("line 3: expected something"),
        ResponseTemplate::new(200),
    )
    .await;
    let body = request("root = this");
    for _ in 0..2 {
        let (status, answer) = post(&state, "dev@hel.fi", "t2520-again", &body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a refused harness is a lint trace: {answer}"
        );
    }
    runner.reset().await;
    common::take_budgets(&runner).await;
    Mock::given(method("POST"))
        .and(path_regex(STREAM))
        .respond_with(ResponseTemplate::new(503))
        .mount(&runner)
        .await;
    let (status, _) = post(&state, "dev@hel.fi", "t2520-again", &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    runner.reset().await;
    common::take_budgets(&runner).await;
    Mock::given(method("POST"))
        .and(path_regex(STREAM))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(STREAM))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    let (status, answer) = post(&state, "dev@hel.fi", "t2520-again", &body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

/// T-2520, PL-43: a runner that creates the stream and never posts a message still gets the
/// delete, once the test's deadline has passed.
#[tokio::test]
async fn a_runner_that_times_out_mid_test_still_gets_the_delete() {
    let (runner, state) = world_of(
        "t2520-silent",
        ResponseTemplate::new(200),
        ResponseTemplate::new(200),
    )
    .await;
    let started = std::time::Instant::now();
    let (status, answer) = post(
        &state,
        "dev@hel.fi",
        "t2520-silent",
        &request("root = this"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["input"]["events"], 0, "{answer}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let created = asked(&runner, "POST").await;
    assert_eq!(asked(&runner, "DELETE").await, created);
}

/// T-2520, PL-43: a project name reaches the runner's URL template only as a DNS label: one
/// with reserved characters is not a project, and no stream is created for it.
#[tokio::test]
async fn the_project_name_is_url_templated_safely_even_with_reserved_characters() {
    let (runner, state) = world_of(
        "t2520-safe",
        ResponseTemplate::new(200),
        ResponseTemplate::new(200),
    )
    .await;
    for project in ["a%2Fb", "a..b", "UPPER", "a%40evil.example", "a%3Fx%3D1"] {
        let (status, answer) = post(&state, "dev@hel.fi", project, &request("root = this")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{project}: {answer}");
    }
    assert!(runner
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

/// T-2520, T-4936: every test reaches the Portal under a fresh capture id, 26 characters of
/// base32 from the OS's randomness, while the stream keeps the project's one name.
#[tokio::test]
async fn the_capture_id_is_unique_per_call_never_reused() {
    let (runner, state) = world_of(
        "t2520-ids",
        ResponseTemplate::new(400).set_body_string("line 1: no"),
        ResponseTemplate::new(200),
    )
    .await;
    for _ in 0..5 {
        post(&state, "dev@hel.fi", "t2520-ids", &request("root = this")).await;
    }
    let capture = regex::Regex::new(r"/internal/pipeline-tests/([a-z2-7]{26})").expect("regex");
    let mut ids: Vec<String> = common::stream_requests(&runner)
        .await
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .filter_map(|r| {
            let body = String::from_utf8_lossy(&r.body).into_owned();
            capture.captures(&body).map(|c| c[1].to_owned())
        })
        .collect();
    assert_eq!(ids.len(), 5);
    assert_eq!(asked(&runner, "DELETE").await, asked(&runner, "POST").await);
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 5, "a capture id was used twice");
}

/// T-2520, PL-43: without a capture route there is nowhere for the harness to post, so the
/// test is `503` before any stream exists.
#[tokio::test]
async fn a_capture_route_not_configured_is_503_before_a_stream_is_created() {
    let (runner, _) = world_of(
        "t2520-nocap",
        ResponseTemplate::new(200),
        ResponseTemplate::new(200),
    )
    .await;
    let mut config = config(Some(&runner));
    config.pipeline_test_capture_url = None;
    let state = AppState::new(config, None).with_mirror(mirror("t2520-nocap"));
    let (status, answer) = post(&state, "dev@hel.fi", "t2520-nocap", &request("root = this")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
    assert!(runner
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

/// T-2520: what the person reads about a runner that failed names no runner address and no
/// credential, whichever way it failed.
#[tokio::test]
async fn the_runner_error_shown_to_the_person_carries_no_runner_url_or_credential() {
    for (project, create) in [
        (
            "t2520-quiet-a",
            ResponseTemplate::new(500)
                .set_body_string("panic at http://runner.internal:4195 token=abc"),
        ),
        (
            "t2520-quiet-b",
            ResponseTemplate::new(200).set_delay(Duration::from_secs(5)),
        ),
    ] {
        let (runner, state) = world_of(project, create, ResponseTemplate::new(200)).await;
        let (status, answer) = post(&state, "dev@hel.fi", project, &request("root = this")).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{project}: {answer}"
        );
        let text = answer.to_string();
        for leak in [
            runner.uri().as_str(),
            "127.0.0.1",
            "runner.internal",
            "token=",
            "streams/",
        ] {
            assert!(!text.contains(leak), "{project}: {leak} in {text}");
        }
    }
}

/// A project where `dev@hel.fi` proposes and `reader@hel.fi` only reads, holding one saved
/// NATS DataSource (T-4819).
fn with_saved_source(project: &str) -> Arc<Mirror> {
    let mirror = mirror(project);
    mirror.upsert(org(
        "Role",
        "source-reader",
        json!({ "rules": [{ "kinds": ["DataSource"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(org(
        "RoleBinding",
        "readers",
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "source-reader",
            "scope": { "project": project }
        }),
    ));
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "DataSource".to_owned(),
        metadata: ObjectMeta::new("demo-counters", project),
        spec: json!({
            "type": "nats",
            "input": {
                "urls": ["nats://demo:${DEMO_FEED_PASSWORD}@demo-nats.dev.svc.cluster.local:4222"],
                "subject": "helsinki.demo.counters"
            }
        }),
        status: None,
    });
    mirror
}

/// T-4819: Test connection runs the saved source's own input on the runner and answers its first
/// record; the `${VAR}` stays for the runner to fill and never comes back in the answer.
#[tokio::test]
async fn a_saved_source_is_tested_with_its_own_input_and_answers_its_first_record() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(with_saved_source("espoo"));
    let record = [json!({ "input": "{\"count\":7}", "error": null })];
    let empty = json!({});
    let (answer, harness) = tokio::join!(
        send(
            &state,
            "dev@hel.fi",
            "/api/v1/projects/espoo/datasources/demo-counters/test",
            &empty
        ),
        play_runner(&runner, &state, &record),
    );
    assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);
    assert_eq!(answer.1["records"], 1, "{}", answer.1);
    assert_eq!(answer.1["sample"]["count"], 7, "{}", answer.1);
    assert_eq!(
        harness["input"]["nats"]["subject"],
        "helsinki.demo.counters"
    );
    assert!(
        !answer.1.to_string().contains("DEMO_FEED_PASSWORD"),
        "{}",
        answer.1
    );
    runner.verify().await;
}

/// T-4819: a read that failed is said in plain words, not as the runner's error chain.
#[tokio::test]
async fn a_failed_read_is_said_in_plain_words() {
    let runner = common::runner().await;
    let (create, delete) = runner_mocks(ResponseTemplate::new(200));
    create.mount(&runner).await;
    delete.mount(&runner).await;
    let state = AppState::new(config(Some(&runner)), None).with_mirror(with_saved_source("hanko"));
    let failed = [
        json!({ "input": "", "error": "fetch: nats: dial tcp 10.0.0.9:4222: connect: connection refused" }),
    ];
    let empty = json!({});
    let (answer, _) = tokio::join!(
        send(
            &state,
            "dev@hel.fi",
            "/api/v1/projects/hanko/datasources/demo-counters/test",
            &empty
        ),
        play_runner(&runner, &state, &failed),
    );
    assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);
    assert_eq!(
        answer.1["skipped"], "the feed could not be reached: connection refused",
        "{}",
        answer.1
    );
}

/// PL-122 (T-7088): a webhook source's addresses need a session and a grant that reads the
/// DataSource; its test event needs one that proposes it. Anonymous is 401 on both, a project
/// member who reads only pipelines is 403 on the addresses, and a reader is 403 on the test event.
#[tokio::test]
async fn a_webhook_source_answers_only_a_session_with_the_grant_each_route_names() {
    let mirror = with_saved_source("vantaa");
    mirror.upsert(org(
        "Role",
        "pipeline-reader",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(org(
        "RoleBinding",
        "pipeline-readers",
        json!({
            "subjects": [{ "user": "pipes@hel.fi" }],
            "role": "pipeline-reader",
            "scope": { "project": "vantaa" }
        }),
    ));
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "DataSource".to_owned(),
        metadata: ObjectMeta::new("ttn", "vantaa"),
        spec: json!({
            "type": "webhook",
            "webhook": { "auth": { "bearer": { "tokenRef": { "name": "ttn", "key": "token" } } } }
        }),
        status: None,
    });
    let state = AppState::new(config(None), None).with_mirror(mirror);
    let info = "/api/v1/projects/vantaa/datasources/ttn/webhook";
    let event = "/api/v1/projects/vantaa/datasources/ttn/test-event";
    let anonymous = |method: &str, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
            .echo_csrf()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"pipeline":"p","body":"{}"}"#))
            .expect("request")
    };
    for (method, uri) in [("GET", info), ("POST", event)] {
        let response = server::app(state.clone())
            .oneshot(anonymous(method, uri))
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
    let read = |who: &str| {
        Request::builder()
            .uri(info)
            .header(header::COOKIE, cookies(&state.config, identity(who)))
            .body(Body::empty())
            .expect("request")
    };
    let pipes = server::app(state.clone())
        .oneshot(read("pipes@hel.fi"))
        .await
        .expect("response");
    assert_eq!(pipes.status(), StatusCode::FORBIDDEN);
    let reader = server::app(state.clone())
        .oneshot(read("reader@hel.fi"))
        .await
        .expect("response");
    assert_eq!(reader.status(), StatusCode::OK);
    let body = json!({ "pipeline": "p", "body": "{}" });
    let (status, answer) = send(&state, "reader@hel.fi", event, &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    let (status, answer) = send(&state, "stranger@hel.fi", event, &body).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
}

/// G-08, G-22 (T-7657): a discovery and a paging guess read the address once on the project's
/// runner, and a read that fails answers 200 with the reason in `skipped` (the refusals before
/// the read are `datasource_test`'s own tests).
#[tokio::test]
async fn a_discovery_and_a_paging_guess_read_once_on_the_runner_and_say_why_they_found_nothing() {
    let failed = [
        json!({ "input": "", "error": "fetch: dial tcp 1.1.1.1:443: connect: connection refused" }),
    ];
    for (project, route, body) in [
        (
            "loviisa",
            "datasource-discovery",
            json!({ "kind": "ckan", "url": "https://1.1.1.1/api/3/action/package_search" }),
        ),
        (
            "kotka",
            "datasource-paging",
            json!({ "url": "https://1.1.1.1/api/items" }),
        ),
    ] {
        let runner = common::runner().await;
        let (create, delete) = runner_mocks(ResponseTemplate::new(200));
        create.mount(&runner).await;
        delete.mount(&runner).await;
        let state =
            AppState::new(config(Some(&runner)), None).with_mirror(with_saved_source(project));
        let uri = format!("/api/v1/projects/{project}/{route}");
        let (answer, _) = tokio::join!(
            send(&state, "dev@hel.fi", &uri, &body),
            play_runner(&runner, &state, &failed),
        );
        assert_eq!(answer.0, StatusCode::OK, "{route}: {}", answer.1);
        assert!(
            answer.1["skipped"]
                .as_str()
                .is_some_and(|why| why.contains("connection refused")),
            "{route}: {}",
            answer.1
        );
    }
}

/// The runner's environment as a host hands it: one credential, under the variable a sync names.
struct OneCredential(String, String);

impl joinedcontext_portal::pipeline_secrets::RunnerSecrets for OneCredential {
    fn replace(
        &self,
        _values: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    ) {
    }

    fn credential(&self, project: &str, name: &str) -> Option<zeroize::Zeroizing<String>> {
        (project == "porvoo" && name == self.0).then(|| zeroize::Zeroizing::new(self.1.clone()))
    }
}

fn webhook_source(name: &str, auth: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "DataSource".to_owned(),
        metadata: ObjectMeta::new(name, "porvoo"),
        spec: json!({ "type": "webhook", "webhook": { "auth": auth } }),
        status: None,
    }
}

async fn get(state: &AppState, who: &str, uri: &str) -> (StatusCode, Value) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookies(&state.config, identity(who)))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// PL-122 (T-7657): the addresses name each reading pipeline under the published listener and the
/// header a sender sets; a name that is no source, a stranger and a spec that does not parse
/// answer 404, 404 and 400.
#[tokio::test]
async fn a_webhook_source_lists_each_reading_pipelines_address_and_its_header() {
    let mirror = with_saved_source("porvoo");
    mirror.upsert(webhook_source(
        "ttn",
        json!({ "bearer": { "tokenRef": { "name": "ttn", "key": "token" } } }),
    ));
    mirror.upsert(webhook_source(
        "gh",
        json!({ "hmac": { "header": "X-Hub-Signature-256", "secretRef": { "name": "gh", "key": "secret" } } }),
    ));
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "DataSource".to_owned(),
        metadata: ObjectMeta::new("broken", "porvoo"),
        spec: json!({ "type": "webhook", "webhook": "not a block" }),
        status: None,
    });
    let mut pipeline = request(".")["pipeline"].clone();
    pipeline["spec"]["source"]["dataSourceRef"]["name"] = json!("ttn");
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Pipeline".to_owned(),
        metadata: ObjectMeta::new("uplinks", "porvoo"),
        spec: pipeline["spec"].clone(),
        status: None,
    });
    let mut config = config(None);
    config.ingest_url = Some("https://ingest.example.org".into());
    let state = AppState::new(config, None).with_mirror(mirror);
    let base = "/api/v1/projects/porvoo/datasources";

    let (status, info) = get(&state, "reader@hel.fi", &format!("{base}/ttn/webhook")).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert_eq!(info["auth"], "bearer");
    assert_eq!(info["header"], "Authorization");
    assert_eq!(info["published"], true);
    assert_eq!(
        info["addresses"],
        json!([{ "pipeline": "uplinks", "url": "https://ingest.example.org/porvoo.uplinks/ttn" }])
    );
    let (status, info) = get(&state, "reader@hel.fi", &format!("{base}/gh/webhook")).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert_eq!(info["auth"], "hmac");
    assert_eq!(info["header"], "X-Hub-Signature-256");
    assert_eq!(info["addresses"], json!([]));

    for (who, name, expected) in [
        ("reader@hel.fi", "Not_A_Name", StatusCode::NOT_FOUND),
        ("reader@hel.fi", "nope", StatusCode::NOT_FOUND),
        ("stranger@hel.fi", "ttn", StatusCode::NOT_FOUND),
        ("reader@hel.fi", "broken", StatusCode::BAD_REQUEST),
        (
            "reader@hel.fi",
            "demo-counters",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (status, body) = get(&state, who, &format!("{base}/{name}/webhook")).await;
        assert_eq!(status, expected, "{who} {name}: {body}");
    }
}

/// PL-122 (T-7657): a test event goes to the project's runner (its `{project}` filled in) under
/// the reading pipeline's stream, carries the source's credential, and answers the runner's
/// status with the first 2048 characters of its body. Before that: a pipeline that reads no such
/// source and an empty or oversized body are 422 naming the field, a Portal with no runner
/// beside it or no credential yet is 503, and a runner that does not answer is 503.
///
/// The host's runner is process-wide, so this one test walks every step in order.
#[tokio::test]
async fn a_webhook_test_event_reaches_the_projects_runner_signed_with_the_sources_credential() {
    let runner = MockServer::start().await;
    let mirror = mirror("porvoo");
    mirror.upsert(webhook_source(
        "ttn",
        json!({ "bearer": { "tokenRef": { "name": "ttn", "key": "token" } } }),
    ));
    let mut pipeline = request(".")["pipeline"].clone();
    pipeline["spec"]["source"]["dataSourceRef"]["name"] = json!("ttn");
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Pipeline".to_owned(),
        metadata: ObjectMeta::new("uplinks", "porvoo"),
        spec: pipeline["spec"].clone(),
        status: None,
    });
    let state = AppState::new(config(Some(&runner)), None).with_mirror(mirror.clone());
    let uri = "/api/v1/projects/porvoo/datasources/ttn/test-event";
    let event = |pipeline: &str, body: String| json!({ "pipeline": pipeline, "body": body });

    for (body, field) in [
        (event("other", "{}".into()), "pipeline"),
        (event("Not_A_Name", "{}".into()), "pipeline"),
        (event("uplinks", "  ".into()), "body"),
        (event("uplinks", "x".repeat(64 * 1024 + 1)), "body"),
    ] {
        let (status, answer) = send(&state, "dev@hel.fi", uri, &body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
        assert_eq!(answer["errors"][0]["path"], field, "{answer}");
    }

    let unconfigured = AppState::new(config(None), None).with_mirror(mirror.clone());
    let (status, answer) = send(
        &unconfigured,
        "dev@hel.fi",
        uri,
        &event("uplinks", "{}".into()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");

    let reference: jc_core::envelope::SecretRef =
        serde_json::from_value(json!({ "name": "ttn", "key": "token" })).expect("a reference");
    let variable = jc_core::kinds::data_source::env_var_of("ttn", &reference);
    joinedcontext_portal::pipeline_secrets::hand_runner_secrets_to(Arc::new(OneCredential(
        "ANOTHER_VARIABLE".into(),
        "tok-1".into(),
    )));
    let (status, answer) = send(&state, "dev@hel.fi", uri, &event("uplinks", "{}".into())).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("secret 'ttn'"),
        "{answer}"
    );

    joinedcontext_portal::pipeline_secrets::hand_runner_secrets_to(Arc::new(OneCredential(
        variable,
        "tok-1".into(),
    )));
    // A source written into the repository by hand with no credential cannot be signed.
    let mut bare = webhook_source("bare", json!({}));
    bare.metadata = ObjectMeta::new("ttn", "porvoo");
    let unsigned = Arc::new(Mirror::new());
    for envelope in mirror.matching(|env| env.metadata.name != "ttn" || env.kind != "DataSource") {
        unsigned.upsert(envelope);
    }
    unsigned.upsert(bare);
    let unsigned = AppState::new(config(Some(&runner)), None).with_mirror(unsigned);
    let (status, answer) = send(&unsigned, "dev@hel.fi", uri, &event("uplinks", "{}".into())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert_eq!(answer["errors"][0]["path"], "spec.webhook.auth", "{answer}");

    Mock::given(method("POST"))
        .and(wiremock::matchers::path(
            "/porvoo/ingest/default/porvoo.uplinks/ttn",
        ))
        .and(wiremock::matchers::header("authorization", "Bearer tok-1"))
        .and(wiremock::matchers::body_string(r#"{"temperature": 21.5}"#))
        .respond_with(ResponseTemplate::new(202).set_body_string("é".repeat(3000)))
        .expect(1)
        .mount(&runner)
        .await;
    let (status, answer) = send(
        &state,
        "dev@hel.fi",
        uri,
        &event("uplinks", r#"{"temperature": 21.5}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], 202);
    assert_eq!(answer["answer"], "é".repeat(2048));
    // A short answer comes back whole (T-7894).
    Mock::given(method("POST"))
        .and(wiremock::matchers::path(
            "/porvoo/ingest/default/porvoo.uplinks/ttn",
        ))
        .and(wiremock::matchers::body_string(r#"{"short": 1}"#))
        .respond_with(ResponseTemplate::new(200).set_body_string("taken"))
        .expect(1)
        .mount(&runner)
        .await;
    let (status, answer) = send(
        &state,
        "dev@hel.fi",
        uri,
        &event("uplinks", r#"{"short": 1}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        (answer["status"].as_u64(), answer["answer"].as_str()),
        (Some(200), Some("taken"))
    );

    // A bound port nobody listens on: the runner is down.
    let closed = tokio::net::TcpSocket::new_v4().expect("a socket");
    closed.bind("127.0.0.1:0".parse().unwrap()).expect("bound");
    let mut down = config(None);
    down.pipeline_runner_url = Some(format!("http://{}/", closed.local_addr().unwrap()));
    let down = AppState::new(down, None).with_mirror(mirror);
    let (status, answer) = send(&down, "dev@hel.fi", uri, &event("uplinks", "{}".into())).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("did not answer"),
        "{answer}"
    );
    runner.verify().await;
}

/// T-4819: anonymous is 401, a reader is 403, an unknown source is 404, and none reaches the runner.
#[tokio::test]
async fn testing_a_source_needs_a_session_and_a_grant_that_proposes_it() {
    let state = AppState::new(config(None), None).with_mirror(with_saved_source("turku"));
    let uri = "/api/v1/projects/turku/datasources/demo-counters/test";
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
                .echo_csrf()
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (status, body) = send(&state, "reader@hel.fi", uri, &json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = send(&state, "stranger@hel.fi", uri, &json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = send(
        &state,
        "dev@hel.fi",
        "/api/v1/projects/turku/datasources/nope/test",
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}
