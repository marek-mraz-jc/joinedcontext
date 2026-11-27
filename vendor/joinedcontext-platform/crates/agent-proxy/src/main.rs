//! Main binary entry point for `jc-agent-proxy`.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // OPS-15: JSON lines on stdout, nothing on disk.
    jc_core::logs::init("info")?;
    agent_proxy::run(cfg!(feature = "test-stub"), agent_proxy::interrupted()).await
}
