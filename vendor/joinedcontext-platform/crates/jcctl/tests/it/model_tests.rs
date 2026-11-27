//! `jcctl model` against a Model Tools that answers from a script (T-0406, DM-02, DM-19, DM-32).
//!
//! The fake speaks the same HTTP the image speaks — one answer per connection, always
//! `Content-Length`, then close — so these exercise the real client, the real repository walk
//! and the real exit codes rather than a mock of them.

use crate::common;

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};

const PIN: &str = "linkml-1.11.1";
const MODEL_PATH: &str = "projects/ovzdusie/spaces/ovzdusie/datamodels/parking-spot.yaml";
const MODEL_DIR: &str = "projects/ovzdusie/spaces/ovzdusie/datamodels";

const MODEL: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: DataModel
metadata:
  name: parking-spot
  namespace: ovzdusie
spec:
  contextSpaceRef: ovzdusie
  linkml: parking-spot.linkml.yaml
  version: 1.0.0
  lifecycle: draft
  classes: ["ParkingSpot"]
  artifacts:
    jsonSchema: json-schema/parking-spot.v1.json
    context: context/parking-spot.v1.jsonld
    docs: docs/parking-spot.md
    example: examples/parking-spot.example.jsonld
"#;

const SOURCE: &str = "id: https://banskabystrica.sk/parking\nname: parking\n";

/// What a Model Tools that renders the whole DM-02 set answers.
fn full_answer() -> String {
    json!({
        "generatorVersion": PIN,
        "jsonSchema": {"$schema": "http://json-schema.org/draft-07/schema#", "title": "ParkingSpot"},
        "context": {"@context": {"status": "https://banskabystrica.sk/parking/status"}},
        "docs": "# ParkingSpot\n\nOne parking spot.\n",
        "example": {"id": "urn:ngsi-ld:ParkingSpot:banskabystrica.sk:parking:1"},
        "shacl": "@prefix sh: <http://www.w3.org/ns/shacl#> .\n",
        "owl": "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n",
        "errors": []
    })
    .to_string()
}

/// A Model Tools that answers `routes` and 404s everything else. The thread is detached; the
/// test binary is what ends it.
fn spawn(routes: Vec<(&'static str, String)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let request = read_request(&mut stream);
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let (status, body) = match routes.iter().find(|(route, _)| *route == path) {
                Some((_, body)) => (200, body.clone()),
                None => (404, json!({"errors": [format!("no {path}")]}).to_string()),
            };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Reads one request: the head, then as many body bytes as `Content-Length` announced.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(read) = stream.read(&mut chunk) {
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&raw).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let announced: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().to_owned())
                })
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            if body.len() >= announced {
                break;
            }
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

