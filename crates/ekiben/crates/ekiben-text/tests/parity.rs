//! Replays tests/parity/cases.json: each input, with only the metadata `source: eea`, run through the
//! case's processor gives Bento 1.22.0's recorded stdout (scripts/record-text-parity.py): one line per
//! message, its content (hex for `msgpack: from_json`) or `ERROR: ` and the error. A case marked
//! `"compare": "failure"` only requires the message to fail, where the error text comes from the
//! query library (gojq or go-jmespath there, jaq or jmespath.rs here).

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    ComponentConfig, OrgId, Part, ProcContext, Registry, ResourceManager, StreamMetrics,
};
use serde_json::Value as Json;
use tokio::sync::watch;

fn ctx() -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_text::register(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("t"));
    let (_tx, rx) = watch::channel(false);
    let c = ProcContext {
        stream_id: "parity".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("parity", "t"),
        label: String::new(),
        component_type: "processor".to_string(),
        path: "pipeline.processors".to_string(),
        file_root: None,
        shutdown_rx: rx,
    };
    (reg, c)
}

async fn run(case: &Json) -> (String, bool) {
    let (reg, c) = ctx();
    let name = case["name"].as_str().unwrap_or_default();
    let mut procs = vec![case["processor"].clone()];
    if name.ends_with("then to_json") {
        procs.push(serde_json::json!({"msgpack": {"operator": "to_json"}}));
    }
    let hexed = case["processor"]["msgpack"]["operator"] == "from_json" && procs.len() == 1;
    let mut part = Part::new(Bytes::from(case["input"].as_str().unwrap_or_default().to_string()));
    part.meta_set_str("source", "eea");
    let mut batch = vec![part];
    for conf in &procs {
        let comp = ComponentConfig::from_json_value(conf, "noop", None).expect("config parses");
        let p = reg.build_processor_list(&[comp], &c).expect("builds").remove(0);
        let (ok, errored): (Vec<Part>, Vec<Part>) =
            batch.into_iter().partition(|p| !p.is_errored());
        let mut next: Vec<Part> = p.process_batch(&c, ok).await.expect("no batch error").into_iter().flatten().collect();
        next.extend(errored);
        batch = next;
    }
    let mut out = String::new();
    let mut failed = false;
    for p in &batch {
        match p.error_get() {
            Some(e) => {
                failed = true;
                out.push_str(&format!("ERROR: {e}\n"));
            }
            None if hexed => out.push_str(&format!("{}\n", hex::encode(p.as_bytes_ref()))),
            None => out.push_str(&format!("{}\n", String::from_utf8_lossy(&p.as_bytes_ref()))),
        }
    }
    (out, failed)
}

#[tokio::test]
async fn text_processors_match_bento() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let cases: Vec<Json> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    let mut failed = Vec::new();
    for case in &cases {
        let want = case["expected"]["text"].as_str().expect("recorded");
        let (got, errored) = run(case).await;
        let ok = match case.get("compare").and_then(Json::as_str) {
            Some("failure") => errored && want.starts_with("ERROR: "),
            _ => got == want,
        };
        if !ok {
            failed.push(format!("{}: want {want:?}\n  got {got:?}", case["name"]));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} cases differ from Bento:\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n")
    );
}
