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
    cmd
}

#[test]
fn a_refused_configuration_exits_2_and_says_why() {
    let out = jc().args(["--role", "keycloak"]).output().expect("run jc");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown module 'keycloak'"), "{stderr}");

    let out = jc().env("JCBIN_ROLE", "wardkeep").output().expect("run jc");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("key-encryption key"));
}

#[test]
fn a_start_failure_exits_1() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let out = jc()
        .args(["--role", "controlroom", "--listen"])
        .arg(taken.local_addr().unwrap().to_string())
        .output()
        .expect("run jc");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot listen"));
}

#[cfg(unix)]
#[test]
fn serves_from_a_config_file_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("jc.toml");
    std::fs::write(
        &config,
        "role = \"controlroom,dataroom\"\nlisten = \"127.0.0.1:0\"\nscheme = \"http\"\nlog_format = \"text\"\n",
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
    let stderr = child.stderr.take().unwrap();
    let mut lines = BufReader::new(stderr).lines();
    let mut addr = None;
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some(rest) = line.split("addr=").nth(1)
            && line.contains("jc listening")
        {
            addr = rest.split_whitespace().next().map(str::to_owned);
            break;
        }
    }
    let addr = addr.expect("jc logged its address");
    let body = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            reqwest::Client::new()
                .get(format!("http://{addr}/dashboards/api/health"))
                .header("host", "city.test")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        })
    })
    .join()
    .unwrap();
    assert!(body.contains("database"), "{body}");

    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "jc exited with {status}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "jc did not stop"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    assert!(rest.iter().any(|l| l.contains("shutting down")), "{rest:?}");
}
