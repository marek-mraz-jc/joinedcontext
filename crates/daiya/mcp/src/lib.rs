#![forbid(unsafe_code)]
//! `daiya-mcp`: MCP tool definitions and dispatcher generated from the single
//! [`OperationRegistry`] of `daiya-model` (R10, R10a).
//!
//! Every MCP tool call executes the exact same operation and [`Authorizer`] check as the
//! OData 4.01 service and CLI.

use daiya_api::odata::{
    ODataImportActionBody, ODataQueryActionBody, exec_odata_apply,
    exec_odata_datasource_editor_schema, exec_odata_datasource_health, exec_odata_datasource_query,
    exec_odata_datasource_resource, exec_odata_export, exec_odata_import, exec_odata_plan,
};
use daiya_api::{
    DaiyaState, PanelTimeRange, PublicDashboardQueryReq, execute_public_dashboard_query,
    panel_view, rotate_public_dashboard_token,
};
use daiya_import::{DesiredBundle, PlanBundle};
use daiya_model::{Action, CrError, CrResult, ObjectTarget, RequestCtx};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

/// MCP tool specification generated from the operation registry (R10, R10a).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Generates the full list of MCP tools from the `daiya` [`OperationRegistry`] (R10, R10a).
#[must_use]
pub fn list_mcp_tools(state: &DaiyaState) -> Vec<McpToolSpec> {
    let mut tools = Vec::with_capacity(state.registry.operations.len() + 1);

    // Entity-set list/search tool
    tools.push(McpToolSpec {
        name: "daiya_list_dashboards".into(),
        description: "List and search dashboards in the caller's organization.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Optional substring filter on title" }
            }
        }),
    });

    for op in &state.registry.operations {
        let mut props = Map::new();
        let mut req = Vec::new();
        for p in &op.params {
            props.insert(
                p.name.clone(),
                json!({
                    "type": p.json_type,
                    "description": p.description,
                }),
            );
            if p.required {
                req.push(Value::String(p.name.clone()));
            }
        }
        tools.push(McpToolSpec {
            name: op.mcp_tool.clone(),
            description: op.description.clone(),
            input_schema: json!({
                "type": "object",
                "properties": props,
                "required": req,
            }),
        });
    }

    tools
}

/// A required string argument; a missing or non-string one is a 400, never an empty uid.
fn required_str<'a>(args: &'a Value, name: &str) -> CrResult<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| CrError::BadRequest(format!("missing required argument '{name}'")))
}

/// A required panel id: a non-negative integer that fits a Grafana panel id, refused (not
/// truncated or defaulted to panel 1) otherwise.
fn required_panel_id(args: &Value) -> CrResult<u32> {
    let raw = args
        .get("panelId")
        .ok_or_else(|| CrError::BadRequest("missing required argument 'panelId'".into()))?;
    raw.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| CrError::BadRequest(format!("invalid panelId {raw}")))
}

