//! E2: Bento's Prometheus exporter, metric mapping and namespaces, ported from
//! `internal/impl/prometheus/metrics_prometheus_test.go`,
//! `internal/component/metrics/mapping_test.go` and `namespaced_test.go`, plus ekiben's
//! retention, cap, escaping and per-organization view.

use std::sync::Arc;

use ekiben_core::prometheus::{
    escape_label_value, MetricMapping, MetricsConfig, Namespaced, PromExporter, SeriesOwner,
};
use ekiben_core::{MetricsRegistry, StreamMetrics};
use serde_json::json;

fn prom(conf: serde_json::Value) -> Arc<PromExporter> {
    let c = MetricsConfig::from_json(&json!({ "prometheus": conf })).expect("config");
    Arc::new(PromExporter::new(c).expect("exporter"))
}

fn page(p: &PromExporter) -> String {
    format!("\n{}", p.render(&|_| true))
}

fn apply_test_metrics(nm: &Namespaced) {
    let ctr = nm.counter("counterone");
    ctr.incr(10);
    ctr.incr(11);
    nm.gauge("gaugeone").set(12);
    let ctr_two = nm.counter_vec("countertwo", &["label1"]);
    ctr_two.counter(&["value1"]).incr(10);
    ctr_two.counter(&["value2"]).incr(11);
    nm.gauge_vec("gaugetwo", &["label2"])
        .gauge(&["value3"])
        .set(12);
}

fn assert_contains_test_metrics(body: &str) {
    assert!(body.contains("\ncounterone 21"), "{body}");
    assert!(body.contains("\ngaugeone 12"), "{body}");
    assert!(
        body.contains("\ncountertwo{label1=\"value1\"} 10"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value2\"} 11"),
        "{body}"
    );
    assert!(body.contains("\ngaugetwo{label2=\"value3\"} 12"), "{body}");
}

#[test]
fn bento_metrics_prometheus_test_prometheus_no_push_gateway() {
    let c = MetricsConfig::from_json(&json!({"prometheus": {}})).expect("an empty config parses");
    assert_eq!(c, MetricsConfig::default());
    assert!(PromExporter::new(c).is_ok());
}

#[test]
fn bento_metrics_prometheus_test_prometheus_with_push_gateway() {
    // ekiben never pushes: /metrics is scraped. A push_url is refused, not silently ignored.
    let err = MetricsConfig::from_json(&json!({"prometheus": {"push_url": "http://gw:9091"}}))
        .expect_err("push_url is refused");
    assert!(err.to_string().contains("push_url"), "{err}");
}

#[test]
fn bento_metrics_prometheus_test_prometheus_with_push_gateway_and_push_interval() {
    let err = MetricsConfig::from_json(
        &json!({"prometheus": {"push_url": "http://gw:9091", "push_interval": "1ms"}}),
    )
    .expect_err("a push gateway is refused");
    assert!(err.to_string().contains("not supported"), "{err}");
}

#[test]
fn bento_metrics_prometheus_test_prometheus_metrics() {
    let p = prom(json!({}));
    let nm = Namespaced::bare(p.clone());
    let ctr = nm.counter("counterone");
    ctr.incr(10);
    ctr.incr(11);
    nm.gauge("gaugeone").set(12);
    nm.timer("timerone").timing(13);
    let ctr_two = nm.counter_vec("countertwo", &["label1"]);
    ctr_two.counter(&["value1"]).incr(10);
    ctr_two.counter(&["value2"]).incr(11);
    ctr_two.counter(&["value3"]).incr_f64(10.452);
    nm.gauge_vec("gaugetwo", &["label2"])
        .gauge(&["value3"])
        .set(12);
    nm.gauge("gaugethree").set_f64(10.452);
    nm.timer_vec("timertwo", &["label3", "label4"])
        .timer(&["value4", "value5"])
        .timing(13);

    let body = page(&p);
    assert!(body.contains("\ncounterone 21"), "{body}");
    assert!(body.contains("\ngaugeone 12"), "{body}");
    assert!(body.contains("\ntimerone_count 1"), "{body}");
    assert!(
        body.contains("\ncountertwo{label1=\"value1\"} 10"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value2\"} 11"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value3\"} 10.452"),
        "{body}"
    );
    assert!(body.contains("\ngaugetwo{label2=\"value3\"} 12"), "{body}");
    assert!(
        body.contains("\ntimertwo_sum{label3=\"value4\",label4=\"value5\"} 13"),
        "{body}"
    );
    assert!(body.contains("\ngaugethree 10.452"), "{body}");
    assert!(
        body.contains("# HELP counterone Bento Counter metric"),
        "{body}"
    );
    assert!(
        body.contains("# HELP gaugeone Bento Gauge metric"),
        "{body}"
    );
    assert!(
        body.contains("# HELP timerone Bento Timing metric"),
        "{body}"
    );
    assert!(body.contains("# TYPE timerone summary"), "{body}");
    assert!(body.contains("\ntimerone{quantile=\"0.5\"} 13"), "{body}");
}

