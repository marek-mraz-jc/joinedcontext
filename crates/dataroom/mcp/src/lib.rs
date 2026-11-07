#![forbid(unsafe_code)]
//! Model Context Protocol (MCP) server tools generated from the single operation registry (C11,
//! `research-ckan.md` §9).
//!
//! Every tool runs the same `Authorizer` decision and validation as REST, OData and CLI, and
//! wraps tool text outputs in the untrusted-data fence.

use dataroom_core::ops::{all_operations, execute_operation, OpContext};
use dataroom_core::{Action, ObjectTarget, RequestCtx, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// MCP tool definition returned by `tools/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub segment: String,
}

/// Wraps untrusted catalogue/dataset text inside the platform untrusted-data fence
/// (`research-ckan.md` §9, `research-tenancy.md` §4.2).
pub fn wrap_untrusted_fence(payload: &Value) -> String {
    let serialized = serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".into());
    format!(
        "<untrusted-data>\n{}\n</untrusted-data>",
        serialized.replace("</untrusted-data>", "&lt;/untrusted-data&gt;")
    )
}

/// Lists all MCP tools from the operation registry that `req_ctx` is permitted to see.
pub fn list_mcp_tools(op_ctx: &OpContext, req_ctx: &RequestCtx) -> Vec<McpTool> {
    let cat_target = ObjectTarget::Catalogue {
        org_id: req_ctx.org_id,
        catalogue_id: req_ctx.catalogue_id,
    };
    all_operations()
        .into_iter()
        .filter(|op| {
            if req_ctx.subject.read_only && op.permission != Action::Read {
                return false;
            }
            if op.permission == Action::Read {
                return true;
            }
            op_ctx
                .authorizer
                .check(req_ctx, op.permission, &cat_target)
                .allowed
                || req_ctx.subject.is_member_of(req_ctx.org_id)
        })
        .map(|op| McpTool {
            name: op.mcp_tool.to_owned(),
            description: op.description.to_owned(),
            input_schema: op.input_schema,
            segment: op.segment.to_owned(),
        })
        .collect()
}

/// Handles a JSON-RPC 2.0 MCP request (`initialize`, `tools/list`, `tools/call`).
pub async fn handle_mcp_jsonrpc(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    rpc_request: &Value,
) -> Value {
    let id = rpc_request.get("id").cloned().unwrap_or(Value::Null);
    let method = rpc_request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("");
    let params = rpc_request.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "dataroom-mcp",
                    "version": "0.1.0"
                },
                "capabilities": {
                    "tools": {}
                }
            }
        }),
        "tools/list" => {
            let tools = list_mcp_tools(op_ctx, req_ctx);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": tools
                }
            })
        }
        "tools/call" => {
            let tool_name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_mcp_tool(op_ctx, req_ctx, tool_name, arguments).await {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": wrap_untrusted_fence(&result)
                            }
                        ],
                        "structuredContent": result,
                        "isError": false
                    }
                }),
                Err(err) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": err.to_string()
                            }
                        ],
                        "isError": true
                    }
                }),
            }
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("method '{method}' not found")
            }
        }),
    }
}

/// Calls an MCP tool by name through the shared operation registry.
pub async fn call_mcp_tool(
    op_ctx: &OpContext,
    req_ctx: &RequestCtx,
    tool_name: &str,
    arguments: Value,
) -> Result<Value> {
    execute_operation(op_ctx, req_ctx, tool_name, arguments).await
}
