#![forbid(unsafe_code)]
//! Reduce expression (`GF pkg/expr/mathexp/reduce.go`, `commands.go:ReduceCommand`).
//!
//! Reducers: `sum`, `mean` (and alias `avg`), `min`, `max`, `count`, `last`, `median`.
//! Modes: `""` (strict), `"dropNN"`, `"replaceNN"` (with `replaceWithValue`).

use crate::math::ExprValue;
use daiya_model::{CrError, CrResult, Notice, NoticeSeverity};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Supported reducer IDs (`reduce.go:18-28`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReducerId {
    Sum,
    Mean,
    Min,
    Max,
    Count,
    Last,
    Median,
}

impl ReducerId {
    pub fn parse(s: &str) -> CrResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sum" => Ok(Self::Sum),
            "mean" | "avg" => Ok(Self::Mean),
            "min" => Ok(Self::Min),
            "max" => Ok(Self::Max),
            "count" => Ok(Self::Count),
            "last" | "lastnotnull" => Ok(Self::Last),
            "median" => Ok(Self::Median),
            other => Err(CrError::ExpressionError(format!(
                "reduction '{other}' not implemented"
            ))),
        }
    }
}

/// Non-number handling mode (`query.go:ReduceMode`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ReduceMode {
    Strict,
    DropNonNumber,
    ReplaceNonNumber(f64),
}

/// Mapper for non-number points (`reduce.go:ReduceMapper`).
#[derive(Debug, Clone, Copy)]
pub struct ReduceMapper {
    pub mode: ReduceMode,
}

impl ReduceMapper {
    #[must_use]
    pub const fn new(mode: ReduceMode) -> Self {
        Self { mode }
    }

    #[must_use]
    pub fn map_input(&self, v: Option<f64>) -> Option<Option<f64>> {
        match self.mode {
            ReduceMode::Strict => Some(v),
            ReduceMode::DropNonNumber => match v {
                Some(x) if x.is_finite() => Some(Some(x)),
                _ => None,
            },
            ReduceMode::ReplaceNonNumber(rep) => match v {
                Some(x) if x.is_finite() => Some(Some(x)),
                _ => Some(Some(rep)),
            },
        }
    }

    #[must_use]
    pub fn map_output(&self, v: Option<f64>) -> Option<f64> {
        match self.mode {
            ReduceMode::Strict => v,
            ReduceMode::DropNonNumber => match v {
                Some(x) if x.is_nan() => None,
                other => other,
            },
            ReduceMode::ReplaceNonNumber(rep) => match v {
                Some(x) if x.is_nan() => Some(rep),
                other => other,
            },
        }
    }
}

/// Reduces a slice of `Option<f64>` according to `reducer` (`reduce.go:34-127`).
#[must_use]
pub fn reduce_slice(reducer: ReducerId, values: &[Option<f64>]) -> Option<f64> {
    match reducer {
        ReducerId::Sum => {
            let mut sum = 0.0;
            for v in values {
                match v {
                    Some(x) if !x.is_nan() => sum += *x,
                    _ => return Some(f64::NAN),
                }
            }
            Some(sum)
        }
        ReducerId::Mean => {
            if values.is_empty() {
                return Some(f64::NAN);
            }
            let sum = reduce_slice(ReducerId::Sum, values)?;
            Some(sum / (values.len() as f64))
        }
        ReducerId::Min => {
            if values.is_empty() {
                return Some(f64::NAN);
            }
            let mut min = 0.0;
            for (i, v) in values.iter().enumerate() {
                match v {
                    Some(x) if !x.is_nan() => {
                        if i == 0 || *x < min {
                            min = *x;
                        }
                    }
                    _ => return Some(f64::NAN),
                }
            }
            Some(min)
        }
        ReducerId::Max => {
            if values.is_empty() {
                return Some(f64::NAN);
            }
            let mut max = 0.0;
            for (i, v) in values.iter().enumerate() {
                match v {
                    Some(x) if !x.is_nan() => {
                        if i == 0 || *x > max {
                            max = *x;
                        }
                    }
                    _ => return Some(f64::NAN),
                }
            }
            Some(max)
        }
        ReducerId::Count => Some(values.len() as f64),
        ReducerId::Last => {
            if values.is_empty() {
                Some(f64::NAN)
            } else {
                *values.last().unwrap_or(&None)
            }
        }
        ReducerId::Median => {
            let mut nums = Vec::with_capacity(values.len());
            for v in values {
                match v {
                    Some(x) if !x.is_nan() => nums.push(*x),
                    _ => return Some(f64::NAN),
                }
            }
            if nums.is_empty() {
                return Some(f64::NAN);
            }
            nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            let mid = nums.len() / 2;
            if nums.len() % 2 == 0 {
                Some((nums[mid - 1] + nums[mid]) / 2.0)
            } else {
                Some(nums[mid])
            }
        }
    }
}

