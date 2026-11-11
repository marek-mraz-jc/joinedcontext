//! Process-wide singletons `jc` installs exactly once: the tracing subscriber, the metrics recorder
//! and the rustls crypto provider. (The allocator is chosen in `main.rs`; the tokio runtime and the
//! shutdown token are created there and passed down.)
//!
//! Each installer is idempotent: a second call returns what the first installed instead of
//! panicking, so hosted modules and tests can call them freely.

use crate::config::LogFormat;
use metrics_exporter_prometheus::{BuildError, Matcher, PrometheusBuilder, PrometheusHandle};
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
/// at their level, a probe's lines thinned out: `access::quiet_probes`). Returns `false` when a subscriber was already installed (by an earlier call or
/// by the embedding test harness).
pub fn init_tracing(format: LogFormat) -> bool {
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = filter(tracing_subscriber::EnvFilter::try_from_default_env().ok());
    let fmt = tracing_subscriber::fmt::layer()
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr);
    let fmt = match format {
        LogFormat::Json => fmt.json().boxed(),
        LogFormat::Text => fmt.boxed(),
    };
    tracing_subscriber::registry()
        .with(
            fmt.with_filter(crate::access::quiet_probes())
                .with_filter(filter),
        )
        .try_init()
        .is_ok()
}

/// The default bucket edges of a duration, in seconds: the Portal's and the gateway's.
const SECONDS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// The recorder every module records into, with each module's buckets as its own server sets
/// them. Without buckets the exporter keeps each histogram as a summary: rolling sketches per
/// label set, 45-305 MiB of the prod rehearsal's heap, and no `_bucket` series for the
/// dashboards' `histogram_quantile` (jc-memory-peak).
fn builder() -> Result<PrometheusBuilder, BuildError> {
    let mut tables: Vec<(&str, &[f64])> = Vec::new();
    #[cfg(feature = "portal")]
    tables.extend_from_slice(joinedcontext_portal::telemetry::BUCKETS);
    #[cfg(feature = "antares")]
    tables.extend_from_slice(&[
        (
            "antares_answer_bytes",
            antares_broker::telemetry::ANSWER_BUCKETS_BYTES,
        ),
        (
            "antares_*",
            antares_broker::telemetry::LATENCY_BUCKETS_SECONDS,
        ),
    ]);
    tables.into_iter().try_fold(
        PrometheusBuilder::new().set_buckets(SECONDS)?,
        |builder, (name, buckets)| {
            let matcher = match name.strip_suffix('*') {
                Some(prefix) => Matcher::Prefix(prefix.to_owned()),
                None => Matcher::Full(name.to_owned()),
            };
            builder.set_buckets_for_metric(matcher, buckets)
        },
    )
}

/// Installs the one Prometheus recorder for the `metrics` facade every module records into, and
/// returns its handle. Later calls return the same handle. Fails only when some other recorder
/// was installed before `jc` got to it.
pub fn metrics_handle() -> Result<PrometheusHandle, String> {
    METRICS
        .get_or_init(|| {
            builder()
                .and_then(PrometheusBuilder::install_recorder)
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
    fn every_modules_histogram_renders_as_its_own_buckets_never_a_summary() {
        let recorder = builder().expect("the buckets").build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            for name in [
                "kaisatsu_http_request_duration_seconds",
                "antares_http_request_duration_seconds",
                "antares_answer_bytes",
                "jc_agent_answer_duration_seconds",
                "jc_gateway_request_duration_seconds",
            ] {
                metrics::histogram!(name).record(0.5);
            }
        });
        let text = handle.render();
        assert!(!text.contains("quantile="), "{text}");
        for (name, edge) in [
            ("kaisatsu_http_request_duration_seconds", "0.001"),
            ("antares_http_request_duration_seconds", "60"),
            ("antares_answer_bytes", "67108864"),
            ("jc_agent_answer_duration_seconds", "120"),
            ("jc_gateway_request_duration_seconds", "0.001"),
        ] {
            let bucket = format!("{name}_bucket{{le=\"{edge}\"}}");
            assert!(text.contains(&bucket), "{bucket} missing:\n{text}");
        }
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
