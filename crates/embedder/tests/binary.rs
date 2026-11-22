//! The `jc-embedder` binary as a deployment runs it: a config it cannot use stops it with a log
//! line naming what to fix, and with the real model (`JC_ASSISTANT_TEST_MODEL_DIR`,
//! `ORT_DYLIB_PATH`) it serves, exports metrics and stops cleanly on SIGTERM or SIGINT.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn model_dir() -> String {
    std::env::var("JC_ASSISTANT_TEST_MODEL_DIR").unwrap_or_else(|_| {
        panic!("set JC_ASSISTANT_TEST_MODEL_DIR to a directory filled by scripts/ci/e5-small.sh")
    })
}

/// A fresh directory for this test's config files.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jc-embedder-bin-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

fn config(dir: &Path, model: &str, listen: &str, metrics: Option<&str>) -> PathBuf {
    let metrics = metrics.map_or(String::new(), |m| format!("metrics_addr = \"{m}\"\n"));
    let path = dir.join("embedder.toml");
    std::fs::write(
        &path,
        format!(
            r#"listen_addr = "{listen}"
{metrics}model_path = "{model}"
threads = 1
[[deployments]]
name = "dev"
issuer = "https://id.dev.example/realms/dev"
jwks_url = "https://id.dev.example/realms/dev/certs"
clients = ["jc-assistant"]
max_texts_per_minute = 600
"#
        ),
    )
    .expect("write");
    path
}

fn embedder() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc-embedder"));
    cmd.env_remove("EMBEDDER_CONFIG")
        .env("RUST_LOG", "info")
        .stdin(Stdio::null());
    cmd
}

/// The run's exit and its log, which goes to stdout as JSON lines.
fn stopped(out: &Output) -> (bool, String) {
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// A loopback port nothing listens on, held bound (not listening) by the returned socket.
fn held_port() -> (std::net::TcpListener, String) {
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = held.local_addr().expect("addr").to_string();
    (held, addr)
}

/// A port free a moment ago, for a listener the binary binds itself.
fn free_port() -> String {
    held_port().1
}

#[test]
fn without_a_config_it_stops_and_says_how_to_name_one() {
    let (ok, log) = stopped(&embedder().output().expect("run"));
    assert!(!ok, "{log}");
    assert!(
        log.contains("name the config file: --config <file> or EMBEDDER_CONFIG"),
        "{log}"
    );
    assert!(log.contains("jc-embedder stopped"), "{log}");
    // `--config` with nothing after it names no file either.
    let (ok, log) = stopped(
        &embedder()
            .arg("--config")
            .env_remove("RUST_LOG")
            .output()
            .expect("run"),
    );
    assert!(!ok && log.contains("name the config file"), "{log}");
}

#[test]
fn a_config_it_cannot_read_or_a_model_it_cannot_load_stops_it() {
    let dir = scratch("refused");
    let (ok, log) = stopped(
        &embedder()
            .args(["--verbose", "--config"])
            .arg(dir.join("missing.toml"))
            .output()
            .expect("run"),
    );
    assert!(
        !ok && log.contains("missing.toml could not be read"),
        "{log}"
    );
    // EMBEDDER_CONFIG names the file when no --config does.
    let path = config(&dir, "/nonexistent/e5-small", "127.0.0.1:0", None);
    let (ok, log) = stopped(
        &embedder()
            .env("EMBEDDER_CONFIG", &path)
            .output()
            .expect("run"),
    );
    assert!(
        !ok && log.contains("tokenizer.json could not be read"),
        "{log}"
    );
    std::fs::remove_dir_all(dir).expect("clean");
}

#[test]
fn a_port_taken_by_another_listener_stops_it() {
    let model = model_dir();
    let dir = scratch("taken");
    let (_held, taken) = held_port();
    let path = config(&dir, &model, &taken, None);
    let (ok, log) = stopped(&embedder().arg("--config").arg(&path).output().expect("run"));
    assert!(
        !ok && log.contains(&format!("{taken} could not be bound")),
        "{log}"
    );
    let path = config(&dir, &model, &free_port(), Some(&taken));
    let (ok, log) = stopped(&embedder().arg("--config").arg(&path).output().expect("run"));
    assert!(
        !ok && log.contains(&format!("{taken} could not be bound")),
        "metrics: {log}"
    );
    std::fs::remove_dir_all(dir).expect("clean");
}

/// Plain HTTP/1.0 GET on loopback: the status line and the body, or None while nothing answers.
fn get(addr: &str, path: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    Some(answer)
}

fn serving(child: &mut Child, addr: &str) -> String {
    let began = Instant::now();
    loop {
        if let Some(answer) = get(addr, "/healthz") {
            return answer;
        }
        assert!(
            child.try_wait().expect("wait").is_none(),
            "the embedder stopped before it served"
        );
        assert!(began.elapsed() < Duration::from_secs(120), "never served");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn it_serves_exports_metrics_and_stops_cleanly_on_a_signal() {
    let model = model_dir();
    for signal in ["TERM", "INT"] {
        let dir = scratch(&format!("serve-{signal}"));
        let (listen, metrics) = (free_port(), free_port());
        let path = config(&dir, &model, &listen, Some(&metrics));
        let mut child = embedder()
            .arg("--config")
            .arg(&path)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn");
        let health = serving(&mut child, &listen);
        assert!(
            health.starts_with("HTTP/1.0 200") && health.ends_with("ok"),
            "{health}"
        );
        let exported = get(&metrics, "/metrics").expect("metrics");
        assert!(exported.starts_with("HTTP/1.0 200"), "{exported}");
        let killed = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(child.id().to_string())
            .status()
            .expect("kill");
        assert!(killed.success());
        let (ok, log) = stopped(&child.wait_with_output().expect("wait"));
        assert!(ok, "SIGTERM/SIGINT is a clean stop: {log}");
        assert!(
            log.contains("serving") && log.contains("multilingual-e5-small-int8"),
            "{log}"
        );
        std::fs::remove_dir_all(dir).expect("clean");
    }
}
