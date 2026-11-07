use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use ekiben_core::{CoreError, OverlapPolicy, Registry, ResourceManager, RunSummary, StreamConfig};

use crate::cron::CronSchedule;
use crate::runner::run_stream_once;

#[derive(Clone)]
pub struct ScheduledStreamEntry {
    pub stream_id: String,
    pub config: StreamConfig,
    pub schedule: CronSchedule,
    pub overlap: OverlapPolicy,
    pub next_run_at: DateTime<Utc>,
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
    allow_host_env: bool,
}

impl Scheduler {
    pub fn new(
        registry: Arc<Registry>,
        resources: ResourceManager,
        file_root: Option<std::path::PathBuf>,
        allow_host_env: bool,
    ) -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            registry,
            resources,
            file_root,
            allow_host_env,
        }
    }

    pub fn add_stream(
        &self,
        stream_id: impl Into<String>,
        config: StreamConfig,
        now: DateTime<Utc>,
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

    /// Evaluates due streams at `now`, spawning any due runs asynchronously (so slow runs can overlap
    /// with subsequent ticks to test `Skip` vs `Queue` overlap rules).
    pub fn trigger_due(&self, now: DateTime<Utc>) -> Vec<String> {
        let mut to_spawn = Vec::new();
        {
            let mut guard = self.entries.lock().unwrap();
            for (sid, entry) in guard.iter_mut() {
                if now >= entry.next_run_at {
                    // Advance `next_run_at` strictly past `now` so missed runs after a pause fire only ONCE, never N times!
                    if let Some(next_after_now) = entry.schedule.next_after(now) {
                        entry.next_run_at = next_after_now;
                    }
                    if entry.is_running {
                        match entry.overlap {
                            OverlapPolicy::Skip => {
                                entry.skipped_overlaps += 1;
                            }
                            OverlapPolicy::Queue => {
                                entry.queued_run = true;
                            }
                        }
                    } else {
                        entry.is_running = true;
                        to_spawn.push((sid.clone(), entry.config.clone()));
                    }
                }
            }
        }

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
    pub async fn tick_and_wait(&self, now: DateTime<Utc>) -> Vec<RunSummary> {
        let mut to_run = Vec::new();
        {
            let mut guard = self.entries.lock().unwrap();
            for (sid, entry) in guard.iter_mut() {
                if now >= entry.next_run_at {
                    if let Some(next_after_now) = entry.schedule.next_after(now) {
                        entry.next_run_at = next_after_now;
                    }
                    if entry.is_running {
                        match entry.overlap {
                            OverlapPolicy::Skip => {
                                entry.skipped_overlaps += 1;
                            }
                            OverlapPolicy::Queue => {
                                entry.queued_run = true;
                            }
                        }
                    } else {
                        entry.is_running = true;
                        to_run.push((sid.clone(), entry.config.clone()));
                    }
                }
            }
        }

        let mut summaries = Vec::with_capacity(to_run.len());
        for (sid, conf) in to_run {
            let sum = self.execute_scheduled_run(sid, conf).await;
            summaries.push(sum);
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
                self.allow_host_env,
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

            let run_again = {
                let mut guard = self.entries.lock().unwrap();
                if let Some(entry) = guard.get_mut(&sid) {
                    entry.total_runs += 1;
                    entry.last_run = Some(summary.clone());
                    if entry.queued_run {
                        entry.queued_run = false;
                        true
                    } else {
                        entry.is_running = false;
                        false
                    }
                } else {
                    false
                }
            };

            if !run_again {
                return summary;
            }
        }
    }
}
