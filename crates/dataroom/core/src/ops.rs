#![forbid(unsafe_code)]
//! Single operation registry (`C11`) shared by REST, OData, MCP tools (`research-ckan.md` §9)
//! and CLI subcommands.
//!
//! Every operation declares its name, MCP tool name, segment, required [`Action`], object target
//! kind, and input/output JSON schemas once.

use crate::auth::{Action, Authorizer, Decision, ObjectTarget, RequestCtx};
use crate::ckan_compat::{dataset_from_ckan, dataset_to_ckan};
use crate::error::{DataroomError, Result};
use crate::model::{
    ActivityRow, Catalogue, CataloguePlugin, Dataset, HarvestSource, Publisher, RecordState,
    Visibility,
};
use crate::odata_defs::scope_for_context;
use crate::store::Store;
use chrono::Utc;
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
}

/// Hook trait so `dataroom-core` operations can invoke SHACL validation without a circular
/// dependency on `dataroom-shacl`.
pub trait DatasetValidator: Send + Sync {
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
}

/// Hook trait so `dataroom-core` operations can start a harvest run without a circular
/// dependency on `dataroom-harvest`.
pub trait HarvestRunner: Send + Sync {
    fn run_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        store: &'a dyn Store,
        source: &'a HarvestSource,
    ) -> crate::store::BoxFuture<'a, Result<crate::model::HarvestRun>>;
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
            description: "Start a harvest run for a harvest source.",
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
    let now = Value::String(Utc::now().to_rfc3339());
    for key in timestamps {
        obj.entry((*key).to_owned()).or_insert_with(|| now.clone());
    }
    Ok(raw)
}

fn enforce_decision(decision: Decision) -> Result<()> {
    if decision.allowed {
        Ok(())
    } else if !decision.visible {
        Err(DataroomError::NotFound("not found".into()))
    } else {
        Err(DataroomError::Forbidden(decision.reason))
    }
}

