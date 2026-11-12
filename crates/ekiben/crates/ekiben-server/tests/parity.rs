//! Bloblang parity with Bento 1.22.0 over the joinedcontext-deployment seed pipelines.
//!
//! Each case in `tests/parity/cases.json` is one Bento run a deployment test made (config, stdin,
//! environment) with Bento's stdout and exit code recorded beside it; `scripts/parity/regen.py`
//! captures and regenerates them. A case marked `identical` must give Bento's bytes and exit code;
//! one marked `different` must still differ, so a fix that closes a gap also regenerates the table
//! (`tests/parity/PARITY.md`) instead of passing silently. A case whose mapping reads `now()`
//! carries the `clock` its Bento run started at and replays on it (`ekiben run --clock`).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn blob(sha: &str) -> Vec<u8> {
    let raw = std::fs::read(repo().join(format!("tests/parity/blobs/{sha}.gz")))
        .unwrap_or_else(|e| panic!("blob {sha}: {e}"));
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(raw.as_slice())
        .read_to_end(&mut out)
        .unwrap_or_else(|e| panic!("blob {sha}: {e}"));
    out
}

fn text(case: &Value, key: &str) -> String {
    case[key].as_str().unwrap_or_default().to_string()
}

/// Runs one case in ekiben and answers whether stdout and exit code equal Bento's.
fn same_as_bento(case: &Value) -> (bool, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ekiben"));
    match case["mode"].as_str() {
        Some("blobl") => {
            cmd.arg("blobl");
            for arg in case["args"].as_array().into_iter().flatten() {
                cmd.arg(arg.as_str().unwrap_or_default());
            }
        }
        _ => {
            let config = dir.path().join("c.json");
            std::fs::write(&config, blob(&text(case, "config"))).expect("write config");
            // `--host-env`: a parity case reads the whole environment, as Bento does (E1 keeps
            // `run` to `EKIBEN_VAR_*` otherwise).
            cmd.arg("run").arg("--host-env").arg(&config);
            // A mapping reading now() runs on the second Bento ran it (praha drops readings
            // older than 3 h, so on today's clock its recorded answer would expire).
            if let Some(clock) = case["clock"].as_str() {
                cmd.arg("--clock").arg(clock);
            }
        }
    }
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", dir.path())
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in case["env"].as_object().into_iter().flatten() {
        cmd.env(k, v.as_str().unwrap_or_default());
    }
    let mut child = cmd.spawn().expect("spawn ekiben");
    let input = blob(&text(case, "stdin"));
    let mut stdin = child.stdin.take().expect("stdin");
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().expect("ekiben output");
    let _ = writer.join();
    let expected = blob(&text(&case["bento"], "stdout"));
    let code = case["bento"]["code"].as_i64().unwrap_or_default();
    let same = out.stdout == expected && i64::from(out.status.code().unwrap_or(-1)) == code;
    let stderr = String::from_utf8_lossy(&out.stderr);
    (
        same,
        stderr
            .chars()
            .rev()
            .take(300)
            .collect::<String>()
            .chars()
            .rev()
            .collect(),
    )
}

/// The record `scripts/parity-sql.py` writes for one run of a case: each step's exit and stdout,
/// an exit reduced to ok/failed (Bento retries a failing write until the 60 s cap, ekiben stops).
fn sql_case_record(case: &Path, env: &[(String, String)]) -> String {
    // PG_RESET / MY_RESET empty the databases between cases, as the recording did.
    for key in ["PG_RESET", "MY_RESET"] {
        if let Ok(cmd) = std::env::var(key) {
            let ok = Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "{key} failed");
        }
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let mut steps: Vec<PathBuf> = std::fs::read_dir(case)
        .expect("case dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    steps.sort_by_key(|p| {
        p.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
    });
    let mut out = String::new();
    for step in steps {
        let o = Command::new(env!("CARGO_BIN_EXE_ekiben"))
            .args(["run", "--host-env", "--allow-private-ips"])
            .arg(&step)
            .env("LITE_DSN", format!("file:{}/lite.db", dir.path().display()))
            .envs(env.iter().cloned())
            .output()
            .expect("run ekiben");
        let stem = step
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let code = if o.status.success() { "ok" } else { "failed" };
        out.push_str(&format!(
            "## step {stem} {code}\n{}",
            String::from_utf8_lossy(&o.stdout)
        ));
    }
    out
}

