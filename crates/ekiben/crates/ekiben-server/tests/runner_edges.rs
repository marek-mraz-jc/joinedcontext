//! The runner's failure paths with scripted components: an input or output that does not connect,
//! a read that fails or times out, a write the output refuses, a run over its CPU budget, a stop
//! that outlives its timeout, and the inline resources, buffer and `secret_refs` a run builds.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ekiben_core::{
    AckResponder, Batch, CoreError, OrgId, OrgQuotas, Part, ProcContext, Processor,
    ResourceManager, StreamConfig, StreamContext,
};
use ekiben_server::run_stream_once;
use ekiben_server::runner::spawn_stream;
use ekiben_testkit::full_registry;

/// What the scripted input answers, one step per `read_batch`; past the script, end of input.
#[derive(Clone)]
enum Step {
    Batch(Vec<&'static str>),
    Timeout,
    Fail(&'static str),
    /// Never answers (ignores shutdown).
    Hang,
}

struct Scripted {
    steps: VecDeque<Step>,
    connect_fails: bool,
}

#[async_trait::async_trait]
impl ekiben_core::Input for Scripted {
    fn input_type(&self) -> &'static str {
        "scripted"
    }
    async fn connect(&mut self, _ctx: &StreamContext) -> Result<(), CoreError> {
        if self.connect_fails {
            return Err(CoreError::other("broker refused"));
        }
        Ok(())
    }
    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        match self.steps.pop_front() {
            None => Err(CoreError::EndOfInput),
            Some(Step::Batch(parts)) => Ok((
                parts.into_iter().map(Part::new).collect(),
                AckResponder::noop(),
            )),
            Some(Step::Timeout) => Err(CoreError::Timeout),
            Some(Step::Fail(why)) => Err(CoreError::other(why)),
            Some(Step::Hang) => {
                std::future::pending::<()>().await;
                Err(CoreError::EndOfInput)
            }
        }
    }
}

/// Collects what it writes; refuses to connect or to write when told to.
#[derive(Default)]
struct Collect {
    written: Mutex<Vec<String>>,
    connect_fails: bool,
    write_fails: bool,
    /// Never finishes a write.
    write_hangs: bool,
}

#[async_trait::async_trait]
impl ekiben_core::Output for Collect {
    fn output_type(&self) -> &'static str {
        "collect"
    }
    async fn connect(&self, _ctx: &StreamContext) -> Result<(), CoreError> {
        if self.connect_fails {
            return Err(CoreError::other("sink refused"));
        }
        Ok(())
    }
    async fn write_batch(&self, _ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        if self.write_fails {
            return Err(CoreError::other("disk full"));
        }
        if self.write_hangs {
            std::future::pending::<()>().await;
        }
        let mut w = self.written.lock().unwrap();
        for p in &batch {
            w.push(String::from_utf8_lossy(&p.as_bytes_ref()).into_owned());
        }
        Ok(())
    }
}

/// Burns CPU for 30 ms per batch.
struct Busy;

#[async_trait::async_trait]
impl Processor for Busy {
    fn processor_type(&self) -> &'static str {
        "busy"
    }
    async fn process_batch(
        &self,
        _ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let until = std::time::Instant::now() + Duration::from_millis(30);
        while std::time::Instant::now() < until {
            std::hint::spin_loop();
        }
        Ok(vec![batch])
    }
}

/// An output whose close panics: the stream's task dies before it hands back a summary.
struct PanicsOnClose;

#[async_trait::async_trait]
impl ekiben_core::Output for PanicsOnClose {
    fn output_type(&self) -> &'static str {
        "panics_on_close"
    }
    async fn write_batch(&self, _ctx: &StreamContext, _batch: Batch) -> Result<(), CoreError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), CoreError> {
        panic!("output bug");
    }
}

struct Rig {
    reg: Arc<ekiben_core::Registry>,
    out: Arc<Collect>,
}

