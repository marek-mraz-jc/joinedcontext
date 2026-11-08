#![forbid(unsafe_code)]
//! The `__expr__` pipeline (R4, `GF pkg/expr/graph.go`, `nodes.go`): target parsing, the
//! dependency checks Grafana refuses, topological order and what each command answers.

use daiya_expr::pipeline::ExprCommand;
use daiya_expr::{
    ExprNodeSpec, ReduceMode, ReducerId, Upsampler, evaluate_pipeline,
    is_expression_datasource_uid, parse_duration_ms,
};
use daiya_model::{FieldColumn, Frame, FrameMeta, FrameSet, NoticeSeverity};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn node(target: Value) -> ExprNodeSpec {
    ExprNodeSpec::from_json(&target, 100).unwrap()
}

fn parse_err(target: Value) -> String {
    ExprNodeSpec::from_json(&target, 100)
        .unwrap_err()
        .to_string()
}

fn series_set(ref_id: &str, host: &str, vals: &[Option<f64>]) -> FrameSet {
    let times = (0..vals.len()).map(|i| (i as i64 + 1) * 1000).collect();
    FrameSet::new(
        ref_id,
        vec![Frame {
            name: String::new(),
            ref_id: Some(ref_id.into()),
            fields: vec![
                FieldColumn::time("Time", times),
                FieldColumn::numbers(
                    "Value",
                    BTreeMap::from([("host".to_owned(), host.to_owned())]),
                    vals.to_vec(),
                ),
            ],
            meta: FrameMeta::default(),
        }],
    )
}

fn run(ds: &[FrameSet], nodes: &[ExprNodeSpec]) -> Result<Vec<FrameSet>, String> {
    evaluate_pipeline(ds, &BTreeSet::new(), nodes, 1000, 4000, 100)
        .map(|o| o.frame_sets)
        .map_err(|e| e.to_string())
}

fn set<'a>(sets: &'a [FrameSet], ref_id: &str) -> &'a FrameSet {
    sets.iter()
        .find(|s| s.ref_id == ref_id)
        .unwrap_or_else(|| panic!("no frame set {ref_id}"))
}

fn last_value(fs: &FrameSet) -> Option<f64> {
    let f = fs.frames[0].fields.last().unwrap();
    f.f64_at(f.len() - 1)
}

#[test]
fn expression_datasource_uids_and_durations() {
    assert!(is_expression_datasource_uid("__expr__"));
    assert!(is_expression_datasource_uid("-100"));
    assert!(!is_expression_datasource_uid("prom"));

    for (raw, ms) in [
        ("500ms", 500),
        (" 10s ", 10_000),
        ("5m", 300_000),
        ("1h", 3_600_000),
        ("1d", 86_400_000),
        ("2w", 1_209_600_000),
        ("1.5s", 1_500),
    ] {
        assert_eq!(parse_duration_ms(raw).unwrap(), ms, "{raw}");
    }
    for (raw, msg) in [
        ("", "empty duration"),
        ("xms", "invalid duration 'xms'"),
        ("10y", "unsupported duration unit"),
        ("abcs", "invalid duration 'abcs'"),
    ] {
        assert!(
            parse_duration_ms(raw)
                .unwrap_err()
                .to_string()
                .contains(msg),
            "{raw}"
        );
    }
}

