// SPDX-License-Identifier: EUPL-1.2
//! What the bus logs on a live server, alone in its binary with one process-wide subscriber
//! (tracing caches a callsite's interest, see nats_auth_warning.rs). Skips without
//! ANTARES_TEST_NATS_URL.

use antares_bus::nats::NatsBus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("lock").clone()).expect("utf-8")
    }
}

/// A topology the server confirms is logged with its durables; an answer too large for the
/// server is logged as a failed reply and the asker hears nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_topology_and_a_failed_reply_are_logged() {
    let Ok(url) = std::env::var("ANTARES_TEST_NATS_URL") else {
        eprintln!("SKIP: ANTARES_TEST_NATS_URL not set");
        return;
    };
    let buf = Buf::default();
    let writer = buf.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .expect("the only subscriber of this binary");
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let durable = format!("logged-{n}");
    bus.consume_balanced(&durable).await.expect("durable");
    bus.assert_topology(&[durable.as_str()])
        .await
        .expect("present");
    let logged = buf.text();
    assert!(
        logged.contains("bus topology asserted") && logged.contains(&durable),
        "{logged}"
    );

    let subject = format!("antares.mcp.logged{n}");
    // over the server's default 1 MiB max_payload: the client refuses to send it
    let task = bus
        .reply_ready(subject.clone(), |_| Some(vec![b'x'; 2 * 1024 * 1024]))
        .await
        .expect("ready");
    let heard = bus
        .request(subject.clone(), b"big".to_vec(), Duration::from_millis(500))
        .await;
    assert_eq!(heard, None, "no answer reaches the asker");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !buf.text().contains("reply on") && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let logged = buf.text();
    assert!(
        logged.contains("WARN") && logged.contains(&format!("reply on {subject} failed")),
        "{logged}"
    );
    task.abort();
}
