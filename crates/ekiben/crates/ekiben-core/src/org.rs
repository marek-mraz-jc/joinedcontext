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
///
/// The message and HTTP rates pace a stream (it waits); only the memory and processing-time
/// limits fail a run. An unknown field is refused, so a typo never silently keeps a default.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Exact host names this organization may reach although they resolve to private addresses
    /// (an ekiben extension; example: the in-cluster gateway). Every resolved address is still
    /// checked: loopback, link-local, cloud metadata, unspecified and multicast stay refused, so
    /// a listed name cannot be re-pointed at the node itself.
    #[serde(default)]
    pub private_hosts: Vec<String>,
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
            private_hosts: Vec::new(),
        }
    }
}

impl OrgQuotas {
    /// Refuses a limit of zero: it would stop every stream of the organization for good.
    /// Returns the name of the first offending field.
    pub fn validate(&self) -> Result<(), &'static str> {
        let limits = [
            ("max_concurrent_streams", self.max_concurrent_streams as u64),
            ("max_in_flight_messages", self.max_in_flight_messages as u64),
            (
                "max_memory_bytes_per_run",
                self.max_memory_bytes_per_run as u64,
            ),
            ("max_cpu_time_ms_per_run", self.max_cpu_time_ms_per_run),
            ("max_messages_per_sec", self.max_messages_per_sec),
            ("max_http_requests_per_sec", self.max_http_requests_per_sec),
        ];
        match limits.iter().find(|(_, v)| *v == 0) {
            Some((name, _)) => Err(name),
            None => Ok(()),
        }
    }
}

/// How long a pacer lets a stream run ahead of its rate before it waits (one second of burst).
const PACER_BURST: Duration = Duration::from_secs(1);
/// The longest single sleep of a waiting stream, so a changed limit or a shutdown is seen quickly.
const PACER_MAX_SLEEP: Duration = Duration::from_millis(100);

/// A generic cell rate pacer: each admission of `n` units pushes `next_free` by `n / limit`
/// seconds, and an admission waits while `next_free` is more than [`PACER_BURST`] ahead.
struct Pacer {
    next_free: Instant,
}

impl Pacer {
    fn new() -> Self {
        Self {
            next_free: Instant::now(),
        }
    }

    /// Reserves `amount` units at `limit_per_sec` and returns `None`, or returns how long to wait.
    fn reserve(&mut self, amount: u64, limit_per_sec: u64, now: Instant) -> Option<Duration> {
        let allowed_at = self.next_free.checked_sub(PACER_BURST).unwrap_or(now);
        if now < allowed_at {
            return Some(allowed_at - now);
        }
        let cost = Duration::from_secs_f64(amount as f64 / limit_per_sec.max(1) as f64);
        self.next_free = self.next_free.max(now) + cost;
        None
    }
}

/// The loopback listener a hosting process (`jc`) lets its streams reach: plain HTTP to
/// exactly `addr` (a literal address, never a name), under one of `path_prefixes` (whole
/// segments: `/broker` admits `/broker` and `/broker/...`, not `/brokerx`). Every other private
/// or loopback target stays refused, other mounts of the same listener included. Set by the
/// host, never by a stream or the API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostTarget {
    pub addr: std::net::SocketAddr,
    pub path_prefixes: Vec<String>,
    /// A second listener of the host whose address is known only once it is bound (jc: the
    /// Portal's internal listener, which a stream's report sinks post to), under its own
    /// prefixes and the same rules; nothing passes while it is unset.
    pub late: Option<(Arc<std::sync::OnceLock<std::net::SocketAddr>>, Vec<String>)>,
}

impl HostTarget {
    /// Whether `url` is a request to one of these listeners under an admitted prefix. A path
    /// holding an encoded dot, slash or backslash is refused: the listener may decode it into
    /// another mount.
    pub fn admits(&self, url: &url::Url) -> bool {
        door_admits(self.addr, &self.path_prefixes, url)
            || self.late.as_ref().is_some_and(|(addr, prefixes)| {
                addr.get()
                    .is_some_and(|addr| door_admits(*addr, prefixes, url))
            })
    }
}

