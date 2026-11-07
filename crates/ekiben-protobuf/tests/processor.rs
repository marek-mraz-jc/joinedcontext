//! protobuf as a pipeline processor: a bad message flagged and kept, Bento's config rules, import paths confined.

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
    ekiben_protobuf::register(&reg);
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

fn parity() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity")
}

fn gtfs(paths: serde_json::Value) -> serde_json::Value {
    json!({"protobuf": {"operator": "to_json", "message": "transit_realtime.FeedMessage", "import_paths": paths}})
}

#[tokio::test]
async fn a_message_that_does_not_decode_is_flagged_and_passed_on_unchanged() {
    let (reg, ctx) = make_ctx(None);
    let dir = parity().join("proto/gtfs");
    let p = build(&reg, &ctx, gtfs(json!([dir.to_str().unwrap()]))).unwrap();
    let good = std::fs::read(parity().join("data/hsl-service-alerts.pb")).unwrap();
    let parts = vec![
        Part::new(Bytes::from_static(b"\x0a\xff\xff")),
        Part::new(Bytes::from(good)),
    ];
    let out = p.process_batch(&ctx, parts).await.unwrap();
    let mut bad = out[0][0].clone();
    assert!(bad.is_errored());
    assert_eq!(&bad.as_bytes()[..], b"\x0a\xff\xff");
    let mut ok = out[0][1].clone();
    assert!(!ok.is_errored());
    assert!(ok
        .as_bytes()
        .starts_with(br#"{"header":{"gtfsRealtimeVersion":"2.0""#));
}

#[test]
fn config_rules_follow_bento() {
    let (reg, ctx) = make_ctx(None);
    let dir = parity().join("proto/gtfs");
    let d = dir.to_str().unwrap();
    let err = |c| {
        build(&reg, &ctx, c)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    };
    assert!(err(
        json!({"protobuf": {"operator": "to_json", "message": "transit_realtime.FeedMessage"}})
    )
    .contains("at least one of `import_paths`and `bsr` must be set"));
    assert!(err(json!({"protobuf": {"operator": "to_json", "message": "x.Y", "import_paths": [d], "bsr": [{"module": "buf.build/a/b"}]}}))
        .contains("both `import_paths` and `bsr` can't be set simultaneously"));
    assert!(err(json!({"protobuf": {"operator": "to_json", "message": "x.Y", "bsr": [{"module": "buf.build/a/b"}]}}))
        .contains("bsr (Buf Schema Registry) is not supported"));
    assert!(
        err(json!({"protobuf": {"operator": "to_json", "message": "", "import_paths": [d]}}))
            .contains("message field must not be empty")
    );
    assert!(err(json!({"protobuf": {"operator": "to_json", "message": "transit_realtime.Nope", "import_paths": [d]}}))
        .contains("unable to find message 'transit_realtime.Nope' definition within"));
    assert!(err(json!({"protobuf": {"operator": "sideways", "message": "transit_realtime.FeedMessage", "import_paths": [d]}}))
        .contains("operator not recognised"));
    assert!(err(json!({"protobuf": {"operator": "to_json", "message": "transit_realtime.FeedMessage", "import_paths": [d], "bogus": 1}}))
        .contains("bogus"));
}

#[test]
fn a_schema_that_does_not_compile_is_refused_at_start() {
    let (reg, ctx) = make_ctx(None);
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("bad.proto"),
        "syntax = \"proto3\"; message A { strin x = 1; }",
    )
    .unwrap();
    let r = build(
        &reg,
        &ctx,
        json!({"protobuf": {"operator": "to_json", "message": "A", "import_paths": [tmp.path().to_str().unwrap()]}}),
    );
    assert!(r.is_err());
}

#[test]
fn import_paths_stay_inside_the_file_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("proto")).unwrap();
    std::fs::copy(
        parity().join("proto/gtfs/gtfs-realtime.proto"),
        root.path().join("proto/g.proto"),
    )
    .unwrap();
    std::fs::write(
        outside.path().join("secret.proto"),
        "syntax = \"proto3\"; message S {}",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.proto"),
        root.path().join("proto/s.proto"),
    )
    .unwrap();
    let (reg, ctx) = make_ctx(Some(root.path().to_path_buf()));
    let r = build(&reg, &ctx, gtfs(json!(["proto"])));
    assert!(r
        .err()
        .unwrap()
        .to_string()
        .contains("escapes the configured file root"));
    std::fs::remove_file(root.path().join("proto/s.proto")).unwrap();
    assert!(
        build(&reg, &ctx, gtfs(json!(["proto"]))).is_ok(),
        "a path relative to the root resolves inside it"
    );
    assert!(
        build(&reg, &ctx, gtfs(json!(["../proto"]))).is_err(),
        "'..' is refused"
    );
}