#[test]
fn every_command_type_parses_with_grafana_defaults() {
    let math = node(json!({ "type": "math", "expression": "$A + 1" }));
    assert_eq!(math.ref_id, "A", "refId defaults to A");
    assert!(!math.hidden);
    assert_eq!(math.command.kind_name(), "math");
    assert_eq!(math.command.needs_vars(), ["A"]);

    let reduce = node(json!({
        "refId": "R", "hide": true, "type": "reduce", "expression": "$A", "reducer": "mean"
    }));
    assert!(reduce.hidden);
    assert_eq!(reduce.command.kind_name(), "reduce");
    assert_eq!(reduce.command.needs_vars(), ["A"]);
    assert!(matches!(
        reduce.command,
        ExprCommand::Reduce {
            reducer: ReducerId::Mean,
            mode: ReduceMode::Strict,
            ..
        }
    ));
    for (settings, mode) in [
        (json!({}), ReduceMode::Strict),
        (json!({ "mode": "" }), ReduceMode::Strict),
        (json!({ "mode": "dropNN" }), ReduceMode::DropNonNumber),
        (
            json!({ "mode": "replaceNN", "replaceWithValue": -1 }),
            ReduceMode::ReplaceNonNumber(-1.0),
        ),
    ] {
        let n = node(json!({
            "refId": "R", "type": "reduce", "expression": "A", "reducer": "max", "settings": settings
        }));
        let ExprCommand::Reduce {
            mode: got, reducer, ..
        } = n.command
        else {
            panic!("reduce");
        };
        assert_eq!(got, mode);
        assert_eq!(reducer, ReducerId::Max);
    }

    let resample = node(json!({
        "refId": "S", "type": "resample", "expression": "$A", "window": "10s",
        "downsampler": "last", "upsampler": "pad"
    }));
    assert_eq!(resample.command.kind_name(), "resample");
    assert_eq!(resample.command.needs_vars(), ["A"]);
    assert!(matches!(
        resample.command,
        ExprCommand::Resample {
            window_ms: 10_000,
            downsampler: ReducerId::Last,
            upsampler: Upsampler::Pad,
            ..
        }
    ));

    let threshold = node(json!({
        "refId": "T", "type": "threshold", "expression": "$B",
        "conditions": [{
            "evaluator": { "type": "gt", "params": [5, "x"] },
            "unloadEvaluator": { "type": "lt", "params": [3] }
        }]
    }));
    assert_eq!(threshold.command.kind_name(), "threshold");
    assert_eq!(threshold.command.needs_vars(), ["B"]);
    let ExprCommand::Threshold { condition, .. } = &threshold.command else {
        panic!("threshold");
    };
    assert_eq!(condition.params, [5.0], "non-numbers are not parameters");
    assert_eq!(condition.unload_evaluator.as_ref().unwrap().1, [3.0]);

    let classic = node(json!({
        "refId": "C", "type": "classic_conditions",
        "conditions": [
            { "query": { "params": ["A"] }, "evaluator": { "params": [1] } },
            {
                "operator": { "type": "or" }, "query": { "params": ["B"] },
                "reducer": { "type": "max" }, "evaluator": { "type": "lt", "params": [2] }
            }
        ]
    }));
    assert_eq!(classic.command.kind_name(), "classic_conditions");
    assert_eq!(classic.command.needs_vars(), ["A", "B"]);
    let ExprCommand::Classic { conditions } = &classic.command else {
        panic!("classic");
    };
    assert_eq!(conditions[0].reducer, "avg", "Grafana's default reducer");
    assert_eq!(conditions[0].evaluator_type, "gt");
    assert_eq!(conditions[0].operator, "and");
    assert_eq!(conditions[1].operator, "or");
    assert_eq!(conditions[1].reducer, "max");

    let sql = node(json!({ "refId": "Q", "type": "sql", "expression": "SELECT 1" }));
    assert_eq!(sql.command.kind_name(), "sql");
    assert!(sql.command.needs_vars().is_empty());
    let sql = node(json!({ "refId": "Q", "type": "sql" }));
    assert!(matches!(sql.command, ExprCommand::SqlWarning { raw_sql } if raw_sql.is_empty()));
}

