//! The `ekiben` command line: each subcommand's exit code, its error text for a missing or bad
//! file or flag, and `streams` and `schedule` stopping cleanly on SIGTERM.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

fn ekiben(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ekiben"))
        .args(args)
        .env_remove("EKIBEN_BEARER_TOKEN")
        .output()
        .expect("ekiben runs")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, text).expect("write");
    p.to_string_lossy().into_owned()
}

const ONCE: &str =
    "input:\n  generate:\n    count: 1\n    mapping: 'root = {\"a\": 1}'\noutput:\n  drop: {}\n";

#[test]
fn run_prints_the_summary_and_exits_by_its_success() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cfg = write(d, "job.yaml", ONCE);
    let quotas = write(d, "q.yaml", "max_messages_per_sec: 1000\n");
    let o = ekiben(&[
        "run",
        "--quotas",
        &quotas,
        "--allow-private-ips",
        "--clock",
        "2026-11-07T09:16:19Z",
        &cfg,
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let summary: serde_json::Value = serde_json::from_str(stderr(&o).trim()).unwrap();
    assert_eq!(summary["stream_id"], "job");
    assert_eq!(summary["messages_in"], 1);

    let o = ekiben(&["run", "--allow-private-ips", &cfg]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));

    let failing = write(
        d,
        "fail.yaml",
        "input:\n  file:\n    paths: [/nonexistent/ekiben-t7583.txt]\noutput:\n  drop: {}\n",
    );
    let o = ekiben(&["run", &failing]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("\"success\":false"), "{}", stderr(&o));
}

#[test]
fn run_refuses_missing_files_bad_configs_quotas_and_clocks() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cfg = write(d, "job.yaml", ONCE);
    for (args, want) in [
        (
            vec!["run", "/nonexistent/x.yaml"],
            "Error: failed to read '/nonexistent/x.yaml'",
        ),
        (
            vec!["run", "--quotas", "/nonexistent/q.yaml", &cfg],
            "failed to read quotas",
        ),
    ] {
        let o = ekiben(&args);
        assert_eq!(o.status.code(), Some(1));
        assert!(stderr(&o).contains(want), "{args:?}: {}", stderr(&o));
    }
    let bad_q = write(d, "bad.yaml", "max_messages_per_sec: [\n");
    let o = ekiben(&["run", "--quotas", &bad_q, &cfg]);
    assert!(stderr(&o).contains("quotas '"), "{}", stderr(&o));
    let typed = write(d, "typed.yaml", "max_messages_per_sec: fast\n");
    let o = ekiben(&["run", "--quotas", &typed, &cfg]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("quotas '"), "{}", stderr(&o));
    let bad = write(d, "bad_cfg.yaml", "input: [\n");
    assert_eq!(ekiben(&["run", &bad]).status.code(), Some(1));
    let unknown = write(
        d,
        "unknown.yaml",
        "input:\n  no_such_input: {}\noutput:\n  drop: {}\n",
    );
    assert_eq!(ekiben(&["run", &unknown]).status.code(), Some(1));
    let o = ekiben(&["run", "--clock", "yesterday", &cfg]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("not an RFC 3339 time"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn blobl_maps_each_line_and_reports_bad_ones() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ekiben"))
        .args(["blobl", "root = this.a"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"a\":1}\r\nnot json\n{\"a\":\"x\"}\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "1\nx\n");
    assert!(!stderr(&o).is_empty());
    let o = ekiben(&["blobl", "root = ("]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).starts_with("failed to parse mapping: "),
        "{}",
        stderr(&o)
    );
}

#[test]
fn blobl_imports_and_reads_files_of_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let lib = write(
        dir.path(),
        "lib.blobl",
        "map up {\n  root = this.uppercase()\n}\n",
    );
    let mapping = format!("import \"{lib}\"\nroot = this.apply(\"up\")");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ekiben"))
        .args(["blobl", "--raw", &mapping])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child.stdin.take().unwrap().write_all(b"abc\n").unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "ABC\n");
    let o = ekiben(&["blobl", "import \"/nonexistent/lib.blobl\"\nroot = this"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).starts_with("failed to parse mapping: "),
        "{}",
        stderr(&o)
    );
}

