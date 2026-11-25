//! The pipeline run log on its database (PL-62, T-7198, T-7823): a run with its counts and lines,
//! the changes its records made, the reads the run queue and the alerts make, a project that has
//! no key, a project of another Organization and a database that does not answer.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names. Every test names its projects
//! afresh, because the tests share one database.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use common::unique::unique;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_log::{LogStore, NewLine, Outcome};
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::json;

const REFUSED: &str = "access revoked by anna on 2026-10-21; the gateway answered: 403";
const DM61: &str = "entity type `Tree` is not a class of the space's data model `m` (DM-61)";

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

fn dead_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool")
}

fn line(id: &str, outcome: Outcome, message: &str) -> NewLine {
    NewLine {
        record_id: id.to_owned(),
        step: None,
        outcome,
        message: message.to_owned(),
    }
}

fn project_name() -> String {
    format!("p{}", unique())
}

/// A store on the database with one project the registry keys.
async fn keyed_store() -> (LogStore, String) {
    let pool = pool().await;
    let project = project_name();
    let mirror = common::keyed_mirror(&pool, &[&project]).await;
    (LogStore::new(Some(pool), mirror), project)
}

#[tokio::test]
async fn a_run_keeps_its_counts_and_its_lines_newest_first_in_the_database() {
    let (store, project) = keyed_store().await;
    let step_two = NewLine {
        record_id: "urn:b".into(),
        step: Some(2),
        outcome: Outcome::Rejected,
        message: "capacity is not an integer".into(),
    };
    store
        .append(
            &project,
            "stations",
            "r1",
            &[
                line("urn:a", Outcome::Sent, ""),
                step_two,
                line("urn:c", Outcome::Failed, "step 2 threw"),
            ],
        )
        .await
        .expect("append");
    let runs = store.runs(&project, "stations", 10).await.expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].sent, runs[0].rejected, runs[0].failed), (1, 1, 1));
    assert_eq!(runs[0].created, None, "no hashes were reported");
    let lines = store
        .lines(&project, "stations", "r1", 10, None)
        .await
        .expect("lines");
    let ids: Vec<&str> = lines.iter().map(|l| l.record_id.as_str()).collect();
    assert_eq!(ids, ["urn:c", "urn:b", "urn:a"]);
    assert_eq!(
        lines.iter().map(|l| l.outcome).collect::<Vec<_>>(),
        [Outcome::Failed, Outcome::Rejected, Outcome::Sent]
    );
    assert_eq!(lines[1].step, Some(2));
    assert_eq!(lines[1].message, "capacity is not an integer");
}

#[tokio::test]
async fn a_log_is_paged_by_the_line_before_and_bounded_by_the_limit() {
    let (store, project) = keyed_store().await;
    let many: Vec<NewLine> = (0..7)
        .map(|n| line(&format!("urn:{n}"), Outcome::Sent, ""))
        .collect();
    store
        .append(&project, "x", "r1", &many)
        .await
        .expect("append");
    let first = store
        .lines(&project, "x", "r1", 3, None)
        .await
        .expect("page");
    assert_eq!(first.len(), 3);
    let next = store
        .lines(&project, "x", "r1", 3, Some(first[2].id))
        .await
        .expect("page");
    assert_eq!(next.len(), 3);
    assert!(next[0].id < first[2].id);
    let rest = store
        .lines(&project, "x", "r1", 3, Some(next[2].id))
        .await
        .expect("page");
    assert_eq!(rest.len(), 1);
    assert!(store
        .lines(&project, "x", "no-such-run", 3, None)
        .await
        .expect("page")
        .is_empty());
    assert_eq!(
        store
            .lines(&project, "x", "r1", 0, None)
            .await
            .expect("page")
            .len(),
        1,
        "a limit of none is one line"
    );
}

#[tokio::test]
async fn a_report_with_no_line_is_a_run_that_wrote_nothing() {
    let (store, project) = keyed_store().await;
    store
        .append(&project, "x", "r1", &[])
        .await
        .expect("append");
    let runs = store.runs(&project, "x", 10).await.expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].sent, runs[0].rejected, runs[0].failed), (0, 0, 0));
    assert!(store
        .lines(&project, "x", "r1", 10, None)
        .await
        .expect("lines")
        .is_empty());
}

