//! Dumps every seed pipeline as the Portal hands it to its runner, with the Portal's own code:
//! `StreamDeployer::converge` over a `Mirror` loaded from the seed, every queued run started at
//! once, against a capture server standing in for the runner. Each stream the Portal PUTs or
//! POSTs is one JSON line `{"project", "name", "id", "method", "stream"}` on stdout; every other
//! call it makes is a line `{"call": "<METHOD> <path>"}`, and each outcome a line
//! `{"project", "name", "outcome"}`.
//!
//! Usage: portal-dump <seed dir> (each child directory is one project).
//!
//! Set as the Portal's main.rs sets it on dev (`JC_PORTAL_PIPELINE_IDENTITY=true`): pipeline
//! identity on, validation on with the seed's compiled models.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use joinedcontext_portal::pipeline_validation::{ModelSchema, ModelSchemas};
use joinedcontext_portal::reconciler::streams::{Bentos, StreamDeployer, StreamOutcome};
use joinedcontext_portal::resource::ResourceEnvelope;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};

/// The installation domain both sides see; the Portal fills `{orgDomain}` before it parses.
const ORG_DOMAIN: &str = "example.org";
/// Where the stream's run log and refusals go (the Portal's internal listener).
const INTERNAL: &str = "http://portal-internal.test";

type Calls = Arc<Mutex<Vec<(String, String, Bytes)>>>;

async fn capture(
    State(calls): State<Calls>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> (StatusCode, String) {
    let path = uri.path().to_owned();
    let answer = match (&method, path.as_str()) {
        (&Method::GET, "/streams") => (StatusCode::OK, "{}".to_owned()),
        // Every run has ended by the time the queue asks: the slot frees at once.
        (&Method::GET, p) if p.starts_with("/streams/") => (StatusCode::NOT_FOUND, String::new()),
        _ => (StatusCode::OK, "{}".to_owned()),
    };
    calls
        .lock()
        .unwrap()
        .push((method.to_string(), uri.to_string(), body));
    answer
}

#[tokio::main]
async fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: portal-dump <seed dir>");
        std::process::exit(2);
    };
    let mirror = Mirror::new();
    let mut files: HashMap<(String, String, String), PathBuf> = HashMap::new();
    let mut projects: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("seed dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    projects.sort();
    for dir in &projects {
        load(dir, &mirror, &mut files);
    }
    let schemas = Arc::new(ModelSchemas::default());
    schemas.replace(compile_models(&mirror, &files));

    let calls: Calls = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runner = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .fallback(capture)
        .with_state(Arc::clone(&calls));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let deployer = StreamDeployer::new(runner)
        .with_pipeline_identity(true)
        .with_concurrency(10_000, 10_000)
        .with_validation(INTERNAL, schemas);
    let outcomes = deployer
        .converge(&mirror, &bentos(&files), &BTreeMap::new())
        .await;
    for (project, name, outcome) in &outcomes {
        if matches!(outcome, StreamOutcome::Scheduled(..)) {
            let _ = deployer.run_now(project, name);
        }
    }
    // Each tick sends what is due; a few rounds let every queued run start once.
    for _ in 0..3 {
        deployer.tick_queue(chrono::Utc::now()).await;
    }
    for (project, name, outcome) in &outcomes {
        println!(
            "{}",
            json!({ "project": project, "name": name, "outcome": format!("{outcome:?}") })
        );
    }
    let mut seen = std::collections::HashSet::new();
    for (method, path, body) in calls.lock().unwrap().iter() {
        let id = path
            .strip_prefix("/streams/")
            .filter(|id| !id.contains('/'));
        match (method.as_str(), id) {
            ("PUT" | "POST", Some(id)) if seen.insert(id.to_owned()) => {
                let stream: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                let (project, name) = id.split_once('.').unwrap_or(("", id));
                println!(
                    "{}",
                    json!({ "project": project, "name": name, "id": id, "method": method, "stream": stream })
                );
            }
            _ => println!("{}", json!({ "call": format!("{method} {path}") })),
        }
    }
}

