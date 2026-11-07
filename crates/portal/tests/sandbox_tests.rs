//! A run's tests in the sandbox namespace (SDK-38, T-2675): the Portal creates one ConfigMap and
//! one Job, waits for the Job, reads its pod's log and removes both, whatever the outcome.

use std::collections::BTreeMap;
use std::time::Duration;

use joinedcontext_portal::agents::sandbox::{Outcome, Report, Sandbox, SandboxSettings};
use joinedcontext_portal::apps::kube::KubeClient;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NS: &str = "jc-app-tests";
const NAME: &str = "app-test-run1-2";
const CM: &str = "/api/v1/namespaces/jc-app-tests/configmaps";
const JOBS: &str = "/apis/batch/v1/namespaces/jc-app-tests/jobs";

fn sandbox(server: &MockServer) -> Sandbox {
    let settings = SandboxSettings {
        namespace: NS.into(),
        image: format!("registry/app-builder:1@sha256:{}", "a".repeat(64)),
        rust_image: None,
    };
    Sandbox::new(
        KubeClient::with_token(&server.uri(), "t").expect("a client"),
        settings,
    )
}

fn tested() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "src/App.tsx".to_owned(),
            "export default () => null".to_owned(),
        ),
        (
            "src/App.test.tsx".to_owned(),
            "test('x', () => {})".to_owned(),
        ),
    ])
}

/// Deletes answer 404 (nothing left over) until the run creates the objects.
async fn deletes(server: &MockServer) {
    for collection in [CM, JOBS] {
        Mock::given(method("DELETE"))
            .and(path(format!("{collection}/{NAME}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(server)
            .await;
    }
}

async fn creates(server: &MockServer) {
    for collection in [CM, JOBS] {
        Mock::given(method("POST"))
            .and(path(collection))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .expect(1)
            .mount(server)
            .await;
    }
}

async fn run(sandbox: &Sandbox, files: &BTreeMap<String, String>, wait_ms: u64) -> Report {
    sandbox
        .run_with(
            "run1",
            2,
            files,
            Duration::from_millis(wait_ms),
            Duration::from_millis(10),
        )
        .await
}

#[tokio::test]
async fn a_finished_job_s_log_is_the_report_and_both_objects_are_removed() {
    let server = MockServer::start().await;
    deletes(&server).await;
    creates(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("{JOBS}/{NAME}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": { "conditions": [{ "type": "Complete", "status": "True" }] }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/namespaces/jc-app-tests/pods"))
        .and(query_param("labelSelector", format!("job-name={NAME}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "metadata": { "name": "app-test-run1-2-xyz" } }]
        })))
        .mount(&server)
        .await;
    let log = format!(
        "npm noise\nJC-TESTS {}\n",
        json!({"outcome": "failed", "passed": 3, "failed": 1, "failures": [
            {"file": "src/App.test.tsx", "name": "shows the list", "message": "expected 3 rows"}
        ]})
    );
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/namespaces/jc-app-tests/pods/app-test-run1-2-xyz/log",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(log))
        .mount(&server)
        .await;

    let report = run(&sandbox(&server), &tested(), 2_000).await;
    assert_eq!(report.outcome, Outcome::Failed, "{report:?}");
    assert_eq!((report.passed, report.failed), (3, 1));
    assert_eq!(report.failures[0].name, "shows the list");
    assert!(report.duration_ms.is_some());

    // Removed before (a leftover) and after the run.
    let requests = server.received_requests().await.expect("recorded");
    let removed = |collection: &str| {
        requests
            .iter()
            .filter(|r| {
                r.method.as_str() == "DELETE" && r.url.path() == format!("{collection}/{NAME}")
            })
            .count()
    };
    assert_eq!((removed(CM), removed(JOBS)), (2, 2));
}

#[tokio::test]
async fn a_job_that_never_ends_is_an_error_and_is_still_removed() {
    let server = MockServer::start().await;
    deletes(&server).await;
    creates(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("{JOBS}/{NAME}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "status": { "active": 1 } })),
        )
        .mount(&server)
        .await;

    let report = run(&sandbox(&server), &tested(), 50).await;
    assert_eq!(report.outcome, Outcome::Error);
    assert!(
        report
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("did not finish"),
        "{report:?}"
    );
    let requests = server.received_requests().await.expect("recorded");
    assert!(requests
        .iter()
        .any(|r| r.method.as_str() == "DELETE" && r.url.path() == format!("{JOBS}/{NAME}")));
}

#[tokio::test]
async fn a_refused_create_is_an_error_naming_the_object_and_no_job_starts() {
    let server = MockServer::start().await;
    deletes(&server).await;
    Mock::given(method("POST"))
        .and(path(CM))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"message": "forbidden"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(JOBS))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;

    let report = run(&sandbox(&server), &tested(), 2_000).await;
    assert_eq!(report.outcome, Outcome::Error);
    assert!(
        report
            .reason
            .as_deref()
            .unwrap_or("")
            .contains(&format!("ConfigMap {NAME}")),
        "{report:?}"
    );
}

#[tokio::test]
async fn a_version_without_tests_never_reaches_the_cluster() {
    let server = MockServer::start().await;
    let files = BTreeMap::from([("src/App.tsx".to_owned(), "x".to_owned())]);
    let report = run(&sandbox(&server), &files, 2_000).await;
    assert_eq!(report.outcome, Outcome::Skipped);
    assert!(server
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}