/// Evaluates a `reduce` command over a list of [`ExprValue`]s (`commands.go:175-219`).
pub fn reduce_frames(
    ref_id: &str,
    var_to_reduce: &str,
    reducer: ReducerId,
    mode: ReduceMode,
    inputs: &[ExprValue],
) -> CrResult<Vec<ExprValue>> {
    let mapper = ReduceMapper::new(mode);
    let mut out = Vec::with_capacity(inputs.len());

    for (idx, val) in inputs.iter().enumerate() {
        match val {
            ExprValue::Series {
                labels,
                points,
                notices,
                ..
            } => {
                let mut mapped_vals = Vec::with_capacity(points.len());
                for (_, v) in points {
                    if let Some(mv) = mapper.map_input(*v) {
                        mapped_vals.push(mv);
                    }
                }
                let raw = reduce_slice(reducer, &mapped_vals);
                let final_val = mapper.map_output(raw);
                out.push(ExprValue::Number {
                    name: ref_id.to_owned(),
                    labels: labels.clone(),
                    value: final_val,
                    notices: notices.clone(),
                });
            }
            ExprValue::Number {
                labels,
                value,
                notices,
                ..
            } => {
                let mapped = match mode {
                    ReduceMode::Strict => Some(*value),
                    _ => mapper.map_input(*value),
                };
                let Some(v) = mapped else {
                    continue;
                };
                let mut new_notices = notices.clone();
                if matches!(mode, ReduceMode::Strict) && idx == 0 {
                    new_notices.push(Notice {
                        severity: NoticeSeverity::Warning,
                        text: format!(
                            "Reduce operation is not needed. Input query or expression {var_to_reduce} is already reduced data."
                        ),
                    });
                }
                out.push(ExprValue::Number {
                    name: ref_id.to_owned(),
                    labels: labels.clone(),
                    value: v,
                    notices: new_notices,
                });
            }
            ExprValue::Scalar(v) => {
                out.push(ExprValue::Number {
                    name: ref_id.to_owned(),
                    labels: std::collections::BTreeMap::new(),
                    value: *v,
                    notices: Vec::new(),
                });
            }
            ExprValue::NoData => out.push(ExprValue::NoData),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn reduce_strict_drop_and_replace_modes_match_grafana() {
        let series = ExprValue::Series {
            name: "s".into(),
            labels: BTreeMap::new(),
            points: vec![
                (1000, Some(10.0)),
                (2000, None),
                (3000, Some(f64::NAN)),
                (4000, Some(30.0)),
            ],
            notices: Vec::new(),
        };

        // Strict mode: NaN/None yields NaN
        let strict = reduce_frames(
            "R",
            "A",
            ReducerId::Mean,
            ReduceMode::Strict,
            std::slice::from_ref(&series),
        )
        .unwrap();
        let ExprValue::Number { value, .. } = &strict[0] else {
            panic!("expected number");
        };
        assert!(value.unwrap().is_nan());

        // dropNN: drops None and NaN -> mean(10, 30) = 20
        let dropped = reduce_frames(
            "R",
            "A",
            ReducerId::Mean,
            ReduceMode::DropNonNumber,
            std::slice::from_ref(&series),
        )
        .unwrap();
        assert_eq!(
            dropped[0],
            ExprValue::Number {
                name: "R".into(),
                labels: BTreeMap::new(),
                value: Some(20.0),
                notices: Vec::new(),
            }
        );

        // replaceNN(0.0): mean(10, 0, 0, 30) = 10
        let replaced = reduce_frames(
            "R",
            "A",
            ReducerId::Mean,
            ReduceMode::ReplaceNonNumber(0.0),
            &[series],
        )
        .unwrap();
        assert_eq!(
            replaced[0],
            ExprValue::Number {
                name: "R".into(),
                labels: BTreeMap::new(),
                value: Some(10.0),
                notices: Vec::new(),
            }
        );
    }
}
