#![forbid(unsafe_code)]
//! Input validation caps and per-organization quotas (§3.3, §5.3, R7, R8, R12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input caps enforced on every dashboard write, import, and query (R12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InputLimits {
    /// Maximum size of a dashboard JSON document in bytes (default 4 MiB, R12).
    pub max_dashboard_json_bytes: usize,
    /// Maximum number of panels per dashboard (default 250, R12).
    pub max_panels_per_dashboard: usize,
    /// Maximum number of query targets per panel (default 64, R12).
    pub max_targets_per_panel: usize,
    /// Maximum nesting depth of math expressions and DAG pipelines (default 100, R12 & `parse.go:34`).
    pub max_expression_depth: usize,
    /// Maximum nested folder depth (`GF pkg/services/folder/model.go:30 MaxNestedFolderDepth = 4`).
    pub max_folder_depth: usize,
}

impl Default for InputLimits {
    fn default() -> Self {
        Self {
            max_dashboard_json_bytes: 4 * 1024 * 1024,
            max_panels_per_dashboard: 250,
            max_targets_per_panel: 64,
            max_expression_depth: 100,
            max_folder_depth: 4,
        }
    }
}

/// Per-organization resource and rate quotas (§5.3, R7, R8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgQuotas {
    pub max_dashboards: u64,
    pub max_panels_per_dashboard: u64,
    pub max_datasources: u64,
    pub max_wasm_plugins: u64,
    pub max_queries_per_minute: u64,
    pub max_concurrent_queries: u64,
    pub max_cache_bytes: u64,
    pub max_alert_rules: u64,
    pub max_evaluations_per_minute: u64,
    /// Maximum time range (in seconds) allowed on a public dashboard query (R7, default 31 days).
    pub max_public_range_seconds: i64,
    /// Minimum step/interval (in milliseconds) allowed on a public dashboard query (R7, default 15s).
    pub min_public_interval_ms: i64,
}

impl Default for OrgQuotas {
    fn default() -> Self {
        Self {
            max_dashboards: 500,
            max_panels_per_dashboard: 250,
            max_datasources: 100,
            max_wasm_plugins: 32,
            max_queries_per_minute: 1_200,
            max_concurrent_queries: 64,
            max_cache_bytes: 64 * 1024 * 1024,
            max_alert_rules: 200,
            max_evaluations_per_minute: 600,
            max_public_range_seconds: 31 * 86_400,
            min_public_interval_ms: 15_000,
        }
    }
}
