//! The `jc-functions` binary as a deployment starts it: each required variable missing or a
//! gateway or proxy address with a path refuses the start by name, and a complete environment
//! serves `/healthz` until an interrupt stops it cleanly (SDK-21, OPS-27).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_jc-functions");

/// Every variable the binary reads, set to values that start it; `bind` is the listen address.
fn complete(bind: &str) -> Vec<(&'static str, String)> {
    vec![
        ("JC_FUNCTIONS_BIND", bind.to_owned()),
        (
            "JC_OIDC_ISSUER",
            "https://portal.example/realms/joinedcontext".to_owned(),
        ),
        // Nothing listens on port 9: the key refresh fails in the background and is logged.
        ("JC_OIDC_JWKS_URL", "http://127.0.0.1:9/certs".to_owned()),
        ("JC_FUNCTIONS_CALLER", "joinedcontext-portal".to_owned()),
        ("JC_GATEWAY_URL", "https://gateway.example/".to_owned()),
        (
            "JC_AGENT_PROXY_URL",
            "https://agent-proxy.example/".to_owned(),
        ),
        ("JC_FUNCTIONS_ASSISTANT_CALLER", "jc-assistant".to_owned()),
    ]
}

/// The binary with `vars` and no other `JC_` variable of the calling environment.
fn command(vars: &[(&str, String)]) -> Command {
    let mut cmd = Command::new(BIN);
    for (name, _) in std::env::vars() {
        if name.starts_with("JC_") {
            cmd.env_remove(name);
        }
    }
    cmd.envs(vars.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::null());
    cmd
}

/// Runs the binary to its end, checks it failed and answers its stderr.
fn refused(vars: &[(&str, String)]) -> String {
    let out = command(vars).output().expect("jc-functions runs");
    assert!(!out.status.success(), "started with {vars:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn each_missing_or_empty_required_variable_refuses_the_start_by_name() {
    for name in [
        "JC_OIDC_ISSUER",
        "JC_OIDC_JWKS_URL",
        "JC_FUNCTIONS_CALLER",
        "JC_GATEWAY_URL",
    ] {
        let mut vars = complete("127.0.0.1:0");
        vars.retain(|(k, _)| *k != name);
        assert!(
            refused(&vars).contains(&format!("{name} is not set")),
            "{name} absent"
        );
        vars.push((name, String::new()));
        assert!(
            refused(&vars).contains(&format!("{name} is not set")),
            "{name} empty"
        );
    }
}

#[test]
fn a_gateway_or_proxy_address_with_a_path_or_query_or_no_url_refuses_the_start() {
    for (name, value, says) in [
        (
            "JC_GATEWAY_URL",
            "https://gateway.example/ngsi-ld",
            "JC_GATEWAY_URL must be scheme and authority only",
        ),
        (
            "JC_GATEWAY_URL",
            "https://gateway.example?x=1",
            "JC_GATEWAY_URL must be scheme and authority only",
        ),
        (
            "JC_GATEWAY_URL",
            "gateway without scheme",
            "RelativeUrlWithoutBase",
        ),
        (
            "JC_AGENT_PROXY_URL",
            "https://agent-proxy.example/call",
            "JC_AGENT_PROXY_URL must be scheme and authority only",
        ),
        (
            "JC_AGENT_PROXY_URL",
            "https://agent-proxy.example/?q",
            "JC_AGENT_PROXY_URL must be scheme and authority only",
        ),
        ("JC_AGENT_PROXY_URL", "::", "RelativeUrlWithoutBase"),
    ] {
        let mut vars = complete("127.0.0.1:0");
        vars.retain(|(k, _)| *k != name);
        vars.push((name, value.to_owned()));
        let stderr = refused(&vars);
        assert!(stderr.contains(says), "{name}={value}: {stderr}");
    }
}

#[test]
fn an_address_already_taken_refuses_the_start() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stderr = refused(&complete(&taken.local_addr().unwrap().to_string()));
    assert!(stderr.contains("in use"), "{stderr}");
}

#[test]
fn a_complete_environment_serves_healthz_and_an_interrupt_stops_it_cleanly() {
    let addr = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    // No agent proxy (optional) and no assistant caller: the runtime starts without either.
    let mut vars = complete(&addr.to_string());
    vars.retain(|(k, _)| !matches!(*k, "JC_AGENT_PROXY_URL" | "JC_FUNCTIONS_ASSISTANT_CALLER"));
    vars.push(("JC_FUNCTIONS_ASSISTANT_CALLER", "  ".to_owned()));
    let mut child = command(&vars)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("jc-functions starts");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        if let Ok(stream) = std::net::TcpStream::connect(addr) {
            break stream;
        }
        assert!(
            Instant::now() < deadline,
            "jc-functions did not listen on {addr}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    use std::io::{Read, Write};
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("ok"), "{answer}");
    let interrupt = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(interrupt.success());
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "jc-functions kept running after SIGINT"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status}");
}
