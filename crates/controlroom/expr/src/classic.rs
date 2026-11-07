#![forbid(unsafe_code)]
//! Classic conditions expression (`GF pkg/expr/classic/classic.go`, `evaluator.go`, `reduce.go`).

use crate::math::ExprValue;
use controlroom_model::{CrError, CrResult};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

/// Single condition inside a `classic_conditions` command (`classic.go:ConditionJSON`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassicCondition {
    pub input_ref_id: String,
    pub reducer: String,
    pub evaluator_type: String,
    pub evaluator_params: Vec<f64>,
    /// `"and"`, `"or"`, or `"logic-or"`.
    pub operator: String,
}

fn nil_or_nan(v: Option<f64>) -> bool {
    v.is_none_or(f64::is_nan)
}

/// Reduces a series using classic reducer semantics (`GF pkg/expr/classic/reduce.go`).
pub fn classic_reduce(reducer: &str, points: &[(i64, Option<f64>)]) -> CrResult<Option<f64>> {
    if points.is_empty() {
        return Ok(None);
    }
    let vals: Vec<Option<f64>> = points.iter().map(|(_, v)| *v).collect();
    let mut all_null = true;
    let mut value = 0.0;

    match reducer {
        "avg" | "mean" => {
            let mut valid_count = 0usize;
            for f in &vals {
                if let Some(x) = f
                    && !x.is_nan()
                {
                    value += *x;
                    valid_count += 1;
                    all_null = false;
                }
            }
            if valid_count > 0 {
                value /= valid_count as f64;
            }
        }
        "sum" => {
            for f in &vals {
                if let Some(x) = f
                    && !x.is_nan()
                {
                    value += *x;
                    all_null = false;
                }
            }
        }
        "min" => {
            value = f64::MAX;
            for f in &vals {
                if let Some(x) = f
                    && !x.is_nan()
                {
                    all_null = false;
                    if *x < value {
                        value = *x;
                    }
                }
            }
        }
        "max" => {
            value = f64::MIN;
            for f in &vals {
                if let Some(x) = f
                    && !x.is_nan()
                {
                    all_null = false;
                    if *x > value {
                        value = *x;
                    }
                }
            }
        }
        "count" => {
            value = vals.len() as f64;
            all_null = false;
        }
        "last" => {
            for f in vals.iter().rev() {
                if let Some(x) = f
                    && !x.is_nan()
                {
                    value = *x;
                    all_null = false;
                    break;
                }
            }
        }
        "median" => {
            let mut valid: Vec<f64> = vals
                .iter()
                .filter_map(|v| v.filter(|x| !x.is_nan()))
                .collect();
            if !valid.is_empty() {
                all_null = false;
                valid.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
                let len = valid.len();
                value = if len % 2 == 1 {
                    valid[(len - 1) / 2]
                } else {
                    (valid[(len / 2) - 1] + valid[len / 2]) / 2.0
                };
            }
        }
        "diff" | "diff_abs" | "percent_diff" | "percent_diff_abs" => {
            let mut newest: Option<(usize, f64)> = None;
            for (i, f) in vals.iter().enumerate().rev() {
                if !nil_or_nan(*f) {
                    newest = Some((i, f.unwrap_or(0.0)));
                    all_null = false;
                    break;
                }
            }
            if let Some((newest_idx, newest_val)) = newest
                && newest_idx >= 1
            {
                for f in &vals {
                    if !nil_or_nan(*f) {
                        let oldest_val = f.unwrap_or(0.0);
                        value = match reducer {
                            "diff" => newest_val - oldest_val,
                            "diff_abs" => (newest_val - oldest_val).abs(),
                            "percent_diff" => (newest_val - oldest_val) / oldest_val.abs() * 100.0,
                            _ => ((newest_val - oldest_val) / oldest_val * 100.0).abs(),
                        };
                        break;
                    }
                }
            }
        }
        "count_non_null" => {
            for f in &vals {
                if !nil_or_nan(*f) {
                    value += 1.0;
                }
            }
            if value > 0.0 {
                all_null = false;
            }
        }
        other => {
            return Err(CrError::ExpressionError(format!(
                "invalid classic reducer '{other}'"
            )));
        }
    }

    if all_null { Ok(None) } else { Ok(Some(value)) }
}

