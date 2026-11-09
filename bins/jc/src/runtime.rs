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

/// Libraries whose INFO lines tell an operator nothing, each at the level `jc` logs it at unless
/// `RUST_LOG` names it: rmcp writes three lifecycle lines per MCP session ("Service initialized",
/// "input stream terminated", "serve finished"), 18k of the broker's 115k lines in the 2026-11-09
/// gate; its refusals and failures are WARN and ERROR.
const QUIET: [(&str, &str); 1] = [("rmcp", "warn")];

/// `given` (the `RUST_LOG` filter; `info` when unset or unreadable), with [`QUIET`] for every
/// library it does not name.
fn filter(given: Option<tracing_subscriber::EnvFilter>) -> tracing_subscriber::EnvFilter {
    let mut filter = given.unwrap_or_else(|| tracing_subscriber::EnvFilter::new("info"));
    let named = filter.to_string();
    for (target, level) in QUIET {
        let mentioned = named
            .split(',')
            .any(|d| d.trim().split(['=', '[']).next() == Some(target));
        if !mentioned && let Ok(directive) = format!("{target}={level}").parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter
}

/// Installs the one tracing subscriber (`RUST_LOG` filter, `info` by default, [`QUIET`] libraries
/// at their level). Returns `false` when a subscriber was already installed (by an earlier call or
/// by the embedding test harness).
pub fn init_tracing(format: LogFormat) -> bool {
    let filter = filter(tracing_subscriber::EnvFilter::try_from_default_env().ok());
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
    fn rmcp_lifecycle_lines_stay_out_unless_rust_log_names_rmcp() {
        let of = |v: &str| filter(tracing_subscriber::EnvFilter::try_new(v).ok()).to_string();
        for given in ["info", "info,sqlx=warn", "not a [filter"] {
            assert!(of(given).contains("rmcp=warn"), "{given}: {}", of(given));
        }
        assert!(filter(None).to_string().contains("rmcp=warn"));
        let named = of("info,rmcp=debug");
        assert!(
            named.contains("rmcp=debug") && !named.contains("rmcp=warn"),
            "{named}"
        );
        // A library whose name only starts the same is no mention.
        assert!(of("rmcp_x=debug").contains("rmcp=warn"));
    }

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