#[tokio::test]
async fn the_changes_a_report_made_are_split_by_hash_and_added_to_its_run() {
    let (store, project) = keyed_store().await;
    store
        .append(&project, "x", "r1", &[line("urn:a", Outcome::Sent, "")])
        .await
        .expect("append");
    let hashed = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, hash)| ((*id).to_owned(), (*hash).to_owned()))
            .collect()
    };
    let first = store
        .record_changes(
            &project,
            "x",
            "r1",
            &hashed(&[("urn:a", "h1"), ("urn:b", "h1"), ("", "skipped")]),
        )
        .await
        .expect("changes");
    assert_eq!((first.created, first.updated, first.unchanged), (2, 0, 0));
    let second = store
        .record_changes(
            &project,
            "x",
            "r1",
            &hashed(&[("urn:a", "h1"), ("urn:b", "h2"), ("urn:c", "h1")]),
        )
        .await
        .expect("changes");
    assert_eq!(
        (second.created, second.updated, second.unchanged),
        (1, 1, 1)
    );
    let run = &store.runs(&project, "x", 1).await.expect("runs")[0];
    assert_eq!(
        (run.created, run.updated, run.unchanged),
        (Some(3), Some(1), Some(1))
    );
    let none = store
        .record_changes(&project, "x", "r1", &[])
        .await
        .expect("changes");
    assert_eq!((none.created, none.updated, none.unchanged), (0, 0, 0));
}

