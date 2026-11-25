//! `server::serve` runs until the process is asked to stop, and stops cleanly on SIGTERM and on
//! SIGINT. One test binary of its own: the signals go to this process, and each is held by a
//! handler of the test before it is sent, so no sibling test can be taken down by it.

use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use tokio::signal::unix::{signal, SignalKind};

async fn serve_until(name: &str) {
    // Installed before the signal goes out: from here on neither signal ends the process.
    let _held_term = signal(SignalKind::terminate()).expect("a SIGTERM handler");
    let _held_int = signal(SignalKind::interrupt()).expect("a SIGINT handler");
    let serving = tokio::spawn(server::serve(Config::for_tests()));
    // The loop sends until the server has stopped, so a signal that came before the server
    // listened for it is sent again.
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if serving.is_finished() {
            break;
        }
        let sent = std::process::Command::new("kill")
            .arg(format!("-{name}"))
            .arg(std::process::id().to_string())
            .status()
            .expect("kill runs");
        assert!(sent.success());
    }
    assert!(serving.is_finished(), "the server stops on {name}");
    serving.await.expect("no panic").expect("a clean stop");
}

#[tokio::test]
async fn the_server_stops_cleanly_on_sigterm_and_then_on_sigint() {
    serve_until("TERM").await;
    serve_until("INT").await;
}
