//! The Context Gateway binary: one listener, one broker, one endpoint table (T-0005).

use context_gateway::config::Config;
use std::process::ExitCode;

/// jemalloc, not glibc's malloc (T-4782): after a 100 req/s load glibc kept 928 Mi of the 1 Gi
/// limit, the freed blocks of the broker answers spread over its heap. The image's
/// `_RJEM_MALLOC_CONF` starts jemalloc's background thread, which gives them back. Behind the
/// `jemalloc` feature, which the image build turns on (T-5017).
#[cfg(feature = "jemalloc")]
#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main]
async fn main() -> ExitCode {
    // OPS-15: JSON lines on stdout, nothing on disk.
    if let Err(error) = jc_core::logs::init("context_gateway=info,tower_http=info") {
        eprintln!("logging is not set up: {error}");
        return ExitCode::FAILURE;
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "the gateway is not configured");
            return ExitCode::FAILURE;
        }
    };

    let broker = match context_gateway::serve::broker_of(&config) {
        Ok(broker) => broker,
        Err(error) => {
            // An unusable trust bundle is a deployment fault, and starting without it would
            // talk to a broker nobody verified (R46).
            tracing::error!(%error, "cannot build the outbound client");
            return ExitCode::FAILURE;
        }
    };
    let built = match context_gateway::serve::build(&config, broker).await {
        Ok(built) => built,
        Err(error) => {
            tracing::error!(%error, "the gateway does not start");
            return ExitCode::FAILURE;
        }
    };

    // The MQTT brokers' CONNECT checks answer on a listener of their own, which APISIX does not
    // route to and the NetworkPolicy opens to broker pods only (ADR-N-048, PL-75).
    if let (Some(internal), Some(app)) = (config.internal_bind, built.internal) {
        let listener = match tokio::net::TcpListener::bind(internal).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!(%error, bind = %internal, "cannot listen internally");
                return ExitCode::FAILURE;
            }
        };
        tracing::info!(bind = %internal, "internal listener for the MQTT brokers");
        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                tracing::error!(%error, "the internal listener stopped");
            }
        });
    }

    let app = built.app;
    let listener = match tokio::net::TcpListener::bind(config.bind).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, bind = %config.bind, "cannot listen");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        bind = %config.bind,
        broker = %config.broker_url,
        allocator = if cfg!(feature = "jemalloc") { "jemalloc" } else { "system" },
        "context-gateway listening"
    );

    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
    {
        tracing::error!(%error, "the server stopped");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Stops on SIGTERM, which is how Kubernetes asks (OPS-12).
async fn shutdown() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("a process can always listen for an interrupt");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("a process can always listen for SIGTERM")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}
