//! A run's tests, run where the model's code can do no harm (SDK-38, Architecture/20 §4.1).
//!
//! Each version that builds and holds tests is tested before the run offers publication, the way
//! the build lane tests it: one ConfigMap carries the version's files (gzipped JSON), one Job on
//! the lane's own builder image links the release's installed packages into it, runs `vitest`
//! and `tsc -b` once and prints one line, `JC-TESTS {json}`, which is read back from the pod's log. The
//! namespace is the fence: no traffic in or out, the restricted Pod Security Standard, a pod cap
//! (deployment `components/portal/charts/namespaces/templates/app-tests.yaml`). The pod holds no
//! token and no secret; the Job stops at 1 CPU, 1 GiB and [`DEADLINE_SECS`]. Both objects are
//! removed once the result is in, whatever it says.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::apps::kube::{KubeClient, KubeError};

/// Where the tests run and on what (SDK-38): the sandbox namespace and the lane's builder image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSettings {
    pub namespace: String,
    /// Pinned by digest; checked when the configuration is read.
    pub image: String,
    /// The rust runner (builder/Dockerfile `rust`), pinned by digest: a `wasm` App's version runs
    /// its server's `cargo test` there beside vitest (T-3618). `None` runs such a version on
    /// `image`, where the server's tests are left to the build lane and the report says so.
    pub rust_image: Option<String>,
}

/// How long one Job may run before Kubernetes stops it.
pub const DEADLINE_SECS: u64 = 170;
/// How long the Portal waits for a result, a little past the deadline so a stopped Job is seen.
const WAIT: Duration = Duration::from_secs(DEADLINE_SECS + 30);
const POLL: Duration = Duration::from_secs(2);
/// The vitest run inside the pod, shorter than the Job's deadline so a hung test is reported by
/// the sandbox itself rather than by a killed pod. The test Job carries it as:
///
/// - `JC_TEST_TIMEOUT_MS` — how many milliseconds the lane's `test-project` gives the whole test
///   run before it reports the tests as not finished.
const TEST_TIMEOUT_MS: u64 = 150_000;
/// A ConfigMap holds at most 1 MiB; the gzipped project leaves room for the object around it.
const MAX_PROJECT_BYTES: usize = 900 * 1024;
const LOG_BYTES: u64 = 256 * 1024;
const MARKER: &str = "JC-TESTS ";
pub const MAX_FAILURES: usize = 20;
pub const MAX_MESSAGE: usize = 2000;

/// What one run of a version's tests came to (API/04 §4, the `tests` event).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Passed,
    Failed,
    /// The sandbox could not run them: a timeout, no pod, an unreadable result.
    Error,
    /// Not run: no sandbox in this installation, or a version without tests.
    Skipped,
}

/// One failing test, as the model and the person read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub file: String,
    pub name: String,
    pub message: String,
}

