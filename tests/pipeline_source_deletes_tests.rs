//! A snapshot pipeline follows what its source removed (PL-97, PL-100, T-4891): the source drops 3
//! of 50 records and, after two complete runs without them, exactly those 3 are handed to the
//! runner to delete through the pipeline's Endpoint with the reason; a source that answers 500 or
//! an empty list deletes nothing and is held; another pipeline's entities are never in the
//! ledger; more than the share is held until a steward releases or keeps it, and a reader may not.

mod common;

use base64::Engine;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use axum::http::StatusCode;
use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_log::{NewLine, Outcome};
use joinedcontext_portal::pipeline_source_deletes::{
    marked_at_source, note_seen, reported, run_ended, HoldReason, Marks, Report, ReportCount,
    RunEnd,
};
use joinedcontext_portal::state::AppState;

const PAGE: &str = "/api/v1/projects/helsinki/pipelines/registry/source-deletes";
const STREAM: &str = "/streams/helsinki.registry.source-deletes";
const EVENTS_STREAM: &str = "/streams/helsinki.changes.source-deletes";

/// Each launch has its own stream, the pipeline's name and a random 16-hex suffix (R2POR5).
fn launch_pattern(stream: &str) -> String {
    format!("{}\\.[0-9a-f]{{16}}", stream.replace('.', "\\."))
}

/// Whether `path` is a launch of `stream`.
fn launch_of(path: &str, stream: &str) -> bool {
    path.strip_prefix(stream)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|suffix| suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn id(n: usize) -> String {
    format!("urn:ngsi-ld:Dataset:hel.fi:open:ds-{n}")
}

/// `jana` may approve pipelines and reads the space `open`, `eva` only reads pipelines; `registry` harvests a catalogue every night
/// and deletes what two complete runs in a row no longer list, at most `max_share` percent.
async fn world(max_share: u8) -> (AppState, MockServer) {
    let runner = MockServer::start().await;
    for stream in [STREAM, EVENTS_STREAM] {
        Mock::given(method("POST"))
            .and(path_regex(format!("^{}$", launch_pattern(stream))))
            .respond_with(ResponseTemplate::new(200))
            .mount(&runner)
            .await;
    }
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(runner.uri());
    config.pipeline_test_capture_url = Some("http://portal-internal:8081".to_owned());
    let state = AppState::new(config, None);
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
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-space-reader",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "space-reader",
            "scope": { "project": "helsinki" },
        }),
    ));
    state
        .mirror
        .upsert(envelope("ContextSpace", "open", "helsinki", json!({})));
    state.mirror.upsert(envelope(
        "Endpoint",
        "open-write",
        "helsinki",
        json!({ "contextSpaceRef": "open", "slug": "k7r2m4xq9vbn3tdw6hcy5pajfe" }),
    ));
    for (name, deletes) in [
        (
            "registry",
            json!({ "mode": "snapshot", "misses": 2, "maxShare": max_share }),
        ),
        ("other", json!({ "mode": "never" })),
        (
            "changes",
            json!({ "mode": "events", "deletedWhen": "this.status.or(\"\") == \"withdrawn\"" }),
        ),
    ] {
        state.mirror.upsert(envelope(
            "Pipeline",
            name,
            "helsinki",
            json!({
                "class": "scheduled",
                "schedule": "0 3 * * *",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "ckan" } },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:open-write",
                "sourceDeletes": deletes,
            }),
        ));
    }
    (state, runner)
}

/// One run of `pipeline` that wrote `ids` and failed on `failed` records, then ended `finished`.
async fn run(
    state: &AppState,
    pipeline: &str,
    run: &str,
    ids: &[String],
    failed: usize,
    finished: bool,
) {
    run_reported(
        state,
        pipeline,
        run,
        ids,
        failed,
        finished,
        ReportCount::Complete,
    )
    .await;
}