/// Every manifest of one project directory into the mirror, as the forge would hold it.
fn load(dir: &Path, mirror: &Mirror, files: &mut HashMap<(String, String, String), PathBuf>) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.sort();
    for path in paths {
        let file = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !file.ends_with(".yaml") || file.ends_with("-bento.yaml") || file.contains(".linkml.") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = text.replace("{orgDomain}", ORG_DOMAIN);
        for doc in serde_norway::Deserializer::from_str(&text) {
            let Ok(value) = <Value as serde::Deserialize>::deserialize(doc) else {
                continue;
            };
            if value.get("kind").is_none() || value.pointer("/metadata/name").is_none() {
                continue;
            }
            let Ok(envelope) = serde_json::from_value::<ResourceEnvelope>(value) else {
                continue;
            };
            let ns = envelope.metadata.namespace.clone().unwrap_or_default();
            files.insert(
                (ns, envelope.kind.clone(), envelope.metadata.name.clone()),
                path.clone(),
            );
            mirror.upsert(envelope);
        }
    }
}

/// The author's `bento.yaml` of each Pipeline: in the seed it sits beside the manifest as
/// `<manifest>-bento.yaml` (the forge seed writes it to `pipelines/<name>/bento.yaml`).
fn bentos(files: &HashMap<(String, String, String), PathBuf>) -> Bentos {
    files
        .iter()
        .filter(|((_, kind, _), _)| kind == "Pipeline")
        .filter_map(|((project, _, name), path)| {
            let beside = path.with_file_name(format!("{}-bento.yaml", path.file_stem()?.to_str()?));
            let text = std::fs::read_to_string(beside).ok()?;
            Some((
                (project.clone(), name.clone()),
                text.replace("{orgDomain}", ORG_DOMAIN),
            ))
        })
        .collect()
}

/// The Portal's `pipeline_validation::load` over the flat seed: each space's model compiled from
/// the JSON Schema artifact beside its DataModel manifest.
fn compile_models(
    mirror: &Mirror,
    files: &HashMap<(String, String, String), PathBuf>,
) -> HashMap<(String, String), Arc<ModelSchema>> {
    use jc_core::kinds::{ContextSpaceSpec, DataModelSpec};
    let mut compiled = HashMap::new();
    let all = mirror.matching(|_| true);
    for env in all.iter().filter(|e| e.kind == "ContextSpace") {
        let Ok(space) = serde_json::from_value::<ContextSpaceSpec>(env.spec.clone()) else {
            continue;
        };
        let Some(named) = space.data_model_ref else {
            continue;
        };
        let project = env.metadata.namespace.clone().unwrap_or_default();
        let model = all.iter().find_map(|m| {
            let spec = serde_json::from_value::<DataModelSpec>(m.spec.clone()).ok()?;
            (m.kind == "DataModel"
                && m.metadata.namespace.as_deref() == Some(project.as_str())
                && m.metadata.name == named.name()
                && spec.context_space_ref.as_deref() == Some(env.metadata.name.as_str()))
            .then(|| (m.metadata.name.clone(), spec))
        });
        let Some((model_name, spec)) = model else {
            continue;
        };
        let json_schema = spec
            .artifacts
            .json_schema
            .as_deref()
            .and_then(|relative| {
                let manifest = files.get(&(project.clone(), "DataModel".into(), model_name))?;
                let text = std::fs::read_to_string(manifest.parent()?.join(relative)).ok()?;
                serde_json::from_str(&text).ok()
            })
            .unwrap_or(Value::Null);
        let mut schema = ModelSchema::compile_for_space(
            named.name(),
            spec.version.as_str(),
            &json_schema,
            &spec.classes,
            spec.open_world,
            space.missing_unit_code,
        );
        schema.quality = space.quality;
        compiled.insert((project, env.metadata.name.clone()), Arc::new(schema));
    }
    compiled
}
