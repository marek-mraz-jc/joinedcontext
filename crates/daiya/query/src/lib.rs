#![forbid(unsafe_code)]
//! `daiya-query`: The `Datasource` trait and `FrameSet` of §3.2, outbound SSRF/DNS-pinning
//! guard, built-in datasources (Prometheus, NGSI-LD, OData 4.01, Postgres, Loki slot),
//! per-organization query cache, and WASM component host adapter + WIT world (R3, R5, R6).

pub mod cache;
pub mod factory;
pub mod guard;
pub mod loki;
pub mod ngsild;
pub mod odata;
pub mod postgres;
pub mod prometheus;
pub mod sample_plugin;
pub mod wasm;

pub use cache::{CacheKey, PublicCacheKey, QueryCache};
pub use daiya_model::{
    ColumnValues, FieldColumn, FieldType, Frame, FrameMeta, FrameSet, Notice, NoticeSeverity,
};
pub use factory::{DriverDeps, OutboundPolicy, build_driver, guard_for};
pub use guard::{GuardConfig, OutboundGuard, OutboundRequest, OutboundResponse};
pub use loki::LokiDatasource;
pub use ngsild::NgsiLdDatasource;
pub use odata::ODataDatasource;
pub use postgres::PostgresDatasource;
pub use prometheus::PrometheusDatasource;
pub use sample_plugin::SampleMetricsWasmPlugin;
pub use wasm::{WasmLimits, WasmPluginAdapter, WasmPluginGuest, WasmPluginHost};

use daiya_model::{CrError, InstanceId, OrgId, Subject};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use zeroize::Zeroizing;

/// Datasource execution error (never carries raw secret values, R3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DsError {
    #[error("invalid query for refId '{ref_id}': {reason}")]
    InvalidQuery { ref_id: String, reason: String },

    #[error("outbound guard refused request: {0}")]
    GuardRefused(String),

    #[error("upstream error ({status}): {message}")]
    Upstream { status: u16, message: String },

    #[error("secret resolution failed for '{secret_ref}': {reason}")]
    SecretResolution { secret_ref: String, reason: String },

    #[error("token exchange failed for audience '{audience}': {reason}")]
    TokenExchange { audience: String, reason: String },

    #[error("wasm plugin error: {0}")]
    Wasm(String),

    #[error("datasource '{0}' is not implemented in this deployment")]
    NotImplemented(String),
}

impl From<DsError> for CrError {
    fn from(err: DsError) -> Self {
        match err {
            DsError::InvalidQuery { ref_id, reason } => {
                Self::BadRequest(format!("query [{ref_id}]: {reason}"))
            }
            DsError::GuardRefused(msg) => Self::SsrfBlocked(msg),
            DsError::Upstream { status, message } => {
                Self::OutboundFailed(format!("upstream status {status}: {message}"))
            }
            DsError::SecretResolution { secret_ref, .. } => {
                Self::OutboundFailed(format!("failed to resolve secretRef '{secret_ref}'"))
            }
            DsError::TokenExchange { audience, reason } => {
                Self::Forbidden(format!("token exchange for '{audience}' refused: {reason}"))
            }
            DsError::Wasm(msg) => Self::WasmError(msg),
            DsError::NotImplemented(msg) => Self::NotImplemented(msg),
        }
    }
}

/// Resolves a `secretRef` at call time without storing or logging the secret (R3, §3.2, §4.2).
pub trait SecretResolver: Send + Sync {
    fn resolve_secret(&self, org_id: OrgId, secret_ref: &str)
    -> Result<Zeroizing<String>, DsError>;
}

/// In-memory secret resolver for standalone and test environments.
#[derive(Debug, Clone, Default)]
pub struct MapSecretResolver {
    secrets: BTreeMap<(OrgId, String), String>,
}

impl MapSecretResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        org_id: OrgId,
        secret_ref: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.secrets
            .insert((org_id, secret_ref.into()), value.into());
    }
}

impl SecretResolver for MapSecretResolver {
    fn resolve_secret(
        &self,
        org_id: OrgId,
        secret_ref: &str,
    ) -> Result<Zeroizing<String>, DsError> {
        self.secrets
            .get(&(org_id, secret_ref.to_owned()))
            .map(|s| Zeroizing::new(s.clone()))
            .ok_or_else(|| DsError::SecretResolution {
                secret_ref: secret_ref.to_owned(),
                reason: "secretRef not found in organization secret store".into(),
            })
    }
}

/// Exchanges a caller's token for a target Endpoint's audience (R3, §4.2: never a service token).
pub trait TokenExchanger: Send + Sync {
    fn exchange_for_audience(
        &self,
        caller_token: &str,
        audience: &str,
    ) -> Result<Zeroizing<String>, DsError>;
}