#[test]
fn bento_metrics_prometheus_test_prometheus_hist_metrics() {
    let p = prom(json!({"use_histogram_timing": true}));
    let nm = Namespaced::bare(p.clone());
    apply_test_metrics(&nm);
    nm.timer("timerone").timing(13);
    nm.timer_vec("timertwo", &["label3", "label4"])
        .timer(&["value4", "value5"])
        .timing(14);
    let body = page(&p);
    assert_contains_test_metrics(&body);
    assert!(body.contains("\ntimerone_sum 1.3e-08"), "{body}");
    assert!(
        body.contains("\ntimertwo_sum{label3=\"value4\",label4=\"value5\"} 1.4e-08"),
        "{body}"
    );
    assert!(body.contains("# TYPE timerone histogram"), "{body}");
    assert!(body.contains("\ntimerone_bucket{le=\"0.005\"} 1"), "{body}");
    assert!(body.contains("\ntimerone_bucket{le=\"+Inf\"} 1"), "{body}");
    assert!(body.contains("\ntimerone_count 1"), "{body}");
}

#[test]
fn bento_metrics_prometheus_test_prometheus_with_file_output_path() {
    let err = MetricsConfig::from_json(
        &json!({"prometheus": {"file_output_path": "/tmp/bento_metrics.prom"}}),
    )
    .expect_err("writing metrics to a file is refused");
    assert!(err.to_string().contains("file_output_path"), "{err}");
}

#[test]
fn bento_metrics_prometheus_test_prometheus_default_registry() {
    // There is no Go default registry to share; the field parses and the metrics still render.
    let p = prom(json!({"use_default_registry": true}));
    apply_test_metrics(&Namespaced::bare(p.clone()));
    assert_contains_test_metrics(&page(&p));
}

#[test]
fn bento_mapping_test_path_mapping() {
    struct Case {
        input: &'static str,
        in_labels: &'static [&'static str],
        in_values: &'static [&'static str],
        output: &'static str,
        labels: &'static [&'static str],
        values: &'static [&'static str],
    }
    let c = |input, output| Case {
        input,
        in_labels: &[],
        in_values: &[],
        output,
        labels: &[],
        values: &[],
    };
    let tests: Vec<(&str, Vec<Case>)> = vec![
        (
            r#"if this.contains("foo") { deleted() }"#,
            vec![
                c("foo", ""),
                c("foo", ""),
                c("hello foo world", ""),
                c("hello world", "hello world"),
                Case {
                    input: "hello world",
                    in_labels: &["foo", "bar"],
                    in_values: &["foo1", "bar1"],
                    output: "hello world",
                    labels: &["bar", "foo"],
                    values: &["bar1", "foo1"],
                },
            ],
        ),
        (r#"root = throw("nope")"#, vec![c("foo", "foo")]),
        (
            "root = this\n meta foo = \"bar\"",
            vec![
                Case {
                    labels: &["foo"],
                    values: &["bar"],
                    ..c("foo", "foo")
                },
                Case {
                    in_labels: &["a", "b"],
                    in_values: &["a1", "b1"],
                    labels: &["a", "b", "foo"],
                    values: &["a1", "b1", "bar"],
                    ..c("foo", "foo")
                },
            ],
        ),
        (
            "root = this\n meta foo = \"bar\"\n meta bar = \"baz\"",
            vec![Case {
                labels: &["bar", "foo"],
                values: &["baz", "bar"],
                ..c("foo", "foo")
            }],
        ),
        (
            r#"this.replace_all("foo","bar")"#,
            vec![
                c("foo", "bar"),
                c("hello foo world", "hello bar world"),
                c("hello world", "hello world"),
            ],
        ),
        ("", vec![c("foo", "foo"), c("hello world", "hello world")]),
        (
            "root = 10",
            vec![c("foo", "foo"), c("hello world", "hello world")],
        ),
    ];
    for (mapping, cases) in tests {
        let m = MetricMapping::new(mapping).expect(mapping);
        for (i, def) in cases.iter().enumerate() {
            let names: Vec<String> = def.in_labels.iter().map(|s| s.to_string()).collect();
            let values: Vec<String> = def.in_values.iter().map(|s| s.to_string()).collect();
            let (out, labels, vals) = m.map_path(def.input, &names, &values);
            assert_eq!(out, def.output, "{mapping} case {i}");
            assert_eq!(labels, def.labels, "{mapping} case {i}");
            assert_eq!(vals, def.values, "{mapping} case {i}");
        }
    }
}

