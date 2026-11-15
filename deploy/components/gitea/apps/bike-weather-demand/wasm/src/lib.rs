//! Helsinki bike availability & weather sensitivity WASM engine (T-3337).
//! Computes hour-of-week profile, 3x3 least-squares weather regression,
//! and next 6-hour forecast with uncertainty bands in the browser.

pub mod model;
pub mod series;

use crate::model::{
    compute_estimate, compute_profile, compute_raw_sigma, compute_residual_sigma, solve_regression,
    EstimateHour, ProfileSlot, WeatherAssumption, WeatherEffect,
};
use crate::series::{align_weather, hourly_means, hourly_weather, parse_rfc3339};
use serde::{Deserialize, Serialize};
#[cfg(feature = "browser")]
use wasm_bindgen::prelude::wasm_bindgen;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BikePointInput {
    pub at: String,
    pub value: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherPointInput {
    pub at: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub precipitation: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub now: i64,
    #[serde(default)]
    pub total_slots: usize,
    #[serde(default)]
    pub bikes: Vec<BikePointInput>,
    #[serde(default)]
    pub weather: Vec<WeatherPointInput>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub hours: usize,
    pub profile: Vec<ProfileSlot>,
    pub weather: Option<WeatherEffect>,
    pub sigma: Option<f64>,
    pub estimate: Vec<EstimateHour>,
    pub assumption: Option<WeatherAssumption>,
    pub enough: bool,
}

/// Runs the model estimation pipeline over the parsed input.
pub fn run_estimate(input: &Input) -> Output {
    let mut raw_bikes = Vec::with_capacity(input.bikes.len());
    for b in &input.bikes {
        if let Some(t) = parse_rfc3339(&b.at) {
            if b.value.is_finite() && b.value >= 0.0 {
                raw_bikes.push((t, b.value));
            }
        }
    }

    let bike_hours = hourly_means(&raw_bikes);
    let hours = bike_hours.len();
    let profile = compute_profile(&bike_hours);
    let enough = hours >= 24;

    if !enough {
        return Output {
            hours,
            profile,
            weather: None,
            sigma: None,
            estimate: Vec::new(),
            assumption: None,
            enough: false,
        };
    }

    // Parse weather observations
    let mut raw_weather = Vec::with_capacity(input.weather.len());
    for w in &input.weather {
        if let Some(t) = parse_rfc3339(&w.at) {
            raw_weather.push((t, w.temperature, w.precipitation));
        }
    }
    let weather_hours = hourly_weather(&raw_weather);
    let aligned = align_weather(&bike_hours, &weather_hours);

    // Find the latest observed weather for the assumption
    let mut latest_t: Option<f64> = None;
    let mut latest_rain: bool = false;
    let mut latest_ts: i64 = i64::MIN;

    for w in &input.weather {
        if let Some(t) = parse_rfc3339(&w.at) {
            if let Some(temp) = w.temperature {
                if temp.is_finite() && t > latest_ts {
                    latest_ts = t;
                    latest_t = Some(temp);
                    latest_rain = w.precipitation.unwrap_or(0.0) > 0.1;
                }
            }
        }
    }

    let assumption_candidate = latest_t.map(|temperature| WeatherAssumption {
        temperature,
        raining: latest_rain,
    });

    // The within-slot estimator: residuals, temperatures and rain are each taken from their slot
    // group's mean (the profile's slot, or its hour of day where the profile fell back to it).
    // Regressing raw weather on residuals let the profile absorb every weather effect that follows
    // the time of day, and the coefficients came out biased toward zero.
    let group_of = |slot: usize| -> usize {
        if profile.get(slot).is_some_and(|p| p.count >= 2) {
            slot
        } else {
            168 + slot % 24
        }
    };
    let mut rows: Vec<(usize, f64, f64, f64)> = Vec::new();
    for a in &aligned {
        if let Some(w) = a.weather {
            let slot = series::helsinki_hour_of_week(a.utc_hour);
            if let Some(base) = profile.get(slot).and_then(|p| p.mean) {
                let rain = if w.precipitation > 0.1 { 1.0 } else { 0.0 };
                rows.push((group_of(slot), a.bikes - base, w.temperature, rain));
            }
        }
    }
    let mut sums = vec![(0.0_f64, 0.0_f64, 0.0_f64, 0usize); 192];
    for &(g, r, t, rain) in &rows {
        sums[g].0 += r;
        sums[g].1 += t;
        sums[g].2 += rain;
        sums[g].3 += 1;
    }
    let group_mean = |g: usize| -> Option<(f64, f64, f64)> {
        let (r, t, rain, n) = sums[g];
        (n > 0).then(|| (r / n as f64, t / n as f64, rain / n as f64))
    };
    let mut residuals = Vec::with_capacity(rows.len());
    let mut temps = Vec::with_capacity(rows.len());
    let mut rains = Vec::with_capacity(rows.len());
    for &(g, r, t, rain) in &rows {
        if let Some((mr, mt, mrain)) = group_mean(g) {
            residuals.push(r - mr);
            temps.push(t - mt);
            rains.push(rain - mrain);
        }
    }

    let n_weather = residuals.len();
    let regression_opt = if n_weather >= 3 {
        solve_regression(&residuals, &temps, &rains)
    } else {
        None
    };

    if let Some([b0, b1, b2]) = regression_opt {
        let weather_effect = WeatherEffect {
            per_degree: b1,
            rain: b2,
            hours: n_weather,
        };
        let sigma = compute_residual_sigma(&residuals, &temps, &rains, [b0, b1, b2]);
        let estimate = compute_estimate(
            input.now,
            input.total_slots,
            &profile,
            Some(&weather_effect),
            assumption_candidate.as_ref(),
            &|slot| {
                group_mean(group_of(slot)).map_or(0.0, |(mr, mt, mrain)| mr - b1 * mt - b2 * mrain)
            },
            sigma,
        );

        Output {
            hours,
            profile,
            weather: Some(weather_effect),
            sigma,
            estimate,
            assumption: assumption_candidate,
            enough: true,
        }
    } else {
        // Fallback without weather terms
        let mut raw_residuals = Vec::with_capacity(bike_hours.len());
        for pt in &bike_hours {
            let slot = series::helsinki_hour_of_week(pt.utc_hour);
            if let Some(base) = profile.get(slot).and_then(|p| p.mean) {
                raw_residuals.push(pt.value - base);
            }
        }
        let sigma = compute_raw_sigma(&raw_residuals);
        let estimate = compute_estimate(
            input.now,
            input.total_slots,
            &profile,
            None,
            None,
            &|_| 0.0,
            sigma,
        );

        Output {
            hours,
            profile,
            weather: None,
            sigma,
            estimate,
            assumption: None,
            enough: true,
        }
    }
}

/// WASM entry point: reads JSON input and serialises JSON output or an error object. Never panics.
#[cfg_attr(feature = "browser", wasm_bindgen)]
pub fn estimate(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(parsed) => {
            let out = run_estimate(&parsed);
            serde_json::to_string(&out).unwrap_or_else(|e| {
                serde_json::json!({ "error": format!("the estimate could not be serialized: {e}") }).to_string()
            })
        }
        Err(e) => serde_json::json!({ "error": format!("the estimate could not be read: {e}") })
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fewer_than_24_hours_yields_enough_false_and_no_regression() {
        let input = Input {
            now: 1_700_000_000,
            total_slots: 20,
            bikes: vec![
                BikePointInput {
                    at: "2026-03-29T10:00:00Z".to_string(),
                    value: 5.0,
                },
                BikePointInput {
                    at: "2026-03-29T11:00:00Z".to_string(),
                    value: 6.0,
                },
            ],
            weather: vec![],
        };
        let out = run_estimate(&input);
        assert_eq!(out.hours, 2);
        assert!(!out.enough);
        assert_eq!(out.weather, None);
        assert_eq!(out.sigma, None);
        assert!(out.estimate.is_empty());
        assert_eq!(out.assumption, None);
        assert_eq!(out.profile.len(), 168);
    }

    #[test]
    fn empty_input() {
        let input = Input {
            now: 0,
            total_slots: 0,
            bikes: vec![],
            weather: vec![],
        };
        let out = run_estimate(&input);
        assert_eq!(out.hours, 0);
        assert!(!out.enough);
        assert_eq!(out.weather, None);
        assert_eq!(out.sigma, None);
        assert!(out.estimate.is_empty());
        assert_eq!(out.profile.len(), 168);
    }

    #[test]
    fn bad_json() {
        let raw = estimate("not json at all");
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("should answer json");
        assert!(parsed["error"]
            .as_str()
            .unwrap()
            .starts_with("the estimate could not be read"));
    }

    #[test]
    fn full_pipeline_with_known_weather_recovery() {
        let base_ts = 1_774_742_400; // 2026-03-29 00:00:00 UTC
        let mut bikes = Vec::new();
        let mut weather = Vec::new();

        // 48 hours of synthetic data
        for i in 0..48 {
            let ts = base_ts + i * 3600;
            let at = series::format_rfc3339_utc(ts);
            let t = 10.0 + (i as f64 % 15.0);
            let r = if i % 5 == 0 { 2.0 } else { 0.0 };

            weather.push(WeatherPointInput {
                at: at.clone(),
                temperature: Some(t),
                precipitation: Some(r),
            });

            // Base = 15.0, perDegree = 0.5, rain = -2.0
            let rain_term = if r > 0.1 { 1.0 } else { 0.0 };
            let val = 15.0 + 0.5 * t - 2.0 * rain_term;
            bikes.push(BikePointInput { at, value: val });
        }

        let input = Input {
            now: base_ts + 48 * 3600,
            total_slots: 40,
            bikes,
            weather,
        };

        let out = run_estimate(&input);

        assert!(out.enough);
        assert_eq!(out.hours, 48);
        assert!(out.weather.is_some());
        let w = out.weather.unwrap();
        assert_eq!(w.hours, 48);
        assert!(
            (w.per_degree - 0.5).abs() < 1e-4,
            "perDegree: {}",
            w.per_degree
        );
        assert!((w.rain - (-2.0)).abs() < 1e-4, "rain: {}", w.rain);
        assert!(out.sigma.unwrap() < 1e-4);
        assert_eq!(out.estimate.len(), 6);
        assert!(out.assumption.is_some());
    }

    /// Two days of hourly counts from 2026-03-01, a slow daily wave around ten bikes.
    fn two_days() -> Vec<BikePointInput> {
        (0..48)
            .map(|h| BikePointInput {
                at: format!("2026-03-{:02}T{:02}:00:00Z", 1 + h / 24, h % 24),
                value: 10.0 + (h % 24) as f64 / 4.0,
            })
            .collect()
    }

    #[test]
    fn enough_history_without_weather_estimates_from_the_profile_alone() {
        let mut bikes = two_days();
        // A count that is no count, and a time that is no time, are never a point.
        bikes.push(BikePointInput {
            at: "2026-03-03T00:00:00Z".to_string(),
            value: f64::NAN,
        });
        bikes.push(BikePointInput {
            at: "2026-03-03T01:00:00Z".to_string(),
            value: -1.0,
        });
        bikes.push(BikePointInput {
            at: "yesterday".to_string(),
            value: 5.0,
        });
        let input = Input {
            now: 1_772_668_800,
            total_slots: 20,
            bikes,
            weather: vec![],
        };
        let out = run_estimate(&input);
        assert_eq!(out.hours, 48);
        assert!(out.enough);
        assert_eq!(out.weather, None);
        assert_eq!(out.assumption, None);
        assert!(out.sigma.is_some());
        assert_eq!(out.estimate.len(), 6);
        assert!(out
            .estimate
            .iter()
            .all(|e| e.low <= e.mean && e.mean <= e.high && e.high <= 20.0));
    }

    #[test]
    fn the_entry_answers_a_whole_estimate_as_json() {
        let points: Vec<serde_json::Value> = two_days()
            .iter()
            .map(|p| serde_json::json!({ "at": p.at, "value": p.value }))
            .collect();
        let raw = estimate(
            &serde_json::json!({ "now": 1_772_668_800, "totalSlots": 20, "bikes": points })
                .to_string(),
        );
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("should answer json");
        assert_eq!(parsed["enough"], true);
        assert_eq!(parsed["hours"], 48);
        assert!(parsed.get("error").is_none());
    }
}