/// Default token exchanger: refuses every exchange. A deployment wires its real RFC 8693 exchanger
/// (kaisatsu); until then no caller token ever leaves daiya toward a datasource URL.
#[derive(Debug, Clone, Default)]
pub struct DenyTokenExchanger;

impl TokenExchanger for DenyTokenExchanger {
    fn exchange_for_audience(
        &self,
        _caller_token: &str,
        audience: &str,
    ) -> Result<Zeroizing<String>, DsError> {
        Err(DsError::TokenExchange {
            audience: audience.to_owned(),
            reason: "no token exchanger is configured".into(),
        })
    }
}

/// Test double: marks the caller's token with the audience (`exchanged:{audience}:{token}`). It
/// forwards the caller's token, so it is for tests and local mocks only, never a deployment.
#[derive(Debug, Clone, Default)]
pub struct PassthroughAudienceExchanger;

impl TokenExchanger for PassthroughAudienceExchanger {
    fn exchange_for_audience(
        &self,
        caller_token: &str,
        audience: &str,
    ) -> Result<Zeroizing<String>, DsError> {
        if caller_token.trim().is_empty() {
            return Err(DsError::TokenExchange {
                audience: audience.to_owned(),
                reason: "caller has no bearer token to exchange".into(),
            });
        }
        Ok(Zeroizing::new(format!(
            "exchanged:{audience}:{caller_token}"
        )))
    }
}

/// Per-query execution context (§3.2). Never carries raw credentials.
#[derive(Debug, Clone)]
pub struct QueryCtx {
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub subject: Subject,
    /// Range start in epoch milliseconds.
    pub from_ms: i64,
    /// Range end in epoch milliseconds.
    pub to_ms: i64,
    pub max_data_points: u32,
    pub interval_ms: u64,
    pub timeout: Duration,
    /// Resolved template variables (`name -> value`).
    pub variables: BTreeMap<String, String>,
}

impl QueryCtx {
    #[must_use]
    pub fn new(
        org_id: OrgId,
        instance_id: InstanceId,
        subject: Subject,
        from_ms: i64,
        to_ms: i64,
    ) -> Self {
        Self {
            org_id,
            instance_id,
            subject,
            from_ms,
            to_ms,
            max_data_points: 1000,
            interval_ms: 15_000,
            timeout: Duration::from_secs(10),
            variables: BTreeMap::new(),
        }
    }
}

/// Canonical query target passed to [`Datasource::query`] (§3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    pub ref_id: String,
    #[serde(default)]
    pub hide: bool,
    /// Datasource-specific target model preserved as JSON.
    pub model: Value,
}

/// Health status returned by [`Datasource::health`] (§3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: HealthState,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Ok,
    Error,
}

impl Health {
    #[must_use]
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            status: HealthState::Ok,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: HealthState::Error,
            message: message.into(),
        }
    }
}

/// Request to [`Datasource::resource`] for query editor completion helpers (§3.2, R6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceReq {
    pub path: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

/// Response from [`Datasource::resource`] (§3.2, R6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceResp {
    pub data: Value,
}

/// Field hint in [`EditorSchema`] driving the Portal query editor form (R6, §3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditorFieldHint {
    pub name: String,
    pub label: String,
    pub widget: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_path: Option<String>,
    pub required: bool,
}

/// JSON Schema of a datasource's target model plus field hints for the Portal query editor (§3.2, R6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditorSchema {
    pub kind: String,
    pub json_schema: Value,
    pub field_hints: Vec<EditorFieldHint>,
}

/// Outcome of importing a Grafana target into a [`Datasource`] (§3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportOutcome {
    pub query: Query,
    pub warnings: Vec<String>,
}

pub type DsFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The daiya datasource interface (§3.2, R3).
pub trait Datasource: Send + Sync {
    /// `"prometheus"`, `"ngsild"`, `"postgres"`, `"odata"`, `"loki"`, `"wasm:<id>"`.
    fn kind(&self) -> &'static str;

    /// `QueryData`: executes one or more queries and returns `FrameSet` per `refId`.
    fn query<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        q: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>>;

    /// `CheckHealth`: checks connectivity and credentials.
    fn health<'a>(&'a self, ctx: &'a QueryCtx) -> DsFuture<'a, Health>;

    /// `CallResource`: editor completion helpers only (metric names, label values, entity types, attributes).
    fn resource<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>>;

    /// JSON Schema of the target + field hints, driving the Portal query editor.
    fn editor(&self) -> EditorSchema;

    /// Grafana target JSON -> [`Query`], with any named warnings.
    fn import(&self, grafana_target: &Value) -> Result<ImportOutcome, DsError>;

    /// [`Query`] -> Grafana target JSON, the inverse of `import`.
    fn export(&self, q: &Query) -> Value;
}