fn hist_prom() -> Arc<PromExporter> {
    prom(json!({"use_histogram_timing": true}))
}

#[test]
fn bento_namespaced_test_namespaced_nothing() {
    let p = hist_prom();
    let nm = Namespaced::bare(p.clone());
    let ctr = nm.counter("counterone");
    ctr.incr(10);
    ctr.incr(11);
    nm.gauge("gaugeone").set(12);
    nm.timer("timerone").timing(13);
    let ctr_two = nm.counter_vec("countertwo", &["label1"]);
    ctr_two.counter(&["value1"]).incr(10);
    ctr_two.counter(&["value2"]).incr(11);
    ctr_two.counter(&["value3"]).incr_f64(10.452);
    nm.gauge_vec("gaugetwo", &["label2"])
        .gauge(&["value3"])
        .set(12);
    nm.timer_vec("timertwo", &["label3", "label4"])
        .timer(&["value4", "value5"])
        .timing(13);
    let body = page(&p);
    assert!(body.contains("\ncounterone 21"), "{body}");
    assert!(body.contains("\ngaugeone 12"), "{body}");
    assert!(body.contains("\ntimerone_sum 1.3e-08"), "{body}");
    assert!(
        body.contains("\ncountertwo{label1=\"value1\"} 10"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value2\"} 11"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value3\"} 10.452"),
        "{body}"
    );
    assert!(body.contains("\ngaugetwo{label2=\"value3\"} 12"), "{body}");
    assert!(
        body.contains("\ntimertwo_sum{label3=\"value4\",label4=\"value5\"} 1.3e-08"),
        "{body}"
    );
}

#[test]
fn bento_namespaced_test_namespaced_prefix() {
    let p = hist_prom();
    let nm = Namespaced::bare(p.clone());
    apply_test_metrics(&nm);
    nm.timer("timerone").timing(13);
    nm.timer_vec("timertwo", &["label3", "label4"])
        .timer(&["value4", "value5"])
        .timing(13);
    nm.counter("counterthree").incr(22);
    let body = page(&p);
    assert_contains_test_metrics(&body);
    assert!(body.contains("\ntimerone_sum 1.3e-08"), "{body}");
    assert!(
        body.contains("\ntimertwo_sum{label3=\"value4\",label4=\"value5\"} 1.3e-08"),
        "{body}"
    );
    assert!(body.contains("\ncounterthree 22"), "{body}");
}

