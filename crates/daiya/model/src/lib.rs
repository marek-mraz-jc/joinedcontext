#![forbid(unsafe_code)]
//! `daiya-model`: Grafana classic dashboard JSON serde types (schemaVersion 42, unknown
//! fields kept verbatim), schemaVersion migrations (36..=42 and v2beta1 conversion), CUE-derived
//! validation and §7.1 compatibility verdicts, multi-tenant entity records, `FrameSet` data model,
//! `Authorizer` and `DaiyaStore` traits, and the single operation registry (R1, R9, R10, R10a, R12).

pub mod auth;
pub mod dashboard;
pub mod entities;
pub mod error;
pub mod frames;
pub mod ids;
pub mod limits;
pub mod migration;
pub mod registry;
pub mod store;
pub mod validation;

pub use auth::{
    Action, Authorizer, Decision, GrafanaRole, ObjectTarget, PolicyAuthorizer, RequestCtx,
    RoleBinding, Subject,
};
pub use dashboard::{
    AnnotationContainer, AnnotationQuery, CanonicalJson, DashboardDocument, DashboardIndex,
    DashboardLink, DataSourceRef, DataTransformerConfig, DynamicConfigValue, FieldColor,
    FieldConfig, FieldConfigSource, FieldOverride, GridPos, LibraryPanelRef, MatcherConfig,
    PanelSpec, Threshold, ThresholdsConfig, TimePickerConfig, TimeRangeSpec, VariableModel,
    VariableOption,
};
pub use entities::{
    DashboardRecord, DashboardVersionRecord, DatasourceRecord, FolderRecord, ImportReportRecord,
    LibraryPanelRecord, PublicDashboardRecord, QueryCacheStatRecord, ShareType,
};
pub use error::{CrError, CrResult, ProblemDetails};
pub use frames::{
    ColumnValues, FieldColumn, FieldType, Frame, FrameMeta, FrameSet, Notice, NoticeSeverity,
};
pub use ids::{InstallationId, InstanceId, OrgId, constant_time_eq, sha256_hex};
pub use limits::{InputLimits, OrgQuotas};
pub use migration::{
    DataSourceInfo, DatasourceIndex, LATEST_SCHEMA_VERSION, MIN_SUPPORTED_SCHEMA_VERSION,
    MigrationReport, convert_v2beta1_to_v1, get_schema_version, migrate_to_latest, migrate_v36,
    migrate_v37, migrate_v38, migrate_v39, migrate_v40, migrate_v41, migrate_v42,
};
pub use registry::{
    ActionDef, OperationKind, OperationParam, OperationRegistry, daiya_odata_service,
    daiya_registry,
};
pub use store::DaiyaStore;
pub use validation::{
    FeatureFinding, ValidationReport, Verdict, classify_and_validate, sanitize_markdown_to_html,
};
