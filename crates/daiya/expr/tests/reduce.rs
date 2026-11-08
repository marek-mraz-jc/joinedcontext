#![forbid(unsafe_code)]
//! `reduce` against `GF pkg/expr/mathexp/reduce.go` and `commands.go:ReduceCommand`: every
//! reducer on empty, null and NaN input, the three modes, and what a reduce does to a value
//! that is already a number.

use daiya_expr::reduce::reduce_slice;
use daiya_expr::{ExprValue, ReduceMapper, ReduceMode, ReducerId, reduce_frames};
use std::collections::BTreeMap;

fn is_nan(v: Option<f64>) -> bool {
    v.is_some_and(f64::is_nan)
}

#[test]
fn reducer_ids_parse_case_insensitively_with_aliases() {
    for (s, id) in [
        ("sum", ReducerId::Sum),
        (" Mean ", ReducerId::Mean),
        ("avg", ReducerId::Mean),
        ("MIN", ReducerId::Min),
        ("max", ReducerId::Max),
        ("count", ReducerId::Count),
        ("last", ReducerId::Last),
        ("lastNotNull", ReducerId::Last),
        ("median", ReducerId::Median),
    ] {
        assert_eq!(ReducerId::parse(s).unwrap(), id, "{s}");
    }
    assert_eq!(
        ReducerId::parse("p95").unwrap_err().to_string(),
        "expression evaluation error: reduction 'p95' not implemented"
    );
}

#[test]
fn reducers_match_grafana_on_numbers_nulls_and_nan() {
    let vals = [Some(3.0), Some(-1.0), Some(4.0), Some(2.0)];
    assert_eq!(reduce_slice(ReducerId::Sum, &vals), Some(8.0));
    assert_eq!(reduce_slice(ReducerId::Mean, &vals), Some(2.0));
    assert_eq!(reduce_slice(ReducerId::Min, &vals), Some(-1.0));
    assert_eq!(reduce_slice(ReducerId::Max, &vals), Some(4.0));
    assert_eq!(reduce_slice(ReducerId::Count, &vals), Some(4.0));
    assert_eq!(reduce_slice(ReducerId::Last, &vals), Some(2.0));
    assert_eq!(reduce_slice(ReducerId::Median, &vals), Some(2.5));
    assert_eq!(reduce_slice(ReducerId::Median, &vals[..3]), Some(3.0));
    // The first value decides min and max even when it is the extreme.
    assert_eq!(
        reduce_slice(ReducerId::Min, &[Some(-5.0), Some(1.0)]),
        Some(-5.0)
    );
    assert_eq!(
        reduce_slice(ReducerId::Max, &[Some(5.0), Some(1.0)]),
        Some(5.0)
    );

    // Empty input: sum 0, count 0, everything else NaN.
    assert_eq!(reduce_slice(ReducerId::Sum, &[]), Some(0.0));
    assert_eq!(reduce_slice(ReducerId::Count, &[]), Some(0.0));
    for r in [
        ReducerId::Mean,
        ReducerId::Min,
        ReducerId::Max,
        ReducerId::Last,
        ReducerId::Median,
    ] {
        assert!(is_nan(reduce_slice(r, &[])), "{r:?}");
    }

    // A null or NaN anywhere makes sum, mean, min, max and median NaN; count counts it.
    for bad in [None, Some(f64::NAN)] {
        let with_bad = [Some(1.0), bad, Some(2.0)];
        for r in [
            ReducerId::Sum,
            ReducerId::Mean,
            ReducerId::Min,
            ReducerId::Max,
            ReducerId::Median,
        ] {
            assert!(is_nan(reduce_slice(r, &with_bad)), "{r:?} {bad:?}");
        }
        assert_eq!(reduce_slice(ReducerId::Count, &with_bad), Some(3.0));
        assert_eq!(reduce_slice(ReducerId::Last, &with_bad), Some(2.0));
    }
    // last returns the last point as it is, null included.
    assert_eq!(reduce_slice(ReducerId::Last, &[Some(1.0), None]), None);
}

#[test]
fn mappers_drop_or_replace_non_numbers() {
    let strict = ReduceMapper::new(ReduceMode::Strict);
    assert_eq!(strict.map_input(None), Some(None));
    assert!(is_nan(strict.map_output(Some(f64::NAN))));

    let drop = ReduceMapper::new(ReduceMode::DropNonNumber);
    assert_eq!(drop.map_input(Some(1.0)), Some(Some(1.0)));
    assert_eq!(drop.map_input(None), None);
    assert_eq!(drop.map_input(Some(f64::INFINITY)), None);
    assert_eq!(drop.map_output(Some(f64::NAN)), None);
    assert_eq!(drop.map_output(Some(f64::INFINITY)), Some(f64::INFINITY));
    assert_eq!(drop.map_output(None), None);

    let replace = ReduceMapper::new(ReduceMode::ReplaceNonNumber(-1.0));
    assert_eq!(replace.map_input(Some(2.0)), Some(Some(2.0)));
    assert_eq!(replace.map_input(None), Some(Some(-1.0)));
    assert_eq!(replace.map_input(Some(f64::NEG_INFINITY)), Some(Some(-1.0)));
    assert_eq!(replace.map_output(Some(f64::NAN)), Some(-1.0));
    assert_eq!(replace.map_output(Some(3.0)), Some(3.0));
    assert_eq!(replace.map_output(None), None);
}

