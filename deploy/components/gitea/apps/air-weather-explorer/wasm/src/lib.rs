//! How weather moves air quality (T-3330): a station's pollutant series and a weather station's
//! series put on common hours, smoothed by a rolling mean, the Pearson and Spearman correlation
//! of every pollutant with every weather variable, and the readings far from the rest (MAD).
//! Built natively for `cargo test` and to WebAssembly for the page, where [`analyse`] takes and
//! answers JSON. Times are UTC epoch milliseconds.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
#[cfg(feature = "web")]
use wasm_bindgen::prelude::wasm_bindgen;

const HOUR: i64 = 3_600_000;
/// The fewest common hours a correlation is computed over; fewer say nothing.
pub const MIN_PAIRS: usize = 6;
/// The longest rolling window, in hours.
const MAX_WINDOW: usize = 48;
/// MAD is scaled by this to estimate a normal distribution's standard deviation.
const MAD_SCALE: f64 = 1.4826;

/// One reading: when (epoch ms) and what.
pub type Point = (i64, f64);

/// The hourly means of `points`, keyed by the hour's start; a value that is not finite is left out.
pub fn hourly(points: &[Point]) -> BTreeMap<i64, f64> {
    let mut sums: BTreeMap<i64, (f64, u32)> = BTreeMap::new();
    for &(at, value) in points {
        if !value.is_finite() {
            continue;
        }
        let entry = sums.entry(at.div_euclid(HOUR) * HOUR).or_insert((0.0, 0));
        entry.0 += value;
        entry.1 += 1;
    }
    sums.into_iter()
        .map(|(hour, (sum, n))| (hour, sum / f64::from(n)))
        .collect()
}

/// The mean of each value and the `window - 1` before it on `hours`, over the values present;
/// `None` where none of them is.
pub fn rolling(values: &[Option<f64>], window: usize) -> Vec<Option<f64>> {
    let window = window.clamp(1, MAX_WINDOW);
    (0..values.len())
        .map(|i| {
            let from = (i + 1).saturating_sub(window);
            let present: Vec<f64> = values[from..=i].iter().flatten().copied().collect();
            (!present.is_empty()).then(|| present.iter().sum::<f64>() / present.len() as f64)
        })
        .collect()
}

/// Pearson's correlation of paired values; `None` for too few pairs or a constant side.
pub fn pearson(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len().min(y.len());
    if n < MIN_PAIRS {
        return None;
    }
    let (mx, my) = (
        x[..n].iter().sum::<f64>() / n as f64,
        y[..n].iter().sum::<f64>() / n as f64,
    );
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (dx, dy) = (x[i] - mx, y[i] - my);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx <= f64::EPSILON || syy <= f64::EPSILON {
        return None;
    }
    Some((sxy / (sxx * syy).sqrt()).clamp(-1.0, 1.0))
}

/// The ranks of `values`, 1-based, ties given the mean of the ranks they share.
pub fn ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut out = vec![0.0; values.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && values[order[j + 1]] == values[order[i]] {
            j += 1;
        }
        let rank = (i + j) as f64 / 2.0 + 1.0;
        for &k in &order[i..=j] {
            out[k] = rank;
        }
        i = j + 1;
    }
    out
}

/// Spearman's correlation: Pearson's of the ranks, so ties are handled.
pub fn spearman(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len().min(y.len());
    pearson(&ranks(&x[..n]), &ranks(&y[..n]))
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// The indexes of the values whose robust z-score (distance from the median over 1.4826 MAD) is
/// above `threshold`; none when the values do not spread (MAD 0) or are fewer than [`MIN_PAIRS`].
pub fn outliers(values: &[Option<f64>], threshold: f64) -> Vec<usize> {
    let mut present: Vec<f64> = values.iter().flatten().copied().collect();
    if present.len() < MIN_PAIRS {
        return Vec::new();
    }
    present.sort_by(f64::total_cmp);
    let mid = median(&present);
    let mut deviations: Vec<f64> = present.iter().map(|v| (v - mid).abs()).collect();
    deviations.sort_by(f64::total_cmp);
    let mad = median(&deviations) * MAD_SCALE;
    if mad <= f64::EPSILON {
        return Vec::new();
    }
    values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.filter(|v| ((v - mid).abs() / mad) > threshold).map(|_| i))
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Settings {
    /// Rolling window, in hours; 1 compares the hourly means as they are.
    pub window: usize,
    /// The robust z-score above which a reading is an outlier.
    pub threshold: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: 3,
            threshold: 3.5,
        }
    }
}

/// What the page sends: each attribute's readings by name, of the air station and of the weather
/// station.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub air: BTreeMap<String, Vec<Point>>,
    pub weather: BTreeMap<String, Vec<Point>>,
    #[serde(default)]
    pub settings: Settings,
}

