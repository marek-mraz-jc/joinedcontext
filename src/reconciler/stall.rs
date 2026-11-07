//! A Live stream that takes records in and lands none (T-2967, T-2961).
//!
//! The runner's counters are cumulative, so one scrape cannot tell a stream that stopped
//! writing from one that wrote a lot once: hsl-hfp-vehicles read about 35 messages a second
//! for half an hour with `sent`, `rejected` and `errors` frozen, and its status said Live. This
//! keeps, per stream, the written count and the instant it last moved; when `received` has grown
//! since then and nothing was sent, rejected or failed for [`WINDOW`], the stream is stalled.
//!
//! A quiet source is not a stall: `received` standing still says the source sent nothing. A
//! runner that restarted counts from zero again, which starts the watch over. Everything lives
//! in this process: a Portal that restarts watches for another window before it says anything.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::api::pipelines::PipelineMetrics;

/// How long a stream may take records in and write nothing before it is called stalled. A
/// resident stream that keeps what it reads writes within seconds; one that drops nearly
/// everything (the thirty-bus cap) still writes a record a minute.
pub const WINDOW: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy)]
struct Sample {
    /// `sent + rejected + errors`: what the stream did with the records it took.
    written: u64,
    received: u64,
    /// When `written` last moved (or the watch began), and `received` at that instant.
    since: Instant,
    received_since: u64,
}

#[derive(Debug, Default)]
pub struct StallWatch {
    samples: Mutex<HashMap<(String, String), Sample>>,
}

impl StallWatch {
    /// Why this stream is stalled, or `None`. A stream whose runner exports no `received` or
    /// no `sent` is never called stalled: an absent counter is not zero.
    pub fn observe(
        &self,
        project: &str,
        pipeline: &str,
        metrics: &PipelineMetrics,
        now: Instant,
    ) -> Option<String> {
        let (received, sent) = (metrics.received?, metrics.sent?);
        let written = sent + metrics.rejected.unwrap_or(0) + metrics.errors.unwrap_or(0);
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (project.to_owned(), pipeline.to_owned());
        let fresh = Sample {
            written,
            received,
            since: now,
            received_since: received,
        };
        let held = match samples.get(&key) {
            // Moved, first seen, or the runner restarted and counts from zero again.
            Some(held) if held.written == written && received >= held.received => *held,
            _ => {
                samples.insert(key, fresh);
                return None;
            }
        };
        samples.insert(key, Sample { received, ..held });
        let taken = received - held.received_since;
        let quiet = now.saturating_duration_since(held.since);
        (taken > 0 && quiet >= WINDOW).then(|| {
            format!(
                "the stream took {taken} record(s) in over the last {} minute(s) and sent, \
                 rejected or failed none of them; the source delivers and nothing lands, the \
                 runner's log names the reason",
                quiet.as_secs() / 60
            )
        })
    }

    /// Drops the streams that are not Live this run, so a paused or deleted one starts over.
    pub fn retain(&self, live: &BTreeSet<(String, String)>) {
        self.samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|key, _| live.contains(key));
    }
}

