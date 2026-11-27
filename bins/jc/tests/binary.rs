#![allow(clippy::disallowed_methods)] // These tests start the jc binary and read their database URL.
//! The `jc` binary itself: configuration from a TOML file and `JCBIN_*`, a refused configuration
//! exits 2, a started `jc` serves and stops cleanly on SIGTERM.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

fn jc() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
    cmd.args(["--profile", "development"]);
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

/// `jc doctor`, `jc kaisatsu split-realm` and `jc validate` are commands of their own: each
/// answers its usage without starting a module.
#[test]
fn each_operator_command_answers_its_own_usage() {
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jc"))
            .args(args)
            .output()
            .expect("run jc");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        (out.status.code(), stdout, stderr)
    };
    let (code, _, stderr) = run(&["doctor"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("usage: jc doctor domains"), "{stderr}");
    let (code, stdout, _) = run(&["kaisatsu", "split-realm", "--help"]);
    assert_eq!(code, Some(0));
    assert!(stdout.contains("--retire"), "{stdout}");
    let (code, _, stderr) = run(&["kaisatsu", "split-realm", "--apply", "--restore"]);
    assert_eq!(code, Some(2), "{stderr}");
    let (code, _, stderr) = run(&["validate"]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("usage: joinedcontext-portal validate"),
        "{stderr}"
    );
    // `kaisatsu` alone is no command: the flags parser refuses it as jc's own argument.
    let (code, _, _) = run(&["kaisatsu"]);
    assert_eq!(code, Some(2));
}

/// `jc doctor domains` on a jc.toml without a base domain reports that, exits 1 and never prints
/// the Portal database URL it read from the environment.
#[test]
fn doctor_on_a_file_without_a_base_domain_says_so_and_prints_no_database_url() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("jc.toml");
    std::fs::write(&file, "[[organizations]]\nname = \"alpha\"\n").unwrap();
    let url = "postgres://portal:doctor-word-7898@127.0.0.1:1/portal";
    let out = Command::new(env!("CARGO_BIN_EXE_jc"))
        .args(["doctor", "domains", "--config"])
        .arg(&file)
        .env("JC_PORTAL_DATABASE_URL", url)
        .output()
        .expect("run jc");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("ERROR jc.toml names no base_domain"),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stdout.contains("doctor-word") && !stderr.contains("doctor-word"));
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

#[test]
fn a_dataset_alias_table_a_reader_could_not_trust_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("jc.toml");
    for (table, why) in [
        ("catalog = \"catalog\"", "an alias of itself"),
        ("a1 = \"b1\"\nb1 = \"c1\"", "is an alias too"),
        ("catalog = \"../x\"", "a dataset name is"),
    ] {
        std::fs::write(
            &config,
            format!("role = \"dataroom\"\nlisten = \"127.0.0.1:0\"\n[dataroom.aliases]\n{table}\n"),
        )
        .unwrap();
        let out = jc().env("JCBIN_CONFIG", &config).output().expect("run jc");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains("[dataroom.aliases]") && stderr.contains(why),
            "{stderr}"
        );
    }
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
    assert!(body.contains("\"commit\":\"daiya\""), "{body}");

    sigterm_and_wait(&mut child);
    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    assert!(rest.iter().any(|l| l.contains("shutting down")), "{rest:?}");
}

/// A Portal whose realm does not answer (discovery refused) delays the Portal, never stops jc:
/// the siblings serve, the Portal's mount answers 503 while jc retries, and jc stays up.
#[test]
fn a_portal_whose_realm_does_not_answer_keeps_jc_serving() {
    // Bound and never listening for the whole test: a connect is refused, and no other bind
    // takes the port (a dropped listener's port is anyone's).
    let closed = tokio::net::TcpSocket::new_v4().unwrap();
    closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let issuer = format!("http://{}/realms/jc", closed.local_addr().unwrap());
    let mut child = jc()
        .args([
            "--role",
            "daiya,portal",
            "--listen",
            "127.0.0.1:0",
            "--log-format",
            "text",
        ])
        .env("JCBIN_PORTAL_SIBLINGS", "false")
        .env("JC_OIDC_ISSUER", &issuer)
        .env("JC_OIDC_CLIENT_ID", "portal")
        .env("JC_OIDC_CLIENT_SECRET", "not-used-the-realm-never-answers")
        .env("JC_PORTAL_COOKIE_KEY", "k".repeat(64))
        .env("RUST_LOG", "info")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn jc");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let addr = listening_addr(&mut lines);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in lines.map_while(Result::ok) {
            if line.contains("portal: not built, retrying") {
                let _ = tx.send(());
            }
        }
    });
    for _ in 0..2 {
        rx.recv_timeout(Duration::from_secs(30))
            .expect("jc logs a failed Portal build and retries");
    }
    let (portal, daiya) = block_on(async {
        let get = |path: &str| {
            let url = format!("http://{addr}{path}");
            async move { reqwest::get(url).await.unwrap().status() }
        };
        (
            get("/portal/api/v1/health").await,
            get("/dashboards/api/health").await,
        )
    });
    assert_eq!(portal, reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(daiya, reqwest::StatusCode::OK);
    assert!(child.try_wait().unwrap().is_none(), "jc stopped");
    sigterm_and_wait(&mut child);
}

/// A Portal without a dataroom beside it, in this process or another, builds with no
/// installation publisher: it serves its health route and stops cleanly on SIGTERM.
#[cfg(unix)]
#[test]
fn a_portal_without_a_dataroom_serves_and_stops() {
    let mut child = jc()
        .args([
            "--role",
            "portal",
            "--listen",
            "127.0.0.1:0",
            "--log-format",
            "text",
        ])
        .env("JCBIN_PORTAL_SIBLINGS", "false")
        .env("RUST_LOG", "info")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn jc");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let addr = listening_addr(&mut lines);
    std::thread::spawn(move || lines.map_while(Result::ok).for_each(drop));
    let healthy = block_on(async {
        for _ in 0..240 {
            let status = reqwest::get(format!("http://{addr}/portal/api/v1/health"))
                .await
                .map(|r| r.status());
            if matches!(status, Ok(s) if s.is_success()) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        false
    });
    assert!(healthy, "the Portal answers its health route");
    assert!(child.try_wait().unwrap().is_none(), "jc stopped");
    sigterm_and_wait(&mut child);
}
