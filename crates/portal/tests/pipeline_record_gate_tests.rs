//! A Pipeline proposes only on a green record check of the spec it proposes (T-4840, PF-57): the
//! test on the runner judges the records its mapping makes where their write would be judged and
//! keeps the verdict; a dry run alone, a red check or a spec changed since is a 409 that says
//! which, through the REST door the editor proposes by. Records a caller types into the
//! validation step are judged and kept nowhere.
mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{envelope, person, send};
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "gate";

/// `dev` proposes pipelines, `jana` only reads them and the space's entities; the `bikes` space
/// names its model and the Endpoint `bikes-write` writes into it.
fn state(runner: Option<&MockServer>) -> AppState {
    let mirror = Arc::new(Mirror::new());
    for (role, verbs, who) in [
        ("author", json!(["propose", "read"]), "dev"),
        ("reader", json!(["read"]), "jana"),
    ] {
        mirror.upsert(envelope(
            "Role",
            &format!("pipeline-{role}"),
            ORG_NAMESPACE,
            // Entity read: the proposer reads the space the pipeline writes (PL-94).
            json!({ "rules": [{ "kinds": ["Pipeline", "DataSource", "Entity"], "verbs": verbs }] }),
        ));
        mirror.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{role}"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{who}@hel.fi") }],
                "role": format!("pipeline-{role}"),
                "scope": { "project": PROJECT },
            }),
        ));
    }
    mirror.upsert(envelope(
        "ContextSpace",
        "bikes",
        PROJECT,
        json!({ "urnSegment": "bikes", "dataModelRef": { "kind": "DataModel", "name": "bikes" } }),
    ));
    mirror.upsert(envelope(
        "Endpoint",
        "bikes-write",
        PROJECT,
        json!({ "contextSpaceRef": "bikes", "slug": "k7r2m4xqzvbn3tdw6hcy5pajfe", "audience": "organization" }),
    ));
    mirror.upsert(envelope(
        "DataSource",
        "stations-csv",
        PROJECT,
        json!({ "type": "http", "http": { "url": "https://feeds.example/stations.csv" } }),
    ));
    let mut config = Config::for_tests();
    config.org_domain = Some("hel.fi".into());
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
    config.pipeline_runner_url = runner.map(|r| format!("{}/{{project}}", r.uri()));
    config.pipeline_test_capture_url = Some("http://portal-internal:9090".into());
    config.pipeline_runner_client_id = Some(common::PIPELINE_RUNNER_CLIENT.into());
    let state = AppState::new(config, None).with_mirror(mirror);
    state.model_schemas.replace(HashMap::from([(
        (PROJECT.to_owned(), "bikes".to_owned()),
        Arc::new(ModelSchema::compile(
            "bikes",
            "1.2.0",
            &json!({ "definitions": { "BikeStation": {
                "properties": { "capacity": { "type": "integer" } }
            }}}),
            &["BikeStation".to_owned()],
            false,
        )),
    )]));
    state
}

fn pipeline(class: &str) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha2",
        "kind": "Pipeline",
        "metadata": { "name": "stations", "namespace": PROJECT },
        "spec": {
            "class": class,
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "stations-csv" } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-write" }]
        }
    })
}

fn station(capacity: Value) -> Value {
    json!({ "id": "urn:ngsi-ld:BikeStation:01", "type": "BikeStation",
            "capacity": { "type": "Property", "value": capacity } })
}

