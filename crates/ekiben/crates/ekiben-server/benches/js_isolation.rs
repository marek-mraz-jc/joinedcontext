//! T-7082: how much a `javascript` processor spinning on `while (true) {}` slows other streams.
//! The victim is pipeline_bench's mapping + branch pipeline, timed per call on a runtime with
//! the runner's worker count; spinners call the javascript processor in a loop on the same
//! runtime, as the workers of a stream whose every batch runs to its deadline. Rounds interleave
//! the cases so host load hits each alike. Prints p50/p99 per case and the regression of each
//! against the baseline; run with `cargo bench -p ekiben-server --bench js_isolation`.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, ComponentConfig, OrgId, Part, ProcContext, Processor, ResourceManager,
    StreamMetrics,
};
use ekiben_server::build_full_registry;
use tokio::sync::watch;

const CALLS: usize = 2000;
const ROUNDS: usize = 5;

fn component(v: serde_json::Value) -> ComponentConfig {
    ComponentConfig::from_json_value(&v, "noop", None).expect("component")
}

fn main() {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("runtime");
    let reg = build_full_registry();
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
    let victim = reg
        .build_processor_list(
            &[
                component(serde_json::json!({"mapping": "root.id = this.id\nroot.name = this.name.trim().uppercase()"})),
                component(serde_json::json!({"branch": {
                    "request_map": "root = {\"x\": this.score}",
                    "processors": [{"mapping": "root.res = this.x * 2 + 1"}],
                    "result_map": "root.computed = this.res"
                }})),
            ],
            &pctx,
        )
        .expect("victim pipeline");
    let spin: Arc<dyn Processor> = reg
        .build_processor_list(
            &[component(
                serde_json::json!({"javascript": {"code": "while (true) {}", "timeout_ms": 200}}),
            )],
            &pctx,
        )
        .expect("javascript")
        .remove(0);

    let time = |spinners: usize| -> (Duration, Duration) {
        let stop = Arc::new(AtomicBool::new(false));
        let handles: Vec<_> = (0..spinners)
            .map(|_| {
                let (spin, pctx, stop) = (spin.clone(), pctx.clone(), stop.clone());
                rt.spawn(async move {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = spin.process_batch(&pctx, vec![Part::from_str("x")]).await;
                    }
                })
            })
            .collect();
        // Let every spinner reach its script.
        std::thread::sleep(Duration::from_millis(50));
        let mut took: Vec<Duration> = rt.block_on(async {
            let mut took = Vec::with_capacity(CALLS);
            for _ in 0..CALLS {
                let batch = vec![Part::from_str(
                    r#"{"id":"p-1","name":"  station  ","score":21}"#,
                )];
                let at = Instant::now();
                let res = execute_all(&pctx, &victim, vec![batch])
                    .await
                    .expect("victim runs");
                took.push(at.elapsed());
                std::hint::black_box(res);
            }
            took
        });
        stop.store(true, Ordering::Relaxed);
        rt.block_on(async {
            for h in handles {
                let _ = h.await;
            }
        });
        took.sort_unstable();
        (took[CALLS / 2], took[CALLS * 99 / 100])
    };

    let cases = [0, 1, workers / 2, workers];
    let mut p50 = vec![Vec::new(); cases.len()];
    let mut p99 = vec![Vec::new(); cases.len()];
    for _ in 0..ROUNDS {
        for (i, n) in cases.iter().enumerate() {
            let (a, b) = time(*n);
            p50[i].push(a);
            p99[i].push(b);
        }
    }
    let median = |v: &mut Vec<Duration>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    let base = (median(&mut p50[0]), median(&mut p99[0]));
    println!("workers {workers}, {CALLS} calls x {ROUNDS} rounds, median of the rounds");
    for (i, n) in cases.iter().enumerate() {
        let (a, b) = (median(&mut p50[i]), median(&mut p99[i]));
        println!(
            "spinners {n:>2}: p50 {a:>9.1?} ({:+.1} %)  p99 {b:>9.1?} ({:+.1} %)",
            (a.as_secs_f64() / base.0.as_secs_f64() - 1.0) * 100.0,
            (b.as_secs_f64() / base.1.as_secs_f64() - 1.0) * 100.0
        );
    }
}
