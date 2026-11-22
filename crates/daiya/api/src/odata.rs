#![forbid(unsafe_code)]
//! Native OData 4.01 service at `/odata/v4` (R10, R10a):
//! - `$metadata` (CSDL XML and JSON)
//! - `$batch` (OData 4.01 JSON batch requests)
//! - All 8 entity sets (`Dashboards`, `DashboardVersions`, `Folders`, `Datasources`,
//!   `LibraryPanels`, `PublicDashboards`, `QueryCacheStats`, `ImportReports`) served through
//!   `odata-table` (index rows paged by the store, `crate::sets`; `$filter`, `$orderby`, `$select`, `$top`, `$skip`, `$count`, `$skiptoken`,
//!   `({key})`, `/$count`, `/$export`, `/$allowed`).
//! - All bound and unbound actions and functions of R10a:
//!   `Datasource/Query`, `Datasource/Health`, `Datasource/EditorSchema`, `Datasource/Resource`,
//!   `Dashboard/RenderPanel`, `Dashboard/Export`, `Import`, `Plan`, `Apply`,
//!   `PublicDashboard/Rotate`, `PublicDashboard/Query`, `Allowed`.

use crate::public::{
    PublicDashboardQueryReq, execute_public_dashboard_query, rotate_public_dashboard_token,
};
use crate::render::{
    PanelTimeRange, execute_targets_and_expressions, panel_view, parse_time_str_ms,
};
use crate::sets;
use crate::state::{DaiyaState, OrgCtx, audit, error_response, unit};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use daiya_import::{
    DashboardImportInput, DesiredBundle, apply_plan, compute_plan, export_dashboard,
    import_dashboards, load_plan,
};
use daiya_model::{Action, CrError, CrResult, ObjectTarget, RequestCtx};
use daiya_query::{Health, QueryCtx, ResourceReq};
use jc_router::ClientAddr;
use odata_table::{Operation as TableOp, Scope, TableError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Most sub-requests one `$batch` may carry.
pub const MAX_BATCH_REQUESTS: usize = 100;

/// Read decision on the Organization itself, for the sets that list what is its own.
pub(crate) fn can_read_organization(state: &DaiyaState, ctx: &RequestCtx) -> bool {
    state
        .authorizer
        .check(
            ctx,
            Action::Read,
            &ObjectTarget::Organization { org_id: ctx.org_id },
        )
        .allowed
}

pub(crate) fn map_table_error(err: TableError) -> CrError {
    match err {
        TableError::NotImplemented(w) => CrError::NotImplemented(w),
        TableError::UnknownProperty(p) => {
            CrError::BadRequest(format!("unknown OData property '{p}'"))
        }
        TableError::BadRequest(b) => CrError::BadRequest(b),
        TableError::Source(s) => CrError::Internal(s),
    }
}

fn scope_for_ctx(state: &DaiyaState, ctx: &RequestCtx) -> Scope {
    let mut perms = Vec::new();
    for (verb, suffix) in [
        (Action::Read, "read"),
        (Action::Propose, "propose"),
        (Action::Approve, "approve"),
        (Action::Delete, "delete"),
    ] {
        if state
            .authorizer
            .check(
                ctx,
                verb,
                &ObjectTarget::Organization { org_id: ctx.org_id },
            )
            .allowed
        {
            for prefix in ["dashboards", "folders", "datasources"] {
                perms.push(format!("{prefix}:{suffix}"));
            }
        }
    }
    let refs: Vec<&str> = perms.iter().map(String::as_str).collect();
    Scope::default().holding(&refs)
}

pub(crate) fn parse_odata_query_options(
    model: &odata_csdl::Model,
    set_name: &str,
    raw_query: Option<&str>,
) -> CrResult<odata_url::QueryOptions> {
    let uri = match raw_query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{set_name}?{q}"),
        None => set_name.to_owned(),
    };
    let parsed = odata_url::parse(
        &uri,
        &odata_url::EdmModel(model),
        &odata_url::Limits::strict(),
    )
    .map_err(|e| CrError::BadRequest(format!("invalid OData URL query: {e:?}")))?;
    match parsed {
        odata_url::ODataUri::Resource { options, .. } => Ok(options),
        _ => Ok(odata_url::QueryOptions::default()),
    }
}

