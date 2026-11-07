//! What is happening in a project, as one record every source writes (UI-31, OPS-48, OPS-49).
//!
//! The store is a projection built for reading: the reconciler and the CKAN publisher write
//! their events in-process, every other source emits an OpenTelemetry log line the collector
//! hands to `POST /api/v1/activity`. Losing it loses nothing that is not still in the logs and
//! the traces, so retention is short — seven days of events
//! ([Architecture/09 §6](../../docs/Architecture/09-portal.md)).
//!
//! Two storage arms, like the drafts store: `Db(PgPool)` over migration `0012_activity.sql`,
//! and `Memory` when the Portal runs without a database.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use tokio::sync::{broadcast, RwLock};
use utoipa::ToSchema;

/// The closed vocabulary. A filter over free text is not a filter, so a record whose `kind` is
/// not one of these is refused at the door rather than stored and never found again.
pub const KINDS: &[&str] = &[
    "config.planned",
    "config.applied",
    "config.drifted",
    "change.merged",
    "pipeline.throughput",
    "pipeline.error",
    "pipeline.restarted",
    "pipeline.run",
    "endpoint.traffic",
    "endpoint.rate-limited",
    "access.denied",
    "access.simulated",
    "mcp.tool",
    "agent.answer",
    "federation.forward",
    "federation.error",
    "catalogue.published",
    "person.changed",
    "model.key",
    "assistant.transcript",
];

/// Which component said so.
pub const SOURCES: &[&str] = &[
    "reconciler",
    "pipeline",
    "gateway",
    "broker",
    "ckan",
    "portal",
];

/// How long an event is kept (OPS-49). The per-minute counters it aggregates into live longer,
/// and are not this table.
pub const RETENTION_DAYS: i64 = 7;

/// How often the leader trims (OPS-49): the table holds at most an hour past retention.
pub const TRIM_EVERY_MINUTES: i64 = 60;

/// Rows one DELETE removes, so a first trim over a month of backlog never holds one long lock.
pub const TRIM_BATCH: i64 = 10_000;

/// What one trim removed, and in how many DELETE statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trimmed {
    pub rows: u64,
    pub batches: u32,
}

/// The largest page a caller may ask for, and the page they get without asking.
pub const MAX_LIMIT: i64 = 200;
pub const DEFAULT_LIMIT: i64 = 50;