fn eval_classic_value(eval_type: &str, params: &[f64], val: Option<f64>) -> CrResult<bool> {
    if eval_type == "no_value" {
        return Ok(val.is_none());
    }
    let Some(fv) = val else {
        return Ok(false);
    };
    match eval_type {
        "gt" | "lt" | "eq" | "ne" | "gte" | "lte" => {
            let t = *params.first().ok_or_else(|| {
                CrError::ExpressionError(format!(
                    "evaluator '{eval_type}' is missing threshold parameter"
                ))
            })?;
            Ok(match eval_type {
                "gt" => fv > t,
                "lt" => fv < t,
                "eq" => (fv - t).abs() == 0.0,
                "ne" => (fv - t).abs() != 0.0,
                "gte" => fv >= t,
                _ => fv <= t,
            })
        }
        "within_range" | "outside_range" | "within_range_included" | "outside_range_included" => {
            if params.len() != 2 {
                return Err(CrError::ExpressionError(
                    "ranged evaluator requires 2 parameters".into(),
                ));
            }
            let lower = params[0];
            let upper = params[1];
            Ok(match eval_type {
                "within_range" => (lower < fv && upper > fv) || (upper < fv && lower > fv),
                "outside_range" => (upper < fv && lower < fv) || (upper > fv && lower > fv),
                "within_range_included" => {
                    (lower <= fv && upper >= fv) || (upper <= fv && lower >= fv)
                }
                _ => (upper <= fv && lower <= fv) || (upper >= fv && lower >= fv),
            })
        }
        other => Err(CrError::ExpressionError(format!(
            "invalid evaluator type: {other}"
        ))),
    }
}

