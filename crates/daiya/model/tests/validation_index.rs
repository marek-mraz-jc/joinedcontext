#![forbid(unsafe_code)]
//! Dashboard validation against the CUE rules and R12 caps, the §7.1 verdict matrix, the
//! normalized dashboard index (R1) and the byte-stable export (R2).

use daiya_model::validation::html_escape;
use daiya_model::{
    CanonicalJson, CrError, DashboardDocument, InputLimits, Verdict, classify_and_validate,
    sanitize_markdown_to_html,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn doc(raw: &Value) -> DashboardDocument {
    serde_json::from_value(raw.clone()).unwrap()
}

fn validate(raw: &Value) -> Result<daiya_model::ValidationReport, CrError> {
    classify_and_validate(
        raw.to_string().len(),
        &doc(raw),
        &BTreeSet::new(),
        &InputLimits::default(),
    )
}

fn unprocessable_pointer(err: CrError) -> String {
    match err {
        CrError::Unprocessable { errors, .. } => errors.join(";"),
        other => panic!("expected 422, got {other:?}"),
    }
}

fn panel(kind: &str) -> Value {
    json!({"type": kind, "id": 1, "gridPos": {"h": 4, "w": 12, "x": 0, "y": 0}})
}

fn code_verdicts(report: &daiya_model::ValidationReport) -> Vec<(String, Verdict)> {
    report
        .findings
        .iter()
        .map(|f| (f.code.clone(), f.verdict))
        .collect()
}

#[test]
fn schema_rules_refuse_bad_titles_uids_months_tooltips_and_sizes() {
    let limits = InputLimits::default();
    let ok = json!({"title": "t"});
    let err = classify_and_validate(
        limits.max_dashboard_json_bytes + 1,
        &doc(&ok),
        &BTreeSet::new(),
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("exceeds maximum"), "{err}");

    assert_eq!(
        unprocessable_pointer(validate(&json!({"title": "   "})).unwrap_err()),
        "/title: title is required"
    );
    assert!(validate(&json!({})).is_err());
    assert!(
        unprocessable_pointer(validate(&json!({"title": "t", "uid": "a/b"})).unwrap_err())
            .starts_with("/uid")
    );
    let long_uid = "a".repeat(41);
    assert!(validate(&json!({"title": "t", "uid": long_uid})).is_err());
    let max_uid = "a".repeat(40);
    assert!(validate(&json!({"title": "t", "uid": max_uid})).is_ok());
    assert!(
        unprocessable_pointer(
            validate(&json!({"title": "t", "fiscalYearStartMonth": 12})).unwrap_err()
        )
        .starts_with("/fiscalYearStartMonth")
    );
    assert!(validate(&json!({"title": "t", "fiscalYearStartMonth": 11})).is_ok());
    assert!(
        unprocessable_pointer(validate(&json!({"title": "t", "graphTooltip": 3})).unwrap_err())
            .starts_with("/graphTooltip")
    );
    assert!(validate(&json!({"title": "t", "graphTooltip": 2})).is_ok());
}

#[test]
fn dangerous_link_schemes_are_refused_at_dashboard_and_panel_level() {
    let link = |url: &str| json!({"title": "l", "type": "link", "url": url});
    let err = validate(
        &json!({"title": "t", "links": [link("https://ok.example"), link("javascript:alert(1)")]}),
    )
    .unwrap_err();
    assert_eq!(
        unprocessable_pointer(err),
        "/links/1/url: disallowed URL scheme"
    );
    assert!(validate(&json!({"title": "t", "links": [link("https://ok.example")]})).is_ok());

    let mut p = panel("table");
    p["links"] = json!([link("/d/x"), link(" vbscript:msgbox")]);
    let err = validate(&json!({"title": "t", "panels": [p]})).unwrap_err();
    assert_eq!(
        unprocessable_pointer(err),
        "/panels/0/links/1/url: disallowed URL scheme"
    );
}

#[test]
fn grid_positions_outside_the_24_column_grid_are_refused() {
    for gp in [
        json!({"h": 0, "w": 12, "x": 0, "y": 0}),
        json!({"h": 4, "w": 0, "x": 0, "y": 0}),
        json!({"h": 4, "w": 25, "x": 0, "y": 0}),
        json!({"h": 4, "w": 12, "x": 24, "y": 0}),
    ] {
        let err = validate(&json!({"title": "t", "panels": [{"type": "stat", "gridPos": gp}]}))
            .unwrap_err();
        assert_eq!(
            unprocessable_pointer(err),
            "/panels/0/gridPos: out of 24-column grid bounds"
        );
    }
    let err = validate(&json!({"title": "t", "panels": [{"type": " "}]})).unwrap_err();
    assert_eq!(
        unprocessable_pointer(err),
        "/panels/0/type: must not be empty"
    );
}

#[test]
fn panel_and_target_caps_count_panels_inside_rows() {
    let limits = InputLimits {
        max_panels_per_dashboard: 2,
        max_targets_per_panel: 1,
        ..InputLimits::default()
    };
    let raw = json!({"title": "t", "panels": [
        panel("stat"),
        {"type": "row", "panels": [panel("stat"), panel("gauge")]}
    ]});
    let err = classify_and_validate(1, &doc(&raw), &BTreeSet::new(), &limits).unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("3 panels"), "{err}");

    let mut p = panel("stat");
    p["targets"] = json!([{"refId": "A"}, {"refId": "B"}]);
    let raw = json!({"title": "t", "panels": [p]});
    let err = classify_and_validate(1, &doc(&raw), &BTreeSet::new(), &limits).unwrap_err();
    assert!(err.to_string().contains("2 targets"), "{err}");
}

