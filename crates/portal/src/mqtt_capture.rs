//! The capture of `capture` MqttBrokers (ADR-N-048 §2.10, API/01 §47, T-4880).
//!
//! The reconciler's leader runs one MQTT subscriber per broker in `capture` mode, on the broker's
//! internal listener, under [`USERNAME`] and without a token: the gateway admits it from the
//! address of a Portal pod, on a capture broker only, with `subscribe` on the publish filters of
//! the broker's roles and nothing else. What arrives is kept in the Portal's database: at most
//! `messagesPerTopic` per topic, [`BROKER_MESSAGES`] and [`BROKER_BYTES`] per broker, each for
//! `window` from its arrival. A broker that leaves `capture` or is deleted loses its capture.
//! No payload reaches a log, an activity entry or a metric: the subscriber logs the broker and
//! the error only.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jc_core::kinds::mqtt_broker::{MqttCapture, MqttMode};
use jc_time::TimestampTz;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, SubscribeFilter};

use crate::reconciler::mqtt_brokers::Broker;

/// The username the gateway admits from the Portal's pods; its `CAPTURE_USERNAME` is the same.
pub const USERNAME: &str = "jc-portal-capture";
/// The most messages a broker's capture keeps (ADR-N-048 §2.10).
pub const BROKER_MESSAGES: usize = 1000;
/// The most payload bytes a broker's capture keeps.
pub const BROKER_BYTES: usize = 8 * 1024 * 1024;
/// The broker's internal listener.
const PORT: u16 = 1883;
/// How often the arrived messages are written: one transaction per broker per second, however
/// fast its devices publish.
const FLUSH: Duration = Duration::from_secs(1);
/// How long a subscriber waits before it dials a broker that refused or dropped it again.
const RETRY: Duration = Duration::from_secs(5);

/// A broker to capture, read from its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub project: String,
    pub name: String,
    /// `mqtt-{project}-{name}`: the broker's Service.
    pub release: String,
    /// The roles' publish filters, as written; a `${clientid}` level names the device.
    pub publish: Vec<String>,
    pub per_topic: usize,
    /// The window in seconds.
    pub window: i64,
    /// The broker's `limits.maxMessageBytes`: the largest packet the subscriber accepts.
    pub max_message_bytes: usize,
}

impl Target {
    fn key(&self) -> (String, String) {
        (self.project.clone(), self.name.clone())
    }
}

/// The brokers the platform accepts in `capture` mode.
pub fn targets(brokers: &[Broker]) -> Vec<Target> {
    brokers
        .iter()
        .filter_map(|broker| {
            let spec = broker.spec.as_ref().ok()?;
            if spec.mode != Some(MqttMode::Capture) {
                return None;
            }
            let capture = spec.capture.clone().unwrap_or_default();
            Some(Target {
                project: broker.project.clone(),
                name: broker.name.clone(),
                release: broker.release.clone(),
                publish: spec
                    .roles
                    .iter()
                    .flat_map(|role| role.publish.iter().cloned())
                    .collect(),
                per_topic: usize::try_from(capture.messages_per_topic).unwrap_or(1),
                window: window_of(&capture),
                max_message_bytes: usize::try_from(spec.limits.max_message_bytes)
                    .unwrap_or(BROKER_BYTES),
            })
        })
        .collect()
}

fn window_of(capture: &MqttCapture) -> i64 {
    capture
        .window_seconds()
        .and_then(|seconds| i64::try_from(seconds).ok())
        .unwrap_or(3600)
}

