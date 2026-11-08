#![forbid(unsafe_code)]
//! `math` expressions against `GF pkg/expr/mathexp` (`parse/lex.go`, `parse/parse.go`,
//! `exp.go`, `funcs.go`, `union`): the lexer and grammar, operators with Go's NaN and null
//! rules, label unions and the frame conversions.

use daiya_expr::{ExprValue, MathExpr, eval_math_on_vars};
use daiya_model::{FieldColumn, FieldType, Frame, FrameMeta, Notice, NoticeSeverity};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

type Vars = HashMap<String, Vec<ExprValue>>;

fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn number(l: &[(&str, &str)], v: Option<f64>) -> ExprValue {
    ExprValue::Number {
        name: "in".into(),
        labels: labels(l),
        value: v,
        notices: Vec::new(),
    }
}

fn series(l: &[(&str, &str)], points: &[(i64, Option<f64>)]) -> ExprValue {
    ExprValue::Series {
        name: "in".into(),
        labels: labels(l),
        points: points.to_vec(),
        notices: Vec::new(),
    }
}

fn eval(expr: &str, vars: &Vars) -> Vec<ExprValue> {
    eval_math_on_vars("E", expr, vars, 100).unwrap()
}

fn scalar(expr: &str) -> Option<f64> {
    match eval(expr, &Vars::new()).as_slice() {
        [ExprValue::Scalar(v)] => *v,
        other => panic!("{expr}: expected one scalar, got {other:?}"),
    }
}

fn parse_err(expr: &str) -> String {
    MathExpr::parse(expr).unwrap_err().to_string()
}

