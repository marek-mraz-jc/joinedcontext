#![forbid(unsafe_code)]
//! Single operation and entity-set registry (R10, R10a).
//!
//! Declares all 8 entity sets (`Dashboards`, `DashboardVersions`, `Folders`, `Datasources`,
//! `LibraryPanels`, `PublicDashboards`, `QueryCacheStats`, `ImportReports`) once with `odata-table`
//! and declares every bound and unbound OData action/function (`Datasource/Query`,
//! `Datasource/Health`, `Datasource/EditorSchema`, `Datasource/Resource`,
//! `Dashboard/RenderPanel`, `Dashboard/Export`, `Import`, `Plan`, `Apply`,
//! `PublicDashboard/Rotate`, `PublicDashboard/Query`, `Allowed`).
//!
//! CSDL `$metadata` XML/JSON, MCP tool schemas, and CLI subcommands are all generated from this
//! single declaration table, and `check_allowed` runs the exact same [`Authorizer`] decision as
//! the real call.

use crate::auth::{Action, Authorizer, Decision, ObjectTarget, RequestCtx};
use odata_table::{EntityDef, Field, FieldKind, Operation, Rule, Service};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ODATA_NAMESPACE: &str = "JoinedContext.Controlroom";

fn read_propose_approve_rules(prefix: &str) -> Vec<Rule> {
    vec![
        Rule {
            operation: Operation::Read,
            permission: Some(format!("{prefix}:read")),
        },
        Rule {
            operation: Operation::Export,
            permission: Some(format!("{prefix}:read")),
        },
        Rule {
            operation: Operation::Create,
            permission: Some(format!("{prefix}:propose")),
        },
        Rule {
            operation: Operation::Update,
            permission: Some(format!("{prefix}:propose")),
        },
        Rule {
            operation: Operation::Delete,
            permission: Some(format!("{prefix}:delete")),
        },
    ]
}

fn read_only_rules(prefix: &str) -> Vec<Rule> {
    vec![
        Rule {
            operation: Operation::Read,
            permission: Some(format!("{prefix}:read")),
        },
        Rule {
            operation: Operation::Export,
            permission: Some(format!("{prefix}:read")),
        },
    ]
}

fn non_null(name: &str, kind: FieldKind, description: &str) -> Field {
    Field {
        name: name.to_owned(),
        kind,
        description: description.to_owned(),
        nullable: false,
    }
}

