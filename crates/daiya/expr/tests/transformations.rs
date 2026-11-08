#![forbid(unsafe_code)]
//! The 13 core transformations of R4 against Grafana's `@grafana/data` transformers
//! (`organize.ts`, `merge.ts`, `reduce.ts`, `filterByValue.ts`, `calculateField.ts`,
//! `groupBy.ts`, `sortBy.ts`, `renameByRegex.ts`, `joinByField.ts`, `convertFieldType.ts`,
//! `limit.ts`, `seriesToRows.ts`, `labelsToFields.ts`) and the pipeline that runs them.

use daiya_expr::apply_transformations;
use daiya_expr::tx::{
    sort_frame_by_col, tx_calculate_field, tx_convert_field_type, tx_filter_by_value, tx_group_by,
    tx_join_by_field, tx_labels_to_fields, tx_limit, tx_merge, tx_organize, tx_reduce,
    tx_rename_by_regex, tx_series_to_rows, tx_sort_by,
};
use daiya_model::{DataTransformerConfig, FieldColumn, FieldType, Frame, FrameMeta};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn frame(name: &str, fields: Vec<FieldColumn>) -> Frame {
    Frame {
        name: name.into(),
        ref_id: Some("A".into()),
        fields,
        meta: FrameMeta::default(),
    }
}

fn nums(name: &str, vals: &[Option<f64>]) -> FieldColumn {
    FieldColumn::numbers(name, BTreeMap::new(), vals.to_vec())
}

fn strs(name: &str, vals: &[Option<&str>]) -> FieldColumn {
    FieldColumn::strings(name, vals.iter().map(|v| v.map(str::to_owned)).collect())
}

fn labelled(name: &str, pairs: &[(&str, &str)], vals: &[Option<f64>]) -> FieldColumn {
    let labels = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    FieldColumn::numbers(name, labels, vals.to_vec())
}

fn names(f: &Frame) -> Vec<&str> {
    f.fields.iter().map(|c| c.name.as_str()).collect()
}

fn col<'a>(f: &'a Frame, name: &str) -> &'a FieldColumn {
    f.fields
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no field {name} in {:?}", names(f)))
}

fn floats(c: &FieldColumn) -> Vec<Option<f64>> {
    (0..c.len()).map(|i| c.f64_at(i)).collect()
}

fn texts(c: &FieldColumn) -> Vec<Option<&str>> {
    c.values.iter().map(Value::as_str).collect()
}

fn tx(id: &str, options: Value) -> DataTransformerConfig {
    serde_json::from_value(json!({ "id": id, "options": options })).unwrap()
}

/// Rows of stations: Time, station, pm10.
fn stations() -> Frame {
    frame(
        "air",
        vec![
            FieldColumn::time("Time", vec![1000, 2000, 3000, 4000]),
            strs("station", &[Some("a"), Some("b"), Some("a"), None]),
            nums("pm10", &[Some(10.0), Some(30.0), Some(20.0), None]),
        ],
    )
}

#[test]
fn the_pipeline_runs_steps_in_order_skips_disabled_and_warns_unknown_ids() {
    let mut disabled = tx("limit", json!({ "limitField": 1 }));
    disabled.disabled = Some(true);
    let out = apply_transformations(
        vec![stations()],
        &[
            tx(
                "sortBy",
                json!({ "sort": [{ "field": "pm10", "desc": true }] }),
            ),
            disabled,
            tx("noop", json!({})),
            tx("prepareTimeSeries", json!({})),
            tx("limit", json!({ "limitField": 2 })),
        ],
    );
    assert_eq!(
        floats(col(&out.frames[0], "pm10")),
        [Some(30.0), Some(20.0)]
    );
    let msg = "transformation 'prepareTimeSeries' is not executed in this run and passes through unchanged";
    assert_eq!(out.warnings, [msg]);
    assert_eq!(out.frames[0].meta.notices[0].text, msg);

    // An unknown step over no frames still warns.
    let out = apply_transformations(vec![], &[tx("histogram", json!({}))]);
    assert_eq!(out.warnings.len(), 1);
    assert!(out.frames.is_empty());

    // Every one of the 13 ids (and the seriesToColumns alias) is dispatched.
    let two = vec![
        frame(
            "x",
            vec![FieldColumn::time("Time", vec![1]), nums("a", &[Some(1.0)])],
        ),
        frame(
            "y",
            vec![FieldColumn::time("Time", vec![1]), nums("b", &[Some(2.0)])],
        ),
    ];
    for id in [
        "organize",
        "merge",
        "reduce",
        "filterByValue",
        "calculateField",
        "groupBy",
        "sortBy",
        "renameByRegex",
        "joinByField",
        "seriesToColumns",
        "convertFieldType",
        "limit",
        "seriesToRows",
        "labelsToFields",
    ] {
        let out = apply_transformations(two.clone(), &[tx(id, json!({}))]);
        assert!(out.warnings.is_empty(), "{id}");
    }
}

