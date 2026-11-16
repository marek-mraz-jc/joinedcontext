//! The consumer: group membership, fetching, acknowledgement tracking and offset commits.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ekiben_bloblang::Value;
use ekiben_core::{AckResponder, Batch, ComponentConfig, CoreError, Input, Part, StreamContext};

use crate::client::{self, failed, Broker, Cluster, ConnSettings};
use crate::group::{self, Assignment};
use crate::records::{self, Aborted};
use crate::wire::{error_name, Dec, Enc, WireError};
use crate::{settings, Settings, Topics};

const SESSION_MS: i32 = 30_000;
const REBALANCE_MS: i32 = 60_000;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// JoinGroup waits for every member to rejoin, up to the rebalance timeout.
const JOIN_TIMEOUT: Duration = Duration::from_secs(REBALANCE_MS as u64 / 1000 + 10);
/// The longest one fetch waits, so `read_batch` returns to the runner often enough to see a
/// shutdown.
const FETCH_WAIT_CAP: Duration = Duration::from_secs(1);

type Tp = (String, i32);

fn wire(e: WireError) -> CoreError {
    failed(e)
}

static BATCH_IDS: AtomicU64 = AtomicU64::new(1);

/// One partition's read position and the batches the pipeline has not acknowledged.
#[derive(Debug, Default)]
struct PartState {
    /// The next offset to fetch.
    fetch: i64,
    /// The next offset to hand to the pipeline; a queued batch starting elsewhere is stale.
    deliver: i64,
    /// Batches handed out and not yet acknowledged: start -> (id, end, messages).
    pending: BTreeMap<i64, (u64, i64, usize)>,
    committed: i64,
    high_watermark: i64,
}

impl PartState {
    fn at(offset: i64) -> Self {
        Self {
            fetch: offset,
            deliver: offset,
            committed: offset,
            ..Self::default()
        }
    }

    fn unacked(&self) -> usize {
        self.pending.values().map(|p| p.2).sum()
    }

    /// The offset a commit may store: below the oldest unacknowledged batch.
    fn commit_point(&self) -> i64 {
        self.pending.keys().next().copied().unwrap_or(self.deliver)
    }
}

#[derive(Default)]
struct Shared {
    parts: HashMap<Tp, PartState>,
}

impl Shared {
    fn ack(&mut self, tp: &Tp, start: i64, id: u64, ok: bool) {
        let Some(p) = self.parts.get_mut(tp) else {
            return; // the partition was revoked; its next owner reads from the commit
        };
        if p.pending.get(&start).map(|e| e.0) != Some(id) {
            return; // replaced by a replay
        }
        if ok {
            p.pending.remove(&start);
        } else {
            // read the batch and everything after it again
            p.pending.retain(|s, _| *s < start);
            p.fetch = start;
            p.deliver = start;
        }
    }
}

struct Ready {
    tp: Tp,
    start: i64,
    end: i64,
    parts: Vec<Part>,
}

struct Membership {
    group: String,
    member: String,
    generation: i32,
    coordinator: i32,
    heartbeat: tokio::task::JoinHandle<()>,
    rejoin: Arc<AtomicBool>,
    /// The subscribed topics and their partition counts at the join: a change means rejoin.
    seen: BTreeMap<String, usize>,
}

pub struct KafkaInput {
    s: Settings,
    label: String,
    org: String,
    cluster: Cluster,
    shared: Arc<Mutex<Shared>>,
    ready: VecDeque<Ready>,
    member: Option<Membership>,
    /// A group (or, without one, every partition) was assigned; an empty assignment is one too.
    assigned: bool,
    /// The member id the coordinator gave this input, kept across rejoins.
    member_id: String,
    leaders: Vec<i32>,
    next_leader: usize,
    meta_at: Option<Instant>,
    committed_at: Instant,
    stale_meta: bool,
    backoff: Duration,
}

