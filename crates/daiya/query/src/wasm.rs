#![forbid(unsafe_code)]
//! Third-party datasource plugins as WASM components (R5, §3.3).
//!
//! Host adapter implementing the `jc:daiya/datasource@0.1.0` WIT world (`wit/datasource.wit`):
//! - Enforces every host limit of §3.3 (`memory_bytes = 64 MiB`, `wall_time = 5s`,
//!   `background_wall_time = 60s`, `fuel = 200_000_000_000`, `request_bytes = 2 MiB`,
//!   `response_bytes = 8 MiB`, `per_plugin_concurrency = 16`, `per_org_concurrency = 64`).
//! - Verifies that the component's SHA-256 digest matches the organization datasource's pinned
//!   `wasm_digest`; an unknown or mismatched digest is refused before loading!
//! - Mediates all guest `http-out` and `secret` imports through [`OutboundGuard`] and [`SecretResolver`].

use crate::guard::{OutboundGuard, OutboundRequest, OutboundResponse};
use crate::{
    Datasource, DsError, DsFuture, EditorSchema, Health, ImportOutcome, Query, QueryCtx,
    ResourceReq, ResourceResp, SecretResolver,
};
use daiya_model::{FrameSet, OrgId, constant_time_eq, sha256_hex};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, SemaphorePermit};

/// Host limits for third-party WASM datasource plugins (§3.3, `wasm-host/src/limits.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmLimits {
    /// Linear memory per instance (default 64 MiB, §3.3).
    pub memory_bytes: usize,
    /// Wall time per request call (default 5s, §3.3).
    pub wall_time: Duration,
    /// Wall time for a background refresh job (default 60s, §3.3).
    pub background_wall_time: Duration,
    /// CPU fuel backstop (default 200 billion units, §3.3).
    pub fuel: u64,
    /// Maximum request payload size passed into a plugin (default 2 MiB, §3.3).
    pub request_bytes: usize,
    /// Maximum response payload size returned by a plugin (default 8 MiB, §3.3).
    pub response_bytes: usize,
    /// Maximum concurrent calls per plugin (default 16, §3.3).
    pub per_plugin_concurrency: usize,
    /// Maximum concurrent calls per organization (default 64, §3.3).
    pub per_org_concurrency: usize,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 << 20,
            wall_time: Duration::from_secs(5),
            background_wall_time: Duration::from_secs(60),
            fuel: 200_000_000_000,
            request_bytes: 2 << 20,
            response_bytes: 8 << 20,
            per_plugin_concurrency: 16,
            per_org_concurrency: 64,
        }
    }
}

/// Host capabilities exposed to a WASM guest (`jc:daiya/http-out` and `jc:daiya/secret` only, §3.3).
pub struct WasmHostImports<'a> {
    pub org_id: OrgId,
    pub guard: &'a OutboundGuard,
    pub secrets: &'a dyn SecretResolver,
    pub allowed_secret_refs: &'a std::collections::BTreeMap<String, String>,
}

impl<'a> WasmHostImports<'a> {
    /// Executes an outbound HTTP request through the host's [`OutboundGuard`].
    pub async fn http_out(&self, req: OutboundRequest) -> Result<OutboundResponse, DsError> {
        self.guard.send(req).await
    }

    /// Resolves a secretRef declared in the datasource's configuration.
    pub fn resolve_secret(&self, key_or_ref: &str) -> Result<zeroize::Zeroizing<String>, DsError> {
        let actual_ref = self
            .allowed_secret_refs
            .get(key_or_ref)
            .map_or(key_or_ref, String::as_str);
        if !self.allowed_secret_refs.values().any(|v| v == actual_ref) {
            return Err(DsError::SecretResolution {
                secret_ref: key_or_ref.to_owned(),
                reason: "secretRef is not declared on this datasource manifest".into(),
            });
        }
        self.secrets.resolve_secret(self.org_id, actual_ref)
    }
}

/// Guest interface matching the `jc:daiya/datasource@0.1.0` WIT world exports (R5).
/// (Since `wasmtime` is not in the root `Cargo.lock`, this trait powers the host adapter and test
/// double / sample plugin per R5).
pub trait WasmPluginGuest: Send + Sync {
    fn plugin_id(&self) -> &'static str;
    fn component_bytes(&self) -> &[u8];
    fn memory_usage_bytes(&self) -> usize {
        1 << 20
    }
    fn fuel_consumed(&self) -> u64 {
        1_000_000
    }

    fn export_query<'a>(
        &'a self,
        host: WasmHostImports<'a>,
        ctx: &'a QueryCtx,
        queries_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>>;

    fn export_health<'a>(
        &'a self,
        host: WasmHostImports<'a>,
        ctx: &'a QueryCtx,
    ) -> DsFuture<'a, (bool, String)>;

    fn export_resource<'a>(
        &'a self,
        host: WasmHostImports<'a>,
        ctx: &'a QueryCtx,
        path: &'a str,
        params_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>>;

    fn export_editor_schema(&self) -> String;
    fn export_import_target(&self, grafana_target_json: &str) -> Result<String, String>;
    fn export_export_target(&self, query_json: &str) -> String;
}

