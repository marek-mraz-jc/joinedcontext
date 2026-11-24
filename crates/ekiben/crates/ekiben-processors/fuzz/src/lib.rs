//! Shared harness of the ekiben-processors fuzz targets: a registry with every processor and a
//! context as a stream would build them, and a one-thread runtime to drive a batch.
use std::sync::Arc;

use ekiben_core::{
    execute_all, ComponentConfig, OrgId, Part, ProcContext, Processor, Registry, ResourceManager,
    StreamMetrics,
};

pub fn ctx() -> ProcContext {
    let registry = Arc::new(Registry::new());
    ekiben_processors::register_all(&registry);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("fuzz"));
    ProcContext {
        stream_id: "fuzz".into(),
        org,
        resources,
        registry,
        blobl_env: ekiben_bloblang::Environment::new(),
        metrics: StreamMetrics::new("fuzz", "fuzz"),
        label: "fuzz".into(),
        component_type: "fuzz".into(),
        path: "pipeline.processors".into(),
        file_root: None,
        shutdown_rx: tokio::sync::watch::channel(false).1,
    }
}

/// Builds the processors a config list names; a refusal is `None`.
pub fn build(ctx: &ProcContext, configs: &serde_json::Value) -> Option<Vec<Arc<dyn Processor>>> {
    let confs = configs
        .as_array()?
        .iter()
        .map(|v| ComponentConfig::from_json_value(v, "noop", None))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    ctx.registry.build_processor_list(&confs, ctx).ok()
}

/// Runs one message through `procs`; returns how many messages came out (0 on a batch error).
pub fn run(ctx: &ProcContext, procs: &[Arc<dyn Processor>], message: &[u8]) -> usize {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    rt.block_on(execute_all(
        ctx,
        procs,
        vec![vec![Part::new(message.to_vec())]],
    ))
    .map(|batches| batches.iter().map(Vec::len).sum())
    .unwrap_or(0)
}
