//! The run queue of the pipelines that do not wait for pushes (PL-78, T-4348).
//!
//! A pipeline whose sources wait for pushes (MQTT, a WebSocket, a server or a broker subscriber)
//! stays a resident stream. Every other one is created in the runner only for its run and
//! deleted when the run finishes, fails or reaches [`RUN_TIMEOUT`]: one queue ordered by due
//! time, then project and name, in two lanes, so a long harvest never holds a live poller. Before
//! this every scheduled poll was a resident stream with its cron as the clock; 92 of them fired
//! together and the runner ran out of memory (2026-10-16 09:50Z).
//!
//! [`RunQueue`] is the state alone, read with a clock the caller passes in; `tick` drives it
//! against a `Runner`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Duration as Span, SecondsFormat, Timelike, Utc};
use jc_core::kinds::data_source::DataSourceType;
use serde_json::Value;

use super::streams::StreamOutcome;

/// A pipeline by (project, name).
pub type Key = (String, String);

/// The longest a run may take before it is deleted as failed.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// A clock at least this long runs in the batch lane, a shorter one in the short lane.
const SHORT_BELOW: Duration = Duration::from_secs(5 * 60);
/// A queue that no reconciler pass has planned for this long belongs to a Portal that lost its
/// leadership: it finishes what runs and starts nothing.
const STALE_PLAN: Duration = Duration::from_secs(10 * 60);
/// How long a pipeline whose run failed for a cause outside its source waits before each retry
/// (T-5687); past the last it waits for its clock, as it does when its clock comes sooner.
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(4 * 60 * 60),
];
/// The most retries one pipeline is queued for in a day, whatever earned them (T-5687).
const RETRIES_PER_DAY: usize = 6;

/// The runner inputs that wait for pushes and so stay loaded: servers and broker subscribers.
const RESIDENT_INPUTS: &[&str] = &[
    "http_server",
    "socket_server",
    "kafka",
    "kafka_franz",
    "nats",
    "nats_jetstream",
    "nats_stream",
    "amqp_0_9",
    "amqp_1",
    "redis_streams",
    "redis_pubsub",
    "gcp_pubsub",
    "mqtt",
    "websocket",
];

/// Whether a source of this type waits for pushes and so stays loaded in the runner (PL-78);
/// a poll, a feed and a read that ends are run and deleted.
pub fn is_resident(source: &DataSourceType) -> bool {
    match source {
        DataSourceType::Mqtt | DataSourceType::WebSocket => true,
        DataSourceType::Http | DataSourceType::GtfsRt => false,
        DataSourceType::Runner(name) => RESIDENT_INPUTS.contains(&name.as_str()),
    }
}

/// The two lanes of the queue, each with its own concurrency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane {
    /// A clock of 5 minutes or longer, and a pipeline that runs once.
    Batch,
    /// A clock shorter than 5 minutes.
    Short,
}

impl Lane {
    fn label(self) -> &'static str {
        match self {
            Lane::Batch => "batch",
            Lane::Short => "short",
        }
    }
}

/// When a pipeline runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clock {
    /// Neither `period` nor `schedule`: once after apply.
    Once,
    /// A `period`, counted from 1970-01-01T00:00Z so a Portal restart does not move it, plus the
    /// phase its `schedule` names: `24h` with `55 21 * * *` runs at 21:55 UTC, without one at 00:00.
    Every(Duration, Duration),
    /// A `schedule` of five cron fields, read in UTC.
    Cron(Cron),
}

impl Clock {
    /// The clock of a pipeline: its `period` when it has one, started at the time its `schedule`
    /// names, else its `schedule`. Every period counted from 00:00 alone made all 380 daily
    /// pipelines of dev due at 00:00Z, and the queue drained them into the night's busiest hours
    /// (T-5719).
    pub fn of(period: Option<&str>, schedule: Option<&str>) -> Result<Clock, String> {
        fn given(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|v| !v.is_empty())
        }
        let cron = given(schedule)
            .map(|schedule| {
                Cron::parse(schedule).ok_or_else(|| {
                    format!("schedule `{schedule}` is not five cron fields (`0 5 * * *`)")
                })
            })
            .transpose()?;
        let Some(period) = given(period) else {
            return Ok(cron.map_or(Clock::Once, Clock::Cron));
        };
        let every = super::streams::duration_of(period)
            .filter(|every| *every >= Duration::from_secs(1))
            .ok_or_else(|| {
                format!("period `{period}` is not a duration of at least 1s (`30s`, `5m`, `24h`)")
            })?;
        Ok(Clock::Every(
            every,
            cron.map_or(Duration::ZERO, |cron| cron.phase(every)),
        ))
    }

    /// The first run strictly after `t`; none for [`Clock::Once`].
    pub fn next_after(&self, t: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Clock::Once => None,
            Clock::Every(every, phase) => {
                let every = every.as_millis() as i64;
                let phase = phase.as_millis() as i64;
                let at = ((t.timestamp_millis() - phase).div_euclid(every) + 1) * every + phase;
                DateTime::from_timestamp_millis(at)
            }
            Clock::Cron(cron) => cron.next_after(t),
        }
    }

    /// The lane of a pipeline on this clock, read from the gap between its next two runs.
    pub fn lane(&self, now: DateTime<Utc>) -> Lane {
        let gap = self
            .next_after(now)
            .and_then(|first| Some((first, self.next_after(first)?)))
            .map(|(first, second)| (second - first).to_std().unwrap_or_default());
        match gap {
            Some(gap) if gap < SHORT_BELOW => Lane::Short,
            _ => Lane::Batch,
        }
    }
}

/// Five cron fields as bit sets: minute, hour, day of month, month, day of week (0 and 7 are
/// Sunday). Numbers, `*`, `a-b`, `a,b` and `/n` steps; no names and no `@daily`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    any_day: bool,
    any_weekday: bool,
}

impl Cron {
    pub fn parse(text: &str) -> Option<Cron> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return None;
        };
        let mut weekdays = field(weekday, 0, 7)?;
        if weekdays & (1 << 7) != 0 {
            weekdays |= 1;
        }
        Some(Cron {
            minutes: field(minute, 0, 59)?,
            hours: field(hour, 0, 23)?,
            days: field(day, 1, 31)?,
            months: field(month, 1, 12)?,
            weekdays,
            any_day: day.starts_with('*'),
            any_weekday: weekday.starts_with('*'),
        })
    }

    fn matches(&self, t: DateTime<Utc>) -> bool {
        use chrono::Datelike;
        let has = |set: u64, v: u32| set & (1 << v) != 0;
        let day = has(self.days, t.day());
        let weekday = has(self.weekdays, t.weekday().num_days_from_sunday());
        // Both restricted means either one, as cron reads it.
        let day_ok = match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (true, false) => weekday,
            (false, true) => day,
            (false, false) => day || weekday,
        };
        has(self.minutes, t.minute())
            && has(self.hours, t.hour())
            && has(self.months, t.month())
            && day_ok
    }

    /// How far into each `every` from 1970-01-01T00:00Z this cron first fires; zero when it never
    /// does within the year [`Cron::next_after`] scans.
    fn phase(&self, every: Duration) -> Duration {
        let epoch = DateTime::<Utc>::UNIX_EPOCH;
        let since = self
            .next_after(epoch - Span::minutes(1))
            .and_then(|first| (first - epoch).to_std().ok())
            .unwrap_or_default();
        Duration::from_millis((since.as_millis() % every.as_millis().max(1)) as u64)
    }

    /// ponytail: a minute-by-minute scan over at most a year (half a million cheap checks for a
    /// date that never comes, such as 30 February); a field-skipping search if it ever shows.
    fn next_after(&self, t: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut at = t.with_second(0)?.with_nanosecond(0)? + Span::minutes(1);
        for _ in 0..366 * 24 * 60 {
            if self.matches(at) {
                return Some(at);
            }
            at += Span::minutes(1);
        }
        None
    }
}

/// One cron field as a bit set of the values from `lo` to `hi` it names.
fn field(text: &str, lo: u32, hi: u32) -> Option<u64> {
    let mut set = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, Some(step.parse::<u32>().ok().filter(|s| *s > 0)?)),
            None => (part, None),
        };
        let (from, to) = if range == "*" {
            (lo, hi)
        } else if let Some((from, to)) = range.split_once('-') {
            (from.parse().ok()?, to.parse().ok()?)
        } else {
            let from: u32 = range.parse().ok()?;
            (from, if step.is_some() { hi } else { from })
        };
        if from < lo || to > hi || from > to {
            return None;
        }
        for v in (from..=to).step_by(step.unwrap_or(1) as usize) {
            set |= 1 << v;
        }
    }
    Some(set)
}

/// One run-and-delete pipeline as the reconciler hands it over: its stream as one run, its
/// clock, and whether an output replaces its entities (PL-79).
#[derive(Debug, Clone)]
pub struct Planned {
    pub stream: Value,
    pub clock: Clock,
    pub replace: bool,
}

#[derive(Debug, Clone)]
enum Last {
    Finished { at: DateTime<Utc>, seconds: i64 },
    Failed { at: DateTime<Utc>, reason: String },
}

/// The run a pipeline earned when its last run failed for a cause outside its source (T-5687).
#[derive(Debug, Clone)]
struct Retry {
    at: DateTime<Utc>,
    cause: String,
}

#[derive(Debug)]
struct Entry {
    stream: Value,
    clock: Clock,
    lane: Lane,
    /// When it is due next.
    next: Option<DateTime<Utc>>,
    /// In the queue since this due time.
    queued: Option<DateTime<Utc>>,
    /// In the runner since then.
    running: Option<DateTime<Utc>>,
    /// The id of the run in the runner: its due time, as the pipeline's log names runs (PL-62).
    run: Option<String>,
    /// The [`stream_hash`] of the stream that run runs, kept from its start: the stream may
    /// change while it runs (T-5825).
    run_stream: Option<String>,
    /// Its stream is removed from the runner once before its first run: one an older Portal
    /// loaded as resident, or a run a restarted Portal no longer knows.
    sweep: bool,
    /// Its last run outlived its cap and the runner did not let go of it on delete (T-5370):
    /// a step that never returns. It starts again only once its stream changes.
    stuck: bool,
    last: Option<Last>,
    /// Its one pending retry (T-5687).
    retry: Option<Retry>,
    /// The failed run the last retry was armed for: one failure arms one retry.
    armed_for: Option<String>,
    /// Retries armed since its last run that did not fail for an outside cause.
    attempts: usize,
    /// When each retry of the last day was queued.
    retried: Vec<DateTime<Utc>>,
}

