//! One indicator's history onto a regular clock, and Holt-Winters (additive) on it: the level, the
//! trend and, when the history is fine enough, the daily or weekly season, each smoothing weight
//! picked by the smallest one-step-ahead error.

const HOUR: f64 = 3_600_000.0;
const DAY: f64 = 24.0 * HOUR;

/// At most this many steps on the regular clock: a longer history is read at a coarser step.
pub const MAX_STEPS: usize = 2000;

/// A history on a regular clock: `values[i]` is the value at `start + i · step` (milliseconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub start: f64,
    pub step: f64,
    pub values: Vec<f64>,
}

impl Grid {
    pub fn time(&self, i: usize) -> f64 {
        self.start + i as f64 * self.step
    }
}

/// The points (sorted, distinct times, finite) read at the median step between them, each step's
/// value interpolated between the points around it. `None` for fewer than two points.
pub fn regularise(times: &[f64], values: &[f64]) -> Option<Grid> {
    let n = times.len().min(values.len());
    if n < 2 {
        return None;
    }
    let gaps: Vec<f64> = times[..n].windows(2).map(|w| w[1] - w[0]).collect();
    let mut step = crate::stats::median(&gaps)?;
    let span = times[n - 1] - times[0];
    if step <= 0.0 || span <= 0.0 {
        return None;
    }
    if span / step > (MAX_STEPS - 1) as f64 {
        step = span / (MAX_STEPS - 1) as f64;
    }
    // A hair under the span: a last point a rounding error short of a whole step still counts.
    let count = ((span + step * 1e-9) / step).floor() as usize + 1;
    let mut out = Vec::with_capacity(count);
    let mut j = 0;
    for i in 0..count {
        let t = times[0] + i as f64 * step;
        while j + 1 < n - 1 && times[j + 1] < t {
            j += 1;
        }
        let (t0, t1) = (times[j], times[j + 1]);
        let share = if t1 > t0 {
            ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        out.push(values[j] + share * (values[j + 1] - values[j]));
    }
    Some(Grid {
        start: times[0],
        step,
        values: out,
    })
}

/// The season the clock can carry: a day of hourly (or finer) steps, or a week of daily steps,
/// when the history holds two of them; 0 for none.
pub fn season_length(step: f64, steps: usize) -> usize {
    let length = if step <= HOUR {
        (DAY / step).round() as usize
    } else if (0.5 * DAY..=1.5 * DAY).contains(&step) {
        7
    } else {
        0
    };
    if length >= 2 && steps >= 2 * length {
        length
    } else {
        0
    }
}

/// The three smoothing weights: level, trend, season.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Weights {
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
}