/// `info` < `warning` < `error`: a filter names the floor and gets everything above it.
fn severity_rank(severity: &str) -> Option<i32> {
    match severity {
        "info" => Some(0),
        "warning" => Some(1),
        "error" => Some(2),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    #[schema(value_type = String, format = DateTime)]
    pub time: DateTime<Utc>,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    pub kind: String,
    pub source: String,
    pub summary: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// A small object of named values, never a payload (OPS-48). `details.object` is the
    /// `{plural}/{name}` an object page filters on.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

/// The longest summary a record stores.
const MAX_SUMMARY_CHARS: usize = 2_000;
/// The longest correlationId: a W3C trace id is 32 hex characters, a request id fewer.
const MAX_CORRELATION_BYTES: usize = 128;
/// The largest `details` object, as JSON.
const MAX_DETAILS_BYTES: usize = 4_096;

impl ActivityEvent {
    /// The object an event belongs to, as an object page asks for it.
    pub fn object(&self) -> Option<&str> {
        self.details.get("object").and_then(Value::as_str)
    }

    /// What makes a record storable: a known kind, a known source, a known severity and a
    /// project that could name one. The collector strips attributes, which is redaction and not
    /// a trust boundary; this is the boundary.
    pub fn validate(&self) -> Result<(), String> {
        if !crate::resource::is_dns1123(&self.project) {
            return Err(format!("'{}' is not a project name", self.project));
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return Err(format!("kind '{}' is not in the vocabulary", self.kind));
        }
        if !SOURCES.contains(&self.source.as_str()) {
            return Err(format!("source '{}' is not a known source", self.source));
        }
        if severity_rank(&self.severity).is_none() {
            return Err(format!("severity '{}' is not a severity", self.severity));
        }
        if self.summary.trim().is_empty() {
            return Err("an event with no summary says nothing to a person".into());
        }
        // Every list serves each row for seven days: a record is a sentence and a few named
        // values, never a payload (OPS-48, F-R2POR1-013).
        if self.summary.chars().count() > MAX_SUMMARY_CHARS {
            return Err(format!(
                "a summary is one sentence of at most {MAX_SUMMARY_CHARS} characters"
            ));
        }
        if self
            .correlation_id
            .as_ref()
            .is_some_and(|id| id.len() > MAX_CORRELATION_BYTES)
        {
            return Err(format!(
                "a correlationId is at most {MAX_CORRELATION_BYTES} bytes"
            ));
        }
        if !self.details.is_null() && !self.details.is_object() {
            return Err("details is an object of named values".into());
        }
        if self.details.to_string().len() > MAX_DETAILS_BYTES {
            return Err(format!(
                "details are a few named values of at most {MAX_DETAILS_BYTES} bytes in all"
            ));
        }
        Ok(())
    }
}

/// What a reader asks for. Every parameter is shared by the list and the stream, so a view
/// switches between them without rewriting anything.
#[derive(Debug, Clone, Default)]
pub struct ActivityFilter {
    pub space: Option<String>,
    pub kinds: Vec<String>,
    pub source: Option<String>,
    pub severity: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub object: Option<String>,
    /// Only the calls and changes of one connector, by id (T-5973): `details.connector`.
    pub connector: Option<String>,
    pub limit: i64,
    pub cursor: Option<Cursor>,
    /// The spaces the reader's bindings reach; `None` for every space. An event of no space is a
    /// project's own and reaches every reader (T-4876). Set by the route, never by a parameter.
    pub reach: Option<Vec<String>>,
}

impl ActivityFilter {
    /// Whether one event belongs in this answer. The store's SQL says the same thing; the live
    /// tail runs it over the events as they arrive.
    pub fn matches(&self, event: &ActivityEvent) -> bool {
        if let (Some(reach), Some(space)) = (&self.reach, &event.space) {
            if !reach.contains(space) {
                return false;
            }
        }
        if let Some(space) = &self.space {
            if event.space.as_deref() != Some(space.as_str()) {
                return false;
            }
        }
        if !self.kinds.is_empty() && !self.kinds.iter().any(|kind| kind == &event.kind) {
            return false;
        }
        if let Some(source) = &self.source {
            if &event.source != source {
                return false;
            }
        }
        if let Some(floor) = self.severity.as_deref().and_then(severity_rank) {
            if severity_rank(&event.severity).unwrap_or(0) < floor {
                return false;
            }
        }
        if let Some(since) = self.since {
            if event.time < since {
                return false;
            }
        }
        if let Some(object) = &self.object {
            if event.object() != Some(object.as_str()) {
                return false;
            }
        }
        if let Some(connector) = &self.connector {
            if event.details.get("connector").and_then(Value::as_str) != Some(connector.as_str()) {
                return false;
            }
        }
        true
    }
}

/// Where the next page starts: the time and the row of the last event handed out. Opaque to the
/// caller, because a page boundary is not a contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub time: DateTime<Utc>,
    pub id: i64,
}

impl Cursor {
    pub fn encode(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(
            "{}|{}",
            self.time.timestamp_nanos_opt().unwrap_or(0),
            self.id
        ))
    }

    pub fn decode(text: &str) -> Option<Self> {
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text)
            .ok()?;
        let text = String::from_utf8(raw).ok()?;
        let (nanos, id) = text.split_once('|')?;
        Some(Self {
            time: DateTime::from_timestamp_nanos(nanos.parse().ok()?),
            id: id.parse().ok()?,
        })
    }
}

/// One page of the list, and where the next one starts.
pub struct Page {
    pub items: Vec<ActivityEvent>,
    pub next: Option<String>,
    /// Where each item sits, so a page cut short still names the cursor past its last item.
    keys: Vec<Cursor>,
}

