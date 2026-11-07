//! T-1716 attack vector: a slow or dead dependency stalls everything.
//!
//! A dependency that accepts the connection and never answers must cost the caller a bounded
//! wait and an error, never a task parked forever: the app reconciler's Kubernetes calls and
//! the artifact store's provisioning run inside the reconciler loop, and one hung call there
//! stops every App and every organization behind it.
//!
//! The clock is paused: the runtime jumps to the next timer whenever it would otherwise idle, so
//! a client with a timeout fails at once and a client without one trips the outer guard.

use std::time::Duration;

use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::artifact_store::{Client, Settings};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Longer than any bound a client may choose, shorter than forever.
const GUARD: Duration = Duration::from_secs(120);

async fn never_answers() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(24 * 3600)))
        .mount(&server)
        .await;
    server
}

/// OPS-*, T-1716: the app reconciler's API server calls are bounded.
#[tokio::test(start_paused = true)]
async fn a_hung_api_server_costs_the_reconciler_an_error_not_the_loop() {
    let api = never_answers().await;
    let kube = KubeClient::with_token(&api.uri(), "token").expect("a kube client");

    let outcome = tokio::time::timeout(GUARD, kube.get("v1", "Secret", "apps", "x")).await;

    let result = outcome.expect("the call waited past any sane bound: the client has no timeout");
    assert!(
        result.is_err(),
        "a hung server is not an answer: {result:?}"
    );
}

/// T-1716: provisioning an organization's credentials against a hung store is bounded.
#[tokio::test(start_paused = true)]
async fn a_hung_artifact_store_costs_provisioning_an_error_not_the_loop() {
    let store = never_answers().await;
    let client = Client::new(Settings {
        endpoint: store.uri(),
        bucket: "jc-artifacts".to_owned(),
        region: "us-east-1".to_owned(),
        root_access_key: "jc-root".to_owned(),
        root_secret_key: "root-secret-for-tests".to_owned(),
    })
    .expect("a store client");

    let outcome = tokio::time::timeout(GUARD, client.ensure_organization("hel")).await;

    let result = outcome.expect("the call waited past any sane bound: the client has no timeout");
    assert!(result.is_err(), "a hung store is not an answer");
}