/// The result of one run of a version's tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    pub outcome: Outcome,
    #[serde(default)]
    pub passed: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub failures: Vec<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Report {
    pub fn error(reason: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Error,
            passed: 0,
            failed: 0,
            failures: Vec::new(),
            duration_ms: None,
            reason: Some(reason.into()),
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Skipped,
            ..Self::error(reason)
        }
    }

    /// The report as the model is told it: each failure with its file, name and message.
    pub fn for_the_model(&self) -> Vec<String> {
        match self.outcome {
            Outcome::Failed => self
                .failures
                .iter()
                .map(|failure| {
                    format!(
                        "The test \"{}\" in {} fails: {}",
                        failure.name, failure.file, failure.message
                    )
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// One line for the conversation (AP-51).
    pub fn summary(&self) -> String {
        match self.outcome {
            Outcome::Passed => format!("The tests pass: {} of {}.", self.passed, self.passed),
            Outcome::Failed => {
                let names: Vec<&str> = self
                    .failures
                    .iter()
                    .take(5)
                    .map(|failure| failure.name.as_str())
                    .collect();
                format!(
                    "{} of {} tests fail: {}.",
                    self.failed,
                    self.passed + self.failed,
                    names.join("; ")
                )
            }
            Outcome::Error => format!(
                "The tests could not run: {}",
                self.reason.as_deref().unwrap_or("no reason given")
            ),
            Outcome::Skipped => format!(
                "The tests were not run: {}",
                self.reason.as_deref().unwrap_or("no reason given")
            ),
        }
    }

    /// Holds the report to the limits of API/04 §4 whatever the pod printed: the pod is where the
    /// model's code ran, so its output is untrusted text.
    fn bounded(mut self) -> Self {
        self.failures.truncate(MAX_FAILURES);
        for failure in &mut self.failures {
            cut(&mut failure.file, 300);
            cut(&mut failure.name, 300);
            cut(&mut failure.message, MAX_MESSAGE);
        }
        if let Some(reason) = self.reason.as_mut() {
            cut(reason, MAX_MESSAGE);
        }
        if self.outcome == Outcome::Passed && self.failed > 0 {
            self.outcome = Outcome::Failed;
        }
        self
    }
}

fn cut(text: &mut String, limit: usize) {
    if text.chars().count() > limit {
        *text = text.chars().take(limit).collect::<String>() + "…";
    }
}

/// Whether a version holds a test of its interface, its functions or its server.
pub fn has_tests(files: &BTreeMap<String, String>) -> bool {
    files.iter().any(|(path, content)| {
        ((path.starts_with("src/") || path.starts_with("functions/"))
            && (path.ends_with(".test.ts") || path.ends_with(".test.tsx")))
            || (path.starts_with("server/") && path.ends_with(".rs") && content.contains("#[test]"))
    })
}

/// Whether a version is a `wasm` App's, whose server's tests are `cargo test` (T-3618).
pub fn is_wasm(files: &BTreeMap<String, String>) -> bool {
    files.contains_key("server/Cargo.toml")
}

/// The version as the sandbox reads it: its files, gzipped JSON. The Portal's own files that
/// never run in a test (the workflow under `.gitea/`) stay out; the sandbox refuses any path
/// that is not plain.
pub fn project(files: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let kept: BTreeMap<&String, &String> = files
        .iter()
        .filter(|(path, _)| !path.split('/').any(|part| part.starts_with('.')))
        .collect();
    let json = serde_json::to_vec(&kept).map_err(|err| err.to_string())?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&json).map_err(|err| err.to_string())?;
    let bytes = gz.finish().map_err(|err| err.to_string())?;
    if bytes.len() > MAX_PROJECT_BYTES {
        return Err(format!(
            "the version is {} KiB compressed; the sandbox takes at most {} KiB",
            bytes.len() / 1024,
            MAX_PROJECT_BYTES / 1024
        ));
    }
    Ok(bytes)
}

/// The object name of one version's test: `app-test-{run}-{version}`, a DNS label.
pub fn object_name(run_id: &str, version: u32) -> String {
    let run: String = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(40)
        .collect();
    format!("app-test-{run}-{version}")
}

/// The ConfigMap and the Job of one version's test; a `wasm` version's Job runs on the rust
/// runner when there is one.
pub fn objects(
    settings: &SandboxSettings,
    name: &str,
    project: &[u8],
    wasm: bool,
) -> (Value, Value) {
    let image = match (&settings.rust_image, wasm) {
        (Some(rust), true) => rust,
        _ => &settings.image,
    };
    let labels = json!({
        "app.kubernetes.io/name": "app-test",
        "app.kubernetes.io/part-of": "joinedcontext",
        "joinedcontext.com/app-test": name,
    });
    let config_map = json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": { "name": name, "namespace": settings.namespace, "labels": labels },
        "binaryData": {
            "project.json.gz": base64::engine::general_purpose::STANDARD.encode(project)
        },
    });
    let job = json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": { "name": name, "namespace": settings.namespace, "labels": labels },
        "spec": {
            // One attempt: a second run of the same tests says nothing the first did not.
            "backoffLimit": 0,
            "activeDeadlineSeconds": DEADLINE_SECS,
            "ttlSecondsAfterFinished": 300,
            "template": {
                "metadata": { "labels": labels },
                "spec": {
                    "restartPolicy": "Never",
                    // No token, no Service addresses: the model's code learns nothing of the
                    // cluster, and the namespace admits no traffic anyway.
                    "automountServiceAccountToken": false,
                    "enableServiceLinks": false,
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": 1000,
                        "runAsGroup": 1000,
                        "fsGroup": 1000,
                        "seccompProfile": { "type": "RuntimeDefault" }
                    },
                    "containers": [{
                        "name": "tests",
                        "image": image,
                        "command": [
                            "node", "/opt/template/lane.mjs", "test-project",
                            "/project/project.json.gz", "/tmp/app"
                        ],
                        "env": [
                            { "name": "HOME", "value": "/tmp" },
                            { "name": "JC_TEST_TIMEOUT_MS", "value": TEST_TIMEOUT_MS.to_string() }
                        ],
                        "securityContext": {
                            "readOnlyRootFilesystem": true,
                            "allowPrivilegeEscalation": false,
                            "capabilities": { "drop": ["ALL"] }
                        },
                        "resources": {
                            "requests": { "cpu": "500m", "memory": "512Mi" },
                            "limits": { "cpu": "1", "memory": "1Gi", "ephemeral-storage": "1Gi" }
                        },
                        "volumeMounts": [
                            { "name": "project", "mountPath": "/project", "readOnly": true },
                            { "name": "tmp", "mountPath": "/tmp" }
                        ]
                    }],
                    "volumes": [
                        { "name": "project", "configMap": { "name": name } },
                        { "name": "tmp", "emptyDir": { "sizeLimit": "1Gi" } }
                    ]
                }
            }
        }
    });
    (config_map, job)
}