/// One attribute on the common hours.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub name: String,
    /// The hourly mean, `None` for an hour with no reading.
    pub hourly: Vec<Option<f64>>,
    /// The rolling mean the correlations are computed over.
    pub smooth: Vec<Option<f64>>,
    /// Indexes into the hours of the readings far from the rest.
    pub outliers: Vec<usize>,
}

/// One pollutant against one weather variable.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub air: String,
    pub weather: String,
    /// The hours both have a value.
    pub n: usize,
    pub pearson: Option<f64>,
    pub spearman: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    /// Every hour from the first reading to the last, epoch ms.
    pub hours: Vec<i64>,
    pub air: Vec<Series>,
    pub weather: Vec<Series>,
    pub pairs: Vec<Pair>,
}

/// The most hours one analysis spans: 31 days, so a broken timestamp cannot ask for millions.
const MAX_HOURS: i64 = 31 * 24;

fn series(name: &str, points: &[Point], hours: &[i64], settings: &Settings) -> Series {
    let means = hourly(points);
    let hourly: Vec<Option<f64>> = hours.iter().map(|h| means.get(h).copied()).collect();
    let smooth = rolling(&hourly, settings.window);
    Series {
        name: name.to_owned(),
        outliers: outliers(&hourly, settings.threshold),
        hourly,
        smooth,
    }
}

/// The common hours, every attribute on them, and every pollutant against every weather variable.
pub fn run(input: &Input) -> Output {
    let settings = &input.settings;
    let all = input
        .air
        .values()
        .chain(input.weather.values())
        .flatten()
        .filter(|(_, v)| v.is_finite());
    let (first, last) = all.fold((i64::MAX, i64::MIN), |(lo, hi), &(at, _)| {
        (lo.min(at), hi.max(at))
    });
    let hours: Vec<i64> = if first > last {
        Vec::new()
    } else {
        let (start, end) = (first.div_euclid(HOUR) * HOUR, last.div_euclid(HOUR) * HOUR);
        // The newest hours when the readings span more than the cap.
        let start = start.max(end - (MAX_HOURS - 1) * HOUR);
        (0..=(end - start) / HOUR)
            .map(|i| start + i * HOUR)
            .collect()
    };
    let air: Vec<Series> = input
        .air
        .iter()
        .map(|(name, points)| series(name, points, &hours, settings))
        .collect();
    let weather: Vec<Series> = input
        .weather
        .iter()
        .map(|(name, points)| series(name, points, &hours, settings))
        .collect();
    let mut pairs = Vec::new();
    for a in &air {
        for w in &weather {
            let (x, y): (Vec<f64>, Vec<f64>) = a
                .smooth
                .iter()
                .zip(&w.smooth)
                .filter_map(|(x, y)| Some(((*x)?, (*y)?)))
                .unzip();
            pairs.push(Pair {
                air: a.name.clone(),
                weather: w.name.clone(),
                n: x.len(),
                pearson: pearson(&x, &y),
                spearman: spearman(&x, &y),
            });
        }
    }
    Output {
        hours,
        air,
        weather,
        pairs,
    }
}

