use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::CoreError;
use crate::redact::SecretString;
use crate::traits::{Cache, Input, Output, Processor, RateLimit};

/// Opaque organization identifier (`PART C §2.3, §7`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrgId(pub String);

impl Default for OrgId {
    fn default() -> Self {
        Self("default".to_string())
    }
}

impl OrgId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OrgId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Per-organization quotas enforced by `ekiben` (`Section 7`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrgQuotas {
    #[serde(default = "default_max_concurrent_streams")]
    pub max_concurrent_streams: usize,
    #[serde(default = "default_max_in_flight_messages")]
    pub max_in_flight_messages: usize,
    #[serde(default = "default_max_memory_bytes_per_run")]
    pub max_memory_bytes_per_run: usize,
    #[serde(default = "default_max_cpu_time_ms_per_run")]
    pub max_cpu_time_ms_per_run: u64,
    #[serde(default = "default_max_messages_per_sec")]
    pub max_messages_per_sec: u64,
    #[serde(default = "default_max_http_requests_per_sec")]
    pub max_http_requests_per_sec: u64,
    #[serde(default)]
    pub allow_private_ips: bool,
    #[serde(default)]
    pub allowed_http_hosts: Option<Vec<String>>,
    #[serde(default)]
    pub nats_subject_prefix: Option<String>,
}

fn default_max_concurrent_streams() -> usize {
    64
}
fn default_max_in_flight_messages() -> usize {
    4096
}
fn default_max_memory_bytes_per_run() -> usize {
    256 * 1024 * 1024 // 256 MiB
}
fn default_max_cpu_time_ms_per_run() -> u64 {
    60_000
}
fn default_max_messages_per_sec() -> u64 {
    50_000
}
fn default_max_http_requests_per_sec() -> u64 {
    1_000
}

impl Default for OrgQuotas {
    fn default() -> Self {
        Self {
            max_concurrent_streams: default_max_concurrent_streams(),
            max_in_flight_messages: default_max_in_flight_messages(),
            max_memory_bytes_per_run: default_max_memory_bytes_per_run(),
            max_cpu_time_ms_per_run: default_max_cpu_time_ms_per_run(),
            max_messages_per_sec: default_max_messages_per_sec(),
            max_http_requests_per_sec: default_max_http_requests_per_sec(),
            allow_private_ips: false,
            allowed_http_hosts: None,
            nats_subject_prefix: None,
        }
    }
}

struct TokenWindow {
    window_start: Instant,
    used: u64,
}

impl TokenWindow {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            used: 0,
        }
    }

    fn try_consume(&mut self, amount: u64, limit_per_sec: u64) -> bool {
        if limit_per_sec == 0 {
            return false;
        }
        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.window_start = Instant::now();
            self.used = 0;
        }
        if self.used.saturating_add(amount) > limit_per_sec {
            return false;
        }
        self.used += amount;
        true
    }
}

/// Live runtime state and quota enforcement for a single organization.
/// Uses per-org atomics and per-org spin-free locks so there is **no global lock on the hot path**;
/// one organization hitting its limit stops cleanly without delaying any other organization.
pub struct OrgContext {
    pub org_id: OrgId,
    pub project: Option<String>,
    pub quotas: RwLock<OrgQuotas>,
    pub active_streams: AtomicUsize,
    pub in_flight_messages: AtomicUsize,
    pub total_messages_processed: AtomicU64,
    msg_rate: std::sync::Mutex<TokenWindow>,
    http_rate: std::sync::Mutex<TokenWindow>,
    secrets: RwLock<HashMap<String, SecretString>>,
}

impl OrgContext {
    pub fn new(org_id: OrgId, quotas: OrgQuotas) -> Arc<Self> {
        Arc::new(Self {
            org_id,
            project: None,
            quotas: RwLock::new(quotas),
            active_streams: AtomicUsize::new(0),
            in_flight_messages: AtomicUsize::new(0),
            total_messages_processed: AtomicU64::new(0),
            msg_rate: std::sync::Mutex::new(TokenWindow::new()),
            http_rate: std::sync::Mutex::new(TokenWindow::new()),
            secrets: RwLock::new(HashMap::new()),
        })
    }

    pub fn set_quotas(&self, quotas: OrgQuotas) {
        *self.quotas.write().unwrap() = quotas;
    }

    pub fn get_quotas(&self) -> OrgQuotas {
        self.quotas.read().unwrap().clone()
    }

    pub fn set_secret(&self, name: impl Into<String>, value: impl Into<String>) {
        self.secrets
            .write()
            .unwrap()
            .insert(name.into(), SecretString::new(value));
    }

    pub fn resolve_secret(&self, name: &str) -> Option<String> {
        self.secrets
            .read()
            .unwrap()
            .get(name)
            .map(|s| s.expose().to_string())
    }

    pub fn clear_secrets(&self) {
        self.secrets.write().unwrap().clear();
    }

