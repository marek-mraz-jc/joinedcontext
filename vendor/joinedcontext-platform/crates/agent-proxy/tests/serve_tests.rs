//! The proxy as the binary runs it (AG-35, T-1480, T-7795): `serve` refuses a configuration
//! without its credentials, answers on the listener it is given until its shutdown signal, and
//! the binary these suites build, which carries the stub token, refuses to start at all.

use agent_proxy::config::Config;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn config(pairs: &[(&'static str, &'static str)]) -> Config {
    let pairs = pairs.to_vec();
    Config::from_lookup(move |key| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
    })
    .expect("a configuration")
}

/// A configuration without the forge token, the model key or a client credential does not
/// serve; the listener it was given is never answered on.
#[tokio::test]
async fn a_configuration_without_its_credentials_does_not_serve() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let refused = agent_proxy::serve(config(&[]), listener, std::future::pending()).await;
    let error = refused.expect_err("no credentials, no proxy");
    assert!(
        error.to_string().contains("missing environment variable"),
        "{error}"
    );
}

/// A complete configuration answers `/healthz` until the shutdown signal, then returns.
#[tokio::test]
async fn a_complete_configuration_serves_until_it_is_told_to_stop() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let addr = listener.local_addr().expect("an address");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(agent_proxy::serve(
        config(&[
            ("JC_OIDC_CLIENT_SECRET", "s"),
            ("JC_FORGE_TOKEN", "f"),
            ("JC_MODEL_KEY", "k"),
            ("JC_PORTAL_BASE", "http://portal.invalid:8080"),
        ]),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let mut socket = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connected");
    socket
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: proxy\r\nConnection: close\r\n\r\n")
        .await
        .expect("sent");
    let mut answer = String::new();
    socket.read_to_string(&mut answer).await.expect("read");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("ok"), "{answer}");
    stop.send(()).expect("the proxy is listening");
    let result = served.await.expect("joined");
    assert!(result.is_ok(), "{:?}", result.err().map(|e| e.to_string()));
}

/// The binary built for these suites carries the stub token, so it refuses to start and says
/// why (T-1480): no proxy anyone runs can answer an empty client secret with a made-up token.
#[test]
fn the_binary_built_with_the_stub_refuses_to_start() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_jc-agent-proxy"))
        .env_clear()
        .output()
        .expect("the binary runs");
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("built with the test-stub feature and does not start"),
        "{said}"
    );
}
