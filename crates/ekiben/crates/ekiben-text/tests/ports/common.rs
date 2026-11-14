//! Helpers shared by the Bento test twins.

use std::sync::Arc;

use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    Batch, ComponentConfig, CoreError, OrgId, Part, ProcContext, Processor, Registry,
    ResourceManager, StreamMetrics,
};
use tokio::sync::watch;

pub fn ctx(file_root: Option<std::path::PathBuf>) -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_text::register(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("t"));
    let (_tx, rx) = watch::channel(false);
    let c = ProcContext {
        stream_id: "test-stream".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("test-stream", "t"),
        label: "root".to_string(),
        component_type: "root".to_string(),
        path: "pipeline.processors".to_string(),
        file_root,
        shutdown_rx: rx,
    };
    (reg, c)
}

pub fn build_in(
    yaml: &str,
    root: Option<std::path::PathBuf>,
) -> Result<(Arc<dyn Processor>, ProcContext), CoreError> {
    let conf: serde_json::Value = serde_norway::from_str(yaml).expect("test yaml");
    let (reg, c) = ctx(root);
    let comp = ComponentConfig::from_json_value(&conf, "noop", None)?;
    let p = reg.build_processor_list(&[comp], &c)?.remove(0);
    Ok((p, c))
}

pub fn build_yaml(yaml: &str) -> Result<(Arc<dyn Processor>, ProcContext), CoreError> {
    build_in(yaml, None)
}

pub async fn run_parts(yaml: &str, parts: Vec<Part>) -> Vec<Batch> {
    let (p, c) = build_yaml(yaml).expect("config builds");
    p.process_batch(&c, parts).await.expect("no batch error")
}

pub async fn run(yaml: &str, inputs: &[&str]) -> Vec<Batch> {
    run_parts(
        yaml,
        inputs
            .iter()
            .map(|s| Part::new(Bytes::from(s.to_string())))
            .collect(),
    )
    .await
}

pub fn bytes(p: &Part) -> String {
    String::from_utf8_lossy(&p.as_bytes_ref()).into_owned()
}