    /// Attempts to acquire a concurrent stream slot for this organization.
    pub fn acquire_stream_slot(self: &Arc<Self>) -> Result<OrgStreamGuard, CoreError> {
        let max_streams = self.quotas.read().unwrap().max_concurrent_streams;
        let prev = self.active_streams.fetch_add(1, Ordering::AcqRel);
        if prev >= max_streams {
            self.active_streams.fetch_sub(1, Ordering::AcqRel);
            return Err(CoreError::QuotaExceeded {
                org: self.org_id.0.clone(),
                reason: format!(
                    "concurrent stream limit ({max_streams}) exceeded for organization '{}'",
                    self.org_id
                ),
            });
        }
        Ok(OrgStreamGuard { org: self.clone() })
    }

    /// Checks and accounts for `count` messages and `bytes` against the organization's quotas.
    pub fn check_message_admission(&self, count: usize, bytes: usize) -> Result<(), CoreError> {
        let quotas = self.quotas.read().unwrap().clone();
        if bytes > quotas.max_memory_bytes_per_run {
            return Err(CoreError::QuotaExceeded {
                org: self.org_id.0.clone(),
                reason: format!(
                    "batch size ({bytes} bytes) exceeds per-run memory limit ({} bytes)",
                    quotas.max_memory_bytes_per_run
                ),
            });
        }
        let in_flight = self.in_flight_messages.load(Ordering::Acquire);
        if in_flight.saturating_add(count) > quotas.max_in_flight_messages {
            return Err(CoreError::QuotaExceeded {
                org: self.org_id.0.clone(),
                reason: format!(
                    "in-flight messages ({}) would exceed limit ({})",
                    in_flight + count,
                    quotas.max_in_flight_messages
                ),
            });
        }
        let mut mw = self.msg_rate.lock().unwrap();
        if !mw.try_consume(count as u64, quotas.max_messages_per_sec) {
            return Err(CoreError::QuotaExceeded {
                org: self.org_id.0.clone(),
                reason: format!(
                    "messages per second limit ({}) exceeded",
                    quotas.max_messages_per_sec
                ),
            });
        }
        self.total_messages_processed
            .fetch_add(count as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Checks outbound HTTP request rate limit and host allow-list for this organization.
    pub fn check_http_request(&self, host: &str) -> Result<(), CoreError> {
        let quotas = self.quotas.read().unwrap().clone();
        if let Some(ref allowed) = quotas.allowed_http_hosts {
            let host_lower = host.to_ascii_lowercase();
            let permitted = allowed.iter().any(|pattern| {
                let p = pattern.to_ascii_lowercase();
                if let Some(suffix) = p.strip_prefix("*.") {
                    host_lower == suffix || host_lower.ends_with(&format!(".{suffix}"))
                } else {
                    host_lower == p
                }
            });
            if !permitted {
                return Err(CoreError::AccessDenied {
                    org: self.org_id.0.clone(),
                    reason: format!(
                        "outbound HTTP host '{host}' is not in organization allow-list"
                    ),
                });
            }
        }
        let mut hw = self.http_rate.lock().unwrap();
        if !hw.try_consume(1, quotas.max_http_requests_per_sec) {
            return Err(CoreError::QuotaExceeded {
                org: self.org_id.0.clone(),
                reason: format!(
                    "outbound HTTP requests per second limit ({}) exceeded",
                    quotas.max_http_requests_per_sec
                ),
            });
        }
        Ok(())
    }

    /// Validates or prefixes a NATS subject so the stream can only publish/subscribe within its organization's namespace.
    pub fn validate_nats_subject(&self, subject: &str) -> Result<String, CoreError> {
        let quotas = self.quotas.read().unwrap();
        if let Some(ref prefix) = quotas.nats_subject_prefix {
            let clean_prefix = prefix.trim_end_matches('.');
            let expected_head = format!("{clean_prefix}.");
            if subject == clean_prefix || subject.starts_with(&expected_head) {
                return Ok(subject.to_string());
            }
            return Err(CoreError::AccessDenied {
                org: self.org_id.0.clone(),
                reason: format!(
                    "NATS subject '{subject}' is outside organization prefix '{expected_head}'"
                ),
            });
        }
        Ok(subject.to_string())
    }
}

pub struct OrgStreamGuard {
    org: Arc<OrgContext>,
}

impl Drop for OrgStreamGuard {
    fn drop(&mut self) {
        self.org.active_streams.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Multi-tenant resource manager where every resource (`Cache`, `RateLimit`, `Processor`, `Input`, `Output`)
/// and every org context is strictly namespaced by `(OrgId, label)`.
///
/// An organization can **only** look up or access resources registered under its own `OrgId`.
/// Resources keyed by `(OrgId, label)`.
type OrgScoped<T> = Arc<RwLock<HashMap<(OrgId, String), T>>>;

#[derive(Clone, Default)]
pub struct ResourceManager {
    orgs: Arc<RwLock<HashMap<OrgId, Arc<OrgContext>>>>,
    caches: OrgScoped<Arc<dyn Cache>>,
    rate_limits: OrgScoped<Arc<dyn RateLimit>>,
    processors: OrgScoped<Arc<dyn Processor>>,
    inputs: OrgScoped<Arc<AsyncMutex<Box<dyn Input>>>>,
    outputs: OrgScoped<Arc<dyn Output>>,
}

impl ResourceManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create_org(&self, org_id: &OrgId) -> Arc<OrgContext> {
        if let Some(existing) = self.orgs.read().unwrap().get(org_id) {
            return existing.clone();
        }
        let mut guard = self.orgs.write().unwrap();
        guard
            .entry(org_id.clone())
            .or_insert_with(|| OrgContext::new(org_id.clone(), OrgQuotas::default()))
            .clone()
    }

    pub fn set_org_quotas(&self, org_id: &OrgId, quotas: OrgQuotas) -> Arc<OrgContext> {
        let ctx = self.get_or_create_org(org_id);
        ctx.set_quotas(quotas);
        ctx
    }

    // Cache resources (scoped by OrgId)
    pub fn insert_cache(&self, org_id: &OrgId, label: impl Into<String>, cache: Arc<dyn Cache>) {
        self.caches
            .write()
            .unwrap()
            .insert((org_id.clone(), label.into()), cache);
    }

    pub fn get_cache(&self, org_id: &OrgId, label: &str) -> Result<Arc<dyn Cache>, CoreError> {
        self.caches
            .read()
            .unwrap()
            .get(&(org_id.clone(), label.to_string()))
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "cache resource '{label}' was not found in organization '{org_id}'"
                ))
            })
    }

    pub fn has_cache(&self, org_id: &OrgId, label: &str) -> bool {
        self.caches
            .read()
            .unwrap()
            .contains_key(&(org_id.clone(), label.to_string()))
    }

    // RateLimit resources (scoped by OrgId)
    pub fn insert_rate_limit(
        &self,
        org_id: &OrgId,
        label: impl Into<String>,
        rl: Arc<dyn RateLimit>,
    ) {
        self.rate_limits
            .write()
            .unwrap()
            .insert((org_id.clone(), label.into()), rl);
    }

    pub fn get_rate_limit(
        &self,
        org_id: &OrgId,
        label: &str,
    ) -> Result<Arc<dyn RateLimit>, CoreError> {
        self.rate_limits
            .read()
            .unwrap()
            .get(&(org_id.clone(), label.to_string()))
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "rate limit resource '{label}' was not found in organization '{org_id}'"
                ))
            })
    }

    pub fn has_rate_limit(&self, org_id: &OrgId, label: &str) -> bool {
        self.rate_limits
            .read()
            .unwrap()
            .contains_key(&(org_id.clone(), label.to_string()))
    }

    // Processor resources (scoped by OrgId)
    pub fn insert_processor(
        &self,
        org_id: &OrgId,
        label: impl Into<String>,
        proc: Arc<dyn Processor>,
    ) {
        self.processors
            .write()
            .unwrap()
            .insert((org_id.clone(), label.into()), proc);
    }

    pub fn get_processor(
        &self,
        org_id: &OrgId,
        label: &str,
    ) -> Result<Arc<dyn Processor>, CoreError> {
        self.processors
            .read()
            .unwrap()
            .get(&(org_id.clone(), label.to_string()))
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "processor resource '{label}' was not found in organization '{org_id}'"
                ))
            })
    }

    pub fn has_processor(&self, org_id: &OrgId, label: &str) -> bool {
        self.processors
            .read()
            .unwrap()
            .contains_key(&(org_id.clone(), label.to_string()))
    }

    // Input resources (scoped by OrgId)
    pub fn insert_input(&self, org_id: &OrgId, label: impl Into<String>, input: Box<dyn Input>) {
        self.inputs.write().unwrap().insert(
            (org_id.clone(), label.into()),
            Arc::new(AsyncMutex::new(input)),
        );
    }

    pub fn get_input(
        &self,
        org_id: &OrgId,
        label: &str,
    ) -> Result<Arc<AsyncMutex<Box<dyn Input>>>, CoreError> {
        self.inputs
            .read()
            .unwrap()
            .get(&(org_id.clone(), label.to_string()))
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "input resource '{label}' was not found in organization '{org_id}'"
                ))
            })
    }

    pub fn has_input(&self, org_id: &OrgId, label: &str) -> bool {
        self.inputs
            .read()
            .unwrap()
            .contains_key(&(org_id.clone(), label.to_string()))
    }

    // Output resources (scoped by OrgId)
    pub fn insert_output(&self, org_id: &OrgId, label: impl Into<String>, output: Arc<dyn Output>) {
        self.outputs
            .write()
            .unwrap()
            .insert((org_id.clone(), label.into()), output);
    }

    pub fn get_output(&self, org_id: &OrgId, label: &str) -> Result<Arc<dyn Output>, CoreError> {
        self.outputs
            .read()
            .unwrap()
            .get(&(org_id.clone(), label.to_string()))
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "output resource '{label}' was not found in organization '{org_id}'"
                ))
            })
    }

    pub fn has_output(&self, org_id: &OrgId, label: &str) -> bool {
        self.outputs
            .read()
            .unwrap()
            .contains_key(&(org_id.clone(), label.to_string()))
    }
}
