use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ekiben_core::config::merge_json_patch;
use ekiben_core::{
    redact_config_value, CoreError, EnvAccess, MetricsRegistry, OrgId, Registry, ResourceManager,
    RunMode, RunSummary, StreamConfig, StreamMetrics,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::RwLock;

use crate::runner::{run_stream_once, spawn_stream, RunningStreamHandle};

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

/// Manages multiple isolated streams in `streams` mode (`internal/stream/manager/type.go`).
#[derive(Clone)]
pub struct StreamManager {
    streams: Arc<RwLock<HashMap<String, ManagedStream>>>,
    pub registry: Arc<Registry>,
    pub resources: ResourceManager,
    pub metrics_registry: MetricsRegistry,
    pub file_root: Option<std::path::PathBuf>,
    /// Which process environment variables streams of this manager may read (`${VAR}` in a
    /// posted config, Bloblang `env()`).
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

    /// Creates and starts a new stream under `id`.
    /// If `conf.run == RunMode::Once`, runs the stream to completion, freezes its uptime,
    /// stores its `last_run` summary, and drops its runtime state (`Section 6`).
    pub async fn create(
        &self,
        id: String,
        conf: StreamConfig,
    ) -> Result<Option<RunSummary>, CoreError> {
        {
            let guard = self.streams.read().await;
            if guard.contains_key(&id) {
                return Err(CoreError::KeyAlreadyExists(format!(
                    "stream '{id}' already exists"
                )));
            }
        }

        let redacted = redact_config_value(&conf.raw_source);
        let created_at = Instant::now();

        if conf.run == RunMode::Once {
            let summary = run_stream_once(
                &id,
                conf.clone(),
                self.registry.clone(),
                self.resources.clone(),
                self.file_root.clone(),
                self.env_access,
            )
            .await?;
            let frozen = Duration::from_millis(summary.duration_ms.max(1));
            let metrics = StreamMetrics::new(id.clone(), conf.org.0.clone());
            metrics
                .input_received
                .store(summary.messages_in, std::sync::atomic::Ordering::Relaxed);
            metrics
                .output_sent
                .store(summary.messages_out, std::sync::atomic::Ordering::Relaxed);
            self.metrics_registry.register_stream(metrics.clone());

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
            };
            self.streams.write().await.insert(id, managed);
            return Ok(Some(summary));
        }

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

        let managed = ManagedStream {
            id: id.clone(),
            org: conf.org.clone(),
            config: conf,
            redacted_config: redacted,
            created_at,
            handle: Some(handle),
            frozen_uptime: None,
            metrics,
            last_run: None,
        };
        self.streams.write().await.insert(id, managed);
        Ok(None)
    }

    /// Reads the status and redacted config of stream `id`, enforcing organization ownership when `caller_org` is set.
    pub async fn read(
        &self,
        id: &str,
        caller_org: Option<&OrgId>,
    ) -> Result<StreamEntryStatus, CoreError> {
        let guard = self.streams.read().await;
        let strm = guard
            .get(id)
            .ok_or_else(|| CoreError::KeyNotFound(format!("stream '{id}' does not exist")))?;
        if let Some(c_org) = caller_org {
            if &strm.org != c_org {
                return Err(CoreError::AccessDenied {
                    org: c_org.0.clone(),
                    reason: format!("stream '{id}' belongs to another organization"),
                });
            }
        }
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
            .get(id)
            .ok_or_else(|| CoreError::KeyNotFound(format!("stream '{id}' does not exist")))?;
        if let Some(c_org) = caller_org {
            if &strm.org != c_org {
                return Err(CoreError::AccessDenied {
                    org: c_org.0.clone(),
                    reason: format!("stream '{id}' belongs to another organization"),
                });
            }
        }
        let up_ns = strm.uptime().as_nanos() as u64;
        Ok(strm.metrics.snapshot_json(up_ns))
    }

    pub async fn list(&self, caller_org: Option<&OrgId>) -> HashMap<String, StreamEntryStatus> {
        let guard = self.streams.read().await;
        let mut out = HashMap::new();
        for (id, strm) in guard.iter() {
            if let Some(c_org) = caller_org {
                if &strm.org != c_org {
                    continue;
                }
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
        self.delete(id, caller_org).await?;
        self.create(id.to_string(), conf).await
    }

    /// Patches stream `id`'s configuration and restarts it (`PATCH /streams/{id}`).
    pub async fn patch(
        &self,
        id: &str,
        patch_json: &JsonValue,
        caller_org: Option<&OrgId>,
    ) -> Result<Option<RunSummary>, CoreError> {
        let mut merged_raw = {
            let guard = self.streams.read().await;
            let strm = guard
                .get(id)
                .ok_or_else(|| CoreError::KeyNotFound(format!("stream '{id}' does not exist")))?;
            if let Some(c_org) = caller_org {
                if &strm.org != c_org {
                    return Err(CoreError::AccessDenied {
                        org: c_org.0.clone(),
                        reason: format!("stream '{id}' belongs to another organization"),
                    });
                }
            }
            strm.config.raw_source.clone()
        };
        merge_json_patch(&mut merged_raw, patch_json);
        let new_conf = StreamConfig::from_json_value(&merged_raw)?;
        self.update(id, new_conf, caller_org).await
    }

    /// Stops and removes stream `id` (`DELETE /streams/{id}`).
    pub async fn delete(&self, id: &str, caller_org: Option<&OrgId>) -> Result<(), CoreError> {
        let removed = {
            let mut guard = self.streams.write().await;
            let Some(strm) = guard.get(id) else {
                return Err(CoreError::KeyNotFound(format!(
                    "stream '{id}' does not exist"
                )));
            };
            if let Some(c_org) = caller_org {
                if &strm.org != c_org {
                    return Err(CoreError::AccessDenied {
                        org: c_org.0.clone(),
                        reason: format!("stream '{id}' belongs to another organization"),
                    });
                }
            }
            guard.remove(id).unwrap()
        };

        self.metrics_registry.remove_stream(id);
        if let Some(handle) = removed.handle {
            let _ = handle.stop(Duration::from_secs(5)).await;
        }
        Ok(())
    }

    /// Returns names of active streams that are not yet connected on both input and output (`GET /ready`).
    pub async fn not_ready_streams(&self) -> Vec<String> {
        let guard = self.streams.read().await;
        let mut not_ready = Vec::new();
        for (id, strm) in guard.iter() {
            if strm.is_active() && !strm.is_ready() {
                not_ready.push(id.clone());
            }
        }
        not_ready.sort();
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
