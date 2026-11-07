#![forbid(unsafe_code)]
//! Threshold and hysteresis expressions (`GF pkg/expr/threshold.go`, `hysteresis.go`).

use crate::math::ExprValue;
use controlroom_model::{CrError, CrResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Supported threshold predicates (`threshold.go:31-42`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdType {
    Gt,
    Lt,
    Eq,
    Ne,
    Gte,
    Lte,
    WithinRange,
    OutsideRange,
    WithinRangeIncluded,
    OutsideRangeIncluded,
}

impl ThresholdType {
    pub fn parse(s: &str) -> CrResult<Self> {
        match s {
            "gt" => Ok(Self::Gt),
            "lt" => Ok(Self::Lt),
            "eq" => Ok(Self::Eq),
            "ne" => Ok(Self::Ne),
            "gte" => Ok(Self::Gte),
            "lte" => Ok(Self::Lte),
            "within_range" => Ok(Self::WithinRange),
            "outside_range" => Ok(Self::OutsideRange),
            "within_range_included" => Ok(Self::WithinRangeIncluded),
            "outside_range_included" => Ok(Self::OutsideRangeIncluded),
            other => Err(CrError::ExpressionError(format!(
                "unsupported threshold function '{other}'"
            ))),
        }
    }

    pub fn eval(self, f: f64, params: &[f64]) -> CrResult<bool> {
        match self {
            Self::Gt => {
                let t = *params.first().ok_or_else(|| {
                    CrError::ExpressionError("threshold 'gt' requires 1 parameter".into())
                })?;
                Ok(f > t)
            }
            Self::Lt => {
                let t = *params.first().ok_or_else(|| {
                    CrError::ExpressionError("threshold 'lt' requires 1 parameter".into())
                })?;
                Ok(f < t)
            }
            Self::Eq => {
                let t = *params.first().ok_or_else(|| {
                    CrError::ExpressionError("threshold 'eq' requires 1 parameter".into())
                })?;
                Ok((f - t).abs() == 0.0)
            }
            Self::Ne => {
                let t = *params.first().ok_or_else(|| {
                    CrError::ExpressionError("threshold 'ne' requires 1 parameter".into())
                })?;
                Ok((f - t).abs() != 0.0)
            }
            Self::Gte => {
                let t = *params.first().ok_or_else(|| {
                    CrError::ExpressionError("threshold 'gte' requires 1 parameter".into())
                })?;
                Ok(f >= t)
            }
            Self::Lte => {
                let t = *params.first().ok_or_else(|| {
                    CrError::ExpressionError("threshold 'lte' requires 1 parameter".into())
                })?;
                Ok(f <= t)
            }
            Self::WithinRange => {
                if params.len() < 2 {
                    return Err(CrError::ExpressionError(
                        "threshold 'within_range' requires 2 parameters".into(),
                    ));
                }
                Ok(f > params[0] && f < params[1])
            }
            Self::OutsideRange => {
                if params.len() < 2 {
                    return Err(CrError::ExpressionError(
                        "threshold 'outside_range' requires 2 parameters".into(),
                    ));
                }
                Ok(f < params[0] || f > params[1])
            }
            Self::WithinRangeIncluded => {
                if params.len() < 2 {
                    return Err(CrError::ExpressionError(
                        "threshold 'within_range_included' requires 2 parameters".into(),
                    ));
                }
                Ok(f >= params[0] && f <= params[1])
            }
            Self::OutsideRangeIncluded => {
                if params.len() < 2 {
                    return Err(CrError::ExpressionError(
                        "threshold 'outside_range_included' requires 2 parameters".into(),
                    ));
                }
                Ok(f <= params[0] || f >= params[1])
            }
        }
    }
}

/// Threshold condition with optional hysteresis unloading evaluator (`threshold.go:ThresholdConditionJSON`).
#[derive(Debug, Clone)]
pub struct ThresholdCondition {
    pub evaluator_type: ThresholdType,
    pub params: Vec<f64>,
    pub unload_evaluator: Option<(ThresholdType, Vec<f64>)>,
    pub loaded_fingerprints: BTreeSet<String>,
}

fn eval_single_point(
    v: Option<f64>,
    ty: ThresholdType,
    params: &[f64],
    invert: bool,
) -> CrResult<Option<f64>> {
    let Some(f) = v else {
        return Ok(None);
    };
    let mut hit = ty.eval(f, params)?;
    if invert {
        hit = !hit;
    }
    Ok(Some(if hit { 1.0 } else { 0.0 }))
}

fn fingerprint_labels(labels: &std::collections::BTreeMap<String, String>) -> String {
    labels
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Evaluates a threshold (or hysteresis) command over `inputs` (`threshold.go:180-217`, `hysteresis.go:34-89`).
pub fn eval_threshold_frames(
    ref_id: &str,
    cond: &ThresholdCondition,
    inputs: &[ExprValue],
) -> CrResult<Vec<ExprValue>> {
    let mut out = Vec::with_capacity(inputs.len());
    for val in inputs {
        let fp = fingerprint_labels(val.labels());
        let is_loaded = cond.loaded_fingerprints.contains(&fp);
        let (ty, params, invert) = match (&cond.unload_evaluator, is_loaded) {
            (Some((uty, uparams)), true) => (*uty, uparams.as_slice(), true),
            _ => (cond.evaluator_type, cond.params.as_slice(), false),
        };

        match val {
            ExprValue::Series {
                labels,
                points,
                notices,
                ..
            } => {
                let mut new_pts = Vec::with_capacity(points.len());
                for (t, v) in points {
                    new_pts.push((*t, eval_single_point(*v, ty, params, invert)?));
                }
                out.push(ExprValue::Series {
                    name: ref_id.to_owned(),
                    labels: labels.clone(),
                    points: new_pts,
                    notices: notices.clone(),
                });
            }
            ExprValue::Number {
                labels,
                value,
                notices,
                ..
            } => {
                out.push(ExprValue::Number {
                    name: ref_id.to_owned(),
                    labels: labels.clone(),
                    value: eval_single_point(*value, ty, params, invert)?,
                    notices: notices.clone(),
                });
            }
            ExprValue::Scalar(v) => {
                out.push(ExprValue::Scalar(eval_single_point(
                    *v, ty, params, invert,
                )?));
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

    fn number(host: &str, v: Option<f64>) -> ExprValue {
        ExprValue::Number {
            name: "A".into(),
            labels: BTreeMap::from([("host".into(), host.into())]),
            value: v,
            notices: Vec::new(),
        }
    }

    fn value_of(v: &ExprValue) -> Option<f64> {
        match v {
            ExprValue::Number { value, .. } | ExprValue::Scalar(value) => *value,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn every_predicate_and_its_boundaries() {
        let cases: &[(&str, &[f64], f64, bool)] = &[
            ("gt", &[5.0], 5.0, false),
            ("gt", &[5.0], 5.1, true),
            ("lt", &[5.0], 4.9, true),
            ("lt", &[5.0], 5.0, false),
            ("eq", &[5.0], 5.0, true),
            ("eq", &[5.0], 5.1, false),
            ("ne", &[5.0], 5.0, false),
            ("ne", &[5.0], 4.0, true),
            ("gte", &[5.0], 5.0, true),
            ("gte", &[5.0], 4.9, false),
            ("lte", &[5.0], 5.0, true),
            ("lte", &[5.0], 5.1, false),
            ("within_range", &[1.0, 3.0], 1.0, false),
            ("within_range", &[1.0, 3.0], 2.0, true),
            ("outside_range", &[1.0, 3.0], 3.0, false),
            ("outside_range", &[1.0, 3.0], 3.5, true),
            ("within_range_included", &[1.0, 3.0], 1.0, true),
            ("within_range_included", &[1.0, 3.0], 3.5, false),
            ("outside_range_included", &[1.0, 3.0], 3.0, true),
            ("outside_range_included", &[1.0, 3.0], 2.0, false),
        ];
        for (name, params, value, want) in cases {
            let ty = ThresholdType::parse(name).unwrap();
            assert_eq!(
                ty.eval(*value, params).unwrap(),
                *want,
                "{name} {params:?} {value}"
            );
            // Missing parameters are an error, never a silent false.
            assert!(
                ty.eval(*value, &params[..params.len() - 1]).is_err(),
                "{name}"
            );
        }
        assert!(ThresholdType::parse("between").is_err());
    }

    #[test]
    fn threshold_maps_series_numbers_scalars_and_keeps_no_data() {
        let cond = ThresholdCondition {
            evaluator_type: ThresholdType::Gt,
            params: vec![10.0],
            unload_evaluator: None,
            loaded_fingerprints: BTreeSet::new(),
        };
        let series = ExprValue::Series {
            name: "A".into(),
            labels: BTreeMap::new(),
            points: vec![(0, Some(5.0)), (1, None), (2, Some(11.0))],
            notices: Vec::new(),
        };
        let out = eval_threshold_frames(
            "B",
            &cond,
            &[
                series,
                number("a", Some(12.0)),
                ExprValue::Scalar(Some(1.0)),
                ExprValue::NoData,
            ],
        )
        .unwrap();
        assert_eq!(
            out[0],
            ExprValue::Series {
                name: "B".into(),
                labels: BTreeMap::new(),
                points: vec![(0, Some(0.0)), (1, None), (2, Some(1.0))],
                notices: Vec::new(),
            }
        );
        assert_eq!(value_of(&out[1]), Some(1.0));
        assert_eq!(value_of(&out[2]), Some(0.0));
        assert_eq!(out[3], ExprValue::NoData);
    }

    // Grafana `hysteresis.go`: a loaded dimension is judged by the unloading evaluator,
    // inverted (`threshold.go:158`), so it stays firing until it drops below the unload level.
    #[test]
    fn hysteresis_uses_the_inverted_unload_evaluator_for_loaded_dimensions() {
        let cond = ThresholdCondition {
            evaluator_type: ThresholdType::Gt,
            params: vec![10.0],
            unload_evaluator: Some((ThresholdType::Lt, vec![5.0])),
            loaded_fingerprints: BTreeSet::from(["host=loaded".to_owned()]),
        };
        let out = eval_threshold_frames(
            "B",
            &cond,
            &[
                number("loaded", Some(7.0)),
                number("loaded", Some(4.0)),
                number("fresh", Some(7.0)),
                number("fresh", Some(11.0)),
            ],
        )
        .unwrap();
        let got: Vec<_> = out.iter().map(value_of).collect();
        assert_eq!(got, vec![Some(1.0), Some(0.0), Some(0.0), Some(1.0)]);
    }
}
