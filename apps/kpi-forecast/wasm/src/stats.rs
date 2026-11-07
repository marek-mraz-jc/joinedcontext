//! The few statistics the forecast needs: a median, quantiles, a robust standard deviation and a
//! straight line through points by least squares.

/// The median of the values, or `None` for none. NaN never reaches here (the input is cleaned).
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

/// The `q` quantile (0 to 1) of the values, between the two nearest by linear interpolation;
/// `None` for none.
pub fn quantile(values: &[f64], q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    Some(sorted[low] + (at - low as f64) * (sorted[high] - sorted[low]))
}

/// One standard deviation, robustly: the interquartile range over 1.349. A quarter of the values
/// may be wild before it moves, and unlike a MAD it does not read 0 for values that take two
/// levels. `None` for none.
pub fn robust_sd(values: &[f64]) -> Option<f64> {
    Some((quantile(values, 0.75)? - quantile(values, 0.25)?) / 1.349)
}

/// A straight line `y = intercept + slope · x` and how much of the variation it explains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub slope: f64,
    pub intercept: f64,
    /// R², 0 to 1; 1 for a series that does not vary at all, which the line explains fully.
    pub r2: f64,
}

/// The least-squares line through the points, or `None` for fewer than two distinct x.
pub fn fit_line(xs: &[f64], ys: &[f64]) -> Option<Line> {
    let n = xs.len().min(ys.len());
    if n < 2 {
        return None;
    }
    let mean_x = xs[..n].iter().sum::<f64>() / n as f64;
    let mean_y = ys[..n].iter().sum::<f64>() / n as f64;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (dx, dy) = (xs[i] - mean_x, ys[i] - mean_y);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    if sxx == 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let r2 = if syy == 0.0 {
        1.0
    } else {
        (sxy * sxy / (sxx * syy)).clamp(0.0, 1.0)
    };
    Some(Line {
        slope,
        intercept: mean_y - slope * mean_x,
        r2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_none_one_odd_and_even() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[4.0]), Some(4.0));
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    #[test]
    fn quantiles_between_the_values() {
        assert_eq!(quantile(&[], 0.5), None);
        assert_eq!(quantile(&[3.0, 1.0, 2.0, 4.0, 5.0], 0.25), Some(2.0));
        assert_eq!(quantile(&[1.0, 2.0], 0.5), Some(1.5));
        assert_eq!(quantile(&[7.0], 0.9), Some(7.0));
    }

    #[test]
    fn robust_sd_ignores_one_wild_value_and_sees_two_levels() {
        assert_eq!(robust_sd(&[]), None);
        assert_eq!(robust_sd(&[5.0, 5.0, 5.0, 5.0, 900.0]), Some(0.0));
        let two_levels = [4.0, 4.0, 4.0, -7.0, -7.0, 4.0, 4.0, -7.0];
        assert!(robust_sd(&two_levels).expect("a spread") > 0.0);
    }

    #[test]
    fn line_through_exact_points_and_a_flat_series() {
        let line = fit_line(&[0.0, 1.0, 2.0], &[1.0, 3.0, 5.0]).expect("a line");
        assert!((line.slope - 2.0).abs() < 1e-12 && (line.intercept - 1.0).abs() < 1e-12);
        assert!((line.r2 - 1.0).abs() < 1e-12);
        let flat = fit_line(&[0.0, 1.0, 2.0], &[7.0, 7.0, 7.0]).expect("a line");
        assert_eq!((flat.slope, flat.r2), (0.0, 1.0));
    }

    #[test]
    fn no_line_through_one_point_or_one_x() {
        assert_eq!(fit_line(&[1.0], &[1.0]), None);
        assert_eq!(fit_line(&[2.0, 2.0], &[1.0, 3.0]), None);
    }
}