async fn odata_service_doc(State(state): State<DaiyaState>) -> Response {
    let sets: Vec<Value> = state
        .registry
        .service
        .sets
        .iter()
        .map(|s| {
            json!({
                "name": s.set,
                "kind": "EntitySet",
                "url": s.set,
            })
        })
        .collect();
    (
        StatusCode::OK,
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(json!({
            "@odata.context": "$metadata",
            "value": sets,
        })),
    )
        .into_response()
}

async fn odata_metadata(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    RawQuery(raw_q): RawQuery,
) -> Response {
    let wants_json = raw_q.as_deref().is_some_and(|q| q.contains("$format=json"))
        || headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("application/json") && !a.contains("xml"));

    if wants_json {
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            Json(state.registry.csdl_json()),
        )
            .into_response();
    }

    let xml = state.registry.csdl_xml();
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response()
}

/// Parses a segment like `"Dashboards"`, `"Dashboards('jc-slo')"`, `"Allowed(operation='Import')"`
fn split_paren_segment(seg: &str) -> (&str, Option<&str>) {
    if let Some((head, tail)) = seg.split_once('(')
        && let Some(inner) = tail.strip_suffix(')')
    {
        return (head, Some(inner));
    }
    (seg, None)
}

fn unquote_odata_key(raw: &str) -> String {
    let t = raw.trim();
    if let Some((_, v)) = t.split_once('=') {
        return v.trim().trim_matches(['\'', '"']).to_owned();
    }
    t.trim_matches(['\'', '"']).to_owned()
}

fn parse_func_args(parens: Option<&str>, raw_query: Option<&str>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(inner) = parens {
        for part in inner.split(',') {
            if let Some((k, v)) = part.split_once('=') {
                map.insert(
                    k.trim().to_owned(),
                    v.trim().trim_matches(['\'', '"']).to_owned(),
                );
            }
        }
    }
    if let Some(q) = raw_query
        && let Ok(pairs) = serde_urlencoded::from_str::<Vec<(String, String)>>(q)
    {
        for (k, v) in pairs {
            map.insert(k, v);
        }
    }
    map
}

// =========================================================================
// R10a Bound & Unbound OData Actions & Functions
// =========================================================================

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ODataQueryActionBody {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub max_data_points: Option<u32>,
    #[serde(default)]
    pub queries: Vec<Value>,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
}

pub async fn exec_odata_datasource_query(
    state: &DaiyaState,
    ctx: &RequestCtx,
    bound_ds_uid: Option<&str>,
    body: ODataQueryActionBody,
) -> CrResult<Value> {
    let from_ms = parse_time_str_ms(body.from.as_deref(), -6 * 3_600_000);
    let to_ms = parse_time_str_ms(body.to.as_deref(), 0);
    let frame_sets = execute_targets_and_expressions(
        state,
        ctx,
        bound_ds_uid,
        &body.queries,
        from_ms,
        to_ms,
        body.interval_ms.unwrap_or(15_000),
        body.max_data_points.unwrap_or(1000),
        &body.variables,
        false,
    )
    .await?;
    Ok(json!({
        "@odata.context": "$metadata#Collection(JoinedContext.Daiya.FrameSet)",
        "value": frame_sets,
    }))
}

pub async fn exec_odata_datasource_health(
    state: &DaiyaState,
    ctx: &RequestCtx,
    uid: &str,
) -> CrResult<Value> {
    let dec = state.authorizer.check(
        ctx,
        Action::Read,
        &ObjectTarget::Datasource {
            org_id: ctx.org_id,
            datasource_uid: uid.to_owned(),
        },
    );
    dec.into_result(format!("Datasource '{uid}' not found"))?;

    // A datasource whose driver cannot be built (refused host, bad configuration, a kind
    // with no driver) is reported unhealthy with the reason, never "working".
    let h = match state.driver_for(ctx.org_id, uid).await {
        Ok(Some(driver)) => {
            let q_ctx = QueryCtx::new(ctx.org_id, ctx.instance_id, ctx.subject.clone(), 0, 1000);
            driver.health(&q_ctx).await
        }
        Ok(None) => return Err(CrError::NotFound(format!("Datasource '{uid}' not found"))),
        Err(err) => Health::error(err.public_detail()),
    };
    Ok(json!({
        "@odata.context": "$metadata#JoinedContext.Daiya.HealthStatus",
        "status": h.status,
        "message": h.message,
    }))
}

