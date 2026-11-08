#![forbid(unsafe_code)]
//! The Prometheus driver against the Prometheus HTTP API (`/api/v1/query`, `query_range`,
//! `labels`, `label/{name}/values`, `series`) and Grafana's `promlib` semantics: variable
//! interpolation, step choice, legend names, table format, notices and error answers.

mod common;

use common::{ORG, Reply, closed_port, ctx, loopback_guard, mock, q};
use daiya_model::{NoticeSeverity, Subject};
use daiya_query::prometheus::{
    align_time_ms, escape_promql_var, format_legend, format_prom_interval,
    interpolate_promql_variables, parse_label_values_call,
};
use daiya_query::{
    Datasource, DsError, HealthState, MapSecretResolver, PrometheusDatasource, Query, ResourceReq,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn driver(base: &str) -> PrometheusDatasource {
    PrometheusDatasource::new(
        "prom",
        format!("{base}/"),
        loopback_guard(),
        Arc::new(MapSecretResolver::new()),
    )
}

fn matrix(result: Value) -> Reply {
    Reply::json(
        200,
        &json!({ "status": "success", "data": { "resultType": "matrix", "result": result } }),
    )
}

fn param(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

#[test]
fn builtin_variables_follow_grafana_promlib() {
    let out = interpolate_promql_variables(
        "rate(x[$__rate_interval]) $__interval ${__interval} $__interval_ms ${__interval_ms} \
         $__range $__range_s $__range_ms ${__range} $__rate_interval_ms",
        30_000,
        15_000,
        3_600_000,
        &BTreeMap::new(),
    )
    .unwrap();
    // rate interval = max(step + scrape, 4 * scrape) = max(45s, 60s).
    assert_eq!(
        out,
        "rate(x[1m]) 30s 30s 30000 30000 3600s 3600 3600000 3600s 60000"
    );
    // Steps under a second count as one second; the scrape interval floors at one second too.
    assert_eq!(
        interpolate_promql_variables("$__interval $__rate_interval", 10, 10, 0, &BTreeMap::new())
            .unwrap(),
        "1s 4s"
    );
}

#[test]
fn template_variables_are_replaced_once_and_by_whole_name() {
    let v = vars(&[("job", "api"), ("jobname", "edge"), ("evil", "$job")]);
    let out = interpolate_promql_variables(
        r#"up{job="$job", name="$jobname", x="${job}", y="[[job]]", z="${job:raw}", w="$evil", u="$unknown"}"#,
        15_000,
        15_000,
        60_000,
        &v,
    )
    .unwrap();
    assert_eq!(
        out,
        r#"up{job="api", name="edge", x="api", y="api", z="api", w="$job", u="$unknown"}"#
    );
}

#[test]
fn a_variable_value_cannot_leave_its_literal() {
    // Double and single quotes: the enclosing quote and the backslash are escaped.
    let v = vars(&[("v", r#"a"} or vector(1) or {b="\"#)]);
    let out = interpolate_promql_variables(r#"up{job="$v"}"#, 15_000, 15_000, 60_000, &v).unwrap();
    assert_eq!(out, r#"up{job="a\"} or vector(1) or {b=\"\\"}"#);
    let v = vars(&[("v", "it's\nhere\r")]);
    let out = interpolate_promql_variables("up{job='$v'}", 15_000, 15_000, 60_000, &v).unwrap();
    assert_eq!(out, r"up{job='it\'s\nhere\r'}");
    // A double quote inside a single-quoted literal needs no escape (and gets none).
    let v = vars(&[("v", r#"say "hi""#)]);
    let out = interpolate_promql_variables("up{job='$v'}", 15_000, 15_000, 60_000, &v).unwrap();
    assert_eq!(out, r#"up{job='say "hi"'}"#);
    // An escaped quote inside the literal does not end it.
    let v = vars(&[("v", r#"x""#)]);
    let out = interpolate_promql_variables(r#"up{a="q\"$v"}"#, 15_000, 15_000, 60_000, &v).unwrap();
    assert_eq!(out, r#"up{a="q\"x\""}"#);

    // A raw string cannot hold a backtick; any other value stands as it is.
    let v = vars(&[("v", "a` or vector(1) or `")]);
    let err = interpolate_promql_variables("up{job=`$v`}", 15_000, 15_000, 60_000, &v).unwrap_err();
    assert!(err.contains("'v'") && err.contains("backtick"), "{err}");
    let v = vars(&[("v", r#"C:\x"y"#)]);
    assert_eq!(
        interpolate_promql_variables("up{job=`$v`}", 15_000, 15_000, 60_000, &v).unwrap(),
        r#"up{job=`C:\x"y`}"#
    );

    // Outside a literal only names, numbers and durations pass.
    for ok in ["5m", "instance", "-1.5", "node:cpu", ""] {
        let v = vars(&[("v", ok)]);
        assert_eq!(
            interpolate_promql_variables("topk($v, up)", 15_000, 15_000, 60_000, &v).unwrap(),
            format!("topk({ok}, up)")
        );
    }
    for bad in [
        "1) or vector(1",
        "a b",
        "x-secret_metric",
        "{a=\"b\"}",
        "1,2",
    ] {
        let v = vars(&[("v", bad)]);
        assert!(
            interpolate_promql_variables("topk($v, up)", 15_000, 15_000, 60_000, &v).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn escaping_by_context() {
    assert_eq!(escape_promql_var(r"a\b", Some('"')).unwrap(), r"a\\b");
    assert_eq!(escape_promql_var("a'b", Some('"')).unwrap(), "a'b");
    assert_eq!(escape_promql_var("a'b", Some('\'')).unwrap(), r"a\'b");
    assert!(escape_promql_var("a`b", Some('`')).is_err());
    assert!(escape_promql_var("a(b", None).is_err());
}

#[test]
fn intervals_legends_alignment_and_label_values_calls() {
    for (ms, s) in [
        (0, "1ms"),
        (500, "500ms"),
        (1_500, "1500ms"),
        (15_000, "15s"),
        (90_000, "90s"),
        (120_000, "2m"),
        (7_200_000, "2h"),
        (172_800_000, "2d"),
    ] {
        assert_eq!(format_prom_interval(ms), s);
    }
    assert_eq!(align_time_ms(1_700_000_007_000, 15_000), 1_699_999_995_000);
    assert_eq!(align_time_ms(-1, 1_000), -1_000);
    assert_eq!(align_time_ms(1234, 0), 1234);

    let labels: BTreeMap<String, String> = [
        ("__name__", "http_requests_total"),
        ("method", "GET"),
        ("status", "200"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    assert_eq!(
        format_legend(Some("{{ method }} {{status}} {{missing}}!"), &labels, "e"),
        "GET 200 !"
    );
    let auto = r#"http_requests_total{method="GET", status="200"}"#;
    assert_eq!(format_legend(None, &labels, "e"), auto);
    assert_eq!(format_legend(Some(""), &labels, "e"), auto);
    assert_eq!(format_legend(Some("__auto"), &labels, "e"), auto);
    let only_name: BTreeMap<String, String> = [("__name__".to_owned(), "up".to_owned())].into();
    assert_eq!(format_legend(None, &only_name, "e"), "up");
    assert_eq!(format_legend(None, &BTreeMap::new(), "sum(up)"), "sum(up)");

    assert_eq!(
        parse_label_values_call(" label_values(up{job=\"a\"}, instance) "),
        Some((Some("up{job=\"a\"}".into()), "instance".into()))
    );
    assert_eq!(
        parse_label_values_call("label_values(job)"),
        Some((None, "job".into()))
    );
    assert_eq!(parse_label_values_call("label_values()"), None);
    assert_eq!(parse_label_values_call("query_result(up)"), None);
}

#[tokio::test]
async fn a_range_query_asks_query_range_on_the_aligned_grid_and_names_series() {
    let m = mock(vec![(
        "/api/v1/query_range",
        Reply::json(
            200,
            &json!({ "status": "success",
                "warnings": ["query touched many series"], "infos": ["partial histogram"],
                "data": { "resultType": "matrix", "result": [
                    { "metric": { "__name__": "up", "job": "api" },
                      "values": [[1700000000, "1"], [1700000015.5, "NaN"], [1700000030, "+Inf"], [1700000045, "x"]] },
                    { "metric": { "job": "db" }, "values": [[1700000000, "0"], "broken", [1]] }
            ]}}),
        ),
    )])
    .await;
    let mut c = ctx(Subject::anonymous());
    c.variables = vars(&[("job", "api")]);
    let queries = vec![
        Query {
            ref_id: "H".into(),
            hide: true,
            model: json!({ "expr": "hidden" }),
        },
        Query {
            ref_id: "A".into(),
            hide: false,
            model: json!({ "expr": "up{job=\"$job\"}[$__interval]", "legendFormat": "{{job}}" }),
        },
    ];
    let sets = driver(&m.base).query(&c, &queries).await.unwrap();
    assert_eq!(sets.len(), 1, "the hidden target is not run");
    assert_eq!(m.count(), 1);
    let target = m.target(0);
    assert!(target.starts_with("/api/v1/query_range?"), "{target}");
    assert_eq!(param(&target, "query").unwrap(), "up{job=\"api\"}[15s]");
    assert_eq!(param(&target, "start").unwrap(), "1699999995.000");
    assert_eq!(param(&target, "end").unwrap(), "1700000595.000");
    assert_eq!(param(&target, "step").unwrap(), "15.000");
    assert!(
        m.head(0)
            .to_ascii_lowercase()
            .contains("accept: application/json")
    );

    let set = &sets[0];
    assert_eq!(set.ref_id, "A");
    assert_eq!(set.frames.len(), 2);
    let api = &set.frames[0];
    assert_eq!(api.name, "api");
    assert_eq!(api.ref_id.as_deref(), Some("A"));
    assert_eq!(
        api.fields[0].values,
        vec![
            json!(1_700_000_000_000_i64),
            json!(1_700_000_015_500_i64),
            json!(1_700_000_030_000_i64),
            json!(1_700_000_045_000_i64)
        ]
    );
    // NaN and an unparsable sample have no number; +Inf has none in JSON either.
    assert_eq!(api.fields[1].f64_at(0), Some(1.0));
    assert_eq!(api.fields[1].f64_at(3), None);
    assert_eq!(
        api.fields[1].labels.get("job").map(String::as_str),
        Some("api")
    );
    assert_eq!(api.fields[1].display_name.as_deref(), Some("api"));
    assert_eq!(api.meta.custom["resultType"], "matrix");
    assert_eq!(
        api.meta.executed_query_string.as_deref(),
        Some("up{job=\"api\"}[15s]")
    );
    let severities: Vec<NoticeSeverity> = api.meta.notices.iter().map(|n| n.severity).collect();
    assert_eq!(
        severities,
        vec![NoticeSeverity::Warning, NoticeSeverity::Info]
    );
    assert_eq!(api.meta.notices[0].text, "query touched many series");
    assert_eq!(
        set.frames[1].meta.notices.len(),
        2,
        "every frame carries them"
    );
    // Malformed sample pairs are skipped.
    assert_eq!(set.frames[1].fields[0].values.len(), 1);
}

#[tokio::test]
async fn step_takes_the_largest_of_interval_scrape_safe_step_and_min_step() {
    let m = mock(vec![("/api/v1/query_range", matrix(json!([])))]).await;
    let d = driver(&m.base);
    // 30 days at 11000 points: the safe step (range / 11000, in ms) wins over the 15s interval.
    let mut c = ctx(Subject::anonymous());
    c.from_ms = 0;
    c.to_ms = 30 * 86_400_000;
    d.query(&c, &q(json!({ "expr": "up" }))).await.unwrap();
    assert_eq!(param(&m.target(0), "step").unwrap(), "235.636");

    // The target's min step (legacy `>` form too) and intervalFactor raise it.
    let c = ctx(Subject::anonymous());
    d.query(&c, &q(json!({ "expr": "up", "interval": ">1m" })))
        .await
        .unwrap();
    assert_eq!(param(&m.target(1), "step").unwrap(), "60.000");
    d.query(
        &c,
        &q(json!({ "expr": "up", "interval": "30s", "intervalFactor": 2 })),
    )
    .await
    .unwrap();
    assert_eq!(param(&m.target(2), "step").unwrap(), "60.000");
    // A min step that is no duration (a variable left over) is ignored.
    d.query(&c, &q(json!({ "expr": "up", "interval": "$x" })))
        .await
        .unwrap();
    assert_eq!(param(&m.target(3), "step").unwrap(), "15.000");

    // The datasource's scrape interval floors the step.
    let mut slow = driver(&m.base);
    slow.scrape_interval_ms = 120_000;
    slow.query(&c, &q(json!({ "expr": "up" }))).await.unwrap();
    assert_eq!(param(&m.target(4), "step").unwrap(), "120.000");
}

#[tokio::test]
async fn an_instant_query_asks_query_at_the_range_end() {
    let m = mock(vec![(
        "/api/v1/query?",
        Reply::json(
            200,
            &json!({ "status": "success", "data": { "resultType": "vector", "result": [
                { "metric": { "job": "a" }, "value": [1700000600, "3"] },
                { "metric": { "job": "b" }, "value": [1700000600] }
            ]}}),
        ),
    )])
    .await;
    let sets = driver(&m.base)
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "expr": "up", "instant": true })),
        )
        .await
        .unwrap();
    let target = m.target(0);
    assert_eq!(param(&target, "time").unwrap(), "1700000600.000");
    assert_eq!(param(&target, "query").unwrap(), "up");
    let frames = &sets[0].frames;
    assert_eq!(frames[0].fields[1].f64_at(0), Some(3.0));
    assert_eq!(frames[0].meta.custom["resultType"], "vector");
    assert!(
        frames[1].fields[0].values.is_empty(),
        "a broken pair is no point"
    );
}

#[tokio::test]
async fn the_table_format_gives_one_frame_with_a_column_per_label() {
    let m = mock(vec![(
        "/api/v1/query",
        Reply::json(
            200,
            &json!({ "status": "success", "data": { "resultType": "matrix", "result": [
                { "metric": { "job": "a", "instance": "x" }, "values": [[1, "1"], [2, "2"]] },
                { "metric": { "job": "b" }, "value": [3, "4"] },
                { "metric": {} }
            ]}}),
        ),
    )])
    .await;
    let sets = driver(&m.base)
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "expr": "up", "format": "table" })),
        )
        .await
        .unwrap();
    let frame = &sets[0].frames[0];
    let names: Vec<&str> = frame.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["Time", "instance", "job", "Value #A"]);
    assert_eq!(
        frame.fields[0].values,
        vec![json!(2000), json!(3000), json!(0)]
    );
    assert_eq!(
        frame.fields[1].values,
        vec![json!("x"), Value::Null, Value::Null]
    );
    assert_eq!(
        frame.fields[3].f64_at(0),
        Some(2.0),
        "the last sample of a range"
    );
    assert_eq!(frame.fields[3].f64_at(1), Some(4.0));
    assert_eq!(frame.fields[3].f64_at(2), None);
    assert_eq!(
        frame.meta.preferred_visualisation_type.as_deref(),
        Some("table")
    );
}

#[tokio::test]
async fn upstream_errors_carry_the_prometheus_error_text() {
    let m = mock(vec![
        (
            "/api/v1/query_range?query=bad",
            Reply::json(
                400,
                &json!({ "status": "error", "errorType": "bad_data", "error": "parse error at char 4" }),
            ),
        ),
        (
            "/api/v1/query_range?query=down",
            Reply::raw(503, &"x".repeat(500)),
        ),
        (
            "/api/v1/query_range?query=html",
            Reply::raw(200, "<html>proxy</html>"),
        ),
        (
            "/api/v1/query_range?query=failed",
            Reply::json(200, &json!({ "status": "error", "error": "timeout" })),
        ),
        (
            "/api/v1/query_range?query=nomsg",
            Reply::json(200, &json!({ "status": "error" })),
        ),
    ])
    .await;
    let d = driver(&m.base);
    let c = ctx(Subject::anonymous());
    let run = |expr: &'static str| {
        let d = &d;
        let c = &c;
        async move { d.query(c, &q(json!({ "expr": expr }))).await.unwrap_err() }
    };
    match run("bad").await {
        DsError::Upstream { status, message } => {
            assert_eq!(status, 400);
            assert_eq!(message, "parse error at char 4");
        }
        other => panic!("{other}"),
    }
    match run("down").await {
        DsError::Upstream { status, message } => {
            assert_eq!(status, 503);
            assert_eq!(message.len(), 200, "the body is cut");
        }
        other => panic!("{other}"),
    }
    let err = run("html").await;
    assert!(
        matches!(&err, DsError::Upstream { status: 502, message } if message.contains("invalid JSON")),
        "{err}"
    );
    assert!(run("failed").await.to_string().contains("timeout"));
    assert!(
        run("nomsg")
            .await
            .to_string()
            .contains("prometheus query failed")
    );

    // A target without `expr` and a value that cannot stand in its place are refused before
    // anything is sent.
    let before = m.count();
    let err = d
        .query(&c, &q(json!({ "legendFormat": "x" })))
        .await
        .unwrap_err();
    assert!(matches!(err, DsError::InvalidQuery { .. }), "{err}");
    let mut evil = ctx(Subject::anonymous());
    evil.variables = vars(&[("n", "1) or vector(1")]);
    let err = d
        .query(&evil, &q(json!({ "expr": "topk($n, up)" })))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DsError::InvalidQuery { ref_id, reason } if ref_id == "A" && reason.contains("'n'")),
        "{err}"
    );
    assert_eq!(m.count(), before);
}

#[tokio::test]
async fn credentials_come_from_the_secret_ref_and_never_reach_an_error() {
    let m = mock(vec![("/api/v1/", matrix(json!([])))]).await;
    let mut secrets = MapSecretResolver::new();
    secrets.insert(ORG, "prom-token", "s3cr3t-token");
    let d = PrometheusDatasource::new("prom", m.base.clone(), loopback_guard(), Arc::new(secrets))
        .with_secret_ref("prom-token");
    d.query(&ctx(Subject::anonymous()), &q(json!({ "expr": "up" })))
        .await
        .unwrap();
    assert!(
        m.head(0)
            .to_ascii_lowercase()
            .contains("authorization: bearer s3cr3t-token")
    );

    // A secretRef the organization does not hold: nothing is sent, the health says why.
    let missing = PrometheusDatasource::new(
        "prom",
        m.base.clone(),
        loopback_guard(),
        Arc::new(MapSecretResolver::new()),
    )
    .with_secret_ref("prom-token");
    let err = missing
        .query(&ctx(Subject::anonymous()), &q(json!({ "expr": "up" })))
        .await
        .unwrap_err();
    assert!(matches!(err, DsError::SecretResolution { .. }), "{err}");
    let health = missing.health(&ctx(Subject::anonymous())).await;
    assert_eq!(health.status, HealthState::Error);
    assert!(health.message.contains("credentials"));
    let err = missing
        .resource(
            &ctx(Subject::anonymous()),
            ResourceReq {
                path: "labels".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DsError::SecretResolution { .. }), "{err}");
    assert_eq!(m.count(), 1);
}

#[tokio::test]
async fn health_asks_one_plus_one() {
    let ok = mock(vec![(
        "/api/v1/query?query=1%2B1",
        Reply::json(200, &json!({ "status": "success" })),
    )])
    .await;
    let h = driver(&ok.base).health(&ctx(Subject::anonymous())).await;
    assert_eq!(h.status, HealthState::Ok, "{}", h.message);

    let unauthorized = mock(vec![("/", Reply::raw(401, "no"))]).await;
    let h = driver(&unauthorized.base)
        .health(&ctx(Subject::anonymous()))
        .await;
    assert_eq!(h.status, HealthState::Error);
    assert!(h.message.contains("401"), "{}", h.message);

    let port = closed_port().await;
    let h = driver(&format!("http://127.0.0.1:{port}"))
        .health(&ctx(Subject::anonymous()))
        .await;
    assert_eq!(h.status, HealthState::Error);
    assert!(h.message.contains("unreachable"), "{}", h.message);
}

#[tokio::test]
async fn editor_resources_map_to_the_metadata_api() {
    let m = mock(vec![
        (
            "/api/v1/labels",
            Reply::json(
                200,
                &json!({ "status": "success", "data": ["job", "instance"] }),
            ),
        ),
        (
            "/api/v1/label/",
            Reply::json(200, &json!({ "status": "success", "data": ["a"] })),
        ),
        (
            "/api/v1/series",
            Reply::json(200, &json!({ "status": "success", "data": [] })),
        ),
    ])
    .await;
    let d = driver(&m.base);
    let c = ctx(Subject::anonymous());
    let call = |path: &str, params: &[(&str, &str)]| {
        let r = ResourceReq {
            path: path.into(),
            params: vars(params),
        };
        let d = &d;
        let c = &c;
        async move { d.resource(c, r).await }
    };

    let labels = call("/labels/", &[]).await.unwrap();
    assert_eq!(labels.data["data"], json!(["job", "instance"]));
    assert_eq!(m.target(0), "/api/v1/labels");

    call("label/job/values", &[]).await.unwrap();
    assert_eq!(m.target(1), "/api/v1/label/job/values");
    call("label/job", &[("metric", "up{a=\"b\"}")])
        .await
        .unwrap();
    assert_eq!(m.target(2), "/api/v1/label/job/values?match[]=up{a=\"b\"}");
    call("label/inst ance/values", &[("match[]", "node")])
        .await
        .unwrap();
    assert_eq!(m.target(3), "/api/v1/label/inst ance/values?match[]=node");

    call("series", &[]).await.unwrap();
    assert_eq!(m.target(4), "/api/v1/series?match[]=up");
    call("series", &[("match[]", "node_load1")]).await.unwrap();
    assert_eq!(m.target(5), "/api/v1/series?match[]=node_load1");

    // Template variable queries: label_values(metric, label) and label_values(label).
    call("variable", &[("query", "label_values(up, job)")])
        .await
        .unwrap();
    assert_eq!(m.target(6), "/api/v1/label/job/values?match[]=up");
    call("variable", &[("query", "label_values(job)")])
        .await
        .unwrap();
    assert_eq!(m.target(7), "/api/v1/label/job/values");

    // The paths Grafana's frontend sends, with the API prefix; nothing else passes with it.
    call("/api/v1/labels", &[]).await.unwrap();
    assert_eq!(m.target(8), "/api/v1/labels");
    call("api/v1/label/job/values", &[]).await.unwrap();
    assert_eq!(m.target(9), "/api/v1/label/job/values");
    call("api/v1/series", &[("match[]", "up")]).await.unwrap();
    assert_eq!(m.target(10), "/api/v1/series?match[]=up");
    for refused in [
        "api/v1/admin/tsdb/delete_series",
        "api/v1/query",
        "api/v2/labels",
    ] {
        let err = call(refused, &[]).await.unwrap_err();
        assert!(
            err.to_string().contains("unsupported resource path"),
            "{refused}: {err}"
        );
    }

    let err = call("variable", &[("query", "query_result(up)")])
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported Prometheus resource query")
    );
    let err = call("metrics/../admin", &[]).await.unwrap_err();
    assert!(err.to_string().contains("unsupported resource path"));
    assert_eq!(m.count(), 11);
}

#[tokio::test]
async fn resource_errors_are_errors_not_data() {
    let m = mock(vec![
        (
            "/api/v1/labels",
            Reply::json(
                422,
                &json!({ "status": "error", "errorType": "execution", "error": "too many series" }),
            ),
        ),
        ("/api/v1/series", Reply::raw(200, "not json")),
    ])
    .await;
    let d = driver(&m.base);
    let c = ctx(Subject::anonymous());
    let err = d
        .resource(
            &c,
            ResourceReq {
                path: "labels".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        DsError::Upstream {
            status: 422,
            message: "too many series".into()
        }
    );
    let err = d
        .resource(
            &c,
            ResourceReq {
                path: "series".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid JSON"), "{err}");
}

#[test]
fn import_drops_exemplars_with_a_warning_and_export_is_its_inverse() {
    let d = driver("https://prom.example");
    assert_eq!(d.kind(), "prometheus");
    let target = json!({ "refId": "B", "expr": "up", "exemplar": true, "hide": true, "legendFormat": "{{job}}" });
    let imported = d.import(&target).unwrap();
    assert_eq!(imported.query.ref_id, "B");
    assert!(imported.query.hide);
    assert!(imported.query.model.get("exemplar").is_none());
    assert_eq!(imported.warnings.len(), 1);
    assert!(imported.warnings[0].contains("exemplar"));
    let exported = d.export(&imported.query);
    let mut expected = target.clone();
    expected.as_object_mut().unwrap().remove("exemplar");
    assert_eq!(exported, expected);

    // exemplar: false is dropped silently; refId defaults to A; hide false is not written.
    let plain = d
        .import(&json!({ "expr": "up", "exemplar": false }))
        .unwrap();
    assert!(plain.warnings.is_empty());
    assert_eq!(plain.query.ref_id, "A");
    assert_eq!(
        d.export(&plain.query),
        json!({ "expr": "up", "refId": "A" })
    );
    // A model that is no object exports only its refId.
    let odd = Query {
        ref_id: "C".into(),
        hide: false,
        model: json!("up"),
    };
    assert_eq!(d.export(&odd), json!({ "refId": "C" }));

    assert!(matches!(
        d.import(&json!(["up"])),
        Err(DsError::InvalidQuery { .. })
    ));

    let editor = d.editor();
    assert_eq!(editor.kind, "prometheus");
    assert_eq!(editor.json_schema["required"], json!(["expr"]));
    assert_eq!(
        editor.field_hints[0].resource_path.as_deref(),
        Some("labels")
    );
}
