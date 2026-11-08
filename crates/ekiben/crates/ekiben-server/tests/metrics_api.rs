//! E2 through the server: Bento's metric names on `/metrics`, `metrics.mapping` and histogram
//! timing from `--set` / `-r` / `--metrics`, counters kept after a stream is deleted.

use std::path::PathBuf;

use ekiben_core::{MetricsRegistry, ResourceManager};
use ekiben_server::{
    build_router, load_metrics_config, ServerAuthConfig, ServerState, StreamManager,
};
use ekiben_testkit::full_registry;
use reqwest::StatusCode;
use tokio::net::TcpListener;

async fn serve(metrics: MetricsRegistry) -> String {
    let manager = StreamManager::new(full_registry(), ResourceManager::new(), None, false)
        .with_metrics_registry(metrics);
    let app = build_router(ServerState::new(manager, ServerAuthConfig::default()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

const ONCE: &str = r#"
run: once
input:
  generate:
    count: 3
    interval: ""
    mapping: 'root = {"v": 1}'
pipeline:
  processors:
    - label: step
      mapping: 'root.v = this.v + 1'
output:
  drop: {}
"#;

#[test]
fn set_arguments_build_the_metrics_config() {
    let sets = vec![
        "metrics.prometheus.use_histogram_timing=true".to_string(),
        "metrics.prometheus.histogram_buckets=[0.1, 1]".to_string(),
        "metrics.mapping=root = \"x_\" + this".to_string(),
        "logger.level=debug".to_string(),
    ];
    let c = load_metrics_config(None, &[], &sets).unwrap();
    assert!(c.use_histogram_timing);
    assert_eq!(c.histogram_buckets, vec![0.1, 1.0]);
    assert_eq!(c.mapping, "root = \"x_\" + this");
    assert!(load_metrics_config(
        None,
        &[],
        &["metrics.prometheus.push_url=http://gw".to_string()]
    )
    .is_err());
    assert!(load_metrics_config(None, &[], &["metrics.prometheus".to_string()]).is_err());
}

#[test]
fn a_resources_file_or_a_metrics_file_holds_the_section_and_set_wins() {
    let dir = tempfile::tempdir().unwrap();
    let res: PathBuf = dir.path().join("res.yaml");
    std::fs::write(
        &res,
        "metrics:\n  prometheus:\n    use_histogram_timing: true\n  max_series: 50\n",
    )
    .unwrap();
    let c = load_metrics_config(None, std::slice::from_ref(&res), &[]).unwrap();
    assert!(c.use_histogram_timing);
    assert_eq!(c.max_series, 50);
    let alone: PathBuf = dir.path().join("metrics.yaml");
    std::fs::write(&alone, "prometheus:\n  add_process_metrics: true\n").unwrap();
    let c = load_metrics_config(
        Some(&alone),
        &[],
        &["metrics.prometheus.add_process_metrics=false".to_string()],
    )
    .unwrap();
    assert!(!c.add_process_metrics);
}

#[tokio::test]
async fn metrics_use_bentos_names_and_survive_the_streams_deletion() {
    let base = serve(MetricsRegistry::new()).await;
    let c = reqwest::Client::new();
    let res = c
        .post(format!("{base}/streams/m1"))
        .body(ONCE)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let page = c
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        page.contains("# HELP input_received Bento Counter metric"),
        "{page}"
    );
    assert!(
        page.contains("input_received{label=\"\",path=\"root.input\",stream=\"m1\"} 3"),
        "{page}"
    );
    assert!(
        page.contains("output_sent{label=\"\",path=\"root.output\",stream=\"m1\"} 3"),
        "{page}"
    );
    assert!(
        page.contains("processor_received{label=\"step\",path=\"root.pipeline.processors.0\",stream=\"m1\"} 3"),
        "{page}"
    );
    assert!(!page.contains("ekiben_"), "{page}");

    let res = c.delete(format!("{base}/streams/m1")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let after = c
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        after.contains("input_received{label=\"\",path=\"root.input\",stream=\"m1\"} 3"),
        "a deleted stream's counters stay: {after}"
    );
    assert!(
        !after.contains("input_latency_ns_sum{label=\"\",path=\"root.input\",stream=\"m1\"}"),
        "{after}"
    );
}

#[tokio::test]
async fn a_mapping_and_histogram_timing_apply_to_every_stream() {
    let conf = load_metrics_config(
        None,
        &[],
        &[
            "metrics.prometheus.use_histogram_timing=true".to_string(),
            "metrics.mapping=root = if this.has_prefix(\"processor_\") { deleted() } else { \"jc_\" + this }".to_string(),
        ],
    )
    .unwrap();
    let base = serve(MetricsRegistry::with_config(conf).unwrap()).await;
    let c = reqwest::Client::new();
    let res = c
        .post(format!("{base}/streams/m2"))
        .body(ONCE)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let page = c
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("jc_input_received{"), "{page}");
    assert!(
        page.contains("# TYPE jc_output_latency_ns histogram"),
        "{page}"
    );
    assert!(!page.contains("processor_"), "{page}");
}