/// Registry and execution host for WASM datasource components (R5, §3.3).
pub struct WasmPluginHost {
    pub limits: WasmLimits,
    components_by_digest: Mutex<HashMap<String, Arc<dyn WasmPluginGuest>>>,
    plugin_semaphores: Mutex<HashMap<String, Arc<Semaphore>>>,
    org_semaphores: Mutex<HashMap<OrgId, Arc<Semaphore>>>,
}

impl Default for WasmPluginHost {
    fn default() -> Self {
        Self::new(WasmLimits::default())
    }
}

impl WasmPluginHost {
    #[must_use]
    pub fn new(limits: WasmLimits) -> Self {
        Self {
            limits,
            components_by_digest: Mutex::new(HashMap::new()),
            plugin_semaphores: Mutex::new(HashMap::new()),
            org_semaphores: Mutex::new(HashMap::new()),
        }
    }

    /// Registers a verified WASM component and returns its SHA-256 hex digest.
    pub fn register_component(&self, guest: Arc<dyn WasmPluginGuest>) -> String {
        let digest = sha256_hex(guest.component_bytes());
        self.components_by_digest
            .lock()
            .insert(digest.clone(), guest);
        digest
    }

    /// Loads a [`WasmPluginAdapter`] for a datasource whose manifest pins `pinned_digest` (R5).
    /// An unknown or mismatched digest is refused immediately!
    pub fn instantiate(
        self: &Arc<Self>,
        org_id: OrgId,
        plugin_kind: &'static str,
        pinned_digest: Option<&str>,
        guard: OutboundGuard,
        secrets: Arc<dyn SecretResolver>,
        allowed_secret_refs: std::collections::BTreeMap<String, String>,
    ) -> Result<WasmPluginAdapter, DsError> {
        let digest = pinned_digest
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                DsError::Wasm(
                    "datasource manifest is missing pinned wasm_digest; unsigned/unpinned WASM plugins cannot load"
                        .into(),
                )
            })?;

        let guest = {
            let map = self.components_by_digest.lock();
            map.iter()
                .find(|(k, _)| constant_time_eq(k, digest))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| {
                    DsError::Wasm(format!(
                        "unknown WASM component digest '{digest}': component refused to load"
                    ))
                })?
        };

        // Verify actual bytes match pinned digest
        let actual_digest = sha256_hex(guest.component_bytes());
        if !constant_time_eq(&actual_digest, digest) {
            return Err(DsError::Wasm(
                "component digest mismatch against pinned manifest digest".into(),
            ));
        }

        if guest.memory_usage_bytes() > self.limits.memory_bytes {
            return Err(DsError::Wasm(format!(
                "component linear memory {} bytes exceeds limit of {} bytes",
                guest.memory_usage_bytes(),
                self.limits.memory_bytes
            )));
        }

        let plugin_sem = self
            .plugin_semaphores
            .lock()
            .entry(digest.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(self.limits.per_plugin_concurrency)))
            .clone();

        let org_sem = self
            .org_semaphores
            .lock()
            .entry(org_id)
            .or_insert_with(|| Arc::new(Semaphore::new(self.limits.per_org_concurrency)))
            .clone();

        Ok(WasmPluginAdapter {
            kind: plugin_kind,
            org_id,
            host: self.clone(),
            guest,
            guard,
            secrets,
            allowed_secret_refs,
            plugin_sem,
            org_sem,
        })
    }
}

/// Adapter implementing [`Datasource`] for a digest-pinned WASM plugin (R5, §3.2, §3.3).
pub struct WasmPluginAdapter {
    pub kind: &'static str,
    pub org_id: OrgId,
    pub host: Arc<WasmPluginHost>,
    pub guest: Arc<dyn WasmPluginGuest>,
    pub guard: OutboundGuard,
    pub secrets: Arc<dyn SecretResolver>,
    pub allowed_secret_refs: std::collections::BTreeMap<String, String>,
    plugin_sem: Arc<Semaphore>,
    org_sem: Arc<Semaphore>,
}

impl WasmPluginAdapter {
    /// One call slot of the plugin and one of the organization (§3.3), for every export that
    /// runs guest code; a full plugin or organization is refused at once, never queued.
    fn permits(&self) -> Result<(SemaphorePermit<'_>, SemaphorePermit<'_>), DsError> {
        let plugin = self.plugin_sem.try_acquire().map_err(|_| {
            DsError::Wasm(format!(
                "per-plugin concurrency limit ({}) reached",
                self.host.limits.per_plugin_concurrency
            ))
        })?;
        let org = self.org_sem.try_acquire().map_err(|_| {
            DsError::Wasm(format!(
                "per-organization WASM concurrency limit ({}) reached",
                self.host.limits.per_org_concurrency
            ))
        })?;
        Ok((plugin, org))
    }