/// Declares all 8 OData entity sets of R10 once using `odata-table` (`Service`).
#[must_use]
pub fn controlroom_odata_service() -> Service {
    let dashboards = EntityDef {
        set: "Dashboards".into(),
        type_name: "Dashboard".into(),
        description: "Grafana classic dashboards stored at schemaVersion 42.".into(),
        key: "uid".into(),
        fields: vec![
            non_null("uid", FieldKind::String, "Unique dashboard UID within the organization"),
            non_null("title", FieldKind::String, "Dashboard title"),
            non_null("slug", FieldKind::String, "URL-safe slug derived from title"),
            Field::new("folderUid", FieldKind::String, "Parent folder UID"),
            non_null("panelCount", FieldKind::Integer, "Number of visualization panels"),
            non_null("version", FieldKind::Integer, "Dashboard version number"),
            non_null("schemaVersion", FieldKind::Integer, "Grafana JSON schemaVersion (42)"),
            non_null("isProvisioned", FieldKind::Boolean, "Whether provisioned from code"),
            Field::new("updatedBy", FieldKind::String, "Last actor who updated the dashboard"),
            Field::new("updatedAt", FieldKind::DateTime, "Last update timestamp"),
        ],
        rules: read_propose_approve_rules("dashboards"),
    };

    let dashboard_versions = EntityDef {
        set: "DashboardVersions".into(),
        type_name: "DashboardVersion".into(),
        description: "Immutable version history entries of dashboards.".into(),
        key: "id".into(),
        fields: vec![
            non_null("id", FieldKind::String, "Composite key {dashboardUid}:{version}"),
            non_null("dashboardUid", FieldKind::String, "Dashboard UID"),
            non_null("version", FieldKind::Integer, "Version number"),
            non_null("parentVersion", FieldKind::Integer, "Parent version number"),
            non_null("restoredFrom", FieldKind::Integer, "Version restored from, or 0"),
            non_null("message", FieldKind::String, "Save commit message"),
            Field::new("createdBy", FieldKind::String, "Actor who saved this version"),
            Field::new("createdAt", FieldKind::DateTime, "Timestamp when saved"),
        ],
        rules: read_only_rules("dashboards"),
    };

    let folders = EntityDef {
        set: "Folders".into(),
        type_name: "Folder".into(),
        description: "Dashboard folders nested up to 4 levels.".into(),
        key: "uid".into(),
        fields: vec![
            non_null("uid", FieldKind::String, "Folder UID"),
            Field::new("parentUid", FieldKind::String, "Parent folder UID"),
            non_null("title", FieldKind::String, "Folder title"),
            non_null("version", FieldKind::Integer, "Folder version"),
            Field::new("managedBy", FieldKind::String, "Provisioning manager"),
            Field::new("updatedAt", FieldKind::DateTime, "Last update timestamp"),
        ],
        rules: read_propose_approve_rules("folders"),
    };

    let datasources = EntityDef {
        set: "Datasources".into(),
        type_name: "Datasource".into(),
        description: "Configured datasource plugins and endpoints of the organization.".into(),
        key: "uid".into(),
        fields: vec![
            non_null("uid", FieldKind::String, "Datasource UID"),
            non_null("name", FieldKind::String, "Datasource display name"),
            non_null("type", FieldKind::String, "Plugin kind (prometheus, ngsild, odata, postgres, wasm:<id>)"),
            non_null("access", FieldKind::String, "Access mode (proxy)"),
            non_null("url", FieldKind::String, "Upstream endpoint URL"),
            non_null("isDefault", FieldKind::Boolean, "Whether default datasource for the organization"),
            non_null("readOnly", FieldKind::Boolean, "Whether read-only / provisioned"),
            Field::new("wasmDigest", FieldKind::String, "Pinned SHA-256 digest for WASM plugins"),
            non_null("version", FieldKind::Integer, "Datasource version"),
        ],
        rules: read_propose_approve_rules("datasources"),
    };

    let library_panels = EntityDef {
        set: "LibraryPanels".into(),
        type_name: "LibraryPanel".into(),
        description: "Reusable library panels.".into(),
        key: "uid".into(),
        fields: vec![
            non_null("uid", FieldKind::String, "Library panel UID"),
            Field::new("folderUid", FieldKind::String, "Folder UID"),
            non_null("name", FieldKind::String, "Library panel name"),
            non_null("type", FieldKind::String, "Panel visualization type"),
            Field::new("description", FieldKind::String, "Description"),
            non_null("version", FieldKind::Integer, "Version number"),
        ],
        rules: read_propose_approve_rules("dashboards"),
    };

    let public_dashboards = EntityDef {
        set: "PublicDashboards".into(),
        type_name: "PublicDashboard".into(),
        description: "Public dashboard opt-in configurations (tokens stored hashed).".into(),
        key: "uid".into(),
        fields: vec![
            non_null("uid", FieldKind::String, "Public dashboard UID"),
            non_null("dashboardUid", FieldKind::String, "Target dashboard UID"),
            non_null("isEnabled", FieldKind::Boolean, "Whether public access is enabled"),
            non_null("timeSelectionEnabled", FieldKind::Boolean, "Whether anonymous viewer can pick time range"),
            non_null("annotationsEnabled", FieldKind::Boolean, "Whether annotations are shown"),
            non_null("showQueryText", FieldKind::Boolean, "Whether executed query text is included"),
            non_null("share", FieldKind::String, "Share mode (public)"),
        ],
        rules: read_propose_approve_rules("dashboards"),
    };

    let query_cache_stats = EntityDef {
        set: "QueryCacheStats".into(),
        type_name: "QueryCacheStat".into(),
        description: "Per-datasource query cache hit/miss and memory statistics.".into(),
        key: "id".into(),
        fields: vec![
            non_null("id", FieldKind::String, "Stat entry key"),
            non_null("datasourceUid", FieldKind::String, "Datasource UID"),
            non_null("hits", FieldKind::Integer, "Cache hits"),
            non_null("misses", FieldKind::Integer, "Cache misses"),
            non_null("entries", FieldKind::Integer, "Cached entries"),
            non_null("bytesUsed", FieldKind::Integer, "Bytes used in cache"),
        ],
        rules: read_only_rules("datasources"),
    };

    let import_reports = EntityDef {
        set: "ImportReports".into(),
        type_name: "ImportReport".into(),
        description: "Audit log of dashboard import and check runs.".into(),
        key: "id".into(),
        fields: vec![
            non_null("id", FieldKind::String, "Report ID"),
            non_null("dryRun", FieldKind::Boolean, "Whether run was dry-run"),
            non_null("status", FieldKind::String, "Outcome status"),
            non_null("dashboardCount", FieldKind::Integer, "Dashboards processed"),
            non_null("refusedCount", FieldKind::Integer, "Refused features count"),
            non_null("mappedCount", FieldKind::Integer, "Mapped features count"),
            Field::new("createdAt", FieldKind::DateTime, "Timestamp"),
        ],
        rules: read_only_rules("dashboards"),
    };

    Service {
        namespace: ODATA_NAMESPACE.into(),
        title: "JoinedContext Controlroom OData 4.01 Service".into(),
        sets: vec![
            dashboards,
            dashboard_versions,
            folders,
            datasources,
            library_panels,
            public_dashboards,
            query_cache_stats,
            import_reports,
        ],
    }
}

