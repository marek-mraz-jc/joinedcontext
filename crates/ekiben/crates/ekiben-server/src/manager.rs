use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ekiben_core::config::merge_json_patch;
use ekiben_core::{
    interpolate_env_str, redact_config_value, CoreError, EnvAccess, EnvLintError, MetricsRegistry,
    OrgId, Registry, ResourceManager, RunMode, RunSummary, StreamConfig, StreamMetrics,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::RwLock;

use crate::runner::{spawn_stream, RunningStreamHandle};

/// Status of a managed stream in streams mode (`internal/stream/manager/type.go:StreamStatus`).
pub struct ManagedStream {
    pub id: String,
    pub org: OrgId,
    pub config: StreamConfig,
    pub redacted_config: JsonValue,
    pub created_at: Instant,
    pub handle: Option<Arc<RunningStreamHandle>>,
    pub frozen_uptime: Option<Duration>,
    pub metrics: Arc<StreamMetrics>,
    pub last_run: Option<RunSummary>,
    /// The config as posted, before `${VAR}` interpolation (what a PATCH merges into).
    pub posted: Option<JsonValue>,
}

impl ManagedStream {
    pub fn is_active(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| h.is_running())
    }

    pub fn is_ready(&self) -> bool {
        match &self.handle {
            Some(h) if h.is_running() => h.is_ready(),
            _ => true,
        }
    }

    /// Returns the stream's uptime in seconds and Go-formatted duration string.
    /// When a stream finishes (e.g. a finite `generate` or `file` input or `run: once`),
    /// its uptime freezes at the duration of that run and resets on `PUT /streams/{id}` (`PART B [T-6140]`).
    pub fn uptime(&self) -> Duration {
        if let Some(ref h) = self.handle {
            return h.uptime();
        }
        self.frozen_uptime
            .unwrap_or_else(|| self.created_at.elapsed())
    }

    pub fn current_last_run(&self) -> Option<RunSummary> {
        self.handle
            .as_ref()
            .and_then(|h| h.last_summary())
            .or_else(|| self.last_run.clone())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamEntryStatus {
    pub active: bool,
    pub uptime: f64,
    pub uptime_str: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<RunSummary>,
}

pub fn format_go_duration(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "0s".to_string();
    }
    if nanos < 1_000_000 {
        return format!("{}µs", nanos / 1_000);
    }
    if nanos < 1_000_000_000 {
        return format!("{:.3}ms", d.as_secs_f64() * 1_000.0);
    }
    format!("{:.3}s", d.as_secs_f64())
}

/// A stream's identity: its organization and its id (two organizations may each own `ingest`).
type StreamKey = (OrgId, String);

fn stream_key(caller_org: Option<&OrgId>, id: &str) -> StreamKey {
    (caller_org.cloned().unwrap_or_default(), id.to_string())
}

/// Missing and foreign streams answer alike, so an id never reveals another organization's
/// stream (the API maps this to 404 "Stream not found").
fn not_found(id: &str) -> CoreError {
    CoreError::KeyNotFound(format!("stream '{id}' does not exist"))
}

/// Manages multiple isolated streams in `streams` mode (`internal/stream/manager/type.go`).
///
/// Streams are keyed by `(organization, id)`. Every method takes the caller's organization;
/// `None` means the default organization (a server without auth serves only that one).
#[derive(Clone)]
pub struct StreamManager {
    streams: Arc<RwLock<HashMap<StreamKey, ManagedStream>>>,
    pub registry: Arc<Registry>,
    pub resources: ResourceManager,
    pub metrics_registry: MetricsRegistry,
    pub file_root: Option<std::path::PathBuf>,
    /// Which process variables configs and `env()` read (E1); `true`/`false` convert as the old
    /// `allow_host_env` flag did.
    pub env_access: EnvAccess,
}

impl StreamManager {
    pub fn new(
        registry: Arc<Registry>,
        resources: ResourceManager,
        file_root: Option<std::path::PathBuf>,
        env_access: impl Into<EnvAccess>,
    ) -> Self {
        Self {
            streams: Arc::new(RwLock::new(HashMap::new())),
            registry,
            resources,
            metrics_registry: MetricsRegistry::new(),
            file_root,
            env_access: env_access.into(),
        }
    }

    /// Uses `metrics_registry` (built from the `metrics:` section) instead of the defaults.
    pub fn with_metrics_registry(mut self, metrics_registry: MetricsRegistry) -> Self {
        self.metrics_registry = metrics_registry;
        self
    }

    /// Fills `${VAR}` in `raw` for organization `org`: its variable map, then the process
    /// environment as `env_access` allows.
    pub fn interpolate(&self, org: &OrgId, raw: &str) -> Result<String, EnvLintError> {
        let vars = self.resources.get_or_create_org(org).secrets_snapshot();
        let access = self.env_access;
        interpolate_env_str(raw, |k| access.resolve(&vars, k))
    }

    /// Parses a config file as this server reads it: the organization is read from the text
    /// first, then `${VAR}` is filled from that organization's variables.
    pub fn parse_config(
        &self,
        raw: &str,
        chilled: bool,
    ) -> Result<(StreamConfig, Vec<String>), CoreError> {
        let org = StreamConfig::peek_org(raw);
        let vars = self.resources.get_or_create_org(&org).secrets_snapshot();
        let access = self.env_access;
        StreamConfig::parse_str_with_env(raw, chilled, |k| access.resolve(&vars, k))
    }

    /// Registers the shared resources of a resources file (Bento's `streams -r`) in `org`'s
    /// namespace: every cache, rate limit, processor, input and output it declares becomes visible
    /// to each stream of that organization, and of no other. `${VAR}` is filled as in a posted
    /// config; any other section of the file (`metrics`) is ignored here. Returns how many
    /// resources were registered.
    pub fn register_resources(&self, org: &OrgId, raw: &str) -> Result<usize, CoreError> {
        let vars = self.resources.get_or_create_org(org).secrets_snapshot();
        let access = self.env_access;
        let (mut conf, _) =
            StreamConfig::parse_str_with_env(raw, false, |k| access.resolve(&vars, k))?;
        conf.org = org.clone();
        let r = &conf.resources;
        let count = r.cache_resources.len()
            + r.rate_limit_resources.len()
            + r.processor_resources.len()
            + r.input_resources.len()
            + r.output_resources.len();
        let (_shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        let org_ctx = self.resources.get_or_create_org(org);
        let sctx = ekiben_core::StreamContext {
            stream_id: "resources".to_string(),
            org: org_ctx,
            resources: self.resources.clone(),
            registry: self.registry.clone(),
            blobl_env: ekiben_core::config::build_blobl_env_for_stream(&vars, access),
            metrics: StreamMetrics::new("resources", &org.0),
            label: String::new(),
            path: "resources".to_string(),
            file_root: self.file_root.clone(),
            env_vars: Arc::new(vars),
            shutdown_rx,
        };
        crate::runner::register_inline_resources(&conf, &self.registry, &sctx)?;
        Ok(count)
    }

    /// Creates and starts a new stream under `id` in the organization `conf.org`.
    /// If `conf.run == RunMode::Once`, runs the stream to completion, freezes its uptime,
    /// stores its `last_run` summary, and drops its runtime state (`Section 6`).
    pub async fn create(
        &self,
        id: String,
        conf: StreamConfig,
    ) -> Result<Option<RunSummary>, CoreError> {
        self.create_posted(id, conf, None).await
    }

    /// As [`Self::create`], remembering the config as posted (before interpolation): `GET`
    /// answers it redacted, so a `${SECRET}` never comes back filled in.
    pub async fn create_posted(
        &self,
        id: String,
        conf: StreamConfig,
        posted: Option<JsonValue>,
    ) -> Result<Option<RunSummary>, CoreError> {
        let key: StreamKey = (conf.org.clone(), id.clone());
        if self.streams.read().await.contains_key(&key) {
            return Err(CoreError::KeyAlreadyExists(format!(
                "stream '{id}' already exists"
            )));
        }

        let redacted = redact_config_value(posted.as_ref().unwrap_or(&conf.raw_source));
        let created_at = Instant::now();

        let handle = spawn_stream(
            id.clone(),
            conf.clone(),
            self.registry.clone(),
            self.resources.clone(),
            Some(&self.metrics_registry),
            self.file_root.clone(),
            self.env_access,
        )?;
        let metrics = handle.metrics.clone();

        if conf.run == RunMode::Once {
            let summary = handle
                .wait_for_completion()
                .await
                .ok_or_else(|| CoreError::other("stream task terminated without summary"))?;
            let frozen = Duration::from_millis(summary.duration_ms.max(1));
            let managed = ManagedStream {
                id: id.clone(),
                org: conf.org.clone(),
                config: conf,
                redacted_config: redacted,
                created_at,
                handle: None,
                frozen_uptime: Some(frozen),
                metrics,
                last_run: Some(summary.clone()),
                posted,
            };
            let mut guard = self.streams.write().await;
            if guard.contains_key(&key) {
                return Err(CoreError::KeyAlreadyExists(format!(
                    "stream '{id}' already exists"
                )));
            }
            guard.insert(key, managed);
            return Ok(Some(summary));
        }

        let managed = ManagedStream {
            id: id.clone(),
            org: conf.org.clone(),
            config: conf,
            redacted_config: redacted,
            created_at,
            handle: Some(handle.clone()),
            frozen_uptime: None,
            metrics,
            last_run: None,
            posted,
        };
        let raced = {
            let mut guard = self.streams.write().await;
            match guard.entry(key) {
                Entry::Occupied(_) => true,
                Entry::Vacant(slot) => {
                    slot.insert(managed);
                    false
                }
            }
        };
        if raced {
            let _ = handle.stop(Duration::from_secs(5)).await;
            return Err(CoreError::KeyAlreadyExists(format!(
                "stream '{id}' already exists"
            )));
        }
        Ok(None)
    }

    /// Reads the status and redacted config of the caller's stream `id`.
    pub async fn read(
        &self,
        id: &str,
        caller_org: Option<&OrgId>,
    ) -> Result<StreamEntryStatus, CoreError> {
        let guard = self.streams.read().await;
        let strm = guard
            .get(&stream_key(caller_org, id))
            .ok_or_else(|| not_found(id))?;
        let up = strm.uptime();
        Ok(StreamEntryStatus {
            active: strm.is_active(),
            uptime: up.as_secs_f64(),
            uptime_str: format_go_duration(up),
            config: Some(strm.redacted_config.clone()),
            last_run: strm.current_last_run(),
        })
    }

    pub async fn read_stats(
        &self,
        id: &str,
        caller_org: Option<&OrgId>,
    ) -> Result<JsonValue, CoreError> {
        let guard = self.streams.read().await;
        let strm = guard
            .get(&stream_key(caller_org, id))
            .ok_or_else(|| not_found(id))?;
        let up_ns = u64::try_from(strm.uptime().as_nanos()).unwrap_or(u64::MAX);
        Ok(strm.metrics.snapshot_json(up_ns))
    }

    /// The caller's streams (only its organization's).
    pub async fn list(&self, caller_org: Option<&OrgId>) -> HashMap<String, StreamEntryStatus> {
        let org = caller_org.cloned().unwrap_or_default();
        let guard = self.streams.read().await;
        let mut out = HashMap::new();
        for ((strm_org, id), strm) in guard.iter() {
            if strm_org != &org {
                continue;
            }
            let up = strm.uptime();
            out.insert(
                id.clone(),
                StreamEntryStatus {
                    active: strm.is_active(),
                    uptime: up.as_secs_f64(),
                    uptime_str: format_go_duration(up),
                    config: None,
                    last_run: strm.current_last_run(),
                },
            );
        }
        out
    }

    /// Updates stream `id` by stopping the previous instance and starting a new one (`PUT /streams/{id}`).
    pub async fn update(
        &self,
        id: &str,
        conf: StreamConfig,
        caller_org: Option<&OrgId>,
    ) -> Result<Option<RunSummary>, CoreError> {
        self.update_posted(id, conf, caller_org, None).await
    }

    /// As [`Self::update`], remembering the config as posted.
    pub async fn update_posted(
        &self,
        id: &str,
        mut conf: StreamConfig,
        caller_org: Option<&OrgId>,
        posted: Option<JsonValue>,
    ) -> Result<Option<RunSummary>, CoreError> {
        self.delete(id, caller_org).await?;
        conf.org = caller_org.cloned().unwrap_or_default();
        self.create_posted(id.to_string(), conf, posted).await
    }

    /// Patches stream `id`'s configuration and restarts it (`PATCH /streams/{id}`).
    pub async fn patch(
        &self,
        id: &str,
        patch_json: &JsonValue,
        caller_org: Option<&OrgId>,
    ) -> Result<Option<RunSummary>, CoreError> {
        let (mut merged_raw, mut posted) = {
            let guard = self.streams.read().await;
            let strm = guard
                .get(&stream_key(caller_org, id))
                .ok_or_else(|| not_found(id))?;
            (strm.config.raw_source.clone(), strm.posted.clone())
        };
        merge_json_patch(&mut merged_raw, patch_json);
        if let Some(p) = posted.as_mut() {
            merge_json_patch(p, patch_json);
        }
        let new_conf = StreamConfig::from_json_value(&merged_raw)?;
        self.update_posted(id, new_conf, caller_org, posted).await
    }

    /// Stops and removes the caller's stream `id` (`DELETE /streams/{id}`). Its counters stay
    /// in `/metrics` (and continue if the id is created again); its timings go.
    pub async fn delete(&self, id: &str, caller_org: Option<&OrgId>) -> Result<(), CoreError> {
        let key = stream_key(caller_org, id);
        let removed = self
            .streams
            .write()
            .await
            .remove(&key)
            .ok_or_else(|| not_found(id))?;

        if let Some(handle) = removed.handle {
            let _ = handle.stop(Duration::from_secs(5)).await;
        }
        self.metrics_registry.stream_deleted(&key.0 .0, id);
        Ok(())
    }

    /// Active streams that are not connected on both input and output (`GET /ready`), as
    /// `(organization, id)` sorted.
    pub async fn not_ready_streams(&self) -> Vec<(OrgId, String)> {
        let guard = self.streams.read().await;
        let mut not_ready = Vec::new();
        for ((org, id), strm) in guard.iter() {
            if strm.is_active() && !strm.is_ready() {
                not_ready.push((org.clone(), id.clone()));
            }
        }
        not_ready.sort_by(|a, b| (&a.0 .0, &a.1).cmp(&(&b.0 .0, &b.1)));
        not_ready
    }

    pub async fn stop_all(&self, timeout: Duration) {
        let mut guard = self.streams.write().await;
        for (_, strm) in guard.drain() {
            if let Some(h) = strm.handle {
                let _ = h.stop(timeout).await;
            }
        }
    }
}
