#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL and run the binaries they test; jc runs none of this.
//! The `daiya` CLI subcommands as an operator runs them (R2, R10a): `check`, `import`,
//! `export`, `plan`, `apply` and `allowed`, their json and markdown reports and their exit codes
//! (0 clean, 1 refused or invalid input, 2 changes or warnings, 3 stale plan, 4 store or I/O).
//! Without `--config` every invocation runs on its own empty in-memory store.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_daiya");

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("daiya-cli-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, rel: &str, text: &str) -> String {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path.to_str().unwrap().to_owned()
    }
    fn path(&self, rel: &str) -> String {
        self.0.join(rel).to_str().unwrap().to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn daiya(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn dashboard(uid: &str) -> Value {
    json!({
        "uid": uid,
        "title": format!("Board {uid}"),
        "schemaVersion": 39,
        "panels": [{
            "id": 1, "type": "timeseries", "title": "Edge",
            "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
            "datasource": { "type": "prometheus", "uid": "prometheus" },
            "targets": [{ "refId": "A", "expr": "up" }]
        }]
    })
}

/// A dashboard using a feature daiya refuses (an Angular-only panel type).
fn refused_dashboard(uid: &str) -> Value {
    let mut d = dashboard(uid);
    d["panels"][0]["type"] = json!("graph");
    d["panels"][0]["datasource"] = json!({ "type": "elasticsearch", "uid": "es" });
    d["panels"][0]["targets"] = json!([{ "refId": "A", "query": "*", "datasource": { "type": "elasticsearch", "uid": "es" } }]);
    d
}

#[test]
fn check_reports_a_file_and_a_directory_in_json_and_markdown() {
    let scratch = Scratch::new();
    let one = scratch.write("one.json", &dashboard("one").to_string());
    let out = daiya(&["check", "-f", &one]);
    let report: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(matches!(out.status.code(), Some(0 | 2)), "{}", stderr(&out));
    assert_eq!(report["dashboards"][0]["uid"], "one", "{report}");
    // Migrated from 39 to the current schema.
    assert_eq!(
        report["dashboards"][0]["migration"]["finalSchemaVersion"], 42,
        "{report}"
    );

    scratch.write("dir/a.json", &dashboard("a").to_string());
    scratch.write("dir/b.json", &dashboard("b").to_string());
    scratch.write("dir/notes.txt", "not a dashboard");
    let out = daiya(&["check", "-f", &scratch.path("dir"), "--format", "markdown"]);
    let md = stdout(&out);
    assert!(md.starts_with('#'), "{md}");
    assert!(md.contains('a') && md.contains('b'), "{md}");
    assert!(!md.contains("notes"), "{md}");
}

#[test]
fn check_and_import_refuse_invalid_json_with_1_and_missing_files_with_4() {
    let scratch = Scratch::new();
    let broken = scratch.write("broken.json", "{ \"uid\": ");
    for cmd in ["check", "import"] {
        let out = daiya(&[cmd, "-f", &broken]);
        assert_eq!(out.status.code(), Some(1), "{cmd}: {}", stderr(&out));
        assert!(stderr(&out).contains("invalid JSON"), "{}", stderr(&out));

        let out = daiya(&[cmd, "-f", &scratch.path("absent.json")]);
        assert_eq!(out.status.code(), Some(4), "{cmd}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("failed to read file"),
            "{}",
            stderr(&out)
        );
    }
    // A broken file inside a directory names the file.
    scratch.write("dir/ok.json", &dashboard("ok").to_string());
    let bad = scratch.write("dir/zz.json", "[");
    let out = daiya(&["import", "-f", &scratch.path("dir")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains(&bad), "{}", stderr(&out));
}

#[test]
fn an_unreadable_entry_in_a_dashboard_directory_is_an_io_failure() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    scratch.write("dir/a.json", &dashboard("a").to_string());
    // A directory that only looks like a dashboard file.
    std::fs::create_dir_all(scratch.0.join("dir/b.json")).unwrap();
    let out = daiya(&["check", "-f", &scratch.path("dir")]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));

    // A directory the operator may not list.
    let locked = scratch.0.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let listable = std::fs::read_dir(&locked).is_ok();
    let out = daiya(&["import", "-f", locked.to_str().unwrap()]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    if listable {
        println!("SKIP (locked directory): running with rights that ignore permissions");
    } else {
        assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
        assert!(
            stderr(&out).contains("failed to read directory"),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn check_and_import_refuse_a_dashboard_with_a_refused_feature() {
    let scratch = Scratch::new();
    let file = scratch.write("refused.json", &refused_dashboard("old").to_string());
    let out = daiya(&["check", "-f", &file]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let out = daiya(&["import", "-f", &file, "--format", "md"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(stdout(&out).contains("Import Report"), "{}", stdout(&out));
}

#[test]
fn import_dry_run_reports_without_writing() {
    let scratch = Scratch::new();
    let file = scratch.write("d.json", &dashboard("dry").to_string());
    let out = daiya(&["import", "-f", &file, "--dry-run"]);
    assert!(matches!(out.status.code(), Some(0 | 2)), "{}", stderr(&out));
    let report: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["dryRun"], true, "{report}");
    assert_eq!(report["dashboards"][0]["uid"], "dry", "{report}");
}

#[test]
fn export_of_an_unknown_uid_is_refused_not_a_store_failure() {
    let out = daiya(&["export", "--uid", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("export failed"), "{}", stderr(&out));
    assert!(out.stdout.is_empty());
}

#[test]
fn plan_then_apply_and_a_tampered_plan_is_stale() {
    let scratch = Scratch::new();
    let bundle = scratch.write(
        "bundle.json",
        &json!({ "dashboards": [{ "rawJson": dashboard("planned") }] }).to_string(),
    );
    let out = daiya(&["plan", "-f", &bundle]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let plan: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(plan["hasChanges"], true, "{plan}");
    let plan_file = scratch.write("plan.json", &plan.to_string());

    // A fresh store has the same (empty) state the plan was computed on.
    let out = daiya(&["apply", "-f", &plan_file]);
    assert!(matches!(out.status.code(), Some(0 | 2)), "{}", stderr(&out));
    let applied: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(applied["dashboards"][0]["uid"], "planned", "{applied}");

    let mut stale = plan.clone();
    stale["baseEtag"] = json!("0".repeat(64));
    let stale_file = scratch.write("stale.json", &stale.to_string());
    let out = daiya(&["apply", "-f", &stale_file]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("apply failed"), "{}", stderr(&out));

    // An empty bundle plans no changes.
    let empty = scratch.write("empty.json", "{}");
    let out = daiya(&["plan", "-f", &empty]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn apply_reports_a_refused_dashboard_with_1() {
    let scratch = Scratch::new();
    let bundle = scratch.write(
        "bundle.json",
        &json!({ "dashboards": [{ "rawJson": refused_dashboard("refused") }] }).to_string(),
    );
    let out = daiya(&["plan", "-f", &bundle]);
    let plan: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let plan_file = scratch.write("plan.json", &plan.to_string());
    let out = daiya(&["apply", "-f", &plan_file]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
}

#[test]
fn plan_and_apply_refuse_unreadable_and_invalid_files() {
    let scratch = Scratch::new();
    let absent = scratch.path("absent.json");
    for cmd in ["plan", "apply"] {
        let out = daiya(&[cmd, "-f", &absent]);
        assert_eq!(out.status.code(), Some(4), "{cmd}: {}", stderr(&out));
        assert!(stderr(&out).contains("failed to read"), "{}", stderr(&out));
    }
    let not_a_plan = scratch.write("np.json", "{ \"dashboards\": [] }");
    let out = daiya(&["apply", "-f", &not_a_plan]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("invalid plan file"),
        "{}",
        stderr(&out)
    );

    // A plan for another organization is refused, not applied.
    let bundle = scratch.write("b.json", "{}");
    let plan: Value = serde_json::from_str(&stdout(&daiya(&["plan", "-f", &bundle]))).unwrap();
    let mut foreign = plan.clone();
    foreign["orgId"] = json!(uuid::Uuid::now_v7());
    let foreign_file = scratch.write("foreign.json", &foreign.to_string());
    let out = daiya(&["apply", "-f", &foreign_file]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

#[test]
fn allowed_answers_the_cli_subjects_decision() {
    let out = daiya(&["allowed", "--operation", "export"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let decision: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(decision["allowed"], true, "{decision}");

    // An operation the registry does not know is checked as the strictest action, Admin,
    // which only the CLI's own admin subject holds.
    let out = daiya(&[
        "allowed",
        "--operation",
        "no-such-operation",
        "--object",
        "Dashboard:x",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let decision: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(decision["reason"], "granted admin", "{decision}");
}
