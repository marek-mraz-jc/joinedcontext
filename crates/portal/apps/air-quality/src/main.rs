//! The app process: read the four variables the reconciler sets, bind, serve.
//!
//! In the pod the reconciler sets `JC_BIND_ADDRESS` to `0.0.0.0:8080`, the port the APISIX edge
//! upstreams to and the NetworkPolicy opens to nobody else (AP-26). The loopback default is for
//! a laptop, where nothing stands in front of the process.

use std::sync::Arc;

use air_quality::{router, App, Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let bind = std::env::var("JC_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    tracing::info!(%bind, base = %config.base_path, "air-quality starting");

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let app = router(Arc::new(App::new(config)));
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
