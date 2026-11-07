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

use chrono::{DateTime, Utc};
use jc_core::kinds::mqtt_broker::{MqttCapture, MqttMode};
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
    pub received_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub payload: Vec<u8>,
}

/// Keeps the newest of a broker's `messages` (oldest first) within its caps: none past its
/// window, at most `per_topic` per topic, [`BROKER_MESSAGES`] and [`BROKER_BYTES`] in all.
/// Once the count or the bytes are reached, every older message goes too.
pub fn trim(messages: &mut VecDeque<Message>, per_topic: usize, now: DateTime<Utc>) {
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

/// A stored instant, in milliseconds since the epoch: this crate's sqlx reads no chrono type.
fn at(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or_default()
}

enum Inner {
    Postgres(sqlx::PgPool),
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
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(Mutex::default()),
            },
        }
    }

    fn memory(
        memory: &Mutex<BTreeMap<Key, VecDeque<Message>>>,
    ) -> std::sync::MutexGuard<'_, BTreeMap<Key, VecDeque<Message>>> {
        memory.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds `batch` (oldest first) to the broker's capture and trims it to its caps.
    pub async fn keep(
        &self,
        project: &str,
        broker: &str,
        per_topic: usize,
        batch: Vec<Message>,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let pool = match &self.inner {
            Inner::Memory(memory) => {
                let mut memory = Self::memory(memory);
                let kept = memory
                    .entry((project.to_owned(), broker.to_owned()))
                    .or_default();
                kept.extend(batch);
                trim(kept, per_topic, now);
                return Ok(());
            }
            Inner::Postgres(pool) => pool,
        };
        let mut tx = pool.begin().await?;
        if !batch.is_empty() {
            let mut topics = Vec::with_capacity(batch.len());
            let mut clients = Vec::with_capacity(batch.len());
            let mut received = Vec::with_capacity(batch.len());
            let mut expires = Vec::with_capacity(batch.len());
            let mut payloads = Vec::with_capacity(batch.len());
            for message in batch {
                topics.push(message.topic);
                clients.push(message.client_id);
                received.push(message.received_at.timestamp_millis());
                expires.push(message.expires_at.timestamp_millis());
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
            .bind(now.timestamp_millis())
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
        now: DateTime<Utc>,
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
            Inner::Postgres(pool) => {
                let rows: Vec<Row> = sqlx::query_as(
                    "SELECT topic, client_id, (extract(epoch FROM received_at) * 1000)::bigint, \
                         (extract(epoch FROM expires_at) * 1000)::bigint, payload FROM mqtt_capture \
                         WHERE project = $1 AND broker = $2 AND expires_at > to_timestamp($3 / 1000.0) \
                         AND ($4::text IS NULL OR topic = $4) ORDER BY id DESC LIMIT $5",
                )
                .bind(project)
                .bind(broker)
                .bind(now.timestamp_millis())
                .bind(topic)
                .bind(i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX))
                .fetch_all(pool)
                .await?;
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
            Inner::Postgres(pool) => Ok(sqlx::query(
                "DELETE FROM mqtt_capture WHERE project = $1 AND broker = $2",
            )
            .bind(project)
            .bind(broker)
            .execute(pool)
            .await?
            .rows_affected()),
        }
    }

    /// Deletes every expired message, and every message of a broker not in `capturing` (it left
    /// `capture` or was deleted).
    pub async fn sweep(
        &self,
        capturing: &BTreeSet<Key>,
        now: DateTime<Utc>,
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
            Inner::Postgres(pool) => {
                let (projects, brokers): (Vec<String>, Vec<String>) =
                    capturing.iter().cloned().unzip();
                Ok(sqlx::query(
                    "DELETE FROM mqtt_capture WHERE expires_at <= to_timestamp($1 / 1000.0) OR (project, broker) NOT IN \
                     (SELECT * FROM UNNEST($2::text[], $3::text[]))",
                )
                .bind(now.timestamp_millis())
                .bind(projects)
                .bind(brokers)
                .execute(pool)
                .await?
                .rows_affected())
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
                let task = tokio::spawn(subscribe(Arc::clone(&self.store), target.clone(), host));
                running.insert(release.clone(), (target.clone(), task.abort_handle()));
            }
        }
        if !retire {
            return;
        }
        let capturing = wanted.values().map(Target::key).collect();
        match self.store.sweep(&capturing, Utc::now()).await {
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

/// One broker's subscriber: dials the internal listener, subscribes after every CONNACK (a clean
/// session forgets them) and writes what arrived once per [`FLUSH`]. The buffer holds at most
/// what the broker's capture could keep, so a burst costs no more memory than that.
async fn subscribe(store: Arc<CaptureStore>, target: Target, host: String) {
    let mut options = MqttOptions::new(format!("{USERNAME}-{}", target.release), host, PORT);
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
        tokio::select! {
            event = events.poll() => match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    if let Err(error) = client.try_subscribe_many(topics.clone()) {
                        tracing::warn!(broker = %target.release, %error, "the MQTT capture did not subscribe");
                    }
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let received_at = Utc::now();
                    bytes = bytes.saturating_add(publish.payload.len());
                    buffer.push_back(Message {
                        client_id: client_of(&target.publish, &publish.topic),
                        topic: publish.topic,
                        received_at,
                        expires_at: received_at + chrono::Duration::seconds(target.window),
                        payload: publish.payload.to_vec(),
                    });
                    while buffer.len() > BROKER_MESSAGES || bytes > BROKER_BYTES {
                        let Some(oldest) = buffer.pop_front() else { break };
                        bytes = bytes.saturating_sub(oldest.payload.len());
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    // The broker and the error only: an error never carries a payload.
                    tracing::warn!(broker = %target.release, %error, "the MQTT capture lost its broker; dialing again");
                    tokio::time::sleep(RETRY).await;
                }
            },
            _ = tick.tick() => {
                if buffer.is_empty() {
                    continue;
                }
                let batch: Vec<Message> = std::mem::take(&mut buffer).into();
                bytes = 0;
                if let Err(error) = store
                    .keep(&target.project, &target.name, target.per_topic, batch, Utc::now())
                    .await
                {
                    tracing::warn!(broker = %target.release, %error, "the MQTT capture was not stored");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(second: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + second, 0).unwrap_or_default()
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

    fn topics(messages: &VecDeque<Message>) -> Vec<(String, DateTime<Utc>)> {
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

    fn broker(name: &str, mode: &str) -> Broker {
        let spec = serde_json::from_value(serde_json::json!({
            "mode": mode,
            "roles": [{"name": "camera", "serviceAccounts": ["cam"], "publish": ["cams/${clientid}/frame"]}],
        }))
        .map_err(|error| error.to_string());
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
        let store = Arc::new(CaptureStore::new(None));
        let now = Utc::now();
        let fresh = |topic: &str| Message {
            expires_at: now + chrono::Duration::hours(1),
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
        let store = CaptureStore::new(None);
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
}
