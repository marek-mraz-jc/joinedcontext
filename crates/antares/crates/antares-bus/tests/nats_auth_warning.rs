// SPDX-License-Identifier: EUPL-1.2
//! The unauthenticated-server warning, alone in its binary: tracing caches a callsite's interest
//! process-wide, and a test beside others that log on threads without a subscriber can miss the
//! line its own subscriber should see. Skips without ANTARES_TEST_NATS_URL.

use antares_bus::nats::NatsBus;

/// The CI server asks for no authentication: connecting warns that the change stream is open.
#[tokio::test]
async fn a_server_without_authentication_is_warned_about() {
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
    let Ok(url) = std::env::var("ANTARES_TEST_NATS_URL") else {
        eprintln!("SKIP: ANTARES_TEST_NATS_URL not set");
        return;
    };
    let buf = Buf::default();
    let writer = buf.clone();
    let log = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _log = tracing::subscriber::set_default(log);
    NatsBus::connect(&url).await.expect("connect");
    let logged = String::from_utf8(buf.0.lock().expect("lock").clone()).expect("utf-8");
    assert!(
        logged.contains("WARN") && logged.contains("requires NO authentication"),
        "{logged}"
    );
}
