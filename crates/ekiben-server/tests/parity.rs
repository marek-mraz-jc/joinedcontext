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
            cmd.arg("run").arg(&config);
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