impl Entry {
    /// How long one run may take: [`RUN_TIMEOUT`], and for a pipeline polled every few
    /// minutes three periods (two minutes at least), so a looping poll frees its short lane
    /// long before a batch run's cap (T-5370).
    fn cap(&self, timeout: Duration) -> Duration {
        match self.clock {
            Clock::Every(every, _) if every < SHORT_BELOW => {
                timeout.min((every * 3).max(Duration::from_secs(120)))
            }
            _ => timeout,
        }
    }

    /// Arms the retry the failed run `run` earned: the next step of [`BACKOFF`], unless its clock
    /// comes sooner, the backoff is spent or it had [`RETRIES_PER_DAY`] retries in the last day.
    fn arm(&mut self, run: &str, cause: &str, now: DateTime<Utc>) {
        if self.armed_for.as_deref() == Some(run) {
            return;
        }
        self.armed_for = Some(run.to_owned());
        self.retried.retain(|t| now - *t < Span::days(1));
        let at = BACKOFF
            .get(self.attempts)
            .map(|wait| now + Span::from_std(*wait).unwrap_or_default());
        self.attempts = self.attempts.saturating_add(1);
        let next = self.next;
        let room = self.retried.len() < RETRIES_PER_DAY;
        self.retry = at
            .filter(|at| room && next.is_none_or(|next| *at < next))
            .map(|at| Retry {
                at,
                cause: cause.to_owned(),
            });
    }

    /// Its last run did not fail for an outside cause: the backoff starts over.
    fn clear_retry(&mut self) {
        self.retry = None;
        self.armed_for = None;
        self.attempts = 0;
    }
}

/// What the runner says of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// It still runs, or the runner did not answer.
    Running,
    /// It ended: its input is exhausted.
    Finished,
    /// The runner does not hold it.
    Gone,
}

/// The runner, as the queue uses it.
pub(crate) trait Runner {
    /// Creates the run's stream.
    async fn start(&self, key: &Key, stream: &Value) -> Result<(), String>;
    /// How the run stands.
    async fn state(&self, key: &Key) -> RunState;
    /// Removes the stream; a stream that is not there is fine. False when the runner did not
    /// let go of it in time: its stop hangs on a step that never returns (T-5370).
    async fn delete(&self, key: &Key) -> bool;
    /// Hears that the run `run` of `key` ended, `finished` or not (T-4891), and the
    /// [`stream_hash`] of the stream it ran (T-5825).
    async fn ended(&self, _key: &Key, _run: &str, _finished: bool, _stream: &str) {}
}

/// The queue and its two lanes' concurrency.
#[derive(Debug)]
pub struct RunQueue {
    batch: usize,
    short: usize,
    timeout: Duration,
    entries: BTreeMap<Key, Entry>,
    /// Runs whose pipeline went away while they ran: deleted on the next tick.
    gone: Vec<Key>,
    /// The runs that ended since the last tick, by run id, and whether the runner finished them
    /// (T-4891): a snapshot pipeline judges its deletes on them.
    ends: Vec<(Key, String, bool, String)>,
    planned_at: Option<DateTime<Utc>>,
    /// The pipelines with a run in the durable log and when their newest run last wrote
    /// (T-4421): a scheduled one new to this queue and not among them never ran, so it runs at
    /// once instead of waiting a day for its clock; one whose due time after that run has passed
    /// missed it while no Portal held it, so it is queued under that due time (T-5778). `None`
    /// until the log has been read: a restart then never runs every pipeline at once.
    ran: Option<HashMap<Key, DateTime<Utc>>>,
    /// The [`stream_hash`] of the stream each pipeline's newest logged run ran (T-5825): a
    /// replace pipeline new to this queue whose stream differs from it changed while no Portal
    /// held it, so it runs at once. A pipeline whose newest run has no hash is not in it.
    streams: Option<HashMap<Key, String>>,
    /// The workload accounts (`project/name`) whose Keycloak client the last reconcile could not
    /// converge (T-4426): a pipeline of one is not started, since every write it made would be
    /// refused 401 and replayed until [`RUN_TIMEOUT`], holding its lane.
    unbound: HashSet<String>,
    /// The pipelines whose newest logged run failed for a cause outside its source, with that
    /// run and the cause (T-5687), read before each [`RunQueue::plan`]; `None` when the log was
    /// not read.
    failed: Option<BTreeMap<Key, (String, String)>>,
    /// A fingerprint of each project's spaces, models and policies, and the projects whose
    /// fingerprint changed since the pass before (T-5687).
    rules: BTreeMap<String, u64>,
    rules_changed: HashSet<String>,
}

impl RunQueue {
    pub fn new(batch: usize, short: usize) -> Self {
        Self {
            batch: batch.max(1),
            short: short.max(1),
            timeout: RUN_TIMEOUT,
            entries: BTreeMap::new(),
            gone: Vec::new(),
            ends: Vec::new(),
            planned_at: None,
            ran: None,
            streams: None,
            unbound: HashSet::new(),
            failed: None,
            rules: BTreeMap::new(),
            rules_changed: HashSet::new(),
        }
    }

    /// The pipelines whose newest logged run failed for a cause outside its source (the model
    /// refused a record, a policy, the gateway or the broker refused a write), with that run's id
    /// and the cause (T-5687): each gets one retry with backoff on the next [`RunQueue::plan`].
    pub fn failed(&mut self, failed: BTreeMap<Key, (String, String)>) {
        self.failed = Some(failed);
    }

    /// A fingerprint of each project's spaces, data models and policies: a project whose
    /// fingerprint changed runs its pending retries at once, its cause most likely cleared.
    pub fn rules(&mut self, rules: BTreeMap<String, u64>) {
        for (project, fingerprint) in &rules {
            if self
                .rules
                .get(project)
                .is_some_and(|old| old != fingerprint)
            {
                self.rules_changed.insert(project.clone());
            }
        }
        self.rules = rules;
    }

    /// The workload accounts the last reconcile left without a client, as `project/name`
    /// (T-4426); an empty set once they all converged.
    pub fn unbound(&mut self, accounts: HashSet<String>) {
        self.unbound = accounts;
    }

    /// Whether `key`'s own pipeline account is one the realm has no client for.
    fn is_unbound(&self, key: &Key) -> bool {
        !self.unbound.is_empty()
            && self.unbound.contains(&format!(
                "{}/{}",
                key.0,
                jc_core::kinds::pipeline_identity::account_name(&key.1)
            ))
    }

    /// The pipelines the run log holds a run of, with their newest run's last line, read before
    /// each [`RunQueue::plan`].
    pub fn ran(&mut self, ran: HashMap<Key, DateTime<Utc>>) {
        self.ran = Some(ran);
    }

    /// The [`stream_hash`] of the stream each pipeline's newest logged run ran, read before each
    /// [`RunQueue::plan`] (T-5825).
    pub fn streams(&mut self, streams: HashMap<Key, String>) {
        self.streams = Some(streams);
    }

    /// Whether `planned` replaces and its stream differs from the one the newest logged run of
    /// `key` ran: it changed while no Portal held it (T-5825).
    fn changed_unheld(&self, key: &Key, planned: &Planned) -> bool {
        planned.replace
            && self
                .streams
                .as_ref()
                .and_then(|streams| streams.get(key))
                .is_some_and(|ran| *ran != stream_hash(&planned.stream))
    }

    /// Takes the reconciler's whole set of run-and-delete pipelines: a new one or one whose
    /// stream or clock changed is due by its clock (a run-once one at once, and a scheduled one
    /// that never ran at once too), one no longer in the set leaves the queue and its run is
    /// deleted. A replace pipeline whose stream changed is due at once (T-5824): its entities
    /// hold what the old mapping wrote until it runs, and only a replace run removes that. The
    /// same holds for a replace pipeline new to this queue whose stream differs from the one its
    /// newest logged run ran: it changed while no Portal held it (T-5825).
    pub fn plan(&mut self, wanted: BTreeMap<Key, Planned>, now: DateTime<Utc>) {
        self.planned_at = Some(now);
        let dropped: Vec<Key> = self
            .entries
            .keys()
            .filter(|key| !wanted.contains_key(*key))
            .cloned()
            .collect();
        for key in dropped {
            if self
                .entries
                .remove(&key)
                .is_some_and(|e| e.running.is_some())
            {
                self.gone.push(key);
            }
        }
        for (key, planned) in wanted {
            let first = match planned.clock {
                Clock::Once => Some(now),
                ref clock => clock.next_after(now),
            };
            let lane = planned.clock.lane(now);
            match self.entries.get_mut(&key) {
                Some(entry) if entry.stream == planned.stream && entry.clock == planned.clock => {}
                Some(entry) => {
                    let rewrite = planned.replace && entry.stream != planned.stream;
                    entry.stuck = false;
                    entry.stream = planned.stream;
                    entry.clock = planned.clock;
                    entry.lane = lane;
                    entry.next = if rewrite { Some(now) } else { first };
                }
                None => {
                    let first = match self.ran.as_ref().map(|ran| ran.get(&key)) {
                        _ if self.changed_unheld(&key, &planned) => Some(now),
                        Some(None) => Some(now),
                        Some(Some(last)) => missed(&planned.clock, *last, now).or(first),
                        None => first,
                    };
                    self.entries.insert(
                        key,
                        Entry {
                            stream: planned.stream,
                            clock: planned.clock,
                            lane,
                            next: first,
                            queued: None,
                            running: None,
                            run: None,
                            run_stream: None,
                            sweep: true,
                            stuck: false,
                            last: None,
                            retry: None,
                            armed_for: None,
                            attempts: 0,
                            retried: Vec::new(),
                        },
                    );
                }
            }
        }
        self.retries(now);
    }

