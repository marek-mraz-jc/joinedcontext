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
    ctx.resources
        .insert_cache(ctx.org_id(), name, cache.clone());
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

/// What `tracing` logs on this thread while the guard lives.
fn capture_logs() -> (
    Arc<std::sync::Mutex<Vec<u8>>>,
    tracing::subscriber::DefaultGuard,
) {
    let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || LogSink(sink.clone()))
        .finish();
    (buf, tracing::subscriber::set_default(subscriber))
}

struct LogSink(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn logged(buf: &std::sync::Mutex<Vec<u8>>) -> String {
    String::from_utf8_lossy(
        &buf.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned()
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
    assert!(err.contains("field check is required"), "{err}");
}

#[tokio::test]
async fn group_by_refuses_a_bad_group_and_leaves_a_failing_check_groupless() {
    let ctx = make_ctx();
    for (yaml, want) in [
        ("group_by: 1\n", "group_by: expected an array of groups"),
        (
            "group_by:\n  - check: root = (\n",
            "group '0' parse error: ",
        ),
        (
            "group_by:\n  - check: \"true\"\n    processors: 1\n",
            "group '0' parse error: field processors must be a list",
        ),
        (
            "group_by:\n  - check: \"true\"\n    processors:\n      - 5\n",
            "expected object for component config, got 5",
        ),
    ] {
        let err = build(&ctx, yaml)
            .err()
            .unwrap_or_else(|| panic!("accepted {yaml}"));
        assert!(err.contains(want), "{yaml}: {err}");
    }
    let proc = build(
        &ctx,
        "group_by:\n  - check: this.n > 1\n    processors:\n      - mapping: root = \"big\"\n",
    )
    .expect("built");
    assert_eq!(proc.processor_type(), "group_by");
    let out = run(&ctx, &proc, &[r#"{"n":5}"#, "not json", r#"{"n":0}"#]).await;
    assert_eq!(
        bodies(&out),
        vec![
            vec!["big".to_string()],
            vec!["not json".to_string(), r#"{"n":0}"#.to_string()],
        ]
    );
    assert!(out[1].iter().all(|p| !p.is_errored()));
    proc.close().await.expect("closed");
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
        vec![
            r#"{"foo":1,"bar":5}"#,
            r#"{"foo":1,"bar":6}"#,
            r#"{"foo":1,"bar":8}"#,
        ],
    ];
    assert_eq!(bodies(&out), want);
}

#[tokio::test]
async fn group_by_value_flags_an_interpolation_error() {
    let ctx = make_ctx();
    let proc = build(&ctx, "group_by_value:\n  value: ${! this.foo.number() }\n").expect("built");
    let out = run(&ctx, &proc, &[r#"{"foo":"x"}"#]).await;
    let err = out[0][0].error_get().expect("flagged");
    assert!(
        err.message.starts_with("group value interpolation error:"),
        "{}",
        err.message
    );
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
    let proc = build(
        &ctx,
        &SKIP.replace("  cache: foo", "  ttl: ${! 2+2 }\n  cache: foo"),
    )
    .expect("built");
    let out = run(&ctx, &proc, &["keya"]).await;
    let err = out[0][0].error_get().expect("flagged");
    assert_eq!(
        err.message,
        r#"failed to parse ttl expression: time: missing unit in duration "4""#
    );
}

/// A cache whose store is down: every read and write fails with an error other than a miss.
struct DownCache {
    get_hits: bool,
}

#[async_trait::async_trait]
impl Cache for DownCache {
    async fn get(&self, key: &str) -> Result<bytes::Bytes, ekiben_core::CoreError> {
        if self.get_hits {
            return Err(ekiben_core::CoreError::KeyNotFound(key.into()));
        }
        Err(ekiben_core::CoreError::config("cache store is down"))
    }
    async fn set(
        &self,
        _: &str,
        _: bytes::Bytes,
        _: Option<std::time::Duration>,
    ) -> Result<(), ekiben_core::CoreError> {
        Err(ekiben_core::CoreError::config("cache store is down"))
    }
    async fn add(
        &self,
        _: &str,
        _: bytes::Bytes,
        _: Option<std::time::Duration>,
    ) -> Result<(), ekiben_core::CoreError> {
        Err(ekiben_core::CoreError::config("cache store is down"))
    }
    async fn delete(&self, _: &str) -> Result<(), ekiben_core::CoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn cached_flags_each_message_it_cannot_key_time_or_read() {
    let (logs, _guard) = capture_logs();
    let ctx = make_ctx();
    let foo = add_cache(&ctx, "foo");
    let proc = build(
        &ctx,
        r#"
cached:
  key: ${! this.k.string() }
  ttl: ${! this.ttl.not_null() }
  cache: foo
  skip_on: this.n.number() > 1
  processors:
    - mapping: root = this
"#,
    )
    .expect("built");
    assert_eq!(proc.processor_type(), "cached");
    for (key, stored) in [
        ("bad", &b"not a batch"[..]),
        ("bad2", b"\0\0\0\x01"),
        ("bad3", b"\0\0\0\x01\0\0\0\x01"),
    ] {
        foo.set(key, bytes::Bytes::from_static(stored), None)
            .await
            .unwrap();
    }
    let out = run(
        &ctx,
        &proc,
        &[
            "not json",
            r#"{"k":"a"}"#,
            r#"{"k":"a","ttl":"5x"}"#,
            r#"{"k":"a","ttl":"soon"}"#,
            r#"{"k":"bad","ttl":"1s"}"#,
            r#"{"k":"bad2","ttl":"1s"}"#,
            r#"{"k":"bad3","ttl":"1s"}"#,
            r#"{"k":"skip","ttl":"1s","n":"x"}"#,
            r#"{"k":"skip2","ttl":"1s","n":5}"#,
        ],
    )
    .await;
    let errs: Vec<String> = out[0]
        .iter()
        .map(|p| p.error_get().map(|e| e.message.clone()).unwrap_or_default())
        .collect();
    assert!(
        errs[0].starts_with("failed to interpolate key expression: "),
        "{}",
        errs[0]
    );
    assert!(
        errs[1].starts_with("failed to interpolate ttl expression: "),
        "{}",
        errs[1]
    );
    assert_eq!(
        errs[2],
        "failed to parse ttl expression: time: unknown unit \"x\" in duration \"5x\""
    );
    assert_eq!(
        errs[3],
        r#"failed to parse ttl expression: time: invalid duration "soon""#
    );
    assert!(
        errs[4].starts_with(
            "failed to parse cached result, this indicates the data was not set by this processor"
        ),
        "{}",
        errs[4]
    );
    assert!(
        errs[5].ends_with("failed to extract batch size: message is too small to extract number"),
        "{}",
        errs[5]
    );
    assert!(
        errs[6]
            .ends_with("failed to extract message 0 size: message is too small to extract number"),
        "{}",
        errs[6]
    );
    // A skip_on that fails or answers true passes the result on uncached.
    assert_eq!(&errs[7..], ["", ""]);
    assert!(foo.get("skip").await.is_err());
    assert!(foo.get("skip2").await.is_err());
    let logs = logged(&logs);
    assert!(
        logs.contains("skip_on check failed: failed to execute skip_on mapping")
            && logs.contains("caching will be skipped as a precaution"),
        "{logs}"
    );

    // A cache store that is down flags the message on read; a failed write still answers.
    for (get_hits, want) in [
        (false, "configuration error: cache store is down"),
        (true, ""),
    ] {
        let ctx = make_ctx();
        ctx.resources
            .insert_cache(ctx.org_id(), "foo", Arc::new(DownCache { get_hits }));
        let proc = build(
            &ctx,
            "cached:\n  key: k\n  cache: foo\n  processors:\n    - mapping: root = \"done\"\n",
        )
        .expect("built");
        let out = run(&ctx, &proc, &["x"]).await;
        let err = out[0][0]
            .error_get()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        assert!(
            err.contains(want.trim_start_matches("configuration error: ")),
            "{err}"
        );
        assert_eq!(err.is_empty(), want.is_empty(), "{err}");
        if get_hits {
            assert_eq!(bodies(&out), vec![vec!["done".to_string()]]);
        }
    }
}

#[tokio::test]
async fn cached_stores_a_result_its_children_emptied() {
    let ctx = make_ctx();
    let foo = add_cache(&ctx, "foo");
    let proc = build(
        &ctx,
        "cached:\n  key: k\n  cache: foo\n  skip_on: errored()\n  processors:\n    - mapping: root = deleted()\n",
    )
    .expect("built");
    let out = run(&ctx, &proc, &["x"]).await;
    assert!(bodies(&out).iter().all(Vec::is_empty), "{:?}", bodies(&out));
    // Version 1, a batch of no messages: the next run reads the empty result back.
    assert_eq!(
        &foo.get("k").await.expect("cached")[..],
        b"\0\0\0\x01\0\0\0\0"
    );
    let again = run(&ctx, &proc, &["x"]).await;
    assert!(bodies(&again).iter().all(Vec::is_empty));
}

#[tokio::test]
async fn a_metric_value_that_is_no_integer_is_logged_and_not_counted() {
    let (logs, _guard) = capture_logs();
    let ctx = make_ctx();
    let proc = build(
        &ctx,
        "metric:\n  type: counter_by\n  name: bytes_total\n  value: ${! json(\"n\") }\n",
    )
    .expect("built");
    let out = run(&ctx, &proc, &[r#"{"n":"x"}"#, r#"{"n":4}"#]).await;
    assert_eq!(bodies(&out), vec![vec![r#"{"n":"x"}"#, r#"{"n":4}"#]]);
    assert!(out[0].iter().all(|p| !p.is_errored()));
    assert_eq!(ctx.metrics.get_counter("bytes_total"), 4);
    let logs = logged(&logs);
    assert!(
        logs.contains("metric value is not a valid integer for type CounterBy")
            && logs.contains("metric=bytes_total")
            && logs.contains(r#"value=Some("x")"#),
        "{logs}"
    );
}

#[test]
fn cached_refuses_a_config_it_cannot_build() {
    let ctx = make_ctx();
    add_cache(&ctx, "foo");
    for (yaml, want) in [
        ("cached: 1\n", "cached: expected an object"),
        (
            "cached:\n  cache: foo\n  processors: []\n",
            "field key is required",
        ),
        (
            "cached:\n  cache: foo\n  key: ${! ( }\n  processors: []\n",
            "",
        ),
        (
            "cached:\n  cache: foo\n  key: k\n  skip_on: root = (\n  processors: []\n",
            "",
        ),
        (
            "cached:\n  cache: foo\n  key: k\n",
            "field processors is required",
        ),
        (
            "cached:\n  cache: foo\n  key: k\n  processors:\n    - 5\n",
            "expected object for component config, got 5",
        ),
        (
            "group_by_value:\n  value: ${! ( }\n",
            "failed to parse value expression",
        ),
    ] {
        let err = build(&ctx, yaml)
            .err()
            .unwrap_or_else(|| panic!("accepted {yaml}"));
        assert!(err.contains(want), "{yaml}: {err}");
    }
}

#[tokio::test]
async fn cached_refuses_a_cache_the_pipeline_does_not_declare() {
    let ctx = make_ctx();
    let err = build(&ctx, HAPPY).err().expect("refused");
    assert!(err.contains("cache named foo not found"), "{err}");
}