pub async fn exec_odata_datasource_editor_schema(
    state: &DaiyaState,
    ctx: &RequestCtx,
    uid: &str,
) -> CrResult<Value> {
    let dec = state.authorizer.check(
        ctx,
        Action::Read,
        &ObjectTarget::Datasource {
            org_id: ctx.org_id,
            datasource_uid: uid.to_owned(),
        },
    );
    dec.into_result(format!("Datasource '{uid}' not found"))?;

    let registered = state.get_datasource_driver(ctx.org_id, uid);
    let ds_rec = state.store.get_datasource(ctx.org_id, uid).await?;
    let driver = match (registered, &ds_rec) {
        (Some(driver), _) => Some(driver),
        (None, Some(record)) => state.driver_for_record(record).await.ok(),
        (None, None) => return Err(CrError::NotFound(format!("Datasource '{uid}' not found"))),
    };
    if let Some(driver) = driver {
        return Ok(serde_json::to_value(driver.editor()).unwrap_or(Value::Null));
    }
    // The editor of a datasource whose driver cannot be built yet (the host is refused or
    // the configuration incomplete) is a plain object form, so it can still be fixed.
    let kind = ds_rec.map(|r| r.kind).unwrap_or_default();
    Ok(json!({
        "kind": kind,
        "jsonSchema": { "type": "object" },
        "fieldHints": [],
    }))
}

pub async fn exec_odata_datasource_resource(
    state: &DaiyaState,
    ctx: &RequestCtx,
    uid: &str,
    path: &str,
    params: BTreeMap<String, String>,
) -> CrResult<Value> {
    crate::state::reads_for_someone(&ctx.subject)?;
    let dec = state.authorizer.check(
        ctx,
        Action::Read,
        &ObjectTarget::Datasource {
            org_id: ctx.org_id,
            datasource_uid: uid.to_owned(),
        },
    );
    dec.into_result(format!("Datasource '{uid}' not found"))?;

    let driver = state
        .driver_for(ctx.org_id, uid)
        .await?
        .ok_or_else(|| CrError::NotFound(format!("Datasource '{uid}' not found")))?;
    let q_ctx = QueryCtx::new(ctx.org_id, ctx.instance_id, ctx.subject.clone(), 0, 1000);
    let resp = driver
        .resource(
            &q_ctx,
            ResourceReq {
                path: path.to_owned(),
                params,
            },
        )
        .await?;
    Ok(resp.data)
}

/// `Dashboard/Export`: the stored dashboard as Grafana JSON, after a read decision on the
/// dashboard in its folder. Unknown, other-organization and unreadable dashboards answer the same 404.
pub async fn exec_odata_export(state: &DaiyaState, ctx: &RequestCtx, uid: &str) -> CrResult<Value> {
    let not_found = || CrError::NotFound(format!("Dashboard '{uid}' not found"));
    let rec = state
        .store
        .get_dashboard(ctx.org_id, uid)
        .await?
        .ok_or_else(not_found)?;
    let dec = state.authorizer.check(
        ctx,
        Action::Read,
        &ObjectTarget::Dashboard {
            org_id: ctx.org_id,
            dashboard_uid: rec.uid.clone(),
            folder_uid: rec.folder_uid.clone(),
        },
    );
    if !dec.allowed {
        return Err(not_found());
    }
    export_dashboard(state.store.as_ref(), ctx.org_id, uid).await
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ODataImportActionBody {
    #[serde(default)]
    pub dry_run: bool,
    pub dashboard: Option<Value>,
    #[serde(default)]
    pub dashboards: Vec<ODataImportItem>,
    #[serde(default)]
    pub folder_uid: Option<String>,
}

/// One dashboard of an `Import` body. The provisioning fields of [`DashboardImportInput`]
/// (`isProvisioned`, `allowUiUpdates`, `externalId`) are set by the provisioning loader alone,
/// so a body naming them is refused.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ODataImportItem {
    pub raw_json: Value,
    #[serde(default)]
    pub folder_uid: Option<String>,
}

/// [`exec_odata_import_in_unit`] as one unit (jc-audit): its writes and its audit row commit together.
pub async fn exec_odata_import(
    state: &DaiyaState,
    ctx: &RequestCtx,
    body: ODataImportActionBody,
) -> CrResult<Value> {
    unit(exec_odata_import_in_unit(state, ctx, body)).await
}

