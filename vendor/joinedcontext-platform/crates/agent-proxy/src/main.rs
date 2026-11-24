//! Main binary entry point for `jc-agent-proxy`.

use agent_proxy::config::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // OPS-15: JSON lines on stdout, nothing on disk.
    jc_core::logs::init("info")?;

    // A binary compiled with the suites' stub token would answer an empty client secret with a
    // made-up token; it is never a proxy anyone may run (T-1480).
    if cfg!(feature = "test-stub") {
        return Err(
            "jc-agent-proxy was built with the test-stub feature and does not start".into(),
        );
    }
    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    agent_proxy::serve(config, listener, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}