/// Kind of OData operation in the registry (R10a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OperationKind {
    Action,
    Function,
}

/// Parameter declaration for an operation in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationParam {
    pub name: String,
    pub edm_type: String,
    pub json_type: String,
    pub required: bool,
    pub description: String,
}

/// Declaration of one bound or unbound operation across OData, MCP, and CLI (R10, R10a).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionDef {
    /// Canonical OData name (e.g. `Datasource/Query`, `Dashboard/RenderPanel`, `Import`).
    pub name: String,
    /// MCP tool name (e.g. `controlroom_datasource_query`).
    pub mcp_tool: String,
    /// CLI subcommand name (e.g. `query`, `render-panel`, `import`).
    pub cli_command: String,
    pub kind: OperationKind,
    /// Bound entity type (`Some("Datasource")`, `Some("Dashboard")`, `Some("PublicDashboard")`, or `None` for unbound).
    pub bound_to: Option<String>,
    pub return_type: String,
    pub required_action: Action,
    pub description: String,
    pub params: Vec<OperationParam>,
}

/// Single operation registry holding the entity-set `Service` and all non-CRUD operations (R10, R10a).
#[derive(Debug, Clone)]
pub struct OperationRegistry {
    pub service: Service,
    pub operations: Vec<ActionDef>,
}

