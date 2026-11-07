// SPDX-License-Identifier: EUPL-1.2
//! `ogc-cql2`: both encodings to one tree, every conformance class of OGC 21-065r2, the bounds
//! (mapping rows F3 and F17 of docs/analysis/ogc-features-mapping.md).

use ogc_cql2::{parse, Error, Expr, Geometry, Lang, Op, MAX_DEPTH, MAX_LEN};

fn text(filter: &str) -> Expr {
    parse(Lang::Text, filter).unwrap_or_else(|e| unreachable!("{filter:?}: {e}"))
}

fn json(filter: &str) -> Expr {
    parse(Lang::Json, filter).unwrap_or_else(|e| unreachable!("{filter:?}: {e}"))
}

fn refused(lang: Lang, filter: &str) -> Error {
    parse(lang, filter)
        .err()
        .unwrap_or_else(|| unreachable!("{filter:?} was accepted"))
}

fn prop(name: &str) -> Expr {
    Expr::Property(name.into())
}

/// One `(text, json)` pair per conformance class, at least.
const SAME: &[(&str, &str)] = &[
    // basic-cql2
    ("pm10 = 5", r#"{"op":"=","args":[{"property":"pm10"},5]}"#),
    ("pm10 <> 5", r#"{"op":"<>","args":[{"property":"pm10"},5]}"#),
    (
        "pm10 <= 5.5",
        r#"{"op":"<=","args":[{"property":"pm10"},5.5]}"#,
    ),
    (
        "pm10 >= -2",
        r#"{"op":">=","args":[{"property":"pm10"},-2]}"#,
    ),
    (
        "a < 1 AND b > 2 AND c = 'x'",
        r#"{"op":"and","args":[{"op":"<","args":[{"property":"a"},1]},{"op":">","args":[{"property":"b"},2]},{"op":"=","args":[{"property":"c"},"x"]}]}"#,
    ),
    (
        "(a = 1 OR b = 2) AND NOT c = 3",
        r#"{"op":"and","args":[{"op":"or","args":[{"op":"=","args":[{"property":"a"},1]},{"op":"=","args":[{"property":"b"},2]}]},{"op":"not","args":[{"op":"=","args":[{"property":"c"},3]}]}]}"#,
    ),
    (
        "pm10 IS NULL",
        r#"{"op":"isNull","args":[{"property":"pm10"}]}"#,
    ),
    (
        "pm10 IS NOT NULL",
        r#"{"op":"not","args":[{"op":"isNull","args":[{"property":"pm10"}]}]}"#,
    ),
    (
        "open = TRUE",
        r#"{"op":"=","args":[{"property":"open"},true]}"#,
    ),
    ("TRUE", "true"),
    // advanced-comparison-operators
    (
        "name LIKE 'Sta%'",
        r#"{"op":"like","args":[{"property":"name"},"Sta%"]}"#,
    ),
    (
        "name NOT LIKE 'Sta%'",
        r#"{"op":"not","args":[{"op":"like","args":[{"property":"name"},"Sta%"]}]}"#,
    ),
    (
        "pm10 BETWEEN 10 AND 20",
        r#"{"op":"between","args":[{"property":"pm10"},10,20]}"#,
    ),
    (
        "pm10 NOT BETWEEN 10 AND 20",
        r#"{"op":"not","args":[{"op":"between","args":[{"property":"pm10"},10,20]}]}"#,
    ),
    (
        "type IN ('a', 'b')",
        r#"{"op":"in","args":[{"property":"type"},["a","b"]]}"#,
    ),
    (
        "type NOT IN ('a')",
        r#"{"op":"not","args":[{"op":"in","args":[{"property":"type"},["a"]]}]}"#,
    ),
    // case- and accent-insensitive comparison
    (
        "CASEI(name) = casei('Sásová')",
        r#"{"op":"=","args":[{"op":"casei","args":[{"property":"name"}]},{"op":"casei","args":["Sásová"]}]}"#,
    ),
    (
        "ACCENTI(name) LIKE ACCENTI('sas%')",
        r#"{"op":"like","args":[{"op":"accenti","args":[{"property":"name"}]},{"op":"accenti","args":["sas%"]}]}"#,
    ),
    // basic spatial functions (+), spatial functions
    (
        "S_INTERSECTS(location, POINT(19.1 48.7))",
        r#"{"op":"s_intersects","args":[{"property":"location"},{"type":"Point","coordinates":[19.1,48.7]}]}"#,
    ),
    (
        "S_INTERSECTS(location, BBOX(-180, -90, 180, 90))",
        r#"{"op":"s_intersects","args":[{"property":"location"},{"bbox":[-180,-90,180,90]}]}"#,
    ),
    (
        "s_within(location, POLYGON((0 0, 1 0, 1 1, 0 0)))",
        r#"{"op":"s_within","args":[{"property":"location"},{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}]}"#,
    ),
    (
        "S_CROSSES(route, LINESTRING(0 0, 1 1 5))",
        r#"{"op":"s_crosses","args":[{"property":"route"},{"type":"LineString","coordinates":[[0,0],[1,1,5]]}]}"#,
    ),
    (
        "S_TOUCHES(a, MULTIPOINT((0 0), (1 1)))",
        r#"{"op":"s_touches","args":[{"property":"a"},{"type":"MultiPoint","coordinates":[[0,0],[1,1]]}]}"#,
    ),
    (
        "S_DISJOINT(a, MULTIPOINT(0 0, 1 1))",
        r#"{"op":"s_disjoint","args":[{"property":"a"},{"type":"MultiPoint","coordinates":[[0,0],[1,1]]}]}"#,
    ),
    (
        "S_OVERLAPS(a, MULTILINESTRING((0 0, 1 1), (2 2, 3 3)))",
        r#"{"op":"s_overlaps","args":[{"property":"a"},{"type":"MultiLineString","coordinates":[[[0,0],[1,1]],[[2,2],[3,3]]]}]}"#,
    ),
    (
        "S_CONTAINS(a, MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0))))",
        r#"{"op":"s_contains","args":[{"property":"a"},{"type":"MultiPolygon","coordinates":[[[[0,0],[1,0],[1,1],[0,0]]]]}]}"#,
    ),
    (
        "S_EQUALS(a, GEOMETRYCOLLECTION(POINT(0 0), POINT Z (1 1 1)))",
        r#"{"op":"s_equals","args":[{"property":"a"},{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[0,0]},{"type":"Point","coordinates":[1,1,1]}]}]}"#,
    ),
    // temporal functions
    (
        "T_AFTER(observedAt, TIMESTAMP('2026-09-18T00:00:00Z'))",
        r#"{"op":"t_after","args":[{"property":"observedAt"},{"timestamp":"2026-09-18T00:00:00Z"}]}"#,
    ),
    (
        "T_BEFORE(day, DATE('2026-09-18'))",
        r#"{"op":"t_before","args":[{"property":"day"},{"date":"2026-09-18"}]}"#,
    ),
    (
        "T_DURING(INTERVAL(start, end), INTERVAL('2026-09-18', '..'))",
        r#"{"op":"t_during","args":[{"interval":[{"property":"start"},{"property":"end"}]},{"interval":["2026-09-18",".."]}]}"#,
    ),
    (
        "T_FINISHEDBY(a, INTERVAL('..', '2026-09-18T10:00:00.5Z'))",
        r#"{"op":"t_finishedBy","args":[{"property":"a"},{"interval":["..","2026-09-18T10:00:00.5Z"]}]}"#,
    ),
    (
        "t_metby(a, b)",
        r#"{"op":"t_metBy","args":[{"property":"a"},{"property":"b"}]}"#,
    ),
    // array functions
    (
        "A_CONTAINS(tags, ('a'))",
        r#"{"op":"a_contains","args":[{"property":"tags"},["a"]]}"#,
    ),
    (
        "A_CONTAINEDBY(tags, ('a', 'b'))",
        r#"{"op":"a_containedBy","args":[{"property":"tags"},["a","b"]]}"#,
    ),
    (
        "A_OVERLAPS(tags, ())",
        r#"{"op":"a_overlaps","args":[{"property":"tags"},[]]}"#,
    ),
    ("a = ()", r#"{"op":"=","args":[{"property":"a"},[]]}"#),
    (
        "a = (1, 'x', TRUE)",
        r#"{"op":"=","args":[{"property":"a"},[1,"x",true]]}"#,
    ),
    (
        "A_EQUALS(f(x), tags)",
        r#"{"op":"a_equals","args":[{"op":"f","args":[{"property":"x"}]},{"property":"tags"}]}"#,
    ),
    ("isnull(a)", r#"{"op":"isNull","args":[{"property":"a"}]}"#),
    (
        "S_INTERSECTS(a, BBOX(0, 0, -5, 1, 1, 5e+2))",
        r#"{"op":"s_intersects","args":[{"property":"a"},{"bbox":[0,0,-5,1,1,500]}]}"#,
    ),
    (
        "S_INTERSECTS(a, MULTIPOLYGON(((0 0, 1 0, 0 0)), ((5 5, 6 5, 5 5), (5 5, 6 5, 5 5))))",
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"MultiPolygon","coordinates":[[[[0,0],[1,0],[0,0]]],[[[5,5],[6,5],[5,5]],[[5,5],[6,5],[5,5]]]]}]}"#,
    ),
    // property-property
    (
        "pm10 > pm25",
        r#"{"op":">","args":[{"property":"pm10"},{"property":"pm25"}]}"#,
    ),
    // functions
    (
        "isWeekday(observedAt)",
        r#"{"op":"isWeekday","args":[{"property":"observedAt"}]}"#,
    ),
    (
        "pm10 > maxOf(a, 3)",
        r#"{"op":">","args":[{"property":"pm10"},{"op":"maxOf","args":[{"property":"a"},3]}]}"#,
    ),
    // arithmetic
    (
        "a + b * 2 > c ^ 2 - 1",
        r#"{"op":">","args":[{"op":"+","args":[{"property":"a"},{"op":"*","args":[{"property":"b"},2]}]},{"op":"-","args":[{"op":"^","args":[{"property":"c"},2]},1]}]}"#,
    ),
    (
        "a div 2 % 3 = -b",
        r#"{"op":"=","args":[{"op":"%","args":[{"op":"div","args":[{"property":"a"},2]},3]},{"op":"*","args":[-1,{"property":"b"}]}]}"#,
    ),
    (
        "(a + 1) / 2 = 1",
        r#"{"op":"=","args":[{"op":"/","args":[{"op":"+","args":[{"property":"a"},1]},2]},1]}"#,
    ),
];

#[test]
fn text_and_json_give_the_same_ast() {
    for (t, j) in SAME.iter().copied() {
        assert_eq!(text(t), json(j), "{t}");
        assert_eq!(text(&text(t).to_string()), text(t), "{t} -> {}", text(t));
    }
}

#[test]
fn identifiers_strings_and_numbers() {
    assert_eq!(text(r#""pm10" > 5"#), text("pm10 > 5"));
    assert_eq!(
        text(r#""a b" = 'x'"#),
        Expr::Op(Op::Eq, vec![prop("a b"), Expr::Str("x".into())])
    );
    assert_eq!(
        text(r#""and" = 1"#),
        Expr::Op(Op::Eq, vec![prop("and"), Expr::Num(1.0)])
    );
    assert_eq!(
        text(r#""a""b" = 1"#),
        Expr::Op(Op::Eq, vec![prop("a\"b"), Expr::Num(1.0)])
    );
    assert_eq!(
        text(r#""urn:ngsi-ld:x" > 5"#),
        Expr::Op(Op::Gt, vec![prop("urn:ngsi-ld:x"), Expr::Num(5.0)])
    );
    for name in ["urn:x", "dateObserved.value", "ovzdušie", "_x"] {
        assert_eq!(
            text(&format!("{name} > 5")),
            Expr::Op(Op::Gt, vec![prop(name), Expr::Num(5.0)]),
            "{name}"
        );
    }
    assert_eq!(
        text("a = 'it''s'"),
        Expr::Op(Op::Eq, vec![prop("a"), Expr::Str("it's".into())])
    );
    assert_eq!(
        text("a = 'a);(b|c'"),
        Expr::Op(Op::Eq, vec![prop("a"), Expr::Str("a);(b|c".into())])
    );
    assert_eq!(
        text("a = .5e1"),
        Expr::Op(Op::Eq, vec![prop("a"), Expr::Num(5.0)])
    );
    assert_eq!(
        text("a = 1E-2"),
        Expr::Op(Op::Eq, vec![prop("a"), Expr::Num(0.01)])
    );
    assert_eq!(text("NOT NOT a = 1").to_string(), "NOT (NOT (a = 1))");
    assert_eq!(
        text("a=1 and b=2 or c=3"),
        text("(a = 1 AND b = 2) OR c = 3"),
        "keywords in any case, AND binds tighter"
    );
}

#[test]
fn the_ported_gateway_subset_parses() {
    for filter in [
        "pm10 > 50",
        "pm10 >= 50 AND pm25 < 10",
        "pm10 > 50 OR pm25 > 20",
        "(pm10 > 50 OR pm25 > 20) AND dateObserved > '2026-09-18T00:00:00Z'",
        "name LIKE 'Stanica%'",
        "pm10 BETWEEN 10 AND 20",
        "type IN ('a', 'b', 'c')",
        "pm10 IS NULL",
        "pm10 IS NOT NULL",
        "NOT pm10 > 50",
        "NOT (pm10 BETWEEN 1 AND 2)",
        "name = 'Sásová'",
        "NOT name LIKE 'a%'",
        "S_CROSSES(location, POINT(1 2))",
        "T_EQUALS(dateObserved, TIMESTAMP('2026-09-18T00:00:00Z'))",
        "ACCENTI(name) = 'a'",
        "CASEI(name) = 'a'",
        "S_INTERSECTS(location, POLYGON((19.1 48.7,19.2 48.7,19.2 48.8,19.1 48.7)))",
        "T_DURING(dateObserved, INTERVAL('2026-09-18T00:00:00Z','2026-09-19T00:00:00Z'))",
    ] {
        let tree = text(filter);
        assert_eq!(text(&tree.to_string()), tree, "{filter}");
    }
}

#[test]
fn a_filter_is_a_boolean_expression() {
    for (lang, filter) in [
        (Lang::Text, "pm10"),
        (Lang::Text, "1 + 2"),
        (Lang::Text, "'x'"),
        (Lang::Text, "CASEI(name)"),
        (Lang::Json, r#"{"property":"pm10"}"#),
        (Lang::Json, "5"),
    ] {
        assert_eq!(
            refused(lang, filter).what,
            "a filter is a boolean expression",
            "{filter}"
        );
    }
}

#[test]
fn syntax_errors_name_the_offset() {
    for (filter, at) in [
        ("pm10 >", 6),
        ("pm10 > 5 AND", 12),
        ("pm10 != 5", 5),
        ("(pm10 > 5", 9),
        ("pm10 > 5)", 8),
        ("pm10 = 'open", 7),
        ("\"\" = 1", 0),
        ("a NOT 5", 6),
        ("a IS 5", 5),
        ("a BETWEEN 1 OR 2", 12),
        ("a IN 5", 5),
        ("a = #", 4),
        ("AND = 1", 0),
        ("a = 1e999", 4),
        ("T_AFTER(a, TIMESTAMP('2026-09-18'))", 21),
        ("T_AFTER(a, DATE('2026-13-01'))", 16),
        ("T_AFTER(a, DATE(5))", 16),
        ("T_DURING(a, INTERVAL('yesterday', '..'))", 21),
        ("T_DURING(a, INTERVAL(5, '..'))", 21),
        ("T_DURING(a, INTERVAL('..' '..'))", 26),
        ("S_INTERSECTS(a, BBOX(1, 2, 3))", 16),
        ("S_INTERSECTS(a, BBOX(1, x))", 24),
        ("S_INTERSECTS(a, POINT(1))", 23),
        ("S_INTERSECTS(a, POINT(1 2, 3 4))", 16),
        ("S_INTERSECTS(a, MULTIPOINT((1 2, 3 4)))", 27),
        ("S_INTERSECTS(a, GEOMETRYCOLLECTION(5))", 35),
        ("S_INTERSECTS(a, CIRCLE(1 2))", 25),
        ("S_INTERSECTS(a)", 0),
        ("CASEI(a, b) = 'x'", 0),
        ("f(a b) = 1", 4),
        ("A_CONTAINS(t, (1, 2) = 1", 21),
        ("A_CONTAINS(a + 1, ('x'))", 0),
        ("a = .x", 4),
        ("a = 2e", 5),
        ("S_INTERSECTS(a, GEOMETRYCOLLECTION(CIRCLE(1 2)))", 35),
    ] {
        let e = refused(Lang::Text, filter);
        assert_eq!(e.at, at, "{filter}: {e}");
        assert!(
            e.to_string().starts_with(&format!("CQL2 error at {at}: ")),
            "{e}"
        );
    }
}

#[test]
fn json_refuses_what_the_schema_does_not_define() {
    for filter in [
        "{",
        "null",
        r#"{"op":"=","args":[{"property":"a"},null]}"#,
        r#"{"op":"=","args":[{"property":"a"}]}"#,
        r#"{"op":"=","args":{"property":"a"}}"#,
        r#"{"op":"=","args":[{"property":"a"},1],"extra":1}"#,
        r#"{"op":"in","args":[{"property":"a"},"x"]}"#,
        r#"{"op":"a_contains","args":[5,["x"]]}"#,
        r#"{"op":"between","args":[{"property":"a"},1]}"#,
        r#"{"op":"=","args":[{"property":""},1]}"#,
        r#"{"op":"=","args":[{"property":5},1]}"#,
        r#"{"op":"=","args":[{"property":"a","x":1},1]}"#,
        r#"{"op":"=","args":[{"property":"a"},1e999]}"#,
        r#"{"op":"t_after","args":[{"property":"a"},{"timestamp":"2026-09-18"}]}"#,
        r#"{"op":"t_after","args":[{"property":"a"},{"date":"2026-09-18T00:00:00Z"}]}"#,
        r#"{"op":"t_during","args":[{"property":"a"},{"interval":[".."]}]}"#,
        r#"{"op":"t_during","args":[{"property":"a"},{"interval":["x",".."]}]}"#,
        r#"{"op":"t_during","args":[{"property":"a"},{"interval":[5,".."]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"bbox":[1,2,3]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"bbox":"1,2,3,4"}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"Point","coordinates":[1]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"Point"}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"Point","coordinates":[1,2],"bbox":[1,2,1,2]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"Circle","coordinates":[1,2]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"LineString","coordinates":[]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"LineString","coordinates":[[1]]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"GeometryCollection","geometries":[5]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"GeometryCollection","coordinates":[]}]}"#,
        r#"{"op":"s_intersects","args":[{"property":"a"},{"nothing":1}]}"#,
    ] {
        let e = refused(Lang::Json, filter);
        assert_eq!(e.at, 0, "{filter}");
    }
    let tree = json(
        r#"{"op":"s_intersects","args":[{"property":"a"},{"type":"GeometryCollection","geometries":[{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}]}]}"#,
    );
    let Expr::Op(Op::SIntersects, args) = &tree else {
        unreachable!("{tree:?}")
    };
    assert!(
        matches!(args.get(1), Some(Expr::Geometry(Geometry::Collection(parts))) if parts.len() == 1)
    );
    assert_eq!(text(&tree.to_string()), tree);
}

/// `n` intervals, each the start of the one around it.
fn intervals(n: usize) -> String {
    format!(
        "T_INTERSECTS(t, {}'..'{})",
        "INTERVAL(".repeat(n),
        ", '..')".repeat(n)
    )
}

/// A filter of `MAX_LEN` bytes nesting intervals is refused on the stack of a tokio worker
/// (2 MiB), where unbounded recursion would abort the process.
#[test]
fn nested_intervals_of_max_len_are_refused_on_a_small_stack() {
    let n = (MAX_LEN - 32) / ", '..')INTERVAL(".len();
    let deep = intervals(n);
    assert!(deep.len() <= MAX_LEN, "{}", deep.len());
    let what = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || parse(Lang::Text, &deep).map_err(|e| e.what))
        .map(std::thread::JoinHandle::join);
    assert!(
        matches!(what, Ok(Ok(Err("the filter nests deeper than the limit")))),
        "{what:?}"
    );
}

#[test]
fn depth_and_size_bounded() {
    let nested = |n: usize| format!("{}pm10>50{}", "(".repeat(n), ")".repeat(n));
    assert!(
        parse(Lang::Text, &nested(MAX_DEPTH)).is_ok(),
        "the ceiling itself is allowed"
    );
    for deep in [
        nested(MAX_DEPTH + 1),
        nested(5_000),
        format!("{}pm10>50", "NOT ".repeat(1_000)),
        format!("a = {}1", "-".repeat(1_000)),
        intervals(40),
        format!("{}1{}", "abs(".repeat(40), ")".repeat(40)),
        format!(
            "S_INTERSECTS(g, {}POINT(1 2){})",
            "GEOMETRYCOLLECTION(".repeat(40),
            ")".repeat(40)
        ),
        format!("a IN {}1{}", "(".repeat(40), ")".repeat(40)),
    ] {
        assert_eq!(
            refused(Lang::Text, &deep).what,
            "the filter nests deeper than the limit"
        );
    }
    let wide = (0..64)
        .map(|n| format!("(pm10>{n})"))
        .collect::<Vec<_>>()
        .join(" OR ");
    assert!(
        parse(Lang::Text, &wide).is_ok(),
        "a wide filter is not a deep one"
    );

    let json_deep = format!(
        "{}{}{}",
        r#"{"op":"not","args":["#.repeat(MAX_DEPTH + 1),
        "true",
        "]}".repeat(MAX_DEPTH + 1)
    );
    assert_eq!(
        refused(Lang::Json, &json_deep).what,
        "the filter nests deeper than the limit"
    );

    let long = format!("a = '{}'", "x".repeat(MAX_LEN));
    assert_eq!(
        refused(Lang::Text, &long),
        Error {
            at: MAX_LEN,
            what: "the filter is longer than the limit"
        }
    );
    assert_eq!(
        refused(Lang::Json, &long).what,
        "the filter is longer than the limit"
    );

    let many = format!("a IN ({})", vec!["1"; 5_000].join(","));
    assert!(many.len() < MAX_LEN);
    assert_eq!(
        refused(Lang::Text, &many).what,
        "the filter has more nodes than the limit"
    );
    let many_json = format!(
        r#"{{"op":"in","args":[{{"property":"a"}},[{}]]}}"#,
        vec!["1"; 5_000].join(",")
    );
    assert_eq!(
        refused(Lang::Json, &many_json).what,
        "the filter has more nodes than the limit"
    );
    // Positions count: 3990 list items alone fit, the 200 positions beside them do not.
    let items = vec!["1"; 3_990].join(",");
    let line = vec!["0 0"; 200].join(",");
    assert!(parse(Lang::Text, &format!("a IN ({items})")).is_ok());
    let both = format!("a IN ({items}) AND S_INTERSECTS(b, LINESTRING({line}))");
    assert_eq!(
        refused(Lang::Text, &both).what,
        "the filter has more nodes than the limit"
    );
}

#[test]
fn filter_lang_names() {
    assert_eq!(Lang::from_param("cql2-text"), Some(Lang::Text));
    assert_eq!(Lang::from_param("cql2-json"), Some(Lang::Json));
    assert_eq!(Lang::from_param("CQL2-TEXT"), None);
    assert_eq!(Op::from_name("t_finishedBy"), Op::TFinishedBy);
    assert_eq!(
        Op::from_name("T_FINISHEDBY"),
        Op::Function("T_FINISHEDBY".into()),
        "JSON names are case-sensitive"
    );
    assert_eq!(Op::IsNull.name(), "isNull");
    assert_eq!(Op::Function("f".into()).name(), "f");
}

#[test]
fn instants_are_checked() {
    for date in [
        "2026-09-18x",
        "2026/09-18",
        "2026-09/18",
        "20x6-09-18",
        "2026-13-18",
        "2026-00-18",
        "2026-09-32",
        "2026-09-00",
    ] {
        assert!(
            parse(Lang::Text, &format!("T_AFTER(a, DATE('{date}'))")).is_err(),
            "{date}"
        );
    }
    for stamp in [
        "2026-09-18 00:00:00Z",
        "2026-09-18T00-00:00Z",
        "2026-09-18T00:00-00Z",
        "2026-09-18T24:00:00Z",
        "2026-09-18T00:60:00Z",
        "2026-09-18T00:00:61Z",
        "2026-09-18T00:00:00",
        "2026-09-18T00:00:00+01:00",
        "2026-09-18T00:00:00.Z",
        "2026-09-18T00:00:00.xZ",
        "2026-09-18T00:00:00,5Z",
        "2026-9-18T00:00:00Z",
    ] {
        assert!(
            parse(Lang::Text, &format!("T_AFTER(a, TIMESTAMP('{stamp}'))")).is_err(),
            "{stamp}"
        );
    }
    assert!(parse(
        Lang::Text,
        "T_AFTER(a, TIMESTAMP('2026-09-18T23:59:60.123Z'))"
    )
    .is_ok());
}

#[test]
fn names_that_are_not_plain_are_written_quoted() {
    for (name, written) in [
        ("a b", r#""a b""#),
        ("and", r#""and""#),
        ("1x", r#""1x""#),
        ("a\"b", r#""a""b""#),
        ("ok_1.x", "ok_1.x"),
    ] {
        let tree = Expr::Op(Op::IsNull, vec![prop(name)]);
        assert_eq!(tree.to_string(), format!("{written} IS NULL"));
        assert_eq!(text(&tree.to_string()), tree);
    }
}