/// The sandbox's one result line in a pod's log: the last `JC-TESTS` line, which the sandbox
/// prints after the tests have ended.
// ponytail: the model's code runs as the same user as the printer and could write a line of its
// own to the pod's log; the last line is the sandbox's in every run that ends normally, and the
// build lane runs the tests again before anything is served (SDK-24). A nonce is the upgrade.
pub fn parse_log(log: &str) -> Report {
    let Some(line) = log
        .lines()
        .rev()
        .find_map(|line| line.trim_end().strip_prefix(MARKER))
    else {
        let tail: String = log
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" / ");
        return Report::error(format!(
            "the sandbox printed no result{}",
            if tail.trim().is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        ))
        .bounded();
    };
    match serde_json::from_str::<Report>(line) {
        Ok(report) => report.bounded(),
        Err(err) => Report::error(format!("the sandbox's result is not readable: {err}")).bounded(),
    }
}

/// Whether a Job has ended, from its status: `Some(true)` succeeded, `Some(false)` failed or was
/// stopped at its deadline, `None` still running.
fn ended(job: &Value) -> Option<bool> {
    let status = &job["status"];
    if status["succeeded"].as_u64().unwrap_or(0) > 0 {
        return Some(true);
    }
    if status["failed"].as_u64().unwrap_or(0) > 0 {
        return Some(false);
    }
    status["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|condition| {
            condition["status"] == "True"
                && matches!(condition["type"].as_str(), Some("Complete" | "Failed"))
        })
        .map(|condition| condition["type"] == "Complete")
}

fn said(object: &str, err: &KubeError) -> String {
    match err {
        KubeError::Api {
            status, message, ..
        } => {
            format!("the API server answered {status} for {object}: {message}")
        }
        other => format!("{object}: {other}"),
    }
}

/// The sandbox of one installation.
pub struct Sandbox {
    kube: KubeClient,
    settings: SandboxSettings,
}

impl Sandbox {
    pub fn new(kube: KubeClient, settings: SandboxSettings) -> Self {
        Self { kube, settings }
    }