/// Builds the complete `controlroom` [`OperationRegistry`] (R10, R10a).
#[must_use]
pub fn controlroom_registry() -> OperationRegistry {
    let operations = vec![
        ActionDef {
            name: "Datasource/Query".into(),
            mcp_tool: "controlroom_datasource_query".into(),
            cli_command: "query".into(),
            kind: OperationKind::Action,
            bound_to: Some("Datasource".into()),
            return_type: "Collection(JoinedContext.Controlroom.FrameSet)".into(),
            required_action: Action::Read,
            description: "Run datasource targets and server-side expressions and return FrameSets.".into(),
            params: vec![
                OperationParam {
                    name: "from".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: false,
                    description: "Start of time range (ISO 8601, epoch ms, or relative like now-6h)".into(),
                },
                OperationParam {
                    name: "to".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: false,
                    description: "End of time range (ISO 8601, epoch ms, or now)".into(),
                },
                OperationParam {
                    name: "queries".into(),
                    edm_type: "Collection(JoinedContext.Controlroom.QueryTarget)".into(),
                    json_type: "array".into(),
                    required: true,
                    description: "List of datasource and __expr__ targets".into(),
                },
            ],
        },
        ActionDef {
            name: "Datasource/Health".into(),
            mcp_tool: "controlroom_datasource_health".into(),
            cli_command: "datasource-health".into(),
            kind: OperationKind::Function,
            bound_to: Some("Datasource".into()),
            return_type: "JoinedContext.Controlroom.HealthStatus".into(),
            required_action: Action::Read,
            description: "Check health of a configured datasource.".into(),
            params: vec![OperationParam {
                name: "uid".into(),
                edm_type: "Edm.String".into(),
                json_type: "string".into(),
                required: true,
                description: "Datasource UID".into(),
            }],
        },
        ActionDef {
            name: "Datasource/EditorSchema".into(),
            mcp_tool: "controlroom_datasource_editor_schema".into(),
            cli_command: "editor-schema".into(),
            kind: OperationKind::Function,
            bound_to: Some("Datasource".into()),
            return_type: "JoinedContext.Controlroom.EditorSchema".into(),
            required_action: Action::Read,
            description: "Return the JSON Schema and field hints for the Portal query editor.".into(),
            params: vec![OperationParam {
                name: "uid".into(),
                edm_type: "Edm.String".into(),
                json_type: "string".into(),
                required: true,
                description: "Datasource UID".into(),
            }],
        },
        ActionDef {
            name: "Datasource/Resource".into(),
            mcp_tool: "controlroom_datasource_resource".into(),
            cli_command: "datasource-resource".into(),
            kind: OperationKind::Function,
            bound_to: Some("Datasource".into()),
            return_type: "JoinedContext.Controlroom.ResourceResponse".into(),
            required_action: Action::Read,
            description: "Fetch query editor completion helpers (metric names, label values, entity types, attributes).".into(),
            params: vec![
                OperationParam {
                    name: "uid".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: true,
                    description: "Datasource UID".into(),
                },
                OperationParam {
                    name: "path".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: true,
                    description: "Resource helper path (e.g. labels, types, attributes)".into(),
                },
            ],
        },
        ActionDef {
            name: "Dashboard/RenderPanel".into(),
            mcp_tool: "controlroom_render_panel".into(),
            cli_command: "render-panel".into(),
            kind: OperationKind::Function,
            bound_to: Some("Dashboard".into()),
            return_type: "JoinedContext.Controlroom.PanelView".into(),
            required_action: Action::Read,
            description: "Evaluate a panel's targets, expressions, and transformations and return its frames, fieldConfig, options, and server-rendered HTML table.".into(),
            params: vec![
                OperationParam {
                    name: "uid".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: true,
                    description: "Dashboard UID".into(),
                },
                OperationParam {
                    name: "panelId".into(),
                    edm_type: "Edm.Int64".into(),
                    json_type: "integer".into(),
                    required: true,
                    description: "Panel numeric ID".into(),
                },
                OperationParam {
                    name: "from".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: false,
                    description: "Start of time range".into(),
                },
                OperationParam {
                    name: "to".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: false,
                    description: "End of time range".into(),
                },
            ],
        },
        ActionDef {
            name: "Dashboard/Export".into(),
            mcp_tool: "controlroom_export_dashboard".into(),
            cli_command: "export".into(),
            kind: OperationKind::Function,
            bound_to: Some("Dashboard".into()),
            return_type: "JoinedContext.Controlroom.ExportedDashboard".into(),
            required_action: Action::Read,
            description: "Export a dashboard as canonical Grafana classic JSON at schemaVersion 42.".into(),
            params: vec![OperationParam {
                name: "uid".into(),
                edm_type: "Edm.String".into(),
                json_type: "string".into(),
                required: true,
                description: "Dashboard UID".into(),
            }],
        },
        ActionDef {
            name: "Import".into(),
            mcp_tool: "controlroom_import".into(),
            cli_command: "import".into(),
            kind: OperationKind::Action,
            bound_to: None,
            return_type: "JoinedContext.Controlroom.ImportReport".into(),
            required_action: Action::Propose,
            description: "Validate, migrate to schemaVersion 42, and import dashboards or provisioning bundles (supports dryRun).".into(),
            params: vec![
                OperationParam {
                    name: "dryRun".into(),
                    edm_type: "Edm.Boolean".into(),
                    json_type: "boolean".into(),
                    required: false,
                    description: "Validate and report without writing".into(),
                },
                OperationParam {
                    name: "dashboard".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "object".into(),
                    required: true,
                    description: "Grafana dashboard JSON document".into(),
                },
                OperationParam {
                    name: "folderUid".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: false,
                    description: "Target folder UID".into(),
                },
            ],
        },
        ActionDef {
            name: "Plan".into(),
            mcp_tool: "controlroom_plan".into(),
            cli_command: "plan".into(),
            kind: OperationKind::Function,
            bound_to: None,
            return_type: "JoinedContext.Controlroom.PlanBundle".into(),
            required_action: Action::Propose,
            description: "Compute diff plan and cryptographic state etag against live organization state.".into(),
            params: vec![OperationParam {
                name: "bundle".into(),
                edm_type: "Edm.String".into(),
                json_type: "object".into(),
                required: true,
                description: "Desired state bundle of folders, datasources, and dashboards".into(),
            }],
        },
        ActionDef {
            name: "Apply".into(),
            mcp_tool: "controlroom_apply".into(),
            cli_command: "apply".into(),
            kind: OperationKind::Action,
            bound_to: None,
            return_type: "JoinedContext.Controlroom.ImportReport".into(),
            required_action: Action::Approve,
            description: "Apply a verified plan bundle (refuses stale plans if live state etag changed).".into(),
            params: vec![OperationParam {
                name: "plan".into(),
                edm_type: "JoinedContext.Controlroom.PlanBundle".into(),
                json_type: "object".into(),
                required: true,
                description: "Plan bundle produced by Plan".into(),
            }],
        },
        ActionDef {
            name: "PublicDashboard/Rotate".into(),
            mcp_tool: "controlroom_rotate_public_dashboard".into(),
            cli_command: "rotate-public-token".into(),
            kind: OperationKind::Action,
            bound_to: Some("PublicDashboard".into()),
            return_type: "JoinedContext.Controlroom.PublicDashboard".into(),
            required_action: Action::Approve,
            description: "Rotate the 256-bit access token of a public dashboard, immediately revoking the previous token.".into(),
            params: vec![OperationParam {
                name: "uid".into(),
                edm_type: "Edm.String".into(),
                json_type: "string".into(),
                required: true,
                description: "PublicDashboard UID or Dashboard UID".into(),
            }],
        },
        ActionDef {
            name: "PublicDashboard/Query".into(),
            mcp_tool: "controlroom_public_dashboard_query".into(),
            cli_command: "public-query".into(),
            kind: OperationKind::Action,
            bound_to: None,
            return_type: "Collection(JoinedContext.Controlroom.FrameSet)".into(),
            required_action: Action::Read,
            description: "Anonymous public dashboard panel query door (runs only saved queries, enforces range/rate caps, strips metadata).".into(),
            params: vec![
                OperationParam {
                    name: "accessToken".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: true,
                    description: "256-bit hex access token".into(),
                },
                OperationParam {
                    name: "panelId".into(),
                    edm_type: "Edm.Int64".into(),
                    json_type: "integer".into(),
                    required: true,
                    description: "Panel numeric ID".into(),
                },
            ],
        },
        ActionDef {
            name: "Allowed".into(),
            mcp_tool: "controlroom_allowed".into(),
            cli_command: "allowed".into(),
            kind: OperationKind::Function,
            bound_to: None,
            return_type: "JoinedContext.Controlroom.Decision".into(),
            required_action: Action::Read,
            description: "Check whether the caller is allowed to execute an operation on a target object using the exact same Authorizer logic as the call.".into(),
            params: vec![
                OperationParam {
                    name: "operation".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: true,
                    description: "Operation name or verb (e.g. Import, Apply, Datasource/Query, read, propose, approve)".into(),
                },
                OperationParam {
                    name: "object".into(),
                    edm_type: "Edm.String".into(),
                    json_type: "string".into(),
                    required: false,
                    description: "Target object spec (e.g. Dashboard:jc-slo, Datasource:prometheus, Folder:general)".into(),
                },
            ],
        },
    ];

    OperationRegistry {
        service: controlroom_odata_service(),
        operations,
    }
}

