//! Draft storage and real-time synchronization hub (AG-61, UI-47).
//!
//! A draft (project, kind, name) tracks the candidate manifest being edited, its latest
//! Verdict, who touched it last, and an optimistic concurrency version.
//!
//! Two storage arms:
//! - `Db(PgPool)`: durable PostgreSQL table backed by migration `0006_drafts.sql`.
//! - `Memory`: in-memory fallback when running without a database.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use tokio::sync::{broadcast, RwLock};
use utoipa::ToSchema;

use super::bounds;
use super::parse_input;
use super::verdict_schema;
use super::workspaces;
use super::Caller;
use super::OpError;
use super::Operation;
use super::OperationAnnotations;
use crate::activity::{chrono_to_odt, odt_to_chrono};
use crate::change::Lane;
use crate::error::ApiError;
use crate::ops::verdict::Verdict;
use crate::state::AppState;
use jc_core::kinds::Verb;
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub project: String,
    pub kind: String,
    pub name: String,
    /// The workspace the draft belongs to, or none (CC-76): a draft of one workspace is not
    /// a draft of another, nor of the main project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub manifest: Value,
    pub verdict: Option<Verdict>,
    pub touched_by: String,
    pub touched_kind: String,
    pub version: i64,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
}

/// One line of a draft list (AG-61, API/01 §Drafts): what a caller needs to pick one, and never
/// the manifest or the verdict's trace. A project with 44 drafts answered 2.3 MB of manifests and
/// traces, which is more than one model call can carry, so the assistant answered nothing at all
/// while they existed (T-2248). A caller that needs the manifest reads it with `jc_draft_get`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftLine {
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub touched_by: String,
    pub touched_kind: String,
    pub version: i64,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
    /// The draft's own check, as a line: whether it passed and how much it found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<VerdictLine>,
}

/// A verdict as a line: the answer and the size of the report behind it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VerdictLine {
    pub ok: bool,
    pub findings: usize,
    #[schema(value_type = String, format = DateTime)]
    pub checked_at: DateTime<Utc>,
}

