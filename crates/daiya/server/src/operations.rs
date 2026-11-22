//! The CLI's dispatcher over the single [`daiya_model::OperationRegistry`] (R10a): each
//! operation runs the implementation and the [`daiya_model::Authorizer`] check OData runs.

use daiya_api::odata::{
    ApplyBody, ODataImportActionBody, ODataQueryActionBody, exec_odata_apply,
    exec_odata_datasource_editor_schema, exec_odata_datasource_health, exec_odata_datasource_query,
    exec_odata_datasource_resource, exec_odata_export, exec_odata_import, exec_odata_plan,
};
use daiya_api::{
    DaiyaState, PanelTimeRange, PublicCaller, PublicDashboardQueryReq,
    execute_public_dashboard_query, panel_view, rotate_public_dashboard_token,
};
use daiya_import::DesiredBundle;
use daiya_model::{CrError, CrResult, RequestCtx};
use serde_json::{Value, json};
use std::collections::BTreeMap;

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

/// Executes a CLI subcommand (`query`, `datasource-health`, `editor-schema`, `datasource-resource`,
/// `render-panel`, `export`, `import`, `plan`, `apply`, `rotate-public-token`, `public-query`,
/// `allowed`) under `ctx`, through the operation handlers OData uses.
pub async fn execute_cli_command(
    state: &DaiyaState,
    ctx: &RequestCtx,
    cli_command: &str,
    args: Value,
) -> CrResult<Value> {
    let op = state
        .registry
        .find_operation(cli_command)
        .ok_or_else(|| CrError::NotFound(format!("unknown CLI command '{cli_command}'")))?;

    match op.name.as_str() {
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
            let body: ApplyBody = serde_json::from_value(args).map_err(|e| {
                CrError::BadRequest(format!(
                    "invalid apply args: {e}; give only the digest a plan answered"
                ))
            })?;
            exec_odata_apply(state, ctx, &body.digest).await
        }
        "PublicDashboard/Rotate" => {
            let uid = required_str(&args, "uid")?;
            let rec = rotate_public_dashboard_token(state, ctx, uid).await?;
            Ok(serde_json::to_value(rec).unwrap_or(Value::Null))
        }
        "PublicDashboard/Query" => {
            let token = required_str(&args, "accessToken")?;
            let panel_id = required_panel_id(&args)?;
            // A CLI caller is counted as its own subject, never under a shared address.
            let sets = execute_public_dashboard_query(
                state,
                ctx.subject.actor_id.clone().map(PublicCaller::Subject),
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
            "operation '{other}' not wired in the CLI"
        ))),
    }
}
