//! A source's health from its kept runs (T-5048, PL-62): when it last delivered, how many records
//! its newest run sent against the median of the week before, and the last problem its log holds.
//!
//! A run *fails* when it sent nothing and its log has failed or rejected lines: the source or a
//! step broke. A run that sent nothing and reported nothing found nothing. A pipeline is `failing`
//! as soon as its newest run failed, and the alert check names it after [`ALERT_STREAK`] failed
//! runs in a row or a newest run that sent [`DROP_SHARE`] or less of the week's median.
//!
//! A run that sent records can still lose some: a step threw on them or the model refused them.
//! Over the runs of the `ERROR_WINDOW` ending at the newest one, those failed and rejected
//! records against every record the runs took is the error ratio (T-5291); above
//! [`ERROR_SHARE`] the pipeline is `failing` and alerted at once. A run that sent nothing stays
//! with the streak, so one broken run alone still alerts nobody.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use crate::pipeline_log::Run;

/// Failed runs in a row before the check opens an incident.
pub const ALERT_STREAK: usize = 3;
/// A newest run at or under this share of the 7-day median is a drop (more than 80 % less).
pub const DROP_SHARE: f64 = 0.2;
/// The window of the median.
const WEEK: Duration = Duration::days(7);
/// Failed and rejected records above this share of the window's records alert (T-5291).
pub const ERROR_SHARE: f64 = 0.05;
/// The runs the error ratio counts: those that ended within this time of the newest run.
const ERROR_WINDOW: Duration = Duration::hours(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = PipelineHealthState)]
pub enum State {
    /// Its newest run sent records, near the week's median or above it.
    Ok,
    /// Its newest run found nothing to send, and the week before sent nothing either.
    Empty,
    /// Its newest run sent more than 80 % less than the week's median.
    Drift,
    /// Its newest run sent nothing and its log says why, or more than [`ERROR_SHARE`] of the
    /// last hour's records failed or were rejected.
    Failing,
    /// No run is kept: it never ran, or not since its runs aged out.
    Never,
}

/// What the health view shows for one pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineHealth)]
pub struct Health {
    pub pipeline: String,
    pub state: State,
    /// The newest run that sent at least one record, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<String>,
    /// The newest run, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    /// How many records the newest run sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sent: Option<u64>,
    /// The median of what the runs of the 7 days before the newest one sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_sent: Option<f64>,
    /// The newest run against that median, in percent: -100 sent nothing, 0 the same.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_percent: Option<i64>,
    /// Failed runs in a row, newest first.
    pub failed_runs: usize,
    /// The newest failed or rejected line of the log, URLs without their query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// When that line was taken, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<String>,
    /// The failed and rejected records of the hour ending at the newest run, among the runs that
    /// sent something; none when those runs took no record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<ErrorRatio>,
    /// The check names it: failed [`ALERT_STREAK`] runs in a row, a drop, or an error ratio above
    /// [`ERROR_SHARE`].
    pub alert: bool,
    /// The newest weekly link check of its entities' links (T-5133); none before the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<crate::link_check::Links>,
}

/// How many of the records the runs of an hour took did not reach the space (T-5291).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineErrorRatio)]
pub struct ErrorRatio {
    /// Records a step threw on or the model refused.
    pub errors: u64,
    /// Every record the runs took: sent, failed and rejected.
    pub records: u64,
    /// `errors` of `records`, in percent, one decimal.
    pub percent: f64,
}

impl ErrorRatio {
    /// Whether the share is past [`ERROR_SHARE`].
    pub fn over(&self) -> bool {
        self.errors as f64 > self.records as f64 * ERROR_SHARE
    }
}

/// The error ratio of the runs within `ERROR_WINDOW` of the newest one that sent something.
fn error_ratio(runs: &[Run]) -> Option<ErrorRatio> {
    let end = runs.first().and_then(|run| at(&run.last_at))?;
    let (errors, records) = runs
        .iter()
        .filter(|run| run.sent > 0 && at(&run.last_at).is_some_and(|t| t >= end - ERROR_WINDOW))
        .fold((0u64, 0u64), |(errors, records), run| {
            let lost = run.failed.saturating_add(run.rejected);
            (
                errors.saturating_add(lost),
                records.saturating_add(run.sent).saturating_add(lost),
            )
        });
    (records > 0).then(|| ErrorRatio {
        errors,
        records,
        percent: (errors as f64 * 1000.0 / records as f64).round() / 10.0,
    })
}

fn failed(run: &Run) -> bool {
    run.sent == 0 && run.failed + run.rejected > 0
}

