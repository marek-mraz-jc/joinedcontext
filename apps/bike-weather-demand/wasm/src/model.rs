//! Availability profile, least-squares weather sensitivity regression and
//! short-term forecast generation with uncertainty bands (T-3337).

use crate::series::{div_floor, format_rfc3339_utc, helsinki_hour_of_week, HourlyPoint};
use serde::Serialize;

/// Profile summary for one Helsinki hour of the week (0..=167).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSlot {
    pub slot: usize,
    pub mean: Option<f64>,
    pub count: usize,
}

/// Weather regression terms and sample count.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherEffect {
    pub per_degree: f64,
    pub rain: f64,
    pub hours: usize,
}

/// Estimated bike count for a single future hour.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimateHour {
    pub at: String,
    pub mean: f64,
    pub low: f64,
    pub high: f64,
}

/// Last observed weather held constant across short-term estimate.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherAssumption {
    pub temperature: f64,
    pub raining: bool,
}

/// Computes the 168-slot hour-of-week profile (mean and count per slot).
/// A slot with fewer than 2 points falls back to the mean of that hour of day across all days.
pub fn compute_profile(bikes: &[HourlyPoint]) -> Vec<ProfileSlot> {
    let mut slot_sums = [0.0; 168];
    let mut slot_counts = [0usize; 168];
    let mut hod_sums = [0.0; 24];
    let mut hod_counts = [0usize; 24];

    for pt in bikes {
        let slot = helsinki_hour_of_week(pt.utc_hour);
        slot_sums[slot] += pt.value;
        slot_counts[slot] += 1;

        let hod = slot % 24;
        hod_sums[hod] += pt.value;
        hod_counts[hod] += 1;
    }

    let mut hod_means = [None; 24];
    for h in 0..24 {
        if hod_counts[h] > 0 {
            hod_means[h] = Some(hod_sums[h] / hod_counts[h] as f64);
        }
    }

    let mut profile = Vec::with_capacity(168);
    for slot in 0..168 {
        let count = slot_counts[slot];
        let mean = if count >= 2 {
            Some(slot_sums[slot] / count as f64)
        } else {
            hod_means[slot % 24]
        };
        profile.push(ProfileSlot { slot, mean, count });
    }

    profile
}

/// Solves a 3x3 linear system `A * x = b` using Gaussian elimination with partial pivoting
/// and a relative singularity guard.
pub fn solve_3x3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    let max_val = a.iter().flatten().fold(0.0_f64, |m, v| m.max(v.abs()));
    if max_val < 1e-12 {
        return None;
    }
    let eps = 1e-8 * max_val;

    for k in 0..3 {
        let mut pivot_row = k;
        let mut max_pivot = a[k][k].abs();
        for (i, row) in a.iter().enumerate().skip(k + 1) {
            let val = row[k].abs();
            if val > max_pivot {
                max_pivot = val;
                pivot_row = i;
            }
        }
        if max_pivot < eps {
            return None;
        }
        if pivot_row != k {
            a.swap(k, pivot_row);
            b.swap(k, pivot_row);
        }

        let pivot = a[k][k];
        // A copy of the pivot row: the rows below are changed while it is read.
        let pivot_values = a[k];
        for i in (k + 1)..3 {
            let factor = a[i][k] / pivot;
            a[i][k] = 0.0;
            for (j, value) in a[i].iter_mut().enumerate().skip(k + 1) {
                *value -= factor * pivot_values[j];
            }
            b[i] -= factor * b[k];
        }
    }

    let mut x = [0.0; 3];
    for i in (0..3).rev() {
        let mut sum = b[i];
        for j in (i + 1)..3 {
            sum -= a[i][j] * x[j];
        }
        let diag = a[i][i];
        if diag.abs() < eps {
            return None;
        }
        x[i] = sum / diag;
    }

    if x.iter().all(|v| v.is_finite()) {
        Some(x)
    } else {
        None
    }
}

/// Least-squares regression of residual on `[1, temperature, rain]`.
/// Returns `[intercept, perDegree, rain]` if non-degenerate.
pub fn solve_regression(residuals: &[f64], temps: &[f64], rains: &[f64]) -> Option<[f64; 3]> {
    let n = residuals.len();
    if n < 3 || n != temps.len() || n != rains.len() {
        return None;
    }

    let mut s_1 = 0.0;
    let mut s_t = 0.0;
    let mut s_r = 0.0;
    let mut s_tt = 0.0;
    let mut s_tr = 0.0;
    let mut s_rr = 0.0;
    let mut s_y = 0.0;
    let mut s_ty = 0.0;
    let mut s_ry = 0.0;

    for i in 0..n {
        let y = residuals[i];
        let t = temps[i];
        let r = rains[i];

        s_1 += 1.0;
        s_t += t;
        s_r += r;
        s_tt += t * t;
        s_tr += t * r;
        s_rr += r * r;
        s_y += y;
        s_ty += t * y;
        s_ry += r * y;
    }

    let a = [[s_1, s_t, s_r], [s_t, s_tt, s_tr], [s_r, s_tr, s_rr]];
    let b = [s_y, s_ty, s_ry];

    solve_3x3(a, b)
}