/// Whether the author's `bento.yaml` ends with `mapping: root = deleted()` (T-2979): such a
/// stream does its work in its processors (the vehicles reaper deletes with an `http` one) and
/// no message ever reaches the output, so `sent` standing still says nothing about it. Only
/// the last processor counts: a filter midway that drops everything is the stall this watch
/// exists for (T-2961).
pub fn writes_through_processors(bento: &str) -> bool {
    serde_norway::from_str::<serde_json::Value>(bento)
        .ok()
        .and_then(|config| {
            let last = config.pointer("/pipeline/processors")?.as_array()?.last()?;
            let mapping = last.get("mapping")?.as_str()?;
            Some(mapping.split_whitespace().collect::<String>() == "root=deleted()")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(received: u64, sent: u64, rejected: u64, errors: u64) -> PipelineMetrics {
        PipelineMetrics {
            pipeline: "hsl-hfp-vehicles".to_owned(),
            received: Some(received),
            sent: Some(sent),
            rejected: Some(rejected),
            errors: Some(errors),
            ..PipelineMetrics::default()
        }
    }

    fn after(start: Instant, minutes: u64) -> Instant {
        start + Duration::from_secs(minutes * 60)
    }

    #[test]
    fn records_in_and_nothing_out_for_the_window_is_a_stall() {
        let watch = StallWatch::default();
        let t0 = Instant::now();
        assert_eq!(
            watch.observe("helsinki", "hfp", &counters(1000, 300, 0, 10), t0),
            None
        );
        assert_eq!(
            watch.observe("helsinki", "hfp", &counters(9000, 300, 0, 10), after(t0, 9)),
            None
        );
        let said = watch
            .observe(
                "helsinki",
                "hfp",
                &counters(21000, 300, 0, 10),
                after(t0, 10),
            )
            .expect("stalled after the window");
        assert!(
            said.contains("20000 record(s)") && said.contains("10 minute(s)"),
            "{said}"
        );
    }

    #[test]
    fn a_write_clears_it_and_starts_the_window_over() {
        let watch = StallWatch::default();
        let t0 = Instant::now();
        watch.observe("helsinki", "hfp", &counters(1000, 300, 0, 0), t0);
        assert!(watch
            .observe("helsinki", "hfp", &counters(5000, 300, 0, 0), after(t0, 11))
            .is_some());
        assert_eq!(
            watch.observe("helsinki", "hfp", &counters(5100, 301, 0, 0), after(t0, 12)),
            None
        );
        assert_eq!(
            watch.observe("helsinki", "hfp", &counters(9000, 301, 0, 0), after(t0, 21)),
            None
        );
        assert!(watch
            .observe("helsinki", "hfp", &counters(9100, 301, 0, 0), after(t0, 22))
            .is_some());
    }

    #[test]
    fn a_rejected_or_failed_record_is_the_stream_working() {
        let watch = StallWatch::default();
        let t0 = Instant::now();
        watch.observe("helsinki", "kpi", &counters(10, 0, 0, 0), t0);
        assert_eq!(
            watch.observe("helsinki", "kpi", &counters(50, 0, 40, 0), after(t0, 11)),
            None
        );
        assert_eq!(
            watch.observe("helsinki", "kpi", &counters(90, 0, 40, 40), after(t0, 22)),
            None
        );
    }

    #[test]
    fn a_quiet_source_is_not_a_stall() {
        let watch = StallWatch::default();
        let t0 = Instant::now();
        watch.observe("helsinki", "counters", &counters(98, 98, 0, 0), t0);
        assert_eq!(
            watch.observe(
                "helsinki",
                "counters",
                &counters(98, 98, 0, 0),
                after(t0, 60)
            ),
            None
        );
    }

    #[test]
    fn a_restarted_runner_starts_the_watch_over() {
        let watch = StallWatch::default();
        let t0 = Instant::now();
        watch.observe("helsinki", "hfp", &counters(90000, 3000, 0, 0), t0);
        // Counters from zero: not a stall of the old stream, and the window begins again.
        assert_eq!(
            watch.observe("helsinki", "hfp", &counters(500, 3000, 0, 0), after(t0, 11)),
            None
        );
        assert_eq!(
            watch.observe("helsinki", "hfp", &counters(900, 3000, 0, 0), after(t0, 20)),
            None
        );
        assert!(watch
            .observe(
                "helsinki",
                "hfp",
                &counters(1200, 3000, 0, 0),
                after(t0, 21)
            )
            .is_some());
    }

    #[test]
    fn a_counter_the_runner_does_not_export_is_not_zero() {
        let watch = StallWatch::default();
        let t0 = Instant::now();
        let mut blind = counters(1000, 0, 0, 0);
        blind.sent = None;
        watch.observe("helsinki", "hfp", &blind, t0);
        assert_eq!(
            watch.observe("helsinki", "hfp", &blind, after(t0, 30)),
            None
        );
    }

    #[test]
    fn only_a_last_processor_that_drops_everything_writes_through_processors() {
        let ends = |last: &str| {
            format!("pipeline:\n  processors:\n    - mapping: root = this\n    - {last}\n")
        };
        assert!(writes_through_processors(&ends(
            "mapping: root = deleted()"
        )));
        assert!(writes_through_processors(&ends(
            "mapping: |\n        root  =  deleted()\n"
        )));
        // A conditional drop, a filter midway, another processor at the end: the output is written.
        assert!(!writes_through_processors(&ends(
            "mapping: root = if this.stale { deleted() }"
        )));
        assert!(!writes_through_processors(
            "pipeline:\n  processors:\n    - mapping: root = deleted()\n    - mapping: root = this\n"
        ));
        assert!(!writes_through_processors(&ends(
            "log:\n        message: done"
        )));
        // Nothing to read: no processors, not YAML at all.
        assert!(!writes_through_processors("pipeline: {}\n"));
        assert!(!writes_through_processors("pipeline: [\n"));
    }

    #[test]
    fn a_stream_that_is_not_live_is_forgotten() {
        let watch = StallWatch::default();
        let t0 = Instant::now();
        watch.observe("helsinki", "hfp", &counters(1000, 300, 0, 0), t0);
        watch.retain(&BTreeSet::new());
        // Seen anew: the ten minutes count from here, not from before the pause.
        assert_eq!(
            watch.observe("helsinki", "hfp", &counters(5000, 300, 0, 0), after(t0, 11)),
            None
        );
        assert!(watch
            .observe("helsinki", "hfp", &counters(6000, 300, 0, 0), after(t0, 21))
            .is_some());
    }
}

/// What was last said about each pipeline the reconciler does not run, so a reason reaches the
/// log once when it starts or changes and not on every sync (T-4373: slovensko/ovzdusie was never
/// scheduled for hours and nothing in the log said why).
#[derive(Default)]
pub struct Unscheduled(std::sync::Mutex<std::collections::HashMap<(String, String), String>>);

impl Unscheduled {
    /// Whether `reason` is new for the pipeline; `None` means it runs again and is forgotten.
    pub fn changed(&self, ns: &str, name: &str, reason: Option<&str>) -> bool {
        let mut said = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let key = (ns.to_owned(), name.to_owned());
        match reason {
            None => {
                said.remove(&key);
                false
            }
            Some(reason) => said.insert(key, reason.to_owned()).as_deref() != Some(reason),
        }
    }
}

#[cfg(test)]
mod unscheduled_tests {
    use super::Unscheduled;

    #[test]
    fn a_reason_is_said_once_again_when_it_changes_and_after_the_pipeline_ran() {
        let said = Unscheduled::default();
        assert!(said.changed(
            "slovensko",
            "ovzdusie",
            Some("data source x is not in the mirror")
        ));
        assert!(!said.changed(
            "slovensko",
            "ovzdusie",
            Some("data source x is not in the mirror")
        ));
        assert!(said.changed("slovensko", "ovzdusie", Some("pipeline is disabled")));
        assert!(said.changed("zilina", "ovzdusie", Some("pipeline is disabled")));
        assert!(!said.changed("slovensko", "ovzdusie", None));
        assert!(said.changed("slovensko", "ovzdusie", Some("pipeline is disabled")));
    }
}
