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
async fn a_refused_job_is_an_error_naming_it_and_its_config_map_is_still_removed() {
    let server = MockServer::start().await;
    deletes(&server).await;
    Mock::given(method("POST"))
        .and(path(CM))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(JOBS))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"message": "quota"})))
        .mount(&server)
        .await;

    let report = run(&sandbox(&server), &tested(), 2_000).await;
    assert_eq!(report.outcome, Outcome::Error);
    let reason = report.reason.unwrap_or_default();
    assert!(
        reason.contains(&format!("Job {NAME}")) && reason.contains("quota"),
        "{reason}"
    );
    let requests = server.received_requests().await.expect("recorded");
    let removed = requests
        .iter()
        .filter(|r| r.method.as_str() == "DELETE" && r.url.path() == format!("{CM}/{NAME}"))
        .count();
    assert_eq!(removed, 2, "removed before and after the run");
}

/// The Job's own state as `GET` answers it: `status` and body.
async fn job_answers(server: &MockServer, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{JOBS}/{NAME}")))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

async fn pods_answer(server: &MockServer, status: u16, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/api/v1/namespaces/jc-app-tests/pods"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

const COMPLETE: &str = r#"{"status":{"conditions":[{"type":"Complete","status":"True"}]}}"#;
const FAILED: &str = r#"{"status":{"conditions":[{"type":"Failed","status":"True"}]}}"#;

/// Each way the cluster can fail a run after the create is an error that names what it could
/// not read, never a guess at the tests' outcome.
#[tokio::test]
async fn what_the_cluster_cannot_answer_is_an_error_naming_it() {
    let no_pods = json!({"items": []});
    let one_pod = json!({"items": [{"metadata": {"name": "p1"}}]});
    for (job, pods, log, want) in [
        ((404, "{}"), None, None, format!("the Job {NAME} is gone")),
        (
            (500, r#"{"message":"etcd down"}"#),
            None,
            None,
            format!("Job {NAME}"),
        ),
        (
            (200, COMPLETE),
            Some((500, json!({"message": "x"}))),
            None,
            format!("the pods of {NAME}"),
        ),
        (
            (200, COMPLETE),
            Some((200, no_pods.clone())),
            None,
            format!("the Job {NAME} ended with no pod to read"),
        ),
        (
            (200, FAILED),
            Some((200, no_pods.clone())),
            None,
            format!("the Job {NAME} stopped before its pod ran"),
        ),
        (
            (200, COMPLETE),
            Some((200, one_pod.clone())),
            Some(500),
            "the log of p1".to_owned(),
        ),
    ] {
        let server = MockServer::start().await;
        deletes(&server).await;
        creates(&server).await;
        job_answers(&server, job.0, job.1).await;
        if let Some((status, body)) = pods {
            pods_answer(&server, status, body).await;
        }
        if let Some(status) = log {
            Mock::given(method("GET"))
                .and(path("/api/v1/namespaces/jc-app-tests/pods/p1/log"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;
        }
        let report = run(&sandbox(&server), &tested(), 2_000).await;
        assert_eq!(report.outcome, Outcome::Error, "{want}: {report:?}");
        let reason = report.reason.unwrap_or_default();
        assert!(reason.contains(&want), "{want}: {reason}");
    }
}

/// An API server nobody answers on (port 1 refuses the connection): the error names the
/// object and what the connection said, never an API status it did not get.
#[tokio::test]
async fn an_unreachable_api_server_is_an_error_naming_the_object() {
    let settings = SandboxSettings {
        namespace: NS.into(),
        image: format!("registry/app-builder:1@sha256:{}", "a".repeat(64)),
        rust_image: None,
    };
    let refused = Sandbox::new(
        KubeClient::with_token("http://127.0.0.1:1", "t").expect("a client"),
        settings,
    );
    let report = run(&refused, &tested(), 2_000).await;
    assert_eq!(report.outcome, Outcome::Error, "{report:?}");
    let reason = report.reason.unwrap_or_default();
    assert!(
        reason.starts_with(&format!("ConfigMap {NAME}: ")),
        "{reason}"
    );
    assert!(!reason.contains("the API server answered"), "{reason}");
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