#[test]
fn list_answers_every_component_kind_and_the_extensions() {
    let o = ekiben(&["list"]);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    for key in [
        "scanners",
        "bloblang-functions",
        "bloblang-methods",
        "ekiben-extensions",
    ] {
        assert!(v[key].is_array(), "{key}");
    }
    assert!(v["ekiben-extensions"].as_array().unwrap().iter().any(|e| e
        .as_str()
        .is_some_and(|s| s.starts_with("bloblang-functions/"))));
}

#[test]
fn test_and_lint_exit_by_their_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let good = write(
        d,
        "good.yaml",
        "pipeline:\n  processors:\n    - mapping: 'root = content().uppercase()'\ntests:\n  - name: up\n    input_batch: [{content: a}]\n    output_batches: [[{content_equals: A}]]\n",
    );
    let o = ekiben(&["test", &good]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let bad = write(
        d,
        "bad.yaml",
        "pipeline:\n  processors:\n    - mapping: 'root = content()'\ntests:\n  - name: up\n    input_batch: [{content: a}]\n    output_batches: [[{content_equals: A}]]\n",
    );
    assert_eq!(ekiben(&["test", &bad]).status.code(), Some(1));

    let ok = write(d, "ok.yaml", ONCE);
    assert_eq!(ekiben(&["lint", &ok]).status.code(), Some(0));
    let missing = write(
        d,
        "env.yaml",
        "input:\n  generate:\n    mapping: 'root = \"${EKIBEN_T7583_UNSET}\"'\noutput:\n  drop: {}\n",
    );
    let o = ekiben(&["lint", &missing]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("EKIBEN_T7583_UNSET"), "{}", stderr(&o));
    assert_eq!(
        ekiben(&["lint", "--chilled", &missing]).status.code(),
        Some(0)
    );
    let broken = write(d, "broken.yaml", "input: [\n");
    let o = ekiben(&["lint", &broken]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("broken.yaml: "), "{}", stderr(&o));
    let o = ekiben(&["lint", "/nonexistent/l.yaml"]);
    assert!(stderr(&o).contains("failed to read"), "{}", stderr(&o));
}