async fn exec_odata_import_in_unit(
    state: &DaiyaState,
    ctx: &RequestCtx,
    body: ODataImportActionBody,
) -> CrResult<Value> {
    let dec = state.authorizer.check(
        ctx,
        Action::Propose,
        &ObjectTarget::Organization { org_id: ctx.org_id },
    );
    dec.into_result("Organization not found")?;

    let mut items = body.dashboards;
    if let Some(single) = body.dashboard {
        items.push(ODataImportItem {
            raw_json: single,
            folder_uid: body.folder_uid,
        });
    }
    let inputs: Vec<DashboardImportInput> = items
        .into_iter()
        .map(|item| DashboardImportInput {
            raw_json: item.raw_json,
            folder_uid: item.folder_uid,
            is_provisioned: false,
            allow_ui_updates: true,
            external_id: None,
        })
        .collect();
    let report = import_dashboards(
        state.store.as_ref(),
        state.installation_id,
        ctx.org_id,
        ctx.instance_id,
        ctx.actor_name(),
        &inputs,
        body.dry_run,
        &state.limits,
    )
    .await?;
    let uids: Vec<&str> = report.dashboards.iter().map(|d| d.uid.as_str()).collect();
    audit(
        state,
        ctx,
        "Import",
        format!("dashboards/{}", uids.join(",")),
    )
    .await?;
    Ok(report.to_json())
}

/// The datasources a plan writes pass the checks of the compat datasource routes: the
/// organization's Admin right, no read-only (provisioned) datasource written or overwritten, and a
/// URL the outbound guard admits ([`DaiyaState::check_datasource_url`]).
async fn check_plan_datasources(
    state: &DaiyaState,
    ctx: &RequestCtx,
    desired: &DesiredBundle,
) -> CrResult<()> {
    if desired.datasources.is_empty() {
        return Ok(());
    }
    state
        .authorizer
        .check(
            ctx,
            Action::Admin,
            &ObjectTarget::Organization { org_id: ctx.org_id },
        )
        .into_result("Organization not found")?;
    for ds in &desired.datasources {
        if ds.read_only {
            let why = format!(
                "datasources/{}: readOnly is set by provisioning, never by a plan",
                ds.uid
            );
            return Err(CrError::Unprocessable {
                detail: why.clone(),
                errors: vec![format!("/{why}")],
            });
        }
        if state
            .store
            .get_datasource(ctx.org_id, &ds.uid)
            .await?
            .is_some_and(|existing| existing.read_only)
        {
            return Err(CrError::Forbidden(format!(
                "Cannot update read-only data source '{}'",
                ds.uid
            )));
        }
        let mut record = ds.clone();
        record.org_id = ctx.org_id;
        state.check_datasource_url(&record)?;
    }
    Ok(())
}

/// [`exec_odata_plan_in_unit`] as one unit (jc-audit): its writes and its audit row commit together.
pub async fn exec_odata_plan(
    state: &DaiyaState,
    ctx: &RequestCtx,
    desired: DesiredBundle,
) -> CrResult<Value> {
    unit(exec_odata_plan_in_unit(state, ctx, desired)).await
}

async fn exec_odata_plan_in_unit(
    state: &DaiyaState,
    ctx: &RequestCtx,
    desired: DesiredBundle,
) -> CrResult<Value> {
    let dec = state.authorizer.check(
        ctx,
        Action::Propose,
        &ObjectTarget::Organization { org_id: ctx.org_id },
    );
    dec.into_result("Organization not found")?;
    check_plan_datasources(state, ctx, &desired).await?;

    let plan = compute_plan(
        state.store.as_ref(),
        state.installation_id,
        ctx.org_id,
        ctx.instance_id,
        ctx.actor_name(),
        &desired,
        &state.limits,
    )
    .await?;
    audit(state, ctx, "Plan", format!("plans/{}", plan.digest)).await?;
    Ok(serde_json::to_value(plan).unwrap_or(Value::Null))
}

/// Applies the plan the server holds under `digest` (what `Plan` answered); the approver's own
/// datasource rights are checked against that stored bundle.
/// [`exec_odata_apply_in_unit`] as one unit (jc-audit): its writes and its audit row commit together.
pub async fn exec_odata_apply(
    state: &DaiyaState,
    ctx: &RequestCtx,
    digest: &str,
) -> CrResult<Value> {
    unit(exec_odata_apply_in_unit(state, ctx, digest)).await
}

