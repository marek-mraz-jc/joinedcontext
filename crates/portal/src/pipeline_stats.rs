//! Counts per node of a running pipeline over the last 1, 5 and 15 minutes (PL-68).
//!
//! The runner's counters are cumulative; a rate needs two readings. Each stats request keeps
//! the reading it made, per pipeline, for 15 minutes, and a window's count is what the
//! counters grew by inside it. The Portal keeps no history beyond that: Prometheus does
//! (OPS-16).
//!
//! ponytail: per-replica memory, like the logout marks in `state`; a second Portal replica
//! answers from its own readings, which only shortens its first windows.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;

use serde::Serialize;
use utoipa::ToSchema;

use crate::api::pipelines::NodeCounters;

/// The windows, in seconds, by the name the answer gives them.
const WINDOWS: [u64; 3] = [60, 300, 900];
/// How far before a window's start a reading still serves as its base: two polls of the
/// Studio, so a window is never shorter than the readings allow.
const SLACK: u64 = 10;
/// A reading younger than this is reused instead of asking the runner again, so ten open
/// tabs cost the runner one scrape every two seconds, not ten.
pub const MIN_GAP: u64 = 2;

/// What one node counted in each window.
#[derive(Debug, Default, Clone, PartialEq, Serialize, ToSchema)]
pub struct NodeWindows {
    #[serde(rename = "1m")]
    pub one: NodeCounters,
    #[serde(rename = "5m")]
    pub five: NodeCounters,
    #[serde(rename = "15m")]
    pub fifteen: NodeCounters,
}

#[derive(Debug, Default, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStats {
    pub pipeline: String,
    /// When the newest reading was taken, RFC 3339.
    pub scraped_at: String,
    /// The age of the oldest reading kept, at most 900: a window longer than this holds what
    /// was seen so far, not the whole window.
    pub covered_seconds: u64,
    pub nodes: BTreeMap<String, NodeWindows>,
}

struct Reading {
    at: u64,
    scraped_at: String,
    nodes: BTreeMap<String, NodeCounters>,
}

/// The readings of every pipeline someone looked at in the last 15 minutes.
#[derive(Default)]
pub struct Samples {
    by_pipeline: Mutex<HashMap<(String, String), VecDeque<Reading>>>,
}

impl Samples {
    /// The answer from the readings kept, when the newest is younger than [`MIN_GAP`].
    pub fn fresh(&self, project: &str, name: &str, now: u64) -> Option<PipelineStats> {
        let map = self.by_pipeline.lock().unwrap_or_else(|e| e.into_inner());
        let readings = map.get(&(project.to_owned(), name.to_owned()))?;
        let newest = readings.back()?;
        (now.saturating_sub(newest.at) < MIN_GAP).then(|| stats(name, readings, now))
    }

    /// Keeps one reading, drops what is older than the longest window, and answers.
    pub fn record(
        &self,
        project: &str,
        name: &str,
        now: u64,
        scraped_at: String,
        nodes: BTreeMap<String, NodeCounters>,
    ) -> PipelineStats {
        let oldest_kept = now.saturating_sub(WINDOWS[2] + SLACK);
        let mut map = self.by_pipeline.lock().unwrap_or_else(|e| e.into_inner());
        // A pipeline nobody looked at for 15 minutes leaves nothing behind.
        map.retain(|_, readings| readings.back().is_some_and(|r| r.at >= oldest_kept));
        let readings = map
            .entry((project.to_owned(), name.to_owned()))
            .or_default();
        while readings.front().is_some_and(|r| r.at < oldest_kept) {
            readings.pop_front();
        }
        readings.push_back(Reading {
            at: now,
            scraped_at,
            nodes,
        });
        stats(name, readings, now)
    }
}

fn stats(name: &str, readings: &VecDeque<Reading>, now: u64) -> PipelineStats {
    let Some(newest) = readings.back() else {
        return PipelineStats::default();
    };
    let oldest = readings.front().map_or(now, |r| r.at);
    let window = |seconds: u64| -> Vec<&Reading> {
        let start = now.saturating_sub(seconds + SLACK);
        readings.iter().filter(|r| r.at >= start).collect()
    };
    let [one, five, fifteen] = WINDOWS.map(window);
    let nodes = newest
        .nodes
        .keys()
        .map(|node| {
            let windows = NodeWindows {
                one: grown(&one, node),
                five: grown(&five, node),
                fifteen: grown(&fifteen, node),
            };
            (node.clone(), windows)
        })
        .collect();
    PipelineStats {
        pipeline: name.to_owned(),
        scraped_at: newest.scraped_at.clone(),
        covered_seconds: now.saturating_sub(oldest).min(WINDOWS[2]),
        nodes,
    }
}

