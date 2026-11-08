#![forbid(unsafe_code)]
//! Resample expression (`GF pkg/expr/mathexp/resample.go`).

use crate::math::ExprValue;
use crate::reduce::{ReducerId, reduce_slice};
use daiya_model::{CrError, CrResult};
use serde::{Deserialize, Serialize};

pub const MAX_NEW_SERIES_LENGTH: usize = 1_000_000;

/// Upsampling fill rule (`resample.go:13-22`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Upsampler {
    Pad,
    #[serde(rename = "backfilling")]
    Backfill,
    #[serde(rename = "fillna")]
    FillNa,
}

impl Upsampler {
    pub fn parse(s: &str) -> CrResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pad" => Ok(Self::Pad),
            "backfilling" | "backfill" => Ok(Self::Backfill),
            "fillna" => Ok(Self::FillNa),
            other => Err(CrError::ExpressionError(format!(
                "upsampling '{other}' not implemented"
            ))),
        }
    }
}

/// Resamples a slice of [`ExprValue`]s over `[from_ms, to_ms]` at `interval_ms` (`resample.go:34-108`).
pub fn resample_frames(
    ref_id: &str,
    interval_ms: i64,
    downsampler: ReducerId,
    upsampler: Upsampler,
    from_ms: i64,
    to_ms: i64,
    inputs: &[ExprValue],
) -> CrResult<Vec<ExprValue>> {
    if interval_ms <= 0 {
        return Err(CrError::ExpressionError(
            "resample window interval must be positive".into(),
        ));
    }
    let span = to_ms - from_ms;
    let new_series_len = (span as f64 / interval_ms as f64) as isize;
    if new_series_len <= 0 {
        return Err(CrError::ExpressionError(
            "the series cannot be sampled further; the time range is shorter than the interval"
                .into(),
        ));
    }
    let new_len = new_series_len as usize;
    if new_len > MAX_NEW_SERIES_LENGTH {
        return Err(CrError::ExpressionError(format!(
            "Resample series length too large, max allowed {MAX_NEW_SERIES_LENGTH}, wanted {new_len}"
        )));
    }

    if !matches!(
        downsampler,
        ReducerId::Sum | ReducerId::Mean | ReducerId::Min | ReducerId::Max | ReducerId::Last
    ) {
        return Err(CrError::ExpressionError(format!(
            "downsampling '{downsampler:?}' not implemented"
        )));
    }

    let mut out = Vec::with_capacity(inputs.len());
    for val in inputs {
        match val {
            ExprValue::Series {
                labels,
                points,
                notices,
                ..
            } => {
                let mut resampled = Vec::with_capacity(new_len + 1);
                let mut bookmark = 0usize;
                let mut last_seen: Option<f64> = None;
                let mut idx = 0usize;
                let mut t = from_ms;

                while t <= to_ms && idx <= new_len {
                    let mut vals = Vec::new();
                    let mut s_idx = bookmark;
                    while s_idx < points.len() {
                        let (st, v) = points[s_idx];
                        if st > t {
                            break;
                        }
                        bookmark += 1;
                        s_idx += 1;
                        last_seen = v;
                        vals.push(v);
                    }
                    let value = if vals.is_empty() {
                        match upsampler {
                            Upsampler::Pad => last_seen,
                            Upsampler::Backfill => {
                                if s_idx >= points.len() {
                                    None
                                } else {
                                    points[s_idx].1
                                }
                            }
                            Upsampler::FillNa => None,
                        }
                    } else if vals.len() == 1 {
                        vals[0]
                    } else {
                        reduce_slice(downsampler, &vals)
                    };
                    resampled.push((t, value));
                    t += interval_ms;
                    idx += 1;
                }

                out.push(ExprValue::Series {
                    name: ref_id.to_owned(),
                    labels: labels.clone(),
                    points: resampled,
                    notices: notices.clone(),
                });
            }
            ExprValue::NoData => {
                out.push(ExprValue::NoData);
                return Ok(out);
            }
            other => {
                return Err(CrError::ExpressionError(format!(
                    "can only resample type series, got {other:?}"
                )));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn series(points: &[(i64, Option<f64>)]) -> ExprValue {
        ExprValue::Series {
            name: "A".into(),
            labels: BTreeMap::from([("host".into(), "a".into())]),
            points: points.to_vec(),
            notices: Vec::new(),
        }
    }

    fn points_of(v: &ExprValue) -> Vec<(i64, Option<f64>)> {
        match v {
            ExprValue::Series { points, .. } => points.clone(),
            other => panic!("expected a series, got {other:?}"),
        }
    }

    const S: i64 = 1_000;

    // The expected values are the cases of Grafana v12.4.14 `mathexp/resample_test.go`.
    #[test]
    fn downsampling_matches_grafana_cases() {
        let input = series(&[
            (2 * S, Some(2.0)),
            (4 * S, Some(3.0)),
            (7 * S, Some(1.0)),
            (9 * S, Some(2.0)),
        ]);
        let run = |down, up| {
            let out = resample_frames(
                "B",
                5 * S,
                down,
                up,
                0,
                16 * S,
                std::slice::from_ref(&input),
            )
            .unwrap();
            assert_eq!(out.len(), 1);
            points_of(&out[0])
        };
        let t = [0, 5 * S, 10 * S, 15 * S];
        let zip = |v: [Option<f64>; 4]| t.iter().copied().zip(v).collect::<Vec<_>>();
        assert_eq!(
            run(ReducerId::Mean, Upsampler::Pad),
            zip([None, Some(2.5), Some(1.5), Some(2.0)])
        );
        assert_eq!(
            run(ReducerId::Max, Upsampler::FillNa),
            zip([None, Some(3.0), Some(2.0), None])
        );
        assert_eq!(
            run(ReducerId::Min, Upsampler::FillNa),
            zip([None, Some(2.0), Some(1.0), None])
        );
        assert_eq!(
            run(ReducerId::Sum, Upsampler::FillNa),
            zip([None, Some(5.0), Some(3.0), None])
        );
    }

    #[test]
    fn upsampling_backfill_and_last_pad_match_grafana_cases() {
        let input = series(&[(2 * S, Some(2.0)), (7 * S, Some(1.0))]);
        let out = resample_frames(
            "B",
            2 * S,
            ReducerId::Mean,
            Upsampler::parse("backfilling").unwrap(),
            0,
            11 * S,
            &[input],
        )
        .unwrap();
        assert_eq!(
            points_of(&out[0]),
            vec![
                (0, Some(2.0)),
                (2 * S, Some(2.0)),
                (4 * S, Some(1.0)),
                (6 * S, Some(1.0)),
                (8 * S, Some(1.0)),
                (10 * S, None),
            ]
        );

        let input = series(&[
            (0, Some(0.0)),
            (2 * S, Some(2.0)),
            (4 * S, Some(3.0)),
            (6 * S, Some(4.0)),
            (8 * S, Some(0.0)),
            (10 * S, Some(1.0)),
        ]);
        let out = resample_frames(
            "B",
            3 * S,
            ReducerId::Last,
            Upsampler::Pad,
            0,
            11 * S,
            &[input],
        )
        .unwrap();
        assert_eq!(
            points_of(&out[0]),
            vec![
                (0, Some(0.0)),
                (3 * S, Some(2.0)),
                (6 * S, Some(4.0)),
                (9 * S, Some(0.0)),
            ]
        );
        match &out[0] {
            ExprValue::Series { name, labels, .. } => {
                assert_eq!(name, "B");
                assert_eq!(labels.get("host").map(String::as_str), Some("a"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn refuses_bad_ranges_lengths_reducers_and_non_series() {
        let input = series(&[(2 * S, Some(2.0)), (7 * S, Some(1.0))]);
        let one = std::slice::from_ref(&input);
        // Range shorter than the interval, and an inverted range (Grafana cases 1 and 2).
        assert!(
            resample_frames(
                "B",
                5 * S,
                ReducerId::Mean,
                Upsampler::FillNa,
                0,
                4 * S,
                one
            )
            .is_err()
        );
        assert!(
            resample_frames(
                "B",
                5 * S,
                ReducerId::Mean,
                Upsampler::FillNa,
                11 * S,
                0,
                one
            )
            .is_err()
        );
        assert!(
            resample_frames("B", 0, ReducerId::Mean, Upsampler::FillNa, 0, 4 * S, one).is_err()
        );
        // Result too big (Grafana case "upsampling, result too big").
        let err = resample_frames("B", 1, ReducerId::Mean, Upsampler::Pad, 0, 11_000_000, one)
            .unwrap_err();
        assert!(err.to_string().contains("11000000"), "{err}");
        // Downsamplers Grafana does not implement for resample.
        for r in [ReducerId::Count, ReducerId::Median] {
            assert!(resample_frames("B", S, r, Upsampler::Pad, 0, 4 * S, one).is_err());
        }
        assert!(Upsampler::parse("linear").is_err());
        assert_eq!(Upsampler::parse(" PAD ").unwrap(), Upsampler::Pad);
        assert_eq!(Upsampler::parse("fillna").unwrap(), Upsampler::FillNa);
        let number = ExprValue::Number {
            name: "A".into(),
            labels: BTreeMap::new(),
            value: Some(1.0),
            notices: Vec::new(),
        };
        assert!(
            resample_frames("B", S, ReducerId::Mean, Upsampler::Pad, 0, 4 * S, &[number]).is_err()
        );
        let out = resample_frames(
            "B",
            S,
            ReducerId::Mean,
            Upsampler::Pad,
            0,
            4 * S,
            &[ExprValue::NoData],
        )
        .unwrap();
        assert_eq!(out, vec![ExprValue::NoData]);
    }
}