async fn exec_odata_apply_in_unit(
    state: &DaiyaState,
    ctx: &RequestCtx,
    digest: &str,
) -> CrResult<Value> {
    let dec = state.authorizer.check(
        ctx,
        Action::Approve,
        &ObjectTarget::Organization { org_id: ctx.org_id },
    );
    dec.into_result("Organization not found")?;
    let plan = load_plan(state.store.as_ref(), ctx.org_id, digest).await?;
    check_plan_datasources(state, ctx, plan.desired()).await?;

    let report = apply_plan(
        state.store.as_ref(),
        state.installation_id,
        ctx.org_id,
        ctx.instance_id,
        ctx.actor_name(),
        &plan,
        &state.limits,
    )
    .await?;
    audit(state, ctx, "Apply", format!("plans/{digest}")).await?;
    Ok(report.to_json())
}

/// Body of `Apply`: only the digest of a plan the server holds.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyBody {
    pub digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ODataPublicQueryActionBody {
    pub access_token: String,
    pub panel_id: u32,
    #[serde(flatten)]
    pub query: PublicDashboardQueryReq,
}

// =========================================================================
// HTTP Handlers for `/odata/v4/*`
// =========================================================================

async fn handle_odata_single_segment_get(
    State(state): State<DaiyaState>,
    OrgCtx(ctx): OrgCtx,
    Path(segment): Path<String>,
    RawQuery(raw_q): RawQuery,
) -> Response {
    let (name, parens) = split_paren_segment(&segment);

    // Unbound function `Allowed(operation=..., object=...)`
    if name == "Allowed" || name == "JoinedContext.Daiya.Allowed" {
        let args = parse_func_args(parens, raw_q.as_deref());
        let op = args.get("operation").map_or("read", String::as_str);
        let obj = args.get("object").map(String::as_str);
        let dec = state
            .registry
            .check_allowed(state.authorizer.as_ref(), &ctx, op, obj);
        return (StatusCode::OK, Json(dec)).into_response();
    }

    // Unbound function `Datasource.Health(uid=...)`, `Datasource.EditorSchema(uid=...)`, etc.
    if name == "Datasource.Health" || name == "Datasource_Health" {
        let args = parse_func_args(parens, raw_q.as_deref());
        let uid = args.get("uid").map_or("", String::as_str);
        return match exec_odata_datasource_health(&state, &ctx, uid).await {
            Ok(v) => (StatusCode::OK, Json(v)).into_response(),
            Err(e) => error_response(&e),
        };
    }
    if name == "Datasource.EditorSchema" || name == "Datasource_EditorSchema" {
        let args = parse_func_args(parens, raw_q.as_deref());
        let uid = args.get("uid").map_or("", String::as_str);
        return match exec_odata_datasource_editor_schema(&state, &ctx, uid).await {
            Ok(v) => (StatusCode::OK, Json(v)).into_response(),
            Err(e) => error_response(&e),
        };
    }
    if name == "Datasource.Resource" || name == "Datasource_Resource" {
        let mut args = parse_func_args(parens, raw_q.as_deref());
        let uid = args.remove("uid").unwrap_or_default();
        let path = args.remove("path").unwrap_or_default();
        return match exec_odata_datasource_resource(&state, &ctx, &uid, &path, args).await {
            Ok(v) => (StatusCode::OK, Json(v)).into_response(),
            Err(e) => error_response(&e),
        };
    }
    if name == "Dashboard.RenderPanel" || name == "Dashboard_RenderPanel" {
        let args = parse_func_args(parens, raw_q.as_deref());
        let uid = args.get("uid").map_or("", String::as_str);
        let panel_id = args
            .get("panelId")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1);
        let range = PanelTimeRange {
            from: args.get("from").cloned(),
            to: args.get("to").cloned(),
            ..PanelTimeRange::default()
        };
        return match panel_view(&state, &ctx.subject, ctx.org_id, uid, panel_id, range).await {
            Ok(pv) => (StatusCode::OK, Json(pv)).into_response(),
            Err(e) => error_response(&e),
        };
    }
    if name == "Dashboard.Export" || name == "Dashboard_Export" {
        let args = parse_func_args(parens, raw_q.as_deref());
        let uid = args.get("uid").map_or("", String::as_str);
        return match exec_odata_export(&state, &ctx, uid).await {
            Ok(val) => (StatusCode::OK, Json(val)).into_response(),
            Err(e) => error_response(&e),
        };
    }

    // EntitySet list or single entity by key `EntitySet('key')`
    match get_set(&state, &ctx, name, parens, raw_q.as_deref()).await {
        Ok(body) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "private, no-store")],
            Json(body),
        )
            .into_response(),
        Err(e) => error_response(&e),
    }
}