impl From<&Draft> for DraftLine {
    fn from(draft: &Draft) -> Self {
        Self {
            kind: draft.kind.clone(),
            name: draft.name.clone(),
            workspace: draft.workspace.clone(),
            touched_by: draft.touched_by.clone(),
            touched_kind: draft.touched_kind.clone(),
            version: draft.version,
            updated_at: draft.updated_at,
            verdict: draft.verdict.as_ref().map(|verdict| VerdictLine {
                ok: verdict.ok,
                findings: verdict.findings.len(),
                checked_at: verdict.checked_at,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftEvent {
    pub project: String,
    pub kind: String,
    pub name: String,
    pub version: i64,
    pub touched_by: String,
    pub touched_kind: String,
    pub event: String,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
    /// The draft's context space, so a stream filters what it forwards by the reader's binding
    /// (PF-59, T-1455). In-process only: never sent.
    #[serde(skip)]
    pub space: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("draft conflict: current version is {current}")]
    Conflict { current: i64 },
    #[error("literal secret in field '{0}' is forbidden; use secretRef instead (MF-24)")]
    Secret(String),
    #[error("database error: {0}")]
    Db(String),
    #[error("draft '{kind}/{name}' not found in project '{project}'")]
    NotFound {
        project: String,
        kind: String,
        name: String,
    },
}

/// The PostgreSQL channel every replica's draft events cross (OPS-51, Architecture/09 §10).
pub const DRAFT_EVENTS_CHANNEL: &str = "jc_draft_events";

/// Who is typing in which draft, fanned out per project: to this process's streams, and, when the
/// drafts live in PostgreSQL, to every other replica through [`DRAFT_EVENTS_CHANNEL`].
#[derive(Clone)]
pub struct DraftHub {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<DraftEvent>>>>,
    /// This replica's name on the channel, so its own notifications are not delivered twice.
    origin: Arc<str>,
    /// Set once by [`DraftHub::connect`]; shared by every clone of the hub.
    database: Arc<std::sync::OnceLock<sqlx::PgPool>>,
}

/// One event on the channel: the event's key and metadata with the space the streams filter on,
/// and the replica that sent it. Never the manifest.
#[derive(Serialize, Deserialize)]
struct Notice {
    origin: String,
    event: DraftEvent,
    space: Option<String>,
}

impl Default for DraftHub {
    fn default() -> Self {
        Self::new()
    }
}

impl DraftHub {
    pub fn new() -> Self {
        use argon2::password_hash::rand_core::{OsRng, RngCore};
        let mut bytes = [0u8; 12];
        OsRng.fill_bytes(&mut bytes);
        let origin: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        Self {
            channels: Arc::default(),
            origin: Arc::from(origin),
            database: Arc::default(),
        }
    }

    /// Joins the replicas sharing this database: every event is also a `NOTIFY`, and a listener
    /// forwards the other replicas' events to this process's streams. A second call is a no-op.
    pub fn connect(&self, pool: sqlx::PgPool) {
        if self.database.set(pool.clone()).is_err() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("no async runtime: draft events stay inside this process");
            return;
        };
        let hub = self.clone();
        runtime.spawn(async move { hub.listen(pool).await });
    }

    pub async fn broadcast(&self, event: &DraftEvent) {
        self.deliver(event.clone()).await;
        let Some(pool) = self.database.get() else {
            return;
        };
        let notice = Notice {
            origin: self.origin.to_string(),
            event: event.clone(),
            space: event.space.clone(),
        };
        let Ok(payload) = serde_json::to_string(&notice) else {
            return;
        };
        // This process's streams have it already; a failed NOTIFY only costs the other replicas
        // this one event, and the reader refetches the draft on its next open anyway.
        if let Err(error) = sqlx::query("SELECT pg_notify($1, $2)")
            .bind(DRAFT_EVENTS_CHANNEL)
            .bind(payload)
            .execute(pool)
            .await
        {
            tracing::warn!(%error, project = %event.project, "a draft event did not reach the other replicas");
        }
    }

    pub async fn subscribe(&self, project: &str) -> broadcast::Receiver<DraftEvent> {
        let mut map = self.channels.write().await;
        let sender = map.entry(project.to_string()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        });
        sender.subscribe()
    }

    async fn deliver(&self, event: DraftEvent) {
        let mut map = self.channels.write().await;
        let sender = map.entry(event.project.clone()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        });
        let _ = sender.send(event);
    }

    /// Another replica's notice, as this process's event; `None` for this replica's own and for
    /// anything that is not a notice.
    fn received(&self, payload: &str) -> Option<DraftEvent> {
        let notice: Notice = serde_json::from_str(payload).ok()?;
        if *notice.origin == *self.origin {
            return None;
        }
        let mut event = notice.event;
        event.space = notice.space;
        Some(event)
    }

    /// Listens for as long as the process runs, reconnecting after a lost connection. Events sent
    /// while it reconnects are missed, as they would be by a stream that reconnects.
    async fn listen(self, pool: sqlx::PgPool) {
        loop {
            match sqlx::postgres::PgListener::connect_with(&pool).await {
                Ok(mut listener) => match listener.listen(DRAFT_EVENTS_CHANNEL).await {
                    Ok(()) => loop {
                        match listener.recv().await {
                            Ok(notification) => {
                                if let Some(event) = self.received(notification.payload()) {
                                    self.deliver(event).await;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, "the draft event listener lost its connection");
                                break;
                            }
                        }
                    },
                    Err(error) => tracing::warn!(%error, "could not listen for draft events"),
                },
                Err(error) => tracing::warn!(%error, "could not connect the draft event listener"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

enum DraftStoreInner {
    Db(sqlx::PgPool),
    Memory(RwLock<HashMap<(String, String, String, String), Draft>>),
}

#[derive(Clone)]
pub struct DraftStore {
    inner: Arc<DraftStoreInner>,
    hub: Option<DraftHub>,
}

/// A draft's key: its workspace (empty for none), project, kind and name.
fn key(workspace: &str, project: &str, kind: &str, name: &str) -> (String, String, String, String) {
    (
        workspace.to_owned(),
        project.to_owned(),
        kind.to_owned(),
        name.to_owned(),
    )
}

impl DraftStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        let inner = match db {
            Some(pool) => DraftStoreInner::Db(pool),
            None => DraftStoreInner::Memory(RwLock::new(HashMap::new())),
        };
        Self {
            inner: Arc::new(inner),
            hub: None,
        }
    }

    pub fn with_hub(mut self, hub: DraftHub) -> Self {
        self.hub = Some(hub);
        self
    }

    pub async fn get(
        &self,
        project: &str,
        kind: &str,
        name: &str,
    ) -> Result<Option<Draft>, DraftError> {
        self.get_in(None, project, kind, name).await
    }

    /// The draft of `kind/name` in `workspace`, or outside every workspace for `None` (CC-76).
    pub async fn get_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
    ) -> Result<Option<Draft>, DraftError> {
        let ws = workspace.unwrap_or_default();
        match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let r = map.read().await;
                Ok(r.get(&key(ws, project, kind, name)).cloned())
            }
            DraftStoreInner::Db(pool) => {
                let row = sqlx::query(
                    "SELECT project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at \
                     FROM drafts WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $4",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(ws)
                .fetch_optional(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                row.map(row_to_draft).transpose()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn put(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        manifest: Value,
        expected_version: Option<i64>,
        touched_by: &str,
        touched_kind: &str,
    ) -> Result<Draft, DraftError> {
        self.put_in(
            None,
            project,
            kind,
            name,
            manifest,
            expected_version,
            touched_by,
            touched_kind,
        )
        .await
    }

    /// [`DraftStore::put`] into `workspace` (CC-76).
    #[allow(clippy::too_many_arguments)]
    pub async fn put_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
        manifest: Value,
        expected_version: Option<i64>,
        touched_by: &str,
        touched_kind: &str,
    ) -> Result<Draft, DraftError> {
        let ws = workspace.unwrap_or_default();
        if let Some(secret_field) = crate::api::mutate::find_literal_secret(&manifest) {
            return Err(DraftError::Secret(secret_field));
        }

        let _ = self.sweep(Duration::from_secs(24 * 3600)).await;

        let key = key(ws, project, kind, name);
        let draft = match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                let (new_version, verdict) = if let Some(existing) = w.get(&key) {
                    if let Some(exp) = expected_version {
                        if exp != existing.version {
                            return Err(DraftError::Conflict {
                                current: existing.version,
                            });
                        }
                    }
                    (existing.version + 1, existing.verdict.clone())
                } else {
                    if let Some(exp) = expected_version {
                        if exp != 0 {
                            return Err(DraftError::Conflict { current: 0 });
                        }
                    }
                    (1, None)
                };

                let draft = Draft {
                    project: project.to_string(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                    workspace: workspace.map(str::to_owned),
                    manifest,
                    verdict,
                    touched_by: touched_by.to_string(),
                    touched_kind: touched_kind.to_string(),
                    version: new_version,
                    updated_at: Utc::now(),
                };
                w.insert(key, draft.clone());
                draft
            }
            DraftStoreInner::Db(pool) => {
                let mut tx = pool
                    .begin()
                    .await
                    .map_err(|e| DraftError::Db(e.to_string()))?;
                let existing = sqlx::query(
                    "SELECT version FROM drafts WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $4 FOR UPDATE",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(ws)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                let row = match existing {
                    Some(r) => {
                        let current: i64 = r.get("version");
                        if let Some(exp) = expected_version {
                            if exp != current {
                                return Err(DraftError::Conflict { current });
                            }
                        }
                        sqlx::query(
                            "UPDATE drafts SET manifest = $4, touched_by = $5, touched_kind = $6, \
                             version = version + 1, updated_at = now() \
                             WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $7 \
                             RETURNING project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at",
                        )
                        .bind(project)
                        .bind(kind)
                        .bind(name)
                        .bind(&manifest)
                        .bind(touched_by)
                        .bind(touched_kind)
                        .bind(ws)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| DraftError::Db(e.to_string()))?
                    }
                    None => {
                        if let Some(exp) = expected_version {
                            if exp != 0 {
                                return Err(DraftError::Conflict { current: 0 });
                            }
                        }
                        sqlx::query(
                            "INSERT INTO drafts (project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at) \
                             VALUES ($1, $2, $3, $7, $4, NULL, $5, $6, 1, now()) \
                             RETURNING project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at",
                        )
                        .bind(project)
                        .bind(kind)
                        .bind(name)
                        .bind(&manifest)
                        .bind(touched_by)
                        .bind(touched_kind)
                        .bind(ws)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| DraftError::Db(e.to_string()))?
                    }
                };

                tx.commit()
                    .await
                    .map_err(|e| DraftError::Db(e.to_string()))?;
                row_to_draft(row)?
            }
        };

        if let Some(hub) = &self.hub {
            hub.broadcast(&DraftEvent {
                project: draft.project.clone(),
                kind: draft.kind.clone(),
                name: draft.name.clone(),
                version: draft.version,
                touched_by: draft.touched_by.clone(),
                touched_kind: draft.touched_kind.clone(),
                event: "put".to_string(),
                updated_at: draft.updated_at,
                space: crate::permissions::space_ref(&draft.manifest),
            })
            .await;
        }

        Ok(draft)
    }

    pub async fn set_verdict(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        verdict: Verdict,
    ) -> Result<Draft, DraftError> {
        self.set_verdict_in(None, project, kind, name, verdict)
            .await
    }

    /// [`DraftStore::set_verdict`] on the draft of `workspace` (CC-76).
    pub async fn set_verdict_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
        verdict: Verdict,
    ) -> Result<Draft, DraftError> {
        let ws = workspace.unwrap_or_default();
        let key = key(ws, project, kind, name);
        let draft = match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                let draft = w.get_mut(&key).ok_or_else(|| DraftError::NotFound {
                    project: project.to_string(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                })?;
                draft.verdict = Some(verdict.keeping_records(draft.verdict.as_ref()));
                draft.updated_at = Utc::now();
                draft.clone()
            }
            DraftStoreInner::Db(pool) => {
                let verdict_json =
                    serde_json::to_value(&verdict).map_err(|e| DraftError::Db(e.to_string()))?;
                let row = sqlx::query(
                    // A verdict without a record check keeps the one the draft held (T-4840),
                    // as `Verdict::keeping_records` does in memory.
                    "UPDATE drafts SET verdict = CASE \
                       WHEN $4::jsonb ? 'records' OR verdict IS NULL OR NOT (verdict ? 'records') \
                       THEN $4::jsonb \
                       ELSE $4::jsonb || jsonb_build_object('records', verdict->'records') END, \
                     updated_at = now() \
                     WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $5 \
                     RETURNING project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(&verdict_json)
                .bind(ws)
                .fetch_optional(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?
                .ok_or_else(|| DraftError::NotFound {
                    project: project.to_string(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                })?;

                row_to_draft(row)?
            }
        };

        if let Some(hub) = &self.hub {
            hub.broadcast(&DraftEvent {
                project: draft.project.clone(),
                kind: draft.kind.clone(),
                name: draft.name.clone(),
                version: draft.version,
                touched_by: draft.touched_by.clone(),
                touched_kind: draft.touched_kind.clone(),
                event: "verdict".to_string(),
                updated_at: draft.updated_at,
                space: crate::permissions::space_ref(&draft.manifest),
            })
            .await;
        }

        Ok(draft)
    }

    pub async fn list(&self, project: &str) -> Result<Vec<Draft>, DraftError> {
        self.list_in(None, project).await
    }

    /// The drafts of `project` in `workspace`, or outside every workspace for `None` (CC-76).
    pub async fn list_in(
        &self,
        workspace: Option<&str>,
        project: &str,
    ) -> Result<Vec<Draft>, DraftError> {
        let ws = workspace.unwrap_or_default();
        match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let r = map.read().await;
                let mut drafts: Vec<Draft> = r
                    .iter()
                    .filter(|((key_ws, ..), d)| d.project == project && key_ws == ws)
                    .map(|(_, d)| d)
                    .cloned()
                    .collect();
                drafts.sort_by_key(|d| std::cmp::Reverse(d.updated_at));
                Ok(drafts)
            }
            DraftStoreInner::Db(pool) => {
                let rows = sqlx::query(
                    "SELECT project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at \
                     FROM drafts WHERE project = $1 AND workspace = $2 ORDER BY updated_at DESC",
                )
                .bind(project)
                .bind(ws)
                .fetch_all(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                rows.into_iter().map(row_to_draft).collect()
            }
        }
    }

    pub async fn drop(&self, project: &str, kind: &str, name: &str) -> Result<bool, DraftError> {
        self.drop_in(None, project, kind, name).await
    }

    /// [`DraftStore::drop`] of the draft of `workspace` (CC-76).
    pub async fn drop_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
    ) -> Result<bool, DraftError> {
        let ws = workspace.unwrap_or_default();
        let key = key(ws, project, kind, name);
        let (dropped, dropped_event) = match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                if let Some(removed) = w.remove(&key) {
                    (
                        true,
                        Some(DraftEvent {
                            space: crate::permissions::space_ref(&removed.manifest),
                            project: removed.project,
                            kind: removed.kind,
                            name: removed.name,
                            version: removed.version,
                            touched_by: removed.touched_by,
                            touched_kind: removed.touched_kind,
                            event: "drop".to_string(),
                            updated_at: Utc::now(),
                        }),
                    )
                } else {
                    (false, None)
                }
            }
            DraftStoreInner::Db(pool) => {
                let row = sqlx::query(
                    "DELETE FROM drafts WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $4 \
                     RETURNING project, kind, name, version, touched_by, touched_kind, updated_at, \
                     manifest",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(ws)
                .fetch_optional(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                if let Some(r) = row {
                    let version: i64 = r.get("version");
                    let touched_by: String = r.get("touched_by");
                    let touched_kind: String = r.get("touched_kind");
                    let odt: time::OffsetDateTime = r.get("updated_at");
                    let manifest: Value = r.get("manifest");
                    (
                        true,
                        Some(DraftEvent {
                            space: crate::permissions::space_ref(&manifest),
                            project: project.to_string(),
                            kind: kind.to_string(),
                            name: name.to_string(),
                            version,
                            touched_by,
                            touched_kind,
                            event: "drop".to_string(),
                            updated_at: odt_to_chrono(odt),
                        }),
                    )
                } else {
                    (false, None)
                }
            }
        };

        if let (Some(hub), Some(ev)) = (&self.hub, dropped_event) {
            hub.broadcast(&ev).await;
        }

        Ok(dropped)
    }

    pub async fn sweep(&self, idle: Duration) -> Result<(), DraftError> {
        let cutoff = Utc::now() - idle;
        match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                w.retain(|_, d| d.updated_at >= cutoff);
                Ok(())
            }
            DraftStoreInner::Db(pool) => {
                let cutoff_odt = chrono_to_odt(cutoff);
                sqlx::query("DELETE FROM drafts WHERE updated_at < $1")
                    .bind(cutoff_odt)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|e| DraftError::Db(e.to_string()))
            }
        }
    }
}