#[test]
fn malformed_targets_are_refused_with_grafana_messages() {
    let cases = [
        (json!([1]), "must be an object"),
        (
            json!({ "refId": "X" }),
            "no expression command type in query 'X'",
        ),
        (
            json!({ "type": "nope" }),
            "'nope' is not a recognized expression type",
        ),
        (
            json!({ "type": "math" }),
            "math command 'A' is missing 'expression'",
        ),
        (
            json!({ "type": "math", "expression": "$A +" }),
            "unexpected token",
        ),
        (
            json!({ "type": "reduce" }),
            "reduce command 'A' is missing 'expression'",
        ),
        (
            json!({ "type": "reduce", "expression": "B" }),
            "reduce command 'A' is missing 'reducer'",
        ),
        (
            json!({ "type": "reduce", "expression": "B", "reducer": "stddev" }),
            "reduction 'stddev' not implemented",
        ),
        (
            json!({ "type": "reduce", "expression": "B", "reducer": "sum",
                    "settings": { "mode": "replaceNN" } }),
            "replaceWithValue must be specified",
        ),
        (
            json!({ "type": "reduce", "expression": "B", "reducer": "sum",
                    "settings": { "mode": "keep" } }),
            "reducer mode 'keep' is not supported",
        ),
        (
            json!({ "type": "resample" }),
            "resample command 'A' is missing 'expression'",
        ),
        (
            json!({ "type": "resample", "expression": "B" }),
            "missing 'window'",
        ),
        (
            json!({ "type": "resample", "expression": "B", "window": "1q" }),
            "unsupported duration unit",
        ),
        (
            json!({ "type": "resample", "expression": "B", "window": "1s" }),
            "missing 'downsampler'",
        ),
        (
            json!({ "type": "resample", "expression": "B", "window": "1s", "downsampler": "sum" }),
            "missing 'upsampler'",
        ),
        (
            json!({ "type": "resample", "expression": "B", "window": "1s",
                    "downsampler": "nope", "upsampler": "pad" }),
            "reduction 'nope' not implemented",
        ),
        (
            json!({ "type": "resample", "expression": "B", "window": "1s",
                    "downsampler": "sum", "upsampler": "nope" }),
            "nope",
        ),
        (
            json!({ "type": "threshold" }),
            "threshold command 'A' is missing 'expression'",
        ),
        (
            json!({ "type": "threshold", "expression": "B" }),
            "requires 'conditions' array",
        ),
        (
            json!({ "type": "threshold", "expression": "B", "conditions": [] }),
            "exactly one condition",
        ),
        (
            json!({ "type": "threshold", "expression": "B", "conditions": [{}] }),
            "missing 'evaluator'",
        ),
        (
            json!({ "type": "threshold", "expression": "B",
                    "conditions": [{ "evaluator": { "type": "big" } }] }),
            "big",
        ),
        (
            json!({ "type": "threshold", "expression": "B",
                    "conditions": [{ "evaluator": { "type": "gt", "params": [1] },
                                     "unloadEvaluator": { "type": "small" } }] }),
            "small",
        ),
        (
            json!({ "type": "classic_conditions" }),
            "classic_conditions missing 'conditions' array",
        ),
        (
            json!({ "type": "classic_conditions", "conditions": [
                { "query": { "params": ["A"] }, "evaluator": {} },
                { "operator": { "type": "xor" }, "query": { "params": ["A"] }, "evaluator": {} }
            ] }),
            "condition 2 operator must be `and`, `or` or `logic-or`",
        ),
        (
            json!({ "type": "classic_conditions", "conditions": [
                { "query": { "params": [""] }, "evaluator": {} }
            ] }),
            "condition 1 is missing the query RefID argument",
        ),
        (
            json!({ "type": "classic_conditions", "conditions": [
                { "query": { "params": ["A"] } }
            ] }),
            "classic condition missing evaluator",
        ),
    ];
    for (target, msg) in cases {
        let err = parse_err(target.clone());
        assert!(err.contains(msg), "{target}: {err}");
    }
    // The depth cap applies to math parsing.
    let err = ExprNodeSpec::from_json(&json!({ "type": "math", "expression": "-(-(-$A))" }), 2)
        .unwrap_err()
        .to_string();
    assert!(err.contains("maximum nesting depth of 2"), "{err}");
}

