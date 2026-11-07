//! parquet_decode as a pipeline processor: one message per row, metadata carried, a bad file flagged and kept.

use std::sync::Arc;

use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    ComponentConfig, CoreError, OrgId, Part, ProcContext, Processor, Registry, ResourceManager,
    StreamMetrics,
};
use serde_json::json;
use tokio::sync::watch;

fn make_ctx(file_root: Option<std::path::PathBuf>) -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_parquet::register(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("t"));
    let (_tx, rx) = watch::channel(false);
    let ctx = ProcContext {
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
    (reg, ctx)
}

fn build(
    reg: &Registry,
    ctx: &ProcContext,
    conf: serde_json::Value,
) -> Result<Arc<dyn Processor>, CoreError> {
    let c = ComponentConfig::from_json_value(&conf, "noop", None)?;
    reg.build_processor_list(&[c], ctx).map(|mut v| v.remove(0))
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/parity")
            .join(name),
    )
    .unwrap()
}

#[tokio::test]
async fn each_row_becomes_a_message_with_the_source_metadata() {
    let (reg, ctx) = make_ctx(None);
    let p = build(&reg, &ctx, json!({"parquet_decode": {}})).unwrap();
    let mut part = Part::new(Bytes::from(fixture("int96.parquet")));
    part.meta_set_str("source", "eea");
    let out = p.process_batch(&ctx, vec![part]).await.unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].len(), 3);
    for part in &out[0] {
        assert_eq!(part.meta_get_str("source"), "eea");
        assert!(!part.is_errored());
    }
    let mut first = out[0][0].clone();
    assert_eq!(&first.as_bytes()[..], br#"{"t":[0,0,2440588]}"#);
}

#[tokio::test]
async fn a_file_that_is_not_parquet_is_flagged_and_passed_on_unchanged() {
    let (reg, ctx) = make_ctx(None);
    let p = build(&reg, &ctx, json!({"parquet_decode": {}})).unwrap();
    let out = p
        .process_batch(&ctx, vec![Part::new(Bytes::from_static(b"not parquet"))])
        .await
        .unwrap();
    let mut part = out[0][0].clone();
    assert!(part.is_errored());
    assert_eq!(&part.as_bytes()[..], b"not parquet");
    assert!(part
        .error_get()
        .unwrap()
        .to_string()
        .contains("invalid magic header of parquet file"));
}

#[tokio::test]
async fn unknown_and_mistyped_fields_are_refused() {
    let (reg, ctx) = make_ctx(None);
    assert!(build(&reg, &ctx, json!({"parquet_decode": {"bogus": true}})).is_err());
    assert!(build(
        &reg,
        &ctx,
        json!({"parquet_decode": {"use_parquet_list_format": "yes"}})
    )
    .is_err());
    assert!(build(
        &reg,
        &ctx,
        json!({"parquet_decode": {"byte_array_as_string": true}})
    )
    .is_ok());
}