#[tokio::test]
async fn the_newest_run_of_each_pipeline_is_read_in_one_call_and_unknown_names_are_left_out() {
    let (store, project) = keyed_store().await;
    for (pipeline, run) in [("a", "r1"), ("a", "r2"), ("b", "r1")] {
        store
            .append(&project, pipeline, run, &[line("urn:x", Outcome::Sent, "")])
            .await
            .expect("append");
    }
    store
        .record_changes(&project, "a", "r2", &[("urn:x".to_owned(), "h".to_owned())])
        .await
        .expect("changes");
    let names = ["a", "b", "none"].map(str::to_owned);
    let latest = store.latest_runs(&project, &names).await.expect("latest");
    assert_eq!(
        latest.keys().map(String::as_str).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(latest["a"].run, "r2");
    assert_eq!(latest["a"].created, Some(1));
    assert_eq!(latest["b"].run, "r1");
    assert!(store
        .latest_runs(&project, &[])
        .await
        .expect("latest")
        .is_empty());
}

#[tokio::test]
async fn a_pipeline_with_a_run_is_listed_with_its_time_and_a_stream_hash_only_for_a_logged_run() {
    let (store, project) = keyed_store().await;
    store
        .append(&project, "a", "r1", &[])
        .await
        .expect("append");
    store
        .append(&project, "b", "r1", &[])
        .await
        .expect("append");
    store
        .ran_stream(&project, "a", "r1", "stream-1")
        .await
        .expect("keep");
    store
        .ran_stream(&project, "a", "never-logged", "stream-2")
        .await
        .expect("keep");
    let ran = store.ran().await.expect("ran");
    let (a, b) = (
        (project.clone(), "a".to_owned()),
        (project.clone(), "b".to_owned()),
    );
    for key in [&a, &b] {
        let last = ran[key];
        assert!((jc_time::TimestampTz::now() - last).as_secs().abs() < 120);
    }
    let streams = store.ran_streams().await.expect("streams");
    assert_eq!(streams[&a], "stream-1");
    assert!(!streams.contains_key(&b), "b ran with no stream hash");
}

#[tokio::test]
async fn three_refused_runs_in_a_row_are_a_streak_in_the_database() {
    let (store, project) = keyed_store().await;
    for (pipeline, runs) in [("cut", ["r1", "r2", "r3"]), ("broken", ["r1", "r2", "r3"])] {
        for run in runs {
            let message = if pipeline == "broken" && run == "r2" {
                "timeout"
            } else {
                REFUSED
            };
            store
                .append(
                    &project,
                    pipeline,
                    run,
                    &[line("", Outcome::Failed, message)],
                )
                .await
                .expect("append");
        }
    }
    store
        .append(
            &project,
            "short",
            "r1",
            &[line("", Outcome::Failed, REFUSED)],
        )
        .await
        .expect("append");
    let streaks = store.refused_streaks(3).await.expect("streaks");
    assert_eq!(streaks.len(), 1, "{streaks:?}");
    assert_eq!(
        streaks
            .get(&(project.clone(), "cut".to_owned()))
            .map(String::as_str),
        Some(REFUSED)
    );
    assert!(
        store.refused_streaks(4).await.expect("streaks").is_empty(),
        "no pipeline has four runs"
    );
}

#[tokio::test]
async fn the_newest_run_refused_for_an_outside_cause_is_listed_with_its_words() {
    let (store, project) = keyed_store().await;
    for (pipeline, runs) in [
        ("trees", [("r1", "ok"), ("r2", DM61)]),
        ("healed", [("r1", DM61), ("r2", "ok")]),
        (
            "source",
            [
                ("r1", "ok"),
                ("r2", "HTTP request returned unexpected response code (404)"),
            ],
        ),
    ] {
        for (run, said) in runs {
            let (outcome, said) = if said == "ok" {
                (Outcome::Sent, "")
            } else {
                (Outcome::Failed, said)
            };
            store
                .append(&project, pipeline, run, &[line("urn:a", outcome, said)])
                .await
                .expect("append");
        }
    }
    let outside = store.outside_failures().await.expect("outside");
    assert_eq!(
        outside.get(&(project.clone(), "trees".to_owned())),
        Some(&("r2".to_owned(), DM61.to_owned()))
    );
    assert!(!outside.contains_key(&(project.clone(), "healed".to_owned())));
    assert!(!outside.contains_key(&(project.clone(), "source".to_owned())));
}

#[tokio::test]
async fn the_newest_line_that_did_not_send_is_the_last_problem() {
    let (store, project) = keyed_store().await;
    assert_eq!(store.last_problem(&project, "x").await.expect("read"), None);
    store
        .append(
            &project,
            "x",
            "r1",
            &[
                line("urn:1", Outcome::Failed, "timeout"),
                line("urn:2", Outcome::Rejected, "schema"),
                line("urn:3", Outcome::Sent, ""),
            ],
        )
        .await
        .expect("append");
    let (message, at) = store
        .last_problem(&project, "x")
        .await
        .expect("read")
        .expect("a problem");
    assert_eq!(message, "schema");
    assert!(at.ends_with('Z'), "{at}");
    assert_eq!(
        store.last_problem(&project, "other").await.expect("read"),
        None
    );
}

#[tokio::test]
async fn a_pipeline_that_is_gone_takes_its_runs_and_log_with_it_in_the_database() {
    let (store, project) = keyed_store().await;
    for pipeline in ["kept", "deleted"] {
        store
            .append(&project, pipeline, "r", &[line("urn", Outcome::Sent, "")])
            .await
            .expect("append");
    }
    store
        .record_changes(
            &project,
            "deleted",
            "r",
            &[("urn".to_owned(), "h".to_owned())],
        )
        .await
        .expect("changes");
    let loaded = BTreeMap::from([(project.clone(), HashSet::from(["kept".to_owned()]))]);
    store.forget_gone(&loaded).await.expect("forgotten");
    assert_eq!(
        store.runs(&project, "kept", 10).await.expect("runs").len(),
        1
    );
    assert!(store
        .runs(&project, "deleted", 10)
        .await
        .expect("runs")
        .is_empty());
    assert!(store
        .lines(&project, "deleted", "r", 10, None)
        .await
        .expect("lines")
        .is_empty());
    let none = BTreeMap::<String, HashSet<String>>::new();
    store.forget_gone(&none).await.expect("nothing loaded");
    assert_eq!(
        store.runs(&project, "kept", 10).await.expect("runs").len(),
        1
    );
}

#[tokio::test]
async fn a_pipeline_of_another_organization_never_shows_in_this_one() {
    let pool = pool().await;
    let state = AppState::new(Config::for_tests(), None);
    let (here, there) = (project_name(), project_name());
    common::keyed_project(&state, &pool, &here, "helsinki").await;
    common::keyed_project(&state, &pool, &there, "espoo").await;
    let store = LogStore::new(Some(pool), state.mirror.clone());
    store
        .append(&here, "x", "r1", &[line("urn:a", Outcome::Failed, DM61)])
        .await
        .expect("append");
    store
        .append(
            &there,
            "x",
            "r1",
            &[
                line("urn:a", Outcome::Sent, ""),
                line("urn:b", Outcome::Sent, ""),
            ],
        )
        .await
        .expect("append");
    assert_eq!(store.runs(&here, "x", 5).await.expect("runs")[0].sent, 0);
    assert_eq!(store.runs(&there, "x", 5).await.expect("runs")[0].sent, 2);
    assert_eq!(
        store
            .lines(&here, "x", "r1", 9, None)
            .await
            .expect("lines")
            .len(),
        1
    );
    assert_eq!(
        store
            .lines(&there, "x", "r1", 9, None)
            .await
            .expect("lines")
            .len(),
        2
    );
    let ran = store.ran().await.expect("ran");
    assert!(ran.contains_key(&(here.clone(), "x".to_owned())));
    assert!(ran.contains_key(&(there.clone(), "x".to_owned())));
    let outside = store.outside_failures().await.expect("outside");
    assert!(outside.contains_key(&(here.clone(), "x".to_owned())));
    assert!(!outside.contains_key(&(there.clone(), "x".to_owned())));
    let loaded = BTreeMap::from([(here.clone(), HashSet::new())]);
    store.forget_gone(&loaded).await.expect("forgotten");
    assert!(store.runs(&here, "x", 5).await.expect("runs").is_empty());
    assert_eq!(
        store.runs(&there, "x", 5).await.expect("runs").len(),
        1,
        "the other Organization's project was not loaded, so it keeps its pipelines"
    );
}

/// What a project with no tenancy key does with every call: nothing is kept and nothing is read.
async fn keeps_nothing(store: &LogStore, project: &str) {
    store
        .append(project, "x", "r1", &[line("urn:a", Outcome::Failed, DM61)])
        .await
        .expect("append is not an error");
    store
        .append(project, "x", "r2", &[])
        .await
        .expect("a second refusal is not an error either");
    let changes = store
        .record_changes(project, "x", "r1", &[("urn:a".to_owned(), "h".to_owned())])
        .await
        .expect("changes");
    assert_eq!(
        (changes.created, changes.updated, changes.unchanged),
        (0, 0, 0)
    );
    store
        .ran_stream(project, "x", "r1", "h")
        .await
        .expect("keep");
    assert!(store.runs(project, "x", 5).await.expect("runs").is_empty());
    assert!(store
        .latest_runs(project, &["x".to_owned()])
        .await
        .expect("latest")
        .is_empty());
    assert!(store
        .lines(project, "x", "r1", 5, None)
        .await
        .expect("lines")
        .is_empty());
    assert_eq!(store.last_problem(project, "x").await.expect("read"), None);
    let loaded = BTreeMap::from([(project.to_owned(), HashSet::new())]);
    store.forget_gone(&loaded).await.expect("forgotten");
}

#[tokio::test]
async fn a_project_the_mirror_does_not_hold_keeps_no_run_history() {
    let pool = pool().await;
    let store = LogStore::new(Some(pool), Arc::new(Mirror::new()));
    keeps_nothing(&store, &project_name()).await;
}

#[tokio::test]
async fn a_project_of_an_organization_the_registry_does_not_hold_keeps_no_run_history() {
    let pool = pool().await;
    let project = project_name();
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(common::envelope(
        "Project",
        &project,
        ORG_NAMESPACE,
        json!({ "organizationRef": format!("unregistered-{}", unique()) }),
    ));
    let store = LogStore::new(Some(pool), mirror);
    keeps_nothing(&store, &project).await;
    let ran = store.ran().await.expect("ran");
    assert!(!ran.keys().any(|(name, _)| *name == project));
}

#[tokio::test]
async fn a_database_that_does_not_answer_fails_every_call() {
    let project = project_name();
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(common::envelope(
        "Project",
        &project,
        ORG_NAMESPACE,
        json!({ "organizationRef": "somewhere" }),
    ));
    let store = LogStore::new(Some(dead_pool()), mirror);
    assert!(store
        .append(&project, "x", "r1", &[line("urn:a", Outcome::Sent, "")])
        .await
        .is_err());
    assert!(store
        .record_changes(&project, "x", "r1", &[("urn:a".to_owned(), "h".to_owned())])
        .await
        .is_err());
    assert!(store
        .forget_gone(&BTreeMap::from([(project.clone(), HashSet::new())]))
        .await
        .is_err());
    assert!(store.ran().await.is_err());
    assert!(store.ran_stream(&project, "x", "r1", "h").await.is_err());
    assert!(store.ran_streams().await.is_err());
    assert!(store.runs(&project, "x", 5).await.is_err());
    assert!(store
        .latest_runs(&project, &["x".to_owned()])
        .await
        .is_err());
    assert!(store.refused_streaks(3).await.is_err());
    assert!(store.outside_failures().await.is_err());
    assert!(store.last_problem(&project, "x").await.is_err());
    assert!(store.lines(&project, "x", "r1", 5, None).await.is_err());
}
