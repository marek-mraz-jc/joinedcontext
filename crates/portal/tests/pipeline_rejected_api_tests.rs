//! A pipeline's rejected list is read with read on the pipeline and by nobody else (PL-61,
//! ADR-N-034, T-2708): a caller without it is told the pipeline is not there, and a page is the
//! newest records first, with the rule each broke and its secrets masked. A retry needs propose
//! on the pipeline, replays only what carries no mask and hands it to the runner through the
//! space's validation stage and the pipeline's own write. Its runs and each run's log are read
//! the same way as the list (PL-62).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_outcomes::Reason;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::state::AppState;

const REJECTED: &str = "/api/v1/projects/helsinki/pipelines/stations/rejected";
const RUNS: &str = "/api/v1/projects/helsinki/pipelines/stations/runs";
const RETRY: &str = "/api/v1/projects/helsinki/pipelines/stations/rejected/retry";

async fn world() -> AppState {
    let state = common::state_on_forge().await;
    seed(&state).await;
    state
}

async fn seed(state: &AppState) {
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-pipeline-reader",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "pipeline-reader",
            "scope": { "project": "helsinki" },
        }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "stations",
        "helsinki",
        json!({ "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes" }),
    ));
    for n in 0..3 {
        state
            .rejected
            .reject(
                "helsinki",
                "stations",
                &json!({ "n": n, "password": "hunter2hunter2" }),
                &Reason {
                    rule: "sh:datatype".into(),
                    path: "capacity".into(),
                    message: "capacity is not of the slot's datatype".into(),
                    step: None,
                    run: Some("2026-10-07T08:00:00Z".into()),
                },
            )
            .await
            .expect("kept");
    }
}

