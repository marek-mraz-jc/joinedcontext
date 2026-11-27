//! The binary's shutdown signal (coverage T-7896): `interrupted` resolves on SIGINT.

#[tokio::test]
async fn interrupted_resolves_when_the_process_is_sent_sigint() {
    let waiting = tokio::spawn(agent_proxy::interrupted());
    // The handler is installed when the future is first polled; give the task its turn.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let status = std::process::Command::new("kill")
        .args(["-INT", &std::process::id().to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success());
    tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .expect("it resolves on SIGINT")
        .expect("the task ends");
}