#[test]
fn bento_namespaced_test_namespaced_prefix_static_labels() {
    let p = hist_prom();
    let nm = Namespaced::bare(p.clone()).with_labels(&[("static1", "svalue1")]);
    let ctr = nm.counter("counterone");
    ctr.incr(10);
    ctr.incr(11);
    nm.gauge("gaugeone").set(12);
    nm.timer("timerone").timing(13);
    let ctr_two = nm.counter_vec("countertwo", &["label1"]);
    ctr_two.counter(&["value1"]).incr(10);
    ctr_two.counter(&["value2"]).incr(11);
    nm.gauge_vec("gaugetwo", &["label2"])
        .gauge(&["value3"])
        .set(12);
    nm.timer_vec("timertwo", &["label3", "label4"])
        .timer(&["value4", "value5"])
        .timing(13);
    let nm2 = nm.with_labels(&[("static2", "svalue2")]);
    nm2.counter("counterthree").incr(22);
    let body = page(&p);
    assert!(
        body.contains("\ncounterone{static1=\"svalue1\"} 21"),
        "{body}"
    );
    assert!(
        body.contains("\ngaugeone{static1=\"svalue1\"} 12"),
        "{body}"
    );
    assert!(
        body.contains("\ntimerone_sum{static1=\"svalue1\"} 1.3e-08"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value1\",static1=\"svalue1\"} 10"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value2\",static1=\"svalue1\"} 11"),
        "{body}"
    );
    assert!(
        body.contains("\ngaugetwo{label2=\"value3\",static1=\"svalue1\"} 12"),
        "{body}"
    );
    assert!(
        body.contains(
            "\ntimertwo_sum{label3=\"value4\",label4=\"value5\",static1=\"svalue1\"} 1.3e-08"
        ),
        "{body}"
    );
    assert!(
        body.contains("\ncounterthree{static1=\"svalue1\",static2=\"svalue2\"} 22"),
        "{body}"
    );
}

