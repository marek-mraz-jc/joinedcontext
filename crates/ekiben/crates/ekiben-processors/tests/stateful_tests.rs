//! Bento 1.22.0 `processor_group_by_test.go`, `processor_group_by_value_test.go` and
//! `processor_cached_test.go`, case for case under the Go test names (T-6841).

use std::sync::Arc;

use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, Cache, ComponentConfig, OrgId, Part, ProcContext, Processor, Registry,
    ResourceManager, StreamMetrics,
};
use ekiben_processors::resources::MemoryCache;
use tokio::sync::watch;

fn make_ctx() -> ProcContext {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("org1"));
    let (_tx, rx) = watch::channel(false);
    ProcContext {
        stream_id: "test-stream".to_string(),
        org,
        resources,
        registry: reg,
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("test-stream", "org1"),
        label: String::new(),
        component_type: "root".to_string(),
        path: "pipeline.processors".to_string(),
        file_root: None,
        shutdown_rx: rx,
    }
}

fn add_cache(ctx: &ProcContext, name: &str) -> Arc<dyn Cache> {
    let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new(None, None));
    ctx.resources.insert_cache(ctx.org_id(), name, cache.clone());
    cache
}

fn build(ctx: &ProcContext, yaml: &str) -> Result<Arc<dyn Processor>, String> {
    let val: serde_json::Value = serde_norway::from_str(yaml).expect("yaml");
    let conf = ComponentConfig::from_json_value(&val, "noop", None).map_err(|e| e.to_string())?;
    ctx.registry
        .build_processor_list(&[conf], ctx)
        .map(|mut v| v.remove(0))
        .map_err(|e| e.to_string())
}

async fn run(ctx: &ProcContext, proc: &Arc<dyn Processor>, msgs: &[&str]) -> Vec<Vec<Part>> {
    let batch = msgs.iter().map(|m| Part::from_str(m)).collect();
    execute_all(ctx, std::slice::from_ref(proc), vec![batch])
        .await
        .expect("processed")
}