#[test]
fn verdict_matrix_for_panels_variables_datasources_expressions_and_transformations() {
    let mut bargauge_lcd = panel("bargauge");
    bargauge_lcd["options"] = json!({"displayMode": "lcd"});
    let mut bargauge_basic = panel("bargauge");
    bargauge_basic["options"] = json!({"displayMode": "basic"});
    let mut geomap_markers = panel("geomap");
    geomap_markers["options"] = json!({"layers": [{"type": "markers"}]});
    let mut geomap_heat = panel("geomap");
    geomap_heat["options"] = json!({"layers": [{"type": "markers"}, {"type": "heatmap"}]});
    let mut library = panel("library-panel-ref");
    library["libraryPanel"] = json!({"name": "Shared", "uid": "lib1"});
    let mut queries = panel("timeseries");
    queries["datasource"] = json!({"type": "prometheus", "uid": "prom"});
    queries["targets"] = json!([
        {"refId": "A", "exemplar": true},
        {"refId": "B", "datasource": {"type": "influxdb", "uid": "influx"}},
        {"refId": "C", "datasource": {"type": "__expr__", "uid": "__expr__"}, "type": "math"},
        {"refId": "D", "datasource": {"uid": "-100"}, "type": "sql"},
        {"refId": "E", "datasource": {"type": "__expr__"}, "type": "prql"},
        {"refId": "F", "datasource": {"uid": "__expr__"}},
        {"refId": "G", "datasource": {"type": "wasm:acme", "uid": "acme1"}},
        {"refId": "H", "datasource": {"type": "datasource", "uid": "-- Mixed --"}}
    ]);
    queries["transformations"] = json!([
        {"id": "organize", "options": {}},
        {"id": "partitionByValues", "options": {}}
    ]);
    let raw = json!({
        "title": "Matrix",
        "uid": "matrix",
        "templating": {"list": [
            {"type": "query", "name": "job", "datasource": {"type": "prometheus", "uid": "prom-vars"}},
            {"type": "adhoc", "name": "filters", "datasource": {"uid": "-- Mixed --"}},
            {"type": "custom", "name": "c", "datasource": {"uid": ""}}
        ]},
        "panels": [
            panel("stat"),
            bargauge_lcd,
            bargauge_basic,
            geomap_markers,
            geomap_heat,
            library,
            panel("jc-vegalite"),
            panel("grafana-worldmap-panel"),
            queries,
            {"type": "row", "collapsed": true, "panels": [panel("text")]},
            {"type": "row"}
        ]
    });
    let report = validate(&raw).unwrap();
    assert!(report.valid);
    assert_eq!(report.uid, "matrix");
    assert_eq!(report.title, "Matrix");
    let got = code_verdicts(&report);
    let expect = [
        ("variable.query", Verdict::Supports),
        ("variable.adhoc", Verdict::Refuses),
        ("variable.custom", Verdict::Supports),
        ("panel.stat", Verdict::Supports),
        ("bargauge.displayMode", Verdict::Maps),
        ("panel.geomap", Verdict::Maps),
        ("geomap.layerType", Verdict::Refuses),
        ("panel.libraryPanel", Verdict::Maps),
        ("panel.library-panel-ref", Verdict::Maps),
        ("panel.jc-vegalite", Verdict::Maps),
        ("panel.unsupported.grafana-worldmap-panel", Verdict::Refuses),
        ("panel.timeseries", Verdict::Supports),
        ("datasource.prometheus", Verdict::Supports),
        ("prometheus.exemplar", Verdict::Maps),
        ("datasource.unsupported.influxdb", Verdict::Refuses),
        ("expr.math", Verdict::Supports),
        ("expr.sql", Verdict::Maps),
        ("expr.unsupported.prql", Verdict::Refuses),
        ("datasource.wasm:acme", Verdict::Supports),
        ("datasource.datasource", Verdict::Supports),
        ("transformation.organize", Verdict::Supports),
        (
            "transformation.unsupported.partitionByValues",
            Verdict::Refuses,
        ),
        ("panel.row", Verdict::Supports),
        ("panel.text", Verdict::Supports),
    ];
    for (code, verdict) in expect {
        assert!(
            got.contains(&(code.to_owned(), verdict)),
            "missing {code} {verdict:?} in {got:?}"
        );
    }
    // A target without an expression type defaults to math (Grafana's default).
    assert_eq!(
        got.iter().filter(|(c, _)| c == "expr.math").count(),
        2,
        "{got:?}"
    );
    let bargauge_msgs: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.code == "bargauge.displayMode")
        .map(|f| f.message.as_str())
        .collect();
    assert_eq!(
        bargauge_msgs,
        [
            "bargauge.displayMode mapped to SDK bar list (basic)",
            "bargauge mapped to SDK bar list"
        ]
    );
    let refusal_codes: BTreeSet<&str> = report.refusals().iter().map(|f| f.code.as_str()).collect();
    assert!(refusal_codes.contains("variable.adhoc"));
    assert!(report.mappings().iter().all(|f| f.verdict == Verdict::Maps));
    assert!(!report.mappings().is_empty());

    // Datasources referenced by variables, panels and targets; never the expression or mixed
    // pseudo-datasources.
    assert_eq!(
        report.referenced_datasources,
        ["acme1", "influx", "prom", "prom-vars"]
    );
    assert!(report.missing_datasources.is_empty());
}

