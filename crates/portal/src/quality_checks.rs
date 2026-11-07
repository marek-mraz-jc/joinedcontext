//! A space's checks on write over 24 hours and 7 days (DM-94, API/01 §27.1, ADR-N-046 2.7).
//!
//! The gateway counts every check of a space's `spec.quality` rules in
//! `jc_quality_checks_total{space,type,rule,outcome}` since it started. A window needs two
//! readings: every replica reads the counters once an hour and on a request at most once a
//! minute, keeps them for eight days, and a window's count is what the counters grew by inside
//! it. Names and counts only: a counter holds no value of anyone's entity.
//!
//! ponytail: per-replica memory, like `pipeline_stats`; a restarted replica's windows start at
//! its first reading and say so in `windows`. A Prometheus query replaces the readings once
//! every deployment runs one (production does, dev does not).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use jc_core::kinds::QualityRule;
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::pipelines::{label, parse_line};

/// The gateway's counter (DM-93).
pub const METRIC: &str = "jc_quality_checks_total";
/// The two windows, by the name the answer gives them.
const DAY: chrono::Duration = chrono::Duration::hours(24);
const WEEK: chrono::Duration = chrono::Duration::days(7);
/// How long a reading is kept: the week and a day of slack.
const KEEP: chrono::Duration = chrono::Duration::days(8);
/// The gap between two kept readings, and the period of the background reading.
const HOURLY: chrono::Duration = chrono::Duration::hours(1);
/// A latest reading younger than this answers a request without asking the gateway again.
const MIN_GAP: chrono::Duration = chrono::Duration::seconds(60);
const TIMEOUT: Duration = Duration::from_secs(5);

/// One counter: space, type, rule, outcome.
type Key = (String, String, String, String);

/// What the gateway's counters said at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub at: DateTime<Utc>,
    pub counts: BTreeMap<Key, f64>,
}

/// The `jc_quality_checks_total` samples of a Prometheus text page; a sample missing a label is
/// skipped, not guessed.
pub fn parse(text: &str) -> BTreeMap<Key, f64> {
    text.lines()
        .filter_map(parse_line)
        .filter(|sample| sample.name == METRIC)
        .filter_map(|sample| {
            let key = (
                label(sample.labels, "space")?.to_owned(),
                label(sample.labels, "type")?.to_owned(),
                label(sample.labels, "rule")?.to_owned(),
                label(sample.labels, "outcome")?.to_owned(),
            );
            Some((key, sample.value))
        })
        .collect()
}

/// The checks of one rule in one window.
#[derive(Debug, Default, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub pass: u64,
    pub reject: u64,
    pub flag: u64,
    pub quarantine: u64,
    /// `pass` over all four; `null` when nothing was checked in the window.
    pub pass_rate: Option<f64>,
}

/// One rule of one type.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleChecks {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub rule: String,
    /// The attribute the manifest's rule reads; empty for a rule no longer declared.
    pub attribute: String,
    #[serde(rename = "24h")]
    pub day: Counts,
    #[serde(rename = "7d")]
    pub week: Counts,
    /// The reading at which the rule's `reject` counter last grew, inside seven days.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_rejected: Option<DateTime<Utc>>,
}

/// Where each window starts: its oldest reading.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct Windows {
    #[serde(rename = "24h")]
    #[schema(value_type = String, format = DateTime)]
    pub day: DateTime<Utc>,
    #[serde(rename = "7d")]
    #[schema(value_type = String, format = DateTime)]
    pub week: DateTime<Utc>,
}

/// The answer of `GET …/spaces/{space}/quality/checks`.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct SpaceChecks {
    /// The newest reading; `null` while there is none.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub at: Option<DateTime<Utc>>,
    pub windows: Option<Windows>,
    pub rules: Vec<RuleChecks>,
}

/// What a counter grew by from `base` to `now`; a counter that went down restarted from zero.
fn grew(now: f64, base: f64) -> f64 {
    if now >= base {
        now - base
    } else {
        now
    }
}

/// The readings one replica holds, and where it reads them.
#[derive(Debug)]
pub struct Readings {
    /// `{gateway}/metrics`; `None` without a gateway address, and every answer is empty.
    source: Option<(reqwest::Client, String)>,
    /// One reading an hour at most, oldest first, for `KEEP` (eight days).
    kept: Mutex<VecDeque<Reading>>,
    latest: Mutex<Option<Reading>>,
}

impl Readings {
    pub fn new(gateway_url: Option<String>) -> Self {
        let source = gateway_url.and_then(|url| {
            let http = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
            Some((http, format!("{}/metrics", url.trim_end_matches('/'))))
        });
        Self {
            source,
            kept: Mutex::default(),
            latest: Mutex::default(),
        }
    }

