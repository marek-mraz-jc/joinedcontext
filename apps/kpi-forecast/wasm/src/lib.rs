//! Every Helsinki KPI with its trend, a short forecast and the points that look wrong (T-3332),
//! computed in the visitor's browser: per indicator a least-squares trend, Holt-Winters (additive)
//! on a regular clock with a 95 % interval, and the points whose one-step error lies more than
//! three robust deviations from the median error. One call, JSON in and JSON out, from a Web Worker.

pub mod forecast;
pub mod stats;

use serde::{Deserialize, Serialize};
#[cfg(feature = "web")]
use wasm_bindgen::prelude::wasm_bindgen;

use forecast::{best_fit, regularise, season_length, Ahead, Weights};

const DAY: f64 = 86_400_000.0;
/// Fewer points than this get a trend and no forecast: Holt needs three steps to seed and a few
/// more before its error means anything.
pub const MIN_POINTS: usize = 6;
/// A move smaller than this share of the series' scale over the whole window is flat.
const FLAT: f64 = 0.02;
/// A line explaining less than this share of the variation is no clear trend: flat.
const MIN_R2: f64 = 0.3;
/// How many robust standard deviations from the usual one-step error a point looks wrong at.
const ANOMALY_SDS: f64 = 3.0;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    /// Milliseconds since the epoch.
    pub t: f64,
    pub v: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    pub id: String,
    #[serde(default)]
    pub points: Vec<Point>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub series: Vec<Series>,
    /// Steps ahead; absent: one season, else a quarter of the history, at most 24.
    #[serde(default)]
    pub horizon: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Flat,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trend {
    pub slope_per_day: f64,
    pub r2: f64,
    /// The change in a week as a share of the series' mean; `None` around a mean of zero.
    pub change_per_week: Option<f64>,
    pub direction: Direction,
}

