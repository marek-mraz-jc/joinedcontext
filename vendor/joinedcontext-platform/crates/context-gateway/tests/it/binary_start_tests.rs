//! T-7767: the `context-gateway` binary refuses to start, with a failing exit, on every fault
//! of its deployment, and once it listens it stops cleanly on SIGTERM.

use std::net::TcpListener;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The binary with only `vars` set (and the coverage tool's own variables, so a measured run
/// keeps its profile).
fn gateway(vars: &[(&str, String)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_context-gateway"));
    command
        .env_clear()
        .envs(std::env::vars().filter(|(key, _)| key.starts_with("LLVM_")))
        .envs(vars.iter().map(|(key, value)| (*key, value.as_str())))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("a free port")
}

fn base(bind: u16, metrics: u16) -> Vec<(&'static str, String)> {
    vec![
        ("JC_GATEWAY_BROKER_URL", "http://127.0.0.1:9".to_owned()),
        ("JC_GATEWAY_ORG_DOMAIN", "hel.fi".to_owned()),
        ("JC_GATEWAY_BIND", format!("127.0.0.1:{bind}")),
        ("JC_GATEWAY_METRICS_BIND", format!("127.0.0.1:{metrics}")),
    ]
}

fn exits(vars: &[(&str, String)]) -> ExitStatus {
    gateway(vars).status().expect("the binary runs")
}

#[test]
fn every_deployment_fault_is_a_failing_exit() {
    assert!(!exits(&[]).success(), "no broker named");
    let mut bundle = base(free_port(), free_port());
    bundle.push((
        "JC_GATEWAY_EGRESS_CA_BUNDLE",
        "/nonexistent/ca.pem".to_owned(),
    ));
    assert!(!exits(&bundle).success(), "no trust bundle");
    let mut repo = base(free_port(), free_port());
    repo.push(("JC_GATEWAY_REPO_DIR", "/nonexistent/gw-repo".to_owned()));
    assert!(!exits(&repo).success(), "no repository");

    let held = TcpListener::bind("127.0.0.1:0").expect("a port");
    let taken = held.local_addr().expect("an address").port();
    assert!(
        !exits(&base(taken, free_port())).success(),
        "the public port is taken"
    );
    assert!(
        !exits(&base(free_port(), taken)).success(),
        "the scrape port is taken"
    );
    let mut internal = base(free_port(), free_port());
    internal.push(("JC_GATEWAY_INTERNAL_BIND", format!("127.0.0.1:{taken}")));
    assert!(!exits(&internal).success(), "the internal port is taken");
}

#[test]
fn a_started_gateway_stops_cleanly_on_sigterm() {
    stops_cleanly_on("-TERM");
}

#[test]
fn a_started_gateway_stops_cleanly_on_an_interrupt() {
    stops_cleanly_on("-INT");
}

fn stops_cleanly_on(signal: &str) {
    let (bind, metrics, internal) = (free_port(), free_port(), free_port());
    let mut vars = base(bind, metrics);
    vars.push(("JC_GATEWAY_INTERNAL_BIND", format!("127.0.0.1:{internal}")));
    let mut child = gateway(&vars).spawn().expect("the binary starts");
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", bind)).is_err() {
        assert!(Instant::now() < deadline, "the gateway never listened");
        assert!(
            child.try_wait().ok().flatten().is_none(),
            "the gateway exited"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let stopped = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(stopped.success());
    assert!(child.wait().expect("the gateway exits").success());
}
