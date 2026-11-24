use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ekiben_core::{
    CoreError, EnvAccess, OverlapPolicy, Registry, ResourceManager, RunSummary, StreamConfig,
};
use jiff::Timestamp;

use crate::cron::CronSchedule;
use crate::runner::run_stream_once;

#[derive(Clone)]
pub struct ScheduledStreamEntry {
    pub stream_id: String,
    pub config: StreamConfig,
    pub schedule: CronSchedule,
    pub overlap: OverlapPolicy,
    pub next_run_at: Timestamp,
    pub is_running: bool,
    pub queued_run: bool,
    pub total_runs: u64,
    pub skipped_overlaps: u64,
    pub last_run: Option<RunSummary>,
}

/// Resident scheduler (`Section 6: ekiben schedule`):
/// Holds **only** the schedule table (`ScheduledStreamEntry`), never a resident stream's state.
/// At each due time, spawns the stream as a fresh one-shot run and frees all of its memory when done.
/// - Overlap rule per stream: `Skip` (default) or `Queue`, never two runs of one stream at once.
/// - Missed runs after a pause: runs **once** and advances `next_run_at` past `now`, never $N$ times.
#[derive(Clone)]
pub struct Scheduler {
    entries: Arc<Mutex<BTreeMap<String, ScheduledStreamEntry>>>,
    registry: Arc<Registry>,
    resources: ResourceManager,
    file_root: Option<std::path::PathBuf>,
    env_access: EnvAccess,
}