impl Page {
    /// The first items that fit in `bytes` of their JSON, and the cursor past the last one kept
    /// when the cut dropped any (T-6291): an operation's answer stays small enough for a model to
    /// read, and `next` reads on from the cut.
    pub fn within(mut self, bytes: usize) -> Self {
        let kept = crate::ops::bounds::fitting(&self.items, bytes);
        if kept < self.items.len() {
            self.next = kept
                .checked_sub(1)
                .and_then(|last| self.keys.get(last))
                .map(Cursor::encode);
            self.items.truncate(kept);
            self.keys.truncate(kept);
        }
        self
    }
}

/// The live half: what a connected browser is handed as the events arrive, while the store is
/// what a reconnecting one replays from with `since`.
#[derive(Clone, Default)]
pub struct ActivityHub {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<ActivityEvent>>>>,
}

impl ActivityHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn publish(&self, event: &ActivityEvent) {
        let mut map = self.channels.write().await;
        let sender = map
            .entry(event.project.clone())
            .or_insert_with(|| broadcast::channel(256).0);
        let _ = sender.send(event.clone());
    }

    pub async fn subscribe(&self, project: &str) -> broadcast::Receiver<ActivityEvent> {
        let mut map = self.channels.write().await;
        map.entry(project.to_owned())
            .or_insert_with(|| broadcast::channel(256).0)
            .subscribe()
    }
}

/// sqlx is built with the `time` crate, the rest of the Portal speaks `chrono`; the drafts store
/// converts the same way.
pub(crate) fn odt_to_chrono(odt: time::OffsetDateTime) -> DateTime<Utc> {
    DateTime::from_timestamp(odt.unix_timestamp(), odt.nanosecond()).unwrap_or_else(Utc::now)
}

pub(crate) fn chrono_to_odt(dt: DateTime<Utc>) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(dt.timestamp())
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
}

#[derive(Debug, thiserror::Error)]
pub enum ActivityError {
    #[error("database error: {0}")]
    Db(String),
}

enum Inner {
    Db(sqlx::PgPool),
    /// `(id, event)`, newest last. One process, no durability: what a Portal without a database
    /// shows is what happened since it started.
    Memory(RwLock<Vec<(i64, ActivityEvent)>>),
}

#[derive(Clone)]
pub struct ActivityStore {
    inner: Arc<Inner>,
    hub: Option<ActivityHub>,
    /// When this process last started a trim; `None` until its first.
    trimmed_at: Arc<std::sync::Mutex<Option<DateTime<Utc>>>>,
}