fn door_admits(addr: std::net::SocketAddr, prefixes: &[String], url: &url::Url) -> bool {
    if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let ip = match url.host() {
        Some(url::Host::Ipv4(ip)) => std::net::IpAddr::V4(ip),
        Some(url::Host::Ipv6(ip)) => std::net::IpAddr::V6(ip),
        _ => return false,
    };
    if ip != addr.ip() || url.port_or_known_default() != Some(addr.port()) {
        return false;
    }
    let path = url.path();
    let lower = path.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "\\"]
        .iter()
        .any(|bad| lower.contains(bad))
    {
        return false;
    }
    prefixes.iter().any(|prefix| {
        let prefix = prefix.trim_end_matches('/');
        !prefix.is_empty()
            && prefix.starts_with('/')
            && path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
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
    msg_rate: std::sync::Mutex<Pacer>,
    http_rate: std::sync::Mutex<Pacer>,
    secrets: RwLock<HashMap<String, SecretString>>,
    host_target: Option<Arc<HostTarget>>,
}

impl OrgContext {
    pub fn new(org_id: OrgId, quotas: OrgQuotas) -> Arc<Self> {
        Self::with_host_target(org_id, quotas, None)
    }

    /// As [`Self::new`], for a host that lets its streams reach `host_target`.
    pub fn with_host_target(
        org_id: OrgId,
        quotas: OrgQuotas,
        host_target: Option<Arc<HostTarget>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            org_id,
            project: None,
            quotas: RwLock::new(quotas),
            active_streams: AtomicUsize::new(0),
            in_flight_messages: AtomicUsize::new(0),
            total_messages_processed: AtomicU64::new(0),
            msg_rate: std::sync::Mutex::new(Pacer::new()),
            http_rate: std::sync::Mutex::new(Pacer::new()),
            secrets: RwLock::new(HashMap::new()),
            host_target,
        })
    }

    /// The hosting process's own listener this organization's streams may reach, if any.
    pub fn host_target(&self) -> Option<&HostTarget> {
        self.host_target.as_deref()
    }

    /// Replaces the quotas; running streams pace by the new rates from their next admission on
    /// (a waiting stream rechecks every [`PACER_MAX_SLEEP`]), and a debt run up under the old
    /// rate is forgiven so a raised limit takes effect at once.
    pub fn set_quotas(&self, quotas: OrgQuotas) {
        *self.quotas.write().unwrap() = quotas;
        *self.msg_rate.lock().unwrap() = Pacer::new();
        *self.http_rate.lock().unwrap() = Pacer::new();
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

    /// Replaces the organization's whole variable map (`--org-vars <dir>/<org>.yaml`) at once,
    /// so a reload never shows half a change. Values stay `SecretString` (redacted `Debug`).
    pub fn replace_secrets(&self, vars: HashMap<String, String>) {
        let next: HashMap<String, SecretString> = vars
            .into_iter()
            .map(|(k, v)| (k, SecretString::new(v)))
            .collect();
        *self
            .secrets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }

    /// A copy of the organization's variable map, for `${VAR}` and Bloblang `env()` of its streams.
    pub fn secrets_snapshot(&self) -> HashMap<String, String> {
        self.secrets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(k, v)| (k.clone(), v.expose().to_string()))
            .collect()
    }

    /// Whether `host` is one of the organization's `private_hosts` (exact, case-insensitive).
    pub fn is_private_host_allowed(&self, host: &str) -> bool {
        let clean = host
            .trim_matches(|c| c == '[' || c == ']')
            .trim_end_matches('.');
        let quotas = self
            .quotas
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        quotas
            .private_hosts
            .iter()
            .any(|h| !h.is_empty() && h.eq_ignore_ascii_case(clean))
    }

    /// Whether `host` on `port` is one of the organization's `private_hosts`: an entry `name`
    /// admits every port of that name, an entry `name:port` that one port alone (jc names the
    /// MQTT brokers' 1883 so, and not their HTTP API).
    pub fn is_private_host_port_allowed(&self, host: &str, port: u16) -> bool {
        if self.is_private_host_allowed(host) {
            return true;
        }
        let clean = host
            .trim_matches(|c| c == '[' || c == ']')
            .trim_end_matches('.');
        let quotas = self
            .quotas
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        quotas.private_hosts.iter().any(|entry| {
            entry.rsplit_once(':').is_some_and(|(name, p)| {
                !name.is_empty() && name.eq_ignore_ascii_case(clean) && p.parse() == Ok(port)
            })
        })
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

    /// Fails a batch larger than the per-run memory limit (a hard limit).
    pub fn check_batch_memory(&self, bytes: usize) -> Result<(), CoreError> {
        let limit = self.quotas.read().unwrap().max_memory_bytes_per_run;
        if bytes > limit {
            return Err(CoreError::QuotaExceeded {
                org: self.org_id.0.clone(),
                reason: format!(
                    "batch size ({bytes} bytes) exceeds per-run memory limit ({limit} bytes)"
                ),
            });
        }
        Ok(())
    }

    /// Waits until `count` messages fit the organization's `max_messages_per_sec`, then accounts
    /// for them. Returns false, without admitting, when `cancelled` turns true while waiting.
    pub async fn admit_messages(&self, count: u64, cancelled: impl Fn() -> bool + Sync) -> bool {
        let admitted = pace(&self.msg_rate, count, &cancelled, || {
            self.quotas.read().unwrap().max_messages_per_sec
        })
        .await;
        if admitted {
            self.total_messages_processed
                .fetch_add(count, Ordering::Relaxed);
        }
        admitted
    }

    /// Checks an outbound HTTP host against the organization's allow-list.
    pub fn check_http_host(&self, host: &str) -> Result<(), CoreError> {
        let quotas = self.quotas.read().unwrap();
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
        Ok(())
    }

    /// Waits until one more outbound HTTP request fits `max_http_requests_per_sec`.
    pub async fn admit_http_request(&self) {
        pace(&self.http_rate, 1, &|| false, || {
            self.quotas.read().unwrap().max_http_requests_per_sec
        })
        .await;
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

impl OrgContext {
    /// A NATS KV bucket is stored under the subjects `$KV.<bucket>.>`, outside any subject
    /// prefix, so an organization with `nats_subject_prefix` `a.b` may only use buckets named
    /// `a_b`, `a_b_<name>` or `a_b-<name>`: one organization never opens another's bucket.
    pub fn validate_nats_bucket(&self, bucket: &str) -> Result<(), CoreError> {
        let quotas = self.quotas.read().unwrap();
        let Some(prefix) = quotas.nats_subject_prefix.as_deref() else {
            return Ok(());
        };
        let head = prefix.trim_end_matches('.').replace('.', "_");
        let ok = bucket
            .strip_prefix(head.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['_', '-']));
        if ok {
            return Ok(());
        }
        Err(CoreError::AccessDenied {
            org: self.org_id.0.clone(),
            reason: format!(
                "NATS KV bucket '{bucket}' is outside organization prefix: name it '{head}_<name>'"
            ),
        })
    }
}

/// Waits on `pacer` until `amount` units fit the limit `limit()` reads (live, each round).
async fn pace(
    pacer: &std::sync::Mutex<Pacer>,
    amount: u64,
    cancelled: &(impl Fn() -> bool + Sync),
    limit: impl Fn() -> u64 + Sync,
) -> bool {
    loop {
        let wait = pacer
            .lock()
            .unwrap()
            .reserve(amount, limit(), Instant::now());
        let Some(wait) = wait else {
            return true;
        };
        if cancelled() {
            return false;
        }
        tokio::time::sleep(wait.min(PACER_MAX_SLEEP)).await;
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
    host_target: Option<Arc<HostTarget>>,
}

impl ResourceManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets every organization's streams reach `target` (see [`HostTarget`]); call it before
    /// any organization exists.
    pub fn with_host_target(mut self, target: HostTarget) -> Self {
        self.host_target = Some(Arc::new(target));
        self
    }

    pub fn get_or_create_org(&self, org_id: &OrgId) -> Arc<OrgContext> {
        if let Some(existing) = self.orgs.read().unwrap().get(org_id) {
            return existing.clone();
        }
        let mut guard = self.orgs.write().unwrap();
        guard
            .entry(org_id.clone())
            .or_insert_with(|| {
                OrgContext::with_host_target(
                    org_id.clone(),
                    OrgQuotas::default(),
                    self.host_target.clone(),
                )
            })
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
