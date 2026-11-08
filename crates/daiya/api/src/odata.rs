#![forbid(unsafe_code)]
//! Native OData 4.01 service at `/odata/v4` (R10, R10a):
//! - `$metadata` (CSDL XML and JSON)
//! - `$batch` (OData 4.01 JSON batch requests)
//! - All 8 entity sets (`Dashboards`, `DashboardVersions`, `Folders`, `Datasources`,
//!   `LibraryPanels`, `PublicDashboards`, `QueryCacheStats`, `ImportReports`) served through
//!   `odata-table` (`$filter`, `$orderby`, `$select`, `$top`, `$skip`, `$count`, `$skiptoken`,
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
use crate::state::{DaiyaState, error_response, resolve_request_ctx};
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use daiya_import::{
    DashboardImportInput, DesiredBundle, PlanBundle, apply_plan, compute_plan, export_dashboard,
    import_dashboards,
};
use daiya_model::{Action, CrError, CrResult, ObjectTarget, RequestCtx};
use daiya_query::{QueryCtx, ResourceReq};
use jc_router::ClientAddr;
use odata_table::{
    MemoryTable, Operation as TableOp, Scope, Table, TableError, allowed as table_allowed,
    export as table_export, key_options, query as table_query,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

const MAX_PAGE_SIZE: u64 = 500;
/// Most sub-requests one `$batch` may carry.
pub const MAX_BATCH_REQUESTS: usize = 100;

/// Builds a `MemoryTable` snapshot for `set_name` in `ctx.org_id` using the single `odata-table`
/// `Service` declaration from `daiya-model::registry` (R10).
async fn build_entity_table(
    state: &DaiyaState,
    ctx: &RequestCtx,
    set_name: &str,
) -> CrResult<MemoryTable> {
    let model = Arc::new(state.registry.service.model());
    let info = state
        .registry
        .service
        .info(model, set_name)
        .ok_or_else(|| CrError::NotFound(format!("unknown OData entity set '{set_name}'")))?;

    let rows: Vec<Value> = match set_name {
        "Dashboards" => state
            .store
            .list_dashboards(ctx.org_id)
            .await?
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
            })
            .map(|d| {
                json!({
                    "uid": d.uid,
                    "title": d.title,
                    "slug": d.slug,
                    "folderUid": d.folder_uid,
                    "panelCount": d.panel_count,
                    "version": d.version,
                    "schemaVersion": d.schema_version,
                    "isProvisioned": d.is_provisioned,
                    "updatedBy": d.updated_by,
                    "updatedAt": d.updated_at.to_rfc3339(),
                })
            })
            .collect(),
        "DashboardVersions" => {
            let dashboards = state.store.list_dashboards(ctx.org_id).await?;
            let mut vers = Vec::new();
            for d in dashboards {
                for v in state
                    .store
                    .list_dashboard_versions(ctx.org_id, &d.uid)
                    .await?
                {
                    vers.push(json!({
                        "id": format!("{}:{}", v.dashboard_uid, v.version),
                        "dashboardUid": v.dashboard_uid,
                        "version": v.version,
                        "parentVersion": v.parent_version,
                        "restoredFrom": v.restored_from,
                        "message": v.message,
                        "createdBy": v.created_by,
                        "createdAt": v.created_at.to_rfc3339(),
                    }));
                }
            }
            vers
        }
        "Folders" => state
            .store
            .list_folders(ctx.org_id)
            .await?
            .into_iter()
            .map(|f| {
                json!({
                    "uid": f.uid,
                    "parentUid": f.parent_uid,
                    "title": f.title,
                    "version": f.version,
                    "managedBy": f.managed_by,
                    "updatedAt": f.updated_at.to_rfc3339(),
                })
            })
            .collect(),
        "Datasources" => state
            .store
            .list_datasources(ctx.org_id)
            .await?
            .into_iter()
            .filter(|ds| {
                state
                    .authorizer
                    .check(
                        ctx,
                        Action::Read,
                        &ObjectTarget::Datasource {
                            org_id: ctx.org_id,
                            datasource_uid: ds.uid.clone(),
                        },
                    )
                    .allowed
            })
            .map(|ds| {
                json!({
                    "uid": ds.uid,
                    "name": ds.name,
                    "type": ds.kind,
                    "access": ds.access,
                    "url": ds.url,
                    "isDefault": ds.is_default,
                    "readOnly": ds.read_only,
                    "wasmDigest": ds.wasm_digest,
                    "version": ds.version,
                })
            })
            .collect(),
        "LibraryPanels" => state
            .store
            .list_library_panels(ctx.org_id)
            .await?
            .into_iter()
            .map(|p| {
                json!({
                    "uid": p.uid,
                    "folderUid": p.folder_uid,
                    "name": p.name,
                    "type": p.panel_type,
                    "description": p.description,
                    "version": p.version,
                })
            })
            .collect(),
        "PublicDashboards" => state
            .store
            .list_public_dashboards(ctx.org_id)
            .await?
            .into_iter()
            .map(|p| {
                json!({
                    "uid": p.uid,
                    "dashboardUid": p.dashboard_uid,
                    "isEnabled": p.is_enabled,
                    "timeSelectionEnabled": p.time_selection_enabled,
                    "annotationsEnabled": p.annotations_enabled,
                    "showQueryText": p.show_query_text,
                    "share": p.share.as_str(),
                })
            })
            .collect(),
        "QueryCacheStats" => {
            let mut stats = state.query_cache.stats_for_org(ctx.org_id, ctx.instance_id);
            let stored = state.store.list_query_cache_stats(ctx.org_id).await?;
            for s in stored {
                if !stats.iter().any(|x| x.id == s.id) {
                    stats.push(s);
                }
            }
            stats
                .into_iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "datasourceUid": s.datasource_uid,
                        "hits": s.hits,
                        "misses": s.misses,
                        "entries": s.entries,
                        "bytesUsed": s.bytes_used,
                    })
                })
                .collect()
        }
        "ImportReports" => state
            .store
            .list_import_reports(ctx.org_id)
            .await?
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "dryRun": r.dry_run,
                    "status": r.status,
                    "dashboardCount": r.dashboard_count,
                    "refusedCount": r.refused_count,
                    "mappedCount": r.mapped_count,
                    "createdAt": r.created_at.to_rfc3339(),
                })
            })
            .collect(),
        other => {
            return Err(CrError::NotFound(format!(
                "unknown OData entity set '{other}'"
            )));
        }
    };

    MemoryTable::new(info, rows).map_err(map_table_error)
}