#[test]
fn organize_filters_orders_and_renames() {
    let mut f = stations();
    f.fields[2].display_name = Some("PM10".into());
    // includeByName keeps only the named fields, by name or display name.
    let out = tx_organize(
        vec![f.clone()],
        &json!({
            "includeByName": { "Time": true, "PM10": true, "station": false },
            "indexByName": { "PM10": 0 },
            "renameByName": { "Time": "When", "pm10": "" }
        }),
    );
    assert_eq!(names(&out[0]), ["pm10", "When"]);
    assert_eq!(out[0].fields[1].display_name.as_deref(), Some("When"));
    assert_eq!(
        out[0].fields[0].display_name.as_deref(),
        Some("PM10"),
        "empty rename is ignored"
    );

    // excludeByName by display name; fields without an index keep their order after indexed ones.
    let out = tx_organize(
        vec![f],
        &json!({
            "excludeByName": { "PM10": true },
            "indexByName": { "station": 0 },
            "renameByName": { "station": "Station" }
        }),
    );
    assert_eq!(names(&out[0]), ["Station", "Time"]);

    // No options: unchanged.
    let out = tx_organize(vec![stations()], &json!({}));
    assert_eq!(out, [stations()]);
}

#[test]
fn merge_joins_rows_on_the_fields_every_frame_shares() {
    let a = frame(
        "a",
        vec![
            FieldColumn::time("Time", vec![1000, 2000]),
            nums("temp", &[Some(1.0), Some(2.0)]),
        ],
    );
    let b = frame(
        "b",
        vec![
            FieldColumn::time("Time", vec![1000, 3000]),
            nums("hum", &[Some(50.0), Some(60.0)]),
        ],
    );
    let c = frame(
        "c",
        vec![
            FieldColumn::time("Time", vec![1000]),
            nums("temp", &[Some(9.0)]),
        ],
    );
    let out = tx_merge(vec![a, b, c, Frame::default()]);
    assert_eq!(out.len(), 1);
    let m = &out[0];
    assert_eq!(m.name, "merged");
    assert_eq!(names(m), ["Time", "temp", "hum"]);
    assert_eq!(
        col(m, "Time").values,
        [json!(1000), json!(2000), json!(3000), json!(1000)]
    );
    // Row 1000 of a and b merge; c's temp 9 conflicts with 1 and makes its own row.
    assert_eq!(
        floats(col(m, "temp")),
        [Some(1.0), Some(2.0), None, Some(9.0)]
    );
    assert_eq!(floats(col(m, "hum")), [Some(50.0), None, Some(60.0), None]);
    assert_eq!(col(m, "Time").field_type, FieldType::Time);

    // One frame, or frames sharing no field, pass through.
    assert_eq!(tx_merge(vec![stations()]), [stations()]);
    let x = frame("x", vec![nums("a", &[Some(1.0)])]);
    let y = frame("y", vec![nums("b", &[Some(2.0)])]);
    assert_eq!(tx_merge(vec![x.clone(), y.clone()]), [x, y]);
}

#[test]
fn reduce_series_to_rows_names_columns_after_the_calculations() {
    let a = frame(
        "a",
        vec![
            FieldColumn::time("Time", vec![1, 2, 3]),
            nums("cpu", &[Some(1.0), None, Some(5.0)]),
            nums("mem", &[Some(2.0), Some(2.0), Some(8.0)]),
        ],
    );
    let out = tx_reduce(
        vec![a.clone()],
        &json!({ "reducers": ["max", "mean", "sum", "count", "lastNotNull", "nope"] }),
    );
    let r = &out[0];
    assert_eq!(
        names(r),
        ["Field", "Max", "Mean", "Total", "Count", "Last *"]
    );
    assert_eq!(texts(col(r, "Field")), [Some("cpu"), Some("mem")]);
    assert_eq!(floats(col(r, "Max")), [Some(5.0), Some(8.0)]);
    assert_eq!(
        floats(col(r, "Mean")),
        [Some(3.0), Some(4.0)],
        "nulls are ignored"
    );
    assert_eq!(floats(col(r, "Total")), [Some(6.0), Some(12.0)]);
    assert_eq!(floats(col(r, "Count")), [Some(2.0), Some(3.0)]);
    assert_eq!(floats(col(r, "Last *")), [Some(5.0), Some(8.0)]);

    // Grafana's default calculation is max.
    let out = tx_reduce(vec![a.clone()], &json!({}));
    assert_eq!(names(&out[0]), ["Field", "Max"]);
    // Only unknown reducers: nothing to compute.
    assert_eq!(
        tx_reduce(vec![a.clone()], &json!({ "reducers": ["nope"] })),
        std::slice::from_ref(&a)
    );

    // reduceFields keeps each frame and collapses each non-time field to one value.
    let only_time = frame("t", vec![FieldColumn::time("Time", vec![1])]);
    let out = tx_reduce(
        vec![a, only_time],
        &json!({ "mode": "reduceFields", "reducers": ["min"] }),
    );
    assert_eq!(out.len(), 1, "a frame with nothing to reduce is dropped");
    assert_eq!(names(&out[0]), ["cpu", "mem"]);
    assert_eq!(floats(&out[0].fields[0]), [Some(1.0)]);
    assert_eq!(floats(&out[0].fields[1]), [Some(2.0)]);
}

