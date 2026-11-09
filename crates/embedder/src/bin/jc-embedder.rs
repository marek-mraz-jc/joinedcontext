//! `jc-embedder`: the embedding model as a service several deployments share
//! (`jc_embedder::server`). Configuration: one TOML file, `--config <file>` or `EMBEDDER_CONFIG`;
//! ONNX Runtime from `ORT_DYLIB_PATH`. Logs JSON lines without a text, a token or a claim.

use std::process::ExitCode;
use std::sync::Arc;

use jc_embedder::Embedder;
use jc_embedder::server::{Config, Service, router};

fn config_path() -> Option<std::path::PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--config" {
            return args.next().map(Into::into);
        }
    }
    std::env::var_os("EMBEDDER_CONFIG").map(Into::into)
}

async fn shutdown() {
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = term => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

async fn run() -> Result<(), String> {
    let path = config_path().ok_or("name the config file: --config <file> or EMBEDDER_CONFIG")?;
    let cfg = Config::read(&path)?;
    let embedder = Embedder::load(&cfg.model_path, cfg.threads).map_err(|e| e.to_string())?;
    let service = Arc::new(Service::new(embedder, &cfg)?);
    if let Some(addr) = cfg.metrics_addr {
        let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
            .install_recorder()
            .map_err(|e| format!("the metrics recorder did not install: {e}"))?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("{addr} could not be bound: {e}"))?;
        let app = axum::Router::new().route(
            "/metrics",
            axum::routing::get(move || {
                let handle = handle.clone();
                async move { handle.render() }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await });
    }
    let listener = tokio::net::TcpListener::bind(cfg.listen_addr)
        .await
        .map_err(|e| format!("{} could not be bound: {e}", cfg.listen_addr))?;
    tracing::info!(
        listen = %cfg.listen_addr,
        model = jc_embedder::MODEL,
        sha256 = jc_embedder::MODEL_SHA256,
        deployments = cfg.deployments.len(),
        "serving"
    );
    axum::serve(listener, router(service))
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|e| format!("the listener failed: {e}"))
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "jc-embedder stopped");
            ExitCode::FAILURE
        }
    }
}