fn row_to_draft(row: sqlx::postgres::PgRow) -> Result<Draft, DraftError> {
    let project: String = row.get("project");
    let kind: String = row.get("kind");
    let name: String = row.get("name");
    let workspace: String = row.get("workspace");
    let manifest: Value = row.get("manifest");
    let verdict_val: Option<Value> = row.get("verdict");
    let verdict = verdict_val.and_then(|v| serde_json::from_value(v).ok());
    let touched_by: String = row.get("touched_by");
    let touched_kind: String = row.get("touched_kind");
    let version: i64 = row.get("version");
    let updated_at_odt: time::OffsetDateTime = row.get("updated_at");

    Ok(Draft {
        project,
        kind,
        name,
        workspace: (!workspace.is_empty()).then_some(workspace),
        manifest,
        verdict,
        touched_by,
        touched_kind,
        version,
        updated_at: odt_to_chrono(updated_at_odt),
    })
}

// ---------------------------------------------------------------------------------------------
// The registry's operations on drafts (AG-61).
// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftRef {
    pub kind: String,
    pub name: String,
}

/// A draft of a workspace is written and dropped by the workspace's owner alone, as every other
/// write into it is (API/01 §22, CC-76, T-2482): the workspace name is not a key anybody may
/// pick. `None` is the project's own draft, which the kind's `propose` already decides.
async fn draft_workspace_owned(
    state: &AppState,
    caller: &Caller,
    project: &str,
    workspace: Option<&str>,
    action: &str,
) -> Result<(), OpError> {
    if let Some(name) = workspace {
        workspaces::owned(state, &caller.identity, project, name, action).await?;
    }
    Ok(())
}