#[test]
fn filter_by_value_matchers_include_and_exclude_rows() {
    let rows = |opts: Value| -> Vec<Option<f64>> {
        floats(col(&tx_filter_by_value(vec![stations()], &opts)[0], "pm10"))
    };
    let one = |id: &str, options: Value| json!({ "filters": [{ "fieldName": "pm10", "config": { "id": id, "options": options } }] });
    assert_eq!(
        rows(one("greater", json!({ "value": 15 }))),
        [Some(30.0), Some(20.0)]
    );
    assert_eq!(
        rows(one("greaterOrEqual", json!({ "value": 20 }))),
        [Some(30.0), Some(20.0)]
    );
    assert_eq!(rows(one("lower", json!({ "value": 20 }))), [Some(10.0)]);
    assert_eq!(
        rows(one("lowerOrEqual", json!({ "value": 20 }))),
        [Some(10.0), Some(20.0)]
    );
    assert_eq!(rows(one("equal", json!({ "value": 30 }))), [Some(30.0)]);
    assert_eq!(rows(one("isNull", json!({}))), [None]);
    assert_eq!(
        rows(one("isNotNull", json!({}))),
        [Some(10.0), Some(30.0), Some(20.0)]
    );
    assert_eq!(
        rows(one("between", json!({ "from": 10, "to": 20 }))),
        [Some(10.0), Some(20.0)]
    );
    assert!(rows(one("unknownMatcher", json!({}))).is_empty());

    // String matchers on the station field.
    let on_station = |id: &str, value: Value| json!({ "filters": [{ "fieldName": "station", "config": { "id": id, "options": { "value": value } } }] });
    assert_eq!(
        rows(on_station("equal", json!("a"))),
        [Some(10.0), Some(20.0)]
    );
    assert_eq!(rows(on_station("regex", json!("^b$"))), [Some(30.0)]);
    assert!(
        rows(on_station("regex", json!("("))).is_empty(),
        "a bad regex matches nothing"
    );
    // The default matcher is equal; a missing field matches nothing.
    assert_eq!(
        rows(json!({ "filters": [{ "fieldName": "pm10", "config": { "options": 20 } }] })),
        [Some(20.0)]
    );

    // Exclude with match any: drop rows where pm10 > 25 or station is b.
    let opts = json!({
        "type": "exclude", "match": "any",
        "filters": [
            { "fieldName": "missing", "config": { "id": "isNull" } },
            { "fieldName": "pm10", "config": { "id": "greater", "options": { "value": 25 } } },
            { "fieldName": "station", "config": { "id": "equal", "options": { "value": "b" } } }
        ]
    });
    assert_eq!(rows(opts), [Some(10.0), Some(20.0), None]);
    // Include with match all.
    let opts = json!({
        "type": "include", "match": "all",
        "filters": [
            { "fieldName": "pm10", "config": { "id": "greater", "options": { "value": 5 } } },
            { "fieldName": "station", "config": { "id": "equal", "options": { "value": "a" } } }
        ]
    });
    let out = tx_filter_by_value(vec![stations()], &opts);
    assert_eq!(floats(col(&out[0], "pm10")), [Some(10.0), Some(20.0)]);
    assert_eq!(col(&out[0], "Time").values, [json!(1000), json!(3000)]);

    // No filters: unchanged.
    assert_eq!(
        tx_filter_by_value(vec![stations()], &json!({})),
        [stations()]
    );
    assert_eq!(
        tx_filter_by_value(vec![stations()], &json!({ "filters": [] })),
        [stations()]
    );
}