/// What one node's counters grew by across the readings, oldest first. A counter that went
/// down was reset by a runner restart and counts from zero; one the newest reading lacks
/// stays absent, because absent is not zero.
fn grown(readings: &[&Reading], node: &str) -> NodeCounters {
    let field = |pick: fn(&NodeCounters) -> Option<u64>| -> Option<u64> {
        let values: Vec<Option<u64>> = readings
            .iter()
            .map(|r| r.nodes.get(node).and_then(pick))
            .collect();
        values.last().copied().flatten()?;
        Some(
            values
                .windows(2)
                .map(|pair| match (pair[0], pair[1]) {
                    (Some(before), Some(after)) if after >= before => after - before,
                    (_, Some(after)) => after,
                    (_, None) => 0,
                })
                .sum(),
        )
    };
    NodeCounters {
        received: field(|c| c.received),
        sent: field(|c| c.sent),
        errors: field(|c| c.errors),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(received: u64, errors: Option<u64>) -> BTreeMap<String, NodeCounters> {
        BTreeMap::from([(
            "step_0".to_owned(),
            NodeCounters {
                received: Some(received),
                sent: Some(received - errors.unwrap_or(0)),
                errors,
            },
        )])
    }

    fn record(samples: &Samples, at: u64, received: u64, errors: Option<u64>) -> PipelineStats {
        samples.record("p", "pipe", at, format!("t{at}"), nodes(received, errors))
    }

    #[test]
    fn the_first_reading_counts_nothing_yet() {
        let stats = record(&Samples::default(), 1000, 500, Some(3));
        assert_eq!(stats.covered_seconds, 0);
        let step = &stats.nodes["step_0"];
        assert_eq!(step.one.received, Some(0), "one reading is no rate");
        assert_eq!(step.fifteen.errors, Some(0));
    }

    #[test]
    fn each_window_counts_what_grew_inside_it() {
        let samples = Samples::default();
        // A reading every five seconds for twenty minutes, ten messages each, one error a minute.
        let mut last = PipelineStats::default();
        for i in 0..=240u64 {
            last = record(&samples, 1000 + i * 5, i * 10, Some(i / 12));
        }
        let step = &last.nodes["step_0"];
        // 60 s plus the slack is 14 intervals of five seconds.
        assert_eq!(step.one.received, Some(140));
        assert_eq!(step.five.received, Some(620));
        assert_eq!(step.fifteen.received, Some(1820));
        assert_eq!(step.fifteen.errors, Some(16));
        assert_eq!(last.covered_seconds, 900, "older readings are dropped");
        assert_eq!(last.scraped_at, "t2200");
    }

    #[test]
    fn a_runner_restart_counts_from_zero_not_negative() {
        let samples = Samples::default();
        record(&samples, 1000, 900, None);
        record(&samples, 1005, 950, None);
        let stats = record(&samples, 1010, 20, None);
        assert_eq!(
            stats.nodes["step_0"].one.received,
            Some(70),
            "50 before, 20 after"
        );
    }

    #[test]
    fn a_counter_the_runner_does_not_export_stays_absent() {
        let samples = Samples::default();
        record(&samples, 1000, 10, None);
        let stats = record(&samples, 1005, 20, None);
        assert_eq!(stats.nodes["step_0"].one.errors, None);
        assert_eq!(stats.nodes["step_0"].one.received, Some(10));
    }

    #[test]
    fn a_reading_younger_than_the_gap_is_reused() {
        let samples = Samples::default();
        record(&samples, 1000, 10, None);
        assert!(samples.fresh("p", "pipe", 1001).is_some());
        assert!(samples.fresh("p", "pipe", 1002).is_none());
        assert!(
            samples.fresh("other", "pipe", 1001).is_none(),
            "per project"
        );
    }

    #[test]
    fn a_pipeline_nobody_watches_is_forgotten() {
        let samples = Samples::default();
        record(&samples, 1000, 10, None);
        samples.record("q", "other", 3000, "t".into(), BTreeMap::new());
        assert!(samples.fresh("p", "pipe", 1000).is_none());
        let stats = record(&samples, 3001, 99, None);
        assert_eq!(stats.covered_seconds, 0, "a stale reading is no base");
        assert_eq!(stats.nodes["step_0"].one.received, Some(0));
    }
}
