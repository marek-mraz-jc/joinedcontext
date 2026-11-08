#![allow(clippy::disallowed_methods)] // These tests start the jc binary and read their database URL.
//! The `jc` binary itself: configuration from a TOML file and `JCBIN_*`, a refused configuration
//! exits 2, a started `jc` serves and stops cleanly on SIGTERM.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

fn jc() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
    // Never inherit a JCBIN_* of the developer's shell.
    for (key, _) in std::env::vars() {
        if key.starts_with("JCBIN_") {
            cmd.env_remove(key);
        }
    }
    // Generated secrets go to a private directory of this test process, never the checkout.
    cmd.env(
        "JCBIN_STATE_DIR",
        std::env::temp_dir().join(format!("jc-binary-test-{}", std::process::id())),
    );
    cmd
}

#[test]
fn a_refused_configuration_exits_2_and_says_why() {
    let out = jc().args(["--role", "keycloak"]).output().expect("run jc");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown module 'keycloak'"), "{stderr}");

    let out = jc()
        .env("JCBIN_ROLE", "kaisatsu")
        .env("JCBIN_SCHEME", "ftp")
        .output()
        .expect("run jc");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("scheme must be http or https"));
}

#[test]
fn a_start_failure_exits_1() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let out = jc()
        .args(["--role", "daiya", "--listen"])
        .arg(taken.local_addr().unwrap().to_string())
        .output()
        .expect("run jc");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot listen"));
}

/// Waits for the "jc listening" log line and returns the address it names.
fn listening_addr(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> String {
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some(rest) = line.split("addr=").nth(1)
            && line.contains("jc listening")
        {
            return rest.split_whitespace().next().unwrap().to_owned();
        }
    }
    panic!("jc never logged its address");
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn sigterm_and_wait(child: &mut std::process::Child) {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "jc exited with {status}");
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "jc did not stop"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A secret in jc's initial environment (as a Kubernetes Secret env would put it) is not
/// readable by an ekiben stream through a `file` input of `/proc/self/environ`: `file` components
/// are confined to the ekiben file root.
#[cfg(unix)]
#[test]
fn an_ekiben_file_input_cannot_read_the_process_environment() {
    const SECRET: &str = "portal-oidc-client-secret-51c2";
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "tok\n").unwrap();
    let root = dir.path().join("files");
    let mut child = jc()
        .args([
            "--role",
            "ekiben",
            "--listen",
            "127.0.0.1:0",
            "--log-format",
            "text",
        ])
        .arg("--ekiben-token-file")
        .arg(&token)
        .arg("--ekiben-file-root")
        .arg(&root)
        .env("JC_PORTAL_TEST_OIDC_SECRET", SECRET)
        .env("RUST_LOG", "info")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn jc");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let addr = listening_addr(&mut lines);
    std::thread::spawn(move || for _ in lines {});
    // The output names an absolute path inside the root: without the sandbox the bytes land
    // there directly, with it they land re-rooted below it; the scan below covers both.
    let out_path = root.join("environ.out");
    let body = format!(
        "input:\n  file:\n    paths: [ /proc/self/environ ]\n    codec: all-bytes\noutput:\n  file:\n    path: {}\n    codec: all-bytes\n",
        out_path.display()
    );
    let answer = block_on(async {
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/streams/streams/environ?run=once"))
            .bearer_auth("tok")
            .body(body)
            .send()
            .await
            .unwrap();
        resp.text().await.unwrap()
    });
    assert!(!answer.contains(SECRET), "{answer}");
    let mut stack = vec![root.clone()];
    let mut files = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files += 1;
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !String::from_utf8_lossy(&bytes).contains(SECRET),
                    "a file input read the process environment into {}: {answer}",
                    path.display()
                );
            }
        }
    }
    let _ = files;
    sigterm_and_wait(&mut child);
}

#[cfg(unix)]
#[test]
fn serves_from_a_config_file_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("jc.toml");
    std::fs::write(
        &config,
        "role = \"daiya,dataroom\"\nlisten = \"127.0.0.1:0\"\nscheme = \"http\"\nlog_format = \"text\"\n",
    )
    .unwrap();
    let mut child = jc()
        .env("JCBIN_CONFIG", &config)
        .env("JCBIN_BASE_HOST", "city.test")
        .env("RUST_LOG", "info")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn jc");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let addr = listening_addr(&mut lines);
    let body = block_on(async {
        reqwest::Client::new()
            .get(format!("http://{addr}/dashboards/api/health"))
            .header("host", "city.test")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });
    assert!(body.contains("database"), "{body}");

    sigterm_and_wait(&mut child);
    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    assert!(rest.iter().any(|l| l.contains("shutting down")), "{rest:?}");
}