fn strip_codes(record: &str) -> String {
    record
        .split_inclusive('\n')
        .map(|l| {
            match l
                .strip_prefix("## step ")
                .and_then(|r| r.rsplit_once(" exit "))
            {
                Some((n, code)) => format!(
                    "## step {n} {}\n",
                    if code.trim() == "0" { "ok" } else { "failed" }
                ),
                None => l.to_string(),
            }
        })
        .collect()
}

/// The sql inputs and outputs (T-6834) against Bento 1.22.0's recorded runs: sqlite cases always,
/// postgres and mysql cases where PG_DSN / MY_DSN name a database (the CI service containers);
/// each lint case keeps Bento's verdict unless it carries a deviation.txt.
#[test]
fn sql_cases_match_bento() {
    let root = repo().join("tests/parity/sql");
    let env: Vec<(String, String)> = ["PG_DSN", "MY_DSN"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    let mut checked = 0;
    for group in std::fs::read_dir(&root)
        .expect("tests/parity/sql")
        .flatten()
    {
        if !group.path().is_dir() {
            continue;
        }
        for case in std::fs::read_dir(group.path()).expect("group").flatten() {
            let case = case.path();
            let name = case
                .strip_prefix(&root)
                .unwrap_or(&case)
                .display()
                .to_string();
            let want = std::fs::read_to_string(case.join("expected.txt")).expect("expected.txt");
            if name.starts_with("lint") {
                let ok = Command::new(env!("CARGO_BIN_EXE_ekiben"))
                    .arg("lint")
                    .arg(case.join("1.yaml"))
                    .env("LITE_DSN", "file:/tmp/lint.db")
                    .env("PG_DSN", "postgres://h/db")
                    .output()
                    .expect("lint")
                    .status
                    .success();
                let got = if ok { "accepted" } else { "refused" };
                assert!(
                    got == want.trim() || case.join("deviation.txt").exists(),
                    "{name}: Bento lint {}, ekiben {got}",
                    want.trim()
                );
                checked += 1;
                continue;
            }
            let steps = std::fs::read_to_string(case.join("1.yaml")).unwrap_or_default()
                + &std::fs::read_to_string(case.join("2.yaml")).unwrap_or_default();
            if ["PG_DSN", "MY_DSN"]
                .iter()
                .any(|k| steps.contains(&format!("${{{k}}}")) && !env.iter().any(|(e, _)| e == k))
            {
                eprintln!("SKIP {name}: its database is not given");
                continue;
            }
            let got = sql_case_record(&case, &env);
            assert_eq!(got, strip_codes(&want), "{name} differs from Bento");
            checked += 1;
        }
    }
    assert!(checked >= 5, "only {checked} sql cases ran");
}

#[test]
fn every_recorded_case_keeps_its_verdict_against_bento() {
    let doc: Value = serde_json::from_slice(
        &std::fs::read(repo().join("tests/parity/cases.json")).expect("tests/parity/cases.json"),
    )
    .expect("cases.json is JSON");
    let cases: Vec<&Value> = doc["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|c| c.get("ekiben").is_some())
        .collect();
    assert!(!cases.is_empty(), "no runnable case recorded");
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get().min(6));
    let chunk = cases.len().div_ceil(workers);
    let wrong: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .filter_map(|case| {
                            let (same, stderr) = same_as_bento(case);
                            let recorded = case["ekiben"] == "identical";
                            (same != recorded).then(|| {
                                format!(
                                    "{} ({}): recorded {}, now {}; {stderr}",
                                    text(case, "id"),
                                    case["tests"][0].as_str().unwrap_or_default(),
                                    text(case, "ekiben"),
                                    if same { "identical" } else { "different" },
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("worker"))
            .collect()
    });
    assert!(
        wrong.is_empty(),
        "{} of {} cases changed verdict (rerun scripts/parity/regen.py build when a gap closed):\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}