async fn dry_run(state: &AppState, manifest: &Value) {
    let answer = send(
        state,
        person("dev"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/pipelines?dryRun=All"),
        Some(manifest.clone()),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("JSON");
    assert_eq!(body["verdict"]["ok"], true, "{}", answer.text);
}

async fn validate(state: &AppState, who: &str, manifest: &Value, records: Value) -> Value {
    let answer = send(
        state,
        person(who),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/ops/jc_pipeline_validate"),
        Some(json!({ "pipeline": manifest, "records": records })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("JSON")
}

/// A harness run of the pipeline on the runner through `op`, whose mapping makes `records`.
async fn test_run(
    op: &str,
    state: &AppState,
    runner: &MockServer,
    manifest: &Value,
    records: &[Value],
) -> Value {
    runner.reset().await;
    common::take_budgets(runner).await;
    for verb in ["POST", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(r"^/[a-z-]+/streams/pipeline-test-[a-z0-9-]+$"))
            .respond_with(ResponseTemplate::new(200))
            .mount(runner)
            .await;
    }
    let messages: Vec<Value> = records
        .iter()
        .map(|record| json!({ "input": "{\"capacity\":\"x\"}", "output": record, "error": null }))
        .collect();
    let uri = format!("/api/v1/projects/{PROJECT}/ops/{op}");
    let (answer, ()) = tokio::join!(
        send(
            state,
            person("dev"),
            "POST",
            &uri,
            Some(json!({
                "pipeline": manifest,
                "sample": { "text": "capacity\nx\n", "format": "csv" }
            })),
        ),
        play_runner(runner, state, &messages),
    );
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("JSON")
}

/// The runner's side: waits for the harness, posts each message to its capture route.
async fn play_runner(runner: &MockServer, state: &AppState, messages: &[Value]) {
    let bearer = format!(
        "Bearer {}",
        common::REALM.workload(common::PIPELINE_RUNNER_CLIENT)
    );
    for _ in 0..100 {
        let received = common::stream_requests(runner).await;
        if let Some(create) = received.iter().find(|r| r.method.as_str() == "POST") {
            let harness: Value = serde_json::from_slice(&create.body).expect("a JSON harness");
            let capture = harness["output"]["http_client"]["url"]
                .as_str()
                .expect("the capture url");
            let path = &capture[capture.find("/internal/").expect("internal path")..];
            for message in messages {
                let status = joinedcontext_portal::server::internal_app(state.clone())
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(path)
                            .header(axum::http::header::AUTHORIZATION, &bearer)
                            .body(Body::from(message.to_string()))
                            .expect("request"),
                    )
                    .await
                    .expect("response")
                    .status();
                assert_eq!(status, StatusCode::NO_CONTENT);
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("the runner never received the harness");
}

/// The proposal's status and the gate's reason, when it refused.
async fn propose(state: &AppState, manifest: &Value) -> (StatusCode, Option<String>) {
    let answer = send(
        state,
        person("dev"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/pipelines"),
        Some(manifest.clone()),
    )
    .await;
    let reason = serde_json::from_str::<Value>(&answer.text)
        .ok()
        .and_then(|body| body["reason"].as_str().map(str::to_owned));
    (answer.status, reason)
}

#[tokio::test]
async fn a_pipeline_proposes_only_on_a_green_record_check_of_its_spec() {
    let runner = common::runner().await;
    let state = state(Some(&runner));
    let manifest = pipeline("resident");

    // A dry run alone checks the manifest, never a record of it.
    dry_run(&state, &manifest).await;
    assert_eq!(
        propose(&state, &manifest).await,
        (StatusCode::CONFLICT, Some("records_unchecked".into()))
    );

    // A record the mapping makes that the model refuses: the test is red, and so is the proposal.
    let red = test_run(
        "jc_pipeline_test",
        &state,
        &runner,
        &manifest,
        &[station(json!("many"))],
    )
    .await;
    let finding = &red["check"]["records"][0]["findings"][0];
    assert_eq!(
        (finding["source"].as_str(), finding["field"].as_str()),
        (Some("model"), Some("capacity"))
    );
    dry_run(&state, &manifest).await;
    assert_eq!(
        propose(&state, &manifest).await,
        (StatusCode::CONFLICT, Some("records_failed".into()))
    );

    // Good records typed into the validation step are judged, and change no verdict.
    let typed = validate(&state, "dev", &manifest, json!([station(json!(12))])).await;
    assert_eq!(typed["verdicts"][0]["ok"], true);
    assert_eq!(
        propose(&state, &manifest).await,
        (StatusCode::CONFLICT, Some("records_failed".into())),
        "typed records are no record check of the mapping"
    );

    // The workbench's run of the mapping, green, beside the green dry run opens the gate: the
    // proposal reaches the forge, which this Portal has none of.
    let green = test_run(
        "jc_pipeline_try_mapping",
        &state,
        &runner,
        &manifest,
        &[station(json!(12))],
    )
    .await;
    assert_eq!(green["records"][0]["type"], "BikeStation");
    assert_eq!(
        propose(&state, &manifest).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );

    // The spec changed since its records were checked: stale, though its dry run is green.
    let changed = pipeline("auto");
    dry_run(&state, &changed).await;
    assert_eq!(
        propose(&state, &changed).await,
        (StatusCode::CONFLICT, Some("records_stale".into()))
    );

    // A green run of the mapping is no check of the Pipeline itself: before its dry run the
    // proposal is refused, after it the gate opens.
    let mut other = pipeline("resident");
    other["metadata"]["name"] = json!("stations-2");
    test_run(
        "jc_pipeline_try_mapping",
        &state,
        &runner,
        &other,
        &[station(json!(12))],
    )
    .await;
    assert_eq!(
        propose(&state, &other).await,
        (StatusCode::CONFLICT, Some("verdict_failed".into()))
    );
    dry_run(&state, &other).await;
    assert_eq!(
        propose(&state, &other).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn a_readers_check_opens_no_gate_and_the_route_wants_a_person() {
    let state = state(None);
    let manifest = pipeline("resident");
    let checked = validate(&state, "jana", &manifest, json!([station(json!(12))])).await;
    assert_eq!(checked["verdicts"][0]["ok"], true);
    dry_run(&state, &manifest).await;
    assert_eq!(
        propose(&state, &manifest).await,
        (StatusCode::CONFLICT, Some("records_unchecked".into())),
        "a reader's check is no verdict for the proposer"
    );

    let anonymous = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/{PROJECT}/ops/jc_pipeline_validate"
                ))
                .header("content-type", "application/json")
                .header(CSRF_HEADER, common::CSRF)
                .header("cookie", format!("{CSRF_COOKIE}={}", common::CSRF))
                .body(Body::from(
                    json!({ "pipeline": manifest, "records": [station(json!(12))] }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let stranger = send(
        &state,
        person("mikko"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/ops/jc_pipeline_validate"),
        Some(json!({ "pipeline": manifest, "records": [station(json!(12))] })),
    )
    .await;
    assert_eq!(stranger.status, StatusCode::FORBIDDEN, "{}", stranger.text);
}
