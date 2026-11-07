#![forbid(unsafe_code)]
//! Persistent entity records for every table in `controlroom` (§2.2, §5.3, R7, R8, R10).
//! Every row carries `installation_id`, `org_id` (UUIDv7), and `instance_id`.

use crate::ids::{InstallationId, InstanceId, OrgId};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Folder record (`cr_folder`, nested up to depth 4, `GF pkg/services/folder/model.go:30`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderRecord {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_uid: Option<String>,
    pub title: String,
    pub version: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
    pub created_by: String,
    pub updated_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Stored dashboard record (`cr_dashboard`: whole Grafana JSON at schemaVersion 42 + normalized index, R1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardRecord {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_uid: Option<String>,
    pub title: String,
    pub slug: String,
    pub tags: Vec<String>,
    pub panel_count: i32,
    pub datasource_uids: Vec<String>,
    pub version: i32,
    pub schema_version: i16,
    pub is_provisioned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisioned_external_id: Option<String>,
    pub allow_ui_updates: bool,
    pub data: Value,
    pub created_by: String,
    pub updated_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Version history entry for a dashboard (`cr_dashboard_version`, §2.2, R10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardVersionRecord {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub dashboard_uid: String,
    pub version: i32,
    pub parent_version: i32,
    pub restored_from: i32,
    pub message: String,
    pub data: Value,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

/// Datasource configuration record (`cr_datasource`, R3, R5, §4.2).
/// Never holds raw credentials: only `secret_refs` resolved at call time!
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DatasourceRecord {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub uid: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub access: String,
    pub url: String,
    pub is_default: bool,
    pub read_only: bool,
    pub json_data: Value,
    /// Map of logical credential field name -> secretRef identifier (never raw secret values).
    #[serde(default)]
    pub secret_refs: BTreeMap<String, String>,
    /// Outbound host allow-list for this datasource (R3, §3.3, §4.2).
    #[serde(default)]
    pub allow_hosts: Vec<String>,
    /// Pinned SHA-256 digest when `kind` is `"wasm:<id>"` (R5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm_digest: Option<String>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Reusable library panel record (`cr_library_panel`, §2.2, R10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPanelRecord {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_uid: Option<String>,
    pub uid: String,
    pub name: String,
    #[serde(rename = "type")]
    pub panel_type: String,
    pub description: String,
    pub model: Value,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Share type of a public dashboard (`GF pkg/services/publicdashboards/models/models.go`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ShareType {
    #[default]
    Public,
    Email,
}

impl ShareType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Email => "email",
        }
    }
}

/// Public dashboard record (`cr_public_dashboard`, R7).
/// `access_token_hash` stores the SHA-256 hex digest of the 256-bit random access token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicDashboardRecord {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub uid: String,
    pub dashboard_uid: String,
    /// SHA-256 hex digest of the 256-bit random access token (never the raw token in storage).
    #[serde(skip_serializing)]
    pub access_token_hash: String,
    /// Only populated at creation / rotation time so the caller can form the public link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    pub is_enabled: bool,
    pub time_selection_enabled: bool,
    pub annotations_enabled: bool,
    pub show_query_text: bool,
    pub share: ShareType,
    pub created_by: String,
    pub updated_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Per-datasource query cache statistics (`QueryCacheStats` entity set, R10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryCacheStatRecord {
    pub id: String,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub datasource_uid: String,
    pub hits: u64,
    pub misses: u64,
    pub entries: u64,
    pub bytes_used: u64,
}

/// Import report record (`ImportReports` entity set, R2, R10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportReportRecord {
    pub id: String,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub dry_run: bool,
    pub status: String,
    pub dashboard_count: i32,
    pub refused_count: i32,
    pub mapped_count: i32,
    pub summary: Value,
    pub created_at: DateTime<Utc>,
}