    /// Arms or clears each pipeline's retry from what the log says of its newest run, and brings
    /// the retries of a project whose rules changed forward to now (T-5687). A run in the queue
    /// or the runner is judged once it ended; a run the queue saw fail itself (the runner refused
    /// or lost it) armed its retry when it failed.
    fn retries(&mut self, now: DateTime<Utc>) {
        let failed = self.failed.take();
        let changed = std::mem::take(&mut self.rules_changed);
        for (key, entry) in &mut self.entries {
            if let Some(failed) = &failed {
                let idle = entry.running.is_none() && entry.queued.is_none();
                match (&entry.last, failed.get(key)) {
                    _ if !idle => {}
                    (Some(Last::Failed { .. }), _) => {}
                    (_, Some((run, cause))) => entry.arm(run, cause, now),
                    (_, None) => entry.clear_retry(),
                }
            }
            if changed.contains(&key.0) {
                if let Some(retry) = entry.retry.as_mut() {
                    retry.at = retry.at.min(now);
                }
            }
        }
    }

    /// The streams to remove before anything starts: runs whose pipeline went away, and each
    /// new pipeline's leftover.
    fn sweeps(&mut self) -> Vec<Key> {
        let mut keys = std::mem::take(&mut self.gone);
        for (key, entry) in &mut self.entries {
            if entry.sweep && entry.running.is_none() {
                entry.sweep = false;
                keys.push(key.clone());
            }
        }
        keys
    }

    fn running(&self) -> Vec<Key> {
        self.entries
            .iter()
            .filter(|(_, e)| e.running.is_some())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// What came due joins the queue; a pipeline already queued or running stays one entry.
    fn queue_due(&mut self, now: DateTime<Utc>) {
        for entry in self.entries.values_mut() {
            let Some(due) = entry.next.filter(|due| *due <= now) else {
                continue;
            };
            if entry.queued.is_none() && entry.running.is_none() && !entry.stuck {
                entry.queued = Some(due);
            }
            entry.next = entry.clock.next_after(now);
        }
    }

    /// Ends the runs the runner finished, lost or held too long; queues what came due; starts
    /// what each lane has room for. Returns the runs to delete and the runs to start.
    fn settle(
        &mut self,
        states: Vec<(Key, RunState)>,
        now: DateTime<Utc>,
    ) -> (Vec<Key>, Vec<(Key, Value)>) {
        let mut ended = Vec::new();
        for (key, state) in states {
            let Some(entry) = self.entries.get_mut(&key) else {
                continue;
            };
            let Some(started) = entry.running else {
                continue;
            };
            let seconds = (now - started).num_seconds().max(0);
            let cap = entry.cap(self.timeout);
            let outcome = match state {
                RunState::Finished => Some(("finished", None)),
                RunState::Gone => Some((
                    "failed",
                    Some("the runner no longer holds the run".to_owned()),
                )),
                RunState::Running if (now - started).to_std().unwrap_or_default() >= cap => Some((
                    "timeout",
                    Some(format!(
                        "it ran past {} min and was stopped",
                        cap.as_secs() / 60
                    )),
                )),
                RunState::Running => None,
            };
            let Some((label, failure)) = outcome else {
                continue;
            };
            crate::telemetry::pipeline_run(label, seconds as f64);
            let finished = failure.is_none();
            // A run the runner lost is retried; one past its cap would only run past it again.
            if let (RunState::Gone, Some(reason), Some(run)) = (state, &failure, entry.run.clone())
            {
                entry.arm(&run, reason, now);
            }
            let stream = entry.run_stream.take().unwrap_or_default();
            self.ends.extend(
                entry
                    .run
                    .clone()
                    .map(|run| (key.clone(), run, finished, stream)),
            );
            entry.running = None;
            entry.last = Some(match failure {
                None => Last::Finished { at: now, seconds },
                Some(reason) => Last::Failed { at: now, reason },
            });
            ended.push(key);
        }

        self.queue_due(now);
        self.queue_retries(now);

        let mut starts = Vec::new();
        let fresh = self
            .planned_at
            .is_some_and(|at| (now - at).to_std().unwrap_or_default() < STALE_PLAN);
        for (lane, limit) in [(Lane::Batch, self.batch), (Lane::Short, self.short)] {
            let in_lane = |e: &Entry| e.lane == lane;
            let busy = self
                .entries
                .values()
                .filter(|e| in_lane(e) && e.running.is_some())
                .count();
            let mut waiting: Vec<(DateTime<Utc>, Key)> = self
                .entries
                .iter()
                .filter(|(_, e)| in_lane(e) && e.running.is_none() && !e.sweep)
                .filter_map(|(k, e)| Some((e.queued?, k.clone())))
                .collect();
            waiting.sort();
            // A run whose account has no client would only be refused: it ends failed here, with
            // its reason, and leaves its slot to the next (T-4426).
            if fresh {
                let (refused, rest): (Vec<_>, Vec<_>) = waiting
                    .into_iter()
                    .partition(|(_, key)| self.is_unbound(key));
                waiting = rest;
                for (_, key) in refused {
                    if let Some(entry) = self.entries.get_mut(&key) {
                        crate::telemetry::pipeline_run("failed", 0.0);
                        entry.queued = None;
                        entry.last = Some(Last::Failed {
                            at: now,
                            reason: "not started: the realm holds no client for its account \
                                     yet, so the gateway would refuse its writes (401); it runs \
                                     at its next due time once the client is there"
                                .to_owned(),
                        });
                    }
                }
            }
            let room = if fresh { limit.saturating_sub(busy) } else { 0 };
            for (due, key) in waiting.iter().take(room) {
                if let Some(entry) = self.entries.get_mut(key) {
                    let run = stamp(*due);
                    entry.queued = None;
                    entry.running = Some(now);
                    starts.push((key.clone(), with_run(&entry.stream, &run)));
                    entry.run = Some(run);
                    entry.run_stream = Some(stream_hash(&entry.stream));
                }
            }
            crate::telemetry::pipeline_queue_depth(
                lane.label(),
                waiting.len().saturating_sub(room),
            );
        }
        (ended, starts)
    }

    /// A retry that came due joins the queue under its own time (T-5687).
    fn queue_retries(&mut self, now: DateTime<Utc>) {
        for entry in self.entries.values_mut() {
            let Some(retry) = entry.retry.take_if(|retry| retry.at <= now) else {
                continue;
            };
            if entry.queued.is_none() && entry.running.is_none() && !entry.stuck {
                entry.queued = Some(retry.at);
                entry.retried.push(now);
            }
        }
    }

    /// Queues one run of `key` now, outside its clock (T-4349), and names it: an entry already
    /// queued keeps its place and its run. A held pipeline (T-5370) stays held: another run
    /// would loop in the runner again.
    pub fn run_now(&mut self, key: &Key, now: DateTime<Utc>) -> Result<String, RunNow> {
        let entry = self.entries.get_mut(key).ok_or(RunNow::NotQueued)?;
        if entry.running.is_some() {
            return Err(RunNow::Running(entry.run.clone().unwrap_or_default()));
        }
        if entry.stuck {
            return Err(RunNow::Held);
        }
        Ok(stamp(*entry.queued.get_or_insert(now)))
    }

    /// The runner did not let go of `key`'s run past its cap (T-5370): the reason says what
    /// to change, and the pipeline does not start again until its stream does.
    fn stuck(&mut self, key: &Key, now: DateTime<Utc>) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.stuck = true;
            entry.queued = None;
            entry.last = Some(Last::Failed {
                at: now,
                reason: "it ran past its cap and the runner could not stop it: a step loops or \
                         never returns (a recursive jq def, a range over millions of items), so \
                         the runner restarts to free it; change the step and push again, the \
                         pipeline runs once its steps change"
                    .to_owned(),
            });
        }
    }

    fn failed_to_start(&mut self, key: &Key, reason: String, now: DateTime<Utc>) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.running = None;
            let reason = format!("the runner refused the run: {reason}");
            if let Some(run) = entry.run.clone() {
                entry.arm(&run, &reason, now);
            }
            entry.last = Some(Last::Failed { at: now, reason });
        }
    }

    /// What the Pipeline's status says: `Error` while its last run failed, else where it stands.
    pub fn outcome(&self, key: &Key) -> Option<StreamOutcome> {
        let entry = self.entries.get(key)?;
        let next = match (&entry.retry, entry.next) {
            (Some(retry), _) if !entry.stuck => format!("next try {}", stamp(retry.at)),
            (None, Some(next)) if !entry.stuck => format!("next run {}", stamp(next)),
            _ => "no further run until it changes".to_owned(),
        };
        if let Some(Last::Failed { at, reason }) = &entry.last {
            return Some(StreamOutcome::Error(format!(
                "the last run, ended {}, failed: {reason}; {next}",
                stamp(*at)
            )));
        }
        let (reason, now) = match (entry.running, entry.queued) {
            (Some(since), _) => ("Running", format!("running since {}", stamp(since))),
            (None, Some(due)) => ("Queued", format!("queued, due {}", stamp(due))),
            (None, None) => match &entry.retry {
                Some(retry) => (
                    "Retrying",
                    format!(
                        "the last run failed for a cause outside its source: {}; {next}",
                        retry.cause
                    ),
                ),
                None => ("Waiting", next),
            },
        };
        let last = match &entry.last {
            Some(Last::Finished { at, seconds }) => {
                format!("; the last run ended {} after {seconds} s", stamp(*at))
            }
            _ => String::new(),
        };
        Some(StreamOutcome::Scheduled(reason, format!("{now}{last}")))
    }
}

/// Why a run cannot be queued now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunNow {
    /// The queue does not hold the pipeline: it waits for pushes and always runs, or it is not
    /// deployed.
    NotQueued,
    /// Its run with this id is in the runner.
    Running(String),
    /// Its last run ran past its cap and the runner could not stop it: it waits for its steps
    /// to change (T-5370).
    Held,
}

