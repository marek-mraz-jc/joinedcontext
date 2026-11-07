#![forbid(unsafe_code)]
//! Resample expression (`GF pkg/expr/mathexp/resample.go`).

use crate::math::ExprValue;
use crate::reduce::{ReducerId, reduce_slice};
use controlroom_model::{CrError, CrResult};
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