fn rig(steps: Vec<Step>, input_fails: bool, out: Collect) -> Rig {
    let reg = full_registry();
    let script = Arc::new(Mutex::new(Some(steps)));
    reg.register_input("scripted", move |_, _| {
        let steps = script.lock().unwrap().take().unwrap_or_default();
        Ok(Box::new(Scripted {
            steps: steps.into(),
            connect_fails: input_fails,
        }))
    });
    let out = Arc::new(out);
    let shared = out.clone();
    reg.register_output("collect", move |_, _| {
        Ok(shared.clone() as Arc<dyn ekiben_core::Output>)
    });
    reg.register_processor("busy", |_, _| Ok(Arc::new(Busy)));
    reg.register_output("panics_on_close", |_, _| Ok(Arc::new(PanicsOnClose)));
    Rig { reg, out }
}

fn conf(extra: &str) -> StreamConfig {
    StreamConfig::from_yaml_or_json(&format!(
        "input:\n  scripted: {{}}\noutput:\n  collect: {{}}\n{extra}"
    ))
    .unwrap()
}

async fn run(rig: &Rig, conf: StreamConfig, resources: ResourceManager) -> ekiben_core::RunSummary {
    run_stream_once("job", conf, rig.reg.clone(), resources, None, false)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_input_that_does_not_connect_fails_the_run_with_its_reason() {
    let r = rig(vec![], true, Collect::default());
    let s = run(&r, conf(""), ResourceManager::new()).await;
    assert!(!s.success);
    assert_eq!(
        s.error.as_deref(),
        Some("input connect failed: broker refused")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_output_that_does_not_connect_fails_the_run_with_its_reason() {
    let out = Collect {
        connect_fails: true,
        ..Default::default()
    };
    let r = rig(vec![Step::Batch(vec!["a"])], false, out);
    let s = run(&r, conf(""), ResourceManager::new()).await;
    assert!(!s.success);
    assert_eq!(
        s.error.as_deref(),
        Some("output connect failed: sink refused")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_batches_and_timeouts_are_skipped_and_a_read_error_ends_the_run() {
    let steps = vec![
        Step::Batch(vec![]),
        Step::Timeout,
        Step::Batch(vec!["a", "bb"]),
        Step::Fail("connection reset"),
        Step::Batch(vec!["never"]),
    ];
    let r = rig(steps, false, Collect::default());
    let s = run(&r, conf(""), ResourceManager::new()).await;
    assert!(!s.success);
    assert_eq!(s.error.as_deref(), Some("connection reset"));
    assert_eq!((s.messages_in, s.bytes_in), (2, 3));
    assert_eq!(*r.out.written.lock().unwrap(), vec!["a", "bb"]);
    assert_eq!(s.messages_out, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_write_counts_failed_messages_and_fails_the_run() {
    let out = Collect {
        write_fails: true,
        ..Default::default()
    };
    let mut steps = vec![Step::Batch(vec!["a", "b"])];
    steps.extend((0..200).map(|_| Step::Batch(vec!["c"])));
    let r = rig(steps, false, out);
    let s = run(
        &r,
        conf("pipeline:\n  threads: 1\n"),
        ResourceManager::new(),
    )
    .await;
    assert!(!s.success);
    assert_eq!(s.error.as_deref(), Some("disk full"));
    assert_eq!(s.messages_out, 0);
    assert!(s.messages_failed >= 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_past_its_cpu_budget_stops_with_the_budget_named() {
    let steps = (0..6).map(|_| Step::Batch(vec!["x"])).collect();
    let r = rig(steps, false, Collect::default());
    let resources = ResourceManager::new();
    resources.set_org_quotas(
        &OrgId::default(),
        OrgQuotas {
            max_cpu_time_ms_per_run: 10,
            ..Default::default()
        },
    );
    let c = conf("pipeline:\n  threads: 1\n  processors:\n    - busy: {}\n");
    let s = run(&r, c, resources).await;
    assert!(!s.success);
    assert_eq!(
        s.error.as_deref(),
        Some("stream run exceeded per-run processing time budget (10 ms)")
    );
    assert!(r.out.written.lock().unwrap().len() < 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_that_outlives_its_timeout_answers_timeout() {
    let out = Collect {
        write_hangs: true,
        ..Default::default()
    };
    let r = rig(vec![Step::Batch(vec!["a"]), Step::Hang], false, out);
    let h = spawn_stream(
        "hang".into(),
        conf(""),
        r.reg.clone(),
        ResourceManager::new(),
        None,
        None,
        false,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.is_running());
    let stopped = h.stop(Duration::from_millis(50)).await;
    assert!(matches!(stopped, Err(CoreError::Timeout)), "{stopped:?}");
    // The join handle is gone: a second stop and a wait answer the last summary (none).
    assert!(h.stop(Duration::from_millis(50)).await.unwrap().is_none());
    assert!(h.wait_for_completion().await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finished_stream_keeps_its_summary_and_freezes_its_uptime() {
    let r = rig(vec![Step::Batch(vec!["a"])], false, Collect::default());
    let h = spawn_stream(
        "done".into(),
        conf(""),
        r.reg.clone(),
        ResourceManager::new(),
        None,
        None,
        false,
    )
    .unwrap();
    let first = h.wait_for_completion().await.expect("summary");
    assert!(first.success);
    assert!(!h.is_running());
    assert!(
        !h.is_ready(),
        "a finished stream's input and output are closed"
    );
    let frozen = h.uptime();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(h.uptime(), frozen);
    let again = h.wait_for_completion().await.expect("kept");
    assert_eq!(again.messages_in, first.messages_in);
    let stopped = h.stop(Duration::from_secs(1)).await.unwrap().expect("kept");
    assert_eq!(stopped.messages_out, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_stream_at_its_rate_limit_nacks_the_waiting_batch() {
    let steps = (0..50).map(|_| Step::Batch(vec!["m"])).collect();
    let r = rig(steps, false, Collect::default());
    let resources = ResourceManager::new();
    resources.set_org_quotas(
        &OrgId::default(),
        OrgQuotas {
            max_messages_per_sec: 1,
            ..Default::default()
        },
    );
    let h = spawn_stream(
        "slow".into(),
        conf(""),
        r.reg.clone(),
        resources,
        None,
        None,
        false,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let s = h
        .stop(Duration::from_secs(5))
        .await
        .unwrap()
        .expect("summary");
    assert!(s.messages_in < 50, "{s:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_resources_buffer_and_secret_refs_are_built_for_the_run() {
    let r = rig(vec![Step::Batch(vec!["a"])], false, Collect::default());
    let resources = ResourceManager::new();
    resources
        .get_or_create_org(&OrgId::default())
        .set_secret("API_KEY", "k-123");
    let c = conf(concat!(
        "secret_refs: [API_KEY, NOT_SET]\n",
        "buffer:\n  memory: {}\n",
        "pipeline:\n  processors:\n    - resource: up\n    - mapping: 'root = env(\"API_KEY\")'\n",
        "cache_resources:\n  - label: c\n    memory: {}\n  - memory: {}\n",
        "rate_limit_resources:\n  - label: rl\n    local:\n      count: 10\n      interval: 1s\n  - local: {}\n",
        "processor_resources:\n  - label: up\n    mapping: 'root = content().uppercase()'\n  - mapping: 'root = 1'\n",
        "input_resources:\n  - label: in\n    generate:\n      count: 1\n      mapping: 'root = 1'\n  - generate:\n      count: 1\n      mapping: 'root = 1'\n",
        "output_resources:\n  - label: out\n    drop: {}\n  - drop: {}\n",
    ));
    let s = run(&r, c, resources.clone()).await;
    assert!(s.success, "{:?}", s.error);
    assert_eq!(*r.out.written.lock().unwrap(), vec!["k-123"]);
    let org = OrgId::default();
    assert!(resources.get_cache(&org, "c").is_ok());
    assert!(resources.get_rate_limit(&org, "rl").is_ok());
    assert!(resources.get_output(&org, "out").is_ok());
}

/// A page over the encode-ahead size (8 MiB) is encoded by the writer, whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_over_eight_mib_is_written_whole() {
    let r = rig(vec![Step::Batch(vec!["ab"])], false, Collect::default());
    let c = conf("pipeline:\n  processors:\n    - mapping: 'root = content().string().repeat(4718593)'\n");
    let s = run(&r, c, ResourceManager::new()).await;
    assert!(s.success, "{:?}", s.error);
    let written = r.out.written.lock().unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].len(), 2 * 4_718_593, "over 8 MiB");
}

/// A `secret_refs` name the organization lacks is read from the stream's own scope (the id's
/// part before the first `.`), never from another scope's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_ref_falls_back_to_the_streams_scope() {
    let resources = ResourceManager::new();
    resources
        .get_or_create_org(&OrgId::default())
        .replace_scoped_secrets(HashMap::from([
            ("team".to_string(), HashMap::from([("TEAM_KEY".to_string(), "t-1".to_string())])),
            ("other".to_string(), HashMap::from([("OTHER_KEY".to_string(), "o-1".to_string())])),
        ]));
    let r = rig(vec![Step::Batch(vec!["a"])], false, Collect::default());
    let c = conf(concat!(
        "secret_refs: [TEAM_KEY, OTHER_KEY]\n",
        "pipeline:\n  processors:\n    - mapping: 'root = env(\"TEAM_KEY\") + \"/\" + env(\"OTHER_KEY\").or(\"none\")'\n",
    ));
    let s = run_stream_once("team.job", c, r.reg.clone(), resources, None, false)
        .await
        .unwrap();
    assert!(s.success, "{:?}", s.error);
    assert_eq!(*r.out.written.lock().unwrap(), vec!["t-1/none"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_component_that_does_not_build_refuses_the_spawn() {
    let r = rig(vec![], false, Collect::default());
    for extra in [
        "processor_resources:\n  - label: p\n    mapping: 'root = ('\n",
        "buffer:\n  no_such_buffer: {}\n",
        "pipeline:\n  processors:\n    - no_such_processor: {}\n",
    ] {
        let err = run_stream_once(
            "bad",
            conf(extra),
            r.reg.clone(),
            ResourceManager::new(),
            None,
            false,
        )
        .await
        .expect_err(extra);
        assert!(!err.to_string().is_empty(), "{extra}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_task_that_dies_answers_an_error_not_a_summary() {
    let close_panics = "input:\n  scripted: {}\noutput:\n  panics_on_close: {}\n";
    let r = rig(vec![Step::Batch(vec!["a"])], false, Collect::default());
    let conf = StreamConfig::from_yaml_or_json(close_panics).unwrap();
    let err = run_stream_once(
        "dies",
        conf.clone(),
        r.reg.clone(),
        ResourceManager::new(),
        None,
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        CoreError::other("stream task terminated without summary").to_string()
    );

    let r = rig(vec![Step::Batch(vec!["a"])], false, Collect::default());
    let h = spawn_stream(
        "dies".into(),
        conf.clone(),
        r.reg.clone(),
        ResourceManager::new(),
        None,
        None,
        false,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stopped = h.stop(Duration::from_secs(5)).await;
    assert!(matches!(stopped, Err(CoreError::Other(_))), "{stopped:?}");

    // A run-once stream through the manager answers the same error.
    let r = rig(vec![Step::Batch(vec!["a"])], false, Collect::default());
    let m = ekiben_server::StreamManager::new(r.reg.clone(), ResourceManager::new(), None, false);
    let mut once = conf;
    once.run = ekiben_core::RunMode::Once;
    let err = m.create("dies".into(), once).await.unwrap_err();
    assert!(
        err.to_string().contains("terminated without summary"),
        "{err}"
    );
}