/// Computes the residual standard deviation after regression.
pub fn compute_residual_sigma(
    residuals: &[f64],
    temps: &[f64],
    rains: &[f64],
    beta: [f64; 3],
) -> Option<f64> {
    let n = residuals.len();
    if n < 3 {
        return None;
    }
    let [b0, b1, b2] = beta;
    let mut sse = 0.0;
    for i in 0..n {
        let pred = b0 + b1 * temps[i] + b2 * rains[i];
        let err = residuals[i] - pred;
        sse += err * err;
    }
    let dof = (n - 3).max(1);
    Some((sse / dof as f64).sqrt())
}

/// Computes raw standard deviation of residuals when weather regression is absent.
pub fn compute_raw_sigma(residuals: &[f64]) -> Option<f64> {
    let n = residuals.len();
    if n < 2 {
        return None;
    }
    let mut sse = 0.0;
    for &r in residuals {
        sse += r * r;
    }
    let dof = (n - 1).max(1);
    Some((sse / dof as f64).sqrt())
}

/// Generates the estimate for the next 6 hours, clamped to `0..=total_slots` with ±1.28 σ bands.
pub fn compute_estimate(
    now: i64,
    total_slots: usize,
    profile: &[ProfileSlot],
    weather: Option<&WeatherEffect>,
    assumption: Option<&WeatherAssumption>,
    // Per hour-of-week slot, what the weather terms add before the weather itself: the slot group's
    // mean residual less the terms at its mean weather (the within-slot estimator, see lib.rs).
    offset: &dyn Fn(usize) -> f64,
    sigma: Option<f64>,
) -> Vec<EstimateHour> {
    let current_hour = div_floor(now, 3600) * 3600;
    let mut out = Vec::with_capacity(6);
    let slots_max = total_slots as f64;
    let half_band = 1.28 * sigma.unwrap_or(0.0);

    for k in 1..=6 {
        let future_time = current_hour + k * 3600;
        let slot = helsinki_hour_of_week(future_time);
        let base = profile.get(slot).and_then(|p| p.mean).unwrap_or(0.0);
        let weather_adj = match (weather, assumption) {
            (Some(w), Some(a)) => {
                let rain_term = if a.raining { 1.0 } else { 0.0 };
                offset(slot) + w.per_degree * a.temperature + w.rain * rain_term
            }
            _ => 0.0,
        };
        let raw = base + weather_adj;

        let mean = raw.clamp(0.0, slots_max);
        let low = (mean - half_band).clamp(0.0, slots_max);
        let high = (mean + half_band).clamp(0.0, slots_max);

        out.push(EstimateHour {
            at: format_rfc3339_utc(future_time),
            mean,
            low,
            high,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_fallback() {
        // Monday 5 October 2026 00:00 in Helsinki (EEST, UTC+3) is 2026-10-04T21:00:00Z.
        let monday = 1_791_147_600;
        // Slot 0 (Monday 00:00) has 2 points -> own mean
        let mut pts = vec![
            HourlyPoint {
                utc_hour: monday,
                value: 10.0,
            },
            HourlyPoint {
                utc_hour: monday + 7 * 86_400,
                value: 20.0,
            },
        ];

        // Slot 24 (Tuesday 00:00) has 1 point -> fallback to hod 0 mean
        pts.push(HourlyPoint {
            utc_hour: monday + 86_400,
            value: 100.0,
        });

        let profile = compute_profile(&pts);
        // Slot 0 has 2 points -> 15.0
        assert_eq!(profile[0].count, 2);
        assert_eq!(profile[0].mean, Some(15.0));

        // Slot 24 has 1 point -> fallback to hod 0 mean: (10 + 20 + 100) / 3 = 43.333...
        assert_eq!(profile[24].count, 1);
        let expected_hod0 = (10.0 + 20.0 + 100.0) / 3.0;
        assert!((profile[24].mean.unwrap() - expected_hod0).abs() < 1e-6);

        // Slot 1 has 0 points -> hod 1 has 0 points -> None
        assert_eq!(profile[1].count, 0);
        assert_eq!(profile[1].mean, None);
    }

    #[test]
    fn regression_recovering_known_coefficients() {
        let n = 100;
        let mut residuals = Vec::with_capacity(n);
        let mut temps = Vec::with_capacity(n);
        let mut rains = Vec::with_capacity(n);

        // bikes residual = 0.5 * temp - 2.0 * rain (noise-free, intercept = 0)
        for i in 0..n {
            let t = 5.0 + (i as f64 % 25.0);
            let r = if i % 4 == 0 { 1.0 } else { 0.0 };
            let y = 0.5 * t - 2.0 * r;

            temps.push(t);
            rains.push(r);
            residuals.push(y);
        }

        let beta = solve_regression(&residuals, &temps, &rains).expect("regression should succeed");
        assert!((beta[0] - 0.0).abs() < 1e-6, "intercept: {}", beta[0]);
        assert!((beta[1] - 0.5).abs() < 1e-6, "perDegree: {}", beta[1]);
        assert!((beta[2] - (-2.0)).abs() < 1e-6, "rain: {}", beta[2]);

        let sigma = compute_residual_sigma(&residuals, &temps, &rains, beta).unwrap();
        assert!(sigma < 1e-6, "noise-free sigma: {}", sigma);
    }

    #[test]
    fn constant_temperature_is_singular() {
        let residuals = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let temps = vec![15.0; 5]; // constant temperature
        let rains = vec![0.0, 1.0, 0.0, 1.0, 0.0];

        assert_eq!(solve_regression(&residuals, &temps, &rains), None);
    }

    #[test]
    fn constant_rain_is_singular() {
        let residuals = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let temps = vec![10.0, 12.0, 14.0, 16.0, 18.0];
        let rains = vec![0.0; 5]; // never rained

        assert_eq!(solve_regression(&residuals, &temps, &rains), None);
    }

    #[test]
    fn clamping_to_zero_and_total_slots() {
        let mut profile = Vec::new();
        for slot in 0..168 {
            profile.push(ProfileSlot {
                slot,
                mean: Some(10.0),
                count: 5,
            });
        }

        let weather = WeatherEffect {
            per_degree: 5.0,
            rain: -20.0,
            hours: 50,
        };
        // Very hot: 10 + (5 * 30) = 160 -> clamped to total_slots = 20
        let hot_assumption = WeatherAssumption {
            temperature: 30.0,
            raining: false,
        };
        let est_high = compute_estimate(
            0,
            20,
            &profile,
            Some(&weather),
            Some(&hot_assumption),
            &|_| 0.0,
            Some(2.0),
        );
        assert_eq!(est_high.len(), 6);
        for h in est_high {
            assert_eq!(h.mean, 20.0);
            assert_eq!(h.high, 20.0);
        }

        // Heavy rain and cold: 10 + (5 * -10) + (-20 * 1) = -60 -> clamped to 0.0
        let cold_rain = WeatherAssumption {
            temperature: -10.0,
            raining: true,
        };
        let est_low = compute_estimate(
            0,
            20,
            &profile,
            Some(&weather),
            Some(&cold_rain),
            &|_| 0.0,
            Some(2.0),
        );
        assert_eq!(est_low.len(), 6);
        for h in est_low {
            assert_eq!(h.mean, 0.0);
            assert_eq!(h.low, 0.0);
        }
    }

    #[test]
    fn a_raw_sigma_needs_two_residuals() {
        assert_eq!(compute_raw_sigma(&[]), None);
        assert_eq!(compute_raw_sigma(&[1.0]), None);
        let sigma = compute_raw_sigma(&[1.0, -1.0]).expect("two residuals");
        assert!((sigma - 2.0_f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn a_degenerate_system_has_no_solution() {
        assert_eq!(solve_3x3([[0.0; 3]; 3], [1.0, 2.0, 3.0]), None);
        assert_eq!(
            solve_3x3(
                [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [1.0, 1.0, 1.0]],
                [1.0, 2.0, 3.0]
            ),
            None
        );
        assert_eq!(
            solve_regression(&[1.0, 2.0, 3.0], &[1.0, 2.0], &[0.0, 0.0, 0.0]),
            None
        );
        assert_eq!(
            compute_residual_sigma(&[1.0, 2.0], &[1.0, 2.0], &[0.0, 0.0], [0.0, 0.0, 0.0]),
            None
        );
    }

    #[test]
    fn weather_terms_without_an_assumed_weather_add_nothing() {
        let profile: Vec<ProfileSlot> = (0..168)
            .map(|slot| ProfileSlot {
                slot,
                mean: Some(5.0),
                count: 3,
            })
            .collect();
        let weather = WeatherEffect {
            per_degree: 1.0,
            rain: -2.0,
            hours: 10,
        };
        let with = compute_estimate(0, 10, &profile, Some(&weather), None, &|_| 3.0, None);
        let without = compute_estimate(0, 10, &profile, None, None, &|_| 3.0, None);
        assert_eq!(with, without);
    }
}
