use criterion::{criterion_group, criterion_main, Criterion};
use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, ComponentConfig, OrgId, Part, ProcContext, ResourceManager, StreamMetrics,
};
use ekiben_testkit::full_registry;
use std::hint::black_box;
use tokio::runtime::Runtime;
use tokio::sync::watch;

fn bench_pipeline(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let reg = full_registry();
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("bench-org"));
    let (_tx, rx) = watch::channel(false);
    let pctx = ProcContext {
        stream_id: "bench".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("bench", "bench-org"),
        label: "bench_proc".to_string(),
        component_type: "pipeline".to_string(),
        path: "pipeline.processors".to_string(),
        file_root: None,
        shutdown_rx: rx,
    };

    let branch_cfg_val: serde_json::Value = serde_norway::from_str(
        r#"
- mapping: |
    root.id = this.id
    root.name = this.name.trim().uppercase()
- branch:
    request_map: 'root = {"x": this.score}'
    processors:
      - mapping: 'root.res = this.x * 2 + 1'
    result_map: 'root.computed = this.res'
"#,
    )
    .unwrap();
    let confs: Vec<ComponentConfig> = branch_cfg_val
        .as_array()
        .unwrap()
        .iter()
        .map(|v| ComponentConfig::from_json_value(v, "noop", None).unwrap())
        .collect();
    let procs = reg.build_processor_list(&confs, &pctx).unwrap();

    c.bench_function("pipeline_mapping_and_branch", |b| {
        b.iter(|| {
            let batch = vec![Part::from_str(
                r#"{"id":"p-1","name":"  station  ","score":21}"#,
            )];
            rt.block_on(async {
                let res = execute_all(&pctx, &procs, vec![black_box(batch)])
                    .await
                    .unwrap();
                black_box(res);
            });
        })
    });
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);
