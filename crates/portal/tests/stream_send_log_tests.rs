//! T-3007: a PUT restarts a stream on the runner and fires its clock out of schedule, and a
//! Portal roll re-fired streams with nothing in the log to say which request did it. Every send
//! to the runner names its reason, and a pass that sends nothing says nothing.
//!
//! One test in its own binary: the log is captured by a subscriber on this thread, and a callsite
//! another test hit first without one would stay silent here.

use std::sync::{Arc, Mutex};

use joinedcontext_portal::reconciler::streams::{Bentos, StreamDeployer, StreamOutcome};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    /// What was logged since the last call.
    fn take(&self) -> String {
        let mut bytes = self.0.lock().expect("log");
        let text = String::from_utf8_lossy(&bytes).into_owned();
        bytes.clear();
        text
    }
}

fn manifest(kind: &str, name: &str, spec: serde_json::Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta {
            name: name.to_owned(),
            namespace: Some("helsinki".to_owned()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

/// One socket DataSource (resident, PL-78), the endpoint it writes into and the pipeline between
/// them.
fn mirror(open_message: &str) -> Mirror {
    let mirror = Mirror::new();
    mirror.upsert(manifest(
        "DataSource",
        "citybikes",
        json!({ "type": "websocket", "webSocket": { "url": "wss://example.org/status", "openMessage": open_message } }),
    ));
    mirror.upsert(manifest(
        "Endpoint",
        "helsinki-all",
        json!({
            "contextSpaceRef": "helsinki",
            "slug": "abc123456789012345678901234",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }),
    ));
    mirror.upsert(manifest(
        "Pipeline",
        "citybikes-free",
        json!({
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "citybikes" } },
            "compute": { "kind": "bloblang", "bloblang": "root = this.data.bikes" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all"
        }),
    ));
    mirror
}

#[tokio::test]
async fn every_send_to_the_runner_is_logged_with_its_reason() {
    let log = Log::default();
    let writer = log.clone();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );

    let runner = MockServer::start().await;
    // Each project's write budget, declared before its streams (T-5803).
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex("/resources/rate_limit/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&runner)
        .await;
    Mock::given(method("PUT"))
        .and(path("/streams/helsinki.citybikes-free"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&runner)
        .await;
    Mock::given(method("GET"))
        .and(path("/streams"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "helsinki.citybikes-free": { "active": true } })),
        )
        .mount(&runner)
        .await;
    let deployer = StreamDeployer::new(runner.uri());
    let (bentos, refused) = (Bentos::new(), Default::default());

    // A Portal that has not sent the stream, to a runner that does not run it as rendered.
    let first = mirror("subscribe");
    let outcomes = deployer.converge(&first, &bentos, &refused).await;
    assert_eq!(outcomes[0].2, StreamOutcome::Live);
    let said = log.take();
    assert!(
        said.contains("sending the stream to the runner")
            && said.contains("this Portal has not sent it")
            && said.contains("citybikes-free"),
        "{said}"
    );

    // The same render, which the runner holds and runs: nothing is sent, nothing is said.
    deployer.converge(&first, &bentos, &refused).await;
    let said = log.take();
    assert!(!said.contains("sending the stream"), "{said}");

    // A changed opening message is a changed render.
    deployer
        .converge(&mirror("subscribe all"), &bentos, &refused)
        .await;
    let said = log.take();
    assert!(said.contains("its render changed"), "{said}");
}
