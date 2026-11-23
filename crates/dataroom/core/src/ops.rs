#![forbid(unsafe_code)]
//! Single operation registry (`C11`) shared by REST, OData, MCP tools (`research-ckan.md` §9)
//! and CLI subcommands.
//!
//! Every operation declares its name, MCP tool name, segment, required [`Action`], object target
//! kind, and input/output JSON schemas once.

use crate::auth::{Action, Authorizer, Decision, ObjectTarget, RequestCtx};
use crate::ckan_compat::{
    dataset_from_ckan, dataset_to_ckan, distribution_from_ckan, distribution_to_ckan,
    resource_view_from_ckan, resource_view_to_ckan, validate_view_payload,
};
use crate::error::{DataroomError, Result};
use crate::model::{
    validate_slug, ActivityRow, Catalogue, CataloguePlugin, Dataset, HarvestSource, Publisher,
    RecordState, Visibility,
};
use crate::odata_defs::scope_for_context;
use crate::store::Store;
use jc_time::TimestampTz;
use odata_table::{query as odata_query, QueryOptions};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// Execution context for registered operations.
pub struct OpContext {
    pub store: Arc<dyn Store>,
    pub authorizer: Arc<dyn Authorizer>,
    /// Optional hook for feed/dataset SHACL validation provided by `dataroom-shacl`.
    pub validator: Option<Arc<dyn DatasetValidator>>,
    /// Optional hook for triggering a harvest run provided by `dataroom-harvest`.
    pub harvester: Option<Arc<dyn HarvestRunner>>,
    /// Harvest sources are written by the Portal's reconciler alone (ODP-11, in `jc`): the
    /// direct `harvest_source.propose` is refused.
    pub sources_by_portal: bool,
    /// The installation branding a catalogue layers its own over, as `GET /api/v1/branding`
    /// answers it; `None` answers the neutral defaults.
    pub branding: Option<BrandingOf>,
}

/// The installation branding of a catalogue (the host's live addresses, colours and domain).
pub type BrandingOf = Arc<dyn Fn(&Catalogue) -> crate::BrandingBlock + Send + Sync>;

/// Hook trait so `dataroom-core` operations can invoke SHACL validation without a circular
/// dependency on `dataroom-shacl`.
pub trait DatasetValidator: Send + Sync {
    /// Makes the catalogue's stored uploaded profiles (`Store::list_profiles`) usable before a
    /// validation, whatever this process saw uploaded.
    fn load_stored_profiles(
        &self,
        _catalogue: &Catalogue,
        _stored: &[crate::model::ProfileRecord],
    ) {
    }

    /// Refuses a profile upload that takes a built-in's id or whose shapes do not parse.
    fn check_profile(&self, _profile: &crate::model::ProfileRecord) -> Result<()> {
        Ok(())
    }

    /// Refuses profiles a catalogue may not take up: not installed, needing a publisher it does
    /// not declare, or, where a Portal manages the choice (`managed`), a built-in one.
    fn check_profile_choice(
        &self,
        _catalogue: &Catalogue,
        _added: &[&str],
        _managed: bool,
    ) -> Result<()> {
        Ok(())
    }

    fn validate_dataset(
        &self,
        catalogue: &Catalogue,
        dataset: &Dataset,
        publisher: Option<&Publisher>,
    ) -> Result<Vec<crate::model::ShaclValidationReport>>;

    fn check_catalogue_feed(
        &self,
        catalogue: &Catalogue,
        datasets: &[Dataset],
        publishers: &BTreeMap<String, Publisher>,
        profile_id: Option<&str>,
    ) -> Result<Value>;

    /// Takes an uploaded profile under `catalogue` alone and dry-runs it over `datasets`: the
    /// record to store and the report. A published version is never replaced (C17).
    fn upload_profile(
        &self,
        _profile: crate::model::ProfileRecord,
        _catalogue: &Catalogue,
        _datasets: &[Dataset],
        _publishers: &BTreeMap<String, Publisher>,
    ) -> Result<(crate::model::ProfileRecord, Value)> {
        Err(DataroomError::NotSupported(
            "this validator takes no uploaded profile".into(),
        ))
    }

    /// What `datasets` would answer to the profile version: the report and each dataset's.
    fn dry_run_profile(
        &self,
        _profile_id: &str,
        _version: &str,
        _catalogue: &Catalogue,
        _datasets: &[Dataset],
        _publishers: &BTreeMap<String, Publisher>,
    ) -> Result<(Value, Vec<crate::model::ShaclValidationReport>)> {
        Err(DataroomError::NotSupported(
            "this validator dry-runs no profile".into(),
        ))
    }
}

/// Hook trait so `dataroom-core` operations can start a harvest run without a circular
/// dependency on `dataroom-harvest`.
pub trait HarvestRunner: Send + Sync + 'static {
    /// Runs `source` to its end and answers the finished run.
    fn run_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        store: &'a dyn Store,
        source: &'a HarvestSource,
    ) -> crate::store::BoxFuture<'a, Result<crate::model::HarvestRun>>;

    /// Starts a run of `source` and answers it once its lease is taken (state running);
    /// `harvest_run.get` reads how it ends. This default runs it inline to its end.
    fn start_source(
        self: Arc<Self>,
        ctx: RequestCtx,
        store: Arc<dyn Store>,
        source: HarvestSource,
    ) -> crate::store::BoxFuture<'static, Result<crate::model::HarvestRun>> {
        Box::pin(async move { self.run_source(&ctx, store.as_ref(), &source).await })
    }

    /// One guarded read of `source`'s first page that writes nothing (ODP-15): the answer is
    /// one word, never what the remote said.
    fn test_source<'a>(
        &'a self,
        _ctx: &'a RequestCtx,
        _store: &'a dyn Store,
        _source: &'a HarvestSource,
    ) -> crate::store::BoxFuture<'a, Result<SourceTest>> {
        Box::pin(std::future::ready(Err(DataroomError::NotSupported(
            "this harvest runner tests no source".into(),
        ))))
    }

    /// A dry run of `source` (T-7416): the test's word, and when it is `ok` the first
    /// [`PREVIEW_MAX`] records as a run would parse them, writing nothing.
    fn preview_source<'a>(
        &'a self,
        _ctx: &'a RequestCtx,
        _store: &'a dyn Store,
        _source: &'a HarvestSource,
    ) -> crate::store::BoxFuture<'a, Result<SourcePreview>> {
        Box::pin(std::future::ready(Err(DataroomError::NotSupported(
            "this harvest runner previews no source".into(),
        ))))
    }
}

/// Most records a source preview shows (T-7416).
pub const PREVIEW_MAX: usize = 20;

/// A dry run of a source (T-7416): the test's word and, when `ok`, the parsed records it would
/// write. Records hold only parsed metadata, never an address, status, header or raw body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourcePreview {
    pub outcome: SourceTest,
    pub records: Vec<PreviewRecord>,
    /// The listing went past the preview's time or byte cap; the records may be fewer.
    pub partial: bool,
}

/// One record of a [`SourcePreview`], as a dataset card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewRecord {
    pub name: String,
    pub title: String,
    /// At most 280 characters.
    pub description: String,
    pub keywords: Vec<String>,
    pub formats: Vec<String>,
    pub licence: Option<String>,
}

impl PreviewRecord {
    /// The card of `dataset` in `lang`.
    pub fn of(dataset: &crate::model::Dataset, lang: &str) -> Self {
        let text = |t: &crate::model::Texts| {
            crate::model::text_in_lang(t, Some(lang))
                .unwrap_or_default()
                .to_owned()
        };
        let mut description = text(&dataset.description);
        if let Some((cut, _)) = description.char_indices().nth(280) {
            description.truncate(cut);
            description.push('…');
        }
        let mut formats: Vec<String> = dataset
            .distributions
            .iter()
            .filter_map(|d| d.format.clone().or_else(|| d.media_type.clone()))
            .collect();
        formats.sort();
        formats.dedup();
        let keywords = dataset
            .keywords
            .get(lang)
            .or_else(|| dataset.keywords.values().next())
            .map(|k| k.iter().take(5).cloned().collect())
            .unwrap_or_default();
        Self {
            name: dataset.name.clone(),
            title: Some(text(&dataset.title))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| dataset.name.clone()),
            description,
            keywords,
            formats,
            licence: dataset
                .licence_id
                .clone()
                .or_else(|| dataset.licence_url.clone()),
        }
    }
}

/// What a source test found, one word each (ODP-15): no address, status line, header or body of
/// the remote answer is ever part of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SourceTest {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "unreachable")]
    Unreachable,
    #[serde(rename = "refused host")]
    RefusedHost,
    #[serde(rename = "not the expected format")]
    NotTheExpectedFormat,
    #[serde(rename = "too large")]
    TooLarge,
}

/// Most runs `harvest_run.list` answers (ODP-15).
pub const RUN_LIST_MAX: usize = 50;

/// The one word a stored run error reads as (ODP-15): the run log keeps the detail, a list
/// shows none of it.
pub fn run_error_word(error: &str) -> &'static str {
    let e = error.to_ascii_lowercase();
    if e.contains("past its budget") {
        "over budget"
    } else if e.contains("deadline") {
        "past the time cap"
    } else if e.contains("cap of") {
        "too large"
    } else if e.contains("answered http")
        || e.starts_with("fetch ")
        || e.starts_with("read ")
        || e.contains("dns resolution")
    {
        "unreachable"
    } else if e.contains("is refused")
        || e.contains("allowedhosts")
        || e.contains("allowed_outbound_hosts")
        || e.contains("allow-list")
        || e.contains("credentials")
        || e.contains("internal address")
        || e.contains("not a valid url")
        || e.contains("redirect")
        || e.contains("cross-organization")
    {
        "refused host"
    } else if e.contains("invalid") || e.contains("parse") || e.contains("json") {
        "not the expected format"
    } else {
        "failed"
    }
}

/// Metadata of one registered operation (C11).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationDef {
    /// Canonical operation name (`catalogue.list`, `dataset.search`, ...).
    pub name: &'static str,
    /// MCP tool name from `research-ckan.md` §9 (`jc_catalogue_list`, `jc_dataset_search`, ...).
    pub mcp_tool: &'static str,
    /// CLI subcommand path (`catalogue-list`, `dataset-search`, ...).
    pub cli_command: &'static str,
    /// Segment (`data-read`, `publish`, `operate`, `admin`).
    pub segment: &'static str,
    /// Required action verb.
    pub permission: Action,
    /// Target kind (`Catalogue`, `Dataset`, `Publisher`, `HarvestSource`, `CataloguePlugin`).
    pub target_kind: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// JSON Schema of input.
    pub input_schema: Value,
    /// JSON Schema of output.
    pub output_schema: Value,
}