impl KafkaInput {
    pub fn new(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let s = settings(conf)?;
        let tls = match &s.tls {
            None => None,
            Some(cas) => Some(client::tls_config(cas)?),
        };
        let conn = ConnSettings {
            client_id: s.client_id.clone(),
            tls,
            sasl: s.sasl.as_ref().map(|x| client::Sasl {
                mechanism: x.mechanism,
                username: x.username.clone(),
                password: x.password.clone(),
            }),
            max_response: s.broker_read_max_bytes,
            request_timeout: REQUEST_TIMEOUT,
            org: ctx.org.clone(),
        };
        let seeds = s.seeds.clone();
        Ok(Self {
            s,
            label: conf.label.clone(),
            org: ctx.org.org_id.as_str().to_string(),
            cluster: Cluster::new(conn, seeds),
            shared: Arc::default(),
            ready: VecDeque::new(),
            member: None,
            assigned: false,
            member_id: String::new(),
            leaders: Vec::new(),
            next_leader: 0,
            meta_at: None,
            committed_at: Instant::now(),
            stale_meta: false,
            backoff: Duration::from_millis(100),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        // a poisoned lock only means an ack callback panicked; the offsets are still usable
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The topics this input reads, by name or by matching the cluster's topics.
    fn subscribed(&self) -> Vec<String> {
        let mut out: Vec<String> = match &self.s.topics {
            Topics::Names(n) => n.clone(),
            Topics::Patterns(p) => self
                .cluster
                .meta
                .topics
                .keys()
                .filter(|t| !t.starts_with("__") && p.iter().any(|r| r.is_match(t)))
                .cloned()
                .collect(),
        };
        out.sort();
        out
    }

    async fn refresh(&mut self) -> Result<(), CoreError> {
        let names = match &self.s.topics {
            Topics::Names(n) => Some(n.clone()),
            Topics::Patterns(_) => None,
        };
        self.cluster.refresh(names.as_deref()).await?;
        for (t, why) in &self.cluster.meta.errors {
            if why == &"TOPIC_AUTHORIZATION_FAILED" {
                return Err(CoreError::config(format!(
                    "kafka_franz input: topic {t}: TOPIC_AUTHORIZATION_FAILED; the SASL user may not read it"
                )));
            }
            tracing::warn!("kafka_franz input: topic {t}: {why}");
        }
        self.meta_at = Some(Instant::now());
        self.stale_meta = false;
        Ok(())
    }

    fn partition_counts(&self, topics: &[String]) -> BTreeMap<String, usize> {
        topics
            .iter()
            .filter_map(|t| {
                self.cluster
                    .meta
                    .topics
                    .get(t)
                    .map(|p| (t.clone(), p.len()))
            })
            .collect()
    }

    /// ListOffsets v1: the earliest (-2) or latest (-1) offset of each partition.
    async fn reset_offsets(&mut self, tps: &[Tp]) -> Result<HashMap<Tp, i64>, CoreError> {
        let time = if self.s.oldest { -2 } else { -1 };
        let mut by_leader: BTreeMap<i32, Vec<Tp>> = BTreeMap::new();
        for tp in tps {
            by_leader
                .entry(self.leader(tp)?)
                .or_default()
                .push(tp.clone());
        }
        let mut out = HashMap::new();
        for (node, tps) in by_leader {
            let mut topics: BTreeMap<&str, Vec<i32>> = BTreeMap::new();
            for (t, p) in &tps {
                topics.entry(t).or_default().push(*p);
            }
            let items: Vec<(&str, Vec<i32>)> = topics.into_iter().collect();
            let mut e = Enc::default();
            e.i32(-1).array(&items, |e, (t, ps)| {
                e.str(t).array(ps, |e, p| {
                    e.i32(*p).i64(time);
                });
            });
            let mut d = self
                .cluster
                .call(node, client::LIST_OFFSETS, 1, &e.0, REQUEST_TIMEOUT)
                .await?;
            let answer = d
                .array(|d| {
                    let t = d.str()?;
                    let ps = d.array(|d| Ok((d.i32()?, d.i16()?, d.i64()?, d.i64()?)))?;
                    Ok((t, ps))
                })
                .map_err(wire)?;
            for (t, ps) in answer {
                for (p, code, _ts, off) in ps {
                    if code != 0 {
                        self.stale_meta = true;
                        return Err(failed(format!(
                            "listing offsets of {t}/{p}: {}",
                            error_name(code)
                        )));
                    }
                    out.insert((t.clone(), p), off);
                }
            }
        }
        Ok(out)
    }

    fn leader(&self, (t, p): &Tp) -> Result<i32, CoreError> {
        self.cluster
            .meta
            .topics
            .get(t)
            .and_then(|ps| ps.iter().find(|(i, _)| i == p))
            .map(|(_, l)| *l)
            .ok_or_else(|| failed(format!("{t}/{p} has no leader in the metadata")))
    }

    /// Joins the group (or, without one, takes every partition) and sets each partition's
    /// position from its committed offset or the reset rule.
    async fn assign(&mut self) -> Result<(), CoreError> {
        // an assignment that failed half way left a membership whose heartbeats still run
        if let Some(m) = self.member.take() {
            m.heartbeat.abort();
        }
        self.refresh().await?;
        let topics = self.subscribed();
        if topics.is_empty() {
            return Err(failed("no topic matches topics yet"));
        }
        let (assignment, committed) = match self.s.consumer_group.clone() {
            None => {
                let a: Assignment = topics
                    .iter()
                    .filter_map(|t| {
                        let ps = self.cluster.meta.topics.get(t)?;
                        Some((t.clone(), ps.iter().map(|(p, _)| *p).collect()))
                    })
                    .collect();
                (a, HashMap::new())
            }
            Some(g) => self.join(&group::group_id(&self.org, &g), &topics).await?,
        };
        let tps: Vec<Tp> = assignment
            .iter()
            .flat_map(|(t, ps)| ps.iter().map(move |p| (t.clone(), *p)))
            .collect();
        let missing: Vec<Tp> = tps
            .iter()
            .filter(|tp| !committed.contains_key(*tp))
            .cloned()
            .collect();
        let reset = if missing.is_empty() {
            HashMap::new()
        } else {
            self.reset_offsets(&missing).await?
        };
        let mut shared = self.lock();
        shared.parts.clear();
        for tp in tps {
            let at = committed
                .get(&tp)
                .or_else(|| reset.get(&tp))
                .copied()
                .unwrap_or(0);
            shared.parts.insert(tp, PartState::at(at));
        }
        drop(shared);
        self.ready.clear();
        self.leaders_of_assignment();
        self.assigned = true;
        tracing::info!(
            "kafka_franz input: reading {} partition(s) of {}",
            self.lock().parts.len(),
            topics.join(", ")
        );
        Ok(())
    }

    fn leaders_of_assignment(&mut self) {
        let mut leaders: Vec<i32> = self
            .lock()
            .parts
            .keys()
            .filter_map(|tp| self.leader(tp).ok())
            .collect();
        leaders.sort_unstable();
        leaders.dedup();
        self.leaders = leaders;
    }

    async fn coordinator(&mut self, group_id: &str) -> Result<i32, CoreError> {
        let node = *self
            .cluster
            .meta
            .brokers
            .keys()
            .min()
            .ok_or_else(|| failed("the cluster advertises no broker"))?;
        let d = self
            .cluster
            .call(
                node,
                client::FIND_COORDINATOR,
                1,
                &group::find_coordinator(group_id),
                REQUEST_TIMEOUT,
            )
            .await?;
        let (code, id, host, port) = group::decode_coordinator(d).map_err(wire)?;
        match code {
            0 => {}
            30 => {
                return Err(CoreError::config(format!(
                    "kafka_franz input: GROUP_AUTHORIZATION_FAILED for group {group_id}; grant the SASL user the group {group_id} (organization prefix included)"
                )))
            }
            c => return Err(failed(format!("finding the coordinator of {group_id}: {}", error_name(c)))),
        }
        let port =
            u16::try_from(port).map_err(|_| failed("the coordinator's port is out of range"))?;
        self.cluster.meta.brokers.insert(id, Broker { host, port });
        Ok(id)
    }

    async fn join(
        &mut self,
        group_id: &str,
        topics: &[String],
    ) -> Result<(Assignment, HashMap<Tp, i64>), CoreError> {
        let coordinator = self.coordinator(group_id).await?;
        let joined = loop {
            let body = group::join(group_id, SESSION_MS, REBALANCE_MS, &self.member_id, topics);
            let d = self
                .cluster
                .call(coordinator, client::JOIN_GROUP, 2, &body, JOIN_TIMEOUT)
                .await?;
            let j = group::decode_join(d).map_err(wire)?;
            match j.code {
                0 => break j,
                25 => self.member_id.clear(),
                27 => {}
                30 => {
                    return Err(CoreError::config(format!(
                        "kafka_franz input: GROUP_AUTHORIZATION_FAILED for group {group_id}; grant the SASL user that group"
                    )))
                }
                23 => {
                    return Err(CoreError::config(format!(
                        "kafka_franz input: group {group_id} is used by consumers with another assignor (INCONSISTENT_GROUP_PROTOCOL); give this input its own consumer_group"
                    )))
                }
                c => return Err(failed(format!("joining {group_id}: {}", error_name(c)))),
            }
        };
        if joined.protocol != "range" {
            return Err(failed(format!(
                "group {group_id} chose the assignor {}; this input offers only range",
                joined.protocol
            )));
        }
        self.member_id = joined.member.clone();
        let assignments = if joined.leader == joined.member {
            let mut members = Vec::new();
            for (m, meta) in joined.members {
                members.push((m, group::decode_subscription(meta).map_err(wire)?));
            }
            let mut all: Vec<String> = members.iter().flat_map(|(_, t)| t.clone()).collect();
            all.sort();
            all.dedup();
            if all
                .iter()
                .any(|t| !self.cluster.meta.topics.contains_key(t))
            {
                self.cluster.refresh(Some(&all)).await?;
            }
            let counts = self.partition_counts(&all);
            group::range_assign(&members, &counts)
                .into_iter()
                .map(|(m, a)| (m, group::assignment(&a)))
                .collect()
        } else {
            Vec::new()
        };
        let body = group::sync(group_id, joined.generation, &self.member_id, &assignments);
        let d = self
            .cluster
            .call(coordinator, client::SYNC_GROUP, 1, &body, JOIN_TIMEOUT)
            .await?;
        let (code, assignment) = group::decode_sync(d).map_err(wire)?;
        if code != 0 {
            if code == 25 {
                self.member_id.clear();
            }
            return Err(failed(format!("syncing {group_id}: {}", error_name(code))));
        }
        let assignment = group::decode_assignment(assignment).map_err(wire)?;
        let d = self
            .cluster
            .call(
                coordinator,
                client::OFFSET_FETCH,
                1,
                &group::offset_fetch(group_id, &assignment),
                REQUEST_TIMEOUT,
            )
            .await?;
        let mut committed = HashMap::new();
        for (t, p, off, code) in group::decode_offset_fetch(d).map_err(wire)? {
            if code != 0 {
                return Err(failed(format!(
                    "reading the offset of {t}/{p}: {}",
                    error_name(code)
                )));
            }
            if off >= 0 {
                committed.insert((t, p), off);
            }
        }
        let rejoin = Arc::new(AtomicBool::new(false));
        let broker = self
            .cluster
            .meta
            .brokers
            .get(&coordinator)
            .cloned()
            .ok_or_else(|| failed("the coordinator left the metadata"))?;
        let conn = self.cluster.dial(&broker).await?;
        let heartbeat = tokio::spawn(heartbeats(
            conn,
            group::heartbeat(group_id, joined.generation, &self.member_id),
            ConnSettings {
                client_id: self.cluster.s.client_id.clone(),
                tls: None,
                sasl: None,
                max_response: 1 << 16,
                request_timeout: REQUEST_TIMEOUT,
                org: self.cluster.s.org.clone(),
            },
            rejoin.clone(),
        ));
        self.member = Some(Membership {
            group: group_id.to_string(),
            member: self.member_id.clone(),
            generation: joined.generation,
            coordinator,
            heartbeat,
            rejoin,
            seen: self.partition_counts(topics),
        });
        Ok((assignment, committed))
    }

    /// Commits each partition's offset below its oldest unacknowledged batch.
    async fn commit(&mut self) -> Result<(), CoreError> {
        self.committed_at = Instant::now();
        let Some(m) = &self.member else {
            return Ok(());
        };
        let offsets: Vec<(String, i32, i64)> = self
            .lock()
            .parts
            .iter()
            .filter(|(_, p)| p.commit_point() != p.committed)
            .map(|((t, p), s)| (t.clone(), *p, s.commit_point()))
            .collect();
        if offsets.is_empty() {
            return Ok(());
        }
        let body = group::offset_commit(&m.group, m.generation, &m.member, &offsets);
        let node = m.coordinator;
        let d = self
            .cluster
            .call(node, client::OFFSET_COMMIT, 2, &body, REQUEST_TIMEOUT)
            .await?;
        match group::decode_offset_commit(d).map_err(wire)? {
            0 => {
                let mut shared = self.lock();
                for (t, p, o) in offsets {
                    if let Some(s) = shared.parts.get_mut(&(t, p)) {
                        s.committed = o;
                    }
                }
                Ok(())
            }
            c @ (22 | 25 | 27) => {
                if let Some(m) = &self.member {
                    m.rejoin.store(true, Ordering::Relaxed);
                }
                Err(failed(format!(
                    "committing offsets: {}; rejoining the group",
                    error_name(c)
                )))
            }
            c => Err(failed(format!("committing offsets: {}", error_name(c)))),
        }
    }

    async fn leave(&mut self) {
        if let Err(e) = self.commit().await {
            tracing::warn!("kafka_franz input: {e}");
        }
        if let Some(m) = self.member.take() {
            m.heartbeat.abort();
            let body = group::leave(&m.group, &m.member);
            let _ = self
                .cluster
                .call(
                    m.coordinator,
                    client::LEAVE_GROUP,
                    1,
                    &body,
                    REQUEST_TIMEOUT,
                )
                .await;
        }
    }

    /// Drops the assignment; the next read joins again.
    async fn drop_assignment(&mut self) {
        if let Err(e) = self.commit().await {
            tracing::warn!("kafka_franz input: {e}");
        }
        if let Some(m) = self.member.take() {
            m.heartbeat.abort();
        }
        self.lock().parts.clear();
        self.ready.clear();
        self.leaders.clear();
        self.assigned = false;
    }

    fn needs_rejoin(&self) -> bool {
        let Some(m) = &self.member else {
            return false;
        };
        if m.rejoin.load(Ordering::Relaxed) {
            return true;
        }
        let topics = self.subscribed();
        self.partition_counts(&topics) != m.seen
    }

    /// Fetch v4 from the next leader of the assignment; queues one batch per partition.
    async fn fetch(&mut self) -> Result<(), CoreError> {
        if self.leaders.is_empty() {
            // nothing assigned to this member (more members than partitions)
            tokio::time::sleep(Duration::from_millis(200)).await;
            return Ok(());
        }
        let node = self.leaders[self.next_leader % self.leaders.len()];
        self.next_leader = self.next_leader.wrapping_add(1);
        let queued: Vec<&Tp> = self.ready.iter().map(|r| &r.tp).collect();
        let want: Vec<(Tp, i64)> = self
            .lock()
            .parts
            .iter()
            .filter(|(tp, p)| {
                self.leader(tp).ok() == Some(node)
                    && p.unacked() < self.s.checkpoint_limit
                    && !queued.contains(tp)
            })
            .map(|(tp, p)| (tp.clone(), p.fetch))
            .collect();
        if want.is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Ok(());
        }
        let mut topics: BTreeMap<&str, Vec<(i32, i64)>> = BTreeMap::new();
        for ((t, p), off) in &want {
            topics.entry(t).or_default().push((*p, *off));
        }
        let items: Vec<(&str, Vec<(i32, i64)>)> = topics.into_iter().collect();
        let wait = self.s.fetch_max_wait.min(FETCH_WAIT_CAP);
        let mut e = Enc::default();
        e.i32(-1)
            .i32(wait.as_millis() as i32)
            .i32(1)
            .i32(self.s.fetch_max_bytes)
            .i8(i8::from(self.s.read_committed))
            .array(&items, |e, (t, ps)| {
                e.str(t).array(ps, |e, (p, off)| {
                    e.i32(*p).i64(*off).i32(self.s.fetch_max_partition_bytes);
                });
            });
        let d = self
            .cluster
            .call(node, client::FETCH, 4, &e.0, wait + REQUEST_TIMEOUT)
            .await;
        let d = match d {
            Ok(d) => d,
            Err(e) => {
                self.stale_meta = true;
                return Err(e);
            }
        };
        let positions: HashMap<Tp, i64> = want.into_iter().collect();
        let answer = decode_fetch(d).map_err(wire)?;
        let mut reset = Vec::new();
        for f in answer {
            let tp = (f.topic.clone(), f.partition);
            let Some(&from) = positions.get(&tp) else {
                continue;
            };
            match f.code {
                0 => {}
                1 => {
                    tracing::warn!("kafka_franz input: {}/{}: offset {from} is out of range; resetting", tp.0, tp.1);
                    reset.push(tp);
                    continue;
                }
                3 | 5 | 6 | 74 | 75 => {
                    self.stale_meta = true;
                    continue;
                }
                29 => {
                    return Err(CoreError::config(format!(
                        "kafka_franz input: topic {}: TOPIC_AUTHORIZATION_FAILED; the SASL user may not read it",
                        tp.0
                    )))
                }
                c => {
                    tracing::warn!("kafka_franz input: fetching {}/{}: {}", tp.0, tp.1, error_name(c));
                    continue;
                }
            }
            let aborted = self.s.read_committed.then_some(&f.aborted[..]);
            let decoded = records::decode(f.records, from, aborted).map_err(wire)?;
            let Some(end) = decoded.next_offset else {
                continue;
            };
            {
                let mut shared = self.lock();
                let Some(p) = shared.parts.get_mut(&tp) else {
                    continue;
                };
                if p.fetch != from {
                    continue; // a nack rewound the partition while this fetch ran
                }
                p.fetch = end;
                p.high_watermark = f.high_watermark;
            }
            let hw = f.high_watermark;
            let parts = decoded
                .records
                .into_iter()
                .map(|r| self.part(&tp, r, hw))
                .collect();
            self.ready.push_back(Ready {
                tp,
                start: from,
                end,
                parts,
            });
        }
        if !reset.is_empty() {
            let offsets = self.reset_offsets(&reset).await?;
            let mut shared = self.lock();
            for (tp, off) in offsets {
                if let Some(p) = shared.parts.get_mut(&tp) {
                    p.fetch = off;
                    p.deliver = off;
                    p.pending.clear();
                }
            }
        }
        Ok(())
    }

    fn part(&self, (topic, partition): &Tp, r: records::Record, hw: i64) -> Part {
        let tombstone = r.value.is_none();
        let mut part = Part::new(r.value.unwrap_or_default());
        let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, v) in r.headers {
            let v = v
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            headers.entry(k).or_default().push(v);
        }
        for (k, vs) in headers {
            if self.s.multi_header {
                let list: Vec<Value> = vs.into_iter().map(Value::String).collect();
                part.meta_set(k, Value::Array(list.into()));
            } else if let Some(v) = vs.into_iter().last() {
                part.meta_set_str(k, v);
            }
        }
        if let Some(k) = r.key {
            part.meta_set_str("kafka_key", String::from_utf8_lossy(&k).into_owned());
        }
        part.meta_set_str("kafka_topic", topic.clone());
        part.meta_set_str("kafka_partition", partition.to_string());
        part.meta_set_str("kafka_offset", r.offset.to_string());
        part.meta_set_str("kafka_lag", (hw - r.offset - 1).max(0).to_string());
        part.meta_set_str("kafka_timestamp_ms", r.timestamp_ms.to_string());
        part.meta_set_str(
            "kafka_timestamp_unix",
            r.timestamp_ms.div_euclid(1000).to_string(),
        );
        part.meta_set_str("kafka_tombstone_message", tombstone.to_string());
        part
    }

    /// The next queued batch whose start is still the partition's delivery point.
    fn take_ready(&mut self) -> Option<(Batch, AckResponder)> {
        while let Some(r) = self.ready.pop_front() {
            let id = BATCH_IDS.fetch_add(1, Ordering::Relaxed);
            {
                let mut shared = self.lock();
                let Some(p) = shared.parts.get_mut(&r.tp) else {
                    continue;
                };
                if p.deliver != r.start {
                    continue; // stale: a nack rewound the partition
                }
                p.deliver = r.end;
                if r.parts.is_empty() {
                    continue; // only control or aborted batches: the position moves on
                }
                p.pending.insert(r.start, (id, r.end, r.parts.len()));
            }
            let shared = self.shared.clone();
            let replay = self.s.auto_replay_nacks;
            let (tp, start) = (r.tp, r.start);
            let ack = AckResponder::from_fn(move |err| {
                let ok = err.is_none() || !replay;
                shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .ack(&tp, start, id, ok);
            });
            return Some((r.parts, ack));
        }
        None
    }

    async fn step(&mut self) -> Result<Option<(Batch, AckResponder)>, CoreError> {
        if let Some(b) = self.take_ready() {
            return Ok(Some(b));
        }
        let meta_old = self
            .meta_at
            .is_none_or(|t| t.elapsed() >= self.s.metadata_max_age);
        if self.stale_meta || meta_old {
            self.refresh().await?;
            self.leaders_of_assignment();
        }
        if self.member.is_some() && self.needs_rejoin() {
            tracing::info!("kafka_franz input: the group rebalances; rejoining");
            self.drop_assignment().await;
        }
        if !self.assigned || (self.s.consumer_group.is_none() && self.no_group_grew()) {
            self.assign().await?;
        }
        if self.committed_at.elapsed() >= self.s.commit_period {
            if let Err(e) = self.commit().await {
                tracing::warn!("kafka_franz input: {e}");
            }
        }
        self.fetch().await?;
        Ok(self.take_ready())
    }

    /// Without a group: a topic gained partitions, or a pattern matches a new topic.
    fn no_group_grew(&self) -> bool {
        let shared = self.lock();
        self.subscribed().iter().any(|t| {
            self.cluster.meta.topics.get(t).is_some_and(|ps| {
                ps.iter()
                    .any(|(p, _)| !shared.parts.contains_key(&(t.clone(), *p)))
            })
        })
    }
}

async fn heartbeats(
    mut conn: client::Conn,
    body: Vec<u8>,
    s: ConnSettings,
    rejoin: Arc<AtomicBool>,
) {
    loop {
        tokio::time::sleep(HEARTBEAT_EVERY).await;
        let code = match conn
            .call(client::HEARTBEAT, 1, &body, &s, REQUEST_TIMEOUT)
            .await
        {
            Ok(d) => group::decode_code(d).unwrap_or(-1),
            Err(e) => {
                tracing::warn!("kafka_franz input: heartbeat: {e}");
                -1
            }
        };
        if code != 0 {
            rejoin.store(true, Ordering::Relaxed);
            return;
        }
    }
}

struct FetchedPartition {
    topic: String,
    partition: i32,
    code: i16,
    high_watermark: i64,
    aborted: Vec<Aborted>,
    records: bytes::Bytes,
}

fn decode_fetch(mut d: Dec) -> Result<Vec<FetchedPartition>, WireError> {
    d.i32()?; // throttle
    let mut out = Vec::new();
    for (topic, parts) in d.array(|d| {
        let t = d.str()?;
        let parts = d.array(|d| {
            let partition = d.i32()?;
            let code = d.i16()?;
            let high_watermark = d.i64()?;
            d.i64()?; // last stable offset
            let aborted = d.array(|d| {
                Ok(Aborted {
                    producer_id: d.i64()?,
                    first_offset: d.i64()?,
                })
            })?;
            Ok((partition, code, high_watermark, aborted, d.bytes()?))
        })?;
        Ok((t, parts))
    })? {
        for (partition, code, high_watermark, aborted, records) in parts {
            out.push(FetchedPartition {
                topic: topic.clone(),
                partition,
                code,
                high_watermark,
                aborted,
                records,
            });
        }
    }
    Ok(out)
}

#[async_trait]
impl Input for KafkaInput {
    fn input_type(&self) -> &'static str {
        "kafka_franz"
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn is_connected(&self) -> bool {
        self.assigned
    }

    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        match self.step().await {
            Ok(Some(b)) => {
                self.backoff = Duration::from_millis(100);
                Ok(b)
            }
            Ok(None) => Err(CoreError::Timeout),
            Err(e @ (CoreError::AccessDenied { .. } | CoreError::Config(_))) => Err(e),
            Err(e) => {
                tracing::warn!("kafka_franz input: {e}; retrying in {:?}", self.backoff);
                self.stale_meta = true;
                self.cluster.drop_conns();
                tokio::time::sleep(self.backoff).await;
                self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
                Err(CoreError::Timeout)
            }
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.leave().await;
        self.cluster.drop_conns();
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn tp() -> Tp {
        ("t".into(), 0)
    }

    #[test]
    fn the_commit_point_waits_for_the_oldest_unacked_batch() {
        let mut s = Shared::default();
        s.parts.insert(tp(), PartState::at(0));
        let p = s.parts.get_mut(&tp()).expect("part");
        p.pending.insert(0, (1, 10, 10));
        p.pending.insert(10, (2, 20, 10));
        p.deliver = 20;
        s.ack(&tp(), 10, 2, true);
        assert_eq!(s.parts[&tp()].commit_point(), 0, "batch 0 is still out");
        s.ack(&tp(), 0, 1, true);
        assert_eq!(s.parts[&tp()].commit_point(), 20);
        assert_eq!(s.parts[&tp()].unacked(), 0);
    }

    #[test]
    fn a_nack_rewinds_and_stale_acks_are_ignored() {
        let mut s = Shared::default();
        s.parts.insert(tp(), PartState::at(0));
        let p = s.parts.get_mut(&tp()).expect("part");
        p.pending.insert(0, (1, 10, 10));
        p.pending.insert(10, (2, 20, 10));
        p.pending.insert(20, (3, 30, 10));
        p.fetch = 30;
        p.deliver = 30;
        s.ack(&tp(), 10, 2, false);
        let p = &s.parts[&tp()];
        assert_eq!((p.fetch, p.deliver, p.commit_point()), (10, 10, 0));
        assert_eq!(p.pending.len(), 1);
        // the ack of a batch the replay replaced changes nothing
        s.ack(&tp(), 20, 3, true);
        assert_eq!(s.parts[&tp()].pending.len(), 1);
        s.ack(&tp(), 0, 1, true);
        assert_eq!(s.parts[&tp()].commit_point(), 10);
        // an ack for a revoked partition is dropped
        s.ack(&("other".into(), 1), 0, 9, true);
    }
}