/// [`run`], with what the runner counted of the run's outcome reports it could not deliver.
async fn run_reported(
    state: &AppState,
    pipeline: &str,
    run: &str,
    ids: &[String],
    failed: usize,
    finished: bool,
    reports: ReportCount,
) {
    let mut lines: Vec<NewLine> = ids
        .iter()
        .map(|id| NewLine {
            record_id: id.clone(),
            step: None,
            outcome: Outcome::Sent,
            message: String::new(),
        })
        .collect();
    lines.extend((0..failed).map(|_| NewLine {
        record_id: String::new(),
        step: None,
        outcome: Outcome::Failed,
        message: "the source answered 500 Internal Server Error".to_owned(),
    }));
    state
        .pipeline_log
        .append("helsinki", pipeline, run, &lines)
        .await
        .expect("log");
    note_seen(state, "helsinki", pipeline, run, ids, true).await;
    run_ended(
        state,
        RunEnd {
            project: "helsinki".to_owned(),
            pipeline: pipeline.to_owned(),
            run: run.to_owned(),
            finished,
            reports,
            stream: String::new(),
        },
    )
    .await;
}

fn without(dropped: &[usize]) -> Vec<String> {
    (0..50).filter(|n| !dropped.contains(n)).map(id).collect()
}

/// The ids and the reason of every delete stream the runner was handed.
async fn handed(runner: &MockServer) -> Vec<(Vec<String>, String)> {
    let requests = runner.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && launch_of(r.url.path(), STREAM))
        .map(|r| {
            let stream: Value = serde_json::from_slice(&r.body).expect("stream");
            let mapping = stream["input"]["generate"]["mapping"].as_str().expect("mapping");
            let encoded = mapping.split('"').nth(1).expect("encoded");
            let chunks: Vec<Value> = serde_json::from_slice(
                &base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .expect("base64"),
            )
            .expect("chunks");
            let ids = chunks
                .iter()
                .flat_map(|chunk| chunk["ids"].as_array().cloned().unwrap_or_default())
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect();
            let http = &stream["pipeline"]["processors"][1]["try"][2]["branch"]["processors"][0]["http"];
            assert_eq!(
                http["url"],
                "${JC_GATEWAY_URL}/api/endpoint/k7r2m4xq9vbn3tdw6hcy5pajfe/ngsi-ld/v1/entityOperations/delete"
            );
            (ids, http["headers"]["JC-Reason"].as_str().unwrap_or_default().to_owned())
        })
        .collect()
}

#[tokio::test]
async fn three_of_fifty_dropped_are_deleted_after_two_complete_runs_with_the_reason() {
    let (state, runner) = world(10).await;
    run(&state, "registry", "r1", &without(&[]), 0, true).await;
    run(&state, "registry", "r2", &without(&[4, 17, 33]), 0, true).await;
    assert!(handed(&runner).await.is_empty(), "one miss deletes nothing");

    let preview = send(&state, person("jana"), "GET", PAGE, None).await;
    assert_eq!(preview.status, StatusCode::OK, "{}", preview.text);
    let preview: Value = serde_json::from_str(&preview.text).expect("json");
    assert_eq!(preview["ledger"], 50);
    assert_eq!(preview["next"], json!([id(17), id(33), id(4)]));
    // eva reads the pipeline, not its space: the counts without one id of the space.
    let hidden = send(&state, person("eva"), "GET", PAGE, None).await;
    let hidden: Value = serde_json::from_str(&hidden.text).expect("json");
    assert_eq!(
        (hidden["orphans"].clone(), hidden["idsHidden"].clone()),
        (json!(3), json!(true))
    );
    assert_eq!(hidden["next"], json!([]));
    assert_eq!(hidden["orphanIds"], json!([]));

    run(&state, "registry", "r3", &without(&[4, 17, 33]), 0, true).await;
    let handed = handed(&runner).await;
    assert_eq!(handed.len(), 1);
    let (mut ids, reason) = handed[0].clone();
    ids.sort();
    assert_eq!(ids, [id(17), id(33), id(4)]);
    assert_eq!(
        reason,
        "source-snapshot%3A%20pipeline%20registry%20run%20r3"
    );

    // The stream reports what went: those rows leave the ledger and the feed names them.
    let report = Report {
        run: "r3".to_owned(),
        deleted: vec![id(4), id(17)],
        gone: vec![id(33)],
        failed: Vec::new(),
        edited: Vec::new(),
    };
    reported(&state, "helsinki", "registry", report)
        .await
        .expect("report");
    let (rows, hold) = state
        .source_ledger
        .book("helsinki", "registry")
        .await
        .expect("book");
    assert_eq!(rows.len(), 47);
    assert!(hold.is_none());
    let filter = joinedcontext_portal::activity::ActivityFilter {
        kinds: vec!["pipeline.source-delete".into()],
        limit: 10,
        ..Default::default()
    };
    let feed = state
        .activity
        .list("helsinki", &filter)
        .await
        .expect("activity")
        .items;
    assert!(
        feed.iter()
            .any(|event| event.kind == "pipeline.source-delete"
                && event.summary.contains("deleted 2 entities")),
        "{feed:?}"
    );

    // The report (T-4892): the source read 47 at r3, nothing orphaned any more, the two deletes
    // with their reason, and the space says the same of each tombstone.
    let report = send(&state, person("jana"), "GET", PAGE, None).await;
    let report: Value = serde_json::from_str(&report.text).expect("json");
    assert_eq!(report["lastComplete"]["run"], "r3");
    assert_eq!(report["lastComplete"]["read"], 47);
    assert_eq!(
        (report["ledger"].clone(), report["orphans"].clone()),
        (json!(47), json!(0))
    );
    assert_eq!(report["space"], "open");
    let removed: Vec<&str> = report["removed"]
        .as_array()
        .expect("removed")
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(removed, [id(17), id(4)]);
    assert_eq!(
        report["removed"][0]["reason"],
        "source-snapshot: pipeline registry run r3"
    );
    let space = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/helsinki/spaces/open/source-removals",
        None,
    )
    .await;
    assert_eq!(space.status, StatusCode::OK, "{}", space.text);
    let space: Value = serde_json::from_str(&space.text).expect("json");
    assert_eq!(space.as_array().map(Vec::len), Some(2));
    assert_eq!(space[0]["pipeline"], "registry");
    assert_eq!(space[0]["run"], "r3");
}

