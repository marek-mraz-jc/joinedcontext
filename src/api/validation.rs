//! `GET /api/v1/organization/health`: the last published result of every validation check
//! (OPS-53, API/01 §26).
//!
//! The checks run outside the Portal and publish one digest each into the ConfigMap
//! `jc-validation-results`, which the deployment mounts at `JC_HEALTH_DIR`. The directory is read
//! on every request, as the branding file is, so a new result shows up without a restart. A
//! digest carries keys, titles, verdicts, counts and task ids only; anything else in a file makes
//! it unreadable rather than shown.

use std::io::Read;
use std::path::Path;

use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::is_dns1123;
use crate::state::AppState;

const MAX_FILE: u64 = 256 * 1024;
const MAX_FAILURES: usize = 50;
const MAX_HISTORY: usize = 200;
const MAX_TEXT: usize = 300;
const MAX_EVERY_HOURS: u32 = 744;
const MAX_PASSED: usize = 500;
/// The check the App probe publishes, one key `{project}/{name}` per published App (AP-136).
const APPS_CHECK: &str = "apps";

/// How many results of one run ended in each verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub pass: u32,
    pub fail: u32,
    pub error: u32,
    pub skip: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FailureVerdict {
    Fail,
    Error,
}

/// One result of the last run that did not pass, and the open task it filed.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub key: String,
    pub verdict: FailureVerdict,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// One earlier run, for the trend.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Point {
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
    pub pass: u32,
    pub fail: u32,
    pub error: u32,
    pub skip: u32,
}

