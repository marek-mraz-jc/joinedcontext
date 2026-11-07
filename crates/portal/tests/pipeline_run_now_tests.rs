//! A steward runs a queued pipeline once now (T-4349, PL-78): the run joins the queue, is named by
//! the time it was asked for, reaches the runner with that name and is the run the run list
//! shows once it wrote. A run in progress and a pipeline that always runs answer 409, a reader
//! 403, a pipeline nobody may see 404.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_log::{NewLine, Outcome};
use joinedcontext_portal::reconciler::streams::{Bentos, StreamDeployer};
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::state::AppState;

const HARVEST: &str = "/api/v1/projects/helsinki/pipelines/harvest/runs";

/// `jana` holds propose and approve on Pipeline, `eva` only reads. `harvest` polls on a nightly
/// schedule and runs from the queue; `stations` holds a socket open and always runs.
fn seed(state: &AppState) {
    for (role, verbs, user) in [
        (
            "pipeline-steward",
            json!(["read", "propose", "approve"]),
            "jana",
        ),
        ("pipeline-reader", json!(["read"]), "eva"),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": verbs }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{user}-{role}"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{user}@hel.fi") }],
                "role": role,
                "scope": { "project": "helsinki" },
            }),
        ));
    }
    state.mirror.upsert(envelope(
        "Endpoint",
        "helsinki-all",
        "helsinki",
        json!({
            "contextSpaceRef": "helsinki",
            "slug": "abc123456789012345678901234",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }),
    ));
    for (name, class, extra, source) in [
        (
            "harvest",
            "scheduled",
            json!({ "schedule": "0 5 * * *" }),
            json!({ "type": "http", "http": { "url": "https://example.org/data.json" } }),
        ),
        (
            "stations",
            "resident",
            json!({}),
            json!({ "type": "websocket", "webSocket": { "url": "wss://example.org/live" } }),
        ),
    ] {
        state
            .mirror
            .upsert(envelope("DataSource", name, "helsinki", source));
        let mut spec = json!({
            "class": class,
            "source": { "dataSourceRef": { "kind": "DataSource", "name": name } },
            "compute": { "kind": "bloblang", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
        });
        spec.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        state
            .mirror
            .upsert(envelope("Pipeline", name, "helsinki", spec));
    }
}

/// The Portal with a runner that takes every stream and reports each one running.
async fn world() -> (AppState, Arc<StreamDeployer>, MockServer) {
    let runner = common::runner().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    Mock::given(method("GET"))
        .and(path("/streams/helsinki.harvest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
        .mount(&runner)
        .await;
    let state = common::state_on_forge().await;
    seed(&state);
    let deployer = Arc::new(StreamDeployer::new(runner.uri()));
    let syncer = Syncer::new(
        state.gitea.clone().expect("the forge"),
        Arc::clone(&state.mirror),
    )
    .with_streams(Arc::clone(&deployer));
    let state = state.with_syncer(Arc::new(syncer));
    deployer
        .converge(&state.mirror, &Bentos::new(), &Default::default())
        .await;
    (state, deployer, runner)
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).expect("json")
}

#[tokio::test]
async fn a_steward_runs_a_queued_pipeline_now_and_finds_it_in_the_run_list() {
    let (state, deployer, runner) = world().await;

    let started = send(&state, person("jana"), "POST", HARVEST, None).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text);
    let run = json_of(&started.text)["run"]
        .as_str()
        .expect("the run's id")
        .to_owned();
    assert!(run.ends_with('Z') && run.len() == 20, "{run}");
    // Asked again before it started: the same queued run.
    let again = send(&state, person("jana"), "POST", HARVEST, None).await;
    assert_eq!(again.status, StatusCode::ACCEPTED, "{}", again.text);
    assert_eq!(json_of(&again.text)["run"], run.as_str());

    // The queue's next steps: the leftover is swept, then the run goes to the runner by its name.
    let now = chrono::Utc::now();
    deployer.tick_queue(now).await;
    deployer.tick_queue(now).await;
    let put = runner
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .find(|request| {
            request.method.as_str() == "PUT" && request.url.path() == "/streams/helsinki.harvest"
        })
        .expect("the run was loaded into the runner");
    let body = String::from_utf8_lossy(&put.body).to_string();
    assert!(
        body.contains(&format!(r#"meta jc_run = \"{run}\""#)),
        "{body}"
    );
    assert!(body.contains(r#""count":1"#), "{body}");

    // In the runner now: another run waits until it ends.
    let busy = send(&state, person("jana"), "POST", HARVEST, None).await;
    assert_eq!(busy.status, StatusCode::CONFLICT, "{}", busy.text);
    assert!(busy.text.contains(&run), "{}", busy.text);

    // What the run wrote is filed under its name, which the run list shows.
    state
        .pipeline_log
        .append(
            "helsinki",
            "harvest",
            &run,
            &[NewLine {
                record_id: "urn:ngsi-ld:Dataset:1".into(),
                step: None,
                outcome: Outcome::Sent,
                message: String::new(),
            }],
        )
        .await
        .expect("kept");
    let runs = send(&state, person("eva"), "GET", HARVEST, None).await;
    assert_eq!(runs.status, StatusCode::OK, "{}", runs.text);
    assert_eq!(json_of(&runs.text)["items"][0]["run"], run.as_str());

    let page = state
        .activity
        .list(
            "helsinki",
            &ActivityFilter {
                kinds: vec!["pipeline.run".into()],
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .expect("the feed");
    // One event per accepted request: who asked, and which run it named.
    assert_eq!(page.items.len(), 2, "{:?}", page.items);
    for event in &page.items {
        assert_eq!(event.details["run"], run.as_str());
        assert_eq!(event.details["object"], "pipelines/harvest");
        assert_eq!(event.details["actor"], "jana");
    }
}

#[tokio::test]
async fn a_pipeline_that_always_runs_a_reader_and_an_unknown_name_are_refused() {
    let (state, _, _) = world().await;

    let resident = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/pipelines/stations/runs",
        None,
    )
    .await;
    assert_eq!(resident.status, StatusCode::CONFLICT, "{}", resident.text);
    assert!(resident.text.contains("always runs"), "{}", resident.text);

    let reader = send(&state, person("eva"), "POST", HARVEST, None).await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);

    let unknown = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/pipelines/nope/runs",
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
}