#[test]
fn bento_namespaced_test_namespaced_prefix_static_labels_with_mappings() {
    let p = hist_prom();
    let foo_to_bar =
        Arc::new(MetricMapping::new(r#"root = this.replace_all("foo","bar")"#).unwrap());
    let bar_to_baz =
        Arc::new(MetricMapping::new(r#"root = this.replace_all("bar","baz")"#).unwrap());
    let nm = Namespaced::bare(p.clone())
        .with_labels(&[("static1", "svalue1")])
        .with_mapping(bar_to_baz)
        .with_mapping(foo_to_bar);
    let ctr = nm.counter("counter");
    ctr.incr(10);
    ctr.incr(11);
    nm.gauge("gauge").set(12);
    nm.timer("timer").timing(13);
    let ctr_two = nm.counter_vec("countertwo", &["label1"]);
    ctr_two.counter(&["value1"]).incr(10);
    ctr_two.counter(&["value2"]).incr(11);
    nm.gauge_vec("gaugetwo", &["label2"])
        .gauge(&["value3"])
        .set(12);
    nm.timer_vec("timertwo", &["label3", "label4"])
        .timer(&["value4", "value5"])
        .timing(13);
    let body = page(&p);
    assert!(body.contains("\ncounter{static1=\"svalue1\"} 21"), "{body}");
    assert!(body.contains("\ngauge{static1=\"svalue1\"} 12"), "{body}");
    assert!(
        body.contains("\ntimer_sum{static1=\"svalue1\"} 1.3e-08"),
        "{body}"
    );
    assert!(
        body.contains("\ncountertwo{label1=\"value1\",static1=\"svalue1\"} 10"),
        "{body}"
    );
    assert!(
        body.contains("\ngaugetwo{label2=\"value3\",static1=\"svalue1\"} 12"),
        "{body}"
    );
    assert!(
        body.contains(
            "\ntimertwo_sum{label3=\"value4\",label4=\"value5\",static1=\"svalue1\"} 1.3e-08"
        ),
        "{body}"
    );
}

#[test]
fn bento_namespaced_test_namespaced_prefix_static_labels_with_mapping_labels() {
    let p = hist_prom();
    let foo_to_bar = Arc::new(
        MetricMapping::new(
            "meta = meta().map_each(kv -> kv.value.replace_all(\"value\",\"bar\"))\nmeta extra1 = \"extravalue1\"\nroot = this.replace_all(\"foo\",\"bar\")",
        )
        .unwrap(),
    );
    let bar_to_baz = Arc::new(
        MetricMapping::new(
            "meta = meta().map_each(kv -> kv.value.replace_all(\"bar\",\"baz\"))\nmeta extra2 = \"extravalue2\"\nroot = this.replace_all(\"bar\",\"baz\")",
        )
        .unwrap(),
    );
    let nm = Namespaced::bare(p.clone())
        .with_labels(&[("static1", "svalue1")])
        .with_mapping(bar_to_baz)
        .with_mapping(foo_to_bar);
    let ctr = nm.counter("counter");
    ctr.incr(10);
    ctr.incr(11);
    nm.gauge("gauge").set(12);
    nm.timer("timer").timing(13);
    let ctr_two = nm.counter_vec("countertwo", &["label1"]);
    ctr_two.counter(&["value1"]).incr(10);
    ctr_two.counter(&["value2"]).incr(11);
    nm.gauge_vec("gaugetwo", &["label2"])
        .gauge(&["value3"])
        .set(12);
    nm.timer_vec("timertwo", &["label3", "label4"])
        .timer(&["value4", "value5"])
        .timing(13);
    let body = page(&p);
    let ex = "extra1=\"extravalue1\",extra2=\"extravalue2\"";
    assert!(
        body.contains(&format!("\ncounter{{{ex},static1=\"sbaz1\"}} 21")),
        "{body}"
    );
    assert!(
        body.contains(&format!("\ngauge{{{ex},static1=\"sbaz1\"}} 12")),
        "{body}"
    );
    assert!(
        body.contains(&format!("\ntimer_sum{{{ex},static1=\"sbaz1\"}} 1.3e-08")),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "\ncountertwo{{{ex},label1=\"value1\",static1=\"sbaz1\"}} 10"
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "\ncountertwo{{{ex},label1=\"value2\",static1=\"sbaz1\"}} 11"
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "\ngaugetwo{{{ex},label2=\"value3\",static1=\"sbaz1\"}} 12"
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "\ntimertwo_sum{{{ex},label3=\"value4\",label4=\"value5\",static1=\"sbaz1\"}} 1.3e-08"
        )),
        "{body}"
    );
}

#[test]
fn a_mapping_deleting_a_series_drops_it_and_a_renaming_one_renames_it() {
    let c = MetricsConfig {
        mapping: r#"root = if this == "input_received" { deleted() } else { "x_" + this }"#
            .to_string(),
        ..MetricsConfig::default()
    };
    let p = Arc::new(PromExporter::new(c).unwrap());
    let nm = Namespaced::new(p.clone()).with_labels(&[("stream", "s")]);
    let gone = nm.counter("input_received");
    gone.incr(5);
    assert!(gone.is_noop());
    nm.counter("output_sent").incr(2);
    let body = page(&p);
    assert!(!body.contains("input_received"), "{body}");
    assert!(body.contains("\nx_output_sent{stream=\"s\"} 2"), "{body}");
}

#[test]
fn a_mapping_that_does_not_parse_is_refused() {
    let c = MetricsConfig {
        mapping: "root = this.(".to_string(),
        ..MetricsConfig::default()
    };
    assert!(PromExporter::new(c).is_err());
}

#[test]
fn label_values_are_escaped_so_a_stream_id_cannot_forge_a_series() {
    assert_eq!(escape_label_value("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    let p = prom(json!({}));
    let nm = Namespaced::bare(p.clone()).with_labels(&[("stream", "x\"} 999\nevil_metric{a=\"1")]);
    nm.counter("input_received").incr(1);
    let body = p.render(&|_| true);
    assert!(
        !body.lines().any(|l| l.starts_with("evil_metric")),
        "{body}"
    );
    assert!(
        body.contains("input_received{stream=\"x\\\"} 999\\nevil_metric{a=\\\"1\"} 1"),
        "{body}"
    );
}

#[test]
fn invalid_metric_and_label_names_are_ignored() {
    let p = prom(json!({}));
    let nm = Namespaced::bare(p.clone());
    assert!(nm.counter("bad name").is_noop());
    assert!(nm
        .with_labels(&[("bad-label", "v")])
        .counter("ok")
        .is_noop());
    assert_eq!(p.series_count(), 0);
}

fn owned(p: &Arc<PromExporter>, org: &str, stream: &str) -> Namespaced {
    Namespaced::bare(p.clone())
        .with_labels(&[("stream", stream)])
        .with_owner(SeriesOwner {
            org: org.to_string(),
            stream: stream.to_string(),
        })
}

#[test]
fn a_deleted_streams_counters_stay_and_continue_and_its_timings_go() {
    let p = prom(json!({}));
    let owner = SeriesOwner {
        org: "default".into(),
        stream: "s".into(),
    };
    let nm = owned(&p, "default", "s");
    nm.counter("input_received").incr(3);
    nm.timer("input_latency_ns").timing(10);
    p.stream_deleted(&owner);
    let body = page(&p);
    assert!(body.contains("\ninput_received{stream=\"s\"} 3"), "{body}");
    assert!(!body.contains("input_latency_ns"), "{body}");
    assert!(p.is_deleted(&owner));

    p.stream_created(&owner);
    owned(&p, "default", "s").counter("input_received").incr(2);
    assert!(page(&p).contains("\ninput_received{stream=\"s\"} 5"));
}

#[test]
fn the_series_cap_evicts_the_longest_deleted_stream_first_and_refuses_past_it() {
    let c = MetricsConfig {
        max_series: 3,
        ..MetricsConfig::default()
    };
    let p = Arc::new(PromExporter::new(c).unwrap());
    owned(&p, "o", "first").counter("c").incr(1);
    owned(&p, "o", "second").counter("c").incr(1);
    owned(&p, "o", "live").counter("c").incr(1);
    p.stream_deleted(&SeriesOwner {
        org: "o".into(),
        stream: "first".into(),
    });
    p.stream_deleted(&SeriesOwner {
        org: "o".into(),
        stream: "second".into(),
    });

    owned(&p, "o", "new1").counter("c").incr(1);
    let body = page(&p);
    assert!(
        !body.contains("stream=\"first\""),
        "the longest-deleted goes first: {body}"
    );
    assert!(body.contains("stream=\"second\""), "{body}");
    assert_eq!(p.series_count(), 3);

    owned(&p, "o", "new2").counter("c").incr(1);
    assert!(!page(&p).contains("stream=\"second\""));
    // Nothing deleted is left: a new series is refused rather than growing past the cap.
    let refused = owned(&p, "o", "new3").counter("c");
    assert!(refused.is_noop());
    assert_eq!(p.series_count(), 3);
    assert!(page(&p).contains("stream=\"live\""));
}

#[test]
fn an_organization_sees_only_its_own_series() {
    let reg = MetricsRegistry::new();
    let a = StreamMetrics::new("ingest", "org-a");
    let b = StreamMetrics::new("ingest", "org-b");
    let d = StreamMetrics::new("plain", "default");
    reg.register_stream(a.clone());
    reg.register_stream(b.clone());
    reg.register_stream(d.clone());
    for m in [&a, &b, &d] {
        m.component("", "root.input")
            .unwrap()
            .counter("input_received")
            .incr(1);
    }
    let a_view = reg.render_prometheus_for_org("org-a");
    assert!(a_view.contains("org=\"org-a\""), "{a_view}");
    assert!(!a_view.contains("org-b"), "{a_view}");
    assert!(!a_view.contains("stream=\"plain\""), "{a_view}");
    let all = reg.render_prometheus();
    assert!(
        all.contains("org-b") && all.contains("stream=\"plain\""),
        "{all}"
    );
    // The default organization keeps Bento's label set: no `org` label.
    assert!(
        all.contains("input_received{label=\"\",path=\"root.input\",stream=\"plain\"} 1"),
        "{all}"
    );
}

#[test]
fn metrics_config_reads_bentos_fields_and_rejects_unknown_ones() {
    let c = MetricsConfig::from_json(&json!({
        "prometheus": {
            "use_histogram_timing": true,
            "histogram_buckets": [1.0, 0.5],
            "add_process_metrics": true
        },
        "mapping": "root = this",
        "max_series": 10
    }))
    .unwrap();
    assert!(c.use_histogram_timing && c.add_process_metrics);
    assert_eq!(c.histogram_buckets, vec![0.5, 1.0]);
    assert_eq!(c.max_series, 10);
    assert!(MetricsConfig::from_json(&json!({"statsd": {}})).is_err());
    assert!(MetricsConfig::from_json(&json!({"prometheus": {"nope": 1}})).is_err());
    assert!(MetricsConfig::from_json(&json!({"max_series": 0})).is_err());
}

#[test]
fn a_poisoned_lock_does_not_take_metrics_down() {
    let p = prom(json!({}));
    let nm = Namespaced::bare(p.clone());
    let t = nm.timer("t");
    let p2 = p.clone();
    let _ = std::thread::spawn(move || {
        let _guard = p2.render(&|_| panic!("poison the render path"));
    })
    .join();
    t.timing(5);
    nm.counter("after").incr(1);
    assert!(page(&p).contains("\nafter 1"));
}