/// Executes an MCP tool call under `ctx`, delegating to the exact same implementation and
/// [`Authorizer`] decision as OData and CLI (R10, R10a).
pub async fn call_mcp_tool(
    state: &DaiyaState,
    ctx: &RequestCtx,
    tool_name: &str,
    args: Value,
) -> CrResult<Value> {
    if tool_name == "daiya_list_dashboards" {
        let q_filter = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let dashboards = state.store.list_dashboards(ctx.org_id).await?;
        let visible: Vec<Value> = dashboards
            .into_iter()
            .filter(|d| {
                state
                    .authorizer
                    .check(
                        ctx,
                        Action::Read,
                        &ObjectTarget::Dashboard {
                            org_id: ctx.org_id,
                            dashboard_uid: d.uid.clone(),
                            folder_uid: d.folder_uid.clone(),
                        },
                    )
                    .allowed
                    && (q_filter.is_empty() || d.title.to_ascii_lowercase().contains(&q_filter))
            })
            .map(|d| {
                json!({
                    "uid": d.uid,
                    "title": d.title,
                    "slug": d.slug,
                    "folderUid": d.folder_uid,
                    "panelCount": d.panel_count,
                    "version": d.version,
                })
            })
            .collect();
        return Ok(json!({ "value": visible }));
    }

    let Some(op_def) = state.registry.find_operation(tool_name) else {
        return Err(CrError::NotFound(format!("unknown MCP tool '{tool_name}'")));
    };

    match op_def.name.as_str() {
        "Datasource/Query" => {
            let body: ODataQueryActionBody = serde_json::from_value(args)
                .map_err(|e| CrError::BadRequest(format!("invalid query args: {e}")))?;
            exec_odata_datasource_query(state, ctx, None, body).await
        }
        "Datasource/Health" => {
            let uid = required_str(&args, "uid")?;
            exec_odata_datasource_health(state, ctx, uid).await
        }
        "Datasource/EditorSchema" => {
            let uid = required_str(&args, "uid")?;
            exec_odata_datasource_editor_schema(state, ctx, uid).await
        }
        "Datasource/Resource" => {
            let uid = required_str(&args, "uid")?;
            let path = required_str(&args, "path")?;
            let params: BTreeMap<String, String> = args
                .get("params")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            exec_odata_datasource_resource(state, ctx, uid, path, params).await
        }
        "Dashboard/RenderPanel" => {
            let uid = required_str(&args, "uid")?;
            let panel_id = required_panel_id(&args)?;
            let range = PanelTimeRange {
                from: args.get("from").and_then(Value::as_str).map(str::to_owned),
                to: args.get("to").and_then(Value::as_str).map(str::to_owned),
                ..PanelTimeRange::default()
            };
            let pv = panel_view(state, &ctx.subject, ctx.org_id, uid, panel_id, range).await?;
            Ok(serde_json::to_value(pv).unwrap_or(Value::Null))
        }
        "Dashboard/Export" => {
            let uid = required_str(&args, "uid")?;
            exec_odata_export(state, ctx, uid).await
        }
        "Import" => {
            let body: ODataImportActionBody = serde_json::from_value(args)
                .map_err(|e| CrError::BadRequest(format!("invalid import args: {e}")))?;
            exec_odata_import(state, ctx, body).await
        }
        "Plan" => {
            let bundle_val = args.get("bundle").cloned().unwrap_or(args);
            let desired: DesiredBundle = serde_json::from_value(bundle_val)
                .map_err(|e| CrError::BadRequest(format!("invalid plan bundle: {e}")))?;
            exec_odata_plan(state, ctx, desired).await
        }
        "Apply" => {
            let plan_val = args.get("plan").cloned().unwrap_or(args);
            let plan: PlanBundle = serde_json::from_value(plan_val)
                .map_err(|e| CrError::BadRequest(format!("invalid apply plan: {e}")))?;
            exec_odata_apply(state, ctx, plan).await
        }
        "PublicDashboard/Rotate" => {
            let uid = required_str(&args, "uid")?;
            let rec = rotate_public_dashboard_token(state, ctx, uid).await?;
            Ok(serde_json::to_value(rec).unwrap_or(Value::Null))
        }
        "PublicDashboard/Query" => {
            let token = required_str(&args, "accessToken")?;
            let panel_id = required_panel_id(&args)?;
            let sets = execute_public_dashboard_query(
                state,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                token,
                panel_id,
                PublicDashboardQueryReq::default(),
            )
            .await?;
            Ok(json!({ "value": sets }))
        }
        "Allowed" => {
            let op = required_str(&args, "operation")?;
            let obj = args.get("object").and_then(Value::as_str);
            let dec = state
                .registry
                .check_allowed(state.authorizer.as_ref(), ctx, op, obj);
            Ok(serde_json::to_value(dec).unwrap_or(Value::Null))
        }
        other => Err(CrError::NotImplemented(format!(
            "operation '{other}' not wired in MCP"
        ))),
    }
}