impl OperationRegistry {
    /// Looks up an operation by its OData name, MCP tool name, or CLI subcommand name.
    #[must_use]
    pub fn find_operation(&self, name: &str) -> Option<&ActionDef> {
        self.operations.iter().find(|op| {
            op.name.eq_ignore_ascii_case(name)
                || op.mcp_tool.eq_ignore_ascii_case(name)
                || op.cli_command.eq_ignore_ascii_case(name)
        })
    }

    /// Evaluates whether `ctx` is allowed to run `operation` on `object_spec` using `authorizer` (R10, R10a).
    #[must_use]
    pub fn check_allowed(
        &self,
        authorizer: &dyn Authorizer,
        ctx: &RequestCtx,
        operation: &str,
        object_spec: Option<&str>,
    ) -> Decision {
        let action = self
            .find_operation(operation)
            .map(|op| op.required_action)
            .or_else(|| Action::parse(operation))
            .unwrap_or(Action::Admin);

        let target = match object_spec.unwrap_or("").split_once(':') {
            Some(("Dashboard" | "dashboard" | "Dashboards", uid)) => ObjectTarget::Dashboard {
                org_id: ctx.org_id,
                dashboard_uid: uid.to_owned(),
                folder_uid: None,
            },
            Some(("Datasource" | "datasource" | "Datasources", uid)) => ObjectTarget::Datasource {
                org_id: ctx.org_id,
                datasource_uid: uid.to_owned(),
            },
            Some(("Folder" | "folder" | "Folders", uid)) => ObjectTarget::Folder {
                org_id: ctx.org_id,
                folder_uid: uid.to_owned(),
            },
            Some(("LibraryPanel" | "libraryPanel" | "LibraryPanels", uid)) => {
                ObjectTarget::LibraryPanel {
                    org_id: ctx.org_id,
                    panel_uid: uid.to_owned(),
                    folder_uid: None,
                }
            }
            Some(("PublicDashboard" | "publicDashboard" | "PublicDashboards", uid)) => {
                ObjectTarget::PublicDashboard {
                    org_id: ctx.org_id,
                    dashboard_uid: uid.to_owned(),
                }
            }
            _ => ObjectTarget::Organization { org_id: ctx.org_id },
        };

        authorizer.check(ctx, action, &target)
    }