#[test]
fn nodes_run_in_dependency_order_whatever_their_declared_order() {
    let ds = [series_set("A", "a", &[Some(1.0), Some(3.0), Some(5.0)])];
    // C needs B, B needs A: declared C first.
    let nodes = [
        node(json!({ "refId": "C", "type": "math", "expression": "$B * 10" })),
        node(json!({ "refId": "B", "type": "reduce", "expression": "$A", "reducer": "mean" })),
    ];
    let out = run(&ds, &nodes).unwrap();
    let refs: Vec<&str> = out.iter().map(|s| s.ref_id.as_str()).collect();
    assert_eq!(
        refs,
        ["A", "C", "B"],
        "datasources first, then declaration order"
    );
    assert_eq!(last_value(set(&out, "B")), Some(3.0));
    assert_eq!(last_value(set(&out, "C")), Some(30.0));
    // A reduce keeps the series labels on its number.
    assert_eq!(
        set(&out, "B").frames[0].fields[0]
            .labels
            .get("host")
            .map(String::as_str),
        Some("a")
    );
}

#[test]
fn resample_threshold_and_classic_nodes_answer_frames() {
    let ds = [
        series_set("A", "a", &[Some(1.0), Some(9.0), Some(4.0)]),
        series_set("B", "b", &[Some(2.0), Some(2.0), Some(2.0)]),
    ];
    let nodes = [
        node(json!({
            "refId": "S", "type": "resample", "expression": "A", "window": "1s",
            "downsampler": "max", "upsampler": "pad"
        })),
        node(json!({
            "refId": "M", "type": "reduce", "expression": "A", "reducer": "max"
        })),
        node(json!({
            "refId": "T", "type": "threshold", "expression": "M",
            "conditions": [{ "evaluator": { "type": "gt", "params": [5] } }]
        })),
        node(json!({
            "refId": "C", "type": "classic_conditions",
            "conditions": [
                { "query": { "params": ["A"] }, "reducer": { "type": "max" },
                  "evaluator": { "type": "gt", "params": [100] } },
                { "operator": { "type": "or" }, "query": { "params": ["B"] },
                  "reducer": { "type": "last" }, "evaluator": { "type": "lt", "params": [3] } }
            ]
        })),
    ];
    let out = run(&ds, &nodes).unwrap();
    let s = &set(&out, "S").frames[0];
    assert_eq!(
        s.fields[0].values,
        [json!(1000), json!(2000), json!(3000), json!(4000)]
    );
    assert_eq!(s.fields[1].f64_at(1), Some(9.0));
    assert_eq!(
        s.fields[1].f64_at(3),
        Some(4.0),
        "pad repeats the last value"
    );
    assert_eq!(last_value(set(&out, "T")), Some(1.0), "max 9 > 5");
    assert_eq!(last_value(set(&out, "C")), Some(1.0), "false OR true");
}

#[test]
fn sql_imports_with_a_named_warning_and_hidden_nodes_and_queries_are_dropped() {
    let ds = [
        series_set("A", "a", &[Some(1.0)]),
        series_set("H", "h", &[Some(2.0)]),
    ];
    let nodes = [
        node(json!({ "refId": "Q", "type": "sql", "expression": "SELECT * FROM A" })),
        node(json!({ "refId": "X", "hide": true, "type": "math", "expression": "$A + $H" })),
    ];
    let hidden = BTreeSet::from(["H".to_owned()]);
    let out = evaluate_pipeline(&ds, &hidden, &nodes, 0, 0, 10).unwrap();
    let refs: Vec<&str> = out.frame_sets.iter().map(|s| s.ref_id.as_str()).collect();
    assert_eq!(refs, ["A", "Q"]);
    assert_eq!(
        out.warnings,
        ["sql expression 'Q' is not enabled in this run (query: SELECT * FROM A)"]
    );
    let q = &out.frame_sets[1].frames[0];
    assert!(q.fields.is_empty());
    assert_eq!(q.meta.notices[0].severity, NoticeSeverity::Warning);
    assert_eq!(q.meta.notices[0].text, out.warnings[0]);
}

#[test]
fn an_empty_datasource_result_is_no_data() {
    let ds = [FrameSet::new("A", vec![])];
    let nodes = [node(
        json!({ "refId": "B", "type": "math", "expression": "$A * 2" }),
    )];
    let out = run(&ds, &nodes).unwrap();
    let b = &set(&out, "B").frames[0];
    assert_eq!(b.name, "no data");
    assert!(b.fields.is_empty());
}

