//! What the example computes in the browser (T-3327): the summary of one numeric attribute over
//! the entities the SDK fetched. Pure Rust, no allocation beyond the sort, no threads.

use wasm_bindgen::prelude::*;

/// `[count, min, max, mean, median]` of the finite values; `NaN` for each but `count` when there
/// is none. A value that is not finite (`NaN`, an infinity) is left out, never counted.
#[wasm_bindgen]
pub fn summarize(values: &[f64]) -> Vec<f64> {
    let mut finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return vec![0.0, f64::NAN, f64::NAN, f64::NAN, f64::NAN];
    }
    finite.sort_by(f64::total_cmp);
    let n = finite.len();
    let mean = finite.iter().sum::<f64>() / n as f64;
    let median = if n % 2 == 1 {
        finite[n / 2]
    } else {
        (finite[n / 2 - 1] + finite[n / 2]) / 2.0
    };
    vec![n as f64, finite[0], finite[n - 1], mean, median]
}

#[cfg(test)]
mod tests {
    use super::summarize;

    #[test]
    fn a_summary_of_the_finite_values() {
        assert_eq!(summarize(&[3.0, 1.0, 2.0]), vec![3.0, 1.0, 3.0, 2.0, 2.0]);
        assert_eq!(summarize(&[4.0, 1.0, 3.0, 2.0]), vec![4.0, 1.0, 4.0, 2.5, 2.5]);
        assert_eq!(summarize(&[f64::NAN, 5.0, f64::INFINITY]), vec![1.0, 5.0, 5.0, 5.0, 5.0]);
    }

    #[test]
    fn nothing_to_summarize_is_a_count_of_zero() {
        let empty = summarize(&[]);
        assert_eq!(empty[0], 0.0);
        assert!(empty[1..].iter().all(|v| v.is_nan()));
        assert_eq!(summarize(&[f64::NAN])[0], 0.0);
    }
}