impl Scheduler {
    /// `env_access`: which process variables the scheduled configs read (`true` is the whole
    /// environment, `false` none, or an explicit [`EnvAccess`]).
    pub fn new(
        registry: Arc<Registry>,
        resources: ResourceManager,
        file_root: Option<std::path::PathBuf>,
        env_access: impl Into<EnvAccess>,
    ) -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            registry,
            resources,
            file_root,
            env_access: env_access.into(),
        }
    }

    pub fn add_stream(
        &self,
        stream_id: impl Into<String>,
        config: StreamConfig,
        now: Timestamp,
    ) -> Result<(), CoreError> {
        let sid = stream_id.into();
        let sched_cfg = config.schedule.clone().ok_or_else(|| {
            CoreError::config(format!("stream '{sid}' has no schedule configuration"))
        })?;

        let expr = if let Some(ref c) = sched_cfg.cron {
            c.clone()
        } else if let Some(ref p) = sched_cfg.period {
            format!("@every {p}")
        } else {
            return Err(CoreError::config(format!(
                "stream '{sid}' schedule requires 'cron' or 'period'"
            )));
        };

        let cron = CronSchedule::parse(&expr, sched_cfg.timezone.as_deref())?;
        let next_run_at = cron.next_after(now).ok_or_else(|| {
            CoreError::config(format!("schedule '{expr}' has no future run time"))
        })?;

        self.entries.lock().unwrap().insert(
            sid.clone(),
            ScheduledStreamEntry {
                stream_id: sid,
                config,
                schedule: cron,
                overlap: sched_cfg.overlap,
                next_run_at,
                is_running: false,
                queued_run: false,
                total_runs: 0,
                skipped_overlaps: 0,
                last_run: None,
            },
        );
        Ok(())
    }

    pub fn get_entry(&self, stream_id: &str) -> Option<ScheduledStreamEntry> {
        self.entries.lock().unwrap().get(stream_id).cloned()
    }

    pub fn list_entries(&self) -> Vec<ScheduledStreamEntry> {
        self.entries.lock().unwrap().values().cloned().collect()
    }

    /// Marks the streams due at `now` running and answers them; a due stream still running counts a
    /// skipped overlap (`Skip`) or queues one more run (`Queue`). `next_run_at` moves strictly past
    /// `now`, so missed runs after a pause fire once, never N times.
    fn take_due(&self, now: Timestamp) -> Vec<(String, StreamConfig)> {
        let mut due = Vec::new();
        let mut guard = self.entries.lock().unwrap();
        for (sid, entry) in guard.iter_mut() {
            if now < entry.next_run_at {
                continue;
            }
            entry.next_run_at = entry.schedule.next_after(now).unwrap_or(entry.next_run_at);
            if !entry.is_running {
                entry.is_running = true;
                due.push((sid.clone(), entry.config.clone()));
            } else if entry.overlap == OverlapPolicy::Queue {
                entry.queued_run = true;
            } else {
                entry.skipped_overlaps += 1;
            }
        }
        due
    }

    /// Evaluates due streams at `now`, spawning any due runs asynchronously (so slow runs can overlap
    /// with subsequent ticks to test `Skip` vs `Queue` overlap rules).
    pub fn trigger_due(&self, now: Timestamp) -> Vec<String> {
        let to_spawn = self.take_due(now);
        let triggered_ids: Vec<String> = to_spawn.iter().map(|(id, _)| id.clone()).collect();
        for (sid, conf) in to_spawn {
            let sched = self.clone();
            tokio::spawn(async move {
                sched.execute_scheduled_run(sid, conf).await;
            });
        }
        triggered_ids
    }

    /// Synchronous tick helper that triggers all due streams at `now` and waits for their runs to finish.
    pub async fn tick_and_wait(&self, now: Timestamp) -> Vec<RunSummary> {
        let mut summaries = Vec::new();
        for (sid, conf) in self.take_due(now) {
            summaries.push(self.execute_scheduled_run(sid, conf).await);
        }
        summaries
    }

    async fn execute_scheduled_run(&self, sid: String, conf: StreamConfig) -> RunSummary {
        loop {
            let summary = match run_stream_once(
                &sid,
                conf.clone(),
                self.registry.clone(),
                self.resources.clone(),
                self.file_root.clone(),
                self.env_access,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => RunSummary {
                    stream_id: sid.clone(),
                    org_id: conf.org.0.clone(),
                    success: false,
                    error: Some(e.to_string()),
                    ..Default::default()
                },
            };

            // Entries are never removed, so the stream's entry is still there.
            let run_again = self
                .entries
                .lock()
                .unwrap()
                .get_mut(&sid)
                .is_some_and(|entry| {
                    entry.total_runs += 1;
                    entry.last_run = Some(summary.clone());
                    entry.is_running = entry.queued_run;
                    std::mem::take(&mut entry.queued_run)
                });

            if !run_again {
                return summary;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scheduler() -> Scheduler {
        Scheduler::new(
            crate::build_full_registry(),
            ResourceManager::new(),
            None,
            false,
        )
    }

    fn conf(text: &str) -> StreamConfig {
        StreamConfig::from_yaml_or_json(text).expect("config")
    }

    const OUT: &str = "output:\n  drop: {}\n";

    fn at(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn add_stream_refuses_what_cannot_be_scheduled() {
        let s = scheduler();
        let now = at("2026-03-28T10:00:00Z");
        let gen = "input:\n  generate:\n    count: 1\n    mapping: 'root = 1'\n";
        for (sched, want) in [
            ("", "has no schedule configuration"),
            (
                "schedule:\n  overlap: queue\n",
                "requires 'cron' or 'period'",
            ),
            (
                "schedule:\n  cron: '0 0 * * *'\n  timezone: Mars/Olympus\n",
                "unknown timezone",
            ),
            (
                "schedule:\n  cron: '0 0 30 2 *'\n",
                "has no future run time",
            ),
        ] {
            let err = s
                .add_stream("x", conf(&format!("{sched}{gen}{OUT}")), now)
                .expect_err(sched)
                .to_string();
            assert!(err.contains(want), "{sched}: {err}");
        }
        assert!(s.list_entries().is_empty());
        assert!(s.get_entry("x").is_none());
        let ok = format!("schedule:\n  period: 1m\n{gen}{OUT}");
        s.add_stream("p", conf(&ok), now).expect("period");
        let e = s.get_entry("p").expect("entry");
        assert_eq!(e.next_run_at, at("2026-03-28T10:01:00Z"));
        assert_eq!(s.list_entries().len(), 1);
    }

    #[tokio::test]
    async fn a_run_that_does_not_build_is_recorded_as_failed() {
        let s = scheduler();
        let now = at("2026-03-28T10:00:00Z");
        let text = format!("schedule:\n  period: 1m\ninput:\n  no_such_input: {{}}\n{OUT}");
        s.add_stream("bad", conf(&text), now).expect("added");
        let runs = s.tick_and_wait(at("2026-03-28T10:01:00Z")).await;
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].success);
        assert!(runs[0].error.is_some());
        let e = s.get_entry("bad").expect("entry");
        assert_eq!(e.total_runs, 1);
        assert!(!e.is_running);
    }

    #[tokio::test]
    async fn an_overlapping_tick_skips_or_queues_by_the_stream_rule() {
        let s = scheduler();
        let now = at("2026-03-28T10:00:00Z");
        let slow =
            "input:\n  generate:\n    count: 2\n    interval: 300ms\n    mapping: 'root = 1'\n";
        for (id, overlap) in [("skip", "skip"), ("queue", "queue")] {
            let text = format!("schedule:\n  period: 1s\n  overlap: {overlap}\n{slow}{OUT}");
            s.add_stream(id, conf(&text), now).expect("added");
        }
        assert!(s.trigger_due(now).is_empty(), "nothing is due yet");
        let first = s.trigger_due(at("2026-03-28T10:00:01Z"));
        assert_eq!(first.len(), 2);
        assert!(s.trigger_due(at("2026-03-28T10:00:02Z")).is_empty());
        assert_eq!(s.get_entry("skip").expect("skip").skipped_overlaps, 1);
        assert!(s.get_entry("queue").expect("queue").queued_run);
        for _ in 0..100 {
            let q = s.get_entry("queue").expect("queue");
            if q.total_runs == 2 && !q.is_running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let q = s.get_entry("queue").expect("queue");
        assert_eq!(
            (q.total_runs, q.queued_run, q.is_running),
            (2, false, false)
        );
        let k = s.get_entry("skip").expect("skip");
        assert_eq!(k.total_runs, 1);
        assert!(k.last_run.expect("run").success);
    }
}