    fn check_limits(&self, req_len: usize) -> Result<(), DsError> {
        if req_len > self.host.limits.request_bytes {
            return Err(DsError::Wasm(format!(
                "WASM request payload {} bytes exceeds limit of {} bytes",
                req_len, self.host.limits.request_bytes
            )));
        }
        if self.guest.memory_usage_bytes() > self.host.limits.memory_bytes {
            return Err(DsError::Wasm(format!(
                "WASM linear memory {} bytes exceeds limit of {} bytes",
                self.guest.memory_usage_bytes(),
                self.host.limits.memory_bytes
            )));
        }
        if self.guest.fuel_consumed() > self.host.limits.fuel {
            return Err(DsError::Wasm(format!(
                "WASM fuel {} exceeds limit of {}",
                self.guest.fuel_consumed(),
                self.host.limits.fuel
            )));
        }
        Ok(())
    }
}

impl Datasource for WasmPluginAdapter {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn query<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        queries: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>> {
        Box::pin(async move {
            let _permits = self.permits()?;

            let req_json =
                serde_json::to_string(queries).map_err(|e| DsError::Wasm(e.to_string()))?;
            self.check_limits(req_json.len())?;

            let imports = WasmHostImports {
                org_id: self.org_id,
                guard: &self.guard,
                secrets: self.secrets.as_ref(),
                allowed_secret_refs: &self.allowed_secret_refs,
            };

            let wall_limit = ctx.timeout.min(self.host.limits.wall_time);
            let resp_json =
                tokio::time::timeout(wall_limit, self.guest.export_query(imports, ctx, &req_json))
                    .await
                    .map_err(|_| {
                        DsError::Wasm(format!(
                            "WASM plugin exceeded wall-time limit of {}ms",
                            wall_limit.as_millis()
                        ))
                    })?
                    .map_err(DsError::Wasm)?;

            if resp_json.len() > self.host.limits.response_bytes {
                return Err(DsError::Wasm(format!(
                    "WASM response payload {} bytes exceeds limit of {} bytes",
                    resp_json.len(),
                    self.host.limits.response_bytes
                )));
            }

            let frame_sets: Vec<FrameSet> = serde_json::from_str(&resp_json).map_err(|e| {
                DsError::Wasm(format!("invalid FrameSet JSON from WASM guest: {e}"))
            })?;
            Ok(frame_sets)
        })
    }

    fn health<'a>(&'a self, ctx: &'a QueryCtx) -> DsFuture<'a, Health> {
        Box::pin(async move {
            let _permits = match self.permits() {
                Ok(p) => p,
                Err(e) => return Health::error(e.to_string()),
            };
            let imports = WasmHostImports {
                org_id: self.org_id,
                guard: &self.guard,
                secrets: self.secrets.as_ref(),
                allowed_secret_refs: &self.allowed_secret_refs,
            };
            let wall_limit = ctx.timeout.min(self.host.limits.wall_time);
            match tokio::time::timeout(wall_limit, self.guest.export_health(imports, ctx)).await {
                Ok((true, msg)) => Health::ok(msg),
                Ok((false, msg)) => Health::error(msg),
                Err(_) => Health::error("WASM health check timed out"),
            }
        })
    }

    fn resource<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>> {
        Box::pin(async move {
            let params_json =
                serde_json::to_string(&r.params).map_err(|e| DsError::Wasm(e.to_string()))?;
            self.check_limits(params_json.len() + r.path.len())?;
            let _permits = self.permits()?;
            let imports = WasmHostImports {
                org_id: self.org_id,
                guard: &self.guard,
                secrets: self.secrets.as_ref(),
                allowed_secret_refs: &self.allowed_secret_refs,
            };
            let wall_limit = ctx.timeout.min(self.host.limits.wall_time);
            let out_str = tokio::time::timeout(
                wall_limit,
                self.guest
                    .export_resource(imports, ctx, &r.path, &params_json),
            )
            .await
            .map_err(|_| DsError::Wasm("WASM resource call timed out".into()))?
            .map_err(DsError::Wasm)?;

            if out_str.len() > self.host.limits.response_bytes {
                return Err(DsError::Wasm("WASM resource response too large".into()));
            }
            let data: Value =
                serde_json::from_str(&out_str).map_err(|e| DsError::Wasm(e.to_string()))?;
            Ok(ResourceResp { data })
        })
    }

    fn editor(&self) -> EditorSchema {
        let raw = self.guest.export_editor_schema();
        serde_json::from_str(&raw).unwrap_or_else(|_| EditorSchema {
            kind: self.kind.to_owned(),
            json_schema: serde_json::json!({"type": "object"}),
            field_hints: Vec::new(),
        })
    }

    fn import(&self, grafana_target: &Value) -> Result<ImportOutcome, DsError> {
        let s = serde_json::to_string(grafana_target).map_err(|e| DsError::Wasm(e.to_string()))?;
        self.check_limits(s.len())?;
        let out = self.guest.export_import_target(&s).map_err(DsError::Wasm)?;
        if out.len() > self.host.limits.response_bytes {
            return Err(DsError::Wasm("WASM import response too large".into()));
        }
        serde_json::from_str(&out).map_err(|e| DsError::Wasm(e.to_string()))
    }

    fn export(&self, q: &Query) -> Value {
        let s = serde_json::to_string(q).unwrap_or_default();
        let out = self.guest.export_export_target(&s);
        serde_json::from_str(&out).unwrap_or(Value::Null)
    }
}