fn ab() -> Frame {
    frame(
        "ab",
        vec![
            FieldColumn::time("Time", vec![1, 2, 3]),
            nums("a", &[Some(6.0), Some(2.0), None]),
            FieldColumn {
                display_name: Some("B".into()),
                ..nums("b", &[Some(3.0), Some(4.0), Some(1.0)])
            },
            strs("s", &[Some("x"), Some("y"), Some("z")]),
        ],
    )
}

fn calc(opts: Value) -> Frame {
    tx_calculate_field(vec![ab()], &opts).remove(0)
}

#[test]
fn calculate_field_binary_operations_follow_grafana_operators() {
    let bin = |left: Value, op: &str, right: Value| {
        calc(
            json!({ "mode": "binary", "binary": { "left": left, "operator": op, "right": right } }),
        )
    };
    let last = |f: &Frame| floats(f.fields.last().unwrap());

    // Grafana's binary operators: + - * / % ^.
    let f = bin(json!("a"), "+", json!("B"));
    assert_eq!(
        f.fields.last().unwrap().name,
        "a + B",
        "default name: left operator right"
    );
    assert_eq!(
        last(&f),
        [Some(9.0), Some(6.0), Some(1.0)],
        "a null operand counts as 0"
    );
    assert_eq!(
        last(&bin(json!("a"), "-", json!("b")))[..2],
        [Some(3.0), Some(-2.0)]
    );
    assert_eq!(
        last(&bin(json!("a"), "*", json!("2")))[..2],
        [Some(12.0), Some(4.0)]
    );
    assert_eq!(
        last(&bin(json!("a"), "/", json!("b")))[..2],
        [Some(2.0), Some(0.5)]
    );
    assert_eq!(
        last(&bin(json!("a"), "%", json!("b")))[..2],
        [Some(0.0), Some(2.0)]
    );
    assert_eq!(
        last(&bin(json!("a"), "^", json!("b")))[..2],
        [Some(216.0), Some(16.0)]
    );
    assert_eq!(
        last(&bin(json!("a"), "/", json!("0")))[0],
        None,
        "x / 0 is not a finite number"
    );

    // Grafana 11.4+ operand objects: a field matcher or a fixed value.
    let f = bin(
        json!({ "matcher": { "id": "byName", "options": "a" } }),
        "*",
        json!({ "fixed": "10" }),
    );
    assert_eq!(f.fields.last().unwrap().name, "a * 10");
    assert_eq!(last(&f)[..2], [Some(60.0), Some(20.0)]);
    let f = bin(
        json!({ "fixed": 1.5 }),
        "+",
        json!({ "matcher": { "options": "missing" } }),
    );
    assert_eq!(
        last(&f),
        [Some(1.5), Some(1.5), Some(1.5)],
        "an unknown field counts as 0"
    );
    let f = bin(json!("nope"), "+", json!({ "fixed": "x" }));
    assert_eq!(last(&f), [Some(0.0), Some(0.0), Some(0.0)]);
    let f = calc(json!({ "mode": "binary", "binary": {} }));
    assert_eq!(last(&f), [Some(0.0), Some(0.0), Some(0.0)]);

    // An alias names the field; replaceFields keeps only the time field and the result.
    let f = calc(json!({
        "mode": "binary", "alias": "sum", "replaceFields": true,
        "binary": { "left": "a", "operator": "+", "right": "b" }
    }));
    assert_eq!(names(&f), ["Time", "sum"]);
}