/// What `scripts/publish-health.py` writes for one check.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Digest {
    pub check: String,
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
    pub every_hours: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub counts: Counts,
    #[serde(default)]
    pub failures: Vec<Failure>,
    #[serde(default)]
    pub history: Vec<Point>,
    /// The keys that passed, for a check whose passes a page shows (`apps`, AP-136); empty for
    /// every other check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Green,
    Red,
    Stale,
    Unreadable,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CheckHealth {
    pub check: String,
    pub state: CheckState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Digest>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ValidationHealth {
    pub checks: Vec<CheckHealth>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_task(task: &str) -> bool {
    task.strip_prefix("T-")
        .is_some_and(|n| (4..=6).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The digest in `bytes` if it is one this page may show under `name`.
fn digest(name: &str, bytes: &[u8]) -> Option<Digest> {
    let digest: Digest = serde_json::from_slice(bytes).ok()?;
    let short = |text: &str| text.chars().count() <= MAX_TEXT;
    let ok = digest.check == name
        && (1..=MAX_EVERY_HOURS).contains(&digest.every_hours)
        && digest.run.as_deref().is_none_or(short)
        && digest.failures.len() <= MAX_FAILURES
        && digest.history.len() <= MAX_HISTORY
        && digest.passed.len() <= MAX_PASSED
        && digest.passed.iter().all(|key| short(key))
        && digest
            .failures
            .iter()
            .all(|f| short(&f.key) && short(&f.title) && f.task.as_deref().is_none_or(valid_task));
    ok.then_some(digest)
}

fn state_of(digest: &Digest, now: DateTime<Utc>) -> CheckState {
    if now - digest.at > Duration::hours(2 * i64::from(digest.every_hours)) {
        CheckState::Stale
    } else if digest.counts.fail + digest.counts.error > 0 {
        CheckState::Red
    } else {
        CheckState::Green
    }
}

fn read_capped(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_FILE).then_some(bytes)
}

/// Every `{check}.json` under `dir`, by name. The kubelet's own `..data` links and anything
/// that is not a JSON file are not checks. A directory that does not exist is an installation
/// that has published nothing yet.
pub fn read_dir(dir: &Path, now: DateTime<Utc>) -> std::io::Result<Vec<CheckHealth>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut checks = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file = entry.file_name();
        let Some(name) = file.to_str().and_then(|f| f.strip_suffix(".json")) else {
            continue;
        };
        if !valid_name(name) {
            continue;
        }
        let result = read_capped(&entry.path()).and_then(|bytes| digest(name, &bytes));
        checks.push(CheckHealth {
            check: name.to_owned(),
            state: result
                .as_ref()
                .map_or(CheckState::Unreadable, |d| state_of(d, now)),
            result,
        });
    }
    checks.sort_by(|a, b| a.check.cmp(&b.check));
    Ok(checks)
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/health",
    summary = "Read Validation Health",
    description = "The last published result of every validation check, with its state. Only an administrator of the organization: approve and delete on RoleBinding at organization scope (OPS-53, PF-03).",
    tag = "system",
    responses(
        (status = 200, description = "Every published check", body = ValidationHealth),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an administrator of the organization", body = crate::error::ProblemDetails),
        (status = 503, description = "The results directory cannot be read", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_health(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ValidationHealth>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, ORG_NAMESPACE);
    if !effective.administers_organization() {
        return Err(ApiError::Denied(
            "only an administrator of the organization reads the validation health: it needs \
             approve and delete on RoleBinding at organization scope (OPS-53, PF-03)"
                .into(),
        ));
    }
    let Some(dir) = state.config.health_dir.as_deref() else {
        return Ok(Json(ValidationHealth { checks: Vec::new() }));
    };
    let checks = read_dir(Path::new(dir), Utc::now()).map_err(|err| {
        tracing::warn!(error = %err, "the validation results directory cannot be read");
        ApiError::Unavailable(
            "the validation results cannot be read: check the jc-validation-results mount".into(),
        )
    })?;
    Ok(Json(ValidationHealth { checks }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AppCheckState {
    Green,
    Red,
    Amber,
}

/// The last probe of one App (AP-136).
#[derive(Debug, Serialize, ToSchema)]
pub struct AppCheck {
    pub name: String,
    pub state: AppCheckState,
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppChecks {
    pub checks: Vec<AppCheck>,
}

/// `project`'s share of the `apps` digest under `dir`: nothing when it is absent or unreadable.
pub fn app_checks(dir: &Path, project: &str, now: DateTime<Utc>) -> Vec<AppCheck> {
    let Some(digest) = read_capped(&dir.join(format!("{APPS_CHECK}.json")))
        .and_then(|bytes| digest(APPS_CHECK, &bytes))
    else {
        return Vec::new();
    };
    let stale = state_of(&digest, now) == CheckState::Stale;
    let prefix = format!("{project}/");
    let row = |key: &str, state: AppCheckState, reason: Option<&str>| {
        let name = key.strip_prefix(&prefix)?;
        (!name.is_empty() && !name.contains('/')).then(|| AppCheck {
            name: name.to_owned(),
            state: if stale { AppCheckState::Amber } else { state },
            at: digest.at,
            reason: reason.map(str::to_owned),
        })
    };
    let mut checks: Vec<AppCheck> = digest
        .failures
        .iter()
        .filter_map(|f| row(&f.key, AppCheckState::Red, Some(&f.title)))
        .chain(
            digest
                .passed
                .iter()
                .filter_map(|key| row(key, AppCheckState::Green, None)),
        )
        .collect();
    checks.sort_by(|a, b| a.name.cmp(&b.name));
    // A key both failed and passed is a probe that contradicts itself: the failure stands.
    checks.dedup_by(|later, earlier| later.name == earlier.name);
    checks
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/app-checks",
    summary = "List App Checks",
    description = "The probe's last verdict on each published App of the project: green, red with the reason, or amber when the probe has not run for two of its intervals. Needs `read` on App (AP-136).",
    tag = "apps",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The App checks of the project", body = AppChecks),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller lacks read on App", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_app_checks(
    user: CurrentUser,
    State(state): State<AppState>,
    UrlPath(project): UrlPath<String>,
) -> Result<Json<AppChecks>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !is_dns1123(&project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    effective.check("App", Verb::Read, None)?;
    let checks = state
        .config
        .health_dir
        .as_deref()
        .map(|dir| app_checks(Path::new(dir), &project, Utc::now()))
        .unwrap_or_default();
    Ok(Json(AppChecks { checks }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization/health", get(get_health))
        .route("/projects/{project}/app-checks", get(list_app_checks))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-09-25T10:00:00Z";

    fn now() -> DateTime<Utc> {
        NOW.parse().unwrap()
    }

    fn body(check: &str, at: &str, fail: u32) -> String {
        format!(
            r#"{{"check":"{check}","at":"{at}","everyHours":1,"run":"dev-validate",
            "counts":{{"pass":3,"fail":{fail},"error":0,"skip":1}},
            "failures":[{{"key":"images/portal","verdict":"fail","title":"unsigned","task":"T-2901"}}],
            "history":[{{"at":"{at}","pass":3,"fail":0,"error":0,"skip":1}}]}}"#
        )
    }

    /// A fresh directory of its own per test, under the system's temporary directory.
    fn tempdir(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jc-health-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    fn states(checks: &[CheckHealth]) -> Vec<(&str, CheckState)> {
        checks.iter().map(|c| (c.check.as_str(), c.state)).collect()
    }

    #[test]
    fn a_run_is_green_red_or_stale_by_its_counts_and_its_age() {
        let dir = tempdir("green");
        write(
            dir.as_path(),
            "budgets.json",
            &body("budgets", "2026-09-25T09:30:00Z", 0),
        );
        write(
            dir.as_path(),
            "authz.json",
            &body("authz", "2026-09-25T09:30:00Z", 2),
        );
        // Older than two runs: stale even though it was red.
        write(
            dir.as_path(),
            "sweep.json",
            &body("sweep", "2026-09-25T07:59:00Z", 1),
        );
        // Exactly two hours is still on time.
        write(
            dir.as_path(),
            "drift.json",
            &body("drift", "2026-09-25T08:00:00Z", 0),
        );
        let checks = read_dir(dir.as_path(), now()).unwrap();
        assert_eq!(
            states(&checks),
            [
                ("authz", CheckState::Red),
                ("budgets", CheckState::Green),
                ("drift", CheckState::Green),
                ("sweep", CheckState::Stale),
            ]
        );
        let failure = &checks[0].result.as_ref().unwrap().failures[0];
        assert_eq!(failure.task.as_deref(), Some("T-2901"));
    }

    #[test]
    fn a_file_that_is_not_a_digest_is_unreadable_and_shows_nothing_of_itself() {
        let dir = tempdir("unreadable");
        let good = |name: &str| body(name, NOW, 0);
        let cases = [
            ("renamed", good("other")),
            (
                "detail",
                good("detail").replace(r#""task":"T-2901""#, r#""detail":"password=hunter2""#),
            ),
            ("task", good("task").replace("T-2901", "rm -rf")),
            (
                "every",
                good("every").replace(r#""everyHours":1"#, r#""everyHours":0"#),
            ),
            (
                "monthly",
                good("monthly").replace(r#""everyHours":1"#, r#""everyHours":745"#),
            ),
            ("title", good("title").replace("unsigned", &"x".repeat(301))),
            ("broken", "{".to_owned()),
        ];
        for (name, contents) in &cases {
            write(dir.as_path(), &format!("{name}.json"), contents);
        }
        let big = format!("{}{}", " ".repeat(MAX_FILE as usize), body("big", NOW, 0));
        write(dir.as_path(), "big.json", &big);
        let checks = read_dir(dir.as_path(), now()).unwrap();
        assert_eq!(checks.len(), cases.len() + 1);
        for check in &checks {
            assert_eq!(check.state, CheckState::Unreadable, "{}", check.check);
            assert!(check.result.is_none());
        }
        let json = serde_json::to_string(&checks).unwrap();
        assert!(!json.contains("hunter2"));
    }

    #[test]
    fn only_json_files_with_a_check_name_are_checks_and_no_directory_is_no_checks() {
        let dir = tempdir("names");
        write(dir.as_path(), "..data", "{}");
        write(dir.as_path(), "README", "not a check");
        write(dir.as_path(), "Bad_Name.json", &body("Bad_Name", NOW, 0));
        assert!(read_dir(dir.as_path(), now()).unwrap().is_empty());
        assert!(read_dir(&dir.as_path().join("missing"), now())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn task_ids_are_the_boards() {
        assert!(valid_task("T-2901") && valid_task("T-123456"));
        assert!(!valid_task("T-12") && !valid_task("t-2901") && !valid_task("T-29a1"));
    }

    fn apps_digest(at: &str, passed: &[&str]) -> String {
        format!(
            r#"{{"check":"apps","at":"{at}","everyHours":1,
            "counts":{{"pass":{},"fail":1,"error":0,"skip":0}},
            "failures":[{{"key":"helsinki/alerts","verdict":"fail","title":"no row read in 60 s"}}],
            "passed":{}}}"#,
            passed.len(),
            serde_json::to_string(passed).unwrap()
        )
    }

    // AP-136: the chip of each App of one project, and of no other project.
    #[test]
    fn an_app_is_green_when_it_passed_red_with_the_reason_when_it_failed() {
        let dir = tempdir("apps");
        let passed = [
            "helsinki/bikes",
            "praha/bikes",
            "helsinki/a/b",
            "helsinki/alerts",
        ];
        write(
            &dir,
            "apps.json",
            &apps_digest("2026-09-25T09:30:00Z", &passed),
        );
        let checks = app_checks(&dir, "helsinki", now());
        let rows: Vec<_> = checks
            .iter()
            .map(|c| (c.name.as_str(), c.state, c.reason.as_deref()))
            .collect();
        // A key that both failed and passed stays red.
        assert_eq!(
            rows,
            [
                ("alerts", AppCheckState::Red, Some("no row read in 60 s")),
                ("bikes", AppCheckState::Green, None),
            ]
        );
        assert!(app_checks(&dir, "espoo", now()).is_empty());
    }

    #[test]
    fn a_probe_that_stopped_running_turns_every_app_amber() {
        let dir = tempdir("apps-stale");
        write(
            &dir,
            "apps.json",
            &apps_digest("2026-09-25T07:00:00Z", &["helsinki/bikes"]),
        );
        let states: Vec<_> = app_checks(&dir, "helsinki", now())
            .iter()
            .map(|c| c.state)
            .collect();
        assert_eq!(states, [AppCheckState::Amber, AppCheckState::Amber]);
    }

    #[test]
    fn no_digest_or_an_unreadable_one_is_no_chip() {
        let dir = tempdir("apps-none");
        assert!(app_checks(&dir, "helsinki", now()).is_empty());
        let many: Vec<String> = (0..=MAX_PASSED)
            .map(|i| format!("helsinki/app-{i}"))
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        write(&dir, "apps.json", &apps_digest(NOW, &many));
        assert!(app_checks(&dir, "helsinki", now()).is_empty());
    }
}