#[test]
fn referenced_datasources_missing_from_a_known_list_are_refused() {
    let mut p = panel("timeseries");
    p["datasource"] = json!({"type": "prometheus", "uid": "prom"});
    p["targets"] = json!([{"refId": "A", "datasource": {"type": "loki", "uid": "gone"}}]);
    let raw = json!({"title": "t", "panels": [p]});
    let known: BTreeSet<String> = ["prom".to_owned()].into();
    let report = classify_and_validate(1, &doc(&raw), &known, &InputLimits::default()).unwrap();
    assert_eq!(report.missing_datasources, ["gone"]);
    let refusal = report
        .refusals()
        .into_iter()
        .find(|f| f.code == "datasource.missing")
        .unwrap();
    assert_eq!(refusal.message, "datasource not found: gone");

    // An offline check with no list of known datasources reports none as missing.
    let report = validate(&raw).unwrap();
    assert!(report.missing_datasources.is_empty());
}

#[test]
fn markdown_is_rendered_with_every_html_character_escaped() {
    let html = sanitize_markdown_to_html(
        "# Title & <b>\n## Sub \"q\"\n### Small 'x'\n\n  plain <a href=\"javascript:x\">  \n",
    );
    assert_eq!(
        html,
        "<h1>Title &amp; &lt;b&gt;</h1>\n<h2>Sub &quot;q&quot;</h2>\n<h3>Small &#x27;x&#x27;</h3>\n<p>plain &lt;a href=&quot;javascript:x&quot;&gt;</p>\n"
    );
    assert_eq!(
        html_escape("a&b<c>d\"e'f"),
        "a&amp;b&lt;c&gt;d&quot;e&#x27;f"
    );
    assert_eq!(sanitize_markdown_to_html(""), "");
}