/// A repository holding one DataModel, its source and the pin.
fn repo(test: &str, pin: &str) -> std::path::PathBuf {
    let dir = common::demo_repo(test);
    common::write(&dir, MODEL_PATH, MODEL);
    common::write(
        &dir,
        &format!("{MODEL_DIR}/parking-spot.linkml.yaml"),
        SOURCE,
    );
    common::write(
        &dir,
        "platform-settings.yaml",
        &format!("retention:\n  history: 90d\nmodelTools:\n  image: ghcr.io/x@sha256:0\n  generatorVersion: {pin}\n"),
    );
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jcctl"))
        .args(args)
        .env_remove("JC_MODEL_TOOLS_URL")
        .output()
        .expect("jcctl runs")
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is one JSON document ({err}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn artifact(dir: &Path, relative: &str) -> String {
    std::fs::read_to_string(dir.join(MODEL_DIR).join(relative))
        .unwrap_or_else(|err| panic!("{relative}: {err}"))
}

#[test]
fn generate_writes_the_four_committed_artifacts() {
    let dir = repo("model-generate", PIN);
    let url = spawn(vec![
        (
            "/healthz",
            json!({"status": "ok", "generatorVersion": PIN}).to_string(),
        ),
        ("/generate", full_answer()),
    ]);

    let output = run(&[
        "model",
        "generate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = report(&output);
    assert_eq!(json["generatorVersion"], PIN);
    assert_eq!(json["models"][0]["written"].as_array().unwrap().len(), 4);
    assert_eq!(json["stale"], 0);
    assert_eq!(json["failed"], 0);

    // JSON artifacts are written pretty-printed and Markdown as it came, both ending in one
    // newline, so a review reads a diff and not a reflowed line.
    assert!(artifact(&dir, "json-schema/parking-spot.v1.json")
        .contains("\n  \"title\": \"ParkingSpot\""));
    assert!(artifact(&dir, "context/parking-spot.v1.jsonld").starts_with("{\n  \"@context\""));
    assert_eq!(
        artifact(&dir, "docs/parking-spot.md"),
        "# ParkingSpot\n\nOne parking spot.\n"
    );
    assert!(
        artifact(&dir, "examples/parking-spot.example.jsonld").contains("urn:ngsi-ld:ParkingSpot")
    );

    // SHACL and OWL are rendered and are not part of the committed set (DM-02, DM-44).
    assert!(!dir.join(MODEL_DIR).join("shacl").exists());
}

#[test]
fn generate_is_idempotent_and_rewrites_nothing_it_already_matches() {
    let dir = repo("model-generate-twice", PIN);
    let url = spawn(vec![
        ("/healthz", json!({"generatorVersion": PIN}).to_string()),
        ("/generate", full_answer()),
    ]);
    let args = [
        "model",
        "generate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ];

    assert!(run(&args).status.success());
    let second = run(&args);
    assert!(second.status.success());
    assert!(
        report(&second)["models"][0]["written"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a second run writes nothing"
    );
}

#[test]
fn diff_names_the_stale_artifact_and_exits_two() {
    let dir = repo("model-diff", PIN);
    let url = spawn(vec![
        ("/healthz", json!({"generatorVersion": PIN}).to_string()),
        ("/generate", full_answer()),
    ]);
    let repo_dir = dir.to_str().unwrap().to_owned();

    assert!(
        run(&["model", "generate", "--repo-dir", &repo_dir, "--url", &url])
            .status
            .success()
    );
    let clean = run(&["model", "diff", "--repo-dir", &repo_dir, "--url", &url]);
    assert!(
        clean.status.success(),
        "a fresh repository has no stale artifact"
    );

    // What a hand-edited artifact looks like: DM-01 says these are generated, never edited.
    std::fs::write(
        dir.join(MODEL_DIR).join("docs/parking-spot.md"),
        "# edited by hand\n",
    )
    .unwrap();

    let output = run(&["model", "diff", "--repo-dir", &repo_dir, "--url", &url]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a stale artifact is a pending change"
    );
    let json = report(&output);
    assert_eq!(json["stale"], 1);
    assert_eq!(
        json["models"][0]["stale"][0],
        format!("{MODEL_DIR}/docs/parking-spot.md")
    );
    // `diff` reports; it does not fix.
    assert_eq!(artifact(&dir, "docs/parking-spot.md"), "# edited by hand\n");
}

#[test]
fn a_model_tools_of_another_version_than_the_pin_is_refused() {
    let dir = repo("model-pin", PIN);
    let url = spawn(vec![
        (
            "/healthz",
            json!({"generatorVersion": "linkml-1.9.0"}).to_string(),
        ),
        ("/generate", full_answer()),
    ]);

    let output = run(&[
        "model",
        "generate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(PIN) && stderr.contains("linkml-1.9.0"),
        "{stderr}"
    );
    assert!(stderr.contains("DM-19"), "{stderr}");
    assert!(
        !dir.join(MODEL_DIR).join("docs/parking-spot.md").exists(),
        "nothing is written before the pin is checked"
    );
}

#[test]
fn a_repository_with_no_pin_cannot_be_generated_into() {
    let dir = repo("model-no-pin", PIN);
    std::fs::remove_file(dir.join("platform-settings.yaml")).unwrap();
    let url = spawn(vec![(
        "/healthz",
        json!({"generatorVersion": PIN}).to_string(),
    )]);

    let output = run(&[
        "model",
        "generate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("DM-19"));
}

#[test]
fn a_declared_artifact_the_service_does_not_render_fails_the_run() {
    // A Model Tools older than the four DM-02 artifacts, or one whose renderer for an artifact
    // failed: the declared file must fail the run by name rather than be committed empty.
    let dir = repo("model-missing", PIN);
    let url = spawn(vec![
        ("/healthz", json!({"generatorVersion": PIN}).to_string()),
        (
            "/generate",
            json!({
                "generatorVersion": PIN,
                "jsonSchema": {"title": "ParkingSpot"},
                "context": {"@context": {}},
                "shacl": "",
                "owl": "",
                "errors": []
            })
            .to_string(),
        ),
    ]);

    let output = run(&[
        "model",
        "generate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert_eq!(output.status.code(), Some(1));
    let json = report(&output);
    assert_eq!(json["failed"], 1);
    assert_eq!(
        json["models"][0]["missing"],
        json!([
            format!("{MODEL_DIR}/docs/parking-spot.md"),
            format!("{MODEL_DIR}/examples/parking-spot.example.jsonld")
        ])
    );
    // The two it did render are still written: a partial set is visible, not silently absent.
    assert!(dir
        .join(MODEL_DIR)
        .join("json-schema/parking-spot.v1.json")
        .exists());
}

#[test]
fn validate_reports_what_does_not_compile_and_writes_nothing() {
    let dir = repo("model-validate", PIN);
    let url = spawn(vec![
        // Another version on purpose: `validate` reads no artifact and writes none, so it is
        // not the command a wrong generator can corrupt anything with.
        (
            "/healthz",
            json!({"generatorVersion": "linkml-1.9.0"}).to_string(),
        ),
        (
            "/generate",
            json!({
                "generatorVersion": "linkml-1.9.0",
                "errors": ["parking-spot.linkml.yaml: slot `status` has no range"]
            })
            .to_string(),
        ),
    ]);

    let output = run(&[
        "model",
        "validate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert_eq!(output.status.code(), Some(1));
    let json = report(&output);
    assert_eq!(
        json["models"][0]["errors"][0],
        "parking-spot.linkml.yaml: slot `status` has no range"
    );
    assert!(!dir.join(MODEL_DIR).join("json-schema").exists());
}

#[test]
fn a_model_that_does_not_compile_writes_no_half_set() {
    let dir = repo("model-broken", PIN);
    let url = spawn(vec![
        ("/healthz", json!({"generatorVersion": PIN}).to_string()),
        (
            "/generate",
            json!({
                "generatorVersion": PIN,
                "jsonSchema": {"title": "half"},
                "errors": ["the source does not parse"]
            })
            .to_string(),
        ),
    ]);

    let output = run(&[
        "model",
        "generate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!dir.join(MODEL_DIR).join("json-schema").exists());
}

#[test]
fn import_writes_the_linkml_a_catalogue_model_becomes() {
    let dir = common::temp_dir("model-import");
    let out = dir.join("air-quality.linkml.yaml");
    let url = spawn(vec![
        ("/healthz", json!({"generatorVersion": PIN}).to_string()),
        (
            "/import-sdm",
            json!({
                "generatorVersion": PIN,
                "linkml": "id: https://smart-data-models.github.io/air\nname: air\n",
                "jsonSchema": {},
                "errors": []
            })
            .to_string(),
        ),
    ]);

    let output = run(&[
        "model",
        "import",
        "dataModel.Environment/AirQualityObserved",
        "--out",
        out.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "id: https://smart-data-models.github.io/air\nname: air\n"
    );
    assert_eq!(
        report(&output)["model"],
        "dataModel.Environment/AirQualityObserved"
    );
}

#[test]
fn an_unreachable_model_tools_is_an_error_and_not_an_empty_report() {
    let dir = repo("model-unreachable", PIN);
    // Port 1 is reserved and nothing listens on it.
    let output = run(&[
        "model",
        "diff",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        "http://127.0.0.1:1",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot reach Model Tools"));
}

/// An organization model the space's model imports (DM-75, DM-76).
const ORGANIZATION_MODEL: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: DataModel
metadata:
  name: stations
  namespace: org
spec:
  linkml: stations.linkml.yaml
  version: 1.3.0
  lifecycle: published
  classes: ["Station"]
  artifacts:
    jsonSchema: json-schema/stations.v1.json
    context: context/stations.v1.jsonld
    docs: docs/stations.md
    example: examples/stations.example.jsonld
"#;
const ORGANIZATION_SOURCE: &str = "id: https://example.org/stations\nname: stations\n";
const IMPORTING_SOURCE: &str =
    "id: https://banskabystrica.sk/parking\nname: parking\nimports: [linkml:types, org.stations.v1]\n";

/// A Model Tools like [`spawn`] that also keeps every `/generate` body it was sent.
fn spawn_recording() -> (String, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = std::sync::Arc::clone(&bodies);
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let request = read_request(&mut stream);
            let body = if request.starts_with("POST /generate") {
                if let Some((_, sent)) = request.split_once("\r\n\r\n") {
                    if let Ok(sent) = serde_json::from_str(sent) {
                        kept.lock().expect("not poisoned").push(sent);
                    }
                }
                full_answer()
            } else {
                json!({"status": "ok", "generatorVersion": PIN}).to_string()
            };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (format!("http://127.0.0.1:{port}"), bodies)
}

fn with_organization_model(dir: &Path) {
    common::write(dir, "datamodels/stations/stations.yaml", ORGANIZATION_MODEL);
    common::write(
        dir,
        "datamodels/stations/stations.linkml.yaml",
        ORGANIZATION_SOURCE,
    );
}

fn sent_imports(bodies: &std::sync::Mutex<Vec<Value>>) -> Vec<Value> {
    bodies
        .lock()
        .expect("not poisoned")
        .iter()
        .filter_map(|body| body.get("imports").cloned())
        .collect()
}

#[test]
fn generate_hands_model_tools_the_organization_model_a_space_imports_dm75() {
    let dir = repo("model-generate-imports", PIN);
    common::write(
        &dir,
        &format!("{MODEL_DIR}/parking-spot.linkml.yaml"),
        IMPORTING_SOURCE,
    );
    with_organization_model(&dir);
    let (url, bodies) = spawn_recording();

    let output = run(&[
        "model",
        "generate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        sent_imports(&bodies),
        [json!({ "org.stations.v1": ORGANIZATION_SOURCE })],
        "only the importing model sends imports, and exactly the one it names"
    );
}

#[test]
fn a_project_repository_reads_organization_models_from_the_org_dir_dm75() {
    let dir = repo("model-generate-org-dir", PIN);
    common::write(
        &dir,
        &format!("{MODEL_DIR}/parking-spot.linkml.yaml"),
        IMPORTING_SOURCE,
    );
    let organization = common::demo_repo("model-generate-org-dir-organization");
    with_organization_model(&organization);
    let (url, bodies) = spawn_recording();

    // Without the organization checkout the import has nothing to resolve to.
    let output = run(&[
        "model",
        "validate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert!(!output.status.success());
    let errors = report(&output)["models"][0]["errors"].to_string();
    assert!(
        errors.contains("org.stations.v1") && errors.contains("--org-dir"),
        "{errors}"
    );

    let output = run(&[
        "model",
        "validate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--org-dir",
        organization.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        sent_imports(&bodies),
        [json!({ "org.stations.v1": ORGANIZATION_SOURCE })]
    );
}

#[test]
fn an_import_pinned_to_another_major_or_to_a_draft_fails_the_model_dm22() {
    let dir = repo("model-generate-import-major", PIN);
    common::write(
        &dir,
        &format!("{MODEL_DIR}/parking-spot.linkml.yaml"),
        &IMPORTING_SOURCE.replace("org.stations.v1", "org.stations.v2"),
    );
    with_organization_model(&dir);
    let (url, bodies) = spawn_recording();

    let output = run(&[
        "model",
        "validate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert!(!output.status.success());
    let errors = report(&output)["models"].to_string();
    assert!(
        errors.contains("is at version 1.3.0, and the import pins major 2"),
        "{errors}"
    );
    assert!(
        sent_imports(&bodies).is_empty(),
        "nothing unresolved reaches Model Tools"
    );

    common::write(
        &dir,
        &format!("{MODEL_DIR}/parking-spot.linkml.yaml"),
        IMPORTING_SOURCE,
    );
    common::write(
        &dir,
        "datamodels/stations/stations.yaml",
        &ORGANIZATION_MODEL.replace("lifecycle: published", "lifecycle: draft"),
    );
    let output = run(&[
        "model",
        "validate",
        "--repo-dir",
        dir.to_str().unwrap(),
        "--url",
        &url,
    ]);
    assert!(!output.status.success());
    assert!(report(&output)["models"].to_string().contains("is a draft"));
}

/// A DataModel `name` in `namespace` under `rel_dir`, with `spec` as its whole spec block and,
/// when given, its LinkML source beside it as `{name}.linkml.yaml`.
fn data_model(
    dir: &Path,
    rel_dir: &str,
    name: &str,
    namespace: &str,
    spec: &str,
    source: Option<&str>,
) {
    common::write(
        dir,
        &format!("{rel_dir}/{name}.yaml"),
        &format!("apiVersion: joinedcontext.com/v1alpha1\nkind: DataModel\nmetadata:\n  name: {name}\n  namespace: {namespace}\nspec:\n{spec}"),
    );
    if let Some(source) = source {
        common::write(dir, &format!("{rel_dir}/{name}.linkml.yaml"), source);
    }
}

/// A published model's spec at major 1, its source `{name}.linkml.yaml`, only a JSON Schema.
fn published(name: &str) -> String {
    format!("  contextSpaceRef: ovzdusie\n  linkml: {name}.linkml.yaml\n  version: 1.0.0\n  lifecycle: published\n  classes: [\"Zone\"]\n  artifacts:\n    jsonSchema: json-schema/{name}.v1.json\n")
}

fn validate(dir: &Path, url: &str, source: &str) -> String {
    common::write(
        dir,
        &format!("{MODEL_DIR}/parking-spot.linkml.yaml"),
        source,
    );
    let tools = jcctl::model::ModelTools::new(url);
    jcctl::model::run(dir, None, &tools, jcctl::model::Mode::Validate)
        .expect("the run reports")
        .to_json()
        .to_string()
}

/// DM-76, DM-22: each way an import resolves or fails names the import; a model that does not
/// read or leaves its directory is reported, not fatal; a repository or organization checkout
/// that is not there stops the run.
#[test]
fn every_import_outcome_and_every_unreadable_model_is_reported() {
    let dir = repo("model-import-outcomes", PIN);
    with_organization_model(&dir);
    data_model(
        &dir,
        MODEL_DIR,
        "zones",
        "ovzdusie",
        &published("zones"),
        Some("id: https://example.org/zones\nname: zones\n"),
    );
    data_model(
        &dir,
        MODEL_DIR,
        "broken",
        "ovzdusie",
        "  version: banana\n",
        None,
    );
    data_model(
        &dir,
        MODEL_DIR,
        "outside",
        "ovzdusie",
        &published("outside").replace(
            "linkml: outside.linkml.yaml",
            "linkml: ../outside.linkml.yaml",
        ),
        None,
    );
    let (url, _) = spawn_recording();

    let said = validate(
        &dir,
        &url,
        "name: parking\nimports: [project.zones.v1, org.stations.v1, org.stations.v1]\n",
    );
    assert!(said.contains("broken.yaml"), "{said}");
    assert!(said.contains("outside.yaml"), "{said}");
    let parking = |said: &str| {
        let report: Value = serde_json::from_str(said).expect("json");
        report["models"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| {
                m["id"]
                    .as_str()
                    .is_some_and(|id| id.contains("parking-spot"))
            })
            .map(|m| m["errors"].to_string())
            .unwrap_or_default()
    };
    assert_eq!(parking(&said), "[]", "{said}");
    for (imports, expected) in [
        ("project.nothing.v1", "no model of this project `nothing`"),
        ("project.broken.v1", "broken.yaml"),
        ("project.outside.v1", "import 'project.outside.v1'"),
        ("org.Bad.v1", "a model import is"),
    ] {
        let said = validate(
            &dir,
            &url,
            &format!("name: parking\nimports: [{imports}]\n"),
        );
        assert!(parking(&said).contains(expected), "{imports}: {said}");
    }
    // An organization model imports organization models only; one that does not parse is
    // Model Tools' to report.
    common::write(
        &dir,
        "datamodels/stations/stations.linkml.yaml",
        "name: stations\nimports: [project.zones.v1]\n",
    );
    let said = validate(&dir, &url, IMPORTING_SOURCE);
    assert!(
        parking(&said).contains("organization models only"),
        "{said}"
    );
    common::write(&dir, "datamodels/stations/stations.linkml.yaml", "a: [");
    let said = validate(&dir, &url, IMPORTING_SOURCE);
    assert_eq!(parking(&said), "[]", "{said}");
    let said = validate(&dir, &url, "a: [");
    assert_eq!(parking(&said), "[]", "{said}");

    let tools = jcctl::model::ModelTools::new(&url);
    let gone = dir.join("not-there");
    for (repo_dir, org_dir) in [
        (gone.as_path(), None),
        (dir.as_path(), Some(gone.as_path())),
    ] {
        let got = jcctl::model::run(repo_dir, org_dir, &tools, jcctl::model::Mode::Validate);
        assert!(
            matches!(got, Err(jcctl::model::Error::Repository(_))),
            "{got:?}"
        );
    }
}

/// DM-02: an artifact the model does not declare is not written; a target that cannot be
/// created or written stops the run naming it.
#[test]
fn generate_skips_an_undeclared_artifact_and_names_one_it_cannot_write() {
    let dir = repo("model-generate-write-failures", PIN);
    std::fs::remove_file(dir.join(MODEL_PATH)).expect("the default model goes");
    data_model(
        &dir,
        MODEL_DIR,
        "zones",
        "ovzdusie",
        &published("zones").replace("published", "draft"),
        Some(SOURCE),
    );
    let (url, _) = spawn_recording();
    let tools = jcctl::model::ModelTools::new(&url);
    let generate = || jcctl::model::run(&dir, None, &tools, jcctl::model::Mode::Generate);
    let report = generate().expect("the run reports").to_json();
    assert_eq!(
        report["models"][0]["written"],
        json!([format!("{MODEL_DIR}/json-schema/zones.v1.json")]),
        "{report}"
    );

    let target = dir.join(MODEL_DIR).join("json-schema");
    std::fs::remove_dir_all(&target).expect("the artifact dir goes");
    std::fs::write(&target, "a file where a directory goes").expect("a blocking file");
    let got = generate();
    assert!(
        matches!(got, Err(jcctl::model::Error::Io { .. })),
        "{got:?}"
    );
    std::fs::remove_file(&target).expect("the blocking file goes");
    std::fs::create_dir_all(target.join("zones.v1.json")).expect("a directory where the file goes");
    let got = generate();
    assert!(
        matches!(got, Err(jcctl::model::Error::Io { .. })),
        "{got:?}"
    );
}

/// DM-07: an import into a path that is no file is an error naming it.
#[test]
fn an_import_into_the_root_directory_is_an_io_error() {
    let url = spawn(vec![(
        "/import-sdm",
        json!({ "generatorVersion": PIN, "linkml": "name: air\n", "errors": [] }).to_string(),
    )]);
    let tools = jcctl::model::ModelTools::new(&url);
    let got = jcctl::model::import(&tools, "sdm/Air", Path::new("/"));
    assert!(
        matches!(got, Err(jcctl::model::Error::Io { .. })),
        "{got:?}"
    );
}

/// MF-49: what an export carries of the organization's models, and each way it is refused.
#[test]
fn organization_imports_carry_what_resolves_and_refuse_what_does_not() {
    let organization = common::temp_dir("model-org-imports-organization");
    with_organization_model(&organization);
    let project = |test: &str, source: &str| {
        let dir = common::demo_repo(test);
        data_model(
            &dir,
            MODEL_DIR,
            "zones",
            "ovzdusie",
            &published("zones"),
            Some(source),
        );
        data_model(
            &dir,
            MODEL_DIR,
            "broken",
            "ovzdusie",
            "  version: banana\n",
            None,
        );
        dir
    };
    let carried = |dir: &Path, org: Option<&Path>| jcctl::model::organization_imports(dir, org);

    let dir = project(
        "model-org-imports-ok",
        "name: zones\nimports: [project.other.v1, org.stations.v1]\n",
    );
    let other = common::demo_repo("model-org-imports-unparsed");
    data_model(
        &other,
        MODEL_DIR,
        "zones",
        "ovzdusie",
        &published("zones"),
        Some("a: ["),
    );
    let got = carried(&dir, Some(&organization)).expect("the import resolves");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].name, "stations");
    assert_eq!(carried(&other, None), Ok(Vec::new()));

    // Twice the same organization model, transitively: carried once.
    common::write(
        &organization,
        "datamodels/stations/stations.linkml.yaml",
        "name: stations\nimports: [org.stations.v1]\n",
    );
    assert_eq!(carried(&dir, Some(&organization)).map(|c| c.len()), Ok(1));

    // Two organization models, the one imported importing the other: the organization checkout
    // read for the first serves the second, and both are carried.
    let regions = ORGANIZATION_MODEL
        .replace("stations", "regions")
        .replace("Station", "Region");
    common::write(&organization, "datamodels/regions/regions.yaml", &regions);
    common::write(
        &organization,
        "datamodels/regions/regions.linkml.yaml",
        "name: regions\n",
    );
    common::write(
        &organization,
        "datamodels/stations/stations.linkml.yaml",
        "name: stations\nimports: [org.regions.v1]\n",
    );
    let names = carried(&dir, Some(&organization)).map(|models| {
        models
            .into_iter()
            .map(|model| model.name)
            .collect::<Vec<_>>()
    });
    assert_eq!(names, Ok(vec!["regions".to_owned(), "stations".to_owned()]));

    for (source, org, expected) in [
        (
            "name: zones\nimports: [org.Bad.v1]\n",
            Some(organization.as_path()),
            "a model import is",
        ),
        (
            "name: zones\nimports: [org.nothing.v1]\n",
            Some(organization.as_path()),
            "import 'org.nothing.v1'",
        ),
        (
            "name: zones\nimports: [org.stations.v1]\n",
            Some(Path::new("/nonexistent-jcctl-org")),
            "",
        ),
    ] {
        let dir = project("model-org-imports-refused", source);
        let got = carried(&dir, org);
        assert!(
            got.as_ref().is_err_and(|why| why.contains(expected)),
            "{source}: {got:?}"
        );
    }
    std::fs::remove_file(organization.join("datamodels/stations/stations.linkml.yaml"))
        .expect("the source goes");
    assert!(carried(&dir, Some(&organization)).is_err());
    let lost = common::demo_repo("model-org-imports-lost");
    data_model(
        &lost,
        MODEL_DIR,
        "zones",
        "ovzdusie",
        &published("zones"),
        None,
    );
    assert!(carried(&lost, None).is_err());
    assert!(carried(&lost.join("not-there"), None).is_err());
}