/// A draft of a workspace is read where the workspace is (PF-59, T-2482): a copy the caller may
/// not see, or a name that is no live workspace, answers the workspace's own 404.
async fn draft_workspace_visible(
    state: &AppState,
    caller: &Caller,
    project: &str,
    workspace: Option<&str>,
) -> Result<(), OpError> {
    if let Some(name) = workspace {
        workspaces::visible(state, &caller.identity, project, name).await?;
    }
    Ok(())
}

/// The saved draft `draft` of `project` when `identity` reads its manifest; a draft of a space
/// the caller is not bound to answers the same 404 as one that does not exist (PF-59, R20,
/// T-5698). What jc_draft_get filters with, for every door that runs a saved draft.
pub(crate) async fn readable_draft(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    draft: &DraftRef,
) -> Result<Draft, OpError> {
    let effective = crate::permissions::for_request(state, identity, project);
    state
        .drafts
        .get(project, &draft.kind, &draft.name)
        .await
        .map_err(draft_error)?
        .filter(|saved| effective.may_read_manifest(&saved.kind, &saved.manifest))
        .ok_or_else(|| {
            OpError::Api(ApiError::NotFound(format!(
                "draft '{}/{}' not found in project '{project}'",
                draft.kind, draft.name
            )))
        })
}