    /// Keeps `reading` as the latest, and in the hourly series when the last kept one is an
    /// hour older; drops what is older than `KEEP`.
    pub fn record(&self, reading: Reading) {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        if kept
            .back()
            .is_none_or(|last| reading.at - last.at >= HOURLY)
        {
            kept.push_back(reading.clone());
        }
        while kept
            .front()
            .is_some_and(|first| reading.at - first.at > KEEP)
        {
            kept.pop_front();
        }
        *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(reading);
    }

    /// Reads the gateway's counters and records them.
    async fn read(&self) -> Result<(), String> {
        let Some((http, url)) = &self.source else {
            return Ok(());
        };
        let response = http.get(url).send().await.map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("{url} answered {}", response.status()));
        }
        let text = response.text().await.map_err(|e| e.to_string())?;
        self.record(Reading {
            at: Utc::now(),
            counts: parse(&text),
        });
        Ok(())
    }

    /// Reads again when the latest reading is older than a minute. A gateway that does not
    /// answer leaves the readings as they are; the answer's `at` says how old they are.
    pub async fn refresh(&self) {
        let due = self
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(|latest| Utc::now() - latest.at >= MIN_GAP);
        if due {
            if let Err(err) = self.read().await {
                tracing::warn!(error = %err, "the gateway's quality counters were not read");
            }
        }
    }

    /// Reads once an hour for as long as the process runs, so the windows have their bases
    /// whether or not anyone opens the page.
    pub fn start_hourly(self: &Arc<Self>) {
        if self.source.is_none() {
            return;
        }
        let readings = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(HOURLY.to_std().unwrap_or(Duration::from_secs(3600)));
            loop {
                tick.tick().await;
                if let Err(err) = readings.read().await {
                    tracing::warn!(error = %err, "the gateway's quality counters were not read");
                }
            }
        });
    }

    /// The checks of `space` from the readings held, with the attribute of each rule `rules`
    /// declares; a declared rule nothing counted yet has a row of zeros.
    pub fn report(&self, space: &str, rules: &[QualityRule]) -> SpaceChecks {
        let Some(latest) = self
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return SpaceChecks {
                at: None,
                windows: None,
                rules: Vec::new(),
            };
        };
        let kept = self.kept.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // The oldest reading inside a window is its base; none inside means the window starts now.
        let base = |window: chrono::Duration| {
            kept.iter()
                .find(|reading| reading.at >= latest.at - window)
                .unwrap_or(&latest)
        };
        let (day, week) = (base(DAY), base(WEEK));

        let mut rows: BTreeMap<(String, String), String> = rules
            .iter()
            .map(|rule| {
                (
                    (rule.entity_type.clone(), rule.name.clone()),
                    rule.attribute.clone(),
                )
            })
            .collect();
        for (key_space, entity_type, rule, _) in latest.counts.keys() {
            if key_space == space {
                rows.entry((entity_type.clone(), rule.clone())).or_default();
            }
        }

        let count = |base: &Reading, entity_type: &str, rule: &str, outcome: &str| {
            let key = (
                space.to_owned(),
                entity_type.to_owned(),
                rule.to_owned(),
                outcome.to_owned(),
            );
            let now = latest.counts.get(&key).copied().unwrap_or(0.0);
            grew(now, base.counts.get(&key).copied().unwrap_or(0.0)) as u64
        };
        let counts = |base: &Reading, entity_type: &str, rule: &str| {
            let [pass, reject, flag, quarantine] = ["pass", "reject", "flag", "quarantine"]
                .map(|outcome| count(base, entity_type, rule, outcome));
            let all = pass + reject + flag + quarantine;
            Counts {
                pass,
                reject,
                flag,
                quarantine,
                pass_rate: (all > 0).then(|| pass as f64 / all as f64),
            }
        };
        // Every reading in order, the latest last, for when a `reject` counter last grew.
        let series: Vec<&Reading> = kept
            .iter()
            .filter(|reading| reading.at < latest.at)
            .chain(std::iter::once(&latest))
            .collect();
        let last_rejected = |entity_type: &str, rule: &str| {
            let key = (
                space.to_owned(),
                entity_type.to_owned(),
                rule.to_owned(),
                "reject".to_owned(),
            );
            let value = |reading: &Reading| reading.counts.get(&key).copied().unwrap_or(0.0);
            series
                .windows(2)
                .rev()
                .find(|pair| grew(value(pair[1]), value(pair[0])) > 0.0)
                .map(|pair| pair[1].at)
                .filter(|at| *at >= latest.at - WEEK)
        };

        SpaceChecks {
            at: Some(latest.at),
            windows: Some(Windows {
                day: day.at,
                week: week.at,
            }),
            rules: rows
                .into_iter()
                .map(|((entity_type, rule), attribute)| RuleChecks {
                    day: counts(day, &entity_type, &rule),
                    week: counts(week, &entity_type, &rule),
                    last_rejected: last_rejected(&entity_type, &rule),
                    entity_type,
                    rule,
                    attribute,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hours: i64) -> DateTime<Utc> {
        "2026-10-12T00:00:00Z".parse::<DateTime<Utc>>().unwrap() + chrono::Duration::hours(hours)
    }

    fn reading(hours: i64, counts: &[(&str, &str, &str, f64)]) -> Reading {
        Reading {
            at: at(hours),
            counts: counts
                .iter()
                .map(|(space, rule, outcome, value)| {
                    (
                        (
                            (*space).to_owned(),
                            "AirQualityObserved".to_owned(),
                            (*rule).to_owned(),
                            (*outcome).to_owned(),
                        ),
                        *value,
                    )
                })
                .collect(),
        }
    }

    fn rule(name: &str, attribute: &str) -> QualityRule {
        serde_json::from_value(serde_json::json!({
            "name": name, "entityType": "AirQualityObserved", "kind": "range", "attribute": attribute, "max": 500
        }))
        .unwrap()
    }

    #[test]
    fn the_metric_lines_are_read_by_their_labels_and_nothing_else() {
        let text = "# HELP jc_quality_checks_total checks\n\
                    # TYPE jc_quality_checks_total counter\n\
                    jc_quality_checks_total{space=\"air\",type=\"AirQualityObserved\",rule=\"pm10\",outcome=\"pass\"} 12\n\
                    jc_quality_checks_total{space=\"air\",type=\"AirQualityObserved\",outcome=\"pass\"} 3\n\
                    jc_gateway_requests_total{space=\"air\"} 99\n";
        let counts = parse(text);
        assert_eq!(counts.len(), 1, "{counts:?}");
        assert_eq!(
            counts[&(
                "air".into(),
                "AirQualityObserved".into(),
                "pm10".into(),
                "pass".into()
            )],
            12.0
        );
    }

    #[test]
    fn no_reading_is_an_empty_answer_never_all_passing() {
        let readings = Readings::new(None);
        let report = readings.report("air", &[rule("pm10-plausible", "pm10")]);
        assert_eq!(report.at, None);
        assert!(report.rules.is_empty());
    }

    #[test]
    fn the_windows_count_what_grew_inside_them_and_name_the_attribute() {
        let readings = Readings::new(None);
        readings.record(reading(0, &[("air", "pm10-plausible", "pass", 100.0)]));
        readings.record(reading(
            150,
            &[
                ("air", "pm10-plausible", "pass", 900.0),
                ("air", "pm10-plausible", "flag", 10.0),
            ],
        ));
        readings.record(reading(
            180,
            &[
                ("air", "pm10-plausible", "pass", 990.0),
                ("air", "pm10-plausible", "flag", 20.0),
                ("air", "pm10-plausible", "reject", 4.0),
                ("other", "pm10-plausible", "reject", 50.0),
            ],
        ));
        let report = readings.report(
            "air",
            &[rule("pm10-plausible", "pm10"), rule("no2-plausible", "no2")],
        );
        assert_eq!(report.at, Some(at(180)));
        let windows = report.windows.unwrap();
        assert_eq!(
            (windows.day, windows.week),
            (at(180), at(150)),
            "the oldest reading inside each"
        );
        assert_eq!(report.rules.len(), 2);
        let no2 = &report.rules[0];
        assert_eq!(
            (no2.rule.as_str(), no2.day.pass_rate),
            ("no2-plausible", None)
        );
        let pm10 = &report.rules[1];
        assert_eq!(pm10.attribute, "pm10");
        assert_eq!(
            pm10.week,
            Counts {
                pass: 90,
                reject: 4,
                flag: 10,
                quarantine: 0,
                pass_rate: Some(90.0 / 104.0)
            }
        );
        assert_eq!(pm10.last_rejected, Some(at(180)));
    }

    #[test]
    fn a_gateway_restart_counts_from_zero_and_an_undeclared_rule_keeps_its_row() {
        let readings = Readings::new(None);
        readings.record(reading(0, &[("air", "gone", "reject", 40.0)]));
        readings.record(reading(2, &[("air", "gone", "reject", 3.0)]));
        let report = readings.report("air", &[]);
        let gone = &report.rules[0];
        assert_eq!((gone.attribute.as_str(), gone.day.reject), ("", 3));
        assert_eq!(gone.last_rejected, Some(at(2)));
    }

    #[test]
    fn readings_are_kept_hourly_for_eight_days() {
        let readings = Readings::new(None);
        for minutes in [0, 30, 60] {
            readings.record(Reading {
                at: at(0) + chrono::Duration::minutes(minutes),
                counts: BTreeMap::new(),
            });
        }
        readings.record(reading(24 * 9, &[]));
        let kept = readings.kept.lock().unwrap();
        assert_eq!(
            kept.iter().map(|r| r.at).collect::<Vec<_>>(),
            vec![at(24 * 9)]
        );
    }
}