impl ActivityStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: Arc::new(match db {
                Some(pool) => Inner::Db(pool),
                None => Inner::Memory(RwLock::new(Vec::new())),
            }),
            hub: None,
            trimmed_at: Arc::default(),
        }
    }

    pub fn with_hub(mut self, hub: ActivityHub) -> Self {
        self.hub = Some(hub);
        self
    }

    /// Records events that have already been validated, and hands each to the live tail. One
    /// malformed record must not cost the other four hundred, so the caller rejects those on
    /// their own before calling this.
    pub async fn append(&self, events: &[ActivityEvent]) -> Result<(), ActivityError> {
        match &*self.inner {
            Inner::Memory(rows) => {
                let mut w = rows.write().await;
                let first = w.last().map(|(id, _)| id + 1).unwrap_or(1);
                for (id, event) in (first..).zip(events.iter()) {
                    w.push((id, event.clone()));
                }
            }
            Inner::Db(pool) => {
                // One transaction: a batch is stored whole or not at all, and the live tail
                // hears only what was stored (F-POR1-022).
                let db = |err: sqlx::Error| ActivityError::Db(err.to_string());
                let mut tx = pool.begin().await.map_err(db)?;
                for event in events {
                    sqlx::query(
                        "INSERT INTO activity (time, project, space, kind, source, summary, severity, correlation_id, details) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                    )
                    .bind(chrono_to_odt(event.time))
                    .bind(&event.project)
                    .bind(&event.space)
                    .bind(&event.kind)
                    .bind(&event.source)
                    .bind(&event.summary)
                    .bind(&event.severity)
                    .bind(&event.correlation_id)
                    .bind(&event.details)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                }
                tx.commit().await.map_err(db)?;
            }
        }
        if let Some(hub) = &self.hub {
            for event in events {
                hub.publish(event).await;
            }
        }
        Ok(())
    }

    /// One page, newest first.
    pub async fn list(
        &self,
        project: &str,
        filter: &ActivityFilter,
    ) -> Result<Page, ActivityError> {
        let limit = filter.limit.clamp(1, MAX_LIMIT);
        match &*self.inner {
            Inner::Memory(rows) => {
                let r = rows.read().await;
                let mut hits: Vec<(i64, ActivityEvent)> =
                    r.iter()
                        .filter(|(id, event)| {
                            event.project == project
                                && filter.matches(event)
                                && filter.cursor.as_ref().is_none_or(|cursor| {
                                    (event.time, *id) < (cursor.time, cursor.id)
                                })
                        })
                        .cloned()
                        .collect();
                hits.sort_by_key(|(id, event)| std::cmp::Reverse((event.time, *id)));
                Ok(Self::page_of(hits, limit))
            }
            Inner::Db(pool) => Self::list_db(pool, project, filter, limit).await,
        }
    }

    /// The page of `rows` (newest first, at most `limit + 1`): the first `limit` and the cursor past
    /// them when there were more.
    fn page_of(mut rows: Vec<(i64, ActivityEvent)>, limit: i64) -> Page {
        let more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        let keys: Vec<Cursor> = rows
            .iter()
            .map(|(id, event)| Cursor {
                time: event.time,
                id: *id,
            })
            .collect();
        let next = more.then(|| keys.last()).flatten().map(Cursor::encode);
        Page {
            items: rows.into_iter().map(|(_, event)| event).collect(),
            next,
            keys,
        }
    }

    /// [`ActivityStore::list`] over Postgres.
    async fn list_db(
        pool: &sqlx::PgPool,
        project: &str,
        filter: &ActivityFilter,
        limit: i64,
    ) -> Result<Page, ActivityError> {
        let mut sql = String::from(
            "SELECT id, time, project, space, kind, source, summary, severity, correlation_id, details \
             FROM activity WHERE project = $1",
        );
        let mut n = 1;
        if filter.space.is_some() {
            n += 1;
            sql.push_str(&format!(" AND space = ${n}"));
        }
        if filter.reach.is_some() {
            n += 1;
            sql.push_str(&format!(" AND (space IS NULL OR space = ANY(${n}))"));
        }
        if !filter.kinds.is_empty() {
            n += 1;
            sql.push_str(&format!(" AND kind = ANY(${n})"));
        }
        if filter.source.is_some() {
            n += 1;
            sql.push_str(&format!(" AND source = ${n}"));
        }
        if filter.severity.is_some() {
            n += 1;
            sql.push_str(&format!(
                " AND CASE severity WHEN 'error' THEN 2 WHEN 'warning' THEN 1 ELSE 0 END >= ${n}"
            ));
        }
        if filter.since.is_some() {
            n += 1;
            sql.push_str(&format!(" AND time >= ${n}"));
        }
        if filter.object.is_some() {
            n += 1;
            sql.push_str(&format!(" AND details->>'object' = ${n}"));
        }
        if filter.connector.is_some() {
            n += 1;
            sql.push_str(&format!(" AND details->>'connector' = ${n}"));
        }
        if filter.cursor.is_some() {
            sql.push_str(&format!(" AND (time, id) < (${}, ${})", n + 1, n + 2));
            n += 2;
        }
        n += 1;
        sql.push_str(&format!(" ORDER BY time DESC, id DESC LIMIT ${n}"));

        // The shape is built from the filter's own fields, never from a value: every value is
        // bound. `AssertSqlSafe` is how sqlx 0.9 takes a query string that is not `'static`.
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(project);
        if let Some(space) = &filter.space {
            query = query.bind(space);
        }
        if let Some(reach) = &filter.reach {
            query = query.bind(reach.clone());
        }
        if !filter.kinds.is_empty() {
            query = query.bind(filter.kinds.clone());
        }
        if let Some(source) = &filter.source {
            query = query.bind(source);
        }
        if let Some(severity) = &filter.severity {
            query = query.bind(severity_rank(severity).unwrap_or(0));
        }
        if let Some(since) = filter.since {
            query = query.bind(chrono_to_odt(since));
        }
        if let Some(object) = &filter.object {
            query = query.bind(object);
        }
        if let Some(connector) = &filter.connector {
            query = query.bind(connector);
        }
        if let Some(cursor) = &filter.cursor {
            query = query.bind(chrono_to_odt(cursor.time)).bind(cursor.id);
        }
        let rows = query
            .bind(limit + 1)
            .fetch_all(pool)
            .await
            .map_err(|err| ActivityError::Db(err.to_string()))?;
        let items: Vec<(i64, ActivityEvent)> = rows
            .iter()
            .map(|row| {
                (
                    row.get::<i64, _>("id"),
                    ActivityEvent {
                        time: odt_to_chrono(row.get::<time::OffsetDateTime, _>("time")),
                        project: row.get("project"),
                        space: row.get("space"),
                        kind: row.get("kind"),
                        source: row.get("source"),
                        summary: row.get("summary"),
                        severity: row.get("severity"),
                        correlation_id: row.get("correlation_id"),
                        details: row
                            .get::<Option<Value>, _>("details")
                            .unwrap_or(Value::Null),
                    },
                )
            })
            .collect();
        Ok(Self::page_of(items, limit))
    }

    /// Drops what is past the retention window (OPS-49). The Portal trims its own table on a
    /// timer ([`Self::trim_if_due`], from the leader's sync); there is no CronJob.
    pub async fn trim(&self, now: DateTime<Utc>) -> Result<Trimmed, ActivityError> {
        let floor = now - Duration::days(RETENTION_DAYS);
        match &*self.inner {
            Inner::Memory(rows) => {
                let mut w = rows.write().await;
                let before = w.len();
                w.retain(|(_, event)| event.time >= floor);
                let rows = (before - w.len()) as u64;
                Ok(Trimmed {
                    rows,
                    batches: u32::from(rows > 0),
                })
            }
            Inner::Db(pool) => {
                let mut done = Trimmed {
                    rows: 0,
                    batches: 0,
                };
                loop {
                    let deleted = sqlx::query(
                        "DELETE FROM activity WHERE ctid IN \
                         (SELECT ctid FROM activity WHERE time < $1 LIMIT $2)",
                    )
                    .bind(chrono_to_odt(floor))
                    .bind(TRIM_BATCH)
                    .execute(pool)
                    .await
                    .map_err(|err| ActivityError::Db(err.to_string()))?
                    .rows_affected();
                    if deleted > 0 {
                        done.rows += deleted;
                        done.batches += 1;
                    }
                    if deleted < TRIM_BATCH as u64 {
                        return Ok(done);
                    }
                }
            }
        }
    }

    /// Claims the hourly trim: true at most once per [`TRIM_EVERY_MINUTES`] for this store.
    fn trim_due(&self, now: DateTime<Utc>) -> bool {
        let mut last = self.trimmed_at.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|at| now - at < Duration::minutes(TRIM_EVERY_MINUTES)) {
            return false;
        }
        *last = Some(now);
        true
    }

    /// The trim when an hour has passed since this process's last one, else `None`. A failed
    /// trim is retried at the next due hour, not at the next sync.
    pub async fn trim_if_due(&self, now: DateTime<Utc>) -> Option<Result<Trimmed, ActivityError>> {
        if !self.trim_due(now) {
            return None;
        }
        Some(self.trim(now).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(kind: &str, severity: &str, minutes: i64) -> ActivityEvent {
        ActivityEvent {
            time: DateTime::from_timestamp(1_789_000_000 + minutes * 60, 0).expect("a time"),
            project: "helsinki".into(),
            space: Some("air-quality".into()),
            kind: kind.into(),
            source: "gateway".into(),
            summary: "Something happened.".into(),
            severity: severity.into(),
            correlation_id: None,
            details: json!({ "object": "endpoints/public-air" }),
        }
    }

    async fn store_with(events: Vec<ActivityEvent>) -> ActivityStore {
        let store = ActivityStore::new(None);
        store.append(&events).await.expect("appended");
        store
    }

    #[tokio::test]
    async fn a_page_is_newest_first_and_its_cursor_starts_the_next_one() {
        let store = store_with(
            (0..5)
                .map(|i| event("access.denied", "warning", i))
                .collect(),
        )
        .await;
        let first = store
            .list(
                "helsinki",
                &ActivityFilter {
                    limit: 2,
                    ..Default::default()
                },
            )
            .await
            .expect("a page");
        assert_eq!(first.items.len(), 2);
        assert!(first.items[0].time > first.items[1].time, "newest first");
        let cursor = Cursor::decode(&first.next.expect("a next page")).expect("a cursor");

        let second = store
            .list(
                "helsinki",
                &ActivityFilter {
                    limit: 2,
                    cursor: Some(cursor),
                    ..Default::default()
                },
            )
            .await
            .expect("a page");
        assert_eq!(second.items.len(), 2);
        assert!(
            second.items[0].time < first.items[1].time,
            "a cursor does not repeat an event it already handed out"
        );
    }

    #[tokio::test]
    async fn a_severity_names_the_floor_and_a_kind_narrows_to_itself() {
        let store = store_with(vec![
            event("access.denied", "info", 1),
            event("access.denied", "warning", 2),
            event("pipeline.error", "error", 3),
        ])
        .await;

        let warnings = store
            .list(
                "helsinki",
                &ActivityFilter {
                    severity: Some("warning".into()),
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .expect("a page");
        assert_eq!(warnings.items.len(), 2, "warning includes error");

        let denials = store
            .list(
                "helsinki",
                &ActivityFilter {
                    kinds: vec!["pipeline.error".into()],
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .expect("a page");
        assert_eq!(denials.items.len(), 1);
        assert_eq!(denials.items[0].kind, "pipeline.error");
    }

    #[tokio::test]
    async fn another_project_s_events_are_not_in_this_project_s_answer() {
        let mut theirs = event("access.denied", "error", 1);
        theirs.project = "espoo".into();
        let store = store_with(vec![event("access.denied", "info", 2), theirs]).await;

        let page = store
            .list(
                "helsinki",
                &ActivityFilter {
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .expect("a page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].project, "helsinki");
    }

    #[tokio::test]
    async fn an_object_page_sees_only_its_own_object() {
        let mut other = event("endpoint.traffic", "info", 4);
        other.details = json!({ "object": "pipelines/aq-ingest" });
        let store = store_with(vec![event("access.denied", "info", 3), other]).await;

        let page = store
            .list(
                "helsinki",
                &ActivityFilter {
                    object: Some("pipelines/aq-ingest".into()),
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .expect("a page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].kind, "endpoint.traffic");
    }

    #[tokio::test]
    async fn a_connector_filter_sees_only_that_connectors_calls() {
        let mut through = event("mcp.tool", "info", 4);
        through.details = json!({ "connector": "abcdefghijklmnopqrstuvwxyz" });
        let mut other = event("mcp.tool", "info", 3);
        other.details = json!({ "connector": "bbcdefghijklmnopqrstuvwxyz" });
        let store = store_with(vec![event("access.denied", "info", 2), other, through]).await;
        let filter = ActivityFilter {
            connector: Some("abcdefghijklmnopqrstuvwxyz".into()),
            limit: 10,
            ..Default::default()
        };
        let page = store.list("helsinki", &filter).await.expect("a page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            page.items[0].details["connector"],
            "abcdefghijklmnopqrstuvwxyz"
        );
        assert!(filter.matches(&page.items[0]));
    }

    #[tokio::test]
    async fn what_is_past_the_retention_window_is_trimmed() {
        let now = Utc::now();
        let mut old = event("config.applied", "info", 0);
        old.time = now - Duration::days(RETENTION_DAYS + 1);
        let mut fresh = event("config.applied", "info", 0);
        fresh.time = now;
        let store = store_with(vec![old, fresh]).await;

        assert_eq!(store.trim(now).await.expect("trimmed").rows, 1);
        let page = store
            .list(
                "helsinki",
                &ActivityFilter {
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .expect("a page");
        assert_eq!(page.items.len(), 1);
    }

    #[tokio::test]
    async fn the_trim_runs_once_an_hour() {
        let now = Utc::now();
        let mut old = event("config.applied", "info", 0);
        old.time = now - Duration::days(RETENTION_DAYS + 1);
        let store = store_with(vec![old.clone()]).await;

        let first = store.trim_if_due(now).await.expect("due").expect("trimmed");
        assert_eq!(first.rows, 1);
        store.append(&[old]).await.expect("appended");
        let soon = now + Duration::minutes(TRIM_EVERY_MINUTES - 1);
        assert!(store.trim_if_due(soon).await.is_none());
        let later = now + Duration::minutes(TRIM_EVERY_MINUTES);
        let second = store
            .trim_if_due(later)
            .await
            .expect("due")
            .expect("trimmed");
        assert_eq!(second.rows, 1);
    }

    #[test]
    fn a_kind_outside_the_vocabulary_is_refused_and_so_is_a_record_with_no_summary() {
        let mut invented = event("access.denied", "info", 0);
        invented.kind = "something.happened".into();
        assert!(invented.validate().is_err());

        let mut silent = event("access.denied", "info", 0);
        silent.summary = "  ".into();
        assert!(silent.validate().is_err());

        assert!(event("access.denied", "info", 0).validate().is_ok());
    }

    /// F-R2POR1-013 (T-6225): a record past its bounds is refused, not stored.
    #[test]
    fn a_record_past_its_bounds_is_refused() {
        let mut long = event("access.denied", "info", 0);
        long.summary = "x".repeat(MAX_SUMMARY_CHARS + 1);
        assert!(long.validate().is_err());
        let mut traced = event("access.denied", "info", 0);
        traced.correlation_id = Some("c".repeat(MAX_CORRELATION_BYTES + 1));
        assert!(traced.validate().is_err());
        let mut payload = event("access.denied", "info", 0);
        payload.details = json!({ "body": "d".repeat(MAX_DETAILS_BYTES) });
        assert!(payload.validate().is_err());
        let mut fits = event("access.denied", "info", 0);
        fits.summary = "x".repeat(MAX_SUMMARY_CHARS);
        fits.correlation_id = Some("0af7651916cd43dd8448eb211c80319c".into());
        assert!(fits.validate().is_ok());
    }

    #[test]
    fn a_cursor_survives_a_round_trip_and_a_broken_one_is_not_a_cursor() {
        let cursor = Cursor {
            time: DateTime::from_timestamp(1_789_000_000, 0).expect("a time"),
            id: 42,
        };
        assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
        assert_eq!(Cursor::decode("not-a-cursor"), None);
    }

    #[tokio::test]
    async fn the_live_tail_is_per_project() {
        let hub = ActivityHub::new();
        let mut mine = hub.subscribe("helsinki").await;
        let mut theirs = hub.subscribe("espoo").await;
        hub.publish(&event("access.denied", "warning", 1)).await;

        assert_eq!(mine.recv().await.expect("an event").project, "helsinki");
        assert!(
            theirs.try_recv().is_err(),
            "espoo hears nothing of helsinki"
        );
    }
}
