//! The app process: read the variables the reconciler sets, bind, follow the socket, serve.
//!
//! In the pod the reconciler sets `JC_BIND_ADDRESS` to `0.0.0.0:8080`, the port the APISIX edge
//! upstreams to and the NetworkPolicy opens to nobody else (AP-26). The loopback default is for
//! a laptop. On a public app the edge passes anonymous requests straight through, without
//! `X-Access-Token` (AP-28).

use std::sync::Arc;

use hsl_transport::{router, App, Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let bind = std::env::var("JC_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    tracing::info!(%bind, base = %config.base_path, "hsl-transport starting");

    let app = Arc::new(App::new(config));
    // One socket for the whole process, however many browsers are watching.
    tokio::spawn(Arc::clone(&app).follow());
    tokio::spawn(Arc::clone(&app).read_board());

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