#[test]
fn calculate_field_reduce_row_cumulative_and_index_modes() {
    // reduceRow (the default mode) reduces each row over the number fields, nulls skipped.
    let f = calc(json!({}));
    assert_eq!(
        f.fields.last().unwrap().name,
        "Total",
        "Grafana names sum Total"
    );
    assert_eq!(
        floats(f.fields.last().unwrap()),
        [Some(9.0), Some(6.0), Some(1.0)]
    );
    let f = calc(json!({ "mode": "reduceRow", "reduce": { "reducer": "max" } }));
    assert_eq!(f.fields.last().unwrap().name, "Max");
    assert_eq!(
        floats(f.fields.last().unwrap()),
        [Some(6.0), Some(4.0), Some(1.0)]
    );
    let f = calc(json!({ "mode": "reduceRow", "reduce": { "reducer": "mean" }, "alias": "avg" }));
    assert_eq!(f.fields.last().unwrap().name, "avg");
    assert_eq!(
        floats(f.fields.last().unwrap()),
        [Some(4.5), Some(3.0), Some(1.0)]
    );
    let f = calc(json!({ "mode": "reduceRow", "reduce": { "reducer": "p99" } }));
    assert_eq!(
        floats(f.fields.last().unwrap()),
        [Some(9.0), Some(6.0), Some(1.0)],
        "unknown is sum"
    );

    // cumulativeFunctions over the named field, or the first number field.
    let f = calc(json!({ "mode": "cumulativeFunctions", "cumulative": { "field": "B" } }));
    assert_eq!(
        floats(f.fields.last().unwrap()),
        [Some(3.0), Some(7.0), Some(8.0)]
    );
    let f = calc(json!({
        "mode": "cumulativeFunctions", "cumulative": { "field": "b", "reducer": "mean" }
    }));
    assert_eq!(f.fields.last().unwrap().name, "cumulative mean(b)");
    assert_eq!(
        floats(f.fields.last().unwrap())[..2],
        [Some(3.0), Some(3.5)]
    );
    let f = calc(json!({ "mode": "cumulativeFunctions" }));
    assert_eq!(f.fields.last().unwrap().name, "cumulative sum");
    assert_eq!(
        floats(f.fields.last().unwrap()),
        [Some(6.0), Some(8.0), Some(8.0)]
    );

    // index: the row number, or its share of the row count.
    let f = calc(json!({ "mode": "index" }));
    assert_eq!(f.fields.last().unwrap().name, "Row");
    assert_eq!(
        floats(f.fields.last().unwrap()),
        [Some(0.0), Some(1.0), Some(2.0)]
    );
    let f = calc(json!({ "mode": "index", "index": { "asPercentile": true } }));
    assert_eq!(f.fields.last().unwrap().f64_at(1), Some(1.0 / 3.0));

    // replaceFields on a frame without a time field keeps only the result.
    let no_time = frame("n", vec![nums("a", &[Some(1.0)])]);
    let out = tx_calculate_field(
        vec![no_time],
        &json!({ "replaceFields": true, "mode": "index" }),
    );
    assert_eq!(names(&out[0]), ["Row"]);
}

#[test]
fn group_by_groups_rows_in_first_seen_order_and_aggregates() {
    let opts = json!({ "fields": {
        "station": { "operation": "groupby" },
        "pm10": { "operation": "aggregate", "aggregations": ["sum", "count", "nope"] },
        "absent": { "operation": "aggregate", "aggregations": ["max"] },
        "Time": { "operation": "aggregate", "aggregations": [] },
        "other": { "operation": "" }
    }});
    let out = tx_group_by(
        vec![stations(), frame("no key", vec![nums("x", &[])])],
        &opts,
    );
    assert_eq!(out.len(), 1, "a frame without the key field is dropped");
    let g = &out[0];
    assert_eq!(names(g), ["station", "pm10 (sum)", "pm10 (count)"]);
    assert_eq!(texts(col(g, "station")), [Some("a"), Some("b"), None]);
    assert_eq!(
        floats(col(g, "pm10 (sum)")),
        [Some(30.0), Some(30.0), Some(0.0)]
    );
    assert_eq!(
        floats(col(g, "pm10 (count)")),
        [Some(2.0), Some(1.0), Some(0.0)]
    );

    // No groupby field, or no fields option: unchanged.
    let agg_only =
        json!({ "fields": { "pm10": { "operation": "aggregate", "aggregations": ["sum"] } } });
    assert_eq!(tx_group_by(vec![stations()], &agg_only), [stations()]);
    assert_eq!(tx_group_by(vec![stations()], &json!({})), [stations()]);
}

#[test]
fn sort_by_orders_numbers_then_strings_with_nulls_first() {
    let out = tx_sort_by(vec![stations()], &json!({ "sort": [{ "field": "pm10" }] }));
    assert_eq!(
        floats(col(&out[0], "pm10")),
        [None, Some(10.0), Some(20.0), Some(30.0)]
    );
    assert_eq!(
        col(&out[0], "Time").values,
        [json!(4000), json!(1000), json!(3000), json!(2000)]
    );

    let out = tx_sort_by(
        vec![stations()],
        &json!({ "sort": [{ "displayName": "station", "desc": true }] }),
    );
    assert_eq!(
        texts(col(&out[0], "station")),
        [Some("b"), Some("a"), Some("a"), None]
    );

    // A missing field, no sort entries or no sort option: unchanged.
    for opts in [
        json!({ "sort": [{ "field": "nope" }] }),
        json!({ "sort": [] }),
        json!({}),
    ] {
        assert_eq!(tx_sort_by(vec![stations()], &opts), [stations()], "{opts}");
    }

    let mut f = stations();
    sort_frame_by_col(&mut f, 0, true);
    assert_eq!(col(&f, "Time").values[0], json!(4000));
}