    /// Renders the complete OData 4.01 CSDL `$metadata` XML document including all entity sets,
    /// complex types (`FrameSet`, `Frame`, `FieldColumn`, `PanelView`, `EditorSchema`, `HealthStatus`,
    /// `ImportReport`, `PlanBundle`, `Decision`), and all bound/unbound actions and functions of R10a.
    #[must_use]
    pub fn csdl_xml(&self) -> String {
        let ns = &self.service.namespace;
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="utf-8"?>
<edmx:Edmx Version="4.01" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
  <edmx:DataServices>
"#,
        );
        xml.push_str(&format!(
            r#"    <Schema Namespace="{ns}" xmlns="http://docs.oasis-open.org/odata/ns/edm">
"#
        ));

        // EntityTypes
        for set in &self.service.sets {
            xml.push_str(&format!(
                "      <EntityType Name=\"{}\">\n        <Key><PropertyRef Name=\"{}\"/></Key>\n",
                set.type_name, set.key
            ));
            for f in &set.fields {
                let edm_t = match &f.kind {
                    FieldKind::String => "Edm.String".to_owned(),
                    FieldKind::Integer => "Edm.Int64".to_owned(),
                    FieldKind::Number => "Edm.Double".to_owned(),
                    FieldKind::Boolean => "Edm.Boolean".to_owned(),
                    FieldKind::DateTime => "Edm.DateTimeOffset".to_owned(),
                    FieldKind::Object { name, .. } => format!("{ns}.{name}"),
                };
                let nullable = if f.nullable && f.name != set.key {
                    "true"
                } else {
                    "false"
                };
                xml.push_str(&format!(
                    "        <Property Name=\"{}\" Type=\"{}\" Nullable=\"{}\"/>\n",
                    f.name, edm_t, nullable
                ));
            }
            xml.push_str("      </EntityType>\n");
        }