#[tokio::test]
async fn a_reader_of_the_pipeline_pages_through_its_rejected_records() {
    let state = world().await;
    let page = send(
        &state,
        person("jana"),
        "GET",
        &format!("{REJECTED}?limit=2"),
        None,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    let page: Value = serde_json::from_str(&page.text).expect("json");
    assert_eq!(page["total"], 3);
    assert_eq!(page["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(page["items"][0]["record"]["n"], 2, "newest first");
    assert_eq!(page["items"][0]["rule"], "sh:datatype");
    assert_eq!(page["items"][0]["path"], "capacity");
    // T-3252: each record names the run whose log holds it.
    assert_eq!(page["items"][0]["run"], "2026-10-07T08:00:00Z");
    assert!(!page.to_string().contains("hunter2"), "masked: {page}");

    let next = page["next"].as_i64().expect("a next page");
    let rest = send(
        &state,
        person("jana"),
        "GET",
        &format!("{REJECTED}?limit=2&before={next}"),
        None,
    )
    .await;
    let rest: Value = serde_json::from_str(&rest.text).expect("json");
    assert_eq!(rest["items"].as_array().map(Vec::len), Some(1));
    assert!(rest.get("next").is_none(), "{rest}");
}

#[tokio::test]
async fn a_caller_without_read_on_the_pipeline_is_told_it_is_not_there() {
    let state = world().await;
    let refused = send(&state, person("mikko"), "GET", REJECTED, None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    assert!(!refused.text.contains("sh:datatype"));

    let missing = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/helsinki/pipelines/nobody/rejected",
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

/// A state whose pipeline writes through an Endpoint into a space with a compiled model, and a
/// runner at `runner`; `jana` reads pipelines, `eeva` also proposes them.
async fn retry_world(runner: &MockServer) -> AppState {
    retry_world_as(runner, false).await
}

/// The same, with every stream on its own pipeline's identity (PL-19) when `pipeline_identity`.
async fn retry_world_as(runner: &MockServer, pipeline_identity: bool) -> AppState {
    let mut config = Config::for_tests();
    if pipeline_identity {
        config.pipeline_identity = true;
        config.pipeline_namespace = Some("dev-pipeline-identities".into());
    }
    config.pipeline_runner_url = Some(format!("{}/{{project}}", runner.uri()));
    config.pipeline_test_capture_url = Some("http://portal-internal:9090".into());
    let state = AppState::new(config, None);
    seed(&state).await;
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-author",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "eeva-pipeline-author",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "eeva@hel.fi" }],
            "role": "pipeline-author",
            "scope": { "project": "helsinki" },
        }),
    ));
    state.mirror.upsert(envelope(
        "ContextSpace",
        "helsinki",
        "helsinki",
        json!({ "urnSegment": "helsinki", "dataModelRef": { "kind": "DataModel", "name": "bikes" } }),
    ));
    state.mirror.upsert(envelope(
        "Endpoint",
        "bikes",
        "helsinki",
        json!({ "contextSpaceRef": "helsinki", "slug": "k7r2m4xq9vbn3tdw6hcy5pajfe" }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "stations",
        "helsinki",
        json!({
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes"
        }),
    ));
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
    // One record the author fixed at the source: nothing in it was masked.
    state
        .rejected
        .reject(
            "helsinki",
            "stations",
            &json!({ "id": "urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-9", "type": "BikeStation" }),
            &Reason {
                rule: "type".into(),
                path: String::new(),
                message: "BikeStation was not a class of the model".into(),
                step: None,
                run: None,
            },
        )
        .await
        .expect("kept");
    state
}

async fn ids(state: &AppState) -> Vec<i64> {
    state
        .rejected
        .list("helsinki", "stations", 10, None)
        .await
        .expect("the list")
        .iter()
        .map(|record| record.id)
        .collect()
}

#[tokio::test]
async fn a_retry_replays_the_unmasked_records_through_the_stage_and_keeps_the_masked() {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/helsinki/streams/rejected-replay-[a-z0-9]+$"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&runner)
        .await;
    let state = retry_world(&runner).await;
    let all = ids(&state).await;
    let (clean, masked) = (all[0], all[1]);

    let answer = send(
        &state,
        person("eeva"),
        "POST",
        RETRY,
        Some(json!({ "ids": [clean, masked] })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let answer: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(answer, json!({ "replayed": 1, "masked": [masked] }));

    let left = ids(&state).await;
    assert!(!left.contains(&clean), "the replayed record left the list");
    assert!(left.contains(&masked), "the masked record stays");

    let sent = common::stream_requests(&runner).await;
    let stream: Value = serde_json::from_slice(&sent[0].body).expect("a stream config");
    let text = stream.to_string();
    assert!(text.contains("json_schema"), "the stage is in: {text}");
    assert!(!text.contains("[MASKED]"), "no mask is replayed");
    assert!(
        text.contains("k7r2m4xq9vbn3tdw6hcy5pajfe"),
        "the pipeline's own write"
    );
}

/// T-4044: with pipeline identity the replay presents the pipeline's own token from the
/// runner's token service, so the stream names no client secret for the runner's API to serve.
#[tokio::test]
async fn with_pipeline_identity_a_replay_names_no_client_secret() {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/helsinki/streams/rejected-replay-[a-z0-9]+$"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&runner)
        .await;
    let state = retry_world_as(&runner, true).await;
    let clean = ids(&state).await[0];

    let answer = send(
        &state,
        person("eeva"),
        "POST",
        RETRY,
        Some(json!({ "ids": [clean] })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);

    let sent = common::stream_requests(&runner).await;
    let text = String::from_utf8_lossy(&sent[0].body).into_owned();
    assert!(
        !text.contains("client_secret") && !text.contains("JC_CLIENT_SECRET"),
        "{text}"
    );
    assert!(
        text.contains("\"helsinki/stations\""),
        "the pipeline's own client: {text}"
    );
    assert!(text.contains("${JC_PIPELINE_TOKEN_URL}"), "{text}");
}

#[tokio::test]
async fn a_reader_may_not_retry_and_a_runner_that_refuses_puts_the_records_back() {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&runner)
        .await;
    let state = retry_world(&runner).await;
    let clean = ids(&state).await[0];

    let reader = send(
        &state,
        person("jana"),
        "POST",
        RETRY,
        Some(json!({ "ids": [clean] })),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    let stranger = send(
        &state,
        person("mikko"),
        "POST",
        RETRY,
        Some(json!({ "ids": [clean] })),
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
    for ids in [json!([]), json!((0..101).collect::<Vec<i64>>())] {
        let bad = send(
            &state,
            person("eeva"),
            "POST",
            RETRY,
            Some(json!({ "ids": ids })),
        )
        .await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.text);
    }
    assert!(runner
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());

    let refused = send(
        &state,
        person("eeva"),
        "POST",
        RETRY,
        Some(json!({ "ids": [clean] })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        refused.text
    );
    let back = state
        .rejected
        .list("helsinki", "stations", 1, None)
        .await
        .expect("the list");
    assert_eq!(
        back[0].record["id"],
        "urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-9"
    );
    assert_eq!(back[0].rule, "runner");
    assert_eq!(
        state
            .rejected
            .count("helsinki", "stations")
            .await
            .expect("count"),
        4
    );
}

#[tokio::test]
async fn a_reader_lists_the_runs_and_reads_one_runs_log_of_this_pipeline_only() {
    use joinedcontext_portal::pipeline_log::{NewLine, Outcome};
    let state = world().await;
    state.mirror.upsert(envelope(
        "Pipeline",
        "other",
        "helsinki",
        json!({ "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes" }),
    ));
    let line = |id: &str, outcome| NewLine {
        record_id: id.into(),
        step: None,
        outcome,
        message: String::new(),
    };
    let run = "2026-09-25T08:00:00Z";
    let lines: Vec<NewLine> = (0..3)
        .map(|n| line(&format!("urn:s-{n}"), Outcome::Sent))
        .chain([line("urn:s-9", Outcome::Rejected)])
        .collect();
    state
        .pipeline_log
        .append("helsinki", "stations", run, &lines)
        .await
        .expect("kept");
    state
        .pipeline_log
        .append(
            "helsinki",
            "other",
            run,
            &[line("urn:other", Outcome::Sent)],
        )
        .await
        .expect("kept");

    let runs = send(&state, person("jana"), "GET", RUNS, None).await;
    assert_eq!(runs.status, StatusCode::OK, "{}", runs.text);
    let runs: Value = serde_json::from_str(&runs.text).expect("json");
    assert_eq!(runs["items"][0]["run"], run);
    assert_eq!(runs["items"][0]["sent"], 3);
    assert_eq!(runs["items"][0]["rejected"], 1);

    // The list page asks for the latest run only (T-4817).
    let older = "2026-09-25T07:00:00Z";
    state
        .pipeline_log
        .append(
            "helsinki",
            "stations",
            older,
            &[line("urn:s-old", Outcome::Sent)],
        )
        .await
        .expect("kept");
    let latest = send(
        &state,
        person("jana"),
        "GET",
        &format!("{RUNS}?limit=1"),
        None,
    )
    .await;
    assert_eq!(latest.status, StatusCode::OK, "{}", latest.text);
    let latest: Value = serde_json::from_str(&latest.text).expect("json");
    assert_eq!(
        latest["items"].as_array().map(Vec::len),
        Some(1),
        "{latest}"
    );
    let all = send(&state, person("jana"), "GET", RUNS, None).await;
    let all: Value = serde_json::from_str(&all.text).expect("json");
    assert_eq!(all["items"].as_array().map(Vec::len), Some(2), "{all}");

    let page = send(
        &state,
        person("jana"),
        "GET",
        &format!("{RUNS}/{run}/log?limit=3"),
        None,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    let page: Value = serde_json::from_str(&page.text).expect("json");
    assert_eq!(page["items"][0]["recordId"], "urn:s-9", "newest first");
    assert_eq!(page["items"][0]["outcome"], "rejected");
    assert!(!page.to_string().contains("urn:other"), "{page}");
    let next = page["next"].as_i64().expect("a next page");
    let rest = send(
        &state,
        person("jana"),
        "GET",
        &format!("{RUNS}/{run}/log?limit=3&before={next}"),
        None,
    )
    .await;
    let rest: Value = serde_json::from_str(&rest.text).expect("json");
    assert_eq!(rest["items"].as_array().map(Vec::len), Some(1));

    for path in [RUNS.to_owned(), format!("{RUNS}/{run}/log")] {
        let refused = send(&state, person("mikko"), "GET", &path, None).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{path}: {}",
            refused.text
        );
        assert!(!refused.text.contains("urn:s-"));
    }
}

/// T-4593: a steward bound to one space with propose on Pipeline replays the pipelines that
/// write that space and no other, and never reaches the project's pipeline templates, which
/// live in no space.
#[tokio::test]
async fn a_space_steward_replays_its_spaces_pipeline_only_and_never_the_templates() {
    let runner = common::runner().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/helsinki/streams/rejected-replay-[a-z0-9]+$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    let state = retry_world(&runner).await;
    state.mirror.upsert(envelope(
        "RoleBinding",
        "sanna-helsinki-pipelines",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "sanna@hel.fi" }],
            "role": "pipeline-author",
            "scope": { "contextSpace": "helsinki" },
        }),
    ));
    state.mirror.upsert(envelope(
        "ContextSpace",
        "water",
        "helsinki",
        json!({ "urnSegment": "water" }),
    ));
    state.mirror.upsert(envelope(
        "Endpoint",
        "lakes",
        "helsinki",
        json!({ "contextSpaceRef": "water", "slug": "w3qz8rmn5tkd2vxc7bpya4hfge" }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "lake-feed",
        "helsinki",
        json!({
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:lakes"
        }),
    ));
    let clean = ids(&state).await[0];

    let other = send(
        &state,
        person("sanna"),
        "POST",
        "/api/v1/projects/helsinki/pipelines/lake-feed/rejected/retry",
        Some(json!({ "ids": [clean] })),
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
    for (method, uri, body) in [
        ("GET", "/api/v1/projects/helsinki/pipelinetemplates", None),
        (
            "POST",
            "/api/v1/projects/helsinki/pipelinetemplates",
            Some(json!({ "name": "t" })),
        ),
        (
            "DELETE",
            "/api/v1/projects/helsinki/pipelinetemplates/x",
            None,
        ),
    ] {
        let answer = send(&state, person("sanna"), method, uri, body).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{method} {uri}: {}",
            answer.text
        );
    }

    let own = send(
        &state,
        person("sanna"),
        "POST",
        RETRY,
        Some(json!({ "ids": [clean] })),
    )
    .await;
    assert_eq!(own.status, StatusCode::ACCEPTED, "{}", own.text);
}

/// T-5818: the pipeline list reads the newest run of every pipeline in one request, and only of
/// the pipelines the caller reads: a steward of one space sees the pipeline writing it alone.
#[tokio::test]
async fn the_latest_runs_are_one_read_of_the_pipelines_the_caller_reads() {
    use joinedcontext_portal::pipeline_log::{NewLine, Outcome};
    const LATEST: &str = "/api/v1/projects/helsinki/pipeline-runs";
    let runner = common::runner().await;
    let state = retry_world(&runner).await;
    state.mirror.upsert(envelope(
        "RoleBinding",
        "sanna-helsinki-pipelines",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "sanna@hel.fi" }],
            "role": "pipeline-author",
            "scope": { "contextSpace": "helsinki" },
        }),
    ));
    state.mirror.upsert(envelope(
        "ContextSpace",
        "water",
        "helsinki",
        json!({ "urnSegment": "water" }),
    ));
    state.mirror.upsert(envelope(
        "Endpoint",
        "lakes",
        "helsinki",
        json!({ "contextSpaceRef": "water", "slug": "w3qz8rmn5tkd2vxc7bpya4hfge" }),
    ));
    for name in ["lake-feed", "parks", "idle"] {
        state.mirror.upsert(envelope(
            "Pipeline",
            name,
            "helsinki",
            json!({ "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:lakes" }),
        ));
    }
    let sent = |n: usize| -> Vec<NewLine> {
        (0..n)
            .map(|i| NewLine {
                record_id: format!("urn:s-{i}"),
                step: None,
                outcome: Outcome::Sent,
                message: String::new(),
            })
            .collect()
    };
    for (pipeline, run, n) in [
        ("stations", "2026-10-26T08:00:00Z", 1),
        ("stations", "2026-10-26T09:00:00Z", 4),
        ("lake-feed", "2026-10-26T09:00:00Z", 2),
        ("parks", "2026-10-26T09:00:00Z", 3),
    ] {
        state
            .pipeline_log
            .append("helsinki", pipeline, run, &sent(n))
            .await
            .expect("kept");
    }

    let all = send(&state, person("jana"), "GET", LATEST, None).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.text);
    let all: Value = serde_json::from_str(&all.text).expect("json");
    let items = all["items"].as_array().expect("items");
    let read: Vec<(&str, &str, u64)> = items
        .iter()
        .map(|i| {
            (
                i["pipeline"].as_str().unwrap_or_default(),
                i["run"]["run"].as_str().unwrap_or_default(),
                i["run"]["sent"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            ("lake-feed", "2026-10-26T09:00:00Z", 2),
            ("parks", "2026-10-26T09:00:00Z", 3),
            ("stations", "2026-10-26T09:00:00Z", 4),
        ]
    );

    let own = send(&state, person("sanna"), "GET", LATEST, None).await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text);
    let own: Value = serde_json::from_str(&own.text).expect("json");
    let names: Vec<&str> = own["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|i| i["pipeline"].as_str())
        .collect();
    assert_eq!(names, ["stations"]);

    // A caller who cannot read the project is told it is not there (PF-59).
    let refused = send(&state, person("mikko"), "GET", LATEST, None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    assert!(!refused.text.contains("stations"), "{}", refused.text);
}

/// T-5818: in Postgres the latest runs are one row per pipeline asked, its newest by `last_at`,
/// and nothing of a pipeline not asked or of another project.
#[tokio::test]
async fn postgres_answers_the_newest_run_of_each_pipeline_asked() {
    use joinedcontext_portal::pipeline_log::{LogStore, NewLine, Outcome};
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let log = LogStore::new(Some(pool));
    let project = format!("t5818-{}", std::process::id());
    let sent = [NewLine {
        record_id: "urn:s-1".into(),
        step: None,
        outcome: Outcome::Sent,
        message: String::new(),
    }];
    for (proj, pipeline, run) in [
        (project.as_str(), "a", "2026-10-26T08:00:00Z"),
        (project.as_str(), "a", "2026-10-26T09:00:00Z"),
        (project.as_str(), "b", "2026-10-26T07:00:00Z"),
        (project.as_str(), "hidden", "2026-10-26T09:00:00Z"),
        ("t5818-other", "a", "2026-10-26T10:00:00Z"),
    ] {
        log.append(proj, pipeline, run, &sent).await.expect("kept");
    }
    let asked = ["a", "b", "never"].map(str::to_owned);
    let latest = log.latest_runs(&project, &asked).await.expect("read");
    let runs: Vec<(&str, &str)> = latest
        .iter()
        .map(|(name, run)| (name.as_str(), run.run.as_str()))
        .collect();
    assert_eq!(
        runs,
        [("a", "2026-10-26T09:00:00Z"), ("b", "2026-10-26T07:00:00Z")]
    );
}