#[derive(Debug, Serialize)]
pub struct Anomaly {
    pub t: f64,
    pub v: f64,
    pub expected: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Trend, forecast and anomalies.
    Ok,
    /// Some history, too little to forecast: the trend alone (none for a single point).
    Short,
    /// No point at all.
    Empty,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Result {
    pub id: String,
    pub status: Status,
    /// The points read, after dropping the unreadable and repeated ones.
    pub n: usize,
    pub first: Option<f64>,
    pub last: Option<f64>,
    pub latest: Option<f64>,
    pub trend: Option<Trend>,
    /// The regular step the model ran at, in milliseconds.
    pub step: Option<f64>,
    /// Steps in a season; 0 for none.
    pub season: usize,
    pub weights: Option<Weights>,
    /// The spread of the usual one-step error (IQR / 1.349): the forecast interval's unit.
    pub sigma: Option<f64>,
    /// The history as read, `[t, v]`, for the chart.
    pub history: Vec<[f64; 2]>,
    /// What the model expected one step ahead, `[t, v]`.
    pub fitted: Vec<[f64; 2]>,
    pub forecast: Vec<Ahead>,
    pub anomalies: Vec<Anomaly>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub results: Vec<Result>,
    pub rising: usize,
    pub falling: usize,
    pub flat: usize,
    pub short: usize,
    pub anomalies: usize,
}

/// The points sorted by time, the unreadable ones dropped, a repeated time keeping its last value.
fn clean(points: &[Point]) -> (Vec<f64>, Vec<f64>) {
    let mut kept: Vec<(f64, f64)> = points
        .iter()
        .filter(|p| p.t.is_finite() && p.v.is_finite())
        .map(|p| (p.t, p.v))
        .collect();
    // Stable: of two points at one time the later in the input stays later, and wins below.
    kept.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut times: Vec<f64> = Vec::with_capacity(kept.len());
    let mut values: Vec<f64> = Vec::with_capacity(kept.len());
    for (t, v) in kept {
        if times.last() == Some(&t) {
            if let Some(last) = values.last_mut() {
                *last = v;
            }
        } else {
            times.push(t);
            values.push(v);
        }
    }
    (times, values)
}

fn trend_of(times: &[f64], values: &[f64]) -> Option<Trend> {
    let days: Vec<f64> = times.iter().map(|t| (t - times[0]) / DAY).collect();
    let line = stats::fit_line(&days, values)?;
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let spread =
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt();
    let scale = mean.abs().max(spread);
    let moved = line.slope * (days[days.len() - 1] - days[0]);
    let direction = if scale == 0.0 || moved.abs() < FLAT * scale || line.r2 < MIN_R2 {
        Direction::Flat
    } else if moved > 0.0 {
        Direction::Up
    } else {
        Direction::Down
    };
    Some(Trend {
        slope_per_day: line.slope,
        r2: line.r2,
        change_per_week: (mean.abs() > 1e-12).then(|| line.slope * 7.0 / mean.abs()),
        direction,
    })
}

/// One indicator, from its raw points to its trend, forecast and anomalies.
pub fn analyse_series(series: &Series, horizon: Option<usize>) -> Result {
    let (times, values) = clean(&series.points);
    let mut out = Result {
        id: series.id.clone(),
        status: if times.is_empty() {
            Status::Empty
        } else {
            Status::Short
        },
        n: times.len(),
        first: times.first().copied(),
        last: times.last().copied(),
        latest: values.last().copied(),
        trend: None,
        step: None,
        season: 0,
        weights: None,
        sigma: None,
        history: times.iter().zip(&values).map(|(t, v)| [*t, *v]).collect(),
        fitted: Vec::new(),
        forecast: Vec::new(),
        anomalies: Vec::new(),
    };
    if times.len() < 2 {
        return out;
    }
    out.trend = trend_of(&times, &values);
    if times.len() < MIN_POINTS {
        return out;
    }
    let Some(grid) = regularise(&times, &values) else {
        return out;
    };
    let season = season_length(grid.step, grid.values.len());
    let Some(model) = best_fit(&grid.values, season).or_else(|| best_fit(&grid.values, 0)) else {
        return out;
    };
    let steps = horizon.unwrap_or(if model.season > 0 {
        model.season
    } else {
        (grid.values.len() / 4).clamp(1, 24)
    });
    out.status = Status::Ok;
    out.step = Some(grid.step);
    out.season = model.season;
    out.weights = Some(model.weights);
    // The interval's spread from the usual one-step error, not its mean square: one reading far
    // off would otherwise widen every forecast. A model that never errs keeps its own (zero).
    let grid_errors: Vec<f64> = model
        .expected
        .iter()
        .zip(&grid.values)
        .filter_map(|(e, v)| e.map(|e| v - e))
        .collect();
    let sigma = match stats::robust_sd(&grid_errors) {
        Some(sd) if sd > 0.0 => sd,
        _ => model.sigma,
    };
    out.sigma = Some(sigma);
    out.fitted = model
        .expected
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.map(|v| [grid.time(i), v]))
        .collect();
    out.forecast = model.forecast(&grid, steps.min(10 * 24), sigma);

    // The points that look wrong: the one-step error far from the usual one, in robust deviations.
    let errors: Vec<(usize, f64, f64)> = times
        .iter()
        .enumerate()
        .filter_map(|(i, &t)| model.expected_at(&grid, t).map(|e| (i, e, values[i] - e)))
        .collect();
    let residuals: Vec<f64> = errors.iter().map(|e| e.2).collect();
    if let (Some(centre), Some(spread)) = (stats::median(&residuals), stats::robust_sd(&residuals))
    {
        // A series that hardly deviates has a spread near 0, and the model settling after one jump
        // would look wrong at every step: an error under about 4 % of the level never does.
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let floor = 0.01 * mean.abs().max(f64::MIN_POSITIVE);
        let limit = ANOMALY_SDS * spread.max(floor);
        out.anomalies = errors
            .into_iter()
            .filter(|(_, _, r)| (r - centre).abs() > limit)
            .map(|(i, expected, _)| Anomaly {
                t: times[i],
                v: values[i],
                expected,
            })
            .collect();
    }
    out
}