        // ComplexTypes for R10a
        for (ct_name, props) in [
            (
                "FieldColumn",
                &[
                    ("name", "Edm.String"),
                    ("type", "Edm.String"),
                    ("unit", "Edm.String"),
                ][..],
            ),
            (
                "Frame",
                &[
                    ("name", "Edm.String"),
                    ("refId", "Edm.String"),
                    (
                        "fields",
                        "Collection(JoinedContext.Controlroom.FieldColumn)",
                    ),
                ][..],
            ),
            (
                "FrameSet",
                &[
                    ("refId", "Edm.String"),
                    ("frames", "Collection(JoinedContext.Controlroom.Frame)"),
                ][..],
            ),
            (
                "QueryTarget",
                &[("refId", "Edm.String"), ("datasourceUid", "Edm.String")][..],
            ),
            (
                "HealthStatus",
                &[("status", "Edm.String"), ("message", "Edm.String")][..],
            ),
            (
                "EditorSchema",
                &[("kind", "Edm.String"), ("schemaJson", "Edm.String")][..],
            ),
            (
                "ResourceResponse",
                &[("itemsJson", "Edm.String")][..],
            ),
            (
                "PanelView",
                &[
                    ("dashboardUid", "Edm.String"),
                    ("panelId", "Edm.Int64"),
                    ("panelTitle", "Edm.String"),
                    ("panelType", "Edm.String"),
                    ("htmlTable", "Edm.String"),
                    (
                        "frameSets",
                        "Collection(JoinedContext.Controlroom.FrameSet)",
                    ),
                ][..],
            ),
            (
                "ExportedDashboard",
                &[("uid", "Edm.String"), ("dashboardJson", "Edm.String")][..],
            ),
            (
                "PlanBundle",
                &[
                    ("baseEtag", "Edm.String"),
                    ("hasChanges", "Edm.Boolean"),
                    ("planJson", "Edm.String"),
                ][..],
            ),
            (
                "Decision",
                &[
                    ("allowed", "Edm.Boolean"),
                    ("reason", "Edm.String"),
                    ("visible", "Edm.Boolean"),
                ][..],
            ),
        ] {
            xml.push_str(&format!("      <ComplexType Name=\"{ct_name}\">\n"));
            for (p_name, p_type) in props {
                xml.push_str(&format!(
                    "        <Property Name=\"{p_name}\" Type=\"{p_type}\"/>\n"
                ));
            }
            xml.push_str("      </ComplexType>\n");
        }