#[test]
fn streams_refuses_a_bad_start_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let tokens = write(d, "tokens", "");
    let bad_tokens = write(d, "bad_tokens", "org-a\n");
    let bad_metrics = write(d, "m.yaml", "metrics:\n  max_series: many\n");
    let bad_res = write(
        d,
        "r.yaml",
        "processor_resources:\n  - label: p\n    mapping: 'root = ('\n",
    );
    let ingest = write(
        d,
        "ingest.yaml",
        "input:\n  http_server:\n    address: 0.0.0.0:9\n    path: /x\noutput:\n  drop: {}\n",
    );
    let broken = write(d, "broken.yaml", "input: [\n");
    let unknown = write(
        d,
        "unknown.yaml",
        "input:\n  no_such_input: {}\noutput:\n  drop: {}\n",
    );
    let vars = d.join("vars");
    std::fs::create_dir(&vars).unwrap();
    write(&vars, "org-a.yaml", "1BAD: x\n");
    let cfgdir = d.join("cfg");
    std::fs::create_dir(&cfgdir).unwrap();
    write(&cfgdir, "s.yaml", "input: [\n");
    let file_root = d.to_string_lossy().into_owned();
    let vars = vars.to_string_lossy().into_owned();
    let cfgdir = cfgdir.to_string_lossy().into_owned();
    for (args, want) in [
        (
            vec!["--org-tokens-file", tokens.as_str()],
            "set --file-root",
        ),
        (
            vec![
                "--org-tokens-file",
                bad_tokens.as_str(),
                "--file-root",
                &file_root,
            ],
            "org tokens file",
        ),
        (vec!["--metrics", bad_metrics.as_str()], "max_series"),
        (vec!["-r", "/nonexistent/r.yaml"], "failed to read"),
        (vec!["-r", bad_res.as_str()], "r.yaml: "),
        (vec!["--org-vars", vars.as_str()], "org-a.yaml"),
        (
            vec!["--ingest-addr", "256.0.0.1:1"],
            "failed to bind ingest listener",
        ),
        (vec!["/nonexistent/s.yaml"], "failed to read"),
        (vec![broken.as_str()], "Error: "),
        (vec![ingest.as_str()], "ingest.yaml: "),
        (vec!["--config-dir", cfgdir.as_str()], "s: "),
        (vec!["--host", "256.0.0.1"], "failed to bind 256.0.0.1:0"),
        (vec![unknown.as_str()], "Error: "),
        (vec!["--set", "metrics.mapping=root = ("], "Error: "),
    ] {
        let mut all = vec!["streams", "--port", "0"];
        all.extend(args.iter().copied());
        let o = ekiben(&all);
        assert_eq!(o.status.code(), Some(1), "{all:?}");
        assert!(stderr(&o).contains(want), "{all:?}: {}", stderr(&o));
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn terminate(child: &std::process::Child) {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn wait_exit(mut child: std::process::Child) -> std::process::ExitStatus {
    for _ in 0..200 {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("ekiben did not stop on SIGTERM");
}

#[tokio::test]
async fn streams_serves_with_every_option_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (port, ingest) = (free_port(), free_port());
    let tokens = write(d, "tokens", "");
    let metrics = write(d, "m.yaml", "prometheus:\n  use_histogram_timing: true\n");
    let res = write(
        d,
        "r.yaml",
        "cache_resources:\n  - label: c\n    memory: {}\n",
    );
    let cfg = write(
        d,
        "boot.yaml",
        "input:\n  generate:\n    interval: 1h\n    mapping: 'root = {}'\noutput:\n  drop: {}\n",
    );
    let vars = d.join("vars");
    std::fs::create_dir(&vars).unwrap();
    let cfgdir = d.join("cfg");
    std::fs::create_dir(&cfgdir).unwrap();
    let files: PathBuf = d.join("files");
    let child = Command::new(env!("CARGO_BIN_EXE_ekiben"))
        .args([
            "streams",
            "--port",
            &port.to_string(),
            "--bearer-token",
            "admin-t7583",
        ])
        .args([
            "--org-tokens-file",
            &tokens,
            "--file-root",
            files.to_str().unwrap(),
        ])
        .args([
            "--org-vars",
            vars.to_str().unwrap(),
            "--config-dir",
            cfgdir.to_str().unwrap(),
        ])
        .args([
            "--ingest-addr",
            &format!("127.0.0.1:{ingest}"),
            "--metrics",
            &metrics,
        ])
        .args(["-r", &res, "--require-auth", &cfg])
        .args(["--set", "metrics.prometheus.use_histogram_timing=false"])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let c = reqwest::Client::new();
    let mut up = false;
    for _ in 0..100 {
        if c.get(format!("http://127.0.0.1:{port}/ping"))
            .send()
            .await
            .is_ok()
        {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "the server never answered /ping");
    let res = c
        .get(format!("http://127.0.0.1:{port}/streams"))
        .bearer_auth("admin-t7583")
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = res.json().await.unwrap();
    assert!(list["boot"].is_object(), "{list}");
    let unauth = c
        .get(format!("http://127.0.0.1:{port}/streams"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);
    terminate(&child);
    assert_eq!(wait_exit(child).code(), Some(0));
}

#[test]
fn schedule_loads_its_configs_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cfg = write(
        d,
        "tick.yaml",
        "schedule:\n  period: 1s\ninput:\n  generate:\n    count: 1\n    mapping: 'root = 1'\noutput:\n  drop: {}\n",
    );
    let child = Command::new(env!("CARGO_BIN_EXE_ekiben"))
        .args(["schedule", &cfg])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    terminate(&child);
    assert_eq!(wait_exit(child).code(), Some(0));

    let unscheduled = write(d, "plain.yaml", ONCE);
    let o = ekiben(&["schedule", &unscheduled]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("has no schedule configuration"),
        "{}",
        stderr(&o)
    );
    let o = ekiben(&["schedule", "/nonexistent/s.yaml"]);
    assert!(stderr(&o).contains("failed to read"), "{}", stderr(&o));
    let broken = write(d, "broken.yaml", "input: [\n");
    assert_eq!(ekiben(&["schedule", &broken]).status.code(), Some(1));
}