#[test]
fn the_index_lists_panels_datasources_variables_and_expressions() {
    let raw = json!({
        "uid": "idx",
        "title": "  Edge, Core: Latency (p99)!  ",
        "tags": ["slo", "edge"],
        "templating": {"list": [
            {"type": "query", "name": "job", "datasource": {"uid": "prom-vars"}},
            {"type": "custom", "name": "env", "datasource": {"uid": "-- Mixed --"}},
            {"type": "custom", "name": "x", "datasource": {"uid": "__expr__"}},
            {"type": "custom", "name": "y"}
        ]},
        "panels": [
            {"type": "timeseries", "datasource": {"uid": "prom"},
             "targets": [
                {"refId": "A", "datasource": {"uid": "loki"}},
                {"refId": "B", "datasource": {"uid": "__expr__"}},
                {"refId": "C", "datasource": {"uid": ""}},
                {"refId": "D"}
             ],
             "transformations": [{"id": "organize", "options": {}}, {"id": "merge", "options": {}}]},
            {"type": "stat", "datasource": {"uid": "-100"}},
            {"type": "stat", "datasource": {"uid": "-- Mixed --"},
             "targets": [{"refId": "A", "datasource": {"uid": "-100"}}, {"refId": "B", "datasource": {"uid": "-- Mixed --"}}]},
            {"type": "row", "panels": [{"type": "table", "datasource": {"uid": "pg"}}]},
            {"type": "row"}
        ]
    });
    let idx = doc(&raw).extract_index();
    assert_eq!(idx.uid, "idx");
    assert_eq!(idx.slug, "edge-core-latency-p99");
    assert_eq!(idx.tags, ["slo", "edge"]);
    assert_eq!(idx.panel_count, 4);
    assert_eq!(idx.datasource_uids, ["loki", "pg", "prom", "prom-vars"]);
    assert_eq!(idx.variable_names, ["job", "env", "x", "y"]);
    assert_eq!(idx.panel_types, ["stat", "table", "timeseries"]);
    assert_eq!(idx.transformation_ids, ["merge", "organize"]);
    assert!(idx.has_expressions);

    let empty = doc(&json!({})).extract_index();
    assert_eq!(empty.uid, "");
    assert_eq!(empty.slug, "dashboard");
    assert_eq!(empty.panel_count, 0);
    assert!(empty.variable_names.is_empty());
    assert!(!empty.has_expressions);
    assert_eq!(
        doc(&json!({"title": "!!!"})).extract_index().slug,
        "dashboard"
    );
}

#[test]
fn export_drops_the_id_sets_version_42_sorts_keys_and_keeps_unknown_fields() {
    let raw = json!({
        "zeta": {"b": 1, "a": [{"y": 1, "x": 2}]},
        "id": 17,
        "uid": "exp",
        "title": "Export",
        "schemaVersion": 39,
        "panels": [{"type": "stat", "gridPos": {}, "customThing": true}],
        "timepicker": {"nowDelay": "1m", "hidden": false},
        "time": {"from": "now-1h", "to": "now", "raw": true},
        "annotations": {"list": [{"name": "Annotations & Alerts", "builtIn": 1}]}
    });
    let d = doc(&raw);
    // Grafana's gridPos defaults.
    let gp = d.panels.as_ref().unwrap()[0].grid_pos.as_ref().unwrap();
    assert_eq!((gp.h, gp.w, gp.x, gp.y), (9, 12, 0, 0));
    // Annotation queries are enabled unless they say otherwise.
    assert!(d.annotations.as_ref().unwrap().list[0].enable);

    let out = d.to_export_value();
    assert!(out.get("id").is_none());
    assert_eq!(out["schemaVersion"], 42);
    assert_eq!(out["zeta"]["a"][0], json!({"x": 2, "y": 1}));
    assert_eq!(out["panels"][0]["customThing"], true);
    assert_eq!(out["time"]["raw"], true);
    assert_eq!(out["timepicker"]["nowDelay"], "1m");
    let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    // Byte-stable: exporting the export gives the same bytes.
    assert_eq!(doc(&out).to_export_value().to_string(), out.to_string());

    // A document without schemaVersion defaults to the latest.
    assert_eq!(doc(&json!({"title": "x"})).schema_version, 42);

    let exported = CanonicalJson::export_dashboard_json(&json!({"b": 1, "id": 3, "a": 2}));
    assert_eq!(exported.to_string(), r#"{"a":2,"b":1,"schemaVersion":42}"#);
    assert_eq!(
        CanonicalJson::export_dashboard_json(&json!([1])),
        json!([1])
    );
}