pub(crate) fn draft_error(err: DraftError) -> OpError {
    match err {
        DraftError::Conflict { current } => OpError::Conflict(json!({
            "type": "https://joinedcontext.com/errors/draft-conflict",
            "error": "draft_conflict",
            "current": current,
        })),
        DraftError::Secret(path) => OpError::Api(ApiError::BadRequest(format!(
            "literal secret in field '{path}' is forbidden; use secretRef instead (MF-24)"
        ))),
        DraftError::NotFound {
            project,
            kind,
            name,
        } => OpError::Api(ApiError::NotFound(format!(
            "draft '{kind}/{name}' not found in project '{project}'"
        ))),
        DraftError::Db(msg) => OpError::Api(ApiError::Internal(msg)),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftPutInput {
    pub kind: String,
    pub name: String,
    pub manifest: Value,
    #[serde(default)]
    pub expected_version: Option<i64>,
    /// The copy this draft belongs to; `None` is the project's own draft (CC-76, T-2267).
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftGetInput {
    pub kind: String,
    pub name: String,
    /// The copy to read it in; `None` is the project's own draft (CC-76, T-2267).
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftDropInput {
    pub kind: String,
    pub name: String,
    /// The copy to drop it in; `None` is the project's own draft (CC-76, T-2267).
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Which drafts to list: those of one copy, or the project's own (CC-76, T-2267).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftListInput {
    #[serde(default)]
    pub workspace: Option<String>,
}

fn draft_put_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "kind": text("Draft kind, e.g. Endpoint", TERM),
            "name": text("Draft metadata.name", NAME),
            "manifest": manifest("The whole manifest to save as the draft"),
            "expectedVersion": {
                "type": "integer",
                "description": "The version last read; a newer one refuses the save",
                "minimum": 0
            },
            "workspace": workspace()
        },
        "required": ["kind", "name", "manifest"],
        "additionalProperties": false
    })
}

