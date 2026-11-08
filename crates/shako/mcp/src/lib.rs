#![forbid(unsafe_code)]
//! `shako-mcp`: `jc_forge_*` MCP tools generated from `OperationRegistry` (Contract S19).
//!
//! Enforces:
//! - DelegationPolicy connector segment filtering (`forge` vs `apps`): a subject whose
//!   `connector_segments` does not include the tool's segment neither sees the tool in `tools/list`
//!   nor may call it in `tools/call`.
//! - Secret-in-argument refusal before the permission check (THINK-CHECKLIST 12).
//! - Same `Authorizer` decision and 404 hiding for cross-organization objects as REST and OData.

use jc_router::Mount;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shako_api::{ShakoState, exec_odata_operation};
use shako_core::{RequestCtx, ShakoError, ShakoResult};
use shako_git::scan_blob_for_secrets;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolSpec {
    pub name: String,
    pub title: String,
    pub description: String,
    pub connector_segment: String,
    pub input_schema: Value,
}

/// Lists the `jc_forge_*` MCP tools visible to `ctx.subject`'s DelegationPolicy connector segments (S19).
#[must_use]
pub fn list_mcp_tools(state: &ShakoState, ctx: &RequestCtx) -> Vec<McpToolSpec> {
    state
        .registry
        .operations()
        .iter()
        .filter(|op| ctx.subject.allows_connector_segment(op.connector_segment))
        .map(|op| McpToolSpec {
            name: op.mcp_tool.to_owned(),
            title: op.title.to_owned(),
            description: op.description.to_owned(),
            connector_segment: op.connector_segment.to_owned(),
            input_schema: op.input_schema.clone(),
        })
        .collect()
}

/// Calls a `jc_forge_*` MCP tool under `ctx`, checking for secrets in arguments first,
/// verifying the DelegationPolicy connector segment, and delegating to `exec_odata_operation` (S19).
pub async fn call_mcp_tool(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    tool_name: &str,
    arguments: Value,
) -> ShakoResult<Value> {
    // THINK-CHECKLIST 12: refuse any argument containing a secret before the permission check!
    let args_str = arguments.to_string();
    if let Some(reason) = scan_blob_for_secrets("mcp_arguments", args_str.as_bytes()) {
        return Err(ShakoError::BadRequest(format!(
            "refused secret in MCP tool arguments: {reason}"
        )));
    }

    let Some(op_def) = state.registry.find(tool_name) else {
        return Err(ShakoError::NotFound(format!(
            "unknown MCP tool '{tool_name}'"
        )));
    };

    if !ctx.subject.allows_connector_segment(op_def.connector_segment) {
        return Err(ShakoError::NotFound(format!(
            "unknown MCP tool '{tool_name}'"
        )));
    }

    let result = exec_odata_operation(state, ctx, mount, op_def.name, None, arguments).await?;
    Ok(json!({
        "tool": op_def.mcp_tool,
        "untrustedData": true,
        "result": result,
    }))
}