    /// Runs one version's tests and answers what they came to. Never fails: what went wrong is
    /// the report's `error`, and both objects are removed on every path.
    pub async fn run(
        &self,
        run_id: &str,
        version: u32,
        files: &BTreeMap<String, String>,
    ) -> Report {
        self.run_with(run_id, version, files, WAIT, POLL).await
    }

    pub async fn run_with(
        &self,
        run_id: &str,
        version: u32,
        files: &BTreeMap<String, String>,
        wait: Duration,
        poll: Duration,
    ) -> Report {
        if !has_tests(files) {
            return Report::skipped("this version has no tests");
        }
        let started = std::time::Instant::now();
        let project = match project(files) {
            Ok(project) => project,
            Err(reason) => return Report::error(reason),
        };
        let name = object_name(run_id, version);
        let namespace = self.settings.namespace.clone();
        let (config_map, job) = objects(&self.settings, &name, &project, is_wasm(files));
        // A leftover of an earlier attempt with the same name (a Portal that restarted mid-test)
        // would make the create a 409; it is this run's own, so it goes first.
        self.remove(&namespace, &name).await;
        let mut report = match self.kube.create(&config_map).await {
            Err(err) => Report::error(said(&format!("ConfigMap {name}"), &err)),
            Ok(()) => match self.kube.create(&job).await {
                Err(err) => Report::error(said(&format!("Job {name}"), &err)),
                Ok(()) => self.result(&namespace, &name, wait, poll).await,
            },
        };
        self.remove(&namespace, &name).await;
        if report.duration_ms.is_none() {
            report.duration_ms =
                Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        }
        report
    }

    async fn result(&self, namespace: &str, name: &str, wait: Duration, poll: Duration) -> Report {
        let deadline = tokio::time::Instant::now() + wait;
        let succeeded = loop {
            match self.kube.get("batch/v1", "Job", namespace, name).await {
                Ok(Some(job)) => {
                    if let Some(succeeded) = ended(&job) {
                        break succeeded;
                    }
                }
                Ok(None) => return Report::error(format!("the Job {name} is gone")),
                Err(err) => return Report::error(said(&format!("Job {name}"), &err)),
            }
            if tokio::time::Instant::now() >= deadline {
                return Report::error(format!(
                    "the tests did not finish within {} s",
                    wait.as_secs()
                ));
            }
            tokio::time::sleep(poll).await;
        };
        let pods = match self
            .kube
            .pod_names(namespace, &format!("job-name={name}"))
            .await
        {
            Ok(pods) => pods,
            Err(err) => return Report::error(said(&format!("the pods of {name}"), &err)),
        };
        let Some(pod) = pods.first() else {
            return Report::error(if succeeded {
                format!("the Job {name} ended with no pod to read")
            } else {
                format!(
                    "the Job {name} stopped before its pod ran: past its {DEADLINE_SECS} s or \
                     refused by the namespace's quota"
                )
            });
        };
        match self.kube.pod_log(namespace, pod, LOG_BYTES).await {
            Ok(log) => parse_log(&log),
            Err(err) => Report::error(said(&format!("the log of {pod}"), &err)),
        }
    }