#[test]
fn rename_by_regex_matches_the_whole_name_like_string_to_js_regex() {
    let f = frame(
        "m",
        vec![
            nums("pm25", &[Some(1.0)]),
            nums("value_pm10", &[Some(2.0)]),
            nums("banana", &[Some(3.0)]),
        ],
    );
    // A plain pattern is anchored (^...$): "pm" does not rename "pm25".
    let out = tx_rename_by_regex(
        vec![f.clone()],
        &json!({ "regex": "pm", "renamePattern": "x" }),
    );
    assert_eq!(names(&out[0]), ["pm25", "value_pm10", "banana"]);

    // JS replacement: $1 followed by text, $$ for a dollar.
    let out = tx_rename_by_regex(
        vec![f.clone()],
        &json!({ "regex": "value_(.*)", "renamePattern": "$1_avg $$" }),
    );
    assert_eq!(names(&out[0]), ["pm25", "pm10_avg $", "banana"]);
    assert_eq!(out[0].fields[1].display_name.as_deref(), Some("pm10_avg $"));

    // /pattern/flags: unanchored, g replaces every match, i ignores case.
    let out = tx_rename_by_regex(
        vec![f.clone()],
        &json!({ "regex": "/A/gi", "renamePattern": "o" }),
    );
    assert_eq!(names(&out[0])[2], "bonono");
    let out = tx_rename_by_regex(
        vec![f.clone()],
        &json!({ "regex": "/a/", "renamePattern": "o" }),
    );
    assert_eq!(
        names(&out[0])[2],
        "bonana",
        "without g only the first match"
    );
    let out = tx_rename_by_regex(
        vec![f.clone()],
        &json!({ "regex": "/(p)(m)/", "renamePattern": "$2$1-$&" }),
    );
    assert_eq!(names(&out[0])[..2], ["mp-pm25", "value_mp-pm10"]);

    // A bad regex or missing options leave the frames alone.
    for opts in [
        json!({ "regex": "(", "renamePattern": "x" }),
        json!({ "regex": "/(/", "renamePattern": "x" }),
        json!({ "regex": "/a/q", "renamePattern": "x" }),
        json!({ "renamePattern": "x" }),
        json!({ "regex": ".*" }),
    ] {
        assert_eq!(
            tx_rename_by_regex(vec![f.clone()], &opts),
            std::slice::from_ref(&f),
            "{opts}"
        );
    }
}

#[test]
fn join_by_field_outer_joins_sorted_and_inner_keeps_shared_keys() {
    let a = frame(
        "a",
        vec![
            FieldColumn::time("Time", vec![1000, 3000]),
            labelled("temp", &[("room", "1")], &[Some(1.0), Some(3.0)]),
        ],
    );
    let b = frame(
        "b",
        vec![
            FieldColumn::time("Time", vec![2000, 3000]),
            nums("hum", &[Some(20.0), Some(30.0)]),
        ],
    );
    let out = tx_join_by_field(vec![a.clone(), b.clone()], &json!({}));
    let j = &out[0];
    assert_eq!(j.name, "joined");
    assert_eq!(names(j), ["Time", "temp", "hum"]);
    assert_eq!(col(j, "Time").field_type, FieldType::Time);
    assert_eq!(
        col(j, "Time").values,
        [json!(1000), json!(2000), json!(3000)]
    );
    assert_eq!(floats(col(j, "temp")), [Some(1.0), None, Some(3.0)]);
    assert_eq!(floats(col(j, "hum")), [None, Some(20.0), Some(30.0)]);
    assert_eq!(
        col(j, "temp").labels.get("room").map(String::as_str),
        Some("1")
    );

    let out = tx_join_by_field(
        vec![a.clone(), b],
        &json!({ "mode": "inner", "byField": "Time" }),
    );
    assert_eq!(col(&out[0], "Time").values, [json!(3000)]);

    // A string key, joined in first-seen order; a frame without the key adds nulls.
    let k1 = frame(
        "k1",
        vec![
            strs("id", &[Some("y"), Some("x")]),
            nums("v", &[Some(1.0), Some(2.0)]),
        ],
    );
    let k2 = frame(
        "k2",
        vec![strs("id", &[Some("x")]), nums("w", &[Some(9.0)])],
    );
    let k3 = frame("k3", vec![nums("z", &[Some(5.0)])]);
    let out = tx_join_by_field(vec![k1, k2, k3], &json!({ "byField": "id" }));
    let j = &out[0];
    assert_eq!(texts(col(j, "id")), [Some("y"), Some("x")]);
    assert_eq!(floats(col(j, "w")), [None, Some(9.0)]);
    assert_eq!(floats(col(j, "z")), [None, None]);

    // Without a time field the first field is the key; with no fields at all, "Time".
    let n1 = frame(
        "n1",
        vec![
            nums("k", &[Some(2.0), Some(1.0)]),
            nums("v", &[Some(20.0), Some(10.0)]),
        ],
    );
    let n2 = frame("n2", vec![nums("k", &[Some(1.0)]), nums("w", &[Some(5.0)])]);
    let out = tx_join_by_field(vec![n1, n2], &json!({}));
    assert_eq!(floats(col(&out[0], "k")), [Some(1.0), Some(2.0)]);
    assert_eq!(floats(col(&out[0], "w")), [Some(5.0), None]);
    let out = tx_join_by_field(vec![Frame::default(), Frame::default()], &json!({}));
    assert_eq!(names(&out[0]), ["Time"]);

    assert_eq!(tx_join_by_field(vec![a.clone()], &json!({})), [a]);
}