/// The stream of one run with its run id written in, in place of the clock's time, so the
/// pipeline's log files the run under the id this Portal hands out (PL-62, T-4349).
fn with_run(stream: &Value, run: &str) -> Value {
    let named = format!("meta jc_run = {}", Value::from(run));
    let mut stream = stream.clone();
    fn walk(value: &mut Value, named: &str) {
        match value {
            Value::String(text) if text.contains(crate::pipeline_log::TICK) => {
                *text = text.replace(crate::pipeline_log::TICK, named);
            }
            Value::Array(items) => items.iter_mut().for_each(|item| walk(item, named)),
            Value::Object(fields) => fields.values_mut().for_each(|field| walk(field, named)),
            _ => {}
        }
    }
    walk(&mut stream, &named);
    stream
}

/// The due time a pipeline whose newest run last wrote at `last` missed by `now`: a Portal that
/// restarted with runs still queued forgot them, and on a full batch lane the pipelines that
/// waited longest are the ones it forgot, so they waited a whole period more after every
/// restart; slovensko's 300 pipelines, queued last by name, ran on neither cluster for a day
/// (T-5778). Queued under that time, they run before what came due since.
fn missed(clock: &Clock, last: DateTime<Utc>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    clock.next_after(last).filter(|due| *due <= now)
}