#[tokio::test]
async fn the_report_counts_orphans_and_the_records_the_model_refused() {
    let (state, _runner) = world(10).await;
    run(&state, "registry", "r1", &without(&[]), 0, true).await;
    // r2: the source lists 47 of them and one record the model refuses, so it is not in the space.
    let refused = NewLine {
        record_id: "urn:ngsi-ld:Dataset:hel.fi:open:bad".to_owned(),
        step: None,
        outcome: Outcome::Rejected,
        message: "name is required".to_owned(),
    };
    state
        .pipeline_log
        .append("helsinki", "registry", "r2", &[refused])
        .await
        .expect("log");
    run(&state, "registry", "r2", &without(&[4, 17, 33]), 0, true).await;
    let report = send(&state, person("jana"), "GET", PAGE, None).await;
    assert_eq!(report.status, StatusCode::OK, "{}", report.text);
    let report: Value = serde_json::from_str(&report.text).expect("json");
    assert_eq!(report["orphans"], 3);
    assert_eq!(report["orphanIds"], json!([id(17), id(33), id(4)]));
    assert_eq!(report["missing"], 1);
    assert_eq!(
        report["missingIds"],
        json!(["urn:ngsi-ld:Dataset:hel.fi:open:bad"])
    );
    assert_eq!(report["lastComplete"]["read"], 48);
}