/// `GET Set` and `GET Set('key')`, at the route and inside `$batch`.
async fn get_set(
    state: &DaiyaState,
    ctx: &RequestCtx,
    name: &str,
    parens: Option<&str>,
    raw_query: Option<&str>,
) -> CrResult<Value> {
    let scope = scope_for_ctx(state, ctx);
    match parens {
        Some(key) => sets::entity(state, ctx, &scope, name, unquote_odata_key(key)).await,
        None => sets::list(state, ctx, &scope, name, raw_query).await,
    }
}

async fn handle_odata_two_segment_get(
    State(state): State<DaiyaState>,
    OrgCtx(ctx): OrgCtx,
    Path((seg1, seg2)): Path<(String, String)>,
    RawQuery(raw_q): RawQuery,
) -> Response {
    let (set_name, key_parens) = split_paren_segment(&seg1);
    let (sub_op, sub_parens) = split_paren_segment(&seg2);
    let op_short = sub_op
        .strip_prefix("JoinedContext.Daiya.")
        .unwrap_or(sub_op);

    // Bound functions on `Datasources('{uid}')/...` or `Dashboards('{uid}')/...`
    if let Some(k_raw) = key_parens {
        let uid = unquote_odata_key(k_raw);
        match (set_name, op_short) {
            ("Datasources", "Health") => {
                return match exec_odata_datasource_health(&state, &ctx, &uid).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => error_response(&e),
                };
            }
            ("Datasources", "EditorSchema") => {
                return match exec_odata_datasource_editor_schema(&state, &ctx, &uid).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => error_response(&e),
                };
            }
            ("Datasources", "Resource") => {
                let mut args = parse_func_args(sub_parens, raw_q.as_deref());
                let path = args.remove("path").unwrap_or_default();
                return match exec_odata_datasource_resource(&state, &ctx, &uid, &path, args).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => error_response(&e),
                };
            }
            ("Dashboards", "RenderPanel") => {
                let mut args = parse_func_args(sub_parens, raw_q.as_deref());
                let panel_id = args
                    .remove("panelId")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1);
                let range = PanelTimeRange {
                    from: args.remove("from"),
                    to: args.remove("to"),
                    interval_ms: args.remove("intervalMs").and_then(|s| s.parse().ok()),
                    max_data_points: args.remove("maxDataPoints").and_then(|s| s.parse().ok()),
                    variables: args,
                    picked: std::collections::BTreeMap::new(),
                };
                return match panel_view(&state, &ctx.subject, ctx.org_id, &uid, panel_id, range)
                    .await
                {
                    Ok(pv) => (
                        StatusCode::OK,
                        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
                        Json(pv),
                    )
                        .into_response(),
                    Err(e) => error_response(&e),
                };
            }
            ("Dashboards", "Export") => {
                return match exec_odata_export(&state, &ctx, &uid).await {
                    Ok(val) => (StatusCode::OK, Json(val)).into_response(),
                    Err(e) => error_response(&e),
                };
            }
            _ => {}
        }
    }

    // EntitySet/$count, EntitySet/$export, EntitySet/$allowed
    let scope = scope_for_ctx(&state, &ctx);
    let raw = raw_q.as_deref();

    match sub_op {
        "$count" => match sets::count(&state, &ctx, &scope, set_name, raw).await {
            Ok(count) => (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "text/plain")],
                count.to_string(),
            )
                .into_response(),
            Err(e) => error_response(&e),
        },
        "$export" => match sets::export(&state, &ctx, &scope, set_name, raw).await {
            Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
            Err(e) => error_response(&e),
        },
        "$allowed" => {
            let args = parse_func_args(sub_parens, raw);
            let op_name = args.get("op").map_or("read", String::as_str);
            let Some(op) = TableOp::parse(op_name) else {
                return error_response(&CrError::BadRequest(format!(
                    "unknown operation '{op_name}'"
                )));
            };
            let key = args.get("key").map(|k| Value::String(k.clone()));
            match sets::allowed(&state, &ctx, &scope, set_name, op, key).await {
                Ok(decision) => (StatusCode::OK, Json(decision)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        _ => error_response(&CrError::NotFound(format!(
            "unknown OData path '/odata/v4/{seg1}/{seg2}'"
        ))),
    }
}

async fn odata_public_query(
    state: &DaiyaState,
    client: Option<&ClientAddr>,
    body: Value,
) -> Response {
    let parsed: ODataPublicQueryActionBody = match serde_json::from_value(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response(&CrError::BadRequest(format!(
                "invalid PublicDashboard/Query body: {e}"
            )));
        }
    };
    match execute_public_dashboard_query(
        state,
        crate::public::public_caller_of(client),
        &parsed.access_token,
        parsed.panel_id,
        parsed.query,
    )
    .await
    {
        Ok(sets) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "private, no-store")],
            Json(json!({
                "@odata.context": "$metadata#Collection(JoinedContext.Daiya.FrameSet)",
                "value": sets,
            })),
        )
            .into_response(),
        Err(e) => error_response(&e),
    }
}