/// The fingerprint of a pipeline's stream as the reconciler renders it, before a run id is
/// written in (T-5825): SHA-256 of its JSON text, stable across Portal builds. The stream names
/// secrets by environment variable only, so no secret value is hashed.
pub fn stream_hash(stream: &Value) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(stream.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// One step of the queue against the runner: sweeps, then the running runs' state, then ends
/// and starts. The lock is never held across a runner call.
pub(crate) async fn tick<R: Runner>(queue: &Mutex<RunQueue>, runner: &R, now: DateTime<Utc>) {
    let lock = || {
        queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    let (sweeps, running) = {
        let mut q = lock();
        (q.sweeps(), q.running())
    };
    for key in &sweeps {
        let _ = runner.delete(key).await;
    }
    let mut states = Vec::with_capacity(running.len());
    for key in running {
        let state = runner.state(&key).await;
        states.push((key, state));
    }
    let (ended, starts) = lock().settle(states, now);
    for key in &ended {
        if !runner.delete(key).await {
            lock().stuck(key, now);
        }
    }
    let ends = std::mem::take(&mut lock().ends);
    for (key, run, finished, stream) in ends {
        runner.ended(&key, &run, finished, &stream).await;
    }
    for (key, stream) in starts {
        if let Err(reason) = runner.start(&key, &stream).await {
            lock().failed_to_start(&key, reason, now);
            let _ = runner.delete(&key).await;
        }
    }
}

/// The stream of one run: every clock fires once, at once. An `http_client` input (a GTFS-RT
/// feed) polls without end, so it becomes the one tick and the `http` processor an `http`
/// DataSource is rendered with (PL-47).
///
/// A write that still fails after its own retries ends the run as a `failed` line with the
/// target's words. A resident stream nacks it back to its input to replay it, but a run has no
/// later message to wait for: the replay looped until [`RUN_TIMEOUT`] and logged nothing
/// (T-4952). The next run writes the record again, each write an idempotent upsert.
pub fn one_run(stream: &Value) -> Value {
    let report = outcome_url(stream);
    let mut stream = stream.clone();
    if let Some(input) = stream.get_mut("input") {
        once(input, report.as_deref());
    }
    if report.is_some() {
        if let Some(processors) = stream
            .pointer_mut("/pipeline/processors")
            .and_then(Value::as_array_mut)
        {
            processors.push(serde_json::json!({ "catch": [{ "mutation": format!(
                "meta {} = \"the write failed: \" + error().re_replace_all(\"[a-z]+://[^\\\\s'\\\"]+\", \"the target\")",
                crate::pipeline_log::FAILED_KEY
            ) }] }));
        }
    }
    stream
}

/// Where the stream's outcome sink posts its reports, if it has one.
fn outcome_url(value: &Value) -> Option<String> {
    match value {
        Value::Object(fields) => {
            if fields.get("label").and_then(Value::as_str)
                == Some(crate::pipeline_log::SINK_LABELS[1])
            {
                if let Some(url) = fields
                    .get("http_client")
                    .and_then(|client| client.get("url"))
                    .and_then(Value::as_str)
                {
                    return Some(url.to_owned());
                }
            }
            fields.values().find_map(outcome_url)
        }
        Value::Array(items) => items.iter().find_map(outcome_url),
        _ => None,
    }
}

fn once(input: &mut Value, report: Option<&str>) {
    if let Some(inputs) = input
        .get_mut("broker")
        .and_then(|b| b.get_mut("inputs"))
        .and_then(Value::as_array_mut)
    {
        inputs.iter_mut().for_each(|input| once(input, report));
        return;
    }
    let Some(fields) = input.as_object_mut() else {
        return;
    };
    if let Some(generate) = fields.get_mut("generate").and_then(Value::as_object_mut) {
        generate.insert("count".to_owned(), Value::from(1));
        generate.insert("interval".to_owned(), Value::from(""));
        return;
    }
    let Some(Value::Object(mut client)) = fields.remove("http_client") else {
        return;
    };
    let meta: String = match client.remove("metadata") {
        Some(Value::Object(meta)) => meta
            .iter()
            .map(|(name, value)| format!("meta {name} = {value}\n"))
            .collect(),
        _ => String::new(),
    };
    let mut processors = vec![serde_json::json!({ "try": [{ "http": Value::Object(client) }] })];
    // A refused request is a failed run with the source's words, not an empty one (T-4428).
    if let Some(url) = report {
        processors.push(crate::pipeline_log::source_failure(url));
    }
    processors.push(serde_json::json!({ "mutation": super::streams::DROP_ERRORED }));
    if let Some(Value::Array(own)) = fields.remove("processors") {
        processors.extend(own);
    }
    fields.insert(
        "generate".to_owned(),
        serde_json::json!({
            "count": 1,
            "interval": "",
            "mapping": format!("root = \"\"\n{meta}{}", crate::pipeline_log::TICK),
        }),
    );
    fields.insert("processors".to_owned(), Value::Array(processors));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("a time")
            .with_timezone(&Utc)
    }

    fn key(name: &str) -> Key {
        ("p".to_owned(), name.to_owned())
    }

    fn planned(clock: Clock) -> Planned {
        Planned {
            stream: serde_json::json!({"input": {"generate": {"count": 1}}}),
            clock,
            replace: false,
        }
    }

    /// The runner as the queue sees it: each run started is held until the test ends it.
    #[derive(Default)]
    struct Fake {
        held: Mutex<HashMap<Key, RunState>>,
        log: Mutex<Vec<String>>,
        refuse: Mutex<BTreeSet<String>>,
        /// Runs whose stop hangs: a step that never returns.
        stuck: Mutex<BTreeSet<String>>,
        sent: Mutex<HashMap<String, Value>>,
        ends: Mutex<Vec<(String, String, bool)>>,
        /// The stream hash each ended run was told with, by pipeline.
        ran_streams: Mutex<Vec<(String, String)>>,
    }

    impl Fake {
        fn end(&self, name: &str, state: RunState) {
            self.held.lock().unwrap().insert(key(name), state);
        }
        fn held(&self) -> Vec<String> {
            let mut names: Vec<String> = self
                .held
                .lock()
                .unwrap()
                .keys()
                .map(|k| k.1.clone())
                .collect();
            names.sort();
            names
        }
        fn log(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }
    }

    impl Runner for Fake {
        async fn start(&self, key: &Key, stream: &Value) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("start {}", key.1));
            self.sent
                .lock()
                .unwrap()
                .insert(key.1.clone(), stream.clone());
            if self.refuse.lock().unwrap().contains(&key.1) {
                return Err("400 bad config".to_owned());
            }
            self.held
                .lock()
                .unwrap()
                .insert(key.clone(), RunState::Running);
            Ok(())
        }
        async fn state(&self, key: &Key) -> RunState {
            self.held
                .lock()
                .unwrap()
                .get(key)
                .copied()
                .unwrap_or(RunState::Gone)
        }
        async fn delete(&self, key: &Key) -> bool {
            self.log.lock().unwrap().push(format!("delete {}", key.1));
            if self.stuck.lock().unwrap().contains(&key.1) {
                return false;
            }
            self.held.lock().unwrap().remove(key);
            true
        }
        async fn ended(&self, key: &Key, run: &str, finished: bool, stream: &str) {
            let end = (key.1.clone(), run.to_owned(), finished);
            self.ends.lock().unwrap().push(end);
            let ran = (key.1.clone(), stream.to_owned());
            self.ran_streams.lock().unwrap().push(ran);
        }
    }

    /// A snapshot pipeline judges its deletes on the run that ended, by the id its log names it
    /// under, and only a run the runner finished counts as complete (T-4891).
    #[tokio::test]
    async fn each_ended_run_is_told_by_its_id_and_whether_it_finished() {
        let queue = Mutex::new(RunQueue::new(2, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T05:00:00Z");
        let wanted = ["a", "b"]
            .map(|n| (key(n), planned(Clock::Once)))
            .into_iter()
            .collect();
        queue.lock().unwrap().plan(wanted, now);
        step(&queue, &fake, now).await;
        assert!(fake.ends.lock().unwrap().is_empty(), "nothing ended yet");
        fake.end("a", RunState::Finished);
        step(&queue, &fake, now + Span::minutes(1)).await;
        step(&queue, &fake, now + Span::minutes(15)).await;
        assert_eq!(
            *fake.ends.lock().unwrap(),
            [
                ("a".to_owned(), "2026-10-17T05:00:00Z".to_owned(), true),
                ("b".to_owned(), "2026-10-17T05:00:00Z".to_owned(), false),
            ]
        );
    }

    /// One tick, the reconciler having planned just before it as it does on every pass.
    async fn step(queue: &Mutex<RunQueue>, fake: &Fake, now: DateTime<Utc>) {
        queue.lock().unwrap().planned_at = Some(now);
        tick(queue, fake, now).await;
    }

    fn weekly() -> Clock {
        Clock::Every(Duration::from_secs(168 * 3600), Duration::ZERO)
    }

    const DM61: &str =
        "entity type `Tree` is not a class of the space's data model `praha-mesto` (DM-61)";

    /// One reconciler pass over pipeline `a` (weekly, run before) whose newest logged run, when
    /// given, failed for a cause outside its source.
    fn pass(queue: &Mutex<RunQueue>, failed: Option<&str>, rules: u64, now: DateTime<Utc>) {
        let mut q = queue.lock().unwrap();
        q.ran([(key("a"), now)].into_iter().collect());
        q.rules([("p".to_owned(), rules)].into_iter().collect());
        q.failed(
            failed
                .map(|run| (key("a"), (run.to_owned(), DM61.to_owned())))
                .into_iter()
                .collect(),
        );
        q.plan([(key("a"), planned(weekly()))].into_iter().collect(), now);
    }

    fn said(queue: &Mutex<RunQueue>) -> String {
        match queue.lock().unwrap().outcome(&key("a")) {
            Some(StreamOutcome::Scheduled(reason, said)) => format!("{reason}: {said}"),
            other => format!("{other:?}"),
        }
    }

    /// One run of `a` from start to delete at `due`.
    async fn retried_at(queue: &Mutex<RunQueue>, fake: &Fake, due: DateTime<Utc>) {
        step(queue, fake, due - Span::minutes(1)).await;
        assert!(fake.log().is_empty(), "nothing runs before {due}");
        step(queue, fake, due).await;
        assert_eq!(fake.log(), ["start a"], "the retry runs at {due}");
        fake.end("a", RunState::Finished);
        step(queue, fake, due + Span::minutes(1)).await;
        assert_eq!(fake.log(), ["delete a"]);
    }

    /// T-5687: 37 Praha pipelines failed DM-61 on a stale model and waited 168 h. A run the model,
    /// a policy or the broker refused is retried at 15 min, 1 h and 4 h, and then waits for its
    /// clock; seeing the same failed run on later passes arms nothing more.
    #[tokio::test]
    async fn a_run_the_model_refused_is_retried_with_backoff_then_waits_for_its_clock() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-25T16:00:00Z");
        pass(&queue, Some("2026-10-25T14:00:00Z"), 1, now);
        step(&queue, &fake, now).await;
        assert_eq!(fake.log(), ["delete a"], "the leftover sweep, and no run");
        let first = said(&queue);
        assert!(first.starts_with("Retrying: "), "{first}");
        assert!(
            first.contains("(DM-61)") && first.contains("next try 2026-10-25T16:15:00Z"),
            "{first}"
        );
        retried_at(&queue, &fake, at("2026-10-25T16:15:00Z")).await;

        pass(
            &queue,
            Some("2026-10-25T16:15:00Z"),
            1,
            at("2026-10-25T16:20:00Z"),
        );
        pass(
            &queue,
            Some("2026-10-25T16:15:00Z"),
            1,
            at("2026-10-25T16:40:00Z"),
        );
        assert!(
            said(&queue).contains("next try 2026-10-25T17:20:00Z"),
            "{}",
            said(&queue)
        );
        retried_at(&queue, &fake, at("2026-10-25T17:20:00Z")).await;

        pass(
            &queue,
            Some("2026-10-25T17:20:00Z"),
            1,
            at("2026-10-25T17:30:00Z"),
        );
        assert!(
            said(&queue).contains("next try 2026-10-25T21:30:00Z"),
            "{}",
            said(&queue)
        );
        retried_at(&queue, &fake, at("2026-10-25T21:30:00Z")).await;

        pass(
            &queue,
            Some("2026-10-25T21:30:00Z"),
            1,
            at("2026-10-25T21:40:00Z"),
        );
        assert_eq!(said(&queue), "Waiting: next run 2026-10-29T00:00:00Z; the last run ended 2026-10-25T21:31:00Z after 60 s");
    }

    /// A retry never waits past the clock: a daily pipeline that failed at 04:30 runs at 05:00.
    #[test]
    fn a_retry_due_after_the_clock_is_left_to_the_clock() {
        let mut queue = RunQueue::new(1, 1);
        let now = at("2026-10-25T04:50:00Z");
        queue.ran([(key("a"), now)].into_iter().collect());
        queue.failed(
            [(
                key("a"),
                ("2026-10-25T04:30:00Z".to_owned(), DM61.to_owned()),
            )]
            .into(),
        );
        queue.plan([(key("a"), planned(daily_at_5()))].into(), now);
        assert!(
            matches!(queue.outcome(&key("a")), Some(StreamOutcome::Scheduled("Waiting", said)) if said == "next run 2026-10-25T05:00:00Z")
        );
    }

    /// A pass in which the project's spaces, models or policies changed runs its pending retries
    /// at once; a run that then writes clean starts the backoff over, and a log that names no
    /// outside failure (a source that answered 404) leaves the pipeline to its clock.
    #[tokio::test]
    async fn a_rules_change_runs_the_retry_at_once_and_a_clean_run_resets_the_backoff() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-25T16:00:00Z");
        pass(&queue, Some("2026-10-25T14:00:00Z"), 1, now);
        step(&queue, &fake, now).await;
        fake.log();
        pass(
            &queue,
            Some("2026-10-25T14:00:00Z"),
            2,
            now + Span::minutes(2),
        );
        assert!(
            said(&queue).contains("next try 2026-10-25T16:02:00Z"),
            "{}",
            said(&queue)
        );
        step(&queue, &fake, now + Span::minutes(2)).await;
        assert_eq!(fake.log(), ["start a"]);
        fake.end("a", RunState::Finished);
        step(&queue, &fake, now + Span::minutes(3)).await;
        fake.log();

        pass(&queue, None, 2, now + Span::minutes(4));
        assert!(
            said(&queue).starts_with("Waiting: next run 2026-10-29T00:00:00Z"),
            "{}",
            said(&queue)
        );
        pass(
            &queue,
            Some("2026-10-25T16:30:00Z"),
            2,
            now + Span::minutes(40),
        );
        assert!(
            said(&queue).contains("next try 2026-10-25T16:55:00Z"),
            "the backoff starts over: {}",
            said(&queue)
        );
    }

    /// However often its cause clears and comes back, a pipeline is retried at most
    /// [`RETRIES_PER_DAY`] times a day.
    #[tokio::test]
    async fn retries_are_bounded_per_day() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let mut now = at("2026-10-25T00:00:00Z");
        pass(&queue, None, 1, now);
        step(&queue, &fake, now).await;
        fake.log();
        for n in 0..RETRIES_PER_DAY {
            pass(&queue, Some(&format!("failed-{n}")), 1, now);
            retried_at(&queue, &fake, now + Span::minutes(15)).await;
            now += Span::minutes(20);
            pass(&queue, None, 1, now);
        }
        pass(&queue, Some("failed-again"), 1, now);
        assert!(!said(&queue).contains("next try"), "{}", said(&queue));
    }

    /// A run the runner refused to load is retried like a refused write.
    #[tokio::test]
    async fn a_run_the_runner_refused_is_retried_in_15_minutes() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-25T16:00:00Z");
        pass(&queue, None, 1, now);
        queue
            .lock()
            .unwrap()
            .run_now(&key("a"), now)
            .expect("queued");
        fake.refuse.lock().unwrap().insert("a".to_owned());
        step(&queue, &fake, now).await;
        assert_eq!(fake.log(), ["delete a", "start a", "delete a"]);
        pass(&queue, None, 1, now + Span::minutes(1));
        assert!(
            matches!(queue.lock().unwrap().outcome(&key("a")), Some(StreamOutcome::Error(e)) if e.contains("400 bad config") && e.ends_with("next try 2026-10-25T16:15:00Z"))
        );
        fake.refuse.lock().unwrap().clear();
        step(&queue, &fake, now + Span::minutes(15)).await;
        assert_eq!(fake.log(), ["start a"]);
    }

    fn daily_at_5() -> Clock {
        Clock::of(None, Some("0 5 * * *")).expect("a schedule")
    }

    fn weekly_run(replace: bool, count: u64) -> Planned {
        Planned {
            stream: serde_json::json!({"input": {"generate": {"count": count}}}),
            clock: Clock::of(None, Some("0 5 * * 1")).expect("a schedule"),
            replace,
        }
    }

    fn next_of(queue: &RunQueue, name: &str) -> Option<DateTime<Utc>> {
        queue.entries.get(&key(name)).and_then(|entry| entry.next)
    }

    /// T-5824: a replace pipeline whose mapping changed is due at once, since its entities keep
    /// what the old mapping wrote until it runs; an upsert one, a changed clock alone and an
    /// unchanged pipeline keep their clock.
    #[test]
    fn a_changed_replace_stream_is_due_at_once_and_the_rest_keep_their_clock() {
        let mut queue = RunQueue::new(1, 1);
        let start = at("2026-10-27T06:00:00Z");
        let later = at("2026-10-28T09:00:00Z");
        let names = ["replaced", "upserted", "reclocked", "same"];
        let first: BTreeMap<Key, Planned> = names
            .iter()
            .map(|name| (key(name), weekly_run(*name != "upserted", 1)))
            .collect();
        queue.plan(first, start);
        let monday = weekly_run(true, 1).clock.next_after(start);
        assert_eq!(monday, Some(at("2026-11-02T05:00:00Z")));
        for name in names {
            assert_eq!(next_of(&queue, name), monday, "{name}");
        }

        let mut reclocked = weekly_run(true, 1);
        reclocked.clock = daily_at_5();
        let second: BTreeMap<Key, Planned> = [
            (key("replaced"), weekly_run(true, 2)),
            (key("upserted"), weekly_run(false, 2)),
            (key("reclocked"), reclocked),
            (key("same"), weekly_run(true, 1)),
        ]
        .into_iter()
        .collect();
        queue.plan(second, later);
        assert_eq!(next_of(&queue, "replaced"), Some(later));
        assert_eq!(next_of(&queue, "upserted"), monday);
        assert_eq!(next_of(&queue, "reclocked"), daily_at_5().next_after(later));
        assert_eq!(next_of(&queue, "same"), monday);
    }

    /// T-5825: a Portal that restarts with a replace pipeline's stream changed since its newest
    /// logged run runs it at once; the same stream, a run logged without a hash and an upsert
    /// pipeline keep their clock.
    #[test]
    fn a_replace_stream_changed_across_a_restart_is_due_at_once() {
        let mut queue = RunQueue::new(1, 1);
        let now = at("2026-10-27T06:00:00Z");
        let last = now - Span::hours(1);
        let names = ["replaced", "same", "unhashed", "upserted"];
        queue.ran(names.iter().map(|name| (key(name), last)).collect());
        let old = stream_hash(&weekly_run(true, 1).stream);
        queue.streams(
            ["replaced", "same", "upserted"]
                .iter()
                .map(|name| (key(name), old.clone()))
                .collect(),
        );
        let wanted: BTreeMap<Key, Planned> = [
            (key("replaced"), weekly_run(true, 2)),
            (key("same"), weekly_run(true, 1)),
            (key("unhashed"), weekly_run(true, 2)),
            (key("upserted"), weekly_run(false, 2)),
        ]
        .into_iter()
        .collect();
        queue.plan(wanted, now);
        let monday = Some(at("2026-11-02T05:00:00Z"));
        assert_eq!(next_of(&queue, "replaced"), Some(now));
        assert_eq!(next_of(&queue, "same"), monday);
        assert_eq!(next_of(&queue, "unhashed"), monday);
        assert_eq!(next_of(&queue, "upserted"), monday);
    }

    /// T-5825: a run that ends is told with the hash of the stream it started with, even when
    /// its stream changed while it ran, so the log never pairs a run with a stream it did not run.
    #[tokio::test]
    async fn an_ended_run_is_told_the_hash_of_the_stream_it_ran() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-27T06:00:00Z");
        let mut first = weekly_run(true, 1);
        first.clock = Clock::Once;
        queue
            .lock()
            .unwrap()
            .plan([(key("a"), first.clone())].into(), now);
        step(&queue, &fake, now).await;
        assert!(fake.log().contains(&"start a".to_owned()));
        let mut second = weekly_run(true, 2);
        second.clock = Clock::Once;
        queue.lock().unwrap().plan([(key("a"), second)].into(), now);
        fake.end("a", RunState::Finished);
        step(&queue, &fake, now + Span::minutes(1)).await;
        assert_eq!(
            fake.ran_streams.lock().unwrap().first(),
            Some(&("a".to_owned(), stream_hash(&first.stream)))
        );
    }

    /// The hash is the stream's whole JSON text in hex: stable across builds, and any change of
    /// the stream changes it.
    #[test]
    fn a_stream_hash_is_sha256_of_its_text() {
        let stream = serde_json::json!({"input": {"generate": {"count": 1}}});
        assert_eq!(
            stream_hash(&stream),
            "9406b26e4cbbee539aeb8444702f2e6394d766abf9f0a9d94fa5489252e93c31"
        );
        assert_ne!(
            stream_hash(&stream),
            stream_hash(&weekly_run(true, 2).stream)
        );
    }

    /// T-5824: a seed sync that changes many replace streams queues them all at once, and the
    /// batch lane still starts one at a time.
    #[tokio::test]
    async fn changed_replace_streams_run_through_the_lane_one_at_a_time() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let start = at("2026-10-27T06:00:00Z");
        let names = ["a", "b", "c"];
        let wanted = |count| -> BTreeMap<Key, Planned> {
            names
                .iter()
                .map(|name| (key(name), weekly_run(true, count)))
                .collect()
        };
        queue.lock().unwrap().plan(wanted(1), start);
        step(&queue, &fake, start).await;
        assert!(
            !fake.log().iter().any(|line| line.starts_with("start")),
            "nothing is due yet"
        );

        let later = start + Span::hours(1);
        queue.lock().unwrap().plan(wanted(2), later);
        let mut previous: Option<&str> = None;
        for name in names {
            if let Some(previous) = previous {
                fake.end(previous, RunState::Finished);
            }
            step(&queue, &fake, later).await;
            assert!(fake.log().contains(&format!("start {name}")), "{name}");
            let running: Vec<&str> = names
                .into_iter()
                .filter(|n| fake.held.lock().unwrap().get(&key(n)) == Some(&RunState::Running))
                .collect();
            assert_eq!(running, [name], "one run at a time");
            previous = Some(name);
        }
    }

    #[test]
    fn only_sources_that_wait_for_pushes_are_resident() {
        let runner = |name: &str| DataSourceType::Runner(name.to_owned());
        for resident in [
            DataSourceType::Mqtt,
            DataSourceType::WebSocket,
            runner("http_server"),
            runner("socket_server"),
            runner("kafka"),
            runner("nats"),
            runner("nats_jetstream"),
            runner("amqp_0_9"),
            runner("amqp_1"),
            runner("redis_streams"),
        ] {
            assert!(is_resident(&resident), "{resident}");
        }
        for queued in [
            DataSourceType::Http,
            DataSourceType::GtfsRt,
            runner("file"),
            runner("sql_select"),
            runner("aws_s3"),
            runner("csv"),
            runner("http_client"),
        ] {
            assert!(!is_resident(&queued), "{queued}");
        }
    }

    #[test]
    fn a_period_starts_at_the_time_its_schedule_names() {
        let now = at("2026-10-16T13:47:30Z");
        let next = |period: &str, schedule: &str| {
            Clock::of(Some(period), Some(schedule))
                .unwrap()
                .next_after(now)
        };
        // The podujatia seeds of T-5719: before, every one of them ran at 00:00Z.
        assert_eq!(next("24h", "55 21 * * *"), Some(at("2026-10-16T21:55:00Z")));
        assert_eq!(next("24h", "10 2 * * *"), Some(at("2026-10-17T02:10:00Z")));
        assert_eq!(
            next("3h", "7 1-23/3 * * *"),
            Some(at("2026-10-16T16:07:00Z"))
        );
        assert_eq!(
            next("168h", "3 5 * * 1"),
            Some(at("2026-10-19T05:03:00Z")),
            "Monday"
        );
        // The period keeps how often; a schedule that disagrees moves only the start.
        assert_eq!(next("1h", "20 3 * * *"), Some(at("2026-10-16T14:20:00Z")));
        assert_eq!(
            Clock::of(Some("24h"), Some("61 * * * *")),
            Err("schedule `61 * * * *` is not five cron fields (`0 5 * * *`)".to_owned())
        );
    }

    #[test]
    fn a_clock_is_the_period_else_the_schedule_else_once() {
        assert_eq!(Clock::of(None, None), Ok(Clock::Once));
        assert_eq!(Clock::of(Some(" "), Some("")), Ok(Clock::Once));
        assert_eq!(
            Clock::of(Some("5m"), Some("0 5 * * *")),
            Ok(Clock::Every(Duration::from_secs(300), Duration::ZERO))
        );
        assert!(Clock::of(Some("0s"), None).is_err());
        assert!(Clock::of(Some("daily"), None).is_err());
        for bad in [
            "@daily",
            "0 5 * *",
            "61 * * * *",
            "0 5 * * MON",
            "*/0 * * * *",
            "5-1 * * * *",
        ] {
            assert!(Clock::of(None, Some(bad)).is_err(), "{bad}");
        }
        let now = at("2026-10-16T13:47:30Z");
        assert_eq!(
            daily_at_5().next_after(now),
            Some(at("2026-10-17T05:00:00Z"))
        );
        assert_eq!(
            Clock::Every(Duration::from_secs(86_400), Duration::ZERO).next_after(now),
            Some(at("2026-10-17T00:00:00Z"))
        );
        let cron = |s: &str| Clock::of(None, Some(s)).unwrap().next_after(now);
        assert_eq!(cron("*/10 * * * *"), Some(at("2026-10-16T13:50:00Z")));
        assert_eq!(
            cron("41 3 * * 1"),
            Some(at("2026-10-19T03:41:00Z")),
            "Monday"
        );
        assert_eq!(
            cron("42 1 * * 7"),
            Some(at("2026-10-18T01:42:00Z")),
            "7 is Sunday"
        );
        assert_eq!(
            cron("0 0 1,15 * 5"),
            Some(at("2026-10-23T00:00:00Z")),
            "day or weekday"
        );
        assert_eq!(cron("0 0 30 2 *"), None, "30 February never comes");
        assert_eq!(
            Clock::Every(Duration::from_secs(30), Duration::ZERO).lane(now),
            Lane::Short
        );
        assert_eq!(
            Clock::of(None, Some("* * * * *")).unwrap().lane(now),
            Lane::Short
        );
        assert_eq!(
            Clock::Every(SHORT_BELOW, Duration::ZERO).lane(now),
            Lane::Batch
        );
        assert_eq!(daily_at_5().lane(now), Lane::Batch);
        assert_eq!(Clock::Once.lane(now), Lane::Batch);
    }

    #[tokio::test]
    async fn a_pipeline_whose_account_has_no_client_fails_unstarted_and_frees_its_lane() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T04:59:00Z");
        let wanted = ["a", "b"]
            .map(|n| (key(n), planned(daily_at_5())))
            .into_iter()
            .collect();
        queue.lock().unwrap().plan(wanted, now);
        step(&queue, &fake, now).await;
        fake.log();
        // a's account is `p/pl-a`; the reconcile could not make its client (T-4426). `*` (a
        // whole wave that failed) and another project's account hold nobody back.
        queue.lock().unwrap().unbound(
            ["p/pl-a", "*", "q/pl-b"]
                .map(str::to_owned)
                .into_iter()
                .collect(),
        );

        let five = at("2026-10-17T05:00:00Z");
        step(&queue, &fake, five).await;
        assert_eq!(fake.log(), ["start b"], "a never takes the one batch slot");
        match queue.lock().unwrap().outcome(&key("a")) {
            Some(StreamOutcome::Error(said)) => {
                assert!(said.contains("no client for its account"), "{said}");
                assert!(said.contains("401"), "{said}");
            }
            other => panic!("a failed run, not {other:?}"),
        }

        // Once the client is there, a run of a starts like any other.
        queue.lock().unwrap().unbound(HashSet::new());
        fake.end("b", RunState::Finished);
        queue
            .lock()
            .unwrap()
            .run_now(&key("a"), five)
            .expect("queued");
        step(&queue, &fake, five + Span::seconds(60)).await;
        assert_eq!(fake.log(), ["delete b", "start a"]);
    }

    #[tokio::test]
    async fn three_pipelines_due_at_five_run_one_after_another_in_name_order_and_leave() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T04:59:00Z");
        let wanted = ["c", "a", "b"]
            .map(|n| (key(n), planned(daily_at_5())))
            .into_iter()
            .collect();
        queue.lock().unwrap().plan(wanted, now);
        step(&queue, &fake, now).await;
        assert_eq!(
            fake.log(),
            ["delete a", "delete b", "delete c"],
            "each leftover is removed once"
        );

        let five = at("2026-10-17T05:00:00Z");
        step(&queue, &fake, five).await;
        assert_eq!(fake.log(), ["start a"]);
        step(&queue, &fake, five + Span::seconds(5)).await;
        assert_eq!(fake.held(), ["a"], "b waits while a runs");
        assert_eq!(
            queue.lock().unwrap().outcome(&key("b")),
            Some(StreamOutcome::Scheduled(
                "Queued",
                "queued, due 2026-10-17T05:00:00Z".to_owned()
            ))
        );
        for (name, next) in [("a", "b"), ("b", "c")] {
            fake.end(name, RunState::Finished);
            step(&queue, &fake, five + Span::seconds(60)).await;
            assert_eq!(
                fake.log(),
                [format!("delete {name}"), format!("start {next}")]
            );
            assert_eq!(fake.held(), [next]);
        }
        fake.end("c", RunState::Finished);
        step(&queue, &fake, five + Span::seconds(120)).await;
        assert_eq!(fake.log(), ["delete c"]);
        assert!(
            fake.held().is_empty(),
            "the runner holds nothing once the queue drained"
        );
        assert_eq!(
            queue.lock().unwrap().outcome(&key("a")),
            Some(StreamOutcome::Scheduled(
                "Waiting",
                "next run 2026-10-18T05:00:00Z; the last run ended 2026-10-17T05:01:00Z after 60 s"
                    .to_owned()
            ))
        );
    }

    #[tokio::test]
    async fn a_failed_or_timed_out_run_is_deleted_and_the_next_starts() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T05:00:00Z");
        let wanted = ["a", "b", "c", "d"]
            .map(|n| (key(n), planned(Clock::Once)))
            .into_iter()
            .collect();
        queue.lock().unwrap().plan(wanted, now);
        fake.refuse.lock().unwrap().insert("a".to_owned());
        step(&queue, &fake, now).await;
        fake.log();
        assert!(fake.held().is_empty(), "a refused run is not held");
        assert!(
            matches!(queue.lock().unwrap().outcome(&key("a")), Some(StreamOutcome::Error(e)) if e.contains("400 bad config"))
        );

        step(&queue, &fake, now).await;
        assert_eq!(fake.log(), ["start b"]);
        step(&queue, &fake, now + Span::minutes(14)).await;
        assert_eq!(fake.held(), ["b"]);
        step(&queue, &fake, now + Span::minutes(15)).await;
        assert_eq!(
            fake.log(),
            ["delete b", "start c"],
            "a run past the timeout is deleted"
        );
        assert!(
            matches!(queue.lock().unwrap().outcome(&key("b")), Some(StreamOutcome::Error(e)) if e.contains("15 min"))
        );

        fake.held.lock().unwrap().remove(&key("c"));
        step(&queue, &fake, now + Span::minutes(16)).await;
        assert_eq!(
            fake.log(),
            ["delete c", "start d"],
            "a run the runner lost is failed and the next starts"
        );
        assert!(
            matches!(queue.lock().unwrap().outcome(&key("c")), Some(StreamOutcome::Error(e)) if e.contains("no longer holds"))
        );
    }

    /// A run past its cap whose stop hangs (a step that loops, T-5370) is reported with what to
    /// change and never started again until its stream changes; the other pipeline goes on.
    #[tokio::test]
    async fn a_run_the_runner_cannot_stop_is_reported_and_held_until_it_changes() {
        let queue = Mutex::new(RunQueue::new(2, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T05:00:00Z");
        let hourly = || planned(Clock::Every(Duration::from_secs(3600), Duration::ZERO));
        let wanted = || {
            ["loop", "ok"]
                .map(|n| (key(n), hourly()))
                .into_iter()
                .collect()
        };
        queue.lock().unwrap().plan(wanted(), now);
        queue.lock().unwrap().run_now(&key("loop"), now).unwrap();
        queue.lock().unwrap().run_now(&key("ok"), now).unwrap();
        fake.stuck.lock().unwrap().insert("loop".to_owned());
        step(&queue, &fake, now).await;
        fake.log();
        fake.end("ok", RunState::Finished);
        step(&queue, &fake, now + Span::minutes(15)).await;
        assert_eq!(fake.log(), ["delete loop", "delete ok"]);
        let outcome = queue.lock().unwrap().outcome(&key("loop"));
        let Some(StreamOutcome::Error(said)) = outcome else {
            panic!("a stuck run is an error, got {outcome:?}");
        };
        assert!(said.contains("could not stop it"), "{said}");
        assert!(said.contains("no further run until it changes"), "{said}");
        assert_eq!(
            queue
                .lock()
                .unwrap()
                .run_now(&key("loop"), now + Span::minutes(15)),
            Err(RunNow::Held),
            "Run now on a held pipeline is refused, not queued for a start that never comes"
        );

        step(&queue, &fake, now + Span::hours(1)).await;
        assert_eq!(
            fake.log(),
            ["start ok"],
            "the stuck pipeline is not started again"
        );

        let mut changed: BTreeMap<Key, Planned> = wanted();
        changed.insert(
            key("loop"),
            Planned {
                stream: serde_json::json!({"input": {"generate": {"count": 2}}}),
                clock: Clock::Once,
                replace: false,
            },
        );
        queue.lock().unwrap().plan(changed, now + Span::hours(1));
        step(&queue, &fake, now + Span::hours(1)).await;
        assert_eq!(fake.log(), ["start loop"], "a changed stream runs again");
    }

    /// A due time that passes while a pipeline is held does not queue it, so a changed stream
    /// on the same clock waits for its next due time instead of starting a stale run (T-5370).
    #[tokio::test]
    async fn a_held_pipeline_misses_its_due_times_and_runs_at_the_next_after_a_change() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T05:00:00Z");
        let hourly = |count: u64| Planned {
            stream: serde_json::json!({"input": {"generate": {"count": count}}}),
            clock: Clock::Every(Duration::from_secs(3600), Duration::ZERO),
            replace: false,
        };
        let wanted = |count| [(key("loop"), hourly(count))].into_iter().collect();
        queue.lock().unwrap().plan(wanted(1), now);
        queue.lock().unwrap().run_now(&key("loop"), now).unwrap();
        fake.stuck.lock().unwrap().insert("loop".to_owned());
        step(&queue, &fake, now).await;
        step(&queue, &fake, now + Span::minutes(15)).await;
        fake.log();

        step(&queue, &fake, now + Span::minutes(70)).await;
        queue
            .lock()
            .unwrap()
            .plan(wanted(2), now + Span::minutes(70));
        step(&queue, &fake, now + Span::minutes(70)).await;
        assert_eq!(
            fake.log(),
            Vec::<String>::new(),
            "the due time passed while held queued nothing"
        );
        step(&queue, &fake, now + Span::minutes(131)).await;
        step(&queue, &fake, now + Span::minutes(131)).await;
        assert_eq!(fake.log(), ["start loop"], "it runs at its next due time");
    }

    /// A poll every 30 s is capped at two minutes and one every 5 min at the batch cap, so a
    /// looping poll frees its short lane long before 15 min (T-5370).
    #[test]
    fn a_short_poll_is_capped_at_three_periods_and_two_minutes_at_least() {
        let entry = |clock| Entry {
            stream: Value::Null,
            clock,
            lane: Lane::Short,
            next: None,
            queued: None,
            running: None,
            run: None,
            run_stream: None,
            sweep: false,
            stuck: false,
            last: None,
            retry: None,
            armed_for: None,
            attempts: 0,
            retried: Vec::new(),
        };
        let cap = |clock| entry(clock).cap(RUN_TIMEOUT).as_secs();
        assert_eq!(
            cap(Clock::Every(Duration::from_secs(30), Duration::ZERO)),
            120
        );
        assert_eq!(
            cap(Clock::Every(Duration::from_secs(60), Duration::ZERO)),
            180
        );
        assert_eq!(
            cap(Clock::Every(Duration::from_secs(240), Duration::ZERO)),
            720
        );
        assert_eq!(
            cap(Clock::Every(Duration::from_secs(300), Duration::ZERO)),
            900
        );
        assert_eq!(cap(Clock::Once), 900);
    }

    #[tokio::test]
    async fn a_pipeline_due_again_while_queued_or_running_stays_one_entry() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T04:59:00Z");
        let every_five = Clock::of(None, Some("*/5 * * * *")).unwrap();
        let wanted = [
            (key("long"), planned(daily_at_5())),
            (key("often"), planned(every_five)),
        ];
        queue
            .lock()
            .unwrap()
            .plan(wanted.into_iter().collect(), now);
        // Both in the batch lane: `often` waits behind `long` across three of its due times.
        step(&queue, &fake, now).await;
        fake.log();
        for minute in ["05:00", "05:05", "05:10"] {
            step(&queue, &fake, at(&format!("2026-10-17T{minute}:00Z"))).await;
        }
        assert_eq!(fake.log(), ["start long"]);
        fake.end("long", RunState::Finished);
        step(&queue, &fake, at("2026-10-17T05:11:00Z")).await;
        assert_eq!(fake.log(), ["delete long", "start often"]);
        fake.end("often", RunState::Finished);
        step(&queue, &fake, at("2026-10-17T05:12:00Z")).await;
        assert_eq!(fake.log(), ["delete often"], "three due times ran once");
        // Due twice while running: still one run.
        step(&queue, &fake, at("2026-10-17T05:15:00Z")).await;
        step(&queue, &fake, at("2026-10-17T05:20:00Z")).await;
        step(&queue, &fake, at("2026-10-17T05:25:00Z")).await;
        assert_eq!(fake.log(), ["start often"]);
        fake.end("often", RunState::Finished);
        step(&queue, &fake, at("2026-10-17T05:26:00Z")).await;
        assert_eq!(fake.log(), ["delete often"]);
    }

    #[tokio::test]
    async fn a_short_poll_runs_while_a_batch_run_is_in_progress_and_two_slots_run_two() {
        let queue = Mutex::new(RunQueue::new(2, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T04:59:00Z");
        let wanted = [
            (key("harvest-a"), planned(daily_at_5())),
            (key("harvest-b"), planned(daily_at_5())),
            (key("harvest-c"), planned(daily_at_5())),
            (
                key("vehicles"),
                planned(Clock::Every(Duration::from_secs(30), Duration::ZERO)),
            ),
        ];
        queue
            .lock()
            .unwrap()
            .plan(wanted.into_iter().collect(), now);
        step(&queue, &fake, now).await;
        step(&queue, &fake, at("2026-10-17T05:00:00Z")).await;
        assert_eq!(fake.held(), ["harvest-a", "harvest-b", "vehicles"]);
        fake.end("vehicles", RunState::Finished);
        step(&queue, &fake, at("2026-10-17T05:00:20Z")).await;
        step(&queue, &fake, at("2026-10-17T05:00:30Z")).await;
        assert_eq!(
            fake.held(),
            ["harvest-a", "harvest-b", "vehicles"],
            "the poll runs again beside the harvests"
        );
    }

    #[tokio::test]
    async fn a_pipeline_that_leaves_the_plan_has_its_run_deleted_and_a_stale_queue_starts_nothing()
    {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T05:00:00Z");
        queue.lock().unwrap().plan(
            [
                (key("a"), planned(Clock::Once)),
                (key("b"), planned(Clock::Once)),
            ]
            .into_iter()
            .collect(),
            now,
        );
        step(&queue, &fake, now).await;
        step(&queue, &fake, now).await;
        assert_eq!(fake.held(), ["a"]);
        fake.log();
        queue.lock().unwrap().plan(
            [(key("b"), planned(Clock::Once))].into_iter().collect(),
            now,
        );
        step(&queue, &fake, now).await;
        assert_eq!(fake.log(), ["delete a", "start b"]);
        assert_eq!(queue.lock().unwrap().outcome(&key("a")), None);

        fake.end("b", RunState::Finished);
        let later = now + Span::minutes(11);
        queue.lock().unwrap().plan(
            [
                (key("b"), planned(Clock::Once)),
                (key("c"), planned(Clock::Once)),
            ]
            .into_iter()
            .collect(),
            now,
        );
        tick(&queue, &fake, later).await;
        assert_eq!(
            fake.log(),
            ["delete c", "delete b"],
            "the run ends; nothing starts without a fresh plan"
        );
    }

    #[tokio::test]
    async fn a_run_now_joins_the_queue_once_and_is_named_by_its_due_time() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-16T14:00:00Z");
        let tick_stream = serde_json::json!({"input": {"generate": {"count": 1, "mapping": format!("root = \"\"\n{}", crate::pipeline_log::TICK)}}});
        let wanted = [(
            key("a"),
            Planned {
                stream: tick_stream,
                clock: daily_at_5(),
                replace: false,
            },
        )];
        queue
            .lock()
            .unwrap()
            .plan(wanted.into_iter().collect(), now);
        assert_eq!(
            queue.lock().unwrap().run_now(&key("zz"), now),
            Err(RunNow::NotQueued)
        );
        let run = queue
            .lock()
            .unwrap()
            .run_now(&key("a"), now)
            .expect("queued");
        assert_eq!(run, "2026-10-16T14:00:00Z");
        let later = now + Span::seconds(30);
        assert_eq!(
            queue.lock().unwrap().run_now(&key("a"), later),
            Ok(run.clone()),
            "one entry"
        );

        step(&queue, &fake, later).await;
        step(&queue, &fake, later).await;
        assert_eq!(
            fake.held(),
            ["a"],
            "the run went to the runner tonight's clock aside"
        );
        assert_eq!(
            queue.lock().unwrap().run_now(&key("a"), later),
            Err(RunNow::Running(run.clone()))
        );
        let started = fake.sent.lock().unwrap()["a"].clone();
        let mapping = started["input"]["generate"]["mapping"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            mapping.ends_with("meta jc_run = \"2026-10-16T14:00:00Z\""),
            "{mapping}"
        );
        assert!(!mapping.contains("now()"), "{mapping}");
    }

    #[tokio::test]
    async fn a_scheduled_pipeline_that_never_ran_runs_once_on_create_and_then_by_its_clock() {
        let queue = Mutex::new(RunQueue::new(2, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T17:00:00Z");
        let wanted = || {
            [
                (key("fresh"), planned(daily_at_5())),
                (key("old"), planned(daily_at_5())),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        };
        {
            let mut q = queue.lock().unwrap();
            q.ran([(key("old"), now)].into_iter().collect());
            q.plan(wanted(), now);
        }
        step(&queue, &fake, now).await;
        assert_eq!(
            fake.held(),
            ["fresh"],
            "the one that ran before waits for 05:00"
        );
        fake.end("fresh", RunState::Finished);
        // Reconciler passes go on while the log has not caught up: still one run, not one each.
        for minutes in 1..4 {
            let later = now + Span::minutes(minutes);
            queue.lock().unwrap().plan(wanted(), later);
            step(&queue, &fake, later).await;
        }
        let starts: Vec<String> = fake
            .log()
            .into_iter()
            .filter(|l| l.starts_with("start"))
            .collect();
        assert_eq!(starts, ["start fresh"], "one run on create");
        assert!(fake.held().is_empty(), "{:?}", fake.held());
        let five = at("2026-10-18T05:00:00Z");
        queue.lock().unwrap().plan(wanted(), five);
        step(&queue, &fake, five).await;
        assert_eq!(
            fake.held(),
            ["fresh", "old"],
            "both by their clock from then on"
        );
    }

    #[tokio::test]
    async fn without_the_run_log_a_new_scheduled_pipeline_waits_for_its_clock() {
        let queue = Mutex::new(RunQueue::new(1, 1));
        let fake = Fake::default();
        let now = at("2026-10-17T17:00:00Z");
        queue.lock().unwrap().plan(
            [(key("a"), planned(daily_at_5()))].into_iter().collect(),
            now,
        );
        step(&queue, &fake, now).await;
        assert!(
            fake.held().is_empty(),
            "a restart without the log runs nothing early"
        );
    }

    #[test]
    fn one_run_fires_every_clock_once_and_a_feed_poll_becomes_one_tick() {
        let merged = serde_json::json!({"input": {"broker": {"inputs": [
            {"generate": {"interval": "0 5 * * *", "mapping": "root = \"\""}, "processors": [{"http": {}}]},
            {"file": {"paths": ["/x"]}},
        ]}}, "output": {"drop": {}}});
        let run = one_run(&merged);
        assert_eq!(run["input"]["broker"]["inputs"][0]["generate"]["count"], 1);
        assert_eq!(
            run["input"]["broker"]["inputs"][0]["generate"]["interval"],
            ""
        );
        assert_eq!(
            run["input"]["broker"]["inputs"][1],
            merged["input"]["broker"]["inputs"][1]
        );
        assert_eq!(run["output"], merged["output"]);

        let feed = serde_json::json!({"input": {
            "http_client": {"url": "https://feed/vp", "verb": "GET", "metadata": {"gtfs_feed": "vehicle_positions"}},
            "processors": [{"protobuf": {"operator": "to_json"}}],
        }});
        let run = one_run(&feed);
        let input = &run["input"];
        assert!(input.get("http_client").is_none());
        assert_eq!(input["generate"]["count"], 1);
        let mapping = input["generate"]["mapping"].as_str().unwrap();
        assert!(
            mapping.contains("meta gtfs_feed = \"vehicle_positions\"\n"),
            "{mapping}"
        );
        assert_eq!(
            input["processors"][0]["try"][0]["http"],
            serde_json::json!({"url": "https://feed/vp", "verb": "GET"})
        );
        assert_eq!(
            input["processors"][2]["protobuf"]["operator"], "to_json",
            "the feed's own decoding follows the read"
        );
    }

    /// T-5778: on a full batch lane a restarted Portal forgot the runs still queued, and each
    /// waited its whole period again: slovensko's pipelines, queued last by name, ran on neither
    /// cluster for a day. Four pipelines due every 10 minutes keep one lane busy with 15-minute
    /// runs and the Portal restarts every 30 minutes; the daily one still runs that day.
    #[tokio::test]
    async fn a_full_lane_runs_every_pipeline_although_the_portal_restarts_while_it_waits() {
        let every_10 = Clock::Every(Duration::from_secs(600), Duration::ZERO);
        let wanted = || -> BTreeMap<Key, Planned> {
            ["a1", "a2", "a3", "a4"]
                .map(|name| (key(name), planned(every_10.clone())))
                .into_iter()
                .chain([(key("z"), planned(daily_at_5()))])
                .collect()
        };
        let fake = Fake::default();
        let start = at("2026-10-26T04:00:00Z");
        // The run log: each ran the day before.
        let mut log: HashMap<Key, DateTime<Utc>> = wanted()
            .into_keys()
            .map(|key| (key, start - Span::hours(20)))
            .collect();
        let mut queue = Mutex::new(RunQueue::new(1, 1));
        let mut running: HashMap<Key, DateTime<Utc>> = HashMap::new();
        let mut daily = Vec::new();
        for minute in 0..24 * 60 {
            let now = start + Span::minutes(minute);
            if minute % 30 == 0 {
                queue = Mutex::new(RunQueue::new(1, 1));
            }
            {
                let mut q = queue.lock().unwrap();
                q.ran(log.clone());
                q.plan(wanted(), now);
            }
            running.retain(|key, since| {
                let done = now - *since >= Span::minutes(15);
                if done {
                    fake.end(&key.1, RunState::Finished);
                }
                !done
            });
            tick(&queue, &fake, now).await;
            for line in fake.log() {
                if let Some(name) = line.strip_prefix("start ") {
                    running.insert(key(name), now);
                    log.insert(key(name), now);
                    if name == "z" {
                        daily.push(now);
                    }
                }
            }
        }
        assert_eq!(daily.len(), 1, "the daily pipeline ran {daily:?}");
    }
}
