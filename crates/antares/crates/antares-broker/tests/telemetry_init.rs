// SPDX-License-Identifier: EUPL-1.2
//! The standalone telemetry stack in one process, in the order a broker meets
//! it (T-7829): an OTLP endpoint without the switch is said, not exported; a
//! refused endpoint fails the start without its password; with the switch on
//! the exporters and the recorder install once, a second init keeps them, and
//! the gauge sampler feeds the recorder from whatever state it is given.
//! Alone in its binary: the subscriber, the recorder and the environment are
//! process-wide.

use std::time::Duration;

#[derive(Clone, Default)]
struct Buf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_stack_installs_once_and_the_sampler_feeds_it() {
    use antares_broker::telemetry;
    // as `main_entry` does: the OTLP exporter builds the process's first client
    antares_jsonld::install_crypto_provider();
    // The switch off with an endpoint set: nothing exported, and the
    // operator is told. The init installs the process's subscriber.
    std::env::remove_var("ANTARES_TELEMETRY");
    std::env::set_var(
        "ANTARES_OTLP_ENDPOINT",
        "http://otel:t7829pw@127.0.0.1:1/v1/traces",
    );
    let buf = Buf::default();
    let writer = buf.clone();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    assert!(telemetry::init().expect("off").is_none());
    let said = String::from_utf8_lossy(&buf.0.lock().expect("lock")).into_owned();
    assert!(
        said.contains("ANTARES_OTLP_ENDPOINT is set but ANTARES_TELEMETRY is off"),
        "{said}"
    );
    assert!(
        !said.contains("t7829pw"),
        "the endpoint's password logged: {said}"
    );

    // A refused endpoint stops the start; its password is not in the error.
    std::env::set_var("ANTARES_TELEMETRY", "1");
    std::env::set_var(
        "ANTARES_OTLP_ENDPOINT",
        "http://otel:t7829pw@collector .internal:4318/v1/traces",
    );
    let err = telemetry::init()
        .err()
        .expect("a URI with a space is refused");
    assert!(!err.to_string().contains("t7829pw"), "{err}");
    assert!(err.to_string().contains("collector .internal"), "{err}");

    // On: the exporters build, the recorder installs and renders.
    std::env::set_var("ANTARES_OTLP_ENDPOINT", "http://127.0.0.1:1/v1/traces");
    let render = telemetry::init()
        .expect("on")
        .expect("the first recorder renders /q/metrics");
    assert!(
        String::from_utf8_lossy(&buf.0.lock().expect("lock"))
            .contains("OTLP span and log export enabled"),
        "the export is announced"
    );
    // A second init in the same process keeps the first recorder.
    assert!(telemetry::init().expect("again").is_none());

    // The sampler: a state without heap stats, and one whose stats lack a
    // field, over a store that reports its commit queue.
    let bare = antares_api::AppState::new("antares".into());
    telemetry::spawn_sampler(bare);
    let dir = std::env::temp_dir().join(format!("antares-sampler-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut file = antares_api::AppState::with_store(
        "antares".into(),
        std::sync::Arc::new(antares_sql::store::any::AnyStore::Mem(
            antares_sql::store::Store::open_file(&dir).expect("file store"),
        )),
        "file",
    );
    file.mem_stats = Some(std::sync::Arc::new(
        || serde_json::json!({"allocatedBytes": 7829}),
    ));
    telemetry::spawn_sampler(file);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let text = render();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(text.contains("antares_uptime_seconds"), "{text}");
    assert!(
        text.contains("antares_memory_allocated_bytes 7829"),
        "{text}"
    );
    assert!(!text.contains("antares_memory_resident_bytes"), "{text}");
    assert!(text.contains("antares_commit_queue_depth"), "{text}");
    assert!(text.contains("antares_limit_rejections_total"), "{text}");
}