fn bodies(batches: &[Vec<Part>]) -> Vec<Vec<String>> {
    batches
        .iter()
        .map(|b| {
            b.iter()
                .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn test_group_by() {
    let ctx = make_ctx();
    let proc = build(
        &ctx,
        r#"
group_by:
  - check: 'content().contains("foo")'
    processors:
      - archive:
          format: lines
  - check: 'content().contains("bar")'
    processors:
      - bloblang: 'root = content().uppercase()'
      - bloblang: 'root = content().trim()'
"#,
    )
    .expect("built");
    let out = run(
        &ctx,
        &proc,
        &[
            " hello foo world 1 ",
            " hello world 1 ",
            " hello foo bar world 1 ",
            " hello bar world 1 ",
            " hello foo world 2 ",
            " hello world 2 ",
            " hello foo bar world 2 ",
            " hello bar world 2 ",
        ],
    )
    .await;
    assert_eq!(
        bodies(&out),
        vec![
            vec![" hello foo world 1 \n hello foo bar world 1 \n hello foo world 2 \n hello foo bar world 2 ".to_string()],
            vec!["HELLO BAR WORLD 1".to_string(), "HELLO BAR WORLD 2".to_string()],
            vec![" hello world 1 ".to_string(), " hello world 2 ".to_string()],
        ]
    );
}

#[tokio::test]
async fn test_group_by_errs() {
    let ctx = make_ctx();
    let err = build(
        &ctx,
        "group_by:\n  - processors:\n      - archive:\n          format: lines\n",
    )
    .err()
    .expect("refused");
    assert!(err.contains("field 'check' is required"), "{err}");
}

#[tokio::test]
async fn test_group_by_value_basic() {
    let ctx = make_ctx();
    let proc = build(&ctx, "group_by_value:\n  value: ${!json(\"foo\")}\n").expect("built");
    let out = run(
        &ctx,
        &proc,
        &[
            r#"{"foo":0,"bar":0}"#,
            r#"{"foo":3,"bar":1}"#,
            r#"{"bar":2}"#,
            r#"{"foo":2,"bar":3}"#,
            r#"{"foo":4,"bar":4}"#,
            r#"{"foo":1,"bar":5}"#,
            r#"{"foo":1,"bar":6}"#,
            r#"{"foo":0,"bar":7}"#,
            r#"{"foo":1,"bar":8}"#,
        ],
    )
    .await;
    let want: Vec<Vec<&str>> = vec![
        vec![r#"{"foo":0,"bar":0}"#, r#"{"foo":0,"bar":7}"#],
        vec![r#"{"foo":3,"bar":1}"#],
        vec![r#"{"bar":2}"#],
        vec![r#"{"foo":2,"bar":3}"#],
        vec![r#"{"foo":4,"bar":4}"#],
        vec![r#"{"foo":1,"bar":5}"#, r#"{"foo":1,"bar":6}"#, r#"{"foo":1,"bar":8}"#],
    ];
    assert_eq!(bodies(&out), want);
}

#[tokio::test]
async fn group_by_value_flags_an_interpolation_error() {
    let ctx = make_ctx();
    let proc = build(&ctx, "group_by_value:\n  value: ${! this.foo.number() }\n").expect("built");
    let out = run(&ctx, &proc, &[r#"{"foo":"x"}"#]).await;
    let err = out[0][0].error_get().expect("flagged");
    assert!(err.message.starts_with("group value interpolation error:"), "{}", err.message);
}

const HAPPY: &str = r#"
cached:
  key: ${! content() }
  ttl: 3660s
  cache: foo
  skip_on: errored()
  processors:
    - bloblang: 'root = content().string() + " FOO " + uuid_v4()'
    - bloblang: 'root = content().string() + " BAR " + uuid_v4()'
"#;

#[tokio::test]
async fn test_cached_happy() {
    let ctx = make_ctx();
    let foo = add_cache(&ctx, "foo");
    let bar = add_cache(&ctx, "bar");
    let proc = build(&ctx, HAPPY).expect("built");
    let a1 = bodies(&run(&ctx, &proc, &["keya"]).await);
    let b1 = bodies(&run(&ctx, &proc, &["keyb"]).await);
    let a2 = bodies(&run(&ctx, &proc, &["keya"]).await);
    let b2 = bodies(&run(&ctx, &proc, &["keyb"]).await);
    for (got, key) in [(&a1, "keya"), (&b1, "keyb"), (&a2, "keya"), (&b2, "keyb")] {
        assert_eq!(got[0].len(), 1);
        assert!(got[0][0].contains(&format!("{key} FOO ")), "{got:?}");
        assert!(got[0][0].contains(" BAR "), "{got:?}");
    }
    assert_eq!(a1, a2);
    assert_eq!(b1, b2);
    assert!(foo.get("keya").await.is_ok());
    assert!(foo.get("keyb").await.is_ok());
    assert!(bar.get("keya").await.is_err());
    assert!(bar.get("keyb").await.is_err());
    proc.close().await.expect("closed");
}

#[tokio::test]
async fn test_cached_happy_batched() {
    let ctx = make_ctx();
    add_cache(&ctx, "foo");
    let proc = build(
        &ctx,
        r#"
cached:
  key: ${! meta("key") }
  ttl: ${! meta("ttl").or("60s")}
  cache: foo
  processors:
    - bloblang: 'root = this.map_each(ele -> ele + " FOO")'
    - unarchive:
        format: json_array
"#,
    )
    .expect("built");
    let a1 = bodies(&run(&ctx, &proc, &[r#"["valuea","valueb","valuec"]"#]).await);
    let a2 = bodies(&run(&ctx, &proc, &[r#"["valued","valuee"]"#]).await);
    let want = vec![vec![
        r#""valuea FOO""#.to_string(),
        r#""valueb FOO""#.to_string(),
        r#""valuec FOO""#.to_string(),
    ]];
    assert_eq!(a1, want);
    assert_eq!(a2, want);
}

const SKIP: &str = r#"
cached:
  key: ${! content() }
  cache: foo
  skip_on: "errored()"
  processors:
    - bloblang: |
        let body = content().string()
        root = "%s FOO %d".format($body, count($body))
    - bloblang: root = throw("simulated error")
"#;

#[tokio::test]
async fn test_cached_skip() {
    let ctx = make_ctx();
    let foo = add_cache(&ctx, "foo");
    let proc = build(&ctx, SKIP).expect("built");
    let a1 = bodies(&run(&ctx, &proc, &["keya"]).await);
    let a2 = bodies(&run(&ctx, &proc, &["keya"]).await);
    assert_eq!(a1, vec![vec!["keya FOO 1".to_string()]]);
    assert_eq!(a2, vec![vec!["keya FOO 2".to_string()]]);
    assert!(matches!(
        foo.get("keya").await,
        Err(ekiben_core::CoreError::KeyNotFound(_))
    ));
}

#[tokio::test]
async fn test_cached_bad_ttl() {
    let ctx = make_ctx();
    add_cache(&ctx, "foo");
    let proc = build(&ctx, &SKIP.replace("  cache: foo", "  ttl: ${! 2+2 }\n  cache: foo"))
        .expect("built");
    let out = run(&ctx, &proc, &["keya"]).await;
    let err = out[0][0].error_get().expect("flagged");
    assert_eq!(
        err.message,
        r#"failed to parse ttl expression: time: missing unit in duration "4""#
    );
}

#[tokio::test]
async fn cached_refuses_a_cache_the_pipeline_does_not_declare() {
    let ctx = make_ctx();
    let err = build(&ctx, HAPPY).err().expect("refused");
    assert!(err.contains("cache named foo not found"), "{err}");
}