/// Executes a registered operation through the single authorization and validation pipeline (C11).
pub async fn execute_operation(
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
            Ok(serde_json::to_value(cat).unwrap_or(Value::Null))
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
            cat.modified = Utc::now();
            let saved = op_ctx.store.upsert_catalogue(cat).await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "catalogue.propose".into(),
                        object_type: "Catalogue".into(),
                        object_id: saved.id.to_string(),
                        summary: format!("Updated catalogue {}", saved.name),
                        details: json!({}),
                    },
                )
                .await?;
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
                if !q.is_empty() {
                    let hay = format!(
                        "{} {} {} {}",
                        ds.name,
                        ds.title.values().cloned().collect::<Vec<_>>().join(" "),
                        ds.description.values().cloned().collect::<Vec<_>>().join(" "),
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
                skip: Some((page - 1) * 20),
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
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Read, &target))?;
            let mut views = BTreeMap::new();
            for dist in &ds.distributions {
                let v = op_ctx.store.list_resource_views(req_ctx, &dist.id).await?;
                if !v.is_empty() {
                    views.insert(dist.id.clone(), v);
                }
            }
            Ok(dataset_to_ckan(&ds, pub_rec.as_ref(), &views, "en"))
        }
        "dataset.propose" => {
            let raw_ds = input.get("dataset").cloned().unwrap_or(input);
            let existing_name = raw_ds
                .get("id")
                .or_else(|| raw_ds.get("name"))
                .and_then(Value::as_str);
            let existing = match existing_name {
                Some(n) => op_ctx.store.get_dataset(req_ctx, n).await?,
                None => None,
            };
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
                parsed
            } else {
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

            if let Some(validator) = &op_ctx.validator {
                let reports = validator.validate_dataset(&cat, &ds, Some(&pub_rec))?;
                for report in &reports {
                    if report.profile_id == cat.main_profile
                        && !report.conforms
                        && ds.visibility == Visibility::Public
                    {
                        let details: Vec<String> = report
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
                let saved = op_ctx.store.upsert_dataset(req_ctx, ds).await?;
                for mut report in reports {
                    report.dataset_id = saved.id.clone();
                    op_ctx.store.upsert_shacl_report(req_ctx, report).await?;
                }
                op_ctx
                    .store
                    .record_activity(
                        req_ctx,
                        ActivityRow {
                            org_id: req_ctx.org_id,
                            catalogue_id: req_ctx.catalogue_id,
                            id: format!("act-{}", Uuid::now_v7()),
                            at: Utc::now(),
                            actor: req_ctx.actor_name().to_owned(),
                            operation: "dataset.propose".into(),
                            object_type: "Dataset".into(),
                            object_id: saved.id.clone(),
                            summary: format!("Saved dataset {}", saved.name),
                            details: json!({"name": saved.name}),
                        },
                    )
                    .await?;
                return Ok(dataset_to_ckan(
                    &saved,
                    Some(&pub_rec),
                    &BTreeMap::new(),
                    &cat.default_language,
                ));
            }

            let saved = op_ctx.store.upsert_dataset(req_ctx, ds).await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "dataset.propose".into(),
                        object_type: "Dataset".into(),
                        object_id: saved.id.clone(),
                        summary: format!("Saved dataset {}", saved.name),
                        details: json!({"name": saved.name}),
                    },
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
            let ds = op_ctx
                .store
                .get_dataset(req_ctx, name)
                .await?
                .ok_or_else(|| DataroomError::NotFound(format!("dataset '{name}' not found")))?;
            let pub_rec = op_ctx
                .store
                .get_publisher(req_ctx, &ds.publisher_id)
                .await?;
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Delete, &target))?;
            let withdrawn = op_ctx.store.withdraw_dataset(req_ctx, &ds.id).await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "dataset.withdraw".into(),
                        object_type: "Dataset".into(),
                        object_id: withdrawn.id.clone(),
                        summary: format!("Withdrew dataset {}", withdrawn.name),
                        details: json!({"name": withdrawn.name}),
                    },
                )
                .await?;
            Ok(json!({ "id": withdrawn.id, "name": withdrawn.name, "state": "deleted" }))
        }
        "publisher.list" => {
            let publishers = op_ctx.store.list_publishers(req_ctx).await?;
            Ok(json!({ "publishers": publishers }))
        }
        "publisher.propose" => {
            let raw_pub = scoped_record(
                input.get("publisher").cloned().unwrap_or(input),
                req_ctx,
                &["created", "modified"],
            )?;
            let mut publisher: Publisher = serde_json::from_value(raw_pub).map_err(|e| {
                DataroomError::field_error("publisher", format!("invalid publisher: {e}"))
            })?;
            publisher.org_id = req_ctx.org_id;
            publisher.catalogue_id = req_ctx.catalogue_id;
            let target = ObjectTarget::from_publisher(&publisher);
            let decision = op_ctx.authorizer.check(req_ctx, Action::Propose, &target);
            if !decision.allowed {
                return Err(DataroomError::Forbidden(decision.reason));
            }
            let saved = op_ctx.store.upsert_publisher(req_ctx, publisher).await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "publisher.propose".into(),
                        object_type: "Publisher".into(),
                        object_id: saved.id.clone(),
                        summary: format!("Saved publisher {}", saved.name),
                        details: json!({"name": saved.name}),
                    },
                )
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
            let saved = op_ctx.store.upsert_harvest_source(req_ctx, source).await?;
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
                .run_source(req_ctx, op_ctx.store.as_ref(), &source)
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
                .ok_or_else(|| DataroomError::NotFound(format!("harvest run '{run_id}' not found")))?;
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
                .ok_or_else(|| DataroomError::NotFound(format!("harvest run '{run_id}' not found")))?;
            let logs: Vec<_> = run
                .logs
                .into_iter()
                .filter(|l| level.is_none_or(|lvl| l.level.eq_ignore_ascii_case(lvl)))
                .collect();
            Ok(json!({ "run": run.id, "logs": logs }))
        }
        "plugin.list" => {
            let plugins = op_ctx.store.list_plugins(req_ctx).await?;
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
            let saved = op_ctx.store.upsert_plugin(req_ctx, plugin).await?;
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
            let public_ds: Vec<Dataset> = all_ds
                .into_iter()
                .filter(|d| d.state == RecordState::Active && d.visibility == Visibility::Public)
                .collect();
            let publishers = op_ctx.store.list_publishers(req_ctx).await?;
            let pub_map: BTreeMap<String, Publisher> = publishers
                .into_iter()
                .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
                .collect();
            if let Some(validator) = &op_ctx.validator {
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
            let eff = cat.effective_branding(None);
            Ok(serde_json::to_value(eff).unwrap_or(Value::Null))
        }
        "branding.propose" => {
            let target = ObjectTarget::Catalogue {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Admin, &target))?;
            let mut cat = op_ctx
                .store
                .get_catalogue(req_ctx.org_id, req_ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;
            let br_val = input.get("branding").cloned().unwrap_or(input);
            cat.branding = Some(br_val);
            cat.modified = Utc::now();
            let saved = op_ctx.store.upsert_catalogue(cat).await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "branding.propose".into(),
                        object_type: "Catalogue".into(),
                        object_id: saved.id.to_string(),
                        summary: format!("Updated branding on catalogue {}", saved.name),
                        details: json!({}),
                    },
                )
                .await?;
            let eff = saved.effective_branding(None);
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
        "profile.propose" => {
            let raw_pr = input.get("profile").cloned().unwrap_or(input);
            let profile: crate::model::ProfileRecord =
                serde_json::from_value(raw_pr).map_err(|e| {
                    DataroomError::field_error("profile", format!("invalid profile: {e}"))
                })?;
            let target = ObjectTarget::Profile {
                org_id: req_ctx.org_id,
                catalogue_id: req_ctx.catalogue_id,
                profile_id: profile.id.clone(),
            };
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Propose, &target))?;
            let saved = op_ctx.store.upsert_profile(req_ctx, profile).await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "profile.propose".into(),
                        object_type: "Profile".into(),
                        object_id: format!("{}:{}", saved.id, saved.version),
                        summary: format!("Saved profile {} {}", saved.id, saved.version),
                        details: json!({"id": saved.id, "version": saved.version}),
                    },
                )
                .await?;
            Ok(serde_json::to_value(saved).unwrap_or(Value::Null))
        }
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
            let merged = op_ctx
                .store
                .merge_publishers(req_ctx, source_id, target_id)
                .await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "publisher.merge".into(),
                        object_type: "Publisher".into(),
                        object_id: merged.id.clone(),
                        summary: format!("Merged publisher {source_id} into {}", merged.name),
                        details: json!({"source": source_id, "target": merged.name}),
                    },
                )
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
                .ok_or_else(|| DataroomError::NotFound(format!("publisher '{pub_id}' not found")))?;
            let target = ObjectTarget::from_publisher(&pub_rec);
            enforce_decision(op_ctx.authorizer.check(req_ctx, Action::Delete, &target))?;
            let withdrawn = op_ctx.store.withdraw_publisher(req_ctx, &pub_rec.id).await?;
            op_ctx
                .store
                .record_activity(
                    req_ctx,
                    ActivityRow {
                        org_id: req_ctx.org_id,
                        catalogue_id: req_ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: req_ctx.actor_name().to_owned(),
                        operation: "publisher.withdraw".into(),
                        object_type: "Publisher".into(),
                        object_id: withdrawn.id.clone(),
                        summary: format!("Withdrew publisher {}", withdrawn.name),
                        details: json!({"name": withdrawn.name}),
                    },
                )
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
            let target = match (asked_def.target_kind, object_id) {
                ("Dataset", Some(id)) => {
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
                    let read_dec = op_ctx.authorizer.check(req_ctx, Action::Read, &t);
                    if !read_dec.allowed {
                        // Never reveal a hidden record (C11, F2).
                        return Ok(json!({
                            "allowed": false,
                            "reason": "dataset not found",
                            "permission": asked_def.permission.as_str(),
                        }));
                    }
                    t
                }
                ("Dataset", None) => ObjectTarget::Dataset {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    dataset_id: "*".into(),
                    publisher_id: "*".into(),
                    visibility: Visibility::Public,
                    restricted_grants: Vec::new(),
                    publisher_editor_grants: Vec::new(),
                },
                ("Publisher", Some(id)) => {
                    let Some(pub_rec) = op_ctx.store.get_publisher(req_ctx, id).await? else {
                        return Ok(json!({
                            "allowed": false,
                            "reason": "publisher not found",
                            "permission": asked_def.permission.as_str(),
                        }));
                    };
                    ObjectTarget::from_publisher(&pub_rec)
                }
                ("Publisher", None) => ObjectTarget::Publisher {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    publisher_id: "*".into(),
                    editor_grants: Vec::new(),
                },
                ("HarvestSource", Some(id)) => {
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
                ("HarvestSource", None) => ObjectTarget::HarvestSource {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    source_id: "*".into(),
                },
                ("Profile", id_opt) => ObjectTarget::Profile {
                    org_id: req_ctx.org_id,
                    catalogue_id: req_ctx.catalogue_id,
                    profile_id: id_opt.unwrap_or("*").to_owned(),
                },
                ("CataloguePlugin", id_opt) => ObjectTarget::Plugin {
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
        other => Err(DataroomError::NotFound(format!("unknown operation {other}"))),
    }
}
