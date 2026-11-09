//! Process-wide singletons `jc` installs exactly once: the tracing subscriber, the metrics recorder
//! and the rustls crypto provider. (The allocator is chosen in `main.rs`; the tokio runtime and the
//! shutdown token are created there and passed down.)
//!
//! Each installer is idempotent: a second call returns what the first installed instead of
//! panicking, so hosted modules and tests can call them freely.

use crate::config::LogFormat;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::io::IsTerminal;
use std::sync::OnceLock;

static METRICS: OnceLock<Result<PrometheusHandle, String>> = OnceLock::new();

/// Installs the one tracing subscriber (`RUST_LOG` filter, `info` by default). Returns `false` when
/// a subscriber was already installed (by an earlier call or by the embedding test harness).
pub fn init_tracing(format: LogFormat) -> bool {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Json => builder.json().try_init().is_ok(),
        LogFormat::Text => builder.try_init().is_ok(),
    }
}

/// Installs the one Prometheus recorder for the `metrics` facade every module records into, and
/// returns its handle. Later calls return the same handle. Fails only when some other recorder
/// was installed before `jc` got to it.
pub fn metrics_handle() -> Result<PrometheusHandle, String> {
    METRICS
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .map_err(|e| format!("metrics recorder: {e}"))
        })
        .clone()
}

/// Installs ring as the process-wide rustls crypto provider unless one is already installed.
/// Returns `true` when this call installed it.
pub fn install_crypto_provider() -> bool {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return false;
    }
    rustls::crypto::ring::default_provider()
        .install_default()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installers_are_idempotent() {
        // The first call in this test process may or may not win (other tests share it); the
        // second never panics and never installs again.
        let _ = install_crypto_provider();
        assert!(!install_crypto_provider());
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());

        let _ = init_tracing(LogFormat::Text);
        assert!(!init_tracing(LogFormat::Json));
        assert!(!init_tracing(LogFormat::Text));

        let a = metrics_handle().expect("recorder");
        metrics::counter!("jc_runtime_test_total").increment(1);
        let b = metrics_handle().expect("same recorder");
        assert!(b.render().contains("jc_runtime_test_total"));
        assert_eq!(a.render(), b.render());
    }
}