fn map_table_error(err: TableError) -> CrError {
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

fn parse_odata_query_options(
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

    let ds_rec = state.store.get_datasource(ctx.org_id, uid).await?;
    let drv = state.get_datasource_driver(ctx.org_id, uid);
    if ds_rec.is_none() && drv.is_none() {
        return Err(CrError::NotFound(format!("Datasource '{uid}' not found")));
    }

    if let Some(driver) = drv {
        let q_ctx = QueryCtx::new(ctx.org_id, ctx.instance_id, ctx.subject.clone(), 0, 1000);
        let h = driver.health(&q_ctx).await;
        return Ok(json!({
            "@odata.context": "$metadata#JoinedContext.Daiya.HealthStatus",
            "status": h.status,
            "message": h.message,
        }));
    }

    Ok(json!({
        "@odata.context": "$metadata#JoinedContext.Daiya.HealthStatus",
        "status": "ok",
        "message": "Data source is working",
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

    if let Some(driver) = state.get_datasource_driver(ctx.org_id, uid) {
        let schema = driver.editor();
        return Ok(serde_json::to_value(schema).unwrap_or(Value::Null));
    }

    let ds_rec = state
        .store
        .get_datasource(ctx.org_id, uid)
        .await?
        .ok_or_else(|| CrError::NotFound(format!("Datasource '{uid}' not found")))?;

    Ok(json!({
        "kind": ds_rec.kind,
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
        .get_datasource_driver(ctx.org_id, uid)
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
    pub dashboards: Vec<DashboardImportInput>,
    #[serde(default)]
    pub folder_uid: Option<String>,
}

pub async fn exec_odata_import(
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

    let mut inputs = body.dashboards;
    if let Some(single) = body.dashboard {
        inputs.push(DashboardImportInput {
            raw_json: single,
            folder_uid: body.folder_uid,
            is_provisioned: false,
            allow_ui_updates: true,
            external_id: None,
        });
    }
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
    Ok(report.to_json())
}

pub async fn exec_odata_plan(
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
    Ok(serde_json::to_value(plan).unwrap_or(Value::Null))
}

pub async fn exec_odata_apply(
    state: &DaiyaState,
    ctx: &RequestCtx,
    plan: PlanBundle,
) -> CrResult<Value> {
    let dec = state.authorizer.check(
        ctx,
        Action::Approve,
        &ObjectTarget::Organization { org_id: ctx.org_id },
    );
    dec.into_result("Organization not found")?;

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
    Ok(report.to_json())
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
    headers: HeaderMap,
    Path(segment): Path<String>,
    RawQuery(raw_q): RawQuery,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
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
    let table = match build_entity_table(&state, &ctx, name).await {
        Ok(t) => t,
        Err(e) => return error_response(&e),
    };
    let scope = scope_for_ctx(&state, &ctx);

    if let Some(key_raw) = parens {
        let key_val = Value::String(unquote_odata_key(key_raw));
        let opts = match key_options(table.info(), &key_val, None) {
            Ok(o) => o,
            Err(e) => return error_response(&map_table_error(e)),
        };
        match table_query(&table, &opts, &scope, 1).await {
            Ok(mut ans) => {
                if let Some(row) = ans.rows.pop() {
                    (
                        StatusCode::OK,
                        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
                        Json(row),
                    )
                        .into_response()
                } else {
                    error_response(&CrError::NotFound(format!("{name} entity not found")))
                }
            }
            Err(e) => error_response(&map_table_error(e)),
        }
    } else {
        let opts = match parse_odata_query_options(&table.info().model, name, raw_q.as_deref()) {
            Ok(o) => o,
            Err(e) => return error_response(&e),
        };
        match table_query(&table, &opts, &scope, MAX_PAGE_SIZE).await {
            Ok(ans) => {
                let mut payload = json!({
                    "@odata.context": format!("$metadata#{name}"),
                    "value": ans.rows,
                });
                if let Some(c) = ans.count {
                    payload["@odata.count"] = Value::from(c);
                }
                if let Some(next) = ans.next {
                    payload["@odata.nextLink"] = Value::String(format!("{name}?$skiptoken={next}"));
                }
                (
                    StatusCode::OK,
                    [(axum::http::header::CACHE_CONTROL, "private, no-store")],
                    Json(payload),
                )
                    .into_response()
            }
            Err(e) => error_response(&map_table_error(e)),
        }
    }
}

async fn handle_odata_two_segment_get(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path((seg1, seg2)): Path<(String, String)>,
    RawQuery(raw_q): RawQuery,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
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
    let table = match build_entity_table(&state, &ctx, set_name).await {
        Ok(t) => t,
        Err(e) => return error_response(&e),
    };
    let scope = scope_for_ctx(&state, &ctx);

    match sub_op {
        "$count" => {
            let mut opts =
                match parse_odata_query_options(&table.info().model, set_name, raw_q.as_deref()) {
                    Ok(o) => o,
                    Err(e) => return error_response(&e),
                };
            opts.count = Some(true);
            opts.top = Some(0);
            match table_query(&table, &opts, &scope, 1).await {
                Ok(ans) => (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "text/plain")],
                    ans.count.unwrap_or(0).to_string(),
                )
                    .into_response(),
                Err(e) => error_response(&map_table_error(e)),
            }
        }
        "$export" => {
            let opts =
                match parse_odata_query_options(&table.info().model, set_name, raw_q.as_deref()) {
                    Ok(o) => o,
                    Err(e) => return error_response(&e),
                };
            match table_export(&table, &opts, &scope, 100, 10_000).await {
                Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
                Err(e) => error_response(&map_table_error(e)),
            }
        }
        "$allowed" => {
            let args = parse_func_args(sub_parens, raw_q.as_deref());
            let op_name = args.get("op").map_or("read", String::as_str);
            let Some(op) = TableOp::parse(op_name) else {
                return error_response(&CrError::BadRequest(format!(
                    "unknown operation '{op_name}'"
                )));
            };
            let key_val = args.get("key").map(|k| Value::String(k.clone()));
            match table_allowed(&table, &scope, op, key_val.as_ref()).await {
                Ok(dec) => (StatusCode::OK, Json(dec.to_json())).into_response(),
                Err(e) => error_response(&map_table_error(e)),
            }
        }
        _ => error_response(&CrError::NotFound(format!(
            "unknown OData path '/odata/v4/{seg1}/{seg2}'"
        ))),
    }
}

async fn handle_odata_single_segment_post(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    client: Option<Extension<ClientAddr>>,
    Path(segment): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let (name, _) = split_paren_segment(&segment);
    let op_short = name.strip_prefix("JoinedContext.Daiya.").unwrap_or(name);

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
            let plan_val = body.get("plan").cloned().unwrap_or(body);
            let plan: PlanBundle = match serde_json::from_value(plan_val) {
                Ok(p) => p,
                Err(e) => {
                    return error_response(&CrError::BadRequest(format!(
                        "invalid Apply plan: {e}"
                    )));
                }
            };
            match exec_odata_apply(&state, &ctx, plan).await {
                Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                Err(e) => error_response(&e),
            }
        }
        "PublicDashboard.Query" | "PublicDashboard_Query" => {
            let parsed: ODataPublicQueryActionBody = match serde_json::from_value(body) {
                Ok(b) => b,
                Err(e) => {
                    return error_response(&CrError::BadRequest(format!(
                        "invalid PublicDashboard/Query body: {e}"
                    )));
                }
            };
            let ip = client.map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |c| c.0.0);
            match execute_public_dashboard_query(
                &state,
                ip,
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
    headers: HeaderMap,
    Path((seg1, seg2)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
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
    headers: HeaderMap,
    Json(batch): Json<ODataBatchRequest>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
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
            "GET" => match build_entity_table(&state, &ctx, seg_name).await {
                Ok(table) => {
                    let scope = scope_for_ctx(&state, &ctx);
                    let opts = parse_odata_query_options(&table.info().model, seg_name, query_part)
                        .unwrap_or_default();
                    match table_query(&table, &opts, &scope, MAX_PAGE_SIZE).await {
                        Ok(ans) => (200, json!({ "value": ans.rows })),
                        Err(e) => {
                            let ce = map_table_error(e);
                            (ce.status_code(), ce.to_odata_error())
                        }
                    }
                }
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