/// What the subscriber subscribes to: each publish filter with a level that holds a placeholder
/// widened to `+`, as the gateway grants it (context-gateway `mqtt::capture_topics`).
pub fn filters(publish: &[String]) -> Vec<String> {
    let mut filters: Vec<String> = publish
        .iter()
        .map(|filter| {
            filter
                .split('/')
                .map(|level| if level.contains("${") { "+" } else { level })
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    filters.sort();
    filters.dedup();
    filters
}

/// The device a topic names: the level a publish filter holds as exactly `${clientid}`, where
/// the topic matches that filter. MQTT 3.1.1 carries no publisher in a message, so a broker
/// whose filters name no client id captures messages without one.
pub fn client_of(publish: &[String], topic: &str) -> Option<String> {
    let levels: Vec<&str> = topic.split('/').collect();
    publish.iter().find_map(|filter| {
        let mut client = None;
        for (i, pattern) in filter.split('/').enumerate() {
            if pattern == "#" {
                return client;
            }
            let level = levels.get(i)?;
            match pattern {
                "${clientid}" if !level.is_empty() => client = Some((*level).to_owned()),
                "+" => {}
                _ if pattern == *level => {}
                _ => return None,
            }
        }
        (filter.split('/').count() == levels.len())
            .then_some(client)
            .flatten()
    })
}

/// One kept message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub topic: String,
    pub client_id: Option<String>,
    pub received_at: TimestampTz,
    pub expires_at: TimestampTz,
    pub payload: Vec<u8>,
}

/// Keeps the newest of a broker's `messages` (oldest first) within its caps: none past its
/// window, at most `per_topic` per topic, [`BROKER_MESSAGES`] and [`BROKER_BYTES`] in all.
/// Once the count or the bytes are reached, every older message goes too.
pub fn trim(messages: &mut VecDeque<Message>, per_topic: usize, now: TimestampTz) {
    let mut topics: HashMap<String, usize> = HashMap::new();
    let (mut count, mut bytes, mut full) = (0usize, 0usize, false);
    let mut kept = VecDeque::with_capacity(messages.len().min(BROKER_MESSAGES));
    while let Some(message) = messages.pop_back() {
        if full || message.expires_at <= now {
            continue;
        }
        let seen = topics.entry(message.topic.clone()).or_default();
        if *seen >= per_topic {
            continue;
        }
        if count >= BROKER_MESSAGES || bytes.saturating_add(message.payload.len()) > BROKER_BYTES {
            full = true;
            continue;
        }
        *seen += 1;
        count += 1;
        bytes = bytes.saturating_add(message.payload.len());
        kept.push_front(message);
    }
    *messages = kept;
}

type Key = (String, String);
/// A stored message: topic, client id, received and expires in milliseconds, payload.
type Row = (String, Option<String>, i64, i64, Vec<u8>);

/// A stored instant, in milliseconds since the epoch: this crate's sqlx reads no jiff type.
fn at(millis: i64) -> TimestampTz {
    jc_time::TimestampTz::from_millis(millis).unwrap_or_default()
}

enum Inner {
    /// Every statement runs in a `db::tenant_tx` of its project's key (T-7200 PK-1h).
    Postgres(sqlx::PgPool, Arc<crate::store::Mirror>),
    Memory(Mutex<BTreeMap<Key, VecDeque<Message>>>),
}

/// The kept messages: Postgres when the Portal has a database, else this process's memory.
pub struct CaptureStore {
    inner: Inner,
}

/// The caps of [`trim`] in one statement: the newest per topic, then the newest
/// [`BROKER_MESSAGES`] of those, then as many as fit in [`BROKER_BYTES`]; the rest and every
/// expired row of the broker go.
const TRIM: &str = "DELETE FROM mqtt_capture WHERE project = $1 AND broker = $2 AND id NOT IN ( \
    SELECT id FROM ( \
      SELECT id, row_number() OVER (ORDER BY id DESC) AS n, \
             sum(bytes) OVER (ORDER BY id DESC) AS total \
      FROM (SELECT id, bytes, row_number() OVER (PARTITION BY topic ORDER BY id DESC) AS t \
            FROM mqtt_capture WHERE project = $1 AND broker = $2 \
                 AND expires_at > to_timestamp($3 / 1000.0)) per_topic \
      WHERE t <= $4) kept \
    WHERE n <= $5 AND total <= $6)";

impl CaptureStore {
    pub fn new(db: Option<sqlx::PgPool>, mirror: Arc<crate::store::Mirror>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool, mirror),
                None => Inner::Memory(Mutex::default()),
            },
        }
    }

    fn memory(
        memory: &Mutex<BTreeMap<Key, VecDeque<Message>>>,
    ) -> std::sync::MutexGuard<'_, BTreeMap<Key, VecDeque<Message>>> {
        memory.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds `batch` (oldest first) to the broker's capture and trims it to its caps; a project
    /// with no tenancy key keeps no capture.
    pub async fn keep(
        &self,
        project: &str,
        broker: &str,
        per_topic: usize,
        batch: Vec<Message>,
        now: TimestampTz,
    ) -> Result<(), sqlx::Error> {
        let (pool, mirror) = match &self.inner {
            Inner::Memory(memory) => {
                let mut memory = Self::memory(memory);
                let kept = memory
                    .entry((project.to_owned(), broker.to_owned()))
                    .or_default();
                kept.extend(batch);
                trim(kept, per_topic, now);
                return Ok(());
            }
            Inner::Postgres(pool, mirror) => (pool, mirror),
        };
        let Some(key) = crate::project_ids::key_of(pool, mirror, project).await? else {
            return Ok(());
        };
        let mut tx = crate::db::tenant_tx(pool, key).await?;
        if !batch.is_empty() {
            let mut topics = Vec::with_capacity(batch.len());
            let mut clients = Vec::with_capacity(batch.len());
            let mut received = Vec::with_capacity(batch.len());
            let mut expires = Vec::with_capacity(batch.len());
            let mut payloads = Vec::with_capacity(batch.len());
            for message in batch {
                topics.push(message.topic);
                clients.push(message.client_id);
                received.push(message.received_at.as_millis());
                expires.push(message.expires_at.as_millis());
                payloads.push(message.payload);
            }
            sqlx::query(
                "INSERT INTO mqtt_capture (project, broker, topic, client_id, received_at, expires_at, bytes, payload) \
                 SELECT $1, $2, t, c, to_timestamp(r / 1000.0), to_timestamp(e / 1000.0), octet_length(p), p \
                 FROM UNNEST($3::text[], $4::text[], $5::bigint[], $6::bigint[], $7::bytea[]) \
                 WITH ORDINALITY AS m(t, c, r, e, p, i) ORDER BY i",
            )
            .bind(project)
            .bind(broker)
            .bind(topics)
            .bind(clients)
            .bind(received)
            .bind(expires)
            .bind(payloads)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(TRIM)
            .bind(project)
            .bind(broker)
            .bind(now.as_millis())
            .bind(i64::try_from(per_topic).unwrap_or(i64::MAX))
            .bind(BROKER_MESSAGES as i64)
            .bind(BROKER_BYTES as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// The broker's unexpired messages, newest first, only of `topic` when given: at most
    /// `limit`, and whether there were more.
    pub async fn read(
        &self,
        project: &str,
        broker: &str,
        topic: Option<&str>,
        limit: usize,
        now: TimestampTz,
    ) -> Result<(Vec<Message>, bool), sqlx::Error> {
        let mut found: Vec<Message> = match &self.inner {
            Inner::Memory(memory) => Self::memory(memory)
                .get(&(project.to_owned(), broker.to_owned()))
                .map(|kept| {
                    kept.iter()
                        .rev()
                        .filter(|m| m.expires_at > now && topic.is_none_or(|t| m.topic == t))
                        .take(limit.saturating_add(1))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
            Inner::Postgres(pool, mirror) => {
                let Some(key) = crate::project_ids::key_of(pool, mirror, project).await? else {
                    return Ok((Vec::new(), false));
                };
                let mut tx = crate::db::tenant_tx(pool, key).await?;
                let rows: Vec<Row> = sqlx::query_as(
                    "SELECT topic, client_id, (extract(epoch FROM received_at) * 1000)::bigint, \
                         (extract(epoch FROM expires_at) * 1000)::bigint, payload FROM mqtt_capture \
                         WHERE project = $1 AND broker = $2 AND expires_at > to_timestamp($3 / 1000.0) \
                         AND ($4::text IS NULL OR topic = $4) ORDER BY id DESC LIMIT $5",
                )
                .bind(project)
                .bind(broker)
                .bind(now.as_millis())
                .bind(topic)
                .bind(i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX))
                .fetch_all(&mut *tx)
                .await?;
                tx.commit().await?;
                rows.into_iter()
                    .map(
                        |(topic, client_id, received_at, expires_at, payload)| Message {
                            topic,
                            client_id,
                            received_at: at(received_at),
                            expires_at: at(expires_at),
                            payload,
                        },
                    )
                    .collect()
            }
        };
        let more = found.len() > limit;
        found.truncate(limit);
        Ok((found, more))
    }

    /// Deletes the broker's whole capture; answers how many messages went.
    pub async fn purge(&self, project: &str, broker: &str) -> Result<u64, sqlx::Error> {
        match &self.inner {
            Inner::Memory(memory) => Ok(Self::memory(memory)
                .remove(&(project.to_owned(), broker.to_owned()))
                .map_or(0, |kept| kept.len() as u64)),
            Inner::Postgres(pool, mirror) => {
                let Some(key) = crate::project_ids::key_of(pool, mirror, project).await? else {
                    return Ok(0);
                };
                let mut tx = crate::db::tenant_tx(pool, key).await?;
                let gone =
                    sqlx::query("DELETE FROM mqtt_capture WHERE project = $1 AND broker = $2")
                        .bind(project)
                        .bind(broker)
                        .execute(&mut *tx)
                        .await?
                        .rows_affected();
                tx.commit().await?;
                Ok(gone)
            }
        }
    }

    /// Deletes every expired message, and every message of a broker not in `capturing` (it left
    /// `capture` or was deleted), project by project in its key (STO-07).
    pub async fn sweep(
        &self,
        capturing: &BTreeSet<Key>,
        now: TimestampTz,
    ) -> Result<u64, sqlx::Error> {
        match &self.inner {
            Inner::Memory(memory) => {
                let mut memory = Self::memory(memory);
                let before: usize = memory.values().map(VecDeque::len).sum();
                memory.retain(|key, _| capturing.contains(key));
                for kept in memory.values_mut() {
                    kept.retain(|m| m.expires_at > now);
                }
                let after: usize = memory.values().map(VecDeque::len).sum();
                Ok((before - after) as u64)
            }
            Inner::Postgres(pool, mirror) => {
                let mut gone = 0;
                for (project, key) in crate::project_ids::keyed_projects(pool, mirror).await? {
                    let brokers: Vec<&str> = capturing
                        .iter()
                        .filter(|(p, _)| *p == project)
                        .map(|(_, b)| b.as_str())
                        .collect();
                    let mut tx = crate::db::tenant_tx(pool, key).await?;
                    gone += sqlx::query(
                        "DELETE FROM mqtt_capture WHERE project = $2 \
                         AND (expires_at <= to_timestamp($1 / 1000.0) OR NOT (broker = ANY($3)))",
                    )
                    .bind(now.as_millis())
                    .bind(&project)
                    .bind(&brokers)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected();
                    tx.commit().await?;
                }
                Ok(gone)
            }
        }
    }
}

/// The running subscribers, one per capture broker, by release.
pub struct Capture {
    store: Arc<CaptureStore>,
    namespace: String,
    running: Mutex<BTreeMap<String, (Target, tokio::task::AbortHandle)>>,
}

impl Capture {
    pub fn new(store: Arc<CaptureStore>, namespace: String) -> Self {
        Self {
            store,
            namespace,
            running: Mutex::default(),
        }
    }

    fn running(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<String, (Target, tokio::task::AbortHandle)>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs a subscriber for each capture broker of `brokers` and stops the others. `retire` is
    /// false while the mirror misses files: then a broker it lacks keeps its subscriber and its
    /// capture, as its release keeps running.
    pub async fn converge(&self, brokers: &[Broker], retire: bool) {
        let wanted: BTreeMap<String, Target> = targets(brokers)
            .into_iter()
            .map(|target| (target.release.clone(), target))
            .collect();
        {
            let mut running = self.running();
            running.retain(|release, (target, task)| {
                let keep = match wanted.get(release) {
                    Some(wanted) => wanted == target,
                    None => !retire,
                };
                if !keep {
                    task.abort();
                }
                keep
            });
            for (release, target) in &wanted {
                if running.contains_key(release) {
                    continue;
                }
                let host = format!("{release}.{}.svc.cluster.local", self.namespace);
                let task = tokio::spawn(subscribe(
                    Arc::clone(&self.store),
                    target.clone(),
                    host,
                    PORT,
                ));
                running.insert(release.clone(), (target.clone(), task.abort_handle()));
            }
        }
        if !retire {
            return;
        }
        let capturing = wanted.values().map(Target::key).collect();
        match self.store.sweep(&capturing, TimestampTz::now()).await {
            Ok(0) => {}
            Ok(gone) => tracing::info!(messages = gone, "MQTT capture swept"),
            Err(error) => tracing::warn!(%error, "the MQTT capture was not swept"),
        }
    }

    /// Stops every subscriber: this replica no longer leads the reconciler.
    pub fn stop(&self) {
        for (_, (_, task)) in std::mem::take(&mut *self.running()) {
            task.abort();
        }
    }
}

/// One broker's subscriber: dials the internal listener on `port` (tests pass their own),
/// subscribes after every CONNACK (a clean session forgets them) and writes what arrived once
/// per [`FLUSH`]. The buffer holds at most what the broker's capture could keep, so a burst
/// costs no more memory than that.
async fn subscribe(store: Arc<CaptureStore>, target: Target, host: String, port: u16) {
    let mut options = MqttOptions::new(format!("{USERNAME}-{}", target.release), host, port);
    options
        .set_credentials(USERNAME, "")
        .set_keep_alive(Duration::from_secs(30))
        .set_clean_session(true)
        // The topic and the header beside the largest payload the broker lets through.
        .set_max_packet_size(target.max_message_bytes.saturating_add(70_000), 64 * 1024);
    let (client, mut events) = AsyncClient::new(options, 16);
    let topics: Vec<SubscribeFilter> = filters(&target.publish)
        .into_iter()
        .map(|path| SubscribeFilter::new(path, QoS::AtMostOnce))
        .collect();
    let mut buffer: VecDeque<Message> = VecDeque::new();
    let mut bytes = 0usize;
    let mut tick = tokio::time::interval(FLUSH);
    loop {
        // The poll future lives across flushes: rumqttc dials and waits for the CONNACK inside
        // it, so dropping it on a tick would drop the half-open connection with it.
        let event = {
            let poll = events.poll();
            tokio::pin!(poll);
            loop {
                tokio::select! {
                    event = &mut poll => break event,
                    _ = tick.tick() => {
                        if buffer.is_empty() {
                            continue;
                        }
                        let batch: Vec<Message> = std::mem::take(&mut buffer).into();
                        bytes = 0;
                        if let Err(error) = store
                            .keep(&target.project, &target.name, target.per_topic, batch, TimestampTz::now())
                            .await
                        {
                            tracing::warn!(broker = %target.release, %error, "the MQTT capture was not stored");
                        }
                    }
                }
            }
        };
        match event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                if let Err(error) = client.try_subscribe_many(topics.clone()) {
                    tracing::warn!(broker = %target.release, %error, "the MQTT capture did not subscribe");
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let received_at = TimestampTz::now();
                bytes = bytes.saturating_add(publish.payload.len());
                buffer.push_back(Message {
                    client_id: client_of(&target.publish, &publish.topic),
                    topic: publish.topic,
                    received_at,
                    expires_at: received_at + jiff::SignedDuration::from_secs(target.window),
                    payload: publish.payload.to_vec(),
                });
                // `bytes` is the sum of the buffer's payloads, so an empty buffer holds 0 and ends
                // the loop; were it ever not, the empty buffer's `bytes` drop to 0 all the same.
                while buffer.len() > BROKER_MESSAGES || bytes > BROKER_BYTES {
                    let oldest = buffer
                        .pop_front()
                        .map_or(bytes, |oldest| oldest.payload.len());
                    bytes = bytes.saturating_sub(oldest);
                }
            }
            Ok(_) => {}
            Err(error) => {
                // The broker and the error only: an error never carries a payload.
                tracing::warn!(broker = %target.release, %error, "the MQTT capture lost its broker; dialing again");
                tokio::time::sleep(RETRY).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(second: i64) -> TimestampTz {
        TimestampTz::from_second(1_800_000_000 + second).unwrap_or_default()
    }

    fn message(topic: &str, second: i64, bytes: usize) -> Message {
        Message {
            topic: topic.to_owned(),
            client_id: None,
            received_at: at(second),
            expires_at: at(second + 60),
            payload: vec![b'x'; bytes],
        }
    }

    fn topics(messages: &VecDeque<Message>) -> Vec<(String, TimestampTz)> {
        messages
            .iter()
            .map(|m| (m.topic.clone(), m.received_at))
            .collect()
    }

    #[test]
    fn the_filters_widen_every_placeholder_level_like_the_gateway() {
        let publish = vec![
            "cams/${clientid}/frame".to_owned(),
            "cams/dev-${clientid}/#".to_owned(),
            "status".to_owned(),
            "cams/+/frame".to_owned(),
        ];
        assert_eq!(filters(&publish), ["cams/+/#", "cams/+/frame", "status"]);
        assert_eq!(USERNAME, "jc-portal-capture");
    }

    #[test]
    fn the_device_is_the_level_a_filter_names_as_its_client_id() {
        let publish = vec![
            "cams/${clientid}/frame".to_owned(),
            "air/${clientid}/#".to_owned(),
            "status/+".to_owned(),
        ];
        assert_eq!(
            client_of(&publish, "cams/cam-west/frame").as_deref(),
            Some("cam-west")
        );
        assert_eq!(
            client_of(&publish, "air/sensor-1/pm/10").as_deref(),
            Some("sensor-1")
        );
        assert_eq!(client_of(&publish, "status/x"), None);
        assert_eq!(client_of(&publish, "cams/cam-west/frame/extra"), None);
        assert_eq!(client_of(&publish, "cams//frame"), None);
        assert_eq!(client_of(&publish, "other"), None);
    }

    #[test]
    fn a_topic_keeps_its_newest_messages_per_topic() {
        let mut kept: VecDeque<Message> = (0..5)
            .map(|s| message("a", s, 1))
            .chain((0..2).map(|s| message("b", s, 1)))
            .collect();
        trim(&mut kept, 2, at(10));
        assert_eq!(
            topics(&kept),
            [
                ("a".to_owned(), at(3)),
                ("a".to_owned(), at(4)),
                ("b".to_owned(), at(0)),
                ("b".to_owned(), at(1)),
            ]
        );
    }

    #[test]
    fn a_broker_keeps_at_most_its_count_and_its_bytes() {
        let mut kept: VecDeque<Message> = (0..1200)
            .map(|s| message(&format!("t/{s}"), s, 1))
            .collect();
        // At 0 none is past its window: the count alone trims.
        trim(&mut kept, 20, at(0));
        assert_eq!(kept.len(), BROKER_MESSAGES);
        assert_eq!(kept.front().map(|m| m.received_at), Some(at(200)));

        // Three messages of 3 MiB: the newest two fit in 8 MiB, the oldest goes even though a
        // smaller message older still would fit.
        let mib = 1024 * 1024;
        let mut kept: VecDeque<Message> = [
            message("s", 0, 1),
            message("a", 1, 3 * mib),
            message("b", 2, 3 * mib),
            message("c", 3, 3 * mib),
        ]
        .into();
        trim(&mut kept, 20, at(3));
        assert_eq!(
            topics(&kept),
            [("b".to_owned(), at(2)), ("c".to_owned(), at(3))]
        );
    }

    /// A manifest's parse error as the mirror keeps it.
    fn unparsed(error: serde_json::Error) -> String {
        error.to_string()
    }

    fn broker(name: &str, mode: &str) -> Broker {
        let spec = serde_json::from_value(serde_json::json!({
            "mode": mode,
            "roles": [{"name": "camera", "serviceAccounts": ["cam"], "publish": ["cams/${clientid}/frame"]}],
        }))
        .map_err(unparsed);
        Broker {
            project: "p".to_owned(),
            name: name.to_owned(),
            release: format!("mqtt-p-{name}"),
            spec,
            certificate: None,
            door: None,
        }
    }

    #[tokio::test]
    async fn a_subscriber_runs_per_capture_broker_and_its_capture_goes_with_the_mode() {
        let store = Arc::new(CaptureStore::new(None, Default::default()));
        let now = TimestampTz::now();
        let fresh = |topic: &str| Message {
            expires_at: now + jiff::SignedDuration::from_hours(1),
            ..message(topic, 0, 1)
        };
        for name in ["cams", "old"] {
            store
                .keep("p", name, 20, vec![fresh("cams/a/frame")], now)
                .await
                .expect("kept");
        }
        let capture = Capture::new(Arc::clone(&store), "jc-mqtt".to_owned());
        let brokers = [
            broker("cams", "capture"),
            broker("plain", "ingest"),
            broker("old", "relay"),
        ];
        capture.converge(&brokers, true).await;
        assert_eq!(
            capture.running().keys().collect::<Vec<_>>(),
            ["mqtt-p-cams"]
        );
        let held = |name: &'static str| {
            let store = Arc::clone(&store);
            async move {
                store
                    .read("p", name, None, 10, now)
                    .await
                    .expect("read")
                    .0
                    .len()
            }
        };
        assert_eq!((held("cams").await, held("old").await), (1, 0));

        // A mirror missing files retires nothing; a complete one without the broker does.
        capture.converge(&[], false).await;
        assert_eq!(capture.running().len(), 1);
        assert_eq!(held("cams").await, 1);
        capture.converge(&[], true).await;
        assert!(capture.running().is_empty());
        assert_eq!(held("cams").await, 0);

        capture.converge(&brokers, true).await;
        capture.stop();
        assert!(capture.running().is_empty());
    }

    #[tokio::test]
    async fn the_window_purges_and_the_mode_leaving_deletes() {
        let store = CaptureStore::new(None, Default::default());
        let batch = vec![message("a", 0, 1), message("a", 30, 1), message("b", 40, 1)];
        store
            .keep("p", "cams", 20, batch, at(41))
            .await
            .expect("kept");
        store
            .keep("p", "other", 20, vec![message("x", 0, 1)], at(41))
            .await
            .expect("kept");
        // At 70 the message of 0 is past its 60 s window: never read, then swept.
        let (read, more) = store
            .read("p", "cams", None, 10, at(70))
            .await
            .expect("read");
        assert_eq!(read.len(), 2);
        assert!(!more);
        assert_eq!(read[0].topic, "b", "newest first");
        let (one, more) = store
            .read("p", "cams", Some("a"), 1, at(70))
            .await
            .expect("read");
        assert_eq!((one.len(), more), (1, false));
        let (one, more) = store
            .read("p", "cams", None, 1, at(70))
            .await
            .expect("read");
        assert_eq!((one.len(), more), (1, true));

        let capturing: BTreeSet<Key> = [("p".to_owned(), "cams".to_owned())].into();
        // `other` left capture (all of it), and one message of `cams` expired.
        assert_eq!(store.sweep(&capturing, at(70)).await.expect("swept"), 2);
        assert_eq!(store.purge("p", "cams").await.expect("purged"), 2);
        assert!(store
            .read("p", "cams", None, 10, at(70))
            .await
            .expect("read")
            .0
            .is_empty());
    }

    fn dead_pool() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool")
    }

    /// A mirror that places `p` in an Organization, so a store on a dead pool reaches the pool.
    fn mirror_placing_p() -> Arc<crate::store::Mirror> {
        let mirror = Arc::new(crate::store::Mirror::new());
        mirror.upsert(
            serde_json::from_value(serde_json::json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Project",
                "metadata": { "name": "p", "namespace": crate::permissions::ORG_NAMESPACE },
                "spec": { "organizationRef": "o" },
            }))
            .expect("a Project"),
        );
        mirror
    }

    async fn database() -> sqlx::PgPool {
        let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
        crate::db::connect(&url).await.expect("connect + migrate")
    }

    fn unique() -> String {
        jc_types::ProjectId::mint().path_segment().to_string()
    }

    fn capture_broker(publish: &str) -> Broker {
        let spec = serde_json::from_value(serde_json::json!({
            "mode": "capture",
            "roles": [{"name": "camera", "serviceAccounts": ["cam"], "publish": [publish]}],
        }))
        .map_err(unparsed);
        Broker {
            project: "p".to_owned(),
            name: "cams".to_owned(),
            release: "mqtt-p-cams".to_owned(),
            spec,
            certificate: None,
            door: None,
        }
    }

    #[test]
    fn a_manifest_that_does_not_parse_is_no_target() {
        let bad = broker("bad", "no-such-mode");
        assert!(bad.spec.is_err());
        let found = targets(&[bad, broker("ok", "capture")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "ok");
    }

    #[test]
    fn a_capture_broker_without_roles_subscribes_to_nothing() {
        let mut empty = broker("none", "capture");
        empty.spec.as_mut().expect("a capture broker").roles.clear();
        let found = targets(&[empty]);
        assert_eq!(found.len(), 1);
        assert!(found[0].publish.is_empty());
        assert!(filters(&found[0].publish).is_empty());
    }

    #[tokio::test]
    async fn a_poisoned_lock_of_the_memory_store_still_serves() {
        let store = Arc::new(CaptureStore::new(None, Default::default()));
        let poisoner = Arc::clone(&store);
        let joined = std::thread::spawn(move || {
            if let Inner::Memory(memory) = &poisoner.inner {
                let _held = memory.lock().expect("not poisoned yet");
                panic!("poison the lock on purpose");
            }
        })
        .join();
        assert!(joined.is_err());
        store
            .keep("p", "b", 5, vec![message("a", 0, 1)], at(1))
            .await
            .expect("kept");
        assert_eq!(store.purge("p", "b").await.expect("purged"), 1);
        assert_eq!(store.purge("p", "b").await.expect("purged"), 0);
    }

    #[tokio::test]
    async fn a_poisoned_list_of_subscribers_can_still_be_stopped() {
        let capture = Capture::new(
            Arc::new(CaptureStore::new(None, Default::default())),
            "jc-mqtt".to_owned(),
        );
        let outcome = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _held = capture.running.lock().expect("not poisoned yet");
                    panic!("poison the lock on purpose");
                })
                .join()
        });
        assert!(outcome.is_err());
        capture.stop();
        assert!(capture.running().is_empty());
    }

    #[tokio::test]
    async fn a_running_subscriber_stays_until_its_manifest_changes() {
        let capture = Capture::new(
            Arc::new(CaptureStore::new(None, Default::default())),
            "jc-mqtt".to_owned(),
        );
        capture
            .converge(&[capture_broker("cams/${clientid}/frame")], true)
            .await;
        capture
            .converge(&[capture_broker("cams/${clientid}/frame")], true)
            .await;
        assert_eq!(capture.running().len(), 1);
        capture.converge(&[capture_broker("status/#")], true).await;
        let publish = capture
            .running()
            .get("mqtt-p-cams")
            .map(|(target, _)| target.publish.clone());
        assert_eq!(publish, Some(vec!["status/#".to_owned()]));
        capture.stop();
    }

    #[tokio::test]
    async fn a_sweep_that_cannot_reach_the_database_does_not_stop_the_convergence() {
        let store = Arc::new(CaptureStore::new(Some(dead_pool()), mirror_placing_p()));
        let capture = Capture::new(store, "jc-mqtt".to_owned());
        capture.converge(&[], true).await;
        assert!(capture.running().is_empty());
    }

    #[tokio::test]
    async fn a_database_that_is_down_fails_every_store_call() {
        let store = CaptureStore::new(Some(dead_pool()), mirror_placing_p());
        assert!(store
            .keep("p", "b", 5, vec![message("a", 0, 1)], at(1))
            .await
            .is_err());
        assert!(store.read("p", "b", None, 5, at(1)).await.is_err());
        assert!(store.purge("p", "b").await.is_err());
        let capturing: BTreeSet<Key> = BTreeSet::new();
        assert!(store.sweep(&capturing, at(1)).await.is_err());
    }

    #[tokio::test]
    async fn a_project_without_a_key_keeps_reads_and_purges_nothing() {
        let pool = database().await;
        let store = CaptureStore::new(Some(pool), Arc::new(crate::store::Mirror::new()));
        let project = format!("cov-{}", unique());
        store
            .keep(&project, "cams", 5, vec![message("a", 0, 1)], at(1))
            .await
            .expect("nothing kept, no error");
        let (found, more) = store
            .read(&project, "cams", None, 5, at(1))
            .await
            .expect("read");
        assert!(found.is_empty());
        assert!(!more);
        assert_eq!(store.purge(&project, "cams").await.expect("purged"), 0);
    }

    #[tokio::test]
    async fn a_keyed_project_keeps_an_empty_batch_as_a_trim_and_reads_its_messages_back() {
        let pool = database().await;
        let mirror = Arc::new(crate::store::Mirror::new());
        let project = format!("cov-{}", unique());
        let organization = format!("cov-org-{}", unique());
        crate::project_ids::placed(&pool, &mirror, &project, &organization).await;
        let store = CaptureStore::new(Some(pool), mirror);
        let now = TimestampTz::now();
        let fresh = Message {
            expires_at: now + jiff::SignedDuration::from_hours(1),
            received_at: now,
            ..message("cams/a/frame", 0, 4)
        };
        store
            .keep(&project, "cams", 5, vec![fresh.clone()], now)
            .await
            .expect("kept");
        store
            .keep(&project, "cams", 5, Vec::new(), now)
            .await
            .expect("an empty batch only trims");
        let (found, more) = store
            .read(&project, "cams", Some("cams/a/frame"), 5, now)
            .await
            .expect("read");
        assert_eq!((found.len(), more), (1, false));
        assert_eq!(found[0].payload, fresh.payload);
        let (none, _) = store
            .read(&project, "cams", Some("other"), 5, now)
            .await
            .expect("read");
        assert!(none.is_empty());
        assert_eq!(store.purge(&project, "cams").await.expect("purged"), 1);
    }

    // The MQTT 3.1.1 packets of a broker that answers one subscriber, by hand.
    async fn read_packet(socket: &mut tokio::net::TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
        use tokio::io::AsyncReadExt;
        let first = socket.read_u8().await?;
        // The subscriber's CONNECT and SUBSCRIBE are under 128 bytes: one length byte.
        let length = socket.read_u8().await?;
        assert!(
            length < 0x80,
            "a packet of the subscriber's is under 128 bytes"
        );
        let mut body = vec![0u8; usize::from(length)];
        socket.read_exact(&mut body).await?;
        Ok((first, body))
    }

    fn publish_packet(topic: &str, payload: &[u8]) -> Vec<u8> {
        let mut remaining = 2 + topic.len() + payload.len();
        let mut packet = vec![0x30];
        loop {
            let mut byte = (remaining % 128) as u8;
            remaining /= 128;
            if remaining > 0 {
                byte |= 0x80;
            }
            packet.push(byte);
            if remaining == 0 {
                break;
            }
        }
        packet.extend_from_slice(&(topic.len() as u16).to_be_bytes());
        packet.extend_from_slice(topic.as_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    /// Answers one connection: CONNACK after `connack`, SUBACK, then `messages`, then holds it
    /// for `hold`.
    async fn one_connection(
        mut socket: tokio::net::TcpStream,
        (connack, messages): (Duration, Vec<(String, Vec<u8>)>),
        hold: Duration,
    ) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        read_packet(&mut socket).await?;
        tokio::time::sleep(connack).await;
        socket.write_all(&[0x20, 0x02, 0x00, 0x00]).await?;
        // The subscriber's next packet is its SUBSCRIBE.
        let (first, body) = read_packet(&mut socket).await?;
        assert_eq!(first >> 4, 8, "a SUBSCRIBE");
        socket.write_all(&[0x90, 3, body[0], body[1], 0x00]).await?;
        for (topic, payload) in &messages {
            socket.write_all(&publish_packet(topic, payload)).await?;
        }
        tokio::time::sleep(hold).await;
        Ok(())
    }

    fn cams_target() -> Target {
        Target {
            project: "p".to_owned(),
            name: "cams".to_owned(),
            release: "mqtt-p-cams".to_owned(),
            publish: vec!["cams/${clientid}/frame".to_owned()],
            per_topic: 5000,
            window: 3600,
            max_message_bytes: 1024 * 1024,
        }
    }

    /// A subscriber against a broker of its own on the MQTT port: what arrives is stored with the
    /// device it names, a burst beyond the broker's cap keeps the newest, and a broker that drops
    /// the connection or a store that refuses the write stops nothing.
    #[tokio::test]
    async fn a_subscriber_stores_what_its_broker_publishes_and_survives_a_refused_store() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", PORT))
            .await
            .expect("the MQTT port is free on this host");
        let burst: Vec<(String, Vec<u8>)> = (0..BROKER_MESSAGES + 5)
            .map(|n| ("cams/cam-1/frame".to_owned(), n.to_string().into_bytes()))
            .collect();
        // The first CONNACK comes later than a flush tick: the dial must survive the tick.
        let scripts = Arc::new(Mutex::new(VecDeque::from([
            (FLUSH + Duration::from_millis(500), burst),
            // Over 127 bytes: its PUBLISH carries a length of two bytes.
            (
                Duration::ZERO,
                vec![("cams/cam-2/frame".to_owned(), vec![b'l'; 200])],
            ),
        ])));
        let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepting = {
            let served = Arc::clone(&served);
            tokio::spawn(async move {
                // One connection per script: the first subscriber's, then the second's.
                for _ in 0..2 {
                    let (socket, _) = listener.accept().await.expect("accept");
                    let script = scripts
                        .lock()
                        .expect("the scripts' lock")
                        .pop_front()
                        .expect("a script per connection");
                    let served = Arc::clone(&served);
                    tokio::spawn(async move {
                        let _ = one_connection(socket, script, Duration::from_millis(2500)).await;
                        served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    });
                }
            })
        };

        let store = Arc::new(CaptureStore::new(None, Default::default()));
        let first = tokio::spawn(subscribe(
            Arc::clone(&store),
            cams_target(),
            "127.0.0.1".to_owned(),
            PORT,
        ));
        let started = std::time::Instant::now();
        let mut found = Vec::new();
        while started.elapsed() < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            found = store
                .read("p", "cams", None, 5000, TimestampTz::now())
                .await
                .expect("read")
                .0;
            let newest = (BROKER_MESSAGES + 4).to_string().into_bytes();
            if found.len() >= BROKER_MESSAGES && found[0].payload == newest {
                break;
            }
        }
        assert_eq!(found.len(), BROKER_MESSAGES, "the broker's cap holds");
        assert_eq!(
            found[0].payload,
            (BROKER_MESSAGES + 4).to_string().into_bytes()
        );
        assert_eq!(found[0].client_id.as_deref(), Some("cam-1"));
        // The broker drops the connection after its hold: the subscriber says so and dials again.
        while served.load(std::sync::atomic::Ordering::SeqCst) < 1
            && started.elapsed() < Duration::from_secs(20)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 1);
        first.abort();

        // A store whose database is down: the batch is lost with a warning, the subscriber lives.
        let refusing = Arc::new(CaptureStore::new(Some(dead_pool()), mirror_placing_p()));
        let second = tokio::spawn(subscribe(
            refusing,
            cams_target(),
            "127.0.0.1".to_owned(),
            PORT,
        ));
        while served.load(std::sync::atomic::Ordering::SeqCst) < 2
            && started.elapsed() < Duration::from_secs(40)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(!second.is_finished());
        second.abort();
        accepting.abort();
    }

    /// A broker whose release names no topic: after the CONNACK there is nothing to subscribe to,
    /// and the subscriber says so instead of failing.
    #[tokio::test]
    async fn a_broker_with_no_topic_to_subscribe_to_is_named_in_the_log() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("address").port();
        let broker = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept");
            let _ = one_connection(socket, (Duration::ZERO, Vec::new()), Duration::ZERO).await;
        });
        let target = Target {
            publish: Vec::new(),
            ..cams_target()
        };
        let store = Arc::new(CaptureStore::new(None, Default::default()));
        let (_, log) = crate::test_log::logged(tracing::level_filters::LevelFilter::WARN, async {
            tokio::time::timeout(
                Duration::from_secs(3),
                subscribe(store, target, "127.0.0.1".to_owned(), port),
            )
            .await
        })
        .await;
        // The subscriber hung up at its timeout, which ends the broker's connection too.
        broker.await.expect("the broker's connection ended");
        assert!(log.contains("the MQTT capture did not subscribe"), "{log}");
        assert!(log.contains("mqtt-p-cams"), "{log}");
    }
}