async fn handle_odata_single_segment_post(
    State(state): State<DaiyaState>,
    ctx: Result<OrgCtx, Response>,
    client: Option<Extension<ClientAddr>>,
    Path(segment): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> Response {
    let (name, _) = split_paren_segment(&segment);
    let op_short = name.strip_prefix("JoinedContext.Daiya.").unwrap_or(name);
    // The access token names the Organization of a public query, so it needs no context.
    if matches!(op_short, "PublicDashboard.Query" | "PublicDashboard_Query") {
        return match body {
            Ok(Json(body)) => odata_public_query(&state, client.as_deref(), body).await,
            Err(e) => e.into_response(),
        };
    }
    let OrgCtx(ctx) = match ctx {
        Ok(ctx) => ctx,
        Err(refused) => return refused,
    };
    let Json(body) = match body {
        Ok(body) => body,
        Err(e) => return e.into_response(),
    };

    match op_short {
        "Datasource.Query" | "Datasource_Query" => {
            let parsed: ODataQueryActionBody = match serde_json::from_value(body) {
                Ok(b) => b,
                Err(e) => {
                    return error_response(&CrError::BadRequest(format!(
                        "invalid Datasource/Query body: {e}"
                    )));
                }
            };
            match exec_odata_datasource_query(&state, &ctx, None, parsed).await {
                Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        "Import" => {
            let parsed: ODataImportActionBody = match serde_json::from_value(body) {
                Ok(b) => b,
                Err(e) => {
                    return error_response(&CrError::BadRequest(format!(
                        "invalid Import body: {e}"
                    )));
                }
            };
            match exec_odata_import(&state, &ctx, parsed).await {
                Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        "Plan" => {
            let bundle_val = body.get("bundle").cloned().unwrap_or(body);
            let desired: DesiredBundle = match serde_json::from_value(bundle_val) {
                Ok(d) => d,
                Err(e) => {
                    return error_response(&CrError::BadRequest(format!(
                        "invalid Plan bundle: {e}"
                    )));
                }
            };
            match exec_odata_plan(&state, &ctx, desired).await {
                Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        "Apply" => {
            let parsed: ApplyBody = match serde_json::from_value(body) {
                Ok(b) => b,
                Err(e) => {
                    return error_response(&CrError::BadRequest(format!(
                        "invalid Apply body: {e}; send {{\"digest\": \"<digest a Plan answered>\"}}"
                    )));
                }
            };
            match exec_odata_apply(&state, &ctx, &parsed.digest).await {
                Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        "PublicDashboard.Rotate" | "PublicDashboard_Rotate" => {
            let uid = body.get("uid").and_then(Value::as_str).unwrap_or("");
            match rotate_public_dashboard_token(&state, &ctx, uid).await {
                Ok(rec) => (StatusCode::OK, Json(rec)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        _ => error_response(&CrError::NotFound(format!(
            "unknown OData action '{segment}'"
        ))),
    }
}

async fn handle_odata_two_segment_post(
    State(state): State<DaiyaState>,
    OrgCtx(ctx): OrgCtx,
    Path((seg1, seg2)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Response {
    let (set_name, key_parens) = split_paren_segment(&seg1);
    let uid = key_parens.map_or_else(String::new, unquote_odata_key);
    let op_short = seg2.strip_prefix("JoinedContext.Daiya.").unwrap_or(&seg2);
    let payload = body.map_or(Value::Object(Default::default()), |Json(v)| v);

    match (set_name, op_short) {
        ("Datasources", "Query") => {
            let parsed: ODataQueryActionBody = match serde_json::from_value(payload) {
                Ok(b) => b,
                Err(e) => {
                    return error_response(&CrError::BadRequest(format!(
                        "invalid Query body: {e}"
                    )));
                }
            };
            match exec_odata_datasource_query(&state, &ctx, Some(&uid), parsed).await {
                Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        ("Datasources", "Health") => match exec_odata_datasource_health(&state, &ctx, &uid).await {
            Ok(v) => (StatusCode::OK, Json(v)).into_response(),
            Err(e) => error_response(&e),
        },
        ("Dashboards", "Export") => match exec_odata_export(&state, &ctx, &uid).await {
            Ok(v) => (StatusCode::OK, Json(v)).into_response(),
            Err(e) => error_response(&e),
        },
        ("PublicDashboards", "Rotate") => {
            match rotate_public_dashboard_token(&state, &ctx, &uid).await {
                Ok(rec) => (StatusCode::OK, Json(rec)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        _ => error_response(&CrError::NotFound(format!(
            "unknown bound OData action '/odata/v4/{seg1}/{seg2}'"
        ))),
    }
}

/// OData 4.01 JSON `$batch` request envelope (R10a).
#[derive(Debug, Deserialize)]
pub struct ODataBatchRequest {
    pub requests: Vec<ODataBatchSubRequest>,
}

#[derive(Debug, Deserialize)]
pub struct ODataBatchSubRequest {
    pub id: String,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub body: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ODataBatchSubResponse {
    pub id: String,
    pub status: u16,
    pub body: Value,
}

async fn odata_batch(
    State(state): State<DaiyaState>,
    OrgCtx(ctx): OrgCtx,
    Json(batch): Json<ODataBatchRequest>,
) -> Response {
    if batch.requests.len() > MAX_BATCH_REQUESTS {
        return error_response(&CrError::BadRequest(format!(
            "$batch carries {} requests, the limit is {MAX_BATCH_REQUESTS}",
            batch.requests.len()
        )));
    }
    let mut responses = Vec::with_capacity(batch.requests.len());

    for sub in batch.requests {
        let rel = sub
            .url
            .trim_start_matches('/')
            .trim_start_matches("odata/v4/");
        let (path_part, query_part) = rel
            .split_once('?')
            .map_or((rel, None), |(p, q)| (p, Some(q)));
        let (seg_name, parens) = split_paren_segment(path_part);

        let (status, body) = match sub.method.to_ascii_uppercase().as_str() {
            "GET" if seg_name == "Allowed" => {
                let args = parse_func_args(parens, query_part);
                let op = args.get("operation").map_or("read", String::as_str);
                let obj = args.get("object").map(String::as_str);
                let dec = state
                    .registry
                    .check_allowed(state.authorizer.as_ref(), &ctx, op, obj);
                (200, serde_json::to_value(dec).unwrap_or(Value::Null))
            }
            "GET" => match get_set(&state, &ctx, seg_name, parens, query_part).await {
                Ok(body) => (200, body),
                Err(e) => (e.status_code(), e.to_odata_error()),
            },
            "POST" if seg_name == "Import" => {
                let payload = sub.body.unwrap_or(Value::Null);
                match serde_json::from_value::<ODataImportActionBody>(payload) {
                    Ok(imp) => match exec_odata_import(&state, &ctx, imp).await {
                        Ok(v) => (200, v),
                        Err(e) => (e.status_code(), e.to_odata_error()),
                    },
                    Err(e) => {
                        let ce = CrError::BadRequest(e.to_string());
                        (ce.status_code(), ce.to_odata_error())
                    }
                }
            }
            _ => {
                let ce = CrError::NotImplemented(format!(
                    "unsupported batch subrequest {} {}",
                    sub.method, sub.url
                ));
                (ce.status_code(), ce.to_odata_error())
            }
        };

        responses.push(ODataBatchSubResponse {
            id: sub.id,
            status,
            body,
        });
    }

    (StatusCode::OK, Json(json!({ "responses": responses }))).into_response()
}

/// Builds the `/odata/v4` router (R10, R10a).
pub fn odata_router() -> Router<DaiyaState> {
    Router::new()
        .route("/odata/v4", get(odata_service_doc))
        .route("/odata/v4/$metadata", get(odata_metadata))
        .route("/odata/v4/$batch", post(odata_batch))
        .route(
            "/odata/v4/{seg1}",
            get(handle_odata_single_segment_get).post(handle_odata_single_segment_post),
        )
        .route(
            "/odata/v4/{seg1}/{seg2}",
            get(handle_odata_two_segment_get).post(handle_odata_two_segment_post),
        )
}