pub fn run(input: &Input) -> Output {
    let results: Vec<Result> = input
        .series
        .iter()
        .map(|s| analyse_series(s, input.horizon))
        .collect();
    let count = |d: Direction| {
        results
            .iter()
            .filter(|r| {
                r.status == Status::Ok && r.trend.as_ref().is_some_and(|t| t.direction == d)
            })
            .count()
    };
    Output {
        rising: count(Direction::Up),
        falling: count(Direction::Down),
        flat: count(Direction::Flat),
        short: results.iter().filter(|r| r.status != Status::Ok).count(),
        anomalies: results.iter().map(|r| r.anomalies.len()).sum(),
        results,
    }
}

/// The module's one entry: the input as JSON, the output as JSON, or `{"error": …}`.
#[cfg_attr(feature = "web", wasm_bindgen)]
pub fn analyse(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(parsed) => serde_json::to_string(&run(&parsed))
            .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }).to_string()),
        Err(e) => {
            serde_json::json!({ "error": format!("the series could not be read: {e}") }).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const HOUR: f64 = 3_600_000.0;

    fn series(id: &str, points: &[(f64, f64)]) -> Series {
        Series {
            id: id.into(),
            points: points.iter().map(|&(t, v)| Point { t, v }).collect(),
        }
    }

    fn hourly(values: impl IntoIterator<Item = f64>) -> Vec<(f64, f64)> {
        values
            .into_iter()
            .enumerate()
            .map(|(i, v)| (i as f64 * HOUR, v))
            .collect()
    }

    #[test]
    fn no_points_is_empty_and_one_point_is_short_without_a_trend() {
        let empty = analyse_series(&series("a", &[]), None);
        assert_eq!(
            (empty.status, empty.n, empty.latest),
            (Status::Empty, 0, None)
        );
        let one = analyse_series(&series("b", &[(5.0, 7.0)]), None);
        assert_eq!((one.status, one.latest), (Status::Short, Some(7.0)));
        assert!(one.trend.is_none() && one.forecast.is_empty());
    }

    #[test]
    fn a_few_points_get_a_trend_and_no_forecast() {
        let short = analyse_series(
            &series("c", &[(0.0, 1.0), (DAY, 2.0), (2.0 * DAY, 3.0)]),
            None,
        );
        assert_eq!(short.status, Status::Short);
        let trend = short.trend.expect("a trend");
        assert_eq!(trend.direction, Direction::Up);
        assert!((trend.slope_per_day - 1.0).abs() < 1e-9);
        assert!((trend.change_per_week.expect("a mean") - 3.5).abs() < 1e-9);
        assert!(short.forecast.is_empty());
    }

    #[test]
    fn unreadable_and_repeated_points_are_dropped_and_order_does_not_matter() {
        let s = Series {
            id: "d".into(),
            points: vec![
                Point {
                    t: 2.0 * HOUR,
                    v: 3.0,
                },
                Point {
                    t: f64::NAN,
                    v: 1.0,
                },
                Point { t: 0.0, v: 1.0 },
                Point { t: HOUR, v: 9.0 },
                Point { t: HOUR, v: 2.0 },
                Point {
                    t: 3.0 * HOUR,
                    v: f64::INFINITY,
                },
            ],
        };
        let out = analyse_series(&s, None);
        assert_eq!(out.n, 3);
        assert_eq!(
            out.history,
            vec![[0.0, 1.0], [HOUR, 2.0], [2.0 * HOUR, 3.0]]
        );
        assert_eq!(out.latest, Some(3.0));
    }

    #[test]
    fn a_rising_series_is_forecast_on_its_line() {
        let out = analyse_series(
            &series("e", &hourly((0..30).map(|i| 100.0 + f64::from(i)))),
            None,
        );
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.trend.as_ref().map(|t| t.direction), Some(Direction::Up));
        assert_eq!(out.season, 0);
        assert_eq!(out.forecast.len(), 7, "a quarter of 30 steps");
        assert!((out.forecast[0].v - 130.0).abs() < 1e-6);
        assert_eq!(out.forecast[0].t, 30.0 * HOUR);
        assert!(out.anomalies.is_empty());
    }

    #[test]
    fn a_flat_series_is_flat_and_one_spike_in_it_looks_wrong() {
        let mut values = vec![50.0; 40];
        values[25] = 80.0;
        let out = analyse_series(&series("f", &hourly(values)), Some(3));
        assert_eq!(
            out.trend.as_ref().map(|t| t.direction),
            Some(Direction::Flat)
        );
        assert_eq!(out.forecast.len(), 3);
        assert_eq!(out.anomalies.len(), 1);
        assert_eq!(
            (out.anomalies[0].t, out.anomalies[0].v),
            (25.0 * HOUR, 80.0)
        );
    }

    #[test]
    fn one_reading_far_off_does_not_widen_the_forecast() {
        let wobble = |i: i32| f64::from((i * 37) % 11 - 5) / 10.0;
        let mut values: Vec<f64> = (0..60).map(|i| 100.0 + wobble(i)).collect();
        let calm = analyse_series(&series("calm", &hourly(values.clone())), Some(1));
        values[40] = 400.0;
        let spiked = analyse_series(&series("spiked", &hourly(values)), Some(1));
        let width = |r: &Result| r.forecast[0].hi - r.forecast[0].lo;
        assert_eq!(spiked.anomalies.len(), 1);
        assert!(
            width(&spiked) < 2.0 * width(&calm),
            "{} against {}",
            width(&spiked),
            width(&calm)
        );
    }

    #[test]
    fn a_daily_rhythm_is_found_and_carried_forward() {
        let rhythm = (0..24 * 5).map(|i| if i % 24 < 8 { 5.0 } else { 15.0 });
        let out = analyse_series(&series("g", &hourly(rhythm)), None);
        assert_eq!(out.season, 24);
        assert_eq!(out.forecast.len(), 24);
        assert!(out.forecast.iter().take(8).all(|a| (a.v - 5.0).abs() < 1.0));
        assert!(out
            .forecast
            .iter()
            .skip(8)
            .all(|a| (a.v - 15.0).abs() < 1.0));
    }

    #[test]
    fn a_falling_series_counts_as_falling() {
        let out = run(&Input {
            series: vec![
                series("down", &hourly((0..10).map(|i| 50.0 - f64::from(i)))),
                series("none", &[]),
            ],
            horizon: None,
        });
        assert_eq!((out.rising, out.falling, out.flat, out.short), (0, 1, 0, 1));
    }

    #[test]
    fn a_series_around_zero_has_no_relative_change() {
        let out = analyse_series(
            &series("z", &hourly([-1.0, 1.0, -1.0, 1.0, -1.0, 1.0])),
            None,
        );
        let trend = out.trend.expect("a trend");
        assert_eq!(
            (trend.change_per_week, trend.direction),
            (None, Direction::Flat)
        );
    }

    #[test]
    fn json_in_json_out_and_an_error_for_what_it_cannot_read() {
        let answer: Value = serde_json::from_str(&analyse(
            &json!({ "series": [{ "id": "x", "points": [{ "t": 0, "v": 1 }, { "t": 3600000, "v": 2 }] }] }).to_string(),
        ))
        .expect("json");
        assert_eq!(answer["results"][0]["status"], "short");
        assert_eq!(answer["results"][0]["trend"]["direction"], "up");
        let refused: Value = serde_json::from_str(&analyse("{\"series\": 3}")).expect("json");
        assert!(refused["error"]
            .as_str()
            .expect("an error")
            .starts_with("the series could not be read"));
        let unknown: Value =
            serde_json::from_str(&analyse("{\"series\": [], \"extra\": 1}")).expect("json");
        assert!(unknown["error"].is_string());
    }
}