/// A fitted model: where it ended, and what it expected one step ahead at each step it saw.
#[derive(Debug, Clone)]
pub struct Model {
    pub weights: Weights,
    pub season: usize,
    level: f64,
    trend: f64,
    seasonal: Vec<f64>,
    steps: usize,
    /// The one-step-ahead expectation per step; `None` for the steps that seeded the model.
    pub expected: Vec<Option<f64>>,
    /// The root mean square of the one-step errors.
    pub sigma: f64,
    /// What the weights were chosen by: the squared one-step errors, each clipped.
    loss: f64,
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// Holt-Winters additive over `y` with these weights and season (0: Holt's linear trend). A
/// one-step error larger than `clip` updates the model as if it were `clip` (robust Holt-Winters):
/// one reading far off moves the level a little, not by its whole distance; `f64::INFINITY`
/// clips nothing. `None` when `y` is too short to seed it: two seasons, or three steps without one.
pub fn fit(y: &[f64], season: usize, weights: Weights, clip: f64) -> Option<Model> {
    let Weights { alpha, beta, gamma } = weights;
    let (mut level, mut trend, mut seasonal, start) = if season > 0 {
        if y.len() < 2 * season {
            return None;
        }
        let first = mean(&y[..season]);
        let second = mean(&y[season..2 * season]);
        let seasonal: Vec<f64> = y[..season].iter().map(|v| v - first).collect();
        (first, (second - first) / season as f64, seasonal, season)
    } else {
        if y.len() < 3 {
            return None;
        }
        (y[0], y[1] - y[0], vec![0.0], 1)
    };
    let width = seasonal.len();
    let mut expected = vec![None; y.len()];
    let (mut sse, mut loss) = (0.0, 0.0);
    for (i, &raw) in y.iter().enumerate().skip(start) {
        let s = if season > 0 { seasonal[i % width] } else { 0.0 };
        let guess = level + trend + s;
        expected[i] = Some(guess);
        sse += (raw - guess).powi(2);
        let error = (raw - guess).clamp(-clip, clip);
        loss += error * error;
        let value = guess + error;
        let next_level = alpha * (value - s) + (1.0 - alpha) * (level + trend);
        trend = beta * (next_level - level) + (1.0 - beta) * trend;
        if season > 0 {
            seasonal[i % width] = gamma * (value - next_level) + (1.0 - gamma) * s;
        }
        level = next_level;
    }
    let errors = y.len() - start;
    Some(Model {
        weights,
        season,
        level,
        trend,
        seasonal,
        steps: y.len(),
        expected,
        sigma: (sse / errors as f64).sqrt(),
        loss,
    })
}

/// The usual size of a step's change, robustly: the spread of the differences a season apart (a
/// step apart without one, `stats::robust_sd`), as one standard deviation of a single reading.
pub fn robust_scale(y: &[f64], season: usize) -> f64 {
    let lag = season.max(1);
    let differences: Vec<f64> = y.windows(lag + 1).map(|w| w[lag] - w[0]).collect();
    crate::stats::robust_sd(&differences).map_or(0.0, |sd| sd / std::f64::consts::SQRT_2)
}

const ALPHAS: [f64; 8] = [0.05, 0.1, 0.2, 0.3, 0.5, 0.7, 0.9, 0.99];
const BETAS: [f64; 6] = [0.0, 0.01, 0.05, 0.1, 0.2, 0.3];
const GAMMAS: [f64; 5] = [0.05, 0.1, 0.2, 0.3, 0.5];

/// The model with the smallest clipped one-step error over a grid of weights (ties keep the
/// smoother). Errors are clipped at three robust scales; a series whose steps never vary has a
/// scale of 0 and is not clipped, so it still follows a real shift.
// ponytail: grid search, 240 fits of O(n); a Nelder-Mead fit only if the grid's coarseness shows.
pub fn best_fit(y: &[f64], season: usize) -> Option<Model> {
    let gammas: &[f64] = if season > 0 { &GAMMAS } else { &[0.0] };
    let clip = match 3.0 * robust_scale(y, season) {
        c if c > 0.0 => c,
        _ => f64::INFINITY,
    };
    let mut best: Option<Model> = None;
    for &alpha in &ALPHAS {
        for &beta in &BETAS {
            for &gamma in gammas {
                let model = fit(y, season, Weights { alpha, beta, gamma }, clip)?;
                if best.as_ref().is_none_or(|b| model.loss < b.loss) {
                    best = Some(model);
                }
            }
        }
    }
    best
}

/// One forecast step: the value expected and its 95 % interval.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Ahead {
    pub t: f64,
    pub v: f64,
    pub lo: f64,
    pub hi: f64,
}

impl Model {
    /// The next `horizon` steps after the grid's last, the interval `1.96 · sigma` widening with √h.
    pub fn forecast(&self, grid: &Grid, horizon: usize, sigma: f64) -> Vec<Ahead> {
        let width = self.seasonal.len();
        (1..=horizon)
            .map(|h| {
                let s = if self.season > 0 {
                    self.seasonal[(self.steps + h - 1) % width]
                } else {
                    0.0
                };
                let v = self.level + h as f64 * self.trend + s;
                let half = 1.96 * sigma * (h as f64).sqrt();
                Ahead {
                    t: grid.time(self.steps - 1 + h),
                    v,
                    lo: v - half,
                    hi: v + half,
                }
            })
            .collect()
    }