#[test]
fn convert_field_type_converts_values_like_grafana() {
    let f = frame(
        "c",
        vec![
            FieldColumn {
                values: vec![
                    json!("1,234.5"),
                    json!(" 7 "),
                    json!(true),
                    json!(false),
                    json!("x"),
                    json!(3),
                    Value::Null,
                ],
                ..strs("n", &[])
            },
            FieldColumn {
                values: vec![
                    json!(1),
                    json!("s"),
                    Value::Null,
                    json!(true),
                    json!(0),
                    json!(""),
                    json!("false"),
                ],
                ..nums("s", &[])
            },
            FieldColumn {
                values: vec![
                    json!("2024-01-01T00:00:00Z"),
                    json!(5),
                    json!("bad"),
                    Value::Null,
                    json!(1.5),
                    json!(0),
                    json!(0),
                ],
                ..strs("t", &[])
            },
        ],
    );
    let out = tx_convert_field_type(
        vec![f.clone()],
        &json!({ "conversions": [
            { "targetField": "n", "destinationType": "number" },
            { "targetField": "s", "destinationType": "string" },
            { "targetField": "t", "destinationType": "time" },
            { "targetField": "t", "destinationType": "other" },
            { "targetField": "absent", "destinationType": "number" }
        ]}),
    );
    let c = &out[0];
    assert_eq!(col(c, "n").field_type, FieldType::Number);
    assert_eq!(
        floats(col(c, "n")),
        [
            Some(1234.5),
            Some(7.0),
            Some(1.0),
            Some(0.0),
            None,
            Some(3.0),
            None
        ]
    );
    assert_eq!(col(c, "s").field_type, FieldType::String);
    assert_eq!(
        col(c, "s").values,
        [
            json!("1"),
            json!("s"),
            Value::Null,
            json!("true"),
            json!("0"),
            json!(""),
            json!("false")
        ]
    );
    assert_eq!(col(c, "t").field_type, FieldType::Time);
    assert_eq!(
        col(c, "t").values,
        [
            json!(1_704_067_200_000_i64),
            json!(5),
            Value::Null,
            Value::Null,
            json!(1),
            json!(0),
            json!(0)
        ]
    );

    let out = tx_convert_field_type(
        vec![f.clone()],
        &json!({ "conversions": [{ "targetField": "s", "destinationType": "boolean" }] }),
    );
    assert_eq!(col(&out[0], "s").field_type, FieldType::Bool);
    assert_eq!(
        col(&out[0], "s").values,
        [
            json!(true),
            json!(true),
            json!(false),
            json!(true),
            json!(false),
            json!(false),
            json!(false)
        ]
    );
    assert_eq!(tx_convert_field_type(vec![f.clone()], &json!({})), [f]);
}

#[test]
fn limit_keeps_the_first_or_last_rows() {
    let rows = |opts: Value| {
        col(&tx_limit(vec![stations()], &opts)[0], "Time")
            .values
            .clone()
    };
    assert_eq!(rows(json!({ "limitField": 2 })), [json!(1000), json!(2000)]);
    assert_eq!(rows(json!({ "limitField": "1" })), [json!(1000)]);
    assert_eq!(
        rows(json!({ "limitField": -2 })),
        [json!(3000), json!(4000)]
    );
    assert_eq!(rows(json!({ "limitField": 10 })).len(), 4);
    assert_eq!(rows(json!({})).len(), 4, "default 10");
    assert_eq!(
        rows(json!({ "limitField": "x" })).len(),
        4,
        "unparsable is 10"
    );
    assert_eq!(
        rows(json!({ "limitField": 0.5 })).len(),
        4,
        "a fraction is 10"
    );
}