fn at(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn median(mut values: Vec<u64>) -> Option<f64> {
    values.sort_unstable();
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        n if n % 2 == 1 => Some(values[middle] as f64),
        _ => Some((values[middle - 1] + values[middle]) as f64 / 2.0),
    }
}

/// The health of a pipeline from its runs, newest first as [`crate::pipeline_log::LogStore::runs`]
/// lists them, and the newest problem line of its log with when it was taken.
pub fn health(pipeline: &str, runs: &[Run], problem: Option<(String, String)>) -> Health {
    let newest = runs.first();
    let failed_runs = runs.iter().take_while(|run| failed(run)).count();
    let week_before = newest.and_then(|run| at(&run.last_at)).map(|end| {
        runs.iter()
            .skip(1)
            .filter(|run| at(&run.last_at).is_some_and(|t| t >= end - WEEK))
            .map(|run| run.sent)
            .collect::<Vec<_>>()
    });
    let median_sent = week_before.and_then(median);
    let change_percent = match (newest, median_sent) {
        (Some(run), Some(m)) if m > 0.0 => Some(((run.sent as f64 - m) / m * 100.0).round() as i64),
        _ => None,
    };
    // A failed run is counted by the streak, not as a drop: one broken run alone alerts nobody.
    let dropped = matches!((newest, median_sent), (Some(run), Some(m))
        if !failed(run) && m > 0.0 && (run.sent as f64) <= m * DROP_SHARE);
    let errors = error_ratio(runs);
    let erring = errors.is_some_and(|ratio| ratio.over());
    let state = match newest {
        None => State::Never,
        Some(run) if failed(run) || erring => State::Failing,
        Some(_) if dropped => State::Drift,
        Some(run) if run.sent == 0 => State::Empty,
        Some(_) => State::Ok,
    };
    let (last_error, last_error_at) = match problem {
        Some((message, at)) => (Some(without_queries(&message)), Some(at)),
        None => (None, None),
    };
    Health {
        pipeline: pipeline.to_owned(),
        state,
        last_success_at: runs
            .iter()
            .find(|run| run.sent > 0)
            .map(|run| run.last_at.clone()),
        last_run_at: newest.map(|run| run.last_at.clone()),
        last_sent: newest.map(|run| run.sent),
        median_sent,
        change_percent,
        failed_runs,
        last_error,
        last_error_at,
        errors,
        alert: failed_runs >= ALERT_STREAK || dropped || erring,
        links: None,
    }
}

/// The health of each of `names`, in that order, from the store's runs and log.
pub async fn of_pipelines(
    log: &crate::pipeline_log::LogStore,
    project: &str,
    names: &[String],
) -> Result<Vec<Health>, sqlx::Error> {
    let mut items = Vec::with_capacity(names.len());
    for name in names {
        let runs = log
            .runs(project, name, crate::pipeline_log::RUNS_KEPT)
            .await?;
        let problem = log.last_problem(project, name).await?;
        items.push(health(name, &runs, problem));
    }
    Ok(items)
}