/// Evaluates a `classic_conditions` command (`classic.go:Execute`).
pub fn eval_classic_conditions(
    ref_id: &str,
    conditions: &[ClassicCondition],
    vars: &HashMap<String, Vec<ExprValue>>,
) -> CrResult<Vec<ExprValue>> {
    let mut is_firing = false;
    let mut is_no_data = false;

    for (i, cond) in conditions.iter().enumerate() {
        if is_firing && cond.operator == "logic-or" {
            break;
        }
        let data = vars.get(&cond.input_ref_id).map_or(&[][..], Vec::as_slice);
        let (cond_firing, cond_no_data) = if data.is_empty() {
            if cond.evaluator_type == "no_value" {
                (true, false)
            } else {
                (false, true)
            }
        } else {
            let mut f_any = false;
            let mut num_no_data = 0usize;
            for v in data {
                let reduced = match v {
                    ExprValue::NoData => None,
                    ExprValue::Scalar(x) => *x,
                    ExprValue::Number { value, .. } => *value,
                    ExprValue::Series { points, .. } => classic_reduce(&cond.reducer, points)?,
                };
                if eval_classic_value(&cond.evaluator_type, &cond.evaluator_params, reduced)? {
                    f_any = true;
                } else if reduced.is_none() {
                    num_no_data += 1;
                }
            }
            (f_any, num_no_data == data.len())
        };

        if i == 0 {
            is_firing = cond_firing;
            is_no_data = cond_no_data;
        } else if cond.operator == "or" || cond.operator == "logic-or" {
            is_firing = is_firing || cond_firing;
            is_no_data = is_no_data || cond_no_data;
        } else {
            is_firing = is_firing && cond_firing;
            is_no_data = is_no_data && cond_no_data;
        }
    }

    let val = if is_no_data {
        None
    } else if is_firing {
        Some(1.0)
    } else {
        Some(0.0)
    };

    Ok(vec![ExprValue::Number {
        name: ref_id.to_owned(),
        labels: BTreeMap::new(),
        value: val,
        notices: Vec::new(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(vals: &[Option<f64>]) -> Vec<(i64, Option<f64>)> {
        vals.iter()
            .enumerate()
            .map(|(i, v)| (i as i64, *v))
            .collect()
    }

    // Reducer cases from Grafana v12.4.14 `classic/reduce_test.go`.
    #[test]
    fn reducers_match_grafana() {
        let p = pts(&[Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
        let r = |name: &str, p: &[(i64, Option<f64>)]| classic_reduce(name, p).unwrap();
        assert_eq!(r("sum", &p), Some(10.0));
        assert_eq!(r("avg", &p), Some(2.5));
        assert_eq!(r("mean", &p), Some(2.5));
        assert_eq!(r("min", &p), Some(1.0));
        assert_eq!(r("max", &p), Some(4.0));
        assert_eq!(r("count", &p), Some(4.0));
        assert_eq!(r("last", &p), Some(4.0));
        assert_eq!(r("median", &p), Some(2.5));
        assert_eq!(
            r("median", &pts(&[Some(1.0), Some(5.0), Some(2.0)])),
            Some(2.0)
        );
        assert_eq!(
            r("count_non_null", &pts(&[Some(1.0), None, Some(f64::NAN)])),
            Some(1.0)
        );

        // Nulls and NaN are skipped; all-null gives no value except for count.
        let gappy = pts(&[None, Some(3.0), Some(f64::NAN), Some(1.0), None]);
        assert_eq!(r("avg", &gappy), Some(2.0));
        assert_eq!(r("last", &gappy), Some(1.0));
        assert_eq!(r("min", &gappy), Some(1.0));
        let nulls = pts(&[None, None]);
        for name in [
            "avg",
            "sum",
            "min",
            "max",
            "last",
            "median",
            "diff",
            "count_non_null",
        ] {
            assert_eq!(r(name, &nulls), None, "{name}");
        }
        assert_eq!(r("count", &nulls), Some(2.0));
        assert_eq!(r("sum", &[]), None);
        assert!(classic_reduce("p95", &p).is_err());
    }

    #[test]
    fn diff_reducers_compare_newest_with_oldest_non_null() {
        let p = pts(&[None, Some(30.0), Some(40.0), Some(15.0), None]);
        let r = |name: &str| classic_reduce(name, &p).unwrap();
        assert_eq!(r("diff"), Some(-15.0));
        assert_eq!(r("diff_abs"), Some(15.0));
        assert_eq!(r("percent_diff"), Some(-50.0));
        assert_eq!(r("percent_diff_abs"), Some(50.0));
        // A single point has nothing to compare with: 0, not null (Grafana `calculateDiff`).
        assert_eq!(
            classic_reduce("diff", &pts(&[Some(5.0)])).unwrap(),
            Some(0.0)
        );
    }

    fn cond(reducer: &str, ty: &str, params: &[f64], op: &str, input: &str) -> ClassicCondition {
        ClassicCondition {
            input_ref_id: input.into(),
            reducer: reducer.into(),
            evaluator_type: ty.into(),
            evaluator_params: params.to_vec(),
            operator: op.into(),
        }
    }

    fn series(vals: &[Option<f64>]) -> ExprValue {
        ExprValue::Series {
            name: "A".into(),
            labels: BTreeMap::new(),
            points: pts(vals),
            notices: Vec::new(),
        }
    }

    fn fire(conds: &[ClassicCondition], vars: &HashMap<String, Vec<ExprValue>>) -> Option<f64> {
        let out = eval_classic_conditions("C", conds, vars).unwrap();
        match &out[0] {
            ExprValue::Number { name, value, .. } => {
                assert_eq!(name, "C");
                *value
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn conditions_combine_with_and_or_and_report_no_data() {
        let vars = HashMap::from([
            ("A".to_owned(), vec![series(&[Some(1.0), Some(9.0)])]),
            ("B".to_owned(), vec![series(&[Some(1.0), Some(2.0)])]),
            ("N".to_owned(), vec![series(&[None, None])]),
        ]);
        assert_eq!(
            fire(&[cond("last", "gt", &[5.0], "and", "A")], &vars),
            Some(1.0)
        );
        assert_eq!(
            fire(
                &[
                    cond("last", "gt", &[5.0], "and", "A"),
                    cond("max", "gt", &[5.0], "and", "B")
                ],
                &vars
            ),
            Some(0.0)
        );
        assert_eq!(
            fire(
                &[
                    cond("last", "gt", &[5.0], "and", "B"),
                    cond("max", "gt", &[5.0], "or", "A")
                ],
                &vars
            ),
            Some(1.0)
        );
        // logic-or stops at the first firing condition, even before an invalid one.
        assert_eq!(
            fire(
                &[
                    cond("last", "gt", &[5.0], "and", "A"),
                    cond("last", "bogus", &[], "logic-or", "B")
                ],
                &vars
            ),
            Some(1.0)
        );
        // All values null: no data, unless the evaluator asks for no_value.
        assert_eq!(fire(&[cond("avg", "gt", &[0.0], "and", "N")], &vars), None);
        assert_eq!(
            fire(&[cond("avg", "no_value", &[], "and", "N")], &vars),
            Some(1.0)
        );
        assert_eq!(
            fire(&[cond("avg", "no_value", &[], "and", "missing")], &vars),
            Some(1.0)
        );
        assert_eq!(
            fire(&[cond("avg", "gt", &[0.0], "and", "missing")], &vars),
            None
        );
    }

    #[test]
    fn evaluators_ranges_scalars_and_errors() {
        let v = |x: f64| HashMap::from([("A".to_owned(), vec![ExprValue::Scalar(Some(x))])]);
        let one =
            |ty: &str, params: &[f64], x: f64| fire(&[cond("last", ty, params, "and", "A")], &v(x));
        assert_eq!(one("within_range", &[1.0, 3.0], 2.0), Some(1.0));
        // Grafana accepts the bounds in either order.
        assert_eq!(one("within_range", &[3.0, 1.0], 2.0), Some(1.0));
        assert_eq!(one("within_range", &[1.0, 3.0], 3.0), Some(0.0));
        assert_eq!(one("within_range_included", &[1.0, 3.0], 3.0), Some(1.0));
        assert_eq!(one("outside_range", &[1.0, 3.0], 4.0), Some(1.0));
        assert_eq!(one("outside_range", &[1.0, 3.0], 3.0), Some(0.0));
        assert_eq!(one("outside_range_included", &[1.0, 3.0], 3.0), Some(1.0));
        for (ty, x, want) in [
            ("lt", 1.0, 1.0),
            ("eq", 2.0, 1.0),
            ("ne", 2.0, 0.0),
            ("gte", 2.0, 1.0),
            ("lte", 3.0, 0.0),
        ] {
            assert_eq!(one(ty, &[2.0], x), Some(want), "{ty}");
        }
        let num = HashMap::from([(
            "A".to_owned(),
            vec![ExprValue::Number {
                name: "A".into(),
                labels: BTreeMap::new(),
                value: Some(7.0),
                notices: Vec::new(),
            }],
        )]);
        assert_eq!(
            fire(&[cond("last", "gt", &[5.0], "and", "A")], &num),
            Some(1.0)
        );

        for bad in [
            cond("last", "gt", &[], "and", "A"),
            cond("last", "within_range", &[1.0], "and", "A"),
            cond("last", "between", &[1.0], "and", "A"),
        ] {
            assert!(eval_classic_conditions("C", &[bad], &v(2.0)).is_err());
        }
        let s = HashMap::from([("A".to_owned(), vec![series(&[Some(1.0)])])]);
        assert!(
            eval_classic_conditions("C", &[cond("p99", "gt", &[0.0], "and", "A")], &s).is_err()
        );
    }
}