    async fn remove(&self, namespace: &str, name: &str) {
        for (api_version, kind) in [("batch/v1", "Job"), ("v1", "ConfigMap")] {
            if let Err(err) = self.kube.delete(api_version, kind, namespace, name).await {
                tracing::warn!(%name, %kind, error = %err, "a test sandbox object was not removed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> BTreeMap<String, String> {
        paths
            .iter()
            .map(|path| ((*path).to_owned(), format!("// {path}")))
            .collect()
    }

    #[test]
    fn a_version_has_tests_when_its_interface_or_its_functions_hold_one() {
        assert!(has_tests(&files(&[
            "src/App.tsx",
            "src/pages/Map.test.tsx"
        ])));
        assert!(has_tests(&files(&["functions/summary.test.ts"])));
        assert!(!has_tests(&files(&["src/App.tsx", "test-setup.ts"])));
        assert!(!has_tests(&files(&["README.test.ts", "src/App.tsx"])));
        assert!(!has_tests(&BTreeMap::new()));
        // A wasm App's server holds its tests in its Rust (T-3618).
        let mut server = files(&["server/Cargo.toml", "server/src/lib.rs"]);
        assert!(!has_tests(&server));
        server.insert(
            "server/src/lib.rs".into(),
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {}\n}\n".into(),
        );
        assert!(has_tests(&server));
        assert!(is_wasm(&server));
        assert!(!is_wasm(&files(&["src/App.tsx"])));
    }

    #[test]
    fn the_project_leaves_the_portals_dot_folders_out_and_refuses_what_a_config_map_cannot_hold() {
        let mut version = files(&["src/App.tsx", ".gitea/workflows/build.yml", "src/.cache/x"]);
        version.insert("package.json".into(), "{}".into());
        let gz = project(&version).expect("fits");
        let mut json = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&gz[..]), &mut json)
            .expect("gzip");
        let paths: Vec<String> = serde_json::from_str::<BTreeMap<String, String>>(&json)
            .expect("an object")
            .into_keys()
            .collect();
        assert_eq!(paths, ["package.json", "src/App.tsx"]);

        // Random text does not compress: past the limit, the sandbox is not asked at all.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let noise: String = (0..1_600_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(b'0' + (state % 64) as u8)
            })
            .collect();
        let big = BTreeMap::from([("src/big.ts".to_owned(), noise)]);
        assert!(project(&big)
            .expect_err("too big")
            .contains("at most 900 KiB"));
    }

    #[test]
    fn the_object_name_is_a_dns_label_of_the_run_and_the_version() {
        let name = object_name("01J8ZQ4T7K9M2N3P4Q5R6S7T8V", 3);
        assert_eq!(name, "app-test-01j8zq4t7k9m2n3p4q5r6s7t8v-3");
        assert!(crate::resource::is_dns1123(&name));
        assert!(crate::resource::is_dns1123(&object_name(
            &"x".repeat(200),
            u32::MAX
        )));
    }

    #[test]
    fn the_job_holds_no_token_and_runs_restricted_within_its_limits() {
        let settings = SandboxSettings {
            namespace: "dev-app-tests".into(),
            image: format!(
                "ghcr.io/x/joinedcontext-app-builder@sha256:{}",
                "a".repeat(64)
            ),
            rust_image: None,
        };
        let (config_map, job) = objects(&settings, "app-test-r-1", b"gz", false);
        assert_eq!(config_map["metadata"]["namespace"], "dev-app-tests");
        assert_eq!(config_map["binaryData"]["project.json.gz"], "Z3o=");
        let pod = &job["spec"]["template"]["spec"];
        assert_eq!(job["spec"]["backoffLimit"], 0);
        assert_eq!(job["spec"]["activeDeadlineSeconds"], 170);
        assert_eq!(pod["automountServiceAccountToken"], false);
        assert_eq!(pod["enableServiceLinks"], false);
        assert!(pod.get("serviceAccountName").is_none());
        assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
        assert_eq!(
            pod["securityContext"]["seccompProfile"]["type"],
            "RuntimeDefault"
        );
        let container = &pod["containers"][0];
        assert_eq!(container["image"], settings.image);
        assert_eq!(container["securityContext"]["readOnlyRootFilesystem"], true);
        assert_eq!(
            container["securityContext"]["allowPrivilegeEscalation"],
            false
        );
        assert_eq!(
            container["securityContext"]["capabilities"]["drop"],
            json!(["ALL"])
        );
        assert_eq!(container["resources"]["limits"]["cpu"], "1");
        assert_eq!(container["resources"]["limits"]["memory"], "1Gi");
        // Nothing of the Portal's in the pod: no secret, no env beyond the sandbox's own.
        let text = job.to_string();
        assert!(
            !text.contains("secretKeyRef") && !text.contains("secret\":"),
            "{text}"
        );
        let env: Vec<&str> = container["env"]
            .as_array()
            .expect("env")
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert_eq!(env, ["HOME", "JC_TEST_TIMEOUT_MS"]);
    }

    /// T-3618: a `wasm` version's Job runs on the rust runner, a `ui` version's on the lane's
    /// Node image; without a rust runner both run on the Node image.
    #[test]
    fn a_wasm_version_is_tested_on_the_rust_runner_and_a_ui_version_on_the_node_image() {
        let node = format!(
            "ghcr.io/x/joinedcontext-app-builder@sha256:{}",
            "a".repeat(64)
        );
        let rust = format!(
            "ghcr.io/x/joinedcontext-app-builder@sha256:{}",
            "b".repeat(64)
        );
        let image = |rust_image: Option<&String>, wasm: bool| {
            let settings = SandboxSettings {
                namespace: "dev-app-tests".into(),
                image: node.clone(),
                rust_image: rust_image.cloned(),
            };
            let (_, job) = objects(&settings, "app-test-r-1", b"gz", wasm);
            job["spec"]["template"]["spec"]["containers"][0]["image"].clone()
        };
        assert_eq!(image(Some(&rust), true), json!(rust));
        assert_eq!(image(Some(&rust), false), json!(node));
        assert_eq!(image(None, true), json!(node));
    }

    #[test]
    fn the_last_result_line_is_the_report_and_its_limits_hold() {
        let log = "== tests\nJC-TESTS {\"outcome\":\"passed\",\"passed\":9}\nnoise\nJC-TESTS {\"outcome\":\"failed\",\"passed\":1,\"failed\":1,\"failures\":[{\"file\":\"src/a.test.ts\",\"name\":\"a\",\"message\":\"boom\"}],\"durationMs\":4200}\n";
        let report = parse_log(log);
        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(report.failed, 1);
        assert_eq!(report.duration_ms, Some(4200));
        assert_eq!(
            report.for_the_model(),
            ["The test \"a\" in src/a.test.ts fails: boom"]
        );
        assert_eq!(report.summary(), "1 of 2 tests fail: a.");

        let many: Vec<Value> = (0..40)
            .map(|i| json!({"file": "f", "name": format!("t{i}"), "message": "m".repeat(5000)}))
            .collect();
        let line = json!({"outcome": "passed", "passed": 0, "failed": 40, "failures": many});
        let report = parse_log(&format!("JC-TESTS {line}"));
        assert_eq!(
            report.outcome,
            Outcome::Failed,
            "a pass with failures is a failure"
        );
        assert_eq!(report.failures.len(), MAX_FAILURES);
        assert_eq!(report.failures[0].message.chars().count(), MAX_MESSAGE + 1);
    }

    #[test]
    fn a_log_without_a_readable_result_is_an_error_that_says_what_the_pod_printed() {
        let report = parse_log("npm ERR! cannot\nkilled\n");
        assert_eq!(report.outcome, Outcome::Error);
        assert_eq!(
            report.reason.as_deref(),
            Some("the sandbox printed no result: npm ERR! cannot / killed")
        );
        assert_eq!(
            parse_log("").reason.as_deref(),
            Some("the sandbox printed no result")
        );
        let report = parse_log("JC-TESTS {\"outcome\":\"passed\",\"extra\":1}");
        assert_eq!(report.outcome, Outcome::Error);
        assert!(report.for_the_model().is_empty());
        assert!(report.reason.expect("a reason").contains("not readable"));
    }

    #[test]
    fn a_job_has_ended_by_its_counts_or_its_conditions() {
        assert_eq!(ended(&json!({"status": {"succeeded": 1}})), Some(true));
        assert_eq!(ended(&json!({"status": {"failed": 1}})), Some(false));
        assert_eq!(
            ended(
                &json!({"status": {"conditions": [{"type": "Failed", "status": "True", "reason": "DeadlineExceeded"}]}})
            ),
            Some(false)
        );
        assert_eq!(ended(&json!({"status": {"active": 1}})), None);
        assert_eq!(ended(&json!({})), None);
    }
}