    /// What the model expected at time `t`, between the two steps around it; `None` before the
    /// model was seeded or outside the grid.
    pub fn expected_at(&self, grid: &Grid, t: f64) -> Option<f64> {
        let at = (t - grid.start) / grid.step;
        if !(0.0..=(self.steps - 1) as f64).contains(&at) {
            return None;
        }
        let i = at.floor() as usize;
        let j = (i + 1).min(self.steps - 1);
        let (a, b) = (self.expected[i]?, self.expected[j]?);
        Some(a + (at - i as f64) * (b - a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_regular_series_keeps_its_points() {
        let grid = regularise(&[0.0, 10.0, 20.0, 30.0], &[1.0, 2.0, 3.0, 4.0]).expect("a grid");
        assert_eq!((grid.start, grid.step), (0.0, 10.0));
        assert_eq!(grid.values, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_gap_is_filled_between_its_neighbours() {
        let grid = regularise(&[0.0, 10.0, 20.0, 50.0], &[0.0, 1.0, 2.0, 5.0]).expect("a grid");
        assert_eq!(grid.step, 10.0);
        assert_eq!(grid.values, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn a_long_history_is_read_at_a_coarser_step() {
        let times: Vec<f64> = (0..10_000).map(f64::from).collect();
        let grid = regularise(&times, &times).expect("a grid");
        assert_eq!(grid.values.len(), MAX_STEPS);
        assert!((grid.values[MAX_STEPS - 1] - 9_999.0).abs() < 1e-6);
    }

    #[test]
    fn no_grid_for_one_point() {
        assert_eq!(regularise(&[5.0], &[1.0]), None);
        assert_eq!(regularise(&[], &[]), None);
    }

    #[test]
    fn the_season_follows_the_step_and_the_length() {
        assert_eq!(season_length(HOUR, 48), 24);
        assert_eq!(season_length(HOUR, 47), 0);
        assert_eq!(season_length(DAY, 14), 7);
        assert_eq!(season_length(DAY, 10), 0);
        assert_eq!(season_length(6.0 * HOUR, 1000), 0);
    }

    #[test]
    fn holt_follows_a_straight_line_exactly() {
        let y: Vec<f64> = (0..20).map(|i| 3.0 + 2.0 * f64::from(i)).collect();
        let grid = Grid {
            start: 0.0,
            step: 1.0,
            values: y.clone(),
        };
        let model = best_fit(&y, 0).expect("a model");
        assert!(model.sigma < 1e-9);
        let ahead = model.forecast(&grid, 3, model.sigma);
        assert!((ahead[0].v - 43.0).abs() < 1e-9 && (ahead[2].v - 47.0).abs() < 1e-9);
        assert_eq!(ahead[0].t, 20.0);
    }

    #[test]
    fn holt_winters_carries_a_daily_season_forward() {
        let shape = |i: usize| if i % 24 < 12 { 10.0 } else { 20.0 };
        let y: Vec<f64> = (0..24 * 7).map(shape).collect();
        let grid = Grid {
            start: 0.0,
            step: HOUR,
            values: y.clone(),
        };
        let model = best_fit(&y, 24).expect("a model");
        let ahead = model.forecast(&grid, 24, model.sigma);
        for (h, step) in ahead.iter().enumerate() {
            assert!(
                (step.v - shape(y.len() + h)).abs() < 0.5,
                "step {h}: {}",
                step.v
            );
            assert!(step.lo <= step.v && step.v <= step.hi);
        }
    }

    #[test]
    fn the_robust_scale_ignores_one_wild_step_and_the_season() {
        let rhythm: Vec<f64> = (0..96)
            .map(|i| if i % 24 < 12 { 10.0 } else { 20.0 })
            .collect();
        assert_eq!(robust_scale(&rhythm, 24), 0.0);
        let wobble: Vec<f64> = (0..40).map(|i| f64::from((i * 37) % 11)).collect();
        let mut wild = wobble.clone();
        wild[20] = 1000.0;
        let (calm, spiked) = (robust_scale(&wobble, 0), robust_scale(&wild, 0));
        assert!(
            calm > 0.0 && (spiked - calm).abs() < 0.3 * calm,
            "{spiked} against {calm}"
        );
        assert_eq!(robust_scale(&[], 0), 0.0);
    }

    #[test]
    fn a_clipped_error_moves_the_level_by_the_clip_only() {
        let y = [10.0, 10.0, 10.0, 110.0, 10.0];
        let weights = Weights {
            alpha: 1.0,
            beta: 0.0,
            gamma: 0.0,
        };
        let free = fit(&y, 0, weights, f64::INFINITY).expect("a model");
        let held = fit(&y, 0, weights, 5.0).expect("a model");
        assert_eq!(free.expected[4], Some(110.0));
        assert_eq!(held.expected[4], Some(15.0));
    }

    #[test]
    fn too_short_to_seed_is_none() {
        assert!(fit(
            &[1.0, 2.0],
            0,
            Weights {
                alpha: 0.5,
                beta: 0.1,
                gamma: 0.0
            },
            f64::INFINITY
        )
        .is_none());
        assert!(best_fit(&[1.0; 30], 24).is_none());
    }

    #[test]
    fn the_expectation_between_steps_is_interpolated() {
        let y: Vec<f64> = (0..10).map(f64::from).collect();
        let grid = Grid {
            start: 100.0,
            step: 10.0,
            values: y.clone(),
        };
        let model = fit(
            &y,
            0,
            Weights {
                alpha: 1.0,
                beta: 1.0,
                gamma: 0.0,
            },
            f64::INFINITY,
        )
        .expect("a model");
        assert_eq!(
            model.expected_at(&grid, 100.0),
            None,
            "the seed step expects nothing"
        );
        assert!((model.expected_at(&grid, 155.0).expect("between") - 5.5).abs() < 1e-9);
        assert_eq!(model.expected_at(&grid, 500.0), None);
    }
}
