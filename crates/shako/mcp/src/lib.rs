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

    if !ctx
        .subject
        .allows_connector_segment(op_def.connector_segment)
    {
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

#[cfg(test)]
mod tests {
    use super::*;
    use shako_core::{
        Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, Principal, RoleBinding,
        ShakoSigningKey, Subject, TokenVerifier,
    };
    use std::sync::Arc;

    fn world(tmp: &tempfile::TempDir, segments: Option<&[&str]>) -> (ShakoState, RequestCtx) {
        let org = OrgId::from_u128(10);
        let mut subject = Subject::authenticated("agent", "agent", vec![org]);
        subject.connector_segments = segments.map(|s| s.iter().map(|x| (*x).to_owned()).collect());
        let state = ShakoState::new(
            InstanceId::from_u128(1),
            tmp.path().to_path_buf(),
            Arc::new(shako_store_memory::MemoryStore::new()),
            Arc::new(LocalAuthorizer::new(vec![RoleBinding {
                org_id: org,
                principal: Principal::User("agent".into()),
                actions: vec![Action::Admin, Action::Write, Action::Read],
                project_id: None,
                repo: None,
            }])),
            TokenVerifier::new(ShakoSigningKey::generate().expect("key")),
            OutboundGuard::default(),
        );
        let ctx = RequestCtx::new(InstanceId::from_u128(1), org, subject);
        (state, ctx)
    }

    fn names(state: &ShakoState, ctx: &RequestCtx) -> Vec<String> {
        list_mcp_tools(state, ctx)
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    #[test]
    fn the_tool_list_follows_the_connector_segments() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (state, all) = world(&tmp, None);
        let every = names(&state, &all);
        assert_eq!(every.len(), state.registry.operations().len());
        assert!(every.iter().any(|n| n == "jc_forge_publish_package"));
        assert!(every.iter().any(|n| n == "jc_forge_create_repository"));
        let (_, star) = world(&tmp, Some(&["*"]));
        assert_eq!(names(&state, &star), every);
        let (_, apps) = world(&tmp, Some(&["apps"]));
        let tools = list_mcp_tools(&state, &apps);
        let in_apps = state
            .registry
            .operations()
            .iter()
            .filter(|op| op.connector_segment == "apps")
            .count();
        assert_eq!(tools.len(), in_apps, "{tools:?}");
        assert!(tools.len() < every.len());
        assert!(
            tools.iter().all(|t| t.connector_segment == "apps"),
            "{tools:?}"
        );
        let publish = tools
            .iter()
            .find(|t| t.name == "jc_forge_publish_package")
            .expect("publish is an apps tool");
        assert_eq!(publish.title, "Publish Package");
        assert_eq!(publish.description, "Publish a generic package file.");
        assert!(publish.input_schema.is_object());
        let (_, none) = world(&tmp, Some(&[]));
        assert!(names(&state, &none).is_empty());
    }

    #[tokio::test]
    async fn a_call_refuses_secrets_first_then_hides_unknown_and_filtered_tools() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (state, forge) = world(&tmp, Some(&["forge"]));
        let mount = Mount::default();
        let key = "-----BEGIN PRIVATE KEY-----";
        // A secret is refused even for a tool that does not exist (before the lookup).
        let secret = call_mcp_tool(
            &state,
            &forge,
            &mount,
            "jc_forge_nothing",
            json!({ "k": key }),
        )
        .await;
        assert!(
            matches!(&secret, Err(ShakoError::BadRequest(m)) if m.starts_with("refused secret in MCP tool arguments: ")),
            "{secret:?}"
        );
        let unknown = call_mcp_tool(&state, &forge, &mount, "jc_forge_nothing", json!({})).await;
        assert_eq!(
            unknown.err(),
            Some(ShakoError::NotFound(
                "unknown MCP tool 'jc_forge_nothing'".into()
            ))
        );
        let filtered = call_mcp_tool(
            &state,
            &forge,
            &mount,
            "jc_forge_publish_package",
            json!({}),
        )
        .await;
        assert_eq!(
            filtered.err(),
            Some(ShakoError::NotFound(
                "unknown MCP tool 'jc_forge_publish_package'".into()
            ))
        );
    }

    #[tokio::test]
    async fn a_call_answers_the_operation_result_marked_untrusted() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (state, ctx) = world(&tmp, None);
        let made = call_mcp_tool(
            &state,
            &ctx,
            &Mount::default(),
            "jc_forge_create_repository",
            json!({ "Owner": "joinedcontext", "Name": "tooled", "AutoInit": true }),
        )
        .await
        .expect("create");
        assert_eq!(made["tool"], "jc_forge_create_repository");
        assert_eq!(made["untrustedData"], true);
        assert!(made["result"].to_string().contains("tooled"), "{made}");
    }
}