fn series(points: &[(i64, Option<f64>)]) -> ExprValue {
    ExprValue::Series {
        name: "A".into(),
        labels: BTreeMap::from([("h".into(), "a".into())]),
        points: points.to_vec(),
        notices: Vec::new(),
    }
}

fn number(v: Option<f64>) -> ExprValue {
    ExprValue::Number {
        name: "A".into(),
        labels: BTreeMap::from([("h".into(), "n".into())]),
        value: v,
        notices: Vec::new(),
    }
}

fn reduce(mode: ReduceMode, inputs: &[ExprValue]) -> Vec<ExprValue> {
    reduce_frames("R", "A", ReducerId::Sum, mode, inputs).unwrap()
}

#[test]
fn modes_change_what_a_series_reduces_to() {
    let s = series(&[
        (1, Some(1.0)),
        (2, None),
        (3, Some(f64::INFINITY)),
        (4, Some(2.0)),
    ]);
    let ExprValue::Number { value, .. } =
        &reduce(ReduceMode::DropNonNumber, std::slice::from_ref(&s))[0]
    else {
        panic!("number");
    };
    assert_eq!(*value, Some(3.0));
    let ExprValue::Number {
        name,
        value,
        labels,
        ..
    } = &reduce(ReduceMode::ReplaceNonNumber(10.0), std::slice::from_ref(&s))[0]
    else {
        panic!("number");
    };
    assert_eq!(*value, Some(23.0));
    assert_eq!(name, "R");
    assert_eq!(labels.get("h").map(String::as_str), Some("a"));
    // Strict: Inf plus numbers is Inf, a null makes NaN.
    let ExprValue::Number { value, .. } = &reduce(ReduceMode::Strict, &[s])[0] else {
        panic!("number");
    };
    assert!(is_nan(*value));
    // dropNN on an all-null series: mean of nothing is NaN, which dropNN answers as null.
    let all_null = series(&[(1, None)]);
    let out = reduce_frames(
        "R",
        "A",
        ReducerId::Mean,
        ReduceMode::DropNonNumber,
        std::slice::from_ref(&all_null),
    )
    .unwrap();
    assert!(matches!(out[0], ExprValue::Number { value: None, .. }));
    let out = reduce_frames(
        "R",
        "A",
        ReducerId::Mean,
        ReduceMode::ReplaceNonNumber(0.0),
        &[all_null],
    )
    .unwrap();
    assert!(matches!(out[0], ExprValue::Number { value: Some(v), .. } if v == 0.0));
}

#[test]
fn every_number_input_is_kept_with_a_not_needed_notice() {
    // commands.go: a Number passes through MapOutput and every one gets the warning.
    let notice =
        "Reduce operation is not needed. Input query or expression A is already reduced data.";
    for mode in [
        ReduceMode::Strict,
        ReduceMode::DropNonNumber,
        ReduceMode::ReplaceNonNumber(0.0),
    ] {
        let out = reduce(mode, &[number(Some(1.0)), number(Some(2.0))]);
        assert_eq!(out.len(), 2, "{mode:?}");
        for (v, want) in out.iter().zip([1.0, 2.0]) {
            let ExprValue::Number {
                name,
                value,
                notices,
                labels,
            } = v
            else {
                panic!("number");
            };
            assert_eq!(name, "R");
            assert_eq!(*value, Some(want));
            assert_eq!(labels.get("h").map(String::as_str), Some("n"));
            assert_eq!(notices.len(), 1, "{mode:?}");
            assert_eq!(notices[0].text, notice);
        }
    }
    // MapOutput keeps an infinite number under dropNN and drops a null one.
    let out = reduce(
        ReduceMode::DropNonNumber,
        &[
            number(Some(f64::INFINITY)),
            number(None),
            number(Some(f64::NAN)),
        ],
    );
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], ExprValue::Number { value: Some(v), .. } if v == f64::INFINITY));
    // replaceNN replaces a NaN number.
    let out = reduce(ReduceMode::ReplaceNonNumber(7.0), &[number(Some(f64::NAN))]);
    assert!(matches!(out[0], ExprValue::Number { value: Some(v), .. } if v == 7.0));
    // Strict keeps a null number.
    let out = reduce(ReduceMode::Strict, &[number(None)]);
    assert!(matches!(out[0], ExprValue::Number { value: None, .. }));
}

#[test]
fn scalars_become_numbers_and_no_data_stays() {
    let out = reduce(
        ReduceMode::Strict,
        &[ExprValue::Scalar(Some(4.0)), ExprValue::NoData],
    );
    assert_eq!(
        out,
        [
            ExprValue::Number {
                name: "R".into(),
                labels: BTreeMap::new(),
                value: Some(4.0),
                notices: vec![],
            },
            ExprValue::NoData
        ]
    );
}