/// The page's entry: [`Input`] as JSON in, [`Output`] as JSON out, or `{"error": "…"}` naming what
/// could not be read.
#[cfg_attr(feature = "web", wasm_bindgen)]
pub fn analyse(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(input) => serde_json::to_string(&run(&input))
            .unwrap_or_else(|_| r#"{"error":"the analysis could not be written"}"#.to_owned()),
        Err(err) => {
            serde_json::json!({ "error": format!("the readings could not be read: {err}") })
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_918_684_800_000;

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-9)
    }

    #[test]
    fn readings_are_averaged_per_hour() {
        let means = hourly(&[
            (T0, 2.0),
            (T0 + 10 * 60_000, 4.0),
            (T0 + HOUR, 5.0),
            (T0 + HOUR, f64::NAN),
        ]);
        assert_eq!(
            means.into_iter().collect::<Vec<_>>(),
            [(T0, 3.0), (T0 + HOUR, 5.0)]
        );
        assert!(hourly(&[]).is_empty());
        // Before 1970 the hour is still the one the reading falls in.
        assert_eq!(
            hourly(&[(-1, 1.0)]).keys().copied().collect::<Vec<_>>(),
            [-HOUR]
        );
    }

    #[test]
    fn the_rolling_mean_skips_missing_hours() {
        let values = [Some(1.0), None, Some(3.0), Some(5.0), None, None, None];
        let smooth = rolling(&values, 3);
        assert_eq!(
            smooth,
            [
                Some(1.0),
                Some(1.0),
                Some(2.0),
                Some(4.0),
                Some(4.0),
                Some(5.0),
                None
            ]
        );
        assert_eq!(
            rolling(&values, 0),
            rolling(&values, 1),
            "a window under one is one"
        );
        assert!(rolling(&[], 3).is_empty());
    }

    #[test]
    fn pearson_and_spearman_agree_on_a_line_and_differ_on_a_curve() {
        let x: Vec<f64> = (1..=10).map(f64::from).collect();
        let line: Vec<f64> = x.iter().map(|v| 3.0 - 2.0 * v).collect();
        assert!(close(pearson(&x, &line), -1.0));
        assert!(close(spearman(&x, &line), -1.0));
        let curve: Vec<f64> = x.iter().map(|v| v.powi(4)).collect();
        assert!(close(spearman(&x, &curve), 1.0));
        assert!(pearson(&x, &curve).is_some_and(|r| r < 0.95));
    }

    #[test]
    fn a_correlation_needs_enough_pairs_and_spread() {
        let few = [1.0, 2.0, 3.0];
        assert_eq!(pearson(&few, &few), None);
        let flat = [2.0; 8];
        let up: Vec<f64> = (0..8).map(f64::from).collect();
        assert_eq!(pearson(&flat, &up), None);
        assert_eq!(spearman(&flat, &up), None);
    }

    #[test]
    fn ties_share_their_mean_rank() {
        assert_eq!(ranks(&[10.0, 20.0, 20.0, 5.0]), [2.0, 3.5, 3.5, 1.0]);
        assert!(ranks(&[]).is_empty());
    }

    #[test]
    fn a_reading_far_from_the_rest_is_an_outlier_by_mad() {
        let mut values: Vec<Option<f64>> = [10.0, 11.0, 9.0, 10.5, 9.5, 10.0, 11.0]
            .into_iter()
            .map(Some)
            .collect();
        values.push(Some(60.0));
        values.push(None);
        assert_eq!(outliers(&values, 3.5), [7]);
        assert!(
            outliers(&[Some(5.0); 10], 3.5).is_empty(),
            "no spread, no outlier"
        );
        assert!(
            outliers(&[Some(1.0), Some(100.0)], 3.5).is_empty(),
            "too few to say"
        );
    }

    #[test]
    fn windy_hours_with_clean_air_correlate_negatively() {
        // Twelve hours: the wind rises, PM2.5 falls; temperature is absent for half of them.
        let wind: Vec<Point> = (0..12)
            .map(|h| (T0 + h * HOUR + 5 * 60_000, 1.0 + h as f64))
            .collect();
        let pm25: Vec<Point> = (0..12)
            .map(|h| (T0 + h * HOUR, 30.0 - 2.0 * h as f64))
            .collect();
        let temperature: Vec<Point> = (0..6).map(|h| (T0 + h * HOUR, 8.0)).collect();
        let out = run(&Input {
            air: BTreeMap::from([("pm25".to_owned(), pm25)]),
            weather: BTreeMap::from([
                ("windSpeed".to_owned(), wind),
                ("temperature".to_owned(), temperature),
            ]),
            settings: Settings {
                window: 1,
                threshold: 3.5,
            },
        });
        assert_eq!(out.hours.len(), 12);
        let wind = out
            .pairs
            .iter()
            .find(|p| p.weather == "windSpeed")
            .expect("pair");
        assert_eq!(wind.n, 12);
        assert!(close(wind.pearson, -1.0) && close(wind.spearman, -1.0));
        let temperature = out
            .pairs
            .iter()
            .find(|p| p.weather == "temperature")
            .expect("pair");
        assert_eq!(
            (temperature.n, temperature.pearson),
            (6, None),
            "a constant temperature says nothing"
        );
    }

    #[test]
    fn no_readings_give_no_hours_and_a_pair_of_nothing() {
        let out = run(&Input {
            air: BTreeMap::from([("pm10".to_owned(), Vec::new())]),
            weather: BTreeMap::from([("windSpeed".to_owned(), Vec::new())]),
            settings: Settings::default(),
        });
        assert!(out.hours.is_empty());
        assert_eq!(
            out.pairs,
            [Pair {
                air: "pm10".into(),
                weather: "windSpeed".into(),
                n: 0,
                pearson: None,
                spearman: None
            }]
        );
    }

    #[test]
    fn a_broken_timestamp_cannot_ask_for_millions_of_hours() {
        let out = run(&Input {
            air: BTreeMap::from([("pm10".to_owned(), vec![(0, 1.0), (T0, 2.0)])]),
            weather: BTreeMap::new(),
            settings: Settings::default(),
        });
        assert_eq!(out.hours.len() as i64, MAX_HOURS);
        assert_eq!(out.hours.last(), Some(&T0));
        assert_eq!(out.air[0].hourly.last(), Some(&Some(2.0)));
    }

    #[test]
    fn the_entry_answers_json_and_names_what_it_could_not_read() {
        let answer: serde_json::Value =
            serde_json::from_str(&analyse(r#"{"air":{},"weather":{}}"#)).expect("json");
        assert_eq!(answer["hours"], serde_json::json!([]));
        let refused: serde_json::Value =
            serde_json::from_str(&analyse(r#"{"air":{"pm10":[["x",1]]},"weather":{}}"#))
                .expect("json");
        assert!(refused["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("the readings could not be read"));
    }
}