        // Actions and Functions (R10a)
        for op in &self.operations {
            let tag = match op.kind {
                OperationKind::Action => "Action",
                OperationKind::Function => "Function",
            };
            let is_bound = if op.bound_to.is_some() {
                "true"
            } else {
                "false"
            };
            let short_name = op
                .name
                .rsplit_once('/')
                .map_or(op.name.as_str(), |(_, n)| n);
            xml.push_str(&format!(
                "      <{tag} Name=\"{short_name}\" FullName=\"{}\" IsBound=\"{is_bound}\">\n",
                op.name
            ));
            if let Some(bound) = &op.bound_to {
                xml.push_str(&format!(
                    "        <Parameter Name=\"bindingParameter\" Type=\"{ns}.{bound}\"/>\n"
                ));
            }
            for param in &op.params {
                xml.push_str(&format!(
                    "        <Parameter Name=\"{}\" Type=\"{}\" Nullable=\"{}\"/>\n",
                    param.name,
                    param.edm_type,
                    !param.required
                ));
            }
            xml.push_str(&format!(
                "        <ReturnType Type=\"{}\"/>\n      </{tag}>\n",
                op.return_type
            ));
        }

        // EntityContainer
        xml.push_str("      <EntityContainer Name=\"Service\">\n");
        for set in &self.service.sets {
            xml.push_str(&format!(
                "        <EntitySet Name=\"{}\" EntityType=\"{ns}.{}\"/>\n",
                set.set, set.type_name
            ));
        }
        for op in &self.operations {
            let import_tag = match op.kind {
                OperationKind::Action => "ActionImport",
                OperationKind::Function => "FunctionImport",
            };
            let short_name = op.name.replace('/', "_");
            let attr = match op.kind {
                OperationKind::Action => "Action",
                OperationKind::Function => "Function",
            };
            xml.push_str(&format!(
                "        <{import_tag} Name=\"{short_name}\" {attr}=\"{ns}.{}\"/>\n",
                op.name
            ));
        }
        xml.push_str("      </EntityContainer>\n    </Schema>\n  </edmx:DataServices>\n</edmx:Edmx>\n");
        xml
    }

    /// Renders the OData 4.01 CSDL `$metadata` as JSON.
    #[must_use]
    pub fn csdl_json(&self) -> Value {
        let sets: Vec<Value> = self
            .service
            .sets
            .iter()
            .map(|s| {
                json!({
                    "name": s.set,
                    "entityType": format!("{}.{}", self.service.namespace, s.type_name),
                    "key": s.key,
                    "description": s.description,
                })
            })
            .collect();
        let ops: Vec<Value> = self
            .operations
            .iter()
            .map(|op| {
                json!({
                    "name": op.name,
                    "mcpTool": op.mcp_tool,
                    "cliCommand": op.cli_command,
                    "kind": op.kind,
                    "boundTo": op.bound_to,
                    "returnType": op.return_type,
                    "requiredAction": op.required_action,
                    "description": op.description,
                    "parameters": op.params,
                })
            })
            .collect();
        json!({
            "$Version": "4.01",
            "namespace": self.service.namespace,
            "entitySets": sets,
            "operations": ops,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_declares_every_entity_set_and_operation_of_r10_and_r10a() {
        let reg = controlroom_registry();
        let set_names: Vec<&str> = reg.service.sets.iter().map(|s| s.set.as_str()).collect();
        assert_eq!(
            set_names,
            [
                "Dashboards",
                "DashboardVersions",
                "Folders",
                "Datasources",
                "LibraryPanels",
                "PublicDashboards",
                "QueryCacheStats",
                "ImportReports",
            ]
        );

        let xml = reg.csdl_xml();
        for required in [
            "Dashboards",
            "DashboardVersions",
            "Folders",
            "Datasources",
            "LibraryPanels",
            "PublicDashboards",
            "QueryCacheStats",
            "ImportReports",
            "Datasource/Query",
            "Datasource/Health",
            "Datasource/EditorSchema",
            "Datasource/Resource",
            "Dashboard/RenderPanel",
            "Dashboard/Export",
            "Import",
            "Plan",
            "Apply",
            "PublicDashboard/Rotate",
            "PublicDashboard/Query",
            "Allowed",
            "ComplexType Name=\"FrameSet\"",
        ] {
            assert!(xml.contains(required), "CSDL missing {required}");
        }
    }
}