fn draft_get_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "kind": text("Draft kind, e.g. Endpoint", TERM),
            "name": text("Draft metadata.name", NAME),
            "workspace": workspace()
        },
        "required": ["kind", "name"],
        "additionalProperties": false
    })
}

fn draft_list_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "workspace": bounds::workspace()
        },
        "additionalProperties": false
    })
}

/// One draft as the operations answer it (T-0838: no `$ref` a client cannot resolve).
fn draft_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": { "type": "string" },
            "kind": { "type": "string" },
            "name": { "type": "string" },
            "manifest": { "type": "object" },
            "verdict": verdict_schema(),
            "touchedBy": { "type": "string" },
            "touchedKind": { "type": "string", "description": "person, mcp or run" },
            "version": { "type": "integer" },
            "updatedAt": { "type": "string", "format": "date-time" }
        },
        "required": ["project", "kind", "name", "manifest", "version"]
    })
}

fn draft_list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "description": "A line per draft; the manifest is read with jc_draft_get (T-2248)",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "name": { "type": "string" },
                        "workspace": { "type": "string" },
                        "touchedBy": { "type": "string" },
                        "touchedKind": { "type": "string" },
                        "version": { "type": "integer" },
                        "updatedAt": { "type": "string", "format": "date-time" },
                        "verdict": {
                            "type": "object",
                            "properties": {
                                "ok": { "type": "boolean" },
                                "findings": { "type": "integer" },
                                "checkedAt": { "type": "string", "format": "date-time" }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn draft_drop_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "dropped": { "type": "boolean" }
        }
    })
}