#[tokio::test]
async fn a_space_the_caller_cannot_read_lists_no_removal() {
    // eva reads the pipeline but not the space it writes: its tombstones stay out of her sight.
    let (state, _runner) = world(10).await;
    let other = send(
        &state,
        person("eva"),
        "GET",
        "/api/v1/projects/helsinki/spaces/open/source-removals",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
}

#[tokio::test]
async fn an_outage_a_partial_read_or_an_empty_answer_deletes_nothing_and_holds() {
    let (state, runner) = world(100).await;
    run(&state, "registry", "r1", &without(&[]), 0, true).await;
    // The source answered 500 on its second page: half read, one failed line.
    run(
        &state,
        "registry",
        "r2",
        &without(&(25..50).collect::<Vec<_>>()),
        1,
        true,
    )
    .await;
    let (rows, hold) = state
        .source_ledger
        .book("helsinki", "registry")
        .await
        .expect("book");
    assert!(
        rows.values().all(|row| row.misses == 0),
        "an incomplete run counts no miss"
    );
    assert_eq!(hold.map(|h| h.reason), Some(HoldReason::Incomplete));
    // A run that timed out is no complete run either.
    run(&state, "registry", "r3", &without(&[1]), 0, false).await;
    // The source answered an empty list, twice.
    run(&state, "registry", "r4", &[], 0, true).await;
    run(&state, "registry", "r5", &[], 0, true).await;
    let (rows, hold) = state
        .source_ledger
        .book("helsinki", "registry")
        .await
        .expect("book");
    assert!(rows.values().all(|row| row.misses == 0));
    let hold = hold.expect("held");
    assert_eq!((hold.reason, hold.ledger), (HoldReason::Empty, 50));
    assert!(handed(&runner).await.is_empty(), "nothing was deleted");

    // The alert names the hold to the pipeline's subscribers.
    let mut signals = Vec::new();
    joinedcontext_portal::alerts::held_signals(
        &mut signals,
        state.source_ledger.holds().await.expect("holds"),
    );
    assert_eq!(signals.len(), 1);
    assert!(signals[0].detail.contains("run r5 read no record"));

    // A complete run clears the hold.
    run(&state, "registry", "r6", &without(&[]), 0, true).await;
    assert!(state
        .source_ledger
        .book("helsinki", "registry")
        .await
        .expect("book")
        .1
        .is_none());
}

/// F-POR3-006: the ledger and the run log learn of a write from the same outcome report, so a run
/// whose first report the runner dropped looks complete and short of those ids. The runner's own
/// count of dropped reports makes it incomplete: it counts no miss and deletes nothing, and a run
/// without a count is held the same way.
#[tokio::test]
async fn a_run_whose_outcome_report_was_dropped_is_incomplete_and_deletes_nothing() {
    let (state, runner) = world(100).await;
    run(&state, "registry", "r1", &without(&[]), 0, true).await;
    let second_report_only = without(&(0..25).collect::<Vec<_>>());
    for (name, reports) in [
        ("r2", ReportCount::Lost(1)),
        ("r3", ReportCount::Lost(1)),
        (
            "r4",
            ReportCount::Unknown("the runner does not export the outcome_drop counter"),
        ),
        (
            "r5",
            ReportCount::Unknown("the runner does not export the outcome_drop counter"),
        ),
    ] {
        run_reported(
            &state,
            "registry",
            name,
            &second_report_only,
            0,
            true,
            reports,
        )
        .await;
        let (rows, hold) = state
            .source_ledger
            .book("helsinki", "registry")
            .await
            .expect("book");
        assert!(
            rows.values().all(|row| row.misses == 0),
            "{name}: a run with lost reports counts no miss"
        );
        let hold = hold.expect("held");
        assert_eq!(
            (hold.reason, hold.run.as_str()),
            (HoldReason::Incomplete, name)
        );
    }
    assert!(handed(&runner).await.is_empty(), "nothing was deleted");
}

#[tokio::test]
async fn another_writers_entities_are_never_in_the_ledger_and_never_deleted() {
    let (state, runner) = world(100).await;
    let shared: Vec<String> = (100..110).map(id).collect();
    for r in ["r1", "r2", "r3"] {
        run(&state, "other", r, &shared, 0, true).await;
        run(&state, "registry", r, &without(&[]), 0, true).await;
    }
    let (rows, _) = state
        .source_ledger
        .book("helsinki", "registry")
        .await
        .expect("book");
    assert!(shared.iter().all(|id| !rows.contains_key(id)));
    assert!(
        state
            .source_ledger
            .book("helsinki", "other")
            .await
            .expect("book")
            .0
            .is_empty(),
        "a never pipeline keeps no ledger"
    );
    assert!(handed(&runner).await.is_empty());

    // A delete report naming ids outside the ledger takes nothing out.
    let report = Report {
        run: "r3".to_owned(),
        deleted: shared.clone(),
        gone: Vec::new(),
        failed: Vec::new(),
        edited: Vec::new(),
    };
    reported(&state, "helsinki", "registry", report)
        .await
        .expect("report");
    assert_eq!(
        state
            .source_ledger
            .book("helsinki", "registry")
            .await
            .expect("book")
            .0
            .len(),
        50
    );
}

#[tokio::test]
async fn more_than_the_share_is_held_until_a_steward_releases_or_keeps_it() {
    let (state, runner) = world(10).await;
    let dropped: Vec<usize> = (0..8).collect();
    for r in ["r1", "r2", "r3"] {
        let ids = if r == "r1" {
            without(&[])
        } else {
            without(&dropped)
        };
        run(&state, "registry", r, &ids, 0, true).await;
    }
    assert!(handed(&runner).await.is_empty(), "8 of 50 is past 10 %");
    let preview = send(&state, person("jana"), "GET", PAGE, None).await;
    let preview: Value = serde_json::from_str(&preview.text).expect("json");
    assert_eq!(preview["hold"]["reason"], "share-exceeded");
    assert_eq!(preview["hold"]["would"], 8);
    assert_eq!(preview["due"].as_array().map(Vec::len), Some(8));

    let refused = send(
        &state,
        person("eva"),
        "POST",
        PAGE,
        Some(json!({ "decision": "delete" })),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(handed(&runner).await.is_empty());
    let bad = send(
        &state,
        person("jana"),
        "POST",
        PAGE,
        Some(json!({ "decision": "maybe" })),
    )
    .await;
    assert!(bad.status.is_client_error(), "{}", bad.status);

    let kept = send(
        &state,
        person("jana"),
        "POST",
        PAGE,
        Some(json!({ "decision": "keep" })),
    )
    .await;
    assert_eq!(kept.status, StatusCode::ACCEPTED, "{}", kept.text);
    let (rows, hold) = state
        .source_ledger
        .book("helsinki", "registry")
        .await
        .expect("book");
    assert!(hold.is_none() && rows.values().all(|row| row.misses == 0));
    let nothing = send(
        &state,
        person("jana"),
        "POST",
        PAGE,
        Some(json!({ "decision": "delete" })),
    )
    .await;
    assert_eq!(
        nothing.status,
        StatusCode::CONFLICT,
        "nothing is due after a keep"
    );

    for r in ["r4", "r5"] {
        run(&state, "registry", r, &without(&dropped), 0, true).await;
    }
    let released = send(
        &state,
        person("jana"),
        "POST",
        PAGE,
        Some(json!({ "decision": "delete" })),
    )
    .await;
    assert_eq!(released.status, StatusCode::ACCEPTED, "{}", released.text);
    assert_eq!(
        serde_json::from_str::<Value>(&released.text).expect("json")["deleting"],
        8
    );
    let handed = handed(&runner).await;
    assert_eq!(handed.len(), 1);
    assert_eq!(handed[0].0.len(), 8);
    assert!(state
        .source_ledger
        .book("helsinki", "registry")
        .await
        .expect("book")
        .1
        .is_none());

    // A pipeline without a snapshot rule has no dry run.
    let other = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/helsinki/pipelines/other/source-deletes",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::CONFLICT);
}

/// PL-98 (T-5360): what the source marked as deleted goes in one delete stream when the run ends,
/// with the source-event reason; a marked id the pipeline never wrote is never in it.
#[tokio::test]
async fn an_events_run_deletes_what_its_source_marked_once_the_run_ends() {
    let (state, runner) = world(10).await;
    let written: Vec<String> = (0..5).map(id).collect();
    run(&state, "changes", "r1", &written, 0, true).await;
    let marks: Marks = serde_json::from_value(json!({
        "run": "r2",
        "ids": [id(1), id(3), id(99)],
        "failed": [],
    }))
    .expect("marks");
    marked_at_source(&state, "helsinki", "changes", marks)
        .await
        .expect("marked");
    let before = runner.received_requests().await.unwrap_or_default();
    assert!(
        !before
            .iter()
            .any(|r| launch_of(r.url.path(), EVENTS_STREAM)),
        "nothing is deleted before the run ends"
    );
    run(&state, "changes", "r2", &[id(0)], 0, true).await;
    let requests = runner.received_requests().await.unwrap_or_default();
    let streams: Vec<Value> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && launch_of(r.url.path(), EVENTS_STREAM))
        .map(|r| serde_json::from_slice(&r.body).expect("stream"))
        .collect();
    assert_eq!(streams.len(), 1, "one delete stream per run");
    let mapping = streams[0]["input"]["generate"]["mapping"]
        .as_str()
        .expect("mapping");
    let chunks: Vec<Value> = serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(mapping.split('"').nth(1).expect("encoded"))
            .expect("base64"),
    )
    .expect("chunks");
    assert_eq!(chunks[0]["ids"], json!([id(1), id(3)]));
    assert_eq!(chunks[0]["run"], "r2");
    let http =
        &streams[0]["pipeline"]["processors"][1]["try"][2]["branch"]["processors"][0]["http"];
    assert_eq!(
        http["headers"]["JC-Reason"],
        "source-event%3A%20pipeline%20changes%20run%20r2"
    );
}
