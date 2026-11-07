#![forbid(unsafe_code)]
//! `wardkeep-mcp` (W5): JSON-RPC 2.0 MCP server (`/api/v1/realms/{realm}/mcp`) generated from
//! [`wardkeep_core::OperationRegistry`].
//! - `tools/list` filters tools by the caller's [`wardkeep_core::Authorizer`] permissions.
//! - `tools/call` executes the operation through the same `Authorizer` check and organization
//!   `Scope` as REST and OData, so cross-organization probes receive the same not-found answer.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Extension, Json, Router};
use jc_router::Mount;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wardkeep_admin::{AdminState, extract_admin_ctx};
use wardkeep_core::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

pub fn mcp_router(state: AdminState) -> Router {
    Router::new()
        .route("/api/v1/realms/{realm}/mcp", post(mcp_endpoint))
        .with_state(state)
}

async fn mcp_endpoint(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(req): Json<JsonRpcRequest>,
) -> Result<Json<JsonRpcResponse>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;

    match req.method.as_str() {
        "initialize" => Ok(Json(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": "wardkeep-mcp", "version": "0.1.0" },
                "capabilities": { "tools": {} }
            })),
            error: None,
        })),
        "tools/list" => {
            let tools: Vec<Value> = state
                .registry
                .operations()
                .iter()
                .filter(|op| {
                    let target = match op.entity_set {
                        Some("Realms" | "SigningKeys") => ObjectTarget::Realm {
                            realm_id: realm.clone(),
                        },
                        _ => ObjectTarget::Person {
                            realm_id: realm.clone(),
                            org_id: ctx.active_org_id.clone(),
                            person_id: "*".into(),
                        },
                    };
                    state.authorizer.check(&ctx, op.action, &target).allowed
                })
                .map(|op| {
                    json!({
                        "name": op.name,
                        "description": op.description,
                        "inputSchema": op.input_schema,
                    })
                })
                .collect();
            Ok(Json(JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(json!({ "tools": tools })),
                error: None,
            }))
        }
        "tools/call" => {
            let tool_name = req
                .params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(op) = state.registry.find(tool_name) else {
                return Ok(Json(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: None,
                    error: Some(json!({ "code": -32601, "message": "Unknown tool" })),
                }));
            };

            // Same executor as OData and REST: same answer, same decision (W5).
            let args = req.params.get("arguments").cloned().unwrap_or(Value::Null);
            let key = args.get("id").and_then(Value::as_str).map(str::to_owned);
            let mut params = args
                .get("payload")
                .cloned()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            if let (Some(f), Some(obj)) = (args.get("filter"), params.as_object_mut()) {
                obj.insert("filter".into(), f.clone());
            }
            match wardkeep_admin::execute(&state, &mount, &ctx, op, key.as_deref(), &params).await {
                Ok(result) => Ok(Json(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&result).unwrap_or_default()
                        }],
                        "isError": false
                    })),
                    error: None,
                })),
                Err(e) => {
                    let problem = e.to_problem();
                    Ok(Json(JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id: req.id,
                        result: None,
                        error: Some(json!({
                            "code": -32000,
                            "message": problem.detail,
                            "data": { "slug": problem.slug, "status": problem.status }
                        })),
                    }))
                }
            }
        }
        _ => Ok(Json(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: None,
            error: Some(json!({ "code": -32601, "message": "Method not found" })),
        })),
    }
}