/// `text` with the query and fragment cut from every http(s) URL in it, where a key could ride;
/// the store masked credential-shaped words already.
pub fn without_queries(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            match lower.find("http://").or_else(|| lower.find("https://")) {
                Some(start) => match word[start..].find(['?', '#']) {
                    Some(cut) => &word[..start + cut],
                    None => word,
                },
                None => word,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(hours_ago: i64, sent: u64, failed: u64) -> Run {
        let at = (Utc::now() - Duration::hours(hours_ago)).to_rfc3339();
        Run {
            run: at.clone(),
            first_at: at.clone(),
            last_at: at,
            sent,
            rejected: 0,
            failed,
            created: None,
            updated: None,
            unchanged: None,
        }
    }

    #[test]
    fn a_source_near_its_median_is_ok() {
        let runs = [
            run(0, 95, 0),
            run(24, 100, 0),
            run(48, 90, 0),
            run(72, 110, 0),
        ];
        let h = health("podujatia-detva", &runs, None);
        assert_eq!(h.state, State::Ok);
        assert_eq!(h.median_sent, Some(100.0));
        assert_eq!(h.change_percent, Some(-5));
        assert_eq!(h.last_sent, Some(95));
        assert_eq!(h.last_success_at.as_deref(), Some(runs[0].last_at.as_str()));
        assert!(!h.alert);
    }

    #[test]
    fn a_source_that_never_had_records_is_empty_not_alerted() {
        let runs = [run(0, 0, 0), run(24, 0, 0), run(48, 0, 0)];
        let h = health("podujatia-jelsava", &runs, None);
        assert_eq!(h.state, State::Empty);
        assert_eq!(h.last_success_at, None);
        assert_eq!(h.change_percent, None);
        assert_eq!(h.failed_runs, 0);
        assert!(!h.alert);
        assert_eq!(health("x", &[], None).state, State::Never);
    }

    #[test]
    fn one_failed_run_is_red_and_three_in_a_row_alert() {
        let problem = Some((
            "GET https://example.org/feed?apikey=s3cr3t answered 404".to_owned(),
            "2026-10-21T00:00:00Z".to_owned(),
        ));
        let once = [run(0, 0, 1), run(24, 40, 0), run(48, 40, 0)];
        let h = health("podujatia-zvolen", &once, problem.clone());
        assert_eq!(h.state, State::Failing);
        assert_eq!(h.failed_runs, 1);
        assert!(!h.alert);
        assert_eq!(
            h.last_error.as_deref(),
            Some("GET https://example.org/feed answered 404")
        );
        assert_eq!(h.last_success_at.as_deref(), Some(once[1].last_at.as_str()));
        let thrice = [run(0, 0, 1), run(24, 0, 2), run(48, 0, 1), run(72, 40, 0)];
        let h = health("podujatia-zvolen", &thrice, problem);
        assert_eq!(h.failed_runs, 3);
        assert!(h.alert);
    }

    #[test]
    fn a_drop_past_80_percent_of_the_week_median_alerts() {
        let runs = [
            run(0, 19, 0),
            run(24, 100, 0),
            run(48, 100, 0),
            run(200, 1, 0),
        ];
        let h = health("podujatia-lucenec", &runs, None);
        assert_eq!(h.state, State::Drift);
        assert_eq!(
            h.median_sent,
            Some(100.0),
            "the run 8 days back is outside the week"
        );
        assert_eq!(h.change_percent, Some(-81));
        assert!(h.alert);
        // A source that read nothing after a week of records drifted by 100 %.
        let h = health("podujatia-revuca", &[run(0, 0, 0), run(24, 30, 0)], None);
        assert_eq!(
            (h.state, h.change_percent, h.alert),
            (State::Drift, Some(-100), true)
        );
        // 21 of 100 is a drop of 79 %: no alert.
        let h = health("p", &[run(0, 21, 0), run(24, 100, 0)], None);
        assert_eq!((h.state, h.alert), (State::Ok, false));
    }

    #[test]
    fn a_tenth_of_one_run_s_records_failing_alerts_and_five_percent_does_not() {
        // 90 sent, 6 thrown on by a step, 4 refused by the model: 10 of 100.
        let bad = Run {
            rejected: 4,
            ..run(0, 90, 6)
        };
        let h = health("pipeline-test-helsinki", &[bad, run(24, 100, 0)], None);
        assert_eq!(
            h.errors,
            Some(ErrorRatio {
                errors: 10,
                records: 100,
                percent: 10.0
            })
        );
        assert_eq!((h.state, h.alert, h.failed_runs), (State::Failing, true, 0));
        // 5 of 100 is at the threshold, not past it.
        let h = health("p", &[run(0, 95, 5), run(24, 100, 0)], None);
        assert_eq!(h.errors.map(|e| e.percent), Some(5.0));
        assert_eq!((h.state, h.alert), (State::Ok, false));
    }

    #[test]
    fn the_ratio_counts_the_hour_of_runs_that_sent_something() {
        // The run half an hour back counts; the one two hours back and a run that sent nothing
        // (the streak's business) do not.
        let mut half = run(0, 40, 4);
        half.last_at = (Utc::now() - Duration::minutes(30)).to_rfc3339();
        let runs = [
            run(0, 60, 0),
            half,
            run(0, 0, 50),
            run(2, 0, 500),
            run(2, 10, 90),
        ];
        let h = health("helsinki.hsl-hfp-vehicles", &runs, None);
        assert_eq!(
            h.errors,
            Some(ErrorRatio {
                errors: 4,
                records: 104,
                percent: 3.8
            })
        );
        assert!(!h.alert);
        // No record taken in the hour: no ratio, and one failed run alone still alerts nobody.
        let h = health("p", &[run(0, 0, 3)], None);
        assert_eq!((h.errors, h.state, h.alert), (None, State::Failing, false));
    }

    #[test]
    fn queries_are_cut_from_every_url_only() {
        assert_eq!(
            without_queries("fetch (HTTPS://a.sk/x?token=1#f) and http://b.sk/y#frag ok?"),
            "fetch (HTTPS://a.sk/x and http://b.sk/y ok?"
        );
    }
}