#[test]
fn graph_errors_match_grafana() {
    let ds = [series_set("A", "a", &[Some(1.0)])];
    let math = |r: &str, e: &str| node(json!({ "refId": r, "type": "math", "expression": e }));
    let classic = |r: &str, input: &str| {
        node(json!({
            "refId": r, "type": "classic_conditions",
            "conditions": [{ "query": { "params": [input] }, "evaluator": { "params": [0] } }]
        }))
    };
    let sql = |r: &str| node(json!({ "refId": r, "type": "sql", "expression": "SELECT 1" }));

    let cases: Vec<(Vec<ExprNodeSpec>, &str)> = vec![
        (
            vec![math("B", "$A"), math("B", "$A")],
            "multiple queries using the same RefId 'B' is not allowed",
        ),
        (
            vec![math("A", "1")],
            "multiple queries using the same RefId 'A' is not allowed",
        ),
        (
            vec![math("B", "$B + 1")],
            "expression 'B' cannot reference itself",
        ),
        (vec![math("B", "$Z")], "unable to find dependent node 'Z'"),
        (
            vec![math("B", "$A"), classic("C", "B")],
            "only data source queries may be inputs to a classic condition, B is an Expression",
        ),
        (
            vec![classic("C", "A"), math("B", "$C")],
            "classic conditions may not be the input for other expressions, but C is the input for B",
        ),
        (
            vec![sql("Q"), math("B", "$Q")],
            "sql expressions can not be the input for other expressions, but Q is the input for B",
        ),
        (
            vec![math("B", "$C"), math("C", "$B")],
            "cyclic dependency detected",
        ),
    ];
    for (nodes, msg) in cases {
        let err = run(&ds, &nodes).unwrap_err();
        assert!(err.contains(msg), "{msg}: {err}");
    }

    let too_many: Vec<ExprNodeSpec> = (0..3).map(|i| math(&format!("N{i}"), "1")).collect();
    let err = evaluate_pipeline(&ds, &BTreeSet::new(), &too_many, 0, 0, 2)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("has 3 nodes, exceeding maximum depth 2"),
        "{err}"
    );
}

#[test]
fn command_errors_surface_from_the_pipeline() {
    // Resample over an empty time range is refused by the resampler.
    let ds = [series_set("A", "a", &[Some(1.0)])];
    let nodes = [node(json!({
        "refId": "S", "type": "resample", "expression": "A", "window": "1s",
        "downsampler": "max", "upsampler": "pad"
    }))];
    assert!(evaluate_pipeline(&ds, &BTreeSet::new(), &nodes, 5000, 1000, 10).is_err());

    // A scalar threshold reference value works through math.
    let nodes = [
        node(json!({ "refId": "N", "type": "math", "expression": "7" })),
        node(json!({
            "refId": "T", "type": "threshold", "expression": "N",
            "conditions": [{ "evaluator": { "type": "lt", "params": [5] } }]
        })),
    ];
    let out = run(&ds, &nodes).unwrap();
    assert_eq!(last_value(set(&out, "N")), Some(7.0));
    assert_eq!(last_value(set(&out, "T")), Some(0.0));
}

#[test]
fn a_number_input_to_reduce_warns_that_reduce_is_not_needed() {
    let ds = [series_set("A", "a", &[Some(2.0), Some(4.0)])];
    let nodes = [
        node(json!({ "refId": "M", "type": "reduce", "expression": "A", "reducer": "sum" })),
        node(json!({ "refId": "R", "type": "reduce", "expression": "M", "reducer": "sum" })),
    ];
    let out = run(&ds, &nodes).unwrap();
    let r = &set(&out, "R").frames[0];
    assert_eq!(r.fields[0].f64_at(0), Some(6.0));
    assert_eq!(
        r.meta.notices[0].text,
        "Reduce operation is not needed. Input query or expression M is already reduced data."
    );
}