/// Returns all registered operations in `dataroom` (matching `research-ckan.md` §9 and `00-contract.md` C11).
pub fn all_operations() -> Vec<OperationDef> {
    vec![
        OperationDef {
            name: "catalogue.list",
            mcp_tool: "jc_catalogue_list",
            cli_command: "catalogue-list",
            segment: "publish",
            permission: Action::Read,
            target_kind: "Catalogue",
            description: "List catalogues the caller may read: id, host, languages, profiles.",
            input_schema: json!({"type": "object", "properties": {"org": {"type": "string"}}}),
            output_schema: json!({"type": "object", "properties": {"catalogues": {"type": "array"}}}),
        },
        OperationDef {
            name: "catalogue.get",
            mcp_tool: "jc_catalogue_get",
            cli_command: "catalogue-get",
            segment: "publish",
            permission: Action::Read,
            target_kind: "Catalogue",
            description: "Read catalogue settings, profiles, languages and theme tokens.",
            input_schema: json!({"type": "object", "properties": {"catalogue": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "catalogue.propose",
            mcp_tool: "jc_catalogue_propose",
            cli_command: "catalogue-propose",
            segment: "publish",
            permission: Action::Propose,
            target_kind: "Catalogue",
            description: "Create or update catalogue settings and active profile pins.",
            input_schema: json!({"type": "object", "required": ["catalogue"], "properties": {"catalogue": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "dataset.search",
            mcp_tool: "jc_dataset_search",
            cli_command: "dataset-search",
            segment: "data-read",
            permission: Action::Read,
            target_kind: "Dataset",
            description: "Search datasets with full-text query, facet filters and visibility filtering.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "q": {"type": "string"},
                    "publisher": {"type": "string"},
                    "theme": {"type": "string"},
                    "format": {"type": "string"},
                    "licence": {"type": "string"},
                    "page": {"type": "integer", "minimum": 1}
                }
            }),
            output_schema: json!({"type": "object", "properties": {"total": {"type": "integer"}, "datasets": {"type": "array"}}}),
        },
        OperationDef {
            name: "dataset.get",
            mcp_tool: "jc_dataset_get",
            cli_command: "dataset-get",
            segment: "data-read",
            permission: Action::Read,
            target_kind: "Dataset",
            description: "Read one dataset by name or id with its distributions and views.",
            input_schema: json!({"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "dataset.propose",
            mcp_tool: "jc_dataset_propose",
            cli_command: "dataset-propose",
            segment: "publish",
            permission: Action::Propose,
            target_kind: "Dataset",
            description: "Create or update a dataset, running SHACL validation against active profiles.",
            input_schema: json!({"type": "object", "required": ["dataset"], "properties": {"dataset": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "dataset.withdraw",
            mcp_tool: "jc_dataset_withdraw",
            cli_command: "dataset-withdraw",
            segment: "publish",
            permission: Action::Delete,
            target_kind: "Dataset",
            description: "Withdraw a dataset from the catalogue.",
            input_schema: json!({"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "dataset.purge",
            mcp_tool: "jc_dataset_purge",
            cli_command: "dataset-purge",
            segment: "admin",
            permission: Action::Admin,
            target_kind: "Dataset",
            description: "Remove a dataset for good; its activity stays.",
            input_schema: json!({"type": "object", "required": ["name"], "additionalProperties": false, "properties": {"name": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "resource.propose",
            // The Portal's `jc_resource_propose` changes a manifest: a client holding both servers
            // must not mistake one for the other.
            mcp_tool: "jc_catalogue_resource_propose",
            cli_command: "catalogue-resource-propose",
            segment: "publish",
            permission: Action::Propose,
            target_kind: "Dataset",
            description: "Add a resource to a dataset or change one, running SHACL validation against active profiles.",
            input_schema: json!({"type": "object", "required": ["resource"], "additionalProperties": false, "properties": {"dataset": {"type": "string"}, "id": {"type": "string"}, "resource": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "resource.delete",
            // The Portal's `jc_resource_delete` changes a manifest: a client holding both servers
            // must not mistake one for the other.
            mcp_tool: "jc_catalogue_resource_delete",
            cli_command: "catalogue-resource-delete",
            segment: "publish",
            permission: Action::Delete,
            target_kind: "Dataset",
            description: "Remove a resource from a dataset, running SHACL validation against active profiles.",
            input_schema: json!({"type": "object", "required": ["id"], "additionalProperties": false, "properties": {"id": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "resource_view.propose",
            mcp_tool: "jc_resource_view_propose",
            cli_command: "resource-view-propose",
            segment: "publish",
            permission: Action::Propose,
            target_kind: "Dataset",
            description: "Add a view to a resource or change one.",
            input_schema: json!({"type": "object", "required": ["view"], "additionalProperties": false, "properties": {"resource": {"type": "string"}, "id": {"type": "string"}, "view": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "resource_view.delete",
            mcp_tool: "jc_resource_view_delete",
            cli_command: "resource-view-delete",
            segment: "publish",
            permission: Action::Delete,
            target_kind: "Dataset",
            description: "Remove a view of a resource.",
            input_schema: json!({"type": "object", "required": ["id"], "additionalProperties": false, "properties": {"id": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "publisher.list",
            mcp_tool: "jc_publisher_list",
            cli_command: "publisher-list",
            segment: "publish",
            permission: Action::Read,
            target_kind: "Publisher",
            description: "List publishers in the catalogue.",
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object", "properties": {"publishers": {"type": "array"}}}),
        },
        OperationDef {
            name: "publisher.propose",
            mcp_tool: "jc_publisher_propose",
            cli_command: "publisher-propose",
            segment: "publish",
            permission: Action::Propose,
            target_kind: "Publisher",
            description: "Create or update a publisher in the catalogue.",
            input_schema: json!({"type": "object", "required": ["publisher"], "properties": {"publisher": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "harvest_source.list",
            mcp_tool: "jc_harvest_source_list",
            cli_command: "harvest-source-list",
            segment: "publish",
            permission: Action::Read,
            target_kind: "HarvestSource",
            description: "List harvest sources in the catalogue.",
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object", "properties": {"sources": {"type": "array"}}}),
        },
        OperationDef {
            name: "harvest_source.propose",
            mcp_tool: "jc_harvest_source_propose",
            cli_command: "harvest-source-propose",
            segment: "publish",
            permission: Action::Propose,
            target_kind: "HarvestSource",
            description: "Create or update a harvest source.",
            input_schema: json!({"type": "object", "required": ["source"], "properties": {"source": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "harvest_run.start",
            mcp_tool: "jc_harvest_run_start",
            cli_command: "harvest-run-start",
            segment: "operate",
            permission: Action::Harvest,
            target_kind: "HarvestSource",
            description: "Start a harvest run for a harvest source: answers the run in state running once the source is held (a run already holding it answers a conflict); harvest_run.get reads how it ends.",
            input_schema: json!({"type": "object", "required": ["source"], "properties": {"source": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "harvest_run.get",
            mcp_tool: "jc_harvest_run_get",
            cli_command: "harvest-run-get",
            segment: "operate",
            permission: Action::Read,
            target_kind: "HarvestSource",
            description: "Read harvest run counts and skipped items with reasons.",
            input_schema: json!({"type": "object", "required": ["run"], "properties": {"run": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "harvest_run.logs",
            mcp_tool: "jc_harvest_run_logs",
            cli_command: "harvest-run-logs",
            segment: "operate",
            permission: Action::Read,
            target_kind: "HarvestSource",
            description: "Read structured log lines with fingerprints for a harvest run.",
            input_schema: json!({"type": "object", "required": ["run"], "properties": {"run": {"type": "string"}, "level": {"type": "string"}}}),
            output_schema: json!({"type": "object", "properties": {"logs": {"type": "array"}}}),
        },
        OperationDef {
            name: "harvest_run.list",
            mcp_tool: "jc_harvest_run_list",
            cli_command: "harvest-run-list",
            segment: "operate",
            permission: Action::Read,
            target_kind: "HarvestSource",
            description: "List the catalogue's last harvest runs, newest first, at most 50, with counts and one error word.",
            input_schema: json!({"type": "object", "properties": {"source": {"type": "string"}}}),
            output_schema: json!({"type": "object", "properties": {"runs": {"type": "array"}}}),
        },
        OperationDef {
            name: "harvest_source.test",
            mcp_tool: "jc_harvest_source_test",
            cli_command: "harvest-source-test",
            segment: "operate",
            permission: Action::Harvest,
            target_kind: "HarvestSource",
            description: "Read a harvest source's first page once through the harvest guard, writing nothing; answers one word. With preview, also the first 20 records a run would parse, still writing nothing.",
            input_schema: json!({"type": "object", "required": ["source"], "properties": {"source": {"type": "string"}, "preview": {"type": "boolean"}}}),
            output_schema: json!({"type": "object", "properties": {"outcome": {"type": "string"}, "records": {"type": "array"}, "partial": {"type": "boolean"}}}),
        },
        OperationDef {
            name: "plugin.list",
            mcp_tool: "jc_catalogue_plugin_list",
            cli_command: "plugin-list",
            segment: "admin",
            permission: Action::Read,
            target_kind: "CataloguePlugin",
            description: "List plugins enabled on the catalogue with their digests and grants.",
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object", "properties": {"plugins": {"type": "array"}}}),
        },
        OperationDef {
            name: "plugin.propose",
            mcp_tool: "jc_catalogue_plugin_propose",
            cli_command: "plugin-propose",
            segment: "admin",
            permission: Action::Propose,
            target_kind: "CataloguePlugin",
            description: "Enable or update a plugin on the catalogue.",
            input_schema: json!({"type": "object", "required": ["plugin"], "properties": {"plugin": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "catalogue.activity",
            mcp_tool: "jc_catalogue_activity",
            cli_command: "catalogue-activity",
            segment: "operate",
            permission: Action::Read,
            target_kind: "Catalogue",
            description: "Read activity audit log rows for the catalogue.",
            input_schema: json!({"type": "object", "properties": {"object": {"type": "string"}, "limit": {"type": "integer"}}}),
            output_schema: json!({"type": "object", "properties": {"activity": {"type": "array"}}}),
        },
        OperationDef {
            name: "catalogue.feed_check",
            mcp_tool: "jc_catalogue_feed_check",
            cli_command: "feed-check",
            segment: "publish",
            permission: Action::Read,
            target_kind: "Catalogue",
            description: "Validate the catalogue feed against SHACL shapes of its active profiles.",
            input_schema: json!({"type": "object", "properties": {"profile": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "branding.get",
            mcp_tool: "jc_catalogue_branding_get",
            cli_command: "branding-get",
            segment: "publish",
            permission: Action::Read,
            target_kind: "Catalogue",
            description: "Read the catalogue's effective branding block.",
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "branding.propose",
            mcp_tool: "jc_catalogue_branding_propose",
            cli_command: "branding-propose",
            segment: "admin",
            permission: Action::Admin,
            target_kind: "Catalogue",
            description: "Update the catalogue's branding block and write an audit activity row.",
            input_schema: json!({"type": "object", "required": ["branding"], "properties": {"branding": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "profile.list",
            mcp_tool: "jc_catalogue_profile_list",
            cli_command: "profile-list",
            segment: "publish",
            permission: Action::Read,
            target_kind: "Profile",
            description: "List DCAT-AP profiles available and active on the catalogue.",
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object", "properties": {"profiles": {"type": "array"}}}),
        },
        OperationDef {
            name: "profile.propose",
            mcp_tool: "jc_catalogue_profile_propose",
            cli_command: "profile-propose",
            segment: "admin",
            permission: Action::Propose,
            target_kind: "Profile",
            description: "Upload or update a versioned DCAT-AP profile and write an audit activity row.",
            input_schema: json!({"type": "object", "required": ["profile"], "properties": {"profile": {"type": "object"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "profile.activate",
            mcp_tool: "jc_profile_activate",
            cli_command: "profile-activate",
            segment: "admin",
            permission: Action::Approve,
            target_kind: "Profile",
            description: "Pin a profile version, optionally as the main profile, after a dry run over the catalogue.",
            input_schema: json!({"type": "object", "required": ["id", "version"], "additionalProperties": false, "properties": {"id": {"type": "string"}, "version": {"type": "string"}, "makeMain": {"type": "boolean"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "publisher.merge",
            mcp_tool: "jc_publisher_merge",
            cli_command: "publisher-merge",
            segment: "admin",
            permission: Action::Admin,
            target_kind: "Publisher",
            description: "Merge a source publisher into a target publisher, moving its datasets and recording an audit row.",
            input_schema: json!({"type": "object", "required": ["source", "target"], "properties": {"source": {"type": "string"}, "target": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "publisher.withdraw",
            mcp_tool: "jc_publisher_withdraw",
            cli_command: "publisher-withdraw",
            segment: "admin",
            permission: Action::Delete,
            target_kind: "Publisher",
            description: "Withdraw a publisher so its datasets leave public feeds, recording an audit row.",
            input_schema: json!({"type": "object", "required": ["publisher"], "properties": {"publisher": {"type": "string"}}}),
            output_schema: json!({"type": "object"}),
        },
        OperationDef {
            name: "allowed",
            mcp_tool: "jc_catalogue_allowed",
            cli_command: "allowed",
            segment: "data-read",
            permission: Action::Read,
            target_kind: "Catalogue",
            description: "Ask whether the caller may perform an operation on an object without running it.",
            input_schema: json!({"type": "object", "required": ["operation"], "properties": {"operation": {"type": "string"}, "object": {"type": "string"}}}),
            output_schema: json!({"type": "object", "properties": {"allowed": {"type": "boolean"}, "reason": {"type": "string"}}}),
        },
    ]
}

/// Finds an operation by its canonical name, MCP tool name, or CLI subcommand name.
pub fn find_operation(name: &str) -> Option<OperationDef> {
    all_operations()
        .into_iter()
        .find(|op| op.name == name || op.mcp_tool == name || op.cli_command == name)
}

/// Scopes a proposed record to the request: `orgId` and `catalogueId` always come from the
/// request context (a caller never picks the tenant), and the given server-kept timestamps are
/// filled with now when the caller left them out.
fn scoped_record(mut raw: Value, req_ctx: &RequestCtx, timestamps: &[&str]) -> Result<Value> {
    let Some(obj) = raw.as_object_mut() else {
        return Err(DataroomError::field_error("body", "expected a JSON object"));
    };
    obj.insert("orgId".into(), Value::String(req_ctx.org_id.to_string()));
    obj.insert(
        "catalogueId".into(),
        Value::String(req_ctx.catalogue_id.to_string()),
    );
    let now = Value::String(TimestampTz::now().to_rfc3339());
    for key in timestamps {
        obj.entry((*key).to_owned()).or_insert_with(|| now.clone());
    }
    Ok(raw)
}

fn enforce_decision(decision: Decision) -> Result<()> {
    if decision.allowed {
        Ok(())
    } else if !decision.visible {
        Err(not_found())
    } else {
        Err(DataroomError::Forbidden(decision.reason))
    }
}

/// A record that is missing and one the caller may not know of answer alike (C11).
fn not_found() -> DataroomError {
    DataroomError::NotFound("Not found".into())
}

/// The input of an operation, its unknown fields refused.
fn op_input<T: serde::de::DeserializeOwned>(input: Value) -> Result<T> {
    serde_json::from_value(input)
        .map_err(|e| DataroomError::field_error("input", format!("invalid input: {e}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordId {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordName {
    name: String,
}

/// `resource.propose`: `dataset` names the dataset a new resource joins, `id` the stored one a
/// change is merged into.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceProposal {
    dataset: Option<String>,
    id: Option<String>,
    resource: serde_json::Map<String, Value>,
}

/// `resource_view.propose`: `resource` names the resource a new view joins, `id` the stored one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewProposal {
    resource: Option<String>,
    id: Option<String>,
    view: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProfileActivation {
    id: String,
    version: String,
    #[serde(default)]
    make_main: bool,
}

/// The caller's decision on `ds`, with its publisher's record.
async fn authorize_dataset(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    ds: &Dataset,
    action: Action,
) -> Result<Option<Publisher>> {
    let publisher = op_ctx
        .store
        .get_publisher(req_ctx, &ds.publisher_id)
        .await?;
    let target = ObjectTarget::from_dataset(ds, publisher.as_ref());
    enforce_decision(op_ctx.authorizer.check(req_ctx, action, &target))?;
    Ok(publisher)
}

async fn own_catalogue(op_ctx: &OpContext, req_ctx: &RequestCtx) -> Result<Catalogue> {
    op_ctx
        .store
        .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
        .await?
        .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))
}

/// The one way an operation stores a dataset record: a Public one conforms to the main profile,
/// its reports are kept and the change commits with its activity row. `expected` is the stored
/// record's `modified` the change was built on: another write since answers a conflict.
async fn store_dataset(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    cat: &Catalogue,
    publisher: Option<&Publisher>,
    (ds, expected): (Dataset, Option<TimestampTz>),
    operation: &str,
) -> Result<Dataset> {
    let mut reports = Vec::new();
    if let Some(validator) = &op_ctx.validator {
        validator.load_stored_profiles(cat, &op_ctx.store.list_profiles(req_ctx).await?);
        reports = validator.validate_dataset(cat, &ds, publisher)?;
        if let Some(report) = reports.iter().find(|r| {
            r.profile_id == cat.main_profile && !r.conforms && ds.visibility == Visibility::Public
        }) {
            let details = report
                .findings
                .iter()
                .map(|f| {
                    format!(
                        "{}: {}",
                        f.result_path.as_deref().unwrap_or(&f.focus_node),
                        crate::model::text_in_lang(&f.message, Some("en"))
                            .unwrap_or(&f.source_constraint_component)
                    )
                })
                .collect();
            return Err(DataroomError::ShaclViolation {
                profile: report.profile_id.clone(),
                violations: report.violation_count,
                details,
            });
        }
    }
    // The store fills in the id it saves the dataset under.
    let row = activity_row(
        req_ctx,
        operation,
        "Dataset",
        "",
        format!("Saved dataset {}", ds.name),
        json!({"name": ds.name}),
    );
    let saved = op_ctx
        .store
        .save_dataset(req_ctx, ds, expected, row)
        .await?;
    // Reports are derived from the record and written again by its next save.
    for mut report in reports {
        report.dataset_id.clone_from(&saved.id);
        op_ctx.store.upsert_shacl_report(req_ctx, report).await?;
    }
    Ok(saved)
}

/// Every dataset of the catalogue and its publishers, by id and by name: what a profile dry run
/// reads.
async fn dry_run_records(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
) -> Result<(Vec<Dataset>, BTreeMap<String, Publisher>)> {
    let datasets = op_ctx.store.list_datasets(req_ctx).await?;
    let publishers = op_ctx
        .store
        .list_publishers(req_ctx)
        .await?
        .into_iter()
        .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
        .collect();
    Ok((datasets, publishers))
}

/// Whether `dataset.get` (and `allowed` about it) shows `ds` to the caller: the caller may read
/// it, and, as `package_show` does, a draft, a withdrawn dataset, one of a withdrawn publisher
/// or one hidden by the publish gate only to its editors ([`crate::auth::dataset_visible`]).
async fn dataset_shown(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    ds: &Dataset,
    pub_rec: Option<&Publisher>,
) -> Result<bool> {
    let hidden = op_ctx
        .store
        .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
        .await?
        .is_some_and(|c| c.hidden_datasets.contains_key(&ds.name));
    Ok(crate::auth::dataset_visible(
        op_ctx.authorizer.as_ref(),
        req_ctx,
        ds,
        pub_rec,
        hidden,
    ))
}

/// The catalogue as `req_ctx` may read it: the publish gate's hidden datasets (names and
/// reasons) and the outbound host allow-list go only to callers who may propose the catalogue.
fn public_catalogue(op_ctx: &OpContext, req_ctx: &RequestCtx, mut cat: Catalogue) -> Catalogue {
    let target = ObjectTarget::Catalogue {
        org_id: cat.org_id,
        catalogue_id: cat.id,
    };
    if !op_ctx
        .authorizer
        .check(req_ctx, Action::Propose, &target)
        .allowed
    {
        cat.hidden_datasets.clear();
        cat.allowed_outbound_hosts.clear();
    }
    cat
}

/// Authorizes saving `proposed` (C16). An update is decided on the stored record, never on the
/// grants the caller sends along; creating a publisher needs Propose without any editor grant;
/// changing who may edit a publisher (its editor grants), the publisher it is merged into, the
/// instance that declared it (but a caller's claim of an undeclared one) or the IRI its datasets
/// are attributed to needs Admin on it; and changing its state needs Delete, as
/// `publisher.withdraw` does.
pub fn authorize_publisher_write(
    authorizer: &dyn Authorizer,
    req_ctx: &RequestCtx,
    existing: Option<&Publisher>,
    proposed: &Publisher,
) -> Result<()> {
    let base = match existing {
        Some(p) => p.clone(),
        None => Publisher {
            editor_grants: Vec::new(),
            merged_into: None,
            ..proposed.clone()
        },
    };
    let target = ObjectTarget::from_publisher(&base);
    enforce_decision(authorizer.check(req_ctx, Action::Propose, &target))?;
    if proposed.state != base.state {
        enforce_decision(authorizer.check(req_ctx, Action::Delete, &target))?;
    }
    // An instance declares an undeclared publisher as its own with Propose (G2): the name is
    // the caller's verified client, never another's.
    let claims_own = base.declared_by_instance.is_none()
        && proposed.declared_by_instance == req_ctx.subject.client_id;
    if proposed.editor_grants != base.editor_grants
        || proposed.merged_into != base.merged_into
        || (proposed.declared_by_instance != base.declared_by_instance && !claims_own)
        || proposed.uri != base.uri
    {
        enforce_decision(authorizer.check(req_ctx, Action::Admin, &target))?;
    }
    Ok(())
}

/// A dataset's declarations (CMP-08, SEC-17): personal data is one of the two statements, and
/// the IRI of the body it is attributed to is set by an Admin or by the Portal's publisher,
/// which carries the Endpoint's manifest. Anyone else would publish as another body.
fn check_declarations(
    authorizer: &dyn Authorizer,
    req_ctx: &RequestCtx,
    target: &ObjectTarget,
    existing: Option<&Dataset>,
    ds: &Dataset,
) -> Result<()> {
    if let Some(stated) = ds.extras.get(crate::model::PERSONAL_DATA) {
        let allowed = [
            crate::model::PERSONAL_DATA_CONTAINS,
            crate::model::PERSONAL_DATA_NONE,
        ];
        if !allowed.contains(&stated.as_str()) {
            return Err(DataroomError::field_error(
                crate::model::PERSONAL_DATA,
                format!("must be one of {}", allowed.join(", ")),
            ));
        }
    }
    let iri = |d: &Dataset| d.publisher.as_ref().and_then(|a| a.uri.clone());
    let proposed = iri(ds);
    if proposed.is_some()
        && proposed != existing.and_then(iri)
        && !is_portal_publisher(req_ctx)
        && !authorizer.check(req_ctx, Action::Admin, target).allowed
    {
        return Err(DataroomError::Forbidden(
            "publisher_uri: only an administrator declares the body a dataset is attributed to"
                .into(),
        ));
    }
    Ok(())
}

/// The Portal's publisher: the bare principal, or the one bound to this context's catalogue.
fn is_portal_publisher(req_ctx: &RequestCtx) -> bool {
    let (actor, portal) = (req_ctx.actor_name(), crate::auth::PORTAL_PUBLISHER);
    actor == portal || actor == format!("{portal}:{}", req_ctx.catalogue_id)
}

/// The stored publisher a proposal addresses: by its id when it carries one, else by name,
/// without following merges (the record written is the record decided on).
async fn proposed_publisher_record(
    store: &dyn Store,
    req_ctx: &RequestCtx,
    id: &str,
    name: &str,
) -> Result<Option<Publisher>> {
    Ok(store.list_publishers(req_ctx).await?.into_iter().find(|p| {
        if id.is_empty() {
            p.name == name
        } else {
            p.id == id
        }
    }))
}

/// What `catalogue.propose` may change: its host and name, profiles, publisher and contact,
/// branding, hidden datasets and outbound rules only with Admin on the catalogue (each has its own Admin route); a
/// Propose-only caller's values for them are replaced by the stored ones. A new host or name is
/// validated, so `base_url` and robots.txt only ever carry a host name.
fn keep_admin_fields(cat: &mut Catalogue, stored: Catalogue, admin: bool) -> Result<()> {
    if !admin {
        cat.name = stored.name;
        cat.host = stored.host;
        cat.main_profile = stored.main_profile;
        cat.active_profiles = stored.active_profiles;
        cat.publisher_uri = stored.publisher_uri;
        cat.publisher_name = stored.publisher_name;
        cat.contact_point = stored.contact_point;
        cat.theme = stored.theme;
        cat.branding = stored.branding;
        cat.hidden_datasets = stored.hidden_datasets;
        cat.allow_cross_org_harvest = stored.allow_cross_org_harvest;
        cat.allowed_outbound_hosts = stored.allowed_outbound_hosts;
    } else {
        if cat.name != stored.name {
            validate_slug("catalogue.name", &cat.name)?;
        }
        if cat.host != stored.host && !crate::branding::is_host_name(&cat.host) {
            return Err(DataroomError::field_error(
                "catalogue.host",
                "must be a host name such as data.example.org",
            ));
        }
    }
    cat.deleted = stored.deleted;
    cat.portal_base = stored.portal_base;
    cat.left_hosts = stored.left_hosts;
    Ok(())
}

/// One activity row for a change (C13).
fn activity_row(
    req_ctx: &RequestCtx,
    operation: &str,
    object_type: &str,
    object_id: &str,
    summary: String,
    details: Value,
) -> ActivityRow {
    ActivityRow {
        org_id: req_ctx.org_id,
        catalogue_id: req_ctx.catalogue_id,
        id: format!("act-{}", Uuid::now_v7()),
        at: TimestampTz::now(),
        actor: req_ctx.actor_name().to_owned(),
        operation: operation.into(),
        object_type: object_type.into(),
        object_id: object_id.into(),
        summary,
        details,
    }
}

/// Runs `attempt` again, three times at most, while it answers a conflict: for a change of one
/// member (a resource, the state, the branding) that is built anew from each fresh read, so a
/// concurrent write to another member never fails the caller (S11-25).
async fn retry_on_conflict<F, Fut, T>(mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut left = 3;
    loop {
        match attempt().await {
            Err(DataroomError::Conflict(_)) if left > 1 => left -= 1,
            done => return done,
        }
    }
}

/// Executes a registered operation through the single authorization and validation pipeline (C11).
/// Why `harvest_source.propose` is refused where a Portal holds the sources (ODP-11).
pub const SOURCES_BY_PORTAL: &str = "harvest sources are written by the Portal (ODP-11): add the source to a Portal's spec.sources and propose the Change";

pub async fn execute_operation(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    op_name: &str,
    input: Value,
) -> Result<Value> {
    // Boxed: every route, tool and action that calls an operation would otherwise hold the whole
    // registry's future in its own frame.
    Box::pin(dispatch(op_ctx, req_ctx, op_name, input)).await
}

async fn dispatch(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    op_name: &str,
    input: Value,
) -> Result<Value> {
    let def = find_operation(op_name)
        .ok_or_else(|| DataroomError::NotFound(format!("unknown operation: {op_name}")))?;

    match def.name {
        "catalogue.list" => {
            let cats = op_ctx.store.list_catalogues(req_ctx.org_id).await?;
            let visible: Vec<Catalogue> = cats
                .into_iter()
                .filter(|c| c.deleted.is_none())
                .filter(|c| {
                    let target = ObjectTarget::Catalogue {
                        org_id: c.org_id,
                        catalogue_id: c.id,
                    };
                    op_ctx
                        .authorizer
                        .check(req_ctx, Action::Read, &target)
                        .allowed
                })
                .collect();
            let visible: Vec<Catalogue> = visible
                .into_iter()
                .map(|c| public_catalogue(op_ctx, req_ctx, c))
                .collect();
            Ok(json!({ "catalogues": visible }))
        }
        "catalogue.get" => {
            let target = ObjectTarget::Catalogue {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let cat = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;
            Ok(serde_json::to_value(public_catalogue(op_ctx, req_ctx, cat)).unwrap_or(Value::Null))
        }
        "catalogue.propose" => {
            let target = ObjectTarget::Catalogue {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Propose, &target))?;
            let cat_val = input.get("catalogue").cloned().unwrap_or(input);
            let mut cat: Catalogue = serde_json::from_value(cat_val).map_err(|e| {
                DataroomError::field_error("catalogue", format!("invalid catalogue: {e}"))
            })?;
            cat.org_id = req_ctx.org_id;
            cat.id = req_ctx.catalogue_id;
            cat.modified = TimestampTz::now();
            let stored = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;
            let admin = op_ctx
                .authorizer
                .check(req_ctx, Action::Admin, &target)
                .allowed;
            let held: Vec<String> = stored
                .active_profiles
                .iter()
                .map(|pin| pin.profile_id.clone())
                .chain([stored.main_profile.clone()])
                .collect();
            let read = stored.modified;
            keep_admin_fields(&mut cat, stored, admin)?;
            let added: Vec<&str> = cat
                .active_profiles
                .iter()
                .map(|pin| pin.profile_id.as_str())
                .chain([cat.main_profile.as_str()])
                .filter(|id| !held.iter().any(|h| h == id))
                .collect();
            if let Some(validator) = op_ctx.validator.as_ref().filter(|_| !added.is_empty()) {
                validator.load_stored_profiles(&cat, &op_ctx.store.list_profiles(req_ctx).await?);
                validator.check_profile_choice(&cat, &added, op_ctx.sources_by_portal)?;
            }
            let row = activity_row(
                req_ctx,
                "catalogue.propose",
                "Catalogue",
                &cat.id.to_string(),
                format!("Updated catalogue {}", cat.name),
                json!({}),
            );
            let saved = op_ctx.store.save_catalogue(cat, Some(read), row).await?;
            Ok(serde_json::to_value(saved).unwrap_or(Value::Null))
        }
        "dataset.search" => {
            let all_ds = op_ctx.store.list_datasets(req_ctx).await?;
            let publishers = op_ctx.store.list_publishers(req_ctx).await?;
            let pub_map: BTreeMap<String, Publisher> = publishers
                .into_iter()
                .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
                .collect();

            let q = input
                .get("q")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase();
            let pub_filter = input.get("publisher").and_then(Value::as_str);
            let theme_filter = input.get("theme").and_then(Value::as_str);
            let format_filter = input.get("format").and_then(Value::as_str);
            let licence_filter = input.get("licence").and_then(Value::as_str);

            let hidden = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .map(|c| c.hidden_datasets)
                .unwrap_or_default();
            let mut visible_ids = Vec::new();
            for ds in &all_ds {
                if ds.state != RecordState::Active {
                    continue;
                }
                let pub_rec = pub_map.get(&ds.publisher_id);
                if pub_rec.is_some_and(|p| p.state == RecordState::Withdrawn) {
                    continue;
                }
                let target = ObjectTarget::from_dataset(ds, pub_rec);
                if !op_ctx
                    .authorizer
                    .check(req_ctx, Action::Read, &target)
                    .allowed
                {
                    continue;
                }
                // The publish gate's hidden datasets stay with their editors (`jc_hide`).
                if hidden.contains_key(&ds.name)
                    && !op_ctx
                        .authorizer
                        .check(req_ctx, Action::Propose, &target)
                        .allowed
                {
                    continue;
                }
                if !q.is_empty() {
                    let hay = format!(
                        "{} {} {} {}",
                        ds.name,
                        ds.title.values().cloned().collect::<Vec<_>>().join(" "),
                        ds.description
                            .values()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" "),
                        ds.tags.join(" ")
                    )
                    .to_lowercase();
                    if !q.split_whitespace().all(|term| hay.contains(term)) {
                        continue;
                    }
                }
                if let Some(pf) = pub_filter {
                    let matches_pub = ds.publisher_id == pf
                        || pub_rec.is_some_and(|p| p.name == pf || p.id == pf);
                    if !matches_pub {
                        continue;
                    }
                }
                if let Some(tf) = theme_filter {
                    if !ds
                        .themes
                        .iter()
                        .any(|t| t == tf || crate::vocab::theme_code(t).eq_ignore_ascii_case(tf))
                    {
                        continue;
                    }
                }
                if let Some(ff) = format_filter {
                    if !ds.distributions.iter().any(|d| {
                        d.format
                            .as_deref()
                            .is_some_and(|f| f.eq_ignore_ascii_case(ff))
                    }) {
                        continue;
                    }
                }
                if let Some(lf) = licence_filter {
                    if ds.licence_id.as_deref() != Some(lf) {
                        continue;
                    }
                }
                visible_ids.push(ds.id.clone());
            }

            // Use shared `odata-table` (C14) over the `Datasets` entity set.
            let table = op_ctx.store.odata_table(req_ctx, "Datasets").await?;
            let scope = scope_for_context(
                req_ctx,
                op_ctx.authorizer.as_ref(),
                "Datasets",
                Some(visible_ids),
            );
            let page = input
                .get("page")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1);
            let options = QueryOptions {
                top: Some(20),
                skip: Some((page - 1).saturating_mul(20)),
                count: Some(true),
                ..QueryOptions::default()
            };
            let answer = odata_query(table.as_ref(), &options, &scope, 100).await?;
            Ok(json!({
                "total": answer.count.unwrap_or(0),
                "page": page,
                "datasets": answer.rows,
            }))
        }
        "dataset.get" => {
            let name = input
                .get("name")
                .or_else(|| input.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("name", "name is required"))?;
            let ds = op_ctx
                .store
                .get_dataset(req_ctx, name)
                .await?
                .ok_or_else(|| DataroomError::NotFound(format!("dataset '{name}' not found")))?;
            let pub_rec = op_ctx
                .store
                .get_publisher(req_ctx, &ds.publisher_id)
                .await?;
            if !dataset_shown(op_ctx, req_ctx, &ds, pub_rec.as_ref()).await? {
                return Err(DataroomError::NotFound(format!(
                    "dataset '{name}' not found"
                )));
            }
            let mut views = BTreeMap::new();
            for dist in &ds.distributions {
                let v = op_ctx.store.list_resource_views(req_ctx, &dist.id).await?;
                if !v.is_empty() {
                    views.insert(dist.id.clone(), v);
                }
            }
            // In the catalogue's language, as package_show answers.
            let lang = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .map(|c| c.default_language)
                .unwrap_or_else(|| "en".into());
            Ok(dataset_to_ckan(&ds, pub_rec.as_ref(), &views, &lang))
        }
        "dataset.propose" => {
            // A caller who can never write is refused before any lookup, so a name it cannot
            // see answers as a free one does (the authorizer's own write rules, steps 3 and 4).
            if req_ctx.subject.read_only {
                return Err(DataroomError::Forbidden("connector is read-only".into()));
            }
            if !req_ctx.subject.is_member_of(req_ctx.org_id) {
                return Err(DataroomError::Forbidden(
                    "caller is not a member of the organization".into(),
                ));
            }
            let expected_read = input
                .get("dataset")
                .and(input.get("expectedModified"))
                .cloned();
            let raw_ds = input.get("dataset").cloned().unwrap_or(input);
            let non_empty = |key: &str| {
                raw_ds
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            let existing = match non_empty("id").or_else(|| non_empty("name")) {
                Some(n) => op_ctx.store.get_dataset(req_ctx, &n).await?,
                None => None,
            };
            // An update is decided on the stored record first: a caller who may not edit (or
            // see) the dataset cannot overwrite it by naming it under a publisher of its own.
            if let Some(ex) = &existing {
                let ex_pub = op_ctx
                    .store
                    .get_publisher(req_ctx, &ex.publisher_id)
                    .await?;
                let target = ObjectTarget::from_dataset(ex, ex_pub.as_ref());
                enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Propose, &target))?;
            }
            // The record the name names is decided too: the id and the name may name two.
            if let Some(n) = non_empty("name") {
                if let Some(named) = op_ctx.store.get_dataset(req_ctx, &n).await? {
                    if existing.as_ref().is_none_or(|ex| ex.id != named.id) {
                        let named_pub = op_ctx
                            .store
                            .get_publisher(req_ctx, &named.publisher_id)
                            .await?;
                        let target = ObjectTarget::from_dataset(&named, named_pub.as_ref());
                        enforce_decision(op_ctx.authorizer.check(
                            req_ctx,
                            Action::Propose,
                            &target,
                        ))?;
                    }
                }
            }
            let cat = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;

            let mut ds: Dataset = if raw_ds.get("publisherId").is_some() {
                let mut parsed: Dataset = serde_json::from_value(raw_ds).map_err(|e| {
                    DataroomError::field_error("dataset", format!("invalid dataset: {e}"))
                })?;
                parsed.org_id = req_ctx.org_id;
                parsed.catalogue_id = req_ctx.catalogue_id;
                if let (true, Some(ex)) = (parsed.id.is_empty(), &existing) {
                    parsed.id = ex.id.clone();
                }
                parsed
            } else {
                crate::ckan_compat::check_ckan_visibility(&raw_ds)?;
                dataset_from_ckan(
                    req_ctx.org_id,
                    req_ctx.catalogue_id,
                    &raw_ds,
                    existing.as_ref(),
                    &cat.default_language,
                )
            };

            ds.validate_basic()?;
            let pub_rec = op_ctx
                .store
                .get_publisher(req_ctx, &ds.publisher_id)
                .await?
                .ok_or_else(|| {
                    DataroomError::field_error("owner_org", "Organization does not exist")
                })?;
            // Canonicalize publisher_id to the resolved publisher name (following merges).
            ds.publisher_id = pub_rec.name.clone();

            let target = ObjectTarget::from_dataset(&ds, Some(&pub_rec));
            let decision = op_ctx.authorizer.check(req_ctx, Action::Propose, &target);
            if !decision.allowed {
                return Err(DataroomError::Forbidden(decision.reason));
            }
            check_declarations(
                op_ctx.authorizer.as_ref(),
                req_ctx,
                &target,
                existing.as_ref(),
                &ds,
            )?;

            // `expectedModified` names the version a caller read before building the record (a
            // package_patch merges into it); else the version read above.
            let stored_version = existing.as_ref().map(|ex| ex.modified);
            if let Some(read) = expected_read {
                let read: TimestampTz = serde_json::from_value(read).map_err(|_| {
                    DataroomError::field_error(
                        "expectedModified",
                        "an RFC 3339 time, e.g. 2026-11-22T10:00:00Z",
                    )
                })?;
                if stored_version != Some(read) {
                    return Err(crate::store::changed_since_read("dataset", &ds.name));
                }
            }
            let saved = store_dataset(
                op_ctx,
                req_ctx,
                &cat,
                Some(&pub_rec),
                (ds, stored_version),
                "dataset.propose",
            )
            .await?;
            Ok(dataset_to_ckan(
                &saved,
                Some(&pub_rec),
                &BTreeMap::new(),
                &cat.default_language,
            ))
        }
        "dataset.withdraw" => {
            let name = input
                .get("name")
                .or_else(|| input.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("name", "name is required"))?;
            let withdrawn = retry_on_conflict(|| async {
                let mut ds = op_ctx
                    .store
                    .get_dataset(req_ctx, name)
                    .await?
                    .ok_or_else(not_found)?;
                authorize_dataset(op_ctx, req_ctx, &ds, Action::Delete).await?;
                let read = ds.modified;
                ds.state = RecordState::Withdrawn;
                ds.modified = TimestampTz::now();
                let row = activity_row(
                    req_ctx,
                    "dataset.withdraw",
                    "Dataset",
                    "",
                    format!("Withdrew dataset {}", ds.name),
                    json!({"name": ds.name}),
                );
                op_ctx
                    .store
                    .save_dataset(req_ctx, ds, Some(read), row)
                    .await
            })
            .await?;
            Ok(json!({ "id": withdrawn.id, "name": withdrawn.name, "state": "deleted" }))
        }
        "dataset.purge" => dataset_purge(op_ctx, req_ctx, input).await,
        "resource.propose" => resource_propose(op_ctx, req_ctx, input).await,
        "resource.delete" => resource_delete(op_ctx, req_ctx, input).await,
        "resource_view.propose" => resource_view_propose(op_ctx, req_ctx, input).await,
        "resource_view.delete" => resource_view_delete(op_ctx, req_ctx, input).await,
        "publisher.list" => {
            let mut publishers = op_ctx.store.list_publishers(req_ctx).await?;
            // Outside the organization: the active publishers (as organization_list), without
            // the principals that may edit them.
            if !req_ctx.subject.is_member_of(req_ctx.org_id) {
                publishers.retain(|p| p.state != RecordState::Withdrawn);
                for p in &mut publishers {
                    p.editor_grants.clear();
                }
            }
            Ok(json!({ "publishers": publishers }))
        }
        "publisher.propose" => {
            let mut raw_pub = scoped_record(
                input.get("publisher").cloned().unwrap_or(input),
                req_ctx,
                &["created", "modified"],
            )?;
            let str_of = |v: &Value, key: &str| {
                v.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let existing = proposed_publisher_record(
                op_ctx.store.as_ref(),
                req_ctx,
                &str_of(&raw_pub, "id"),
                &str_of(&raw_pub, "name"),
            )
            .await?;
            // An update that leaves out the editor grants, the merge target or the declaring
            // instance keeps the stored ones.
            if let (Some(ex), Some(obj)) = (&existing, raw_pub.as_object_mut()) {
                for (key, stored) in [
                    ("editorGrants", json!(ex.editor_grants)),
                    ("mergedInto", json!(ex.merged_into)),
                    ("declaredByInstance", json!(ex.declared_by_instance)),
                ] {
                    if !obj.contains_key(key) {
                        obj.insert(key.into(), stored);
                    }
                }
            }
            let mut publisher: Publisher = serde_json::from_value(raw_pub).map_err(|e| {
                DataroomError::field_error("publisher", format!("invalid publisher: {e}"))
            })?;
            publisher.org_id = req_ctx.org_id;
            publisher.catalogue_id = req_ctx.catalogue_id;
            authorize_publisher_write(
                op_ctx.authorizer.as_ref(),
                req_ctx,
                existing.as_ref(),
                &publisher,
            )?;
            // A new record's id or name never names a stored publisher: datasets name their
            // publisher by it, so that one's datasets would resolve to this record.
            if existing.is_none() {
                for key in [&publisher.id, &publisher.name] {
                    if !key.is_empty() && op_ctx.store.get_publisher(req_ctx, key).await?.is_some()
                    {
                        return Err(DataroomError::Conflict(format!(
                            "a publisher with id or name '{key}' exists"
                        )));
                    }
                }
            }
            // The write lands on the record it was decided on, as long as that one is unchanged.
            if let (Some(ex), true) = (&existing, publisher.id.is_empty()) {
                publisher.id.clone_from(&ex.id);
            }
            let row = activity_row(
                req_ctx,
                "publisher.propose",
                "Publisher",
                &publisher.id,
                format!("Saved publisher {}", publisher.name),
                json!({"name": publisher.name}),
            );
            let read = existing.as_ref().map(|ex| ex.modified);
            let saved = op_ctx
                .store
                .save_publisher(req_ctx, publisher, read, row)
                .await?;
            Ok(serde_json::to_value(saved).unwrap_or(Value::Null))
        }
        "harvest_source.list" => {
            let target = ObjectTarget::HarvestSource {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                source_id: "*".into(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let sources = op_ctx.store.list_harvest_sources(req_ctx).await?;
            Ok(json!({ "sources": sources }))
        }
        "harvest_source.propose" if op_ctx.sources_by_portal => {
            Err(DataroomError::NotSupported(SOURCES_BY_PORTAL.into()))
        }
        "harvest_source.propose" => {
            let raw_src = scoped_record(
                input.get("source").cloned().unwrap_or(input),
                req_ctx,
                &["modified"],
            )?;
            let mut source: HarvestSource = serde_json::from_value(raw_src).map_err(|e| {
                DataroomError::field_error("source", format!("invalid harvest source: {e}"))
            })?;
            source.org_id = req_ctx.org_id;
            source.catalogue_id = req_ctx.catalogue_id;
            let target = ObjectTarget::HarvestSource {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                source_id: source.id.clone(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Propose, &target))?;
            let row = activity_row(
                req_ctx,
                "harvest_source.propose",
                "HarvestSource",
                &source.id,
                format!("Saved harvest source {}", source.name),
                json!({"name": source.name, "url": source.url}),
            );
            let saved = op_ctx
                .store
                .save_harvest_source(req_ctx, source, row)
                .await?;
            Ok(serde_json::to_value(saved).unwrap_or(Value::Null))
        }
        "harvest_run.start" => {
            let source_name = input
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("source", "source is required"))?;
            let source = op_ctx
                .store
                .get_harvest_source(req_ctx, source_name)
                .await?
                .ok_or_else(|| {
                    DataroomError::NotFound(format!("harvest source '{source_name}' not found"))
                })?;
            let target = ObjectTarget::HarvestSource {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                source_id: source.id.clone(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Harvest, &target))?;
            let Some(harvester) = &op_ctx.harvester else {
                return Err(DataroomError::NotSupported(
                    "harvest runner not configured".into(),
                ));
            };
            let run = harvester
                .clone()
                .start_source(req_ctx.clone(), op_ctx.store.clone(), source)
                .await?;
            Ok(serde_json::to_value(run).unwrap_or(Value::Null))
        }
        "harvest_run.get" => {
            let run_id = input
                .get("run")
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("run", "run is required"))?;
            let target = ObjectTarget::HarvestSource {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                source_id: "*".into(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let run = op_ctx
                .store
                .get_harvest_run(req_ctx, run_id)
                .await?
                .ok_or_else(|| {
                    DataroomError::NotFound(format!("harvest run '{run_id}' not found"))
                })?;
            Ok(serde_json::to_value(run).unwrap_or(Value::Null))
        }
        "harvest_run.logs" => {
            let run_id = input
                .get("run")
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("run", "run is required"))?;
            let level = input.get("level").and_then(Value::as_str);
            let target = ObjectTarget::HarvestSource {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                source_id: "*".into(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let run = op_ctx
                .store
                .get_harvest_run(req_ctx, run_id)
                .await?
                .ok_or_else(|| {
                    DataroomError::NotFound(format!("harvest run '{run_id}' not found"))
                })?;
            let logs: Vec<_> = run
                .logs
                .into_iter()
                .filter(|l| level.is_none_or(|lvl| l.level.eq_ignore_ascii_case(lvl)))
                .collect();
            Ok(json!({ "run": run.id, "logs": logs }))
        }
        "harvest_run.list" => {
            let target = ObjectTarget::HarvestSource {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                source_id: "*".into(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let source = match input.get("source").and_then(Value::as_str) {
                Some(name) => Some(
                    op_ctx
                        .store
                        .get_harvest_source(req_ctx, name)
                        .await?
                        .ok_or_else(|| {
                            DataroomError::NotFound(format!("harvest source '{name}' not found"))
                        })?
                        .id,
                ),
                None => None,
            };
            let mut runs = op_ctx
                .store
                .list_harvest_runs(req_ctx, source.as_deref())
                .await?;
            runs.retain(|r| r.org_id == req_ctx.org_id && r.catalogue_id == req_ctx.catalogue_id);
            runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
            runs.truncate(RUN_LIST_MAX);
            let runs: Vec<Value> = runs
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "source": r.source_id,
                        "state": r.state.as_str(),
                        "startedAt": r.started_at,
                        "finishedAt": r.finished_at,
                        "created": r.created_count,
                        "updated": r.updated_count,
                        "unchanged": r.unchanged_count,
                        "withdrawn": r.withdrawn_count,
                        "skipped": r.skipped.len(),
                        "error": r.error.as_deref().map(run_error_word),
                    })
                })
                .collect();
            Ok(json!({ "runs": runs }))
        }
        "harvest_source.test" => {
            let source_name = input
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("source", "source is required"))?;
            let source = op_ctx
                .store
                .get_harvest_source(req_ctx, source_name)
                .await?
                .ok_or_else(|| {
                    DataroomError::NotFound(format!("harvest source '{source_name}' not found"))
                })?;
            let target = ObjectTarget::HarvestSource {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                source_id: source.id.clone(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Harvest, &target))?;
            let Some(harvester) = &op_ctx.harvester else {
                return Err(DataroomError::NotSupported(
                    "harvest runner not configured".into(),
                ));
            };
            if input.get("preview").and_then(Value::as_bool) == Some(true) {
                let preview = harvester
                    .preview_source(req_ctx, op_ctx.store.as_ref(), &source)
                    .await?;
                return Ok(serde_json::to_value(preview).unwrap_or(Value::Null));
            }
            let outcome = harvester
                .test_source(req_ctx, op_ctx.store.as_ref(), &source)
                .await?;
            Ok(json!({ "outcome": outcome }))
        }
        "plugin.list" => {
            let mut plugins = op_ctx.store.list_plugins(req_ctx).await?;
            // Which plugins run is public (as status_show's extensions); their settings are for
            // those who may change them.
            let target = ObjectTarget::Plugin {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                plugin_name: String::new(),
            };
            if !op_ctx
                .authorizer
                .check(req_ctx, Action::Propose, &target)
                .allowed
            {
                for p in &mut plugins {
                    p.config = Value::Null;
                    p.allowed_hosts.clear();
                }
            }
            Ok(json!({ "plugins": plugins }))
        }
        "plugin.propose" => {
            let raw_pl = scoped_record(
                input.get("plugin").cloned().unwrap_or(input),
                req_ctx,
                &["modified"],
            )?;
            let mut plugin: CataloguePlugin = serde_json::from_value(raw_pl).map_err(|e| {
                DataroomError::field_error("plugin", format!("invalid plugin: {e}"))
            })?;
            plugin.org_id = req_ctx.org_id;
            plugin.catalogue_id = req_ctx.catalogue_id;
            let target = ObjectTarget::Plugin {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                plugin_name: plugin.name.clone(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Propose, &target))?;
            let row = activity_row(
                req_ctx,
                "plugin.propose",
                "CataloguePlugin",
                &plugin.name,
                format!("Saved plugin {} {}", plugin.name, plugin.version),
                json!({"name": plugin.name, "version": plugin.version, "enabled": plugin.enabled}),
            );
            let saved = op_ctx.store.save_plugin(req_ctx, plugin, row).await?;
            Ok(serde_json::to_value(saved).unwrap_or(Value::Null))
        }
        "catalogue.activity" => {
            if !req_ctx.subject.is_member_of(req_ctx.org_id) {
                return Err(DataroomError::NotFound("not found".into()));
            }
            let obj = input.get("object").and_then(Value::as_str);
            let limit = input
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .min(200) as usize;
            let rows = op_ctx.store.list_activity(req_ctx, obj, limit).await?;
            Ok(json!({ "activity": rows }))
        }
        "catalogue.feed_check" => {
            let cat = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;
            let profile = input.get("profile").and_then(Value::as_str);
            let all_ds = op_ctx.store.list_datasets(req_ctx).await?;
            let publishers = op_ctx.store.list_publishers(req_ctx).await?;
            let pub_map: BTreeMap<String, Publisher> = publishers
                .into_iter()
                .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
                .collect();
            // The datasets the public feed carries: no jc_hide one, none of a withdrawn publisher.
            let public_ds: Vec<Dataset> = all_ds
                .into_iter()
                .filter(|d| {
                    d.state == RecordState::Active
                        && d.visibility == Visibility::Public
                        && !cat.hidden_datasets.contains_key(&d.name)
                        && !pub_map
                            .get(&d.publisher_id)
                            .is_some_and(|p| p.state == RecordState::Withdrawn)
                })
                .collect();
            if let Some(validator) = &op_ctx.validator {
                validator.load_stored_profiles(&cat, &op_ctx.store.list_profiles(req_ctx).await?);
                validator.check_catalogue_feed(&cat, &public_ds, &pub_map, profile)
            } else {
                Ok(json!({
                    "conforms": true,
                    "datasetsChecked": public_ds.len(),
                    "violations": 0,
                }))
            }
        }
        "branding.get" => {
            let target = ObjectTarget::Catalogue {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let cat = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;
            let installation = op_ctx.branding.as_ref().map(|of| of(&cat));
            let eff = cat.effective_branding(installation.as_ref());
            Ok(serde_json::to_value(eff).unwrap_or(Value::Null))
        }
        "branding.propose" => {
            let target = ObjectTarget::Catalogue {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Admin, &target))?;
            let br_val = input.get("branding").cloned().unwrap_or(input);
            let saved = retry_on_conflict(|| async {
                let mut cat = op_ctx
                    .store
                    .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                    .await?
                    .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;
                let read = cat.modified;
                cat.branding = Some(br_val.clone());
                cat.modified = TimestampTz::now();
                let row = activity_row(
                    req_ctx,
                    "branding.propose",
                    "Catalogue",
                    &cat.id.to_string(),
                    format!("Updated branding on catalogue {}", cat.name),
                    json!({}),
                );
                op_ctx.store.save_catalogue(cat, Some(read), row).await
            })
            .await?;
            let installation = op_ctx.branding.as_ref().map(|of| of(&saved));
            let eff = saved.effective_branding(installation.as_ref());
            Ok(serde_json::to_value(eff).unwrap_or(Value::Null))
        }
        "profile.list" => {
            let target = ObjectTarget::Profile {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                profile_id: "*".into(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let profiles = op_ctx.store.list_profiles(req_ctx).await?;
            Ok(json!({ "profiles": profiles }))
        }
        "profile.propose" => profile_propose(op_ctx, req_ctx, input).await,
        "profile.activate" => profile_activate(op_ctx, req_ctx, input).await,
        "publisher.merge" => {
            let source_id = input
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("source", "source is required"))?;
            let target_id = input
                .get("target")
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("target", "target is required"))?;
            let target_obj = ObjectTarget::Catalogue {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Admin, &target_obj))?;
            // The target named here may redirect to the publisher it was merged into: the store
            // fills in the one it merged into (an empty object id names it).
            let row = activity_row(
                req_ctx,
                "publisher.merge",
                "Publisher",
                "",
                format!("Merged publisher {source_id} into {target_id}"),
                json!({"source": source_id, "target": target_id}),
            );
            let merged = op_ctx
                .store
                .merge_publishers(req_ctx, source_id, target_id, row)
                .await?;
            Ok(serde_json::to_value(merged).unwrap_or(Value::Null))
        }
        "publisher.withdraw" => {
            let pub_id = input
                .get("publisher")
                .or_else(|| input.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("publisher", "publisher is required"))?;
            let pub_rec = op_ctx
                .store
                .get_publisher(req_ctx, pub_id)
                .await?
                .ok_or_else(not_found)?;
            let target = ObjectTarget::from_publisher(&pub_rec);
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Delete, &target))?;
            if !pub_rec.named_by(pub_id) {
                let field = if input.get("publisher").is_some() {
                    "publisher"
                } else {
                    "id"
                };
                return Err(DataroomError::field_error(
                    field,
                    format!(
                        "'{pub_id}' was merged into '{}': withdraw that publisher by its own name",
                        pub_rec.name
                    ),
                ));
            }
            let row = activity_row(
                req_ctx,
                "publisher.withdraw",
                "Publisher",
                &pub_rec.id,
                format!("Withdrew publisher {}", pub_rec.name),
                json!({"name": pub_rec.name}),
            );
            let withdrawn = op_ctx
                .store
                .withdraw_publisher(req_ctx, &pub_rec.id, row)
                .await?;
            Ok(serde_json::to_value(withdrawn).unwrap_or(Value::Null))
        }
        "allowed" => {
            let asked_op = input
                .get("operation")
                .and_then(Value::as_str)
                .ok_or_else(|| DataroomError::field_error("operation", "operation is required"))?;
            let object_id = input.get("object").and_then(Value::as_str);
            let Some(asked_def) = find_operation(asked_op) else {
                return Ok(json!({
                    "allowed": false,
                    "reason": format!("unknown operation {asked_op}"),
                    "permission": Value::Null,
                }));
            };
            if asked_def.name == "catalogue.activity"
                && !req_ctx.subject.is_member_of(req_ctx.org_id)
            {
                return Ok(json!({
                    "allowed": false,
                    "reason": "not found",
                    "permission": asked_def.permission.as_str(),
                }));
            }
            let target = match (asked_def.name, asked_def.target_kind, object_id) {
                // Decided on the catalogue, as the call is.
                ("publisher.merge", _, _) => ObjectTarget::Catalogue {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                },
                // The object of a run operation is a run, decided on every source like the call.
                ("harvest_run.get" | "harvest_run.logs", _, run) => {
                    if let Some(run_id) = run {
                        if op_ctx
                            .store
                            .get_harvest_run(req_ctx, run_id)
                            .await?
                            .is_none()
                        {
                            return Ok(json!({
                                "allowed": false,
                                "reason": "not found",
                                "permission": asked_def.permission.as_str(),
                            }));
                        }
                    }
                    ObjectTarget::HarvestSource {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        source_id: "*".into(),
                    }
                }
                (_, "Dataset", Some(id)) => {
                    let Some(ds) = op_ctx.store.get_dataset(req_ctx, id).await? else {
                        return Ok(json!({
                            "allowed": false,
                            "reason": "dataset not found",
                            "permission": asked_def.permission.as_str(),
                        }));
                    };
                    let pub_rec = op_ctx
                        .store
                        .get_publisher(req_ctx, &ds.publisher_id)
                        .await?;
                    let t = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
                    if !dataset_shown(op_ctx, req_ctx, &ds, pub_rec.as_ref()).await? {
                        // Never reveal a hidden record (C11, F2).
                        return Ok(json!({
                            "allowed": false,
                            "reason": "dataset not found",
                            "permission": asked_def.permission.as_str(),
                        }));
                    }
                    t
                }
                (_, "Dataset", None) => ObjectTarget::Dataset {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    dataset_id: "*".into(),
                    publisher_id: "*".into(),
                    visibility: Visibility::Public,
                    restricted_grants: Vec::new(),
                    publisher_editor_grants: Vec::new(),
                },
                (_, "Publisher", Some(id)) => {
                    let Some(pub_rec) = op_ctx.store.get_publisher(req_ctx, id).await? else {
                        return Ok(json!({
                            "allowed": false,
                            "reason": "publisher not found",
                            "permission": asked_def.permission.as_str(),
                        }));
                    };
                    ObjectTarget::from_publisher(&pub_rec)
                }
                (_, "Publisher", None) => ObjectTarget::Publisher {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    publisher_id: "*".into(),
                    editor_grants: Vec::new(),
                },
                (_, "HarvestSource", Some(id)) => {
                    let Some(src) = op_ctx.store.get_harvest_source(req_ctx, id).await? else {
                        return Ok(json!({
                            "allowed": false,
                            "reason": "not found",
                            "permission": asked_def.permission.as_str(),
                        }));
                    };
                    ObjectTarget::HarvestSource {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        source_id: src.id,
                    }
                }
                (_, "HarvestSource", None) => ObjectTarget::HarvestSource {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    source_id: "*".into(),
                },
                (_, "Profile", id_opt) => ObjectTarget::Profile {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    profile_id: id_opt.unwrap_or("*").to_owned(),
                },
                (_, "CataloguePlugin", id_opt) => ObjectTarget::Plugin {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    plugin_name: id_opt.unwrap_or("*").to_owned(),
                },
                _ => ObjectTarget::Catalogue {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                },
            };
            let dec = op_ctx
                .authorizer
                .check(req_ctx, asked_def.permission, &target);
            Ok(json!({
                "allowed": dec.allowed,
                "reason": if !dec.visible { "not found".to_owned() } else { dec.reason },
                "permission": asked_def.permission.as_str(),
            }))
        }
        other => Err(DataroomError::NotFound(format!(
            "unknown operation {other}"
        ))),
    }
}

/// The `dataset.purge` operation.
async fn dataset_purge(op_ctx: &OpContext, req_ctx: &RequestCtx, input: Value) -> Result<Value> {
    let RecordName { name } = op_input(input)?;
    let ds = op_ctx
        .store
        .get_dataset(req_ctx, &name)
        .await?
        .ok_or_else(not_found)?;
    // A purge cannot be undone: it needs Admin, while Delete (a withdrawal) is an editor's.
    authorize_dataset(op_ctx, req_ctx, &ds, Action::Admin).await?;
    let row = activity_row(
        req_ctx,
        "dataset.purge",
        "Dataset",
        &ds.id,
        format!("Purged dataset {}", ds.name),
        json!({"name": ds.name}),
    );
    op_ctx.store.purge_dataset(req_ctx, &ds.id, row).await?;
    Ok(json!({}))
}

/// The `resource.propose` operation.
async fn resource_propose(op_ctx: &OpContext, req_ctx: &RequestCtx, input: Value) -> Result<Value> {
    retry_on_conflict(|| resource_propose_once(op_ctx, req_ctx, input.clone())).await
}

/// One try of `resource.propose` on a fresh read of the dataset.
async fn resource_propose_once(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    input: Value,
) -> Result<Value> {
    let ResourceProposal {
        dataset,
        id,
        resource,
    } = op_input(input)?;
    let store = &op_ctx.store;
    let (mut ds, stored) = match (&dataset, &id) {
        (Some(name), None) => (
            store
                .get_dataset(req_ctx, name)
                .await?
                .ok_or_else(not_found)?,
            None,
        ),
        (None, Some(id)) => {
            let (ds, dist) = store
                .get_distribution(req_ctx, id)
                .await?
                .ok_or_else(not_found)?;
            (ds, Some(dist))
        }
        _ => {
            return Err(DataroomError::field_error(
                "id",
                "name either the dataset of a new resource or the id of a stored one",
            ))
        }
    };
    let publisher = authorize_dataset(op_ctx, req_ctx, &ds, Action::Propose).await?;
    let cat = own_catalogue(op_ctx, req_ctx).await?;
    let lang = cat.default_language.as_str();
    let read = ds.modified;
    let (pos, dist) = match stored {
        None => {
            let pos = ds.distributions.len();
            let fallback_id = format!("res-{}-{pos}", ds.name);
            let dist = distribution_from_ckan(&Value::Object(resource), &ds.id, &fallback_id, lang);
            // A create never takes over a stored resource id: its views and updates would
            // resolve to this dataset, decided on its editors instead of the owner's.
            if store.get_distribution(req_ctx, &dist.id).await?.is_some() {
                return Err(DataroomError::Conflict(format!(
                    "a resource with id '{}' exists: use resource_update",
                    dist.id
                )));
            }
            ds.distributions.push(dist.clone());
            (pos, dist)
        }
        Some(stored) => {
            let pos = ds
                .distributions
                .iter()
                .position(|r| r.id == stored.id)
                .ok_or_else(not_found)?;
            let mut merged = distribution_to_ckan(&stored, &ds.id, pos, &[], lang);
            if let Some(merged) = merged.as_object_mut() {
                merged.extend(resource.clone());
                // A plain `name` / `description` names the default language's text; the
                // stored per-language map would otherwise win and the change would be lost.
                for (plain, translated) in [
                    ("name", "name_translated"),
                    ("description", "description_translated"),
                ] {
                    if resource.contains_key(translated) {
                        continue;
                    }
                    if let Some(text) = resource.get(plain).and_then(Value::as_str) {
                        let mut texts = merged
                            .get(translated)
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        texts.insert(lang.to_owned(), json!(text));
                        merged.insert(translated.to_owned(), Value::Object(texts));
                    }
                }
            }
            let mut dist = distribution_from_ckan(&merged, &ds.id, &stored.id, lang);
            // CKAN's resource dict carries no terms of use: an update keeps the declared ones.
            dist.terms_of_use = stored.terms_of_use.clone();
            ds.distributions[pos] = dist.clone();
            (pos, dist)
        }
    };
    ds.modified = TimestampTz::now();
    let saved = store_dataset(
        op_ctx,
        req_ctx,
        &cat,
        publisher.as_ref(),
        (ds, Some(read)),
        "resource.propose",
    )
    .await?;
    Ok(distribution_to_ckan(&dist, &saved.id, pos, &[], lang))
}

/// The `resource.delete` operation.
async fn resource_delete(op_ctx: &OpContext, req_ctx: &RequestCtx, input: Value) -> Result<Value> {
    let RecordId { id } = op_input(input)?;
    retry_on_conflict(|| resource_delete_once(op_ctx, req_ctx, &id)).await
}

/// One try of `resource.delete` on a fresh read of the dataset.
async fn resource_delete_once(op_ctx: &OpContext, req_ctx: &RequestCtx, id: &str) -> Result<Value> {
    let (mut ds, _) = op_ctx
        .store
        .get_distribution(req_ctx, id)
        .await?
        .ok_or_else(not_found)?;
    let publisher = authorize_dataset(op_ctx, req_ctx, &ds, Action::Delete).await?;
    let cat = own_catalogue(op_ctx, req_ctx).await?;
    let read = ds.modified;
    ds.distributions.retain(|r| r.id != id);
    ds.modified = TimestampTz::now();
    store_dataset(
        op_ctx,
        req_ctx,
        &cat,
        publisher.as_ref(),
        (ds, Some(read)),
        "resource.delete",
    )
    .await?;
    Ok(json!({}))
}

/// The `resource_view.propose` operation.
async fn resource_view_propose(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    input: Value,
) -> Result<Value> {
    let ViewProposal { resource, id, view } = op_input(input)?;
    validate_view_payload(&view)?;
    let store = &op_ctx.store;
    let (resource_id, stored) = match (resource, id) {
        (Some(resource), None) => (resource, None),
        (None, Some(id)) => {
            let stored = store
                .get_resource_view(req_ctx, &id)
                .await?
                .ok_or_else(not_found)?;
            (stored.resource_id.clone(), Some(stored))
        }
        _ => {
            return Err(DataroomError::field_error(
                "id",
                "name either the resource of a new view or the id of a stored one",
            ))
        }
    };
    let view_type = view
        .get("view_type")
        .and_then(Value::as_str)
        .or(stored.as_ref().map(|v| v.view_type.as_str()))
        .ok_or_else(|| DataroomError::field_error("view_type", "Missing value"))?;
    let (ds, _) = store
        .get_distribution(req_ctx, &resource_id)
        .await?
        .ok_or_else(not_found)?;
    authorize_dataset(op_ctx, req_ctx, &ds, Action::Propose).await?;
    let mut record = resource_view_from_ckan(
        &view,
        stored.as_ref().map_or("", |v| v.id.as_str()),
        &resource_id,
        view_type,
    );
    match &stored {
        // An update writes the view it was decided on, whatever id its body carries.
        Some(stored) => record.id.clone_from(&stored.id),
        None => {
            // The id lands in links and embed snippets: a slug, as CKAN's generated ids are.
            validate_slug("id", &record.id)?;
            // A create never takes over a stored view: that one is changed by
            // resource_view_update, decided on its own dataset.
            if store
                .get_resource_view(req_ctx, &record.id)
                .await?
                .is_some()
            {
                return Err(DataroomError::Conflict(format!(
                    "a view with id '{}' exists: use resource_view_update",
                    record.id
                )));
            }
        }
    }
    let row = activity_row(
        req_ctx,
        "resource_view.propose",
        "ResourceView",
        &record.id,
        format!("Saved view {} of dataset {}", record.id, ds.name),
        json!({"dataset": ds.name, "resource": record.resource_id}),
    );
    let saved = store.save_resource_view(req_ctx, record, row).await?;
    Ok(resource_view_to_ckan(&saved))
}

/// The `resource_view.delete` operation.
async fn resource_view_delete(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    input: Value,
) -> Result<Value> {
    let RecordId { id } = op_input(input)?;
    let store = &op_ctx.store;
    let view = store
        .get_resource_view(req_ctx, &id)
        .await?
        .ok_or_else(not_found)?;
    // Decided on the view's dataset; a view whose resource is gone is decided on the
    // catalogue. A failed lookup refuses rather than skipping the check.
    match store.get_distribution(req_ctx, &view.resource_id).await? {
        Some((ds, _)) => {
            authorize_dataset(op_ctx, req_ctx, &ds, Action::Delete).await?;
        }
        None => {
            let target = ObjectTarget::Catalogue {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Delete, &target))?;
        }
    }
    let row = activity_row(
        req_ctx,
        "resource_view.delete",
        "ResourceView",
        &id,
        format!("Deleted view {id}"),
        json!({"resource": view.resource_id}),
    );
    store.delete_resource_view(req_ctx, &id, row).await?;
    Ok(json!({}))
}

/// The `profile.propose` operation.
async fn profile_propose(op_ctx: &OpContext, req_ctx: &RequestCtx, input: Value) -> Result<Value> {
    let activate = input
        .get("activate")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let raw_pr = input.get("profile").cloned().unwrap_or(input);
    let mut profile: crate::model::ProfileRecord = serde_json::from_value(raw_pr)
        .map_err(|e| DataroomError::field_error("profile", format!("invalid profile: {e}")))?;
    let target = ObjectTarget::Profile {
        org_id: req_ctx.org_id,
        catalogue_id: req_ctx.catalogue_id,
        profile_id: profile.id.clone(),
    };
    enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Propose, &target))?;
    // Activating changes what the catalogue validates against, which profile.activate
    // decides with Approve; uploading with `activate` may not skip that.
    if activate {
        enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Approve, &target))?;
    }
    // An upload never becomes a built-in: those ship with the release (their ids are refused
    // by the validator).
    profile.built_in = false;
    let mut cat = own_catalogue(op_ctx, req_ctx).await?;
    // An upload activated through a national base needs what that feed needs: the base's
    // choice is checked as if the catalogue took it up (a Portal-managed choice aside).
    if let (true, Some(base), Some(validator)) =
        (activate, profile.base_profile.as_deref(), &op_ctx.validator)
    {
        validator.check_profile_choice(&cat, &[base], false)?;
    }
    let stored = op_ctx.store.list_profiles(req_ctx).await?;
    if stored
        .iter()
        .any(|p| p.id == profile.id && p.version == profile.version)
    {
        return Err(DataroomError::Conflict(format!(
            "profile '{}' version '{}' already exists; upload a new version",
            profile.id, profile.version
        )));
    }
    let mut dry_run = Value::Null;
    if let Some(validator) = &op_ctx.validator {
        validator.load_stored_profiles(&cat, &stored);
        let (datasets, publishers) = dry_run_records(op_ctx, req_ctx).await?;
        (profile, dry_run) = validator.upload_profile(profile, &cat, &datasets, &publishers)?;
    }
    let row = activity_row(
        req_ctx,
        "profile.propose",
        "Profile",
        &format!("{}:{}", profile.id, profile.version),
        format!("Saved profile {} {}", profile.id, profile.version),
        json!({"id": profile.id, "version": profile.version, "activated": activate}),
    );
    if !activate {
        let saved = op_ctx.store.save_profile(req_ctx, profile, row).await?;
        return Ok(json!({ "profile": saved, "dryRun": dry_run }));
    }
    // Activated: the row commits with the catalogue's new pin.
    let saved = op_ctx.store.upsert_profile(req_ctx, profile).await?;
    let read = cat.modified;
    cat.active_profiles.retain(|p| p.profile_id != saved.id);
    cat.active_profiles.push(crate::model::ProfilePin {
        profile_id: saved.id.clone(),
        version: saved.version.clone(),
    });
    cat.modified = TimestampTz::now();
    op_ctx.store.save_catalogue(cat, Some(read), row).await?;
    Ok(json!({ "profile": saved, "dryRun": dry_run }))
}

/// The `profile.activate` operation.
async fn profile_activate(op_ctx: &OpContext, req_ctx: &RequestCtx, input: Value) -> Result<Value> {
    let ProfileActivation {
        id,
        version,
        make_main,
    } = op_input(input)?;
    let target = ObjectTarget::Profile {
        org_id: req_ctx.org_id,
        catalogue_id: req_ctx.catalogue_id,
        profile_id: id.clone(),
    };
    enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Approve, &target))?;
    let validator = op_ctx.validator.as_ref().ok_or_else(|| {
        DataroomError::NotSupported("no validator holds the profiles here".into())
    })?;
    let mut cat = own_catalogue(op_ctx, req_ctx).await?;
    let read = cat.modified;
    validator.load_stored_profiles(&cat, &op_ctx.store.list_profiles(req_ctx).await?);
    let (datasets, publishers) = dry_run_records(op_ctx, req_ctx).await?;
    let (dry_run, reports) =
        validator.dry_run_profile(&id, &version, &cat, &datasets, &publishers)?;
    let held = cat.main_profile == id || cat.active_profiles.iter().any(|p| p.profile_id == id);
    if !held {
        validator.check_profile_choice(&cat, &[id.as_str()], op_ctx.sources_by_portal)?;
    }
    cat.active_profiles.retain(|p| p.profile_id != id);
    cat.active_profiles.push(crate::model::ProfilePin {
        profile_id: id.clone(),
        version: version.clone(),
    });
    if make_main {
        cat.main_profile.clone_from(&id);
    }
    cat.modified = TimestampTz::now();
    let row = activity_row(
        req_ctx,
        "profile.activate",
        "Profile",
        &format!("{id}:{version}"),
        format!("Activated profile {id} {version}"),
        json!({"id": id, "version": version, "main": make_main}),
    );
    let saved = op_ctx.store.save_catalogue(cat, Some(read), row).await?;
    for report in reports {
        op_ctx.store.upsert_shacl_report(req_ctx, report).await?;
    }
    Ok(json!({ "catalogue": saved, "dryRun": dry_run }))
}