/// This module's operations in the registry (`super::init_registry`).
pub fn operations() -> Vec<Operation> {
    vec![        Operation {
            name: "jc_draft_put",
            title: "Put Draft",
            description: "Writes the shared draft of a manifest every window, assistant run and MCP client sees (AG-61)",
            input: draft_put_input_schema,
            output: draft_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<DraftPutInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let mut input: DraftPutInput = parse_input(val)?;
                    // A draft holds a slug only this platform minted, or the Portal form did:
                    // a proposal of the draft takes it as it is (EP-75).
                    crate::endpoint_slug::settle_in(
                        state,
                        project,
                        input.workspace.as_deref(),
                        caller.brings_its_own_slug(),
                        &mut input.manifest,
                    )
                    .await
                    .map_err(|message| OpError::InvalidInput {
                        path: format!("/manifest{}", crate::endpoint_slug::PATH),
                        message,
                    })?;
                    // A draft of one kind holding another's manifest would be judged by the
                    // wrong kind's grants; the manifest itself is judged so a space-bound grant
                    // admits it only in its own space (F-POR3-019).
                    if let Some(written) = input.manifest.get("kind").and_then(Value::as_str) {
                        if written != input.kind {
                            return Err(OpError::InvalidInput {
                                path: "manifest.kind".into(),
                                message: format!(
                                    "the manifest is a {written} and the draft a {}; draft it as {written}",
                                    input.kind
                                ),
                            });
                        }
                    }
                    crate::permissions::for_request(state, &caller.identity, project)
                        .check(&input.kind, Verb::Propose, Some(&input.manifest))?;
                    draft_workspace_owned(state, caller, project, input.workspace.as_deref(), "write a draft into").await?;
                    let draft = state.drafts
                        .put_in(
                            input.workspace.as_deref(),
                            project,
                            &input.kind,
                            &input.name,
                            input.manifest,
                            input.expected_version,
                            &caller.identity.username,
                            caller.via.touched_kind(),
                        )
                        .await
                        .map_err(draft_error)?;
                    Ok(serde_json::to_value(draft)?)
                })
            },
        },        Operation {
            name: "jc_draft_get",
            title: "Get Draft",
            description: "Reads one shared draft with its verdict (AG-61)",
            input: draft_get_input_schema,
            output: draft_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<DraftGetInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let d: DraftGetInput = parse_input(val)?;
                    draft_workspace_visible(state, caller, project, d.workspace.as_deref()).await?;
                    let effective = crate::permissions::for_request(state, &caller.identity, project);
                    let draft = state.drafts
                        .get_in(d.workspace.as_deref(), project, &d.kind, &d.name)
                        .await
                        .map_err(draft_error)?
                        // Not readable is not there (PF-59, R20, T-1455).
                        .filter(|draft| effective.may_read_manifest(&draft.kind, &draft.manifest))
                        .ok_or_else(|| {
                            ApiError::NotFound(format!(
                                "draft '{}/{}' not found in project '{project}'",
                                d.kind, d.name
                            ))
                        })?;
                    Ok(serde_json::to_value(draft)?)
                })
            },
        },        Operation {
            name: "jc_draft_list",
            title: "List Drafts",
            description: "Lists the shared drafts of a project, or of one copy of it (AG-61, CC-76)",
            input: draft_list_input_schema,
            output: draft_list_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<DraftListInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let asked: DraftListInput = parse_input(val)?;
                    draft_workspace_visible(state, caller, project, asked.workspace.as_deref()).await?;
                    // A draft is readable exactly where its manifest would be (PF-59, T-1455).
                    let effective = crate::permissions::for_request(state, &caller.identity, project);
                    // A line per draft, never the manifest and never the verdict's trace
                    // (T-2248): the manifest is read one at a time with jc_draft_get.
                    let items: Vec<crate::ops::drafts::DraftLine> = state.drafts
                        .list_in(asked.workspace.as_deref(), project)
                        .await
                        .map_err(draft_error)?
                        .iter()
                        .filter(|draft| effective.may_read_manifest(&draft.kind, &draft.manifest))
                        // A run sees only the kinds its profile reads (T-6366).
                        .filter(|draft| caller.reads_kind(&draft.kind))
                        .map(crate::ops::drafts::DraftLine::from)
                        .collect();
                    Ok(json!({ "items": items }))
                })
            },
        },        Operation {
            name: "jc_draft_drop",
            title: "Drop Draft",
            description: "Discards a shared draft (AG-61)",
            input: draft_get_input_schema,
            output: draft_drop_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<DraftDropInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let d: DraftDropInput = parse_input(val)?;
                    // Judged by the stored draft, so a grant bound to one space drops only that
                    // space's drafts (F-POR3-019).
                    let stored = state.drafts
                        .get_in(d.workspace.as_deref(), project, &d.kind, &d.name)
                        .await
                        .map_err(draft_error)?;
                    crate::permissions::for_request(state, &caller.identity, project).check(
                        &d.kind,
                        Verb::Propose,
                        stored.as_ref().map(|draft| &draft.manifest),
                    )?;
                    draft_workspace_owned(state, caller, project, d.workspace.as_deref(), "drop a draft of").await?;
                    let dropped = state.drafts
                        .drop_in(d.workspace.as_deref(), project, &d.kind, &d.name)
                        .await
                        .map_err(draft_error)?;
                    Ok(json!({ "dropped": dropped }))
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn memory_store_lifecycle() {
        let store = DraftStore::new(None);
        let manifest = json!({ "kind": "DataSource", "metadata": { "name": "feed-1" } });

        // 1. Initial put creates version 1
        let d1 = store
            .put(
                "helsinki",
                "DataSource",
                "feed-1",
                manifest.clone(),
                None,
                "steward",
                "person",
            )
            .await
            .unwrap();
        assert_eq!(d1.version, 1);
        assert_eq!(d1.touched_by, "steward");
        assert_eq!(d1.touched_kind, "person");
        assert!(d1.verdict.is_none());

        // 2. Put with matching expected_version bumps version
        let d2 = store
            .put(
                "helsinki",
                "DataSource",
                "feed-1",
                manifest.clone(),
                Some(1),
                "assistant",
                "assistant",
            )
            .await
            .unwrap();
        assert_eq!(d2.version, 2);
        assert_eq!(d2.touched_by, "assistant");

        // 3. Put with conflicting expected_version fails
        let conflict = store
            .put(
                "helsinki",
                "DataSource",
                "feed-1",
                manifest.clone(),
                Some(1),
                "steward",
                "person",
            )
            .await
            .unwrap_err();
        match conflict {
            DraftError::Conflict { current } => assert_eq!(current, 2),
            other => panic!("expected conflict, got {other:?}"),
        }

        // 4. Literal secret is refused
        let secret_manifest =
            json!({ "kind": "DataSource", "spec": { "http": { "password": "plain" } } });
        let secret_err = store
            .put(
                "helsinki",
                "DataSource",
                "feed-sec",
                secret_manifest,
                None,
                "steward",
                "person",
            )
            .await
            .unwrap_err();
        assert!(matches!(secret_err, DraftError::Secret(_)));

        // 5. Set verdict attaches verdict without bumping version
        let verdict = Verdict::green(&manifest, None);
        let d_v = store
            .set_verdict("helsinki", "DataSource", "feed-1", verdict.clone())
            .await
            .unwrap();
        assert_eq!(d_v.version, 2);
        assert!(d_v.verdict.is_some());

        // 6. List and get
        let got = store
            .get("helsinki", "DataSource", "feed-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.version, 2);
        assert_eq!(got.verdict, Some(verdict));

        let items = store.list("helsinki").await.unwrap();
        assert_eq!(items.len(), 1);

        // 7. Drop
        let dropped = store
            .drop("helsinki", "DataSource", "feed-1")
            .await
            .unwrap();
        assert!(dropped);
        assert!(store
            .get("helsinki", "DataSource", "feed-1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn draft_hub_broadcasts_events() {
        let hub = DraftHub::new();
        let mut sub = hub.subscribe("helsinki").await;
        let store = DraftStore::new(None).with_hub(hub);

        let manifest = json!({ "kind": "Endpoint", "metadata": { "name": "ep-1" } });
        let _ = store
            .put(
                "helsinki",
                "Endpoint",
                "ep-1",
                manifest.clone(),
                None,
                "steward",
                "person",
            )
            .await
            .unwrap();

        let ev = sub.recv().await.unwrap();
        assert_eq!(ev.project, "helsinki");
        assert_eq!(ev.kind, "Endpoint");
        assert_eq!(ev.name, "ep-1");
        assert_eq!(ev.event, "put");
        assert_eq!(ev.version, 1);
    }

    /// OPS-51: another replica's notice becomes this process's event, space included, so the
    /// stream still filters it by the reader's binding; this replica's own notice and anything
    /// that is not a notice are dropped.
    #[test]
    fn a_notice_from_another_replica_is_delivered_and_its_own_is_not() {
        let here = DraftHub::new();
        let there = DraftHub::new();
        assert_ne!(here.origin, there.origin, "two replicas never share a name");

        let event = DraftEvent {
            project: "helsinki".into(),
            kind: "Endpoint".into(),
            name: "ep-1".into(),
            version: 3,
            touched_by: "steward".into(),
            touched_kind: "person".into(),
            event: "put".into(),
            updated_at: chrono::Utc::now(),
            space: Some("mobility".into()),
        };
        let notice = |origin: &str| {
            serde_json::to_string(&Notice {
                origin: origin.to_owned(),
                event: event.clone(),
                space: event.space.clone(),
            })
            .expect("a notice")
        };

        let received = here
            .received(&notice(&there.origin))
            .expect("another replica's event");
        assert_eq!(received.name, "ep-1");
        assert_eq!(received.version, 3);
        assert_eq!(received.space.as_deref(), Some("mobility"));
        assert!(
            here.received(&notice(&here.origin)).is_none(),
            "its own is already delivered"
        );
        assert!(here.received("not json").is_none());
        assert!(here.received(r#"{"origin":"x"}"#).is_none());
        // The manifest never rides on the channel.
        assert!(!notice(&there.origin).contains("manifest"));
    }
}