fn value(v: &ExprValue) -> Option<f64> {
    match v {
        ExprValue::Number { value, .. } | ExprValue::Scalar(value) => *value,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn points(v: &ExprValue) -> Vec<(i64, Option<f64>)> {
    match v {
        ExprValue::Series { points, .. } => points.clone(),
        other => panic!("expected a series, got {other:?}"),
    }
}

#[test]
fn the_lexer_reads_numbers_variables_and_operators() {
    assert_eq!(scalar("0x1F"), Some(31.0));
    assert_eq!(scalar("0X0a + 1"), Some(11.0));
    assert_eq!(scalar("1e3"), Some(1000.0));
    assert_eq!(scalar("2.5E-1"), Some(0.25));
    assert_eq!(scalar("1e+2"), Some(100.0));
    assert_eq!(scalar(".5"), Some(0.5));
    assert_eq!(scalar("2 ** 3 ** 2"), Some(64.0), "** is left-associative");
    assert_eq!(scalar("7 % 4"), Some(3.0));
    assert_eq!(
        scalar("-7 % 4"),
        Some(-3.0),
        "math.Mod keeps the dividend's sign"
    );
    assert_eq!(scalar("1 + 2 * 3"), Some(7.0));
    assert_eq!(scalar("(1 + 2) * 3"), Some(9.0));
    assert_eq!(scalar("10 / 4 - 1"), Some(1.5));
    assert_eq!(scalar("1 / 0"), Some(f64::INFINITY));
    assert_eq!(scalar("--2"), Some(2.0));

    let parsed = MathExpr::parse("  $A + ${B} * ${ my var } - $A_2 + $A ").unwrap();
    assert_eq!(parsed.raw, "$A + ${B} * ${ my var } - $A_2 + $A");
    assert_eq!(parsed.var_names, ["A", "B", "my var", "A_2"], "each once");
}

#[test]
fn the_lexer_refuses_what_grafana_refuses() {
    for (expr, msg) in [
        ("", "must not be empty"),
        ("   ", "must not be empty"),
        ("$A = 1", "use '==' for equality"),
        ("$A & 1", "unexpected '&'"),
        ("$A | 1", "unexpected '|'"),
        ("${A-B}", "unsupported variable character '-'"),
        ("${A", "unterminated variable"),
        ("${ }", "incomplete variable"),
        ("$", "incomplete variable"),
        ("$ + 1", "incomplete variable"),
        ("0x", "bad hex number: 0x"),
        ("1.2.3", "unexpected trailing token Some(Number(0.3))"),
        ("1e", "illegal number syntax: 1e"),
        ("$A # 1", "invalid character in math expression: '#'"),
    ] {
        let err = parse_err(expr);
        assert!(err.contains(msg), "{expr}: {err}");
    }
}

#[test]
fn the_parser_refuses_bad_grammar_and_unknown_functions() {
    for (expr, msg) in [
        ("$A +", "unexpected token in math expression: None"),
        ("($A + 1", "expected ')', got None"),
        ("($A + 1 2", "expected ')', got Some(Number(2.0))"),
        ("$A $B", "unexpected trailing token Some(Var(\"B\"))"),
        (")", "unexpected token in math expression: Some(RParen)"),
        ("sum($A)", "non existent function sum"),
        ("abs $A", "expected '(' after function abs"),
        ("abs($A", "expected ')' closing function abs, got None"),
        ("abs($A, $B)", "function abs expects 1 argument(s), got 2"),
        ("abs()", "function abs expects 1 argument(s), got 0"),
        ("nan(1)", "function nan expects 0 argument(s), got 1"),
    ] {
        let err = parse_err(expr);
        assert!(err.contains(msg), "{expr}: {err}");
    }
    // Each function call, `!`, `-` and `(` counts one level of the depth cap.
    for expr in ["abs(abs(abs($A)))", "!!!$A", "---$A", "((($A)))"] {
        assert!(MathExpr::parse_with_max_depth(expr, 3).is_ok(), "{expr}");
        let err = MathExpr::parse_with_max_depth(expr, 2)
            .unwrap_err()
            .to_string();
        assert!(err.contains("maximum nesting depth of 2"), "{expr}: {err}");
    }
}

#[test]
fn comparison_and_logic_operators_answer_one_or_zero() {
    for (expr, want) in [
        ("2 == 2", 1.0),
        ("2 == 3", 0.0),
        ("2 != 3", 1.0),
        ("2 != 2", 0.0),
        ("3 > 2", 1.0),
        ("2 > 2", 0.0),
        ("2 >= 2", 1.0),
        ("1 >= 2", 0.0),
        ("1 < 2", 1.0),
        ("2 < 2", 0.0),
        ("2 <= 2", 1.0),
        ("3 <= 2", 0.0),
        ("1 && 2", 1.0),
        ("1 && 0", 0.0),
        ("0 || 0", 0.0),
        ("0 || 3", 1.0),
        ("1 < 2 && 3 > 2 || 0", 1.0),
        ("!0", 1.0),
        ("!5", 0.0),
        ("!(1 == 2)", 1.0),
    ] {
        assert_eq!(scalar(expr), Some(want), "{expr}");
    }
}

#[test]
fn nan_follows_go_comparisons_and_short_circuits() {
    // exp.go binaryOp tests || and && before NaN: `a != 0` holds for NaN in Go.
    assert_eq!(scalar("0 && nan()"), Some(0.0));
    assert_eq!(scalar("1 || nan()"), Some(1.0));
    assert_eq!(scalar("nan() || 0"), Some(1.0));
    assert!(scalar("nan() && 1").unwrap().is_nan());
    assert!(scalar("0 || nan()").unwrap().is_nan());
    assert!(scalar("nan() + 1").unwrap().is_nan());
    assert!(scalar("1 == nan()").unwrap().is_nan());
    assert!(scalar("-nan()").unwrap().is_nan());
    assert!(scalar("!nan()").unwrap().is_nan());
    assert_eq!(scalar("inf()"), Some(f64::INFINITY));
    assert_eq!(scalar("infn()"), Some(f64::NEG_INFINITY));
    // null stays null through operators.
    assert_eq!(scalar("null()"), None);
    assert_eq!(scalar("null() + 1"), None);
    assert_eq!(scalar("1 * null()"), None);
    assert_eq!(scalar("-null()"), None);
}

#[test]
fn functions_follow_grafana_per_float_rules() {
    assert_eq!(scalar("abs(-3)"), Some(3.0));
    assert_eq!(scalar("log(1)"), Some(0.0));
    assert!(scalar("log(-1)").unwrap().is_nan());
    assert_eq!(
        scalar("round(2.5)"),
        Some(3.0),
        "Go math.Round: half away from zero"
    );
    assert_eq!(scalar("round(-2.5)"), Some(-3.0));
    assert_eq!(scalar("ceil(1.1)"), Some(2.0));
    assert_eq!(scalar("floor(-1.1)"), Some(-2.0));
    assert_eq!(scalar("is_nan(nan())"), Some(1.0));
    assert_eq!(scalar("is_nan(1)"), Some(0.0));
    assert_eq!(scalar("is_inf(infn())"), Some(1.0));
    assert_eq!(scalar("is_inf(1)"), Some(0.0));
    assert_eq!(scalar("is_null(null())"), Some(1.0));
    assert_eq!(scalar("is_null(0)"), Some(0.0));
    assert_eq!(scalar("is_number(1)"), Some(1.0));
    assert_eq!(scalar("is_number(inf())"), Some(0.0));
    assert_eq!(scalar("is_number(null())"), Some(0.0));
    // perFloat turns null into NaN before the function.
    assert!(scalar("abs(null())").unwrap().is_nan());
    assert!(scalar("is_nan(null())").unwrap().is_nan());

    let vars = Vars::from([
        (
            "A".to_owned(),
            vec![
                series(&[("h", "a")], &[(1, Some(-1.5)), (2, None)]),
                number(&[("h", "b")], Some(-2.0)),
                number(&[("h", "c")], None),
            ],
        ),
        ("N".to_owned(), vec![ExprValue::NoData]),
    ]);
    let out = eval("abs($A)", &vars);
    let p = points(&out[0]);
    assert_eq!(p[0], (1, Some(1.5)));
    assert!(p[1].1.unwrap().is_nan());
    assert_eq!(value(&out[1]), Some(2.0));
    assert!(value(&out[2]).unwrap().is_nan());
    for v in &out {
        match v {
            ExprValue::Number { name, .. } | ExprValue::Series { name, .. } => {
                assert_eq!(name, "E");
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(out[1].labels(), &labels(&[("h", "b")]));

    let out = eval("is_null($A)", &vars);
    assert_eq!(points(&out[0]), [(1, Some(0.0)), (2, Some(1.0))]);
    assert_eq!(value(&out[1]), Some(0.0));
    assert_eq!(value(&out[2]), Some(1.0));
    let out = eval("is_number($A)", &vars);
    assert_eq!(points(&out[0]), [(1, Some(1.0)), (2, Some(0.0))]);

    for f in ["abs", "is_null", "is_number"] {
        assert_eq!(eval(&format!("{f}($N)"), &vars), [ExprValue::NoData], "{f}");
    }
}

#[test]
fn unary_operators_keep_labels_and_nulls() {
    let vars = Vars::from([(
        "A".to_owned(),
        vec![
            series(&[("h", "a")], &[(1, Some(2.0)), (2, None), (3, Some(0.0))]),
            number(&[("h", "b")], Some(4.0)),
            number(&[("h", "c")], None),
            ExprValue::NoData,
            ExprValue::Scalar(None),
        ],
    )]);
    let out = eval("-$A", &vars);
    assert_eq!(
        points(&out[0]),
        [(1, Some(-2.0)), (2, None), (3, Some(-0.0))]
    );
    assert_eq!(out[0].labels(), &labels(&[("h", "a")]));
    assert_eq!(value(&out[1]), Some(-4.0));
    assert_eq!(value(&out[2]), None);
    assert_eq!(out[3], ExprValue::NoData);
    assert_eq!(out[4], ExprValue::Scalar(None));
    let out = eval("!$A", &vars);
    assert_eq!(points(&out[0]), [(1, Some(0.0)), (2, None), (3, Some(1.0))]);
}

#[test]
fn binary_operators_cover_every_value_kind_pair() {
    let vars = Vars::from([
        (
            "S".to_owned(),
            vec![series(
                &[("h", "a")],
                &[(1, Some(1.0)), (2, None), (3, Some(3.0))],
            )],
        ),
        (
            "T".to_owned(),
            vec![series(
                &[("h", "a")],
                &[(2, Some(5.0)), (3, Some(10.0)), (4, Some(1.0))],
            )],
        ),
        ("N".to_owned(), vec![number(&[("h", "a")], Some(2.0))]),
        ("M".to_owned(), vec![number(&[("h", "a")], None)]),
    ]);
    // Series with scalar or number, both orders.
    assert_eq!(
        points(&eval("$S * 2", &vars)[0]),
        [(1, Some(2.0)), (2, None), (3, Some(6.0))]
    );
    assert_eq!(
        points(&eval("$S + $N", &vars)[0]),
        [(1, Some(3.0)), (2, None), (3, Some(5.0))]
    );
    assert_eq!(
        points(&eval("10 - $S", &vars)[0]),
        [(1, Some(9.0)), (2, None), (3, Some(7.0))]
    );
    assert_eq!(
        points(&eval("$N - $S", &vars)[0]),
        [(1, Some(1.0)), (2, None), (3, Some(-1.0))]
    );
    // Series with series: only the shared times, null where either side is null.
    let st = eval("$S + $T", &vars);
    assert_eq!(points(&st[0]), [(2, None), (3, Some(13.0))]);
    match &st[0] {
        ExprValue::Series {
            name, labels: l, ..
        } => {
            assert_eq!(name, "E");
            assert_eq!(l, &labels(&[("h", "a")]));
        }
        other => panic!("{other:?}"),
    }
    // Number with scalar or number.
    assert_eq!(value(&eval("$N * 3", &vars)[0]), Some(6.0));
    assert_eq!(value(&eval("3 - $N", &vars)[0]), Some(1.0));
    assert_eq!(value(&eval("$N ** $N", &vars)[0]), Some(4.0));
    assert_eq!(value(&eval("$M + 1", &vars)[0]), None);
    assert_eq!(value(&eval("1 + $M", &vars)[0]), None);
    assert_eq!(value(&eval("$N + $M", &vars)[0]), None);
    assert!(matches!(eval("$N + 1", &vars)[0], ExprValue::Number { .. }));
    assert!(matches!(eval("1 + $N", &vars)[0], ExprValue::Number { .. }));
}

#[test]
fn unions_join_by_labels_like_grafana() {
    let vars = Vars::from([
        (
            "A".to_owned(),
            vec![
                number(&[("host", "a")], Some(1.0)),
                number(&[("host", "b")], Some(2.0)),
                number(&[("host", "c")], Some(3.0)),
            ],
        ),
        (
            "B".to_owned(),
            vec![
                number(&[("host", "a")], Some(10.0)),
                number(&[("host", "b")], Some(20.0)),
            ],
        ),
        (
            "Sub".to_owned(),
            vec![number(&[("host", "a"), ("dc", "x")], Some(100.0))],
        ),
        ("One".to_owned(), vec![number(&[("dc", "y")], Some(7.0))]),
        (
            "Other".to_owned(),
            vec![number(&[("zone", "z")], Some(8.0))],
        ),
        (
            "Two".to_owned(),
            vec![number(&[("dc", "y"), ("r", "1")], Some(9.0))],
        ),
        ("Empty".to_owned(), vec![]),
        ("Nd".to_owned(), vec![ExprValue::NoData]),
        ("Plain".to_owned(), vec![number(&[], Some(5.0))]),
    ]);

    // Equal labels pair up; host=c has no partner and is dropped with a notice.
    let out = eval("$A + $B", &vars);
    assert_eq!(out.len(), 2);
    assert_eq!(value(&out[0]), Some(11.0));
    assert_eq!(value(&out[1]), Some(22.0));
    assert_eq!(out[0].labels(), &labels(&[("host", "a")]));
    let ExprValue::Number { notices, .. } = &out[0] else {
        panic!("number");
    };
    assert_eq!(
        notices,
        &[Notice {
            severity: NoticeSeverity::Warning,
            text: "1 items dropped from union(s)".into()
        }]
    );

    // A label subset joins with its superset and the result carries the superset.
    for expr in ["$A + $Sub", "$Sub + $A"] {
        let out = eval(expr, &vars);
        assert_eq!(out.len(), 1, "{expr}");
        assert_eq!(value(&out[0]), Some(101.0));
        assert_eq!(out[0].labels(), &labels(&[("host", "a"), ("dc", "x")]));
    }

    // One value on each side with unrelated labels: combined, labels stripped.
    let out = eval("$One + $Other", &vars);
    assert_eq!(value(&out[0]), Some(15.0));
    assert!(out[0].labels().is_empty());

    // Different label counts that do not contain each other do not join.
    let out = eval("$Other + $Two", &vars);
    assert_eq!(value(&out[0]), Some(17.0), "1x1 still combines");
    let out = eval("$B + $Two", &vars);
    assert!(out.is_empty());

    // Values without labels join with everything and take the other side's labels.
    let out = eval("$Plain * $A", &vars);
    assert_eq!(out.len(), 3);
    assert_eq!(out[2].labels(), &labels(&[("host", "c")]));
    assert_eq!(value(&out[2]), Some(15.0));

    // An empty side yields nothing; no data on either side yields no data.
    assert!(eval("$Empty + 1", &vars).is_empty());
    assert!(eval("1 + $Empty", &vars).is_empty());
    assert_eq!(eval("$Nd + $A", &vars), [ExprValue::NoData]);
    assert_eq!(eval("$A + $Nd", &vars), [ExprValue::NoData]);
    assert_eq!(eval("$Unknown * 2", &vars), [ExprValue::NoData]);
}

#[test]
fn frames_convert_to_values_like_the_grafana_converter() {
    assert_eq!(
        ExprValue::from_frame(&Frame::default()),
        [ExprValue::NoData]
    );

    // A wide time series: one series per number column, named after the frame or the field.
    let wide = Frame {
        name: String::new(),
        ref_id: Some("A".into()),
        fields: vec![
            FieldColumn {
                values: vec![json!("1970-01-01T00:00:01Z"), json!(2000), json!("bad")],
                ..FieldColumn::time("Time", vec![])
            },
            FieldColumn::numbers(
                "cpu",
                labels(&[("h", "a")]),
                vec![Some(1.0), None, Some(3.0)],
            ),
            FieldColumn {
                display_name: Some("Memory".into()),
                ..FieldColumn::numbers("mem", BTreeMap::new(), vec![Some(4.0), Some(5.0)])
            },
            FieldColumn::strings("note", vec![Some("x".into())]),
        ],
        meta: FrameMeta {
            notices: vec![Notice {
                severity: NoticeSeverity::Info,
                text: "from source".into(),
            }],
            ..FrameMeta::default()
        },
    };
    let vals = ExprValue::from_frame(&wide);
    assert_eq!(
        vals[0],
        ExprValue::Series {
            name: "cpu".into(),
            labels: labels(&[("h", "a")]),
            points: vec![(1000, Some(1.0)), (2000, None), (0, Some(3.0))],
            notices: wide.meta.notices.clone(),
        }
    );
    assert_eq!(points(&vals[1]), [(1000, Some(4.0)), (2000, Some(5.0))]);
    let ExprValue::Series { name, .. } = &vals[1] else {
        panic!("series");
    };
    assert_eq!(name, "Memory");

    let named = Frame {
        name: "frame".into(),
        ..wide.clone()
    };
    let ExprValue::Series { name, .. } = &ExprValue::from_frame(&named)[0] else {
        panic!("series");
    };
    assert_eq!(name, "frame");

    // A number table: one number per row, string columns become labels.
    let table = Frame {
        name: "t".into(),
        ref_id: None,
        fields: vec![
            FieldColumn::strings("host", vec![Some("a".into()), None]),
            FieldColumn::numbers("v", labels(&[("k", "1")]), vec![Some(1.0), Some(2.0)]),
            FieldColumn::numbers("w", BTreeMap::new(), vec![Some(9.0), Some(9.0)]),
        ],
        meta: FrameMeta::default(),
    };
    assert_eq!(
        ExprValue::from_frame(&table),
        [
            ExprValue::Number {
                name: "v".into(),
                labels: labels(&[("k", "1"), ("host", "a")]),
                value: Some(1.0),
                notices: vec![],
            },
            ExprValue::Number {
                name: "v".into(),
                labels: labels(&[("k", "1")]),
                value: Some(2.0),
                notices: vec![],
            },
        ]
    );

    let empty_numbers = Frame {
        fields: vec![FieldColumn::numbers("v", BTreeMap::new(), vec![])],
        ..Frame::default()
    };
    assert_eq!(ExprValue::from_frame(&empty_numbers), [ExprValue::NoData]);
    let only_strings = Frame {
        fields: vec![FieldColumn::strings("s", vec![Some("a".into())])],
        ..Frame::default()
    };
    assert_eq!(ExprValue::from_frame(&only_strings), [ExprValue::NoData]);
    let time_only = Frame {
        fields: vec![FieldColumn::time("Time", vec![1])],
        ..Frame::default()
    };
    assert_eq!(ExprValue::from_frame(&time_only), [ExprValue::NoData]);
}

#[test]
fn values_convert_back_to_frames() {
    assert!(ExprValue::Scalar(None).labels().is_empty());
    assert!(ExprValue::NoData.labels().is_empty());

    let f = ExprValue::Scalar(Some(2.0)).to_frame("B");
    assert_eq!(f.name, "B");
    assert_eq!(f.ref_id.as_deref(), Some("B"));
    assert_eq!(f.fields[0].name, "B");
    assert_eq!(f.fields[0].f64_at(0), Some(2.0));

    let notice = Notice {
        severity: NoticeSeverity::Warning,
        text: "w".into(),
    };
    let n = ExprValue::Number {
        name: String::new(),
        labels: labels(&[("h", "a")]),
        value: None,
        notices: vec![notice.clone()],
    }
    .to_frame("C");
    assert_eq!(n.name, "C", "an unnamed number takes the refId");
    assert_eq!(n.fields[0].labels, labels(&[("h", "a")]));
    assert_eq!(n.fields[0].values, [Value::Null]);
    assert_eq!(n.meta.notices, std::slice::from_ref(&notice));
    let n = number(&[], Some(1.0)).to_frame("C");
    assert_eq!(n.name, "in");

    let s = ExprValue::Series {
        name: String::new(),
        labels: BTreeMap::new(),
        points: vec![(1, Some(1.0)), (2, None)],
        notices: vec![notice],
    }
    .to_frame("D");
    assert_eq!(s.name, "D");
    assert_eq!(s.fields[0].field_type, FieldType::Time);
    assert_eq!(s.fields[0].values, [json!(1), json!(2)]);
    assert_eq!(s.fields[1].name, "D");
    assert_eq!(s.fields[1].values, [json!(1.0), Value::Null]);
    assert_eq!(series(&[], &[]).to_frame("D").name, "in");

    let nd = ExprValue::NoData.to_frame("E");
    assert_eq!(nd.name, "no data");
    assert!(nd.fields.is_empty());
}