#[test]
fn series_to_rows_lists_every_point_newest_first() {
    let a = frame(
        "cpu",
        vec![
            FieldColumn::time("Time", vec![1000, 3000]),
            nums("v", &[Some(1.0), Some(3.0)]),
        ],
    );
    let b = frame(
        "",
        vec![
            FieldColumn::time("Time", vec![2000]),
            FieldColumn {
                display_name: Some("mem".into()),
                ..nums("v", &[Some(2.0)])
            },
        ],
    );
    let no_time = frame("x", vec![nums("v", &[Some(9.0)])]);
    let out = tx_series_to_rows(vec![a, b, no_time.clone()]);
    let r = &out[0];
    assert_eq!(names(r), ["Time", "Metric", "Value"]);
    assert_eq!(
        col(r, "Time").values,
        [json!(3000), json!(2000), json!(1000)]
    );
    assert_eq!(
        texts(col(r, "Metric")),
        [Some("cpu"), Some("mem"), Some("cpu")]
    );
    assert_eq!(floats(col(r, "Value")), [Some(3.0), Some(2.0), Some(1.0)]);

    assert_eq!(tx_series_to_rows(vec![no_time.clone()]), [no_time]);
}

#[test]
fn labels_to_fields_moves_labels_into_columns_or_rows() {
    let f = frame(
        "s",
        vec![
            FieldColumn::time("Time", vec![1, 2]),
            labelled(
                "value",
                &[("host", "a"), ("dc", "x"), ("name", "cpu")],
                &[Some(1.0), Some(2.0)],
            ),
        ],
    );
    let out = tx_labels_to_fields(vec![f.clone()], &json!({ "valueLabel": "name" }));
    let c = &out[0];
    assert_eq!(names(c), ["Time", "cpu", "dc", "host"]);
    assert_eq!(texts(col(c, "host")), [Some("a"), Some("a")]);
    assert!(col(c, "cpu").labels.is_empty());

    let out = tx_labels_to_fields(vec![f.clone()], &json!({ "keepLabels": ["host"] }));
    assert_eq!(names(&out[0]), ["Time", "value", "host"]);

    let out = tx_labels_to_fields(
        vec![f.clone()],
        &json!({ "mode": "rows", "keepLabels": ["dc", "host"] }),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "value");
    assert_eq!(texts(col(&out[0], "label")), [Some("dc"), Some("host")]);
    assert_eq!(texts(col(&out[0], "value")), [Some("x"), Some("a")]);
    let out = tx_labels_to_fields(vec![f], &json!({ "mode": "rows", "keepLabels": ["zone"] }));
    assert!(out.is_empty(), "no kept label, no row frame");
}

#[test]
fn edge_cases_of_names_replacements_and_join_keys() {
    let a = frame("a", vec![nums("v", &[Some(3.0), Some(1.0), Some(2.0)])]);
    let out = tx_reduce(
        vec![a.clone()],
        &json!({ "reducers": ["min", "last", "median"] }),
    );
    assert_eq!(names(&out[0]), ["Field", "Min", "Last", "Median"]);
    assert_eq!(floats(&out[0].fields[3]), [Some(2.0)]);

    // An operator Grafana does not list adds; an operand with neither field nor value is blank.
    let f =
        calc(json!({ "mode": "binary", "binary": { "left": "a", "operator": "?", "right": {} } }));
    assert_eq!(f.fields.last().unwrap().name, "a ? ");
    assert_eq!(floats(f.fields.last().unwrap())[0], Some(6.0));

    // $10 reads group 10 when it exists; a group that does not exist and a lone $ stay text.
    let g = frame("g", vec![nums("abcdefghij", &[Some(1.0)])]);
    let out = tx_rename_by_regex(
        vec![g.clone()],
        &json!({ "regex": "/(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)/y", "renamePattern": "$10$1-$11" }),
    );
    assert_eq!(names(&out[0]), ["ja-a1"]);
    let out = tx_rename_by_regex(
        vec![g],
        &json!({ "regex": "(abc)(.*)", "renamePattern": "$2.$9 $" }),
    );
    assert_eq!(names(&out[0]), ["defghij.$9 $"]);

    // A null key sorts before the numbers of an outer join.
    let k1 = frame(
        "k1",
        vec![
            nums("k", &[Some(2.0), None]),
            nums("v", &[Some(1.0), Some(0.0)]),
        ],
    );
    let k2 = frame("k2", vec![nums("k", &[Some(1.0)]), nums("w", &[Some(5.0)])]);
    let out = tx_join_by_field(vec![k1, k2], &json!({ "byField": "k" }));
    assert_eq!(floats(col(&out[0], "k")), [None, Some(1.0), Some(2.0)]);
    assert_eq!(floats(col(&out[0], "v")), [Some(0.0), None, Some(1.0)]);
}
