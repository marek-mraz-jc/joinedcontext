#![forbid(unsafe_code)]
//! Thin Grafana HTTP API compatibility adapters onto the same OData / internal operations (R2, R10a,
//! listed in `COMPAT.md`), serving `provision.sh` and `joinedcontext-portal` (`src/grafana.rs`,
//! `src/api/grafana.rs`).

use crate::odata::{
    ODataImportActionBody, ODataQueryActionBody, exec_odata_datasource_health,
    exec_odata_datasource_query, exec_odata_datasource_resource, exec_odata_import,
};
use crate::render::{PanelTimeRange, panel_view};
use crate::state::{DaiyaState, error_response, resolve_request_ctx};
use axum::extract::{Path, Query as AxumQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use daiya_import::export_dashboard;
use daiya_model::RequestCtx;

/// `ctx` with `org_id` as the active organization (the authorizer only decides inside it).
fn ctx_in_org(ctx: &RequestCtx, org_id: OrgId) -> RequestCtx {
    let mut c = ctx.clone();
    c.org_id = org_id;
    c
}

fn can_read_folder(state: &DaiyaState, ctx: &RequestCtx, folder_uid: &str) -> bool {
    state
        .authorizer
        .check(
            ctx,
            Action::Read,
            &ObjectTarget::Folder {
                org_id: ctx.org_id,
                folder_uid: folder_uid.to_owned(),
            },
        )
        .allowed
}
use daiya_model::{Action, CrError, DatasourceRecord, FolderRecord, ObjectTarget, OrgId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

async fn compat_health() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "commit": "daiya",
            "database": "ok",
            "version": "12.4.14"
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct CreateOrgReq {
    name: String,
}

async fn compat_create_org(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Json(req): Json<CreateOrgReq>,
) -> Response {
    // Creating an organization is an installation admin act: admin of the main organization.
    let ctx = ctx_in_org(&resolve_request_ctx(&state, &headers), state.default_org_id);
    let dec = state.authorizer.check(
        &ctx,
        Action::Admin,
        &ObjectTarget::Organization {
            org_id: state.default_org_id,
        },
    );
    if let Err(e) = dec.into_result("Organization not found") {
        return error_response(&e);
    }
    let mut reg = state.org_registry.write();
    if reg.by_name.contains_key(&req.name) {
        return error_response(&CrError::Conflict("Organization name taken".into()));
    }
    let new_org_id = OrgId::new();
    let (num, _, _) = reg.register_org(&req.name, new_org_id);
    (
        StatusCode::OK,
        Json(json!({
            "orgId": num,
            "id": num,
            "uuid": new_org_id,
            "message": "Organization created",
        })),
    )
        .into_response()
}

async fn compat_get_org_by_name(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let found = state.org_registry.read().by_name.get(&name).copied();
    let Some((num, org_id)) = found else {
        return error_response(&CrError::NotFound("Organization not found".into()));
    };
    // Only a member (or the installation admin) learns that the organization exists.
    let member = state
        .authorizer
        .check(
            &ctx_in_org(&ctx, org_id),
            Action::Read,
            &ObjectTarget::Organization { org_id },
        )
        .allowed;
    let installation_admin = state
        .authorizer
        .check(
            &ctx_in_org(&ctx, state.default_org_id),
            Action::Admin,
            &ObjectTarget::Organization {
                org_id: state.default_org_id,
            },
        )
        .allowed;
    if !member && !installation_admin {
        return error_response(&CrError::NotFound("Organization not found".into()));
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": num,
            "orgId": num,
            "uuid": org_id,
            "name": name,
        })),
    )
        .into_response()
}

async fn compat_get_current_org(State(state): State<DaiyaState>, headers: HeaderMap) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Organization { org_id: ctx.org_id },
    );
    if let Err(e) = dec.into_result("Organization not found") {
        return error_response(&e);
    }
    let reg = state.org_registry.read();
    let (num, name) = reg
        .by_num
        .iter()
        .find(|(_, (_, oid))| *oid == ctx.org_id)
        .map(|(n, (nm, _))| (*n, nm.clone()))
        .unwrap_or((1, "Main Org.".to_owned()));
    (
        StatusCode::OK,
        Json(json!({
            "id": num,
            "name": name,
            "uuid": ctx.org_id,
        })),
    )
        .into_response()
}

async fn compat_list_datasources(State(state): State<DaiyaState>, headers: HeaderMap) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    match state.store.list_datasources(ctx.org_id).await {
        Ok(list) => {
            let items: Vec<Value> = list
                .into_iter()
                .filter(|ds| {
                    state
                        .authorizer
                        .check(
                            &ctx,
                            Action::Read,
                            &ObjectTarget::Datasource {
                                org_id: ctx.org_id,
                                datasource_uid: ds.uid.clone(),
                            },
                        )
                        .allowed
                })
                .enumerate()
                .map(|(idx, ds)| {
                    json!({
                        "id": (idx + 1) as i64,
                        "uid": ds.uid,
                        "name": ds.name,
                        "type": ds.kind,
                        "access": ds.access,
                        "url": ds.url,
                        "isDefault": ds.is_default,
                        "jsonData": ds.json_data,
                        "readOnly": ds.read_only,
                    })
                })
                .collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(e) => error_response(&e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertDatasourceReq {
    #[serde(default)]
    uid: Option<String>,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default = "default_proxy")]
    access: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    json_data: Value,
    #[serde(default)]
    secret_refs: BTreeMap<String, String>,
    #[serde(default)]
    allow_hosts: Vec<String>,
    #[serde(default)]
    wasm_digest: Option<String>,
}

fn default_proxy() -> String {
    "proxy".to_owned()
}

async fn compat_add_datasource(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Json(req): Json<UpsertDatasourceReq>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Propose,
        &ObjectTarget::Organization { org_id: ctx.org_id },
    );
    if let Err(e) = dec.into_result("Organization not found") {
        return error_response(&e);
    }

    let uid = req.uid.unwrap_or_else(|| req.name.clone());
    if let Ok(Some(_)) = state.store.get_datasource(ctx.org_id, &uid).await {
        return error_response(&CrError::Conflict(
            "data source with the same uid already exists".into(),
        ));
    }
    if let Ok(Some(_)) = state
        .store
        .get_datasource_by_name(ctx.org_id, &req.name)
        .await
    {
        return error_response(&CrError::Conflict(
            "data source with the same name already exists".into(),
        ));
    }

    let mut allow_hosts = req.allow_hosts;
    if allow_hosts.is_empty()
        && let Ok(u) = url::Url::parse(&req.url)
        && let Some(h) = u.host_str()
    {
        allow_hosts.push(h.to_owned());
    }

    let now = Utc::now();
    let rec = DatasourceRecord {
        installation_id: state.installation_id,
        org_id: ctx.org_id,
        instance_id: ctx.instance_id,
        project_id: None,
        uid: uid.clone(),
        name: req.name.clone(),
        kind: req.kind,
        access: req.access,
        url: req.url,
        is_default: req.is_default,
        read_only: req.read_only,
        json_data: req.json_data,
        secret_refs: req.secret_refs,
        allow_hosts,
        wasm_digest: req.wasm_digest,
        version: 1,
        created_at: now,
        updated_at: now,
    };

    match state.store.upsert_datasource(rec).await {
        Ok(saved) => (
            StatusCode::OK,
            Json(json!({
                "message": "Datasource added",
                "id": 1,
                "name": saved.name,
                "datasource": {
                    "uid": saved.uid,
                    "name": saved.name,
                    "type": saved.kind,
                    "url": saved.url,
                    "access": saved.access,
                    "jsonData": saved.json_data,
                }
            })),
        )
            .into_response(),
        Err(e) => error_response(&e),
    }
}

async fn compat_update_datasource_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
    Json(req): Json<UpsertDatasourceReq>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Propose,
        &ObjectTarget::Datasource {
            org_id: ctx.org_id,
            datasource_uid: uid.clone(),
        },
    );
    if let Err(e) = dec.into_result("Data source not found") {
        return error_response(&e);
    }

    let now = Utc::now();
    let rec = DatasourceRecord {
        installation_id: state.installation_id,
        org_id: ctx.org_id,
        instance_id: ctx.instance_id,
        project_id: None,
        uid: uid.clone(),
        name: req.name.clone(),
        kind: req.kind,
        access: req.access,
        url: req.url,
        is_default: req.is_default,
        read_only: req.read_only,
        json_data: req.json_data,
        secret_refs: req.secret_refs,
        allow_hosts: req.allow_hosts,
        wasm_digest: req.wasm_digest,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    match state.store.upsert_datasource(rec).await {
        Ok(saved) => (
            StatusCode::OK,
            Json(json!({
                "message": "Datasource updated",
                "id": 1,
                "name": saved.name,
                "datasource": saved,
            })),
        )
            .into_response(),
        Err(e) => error_response(&e),
    }
}

async fn compat_get_datasource_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Datasource {
            org_id: ctx.org_id,
            datasource_uid: uid.clone(),
        },
    );
    if let Err(e) = dec.into_result("Data source not found") {
        return error_response(&e);
    }
    match state.store.get_datasource(ctx.org_id, &uid).await {
        Ok(Some(ds)) => {
            // Never return secret values: only list secureJsonFields booleans like Grafana
            let secure_fields: BTreeMap<String, bool> =
                ds.secret_refs.keys().map(|k| (k.clone(), true)).collect();
            (
                StatusCode::OK,
                Json(json!({
                    "id": 1,
                    "uid": ds.uid,
                    "name": ds.name,
                    "type": ds.kind,
                    "access": ds.access,
                    "url": ds.url,
                    "isDefault": ds.is_default,
                    "jsonData": ds.json_data,
                    "secureJsonFields": secure_fields,
                    "readOnly": ds.read_only,
                    "version": ds.version,
                })),
            )
                .into_response()
        }
        Ok(None) => error_response(&CrError::NotFound("Data source not found".into())),
        Err(e) => error_response(&e),
    }
}

async fn compat_delete_datasource_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Datasource {
            org_id: ctx.org_id,
            datasource_uid: uid.clone(),
        },
    );
    if let Err(e) = dec.into_result("Data source not found") {
        return error_response(&e);
    }
    match state.store.delete_datasource(ctx.org_id, &uid).await {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({ "message": "Data source deleted", "id": 1 })),
        )
            .into_response(),
        Ok(false) => error_response(&CrError::NotFound("Data source not found".into())),
        Err(e) => error_response(&e),
    }
}

async fn compat_datasource_health(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    match exec_odata_datasource_health(&state, &ctx, &uid).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => error_response(&e),
    }
}

async fn compat_datasource_resource(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path((uid, path)): Path<(String, String)>,
    AxumQuery(params): AxumQuery<BTreeMap<String, String>>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    match exec_odata_datasource_resource(&state, &ctx, &uid, &path, params).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => error_response(&e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateFolderReq {
    #[serde(default)]
    uid: Option<String>,
    title: String,
    #[serde(default)]
    parent_uid: Option<String>,
}

async fn compat_create_folder(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Json(req): Json<CreateFolderReq>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Propose,
        &ObjectTarget::Organization { org_id: ctx.org_id },
    );
    if let Err(e) = dec.into_result("Organization not found") {
        return error_response(&e);
    }
    let uid = req.uid.filter(|s| !s.is_empty()).unwrap_or_else(|| {
        format!(
            "folder-{}",
            req.title.to_ascii_lowercase().replace(' ', "-")
        )
    });
    let now = Utc::now();
    let rec = FolderRecord {
        installation_id: state.installation_id,
        org_id: ctx.org_id,
        instance_id: ctx.instance_id,
        project_id: None,
        uid: uid.clone(),
        parent_uid: req.parent_uid,
        title: req.title,
        version: 1,
        managed_by: None,
        created_by: ctx.actor_name().to_owned(),
        updated_by: ctx.actor_name().to_owned(),
        created_at: now,
        updated_at: now,
    };
    match state.store.upsert_folder(rec).await {
        Ok(saved) => (
            StatusCode::OK,
            Json(json!({
                "id": 1,
                "uid": saved.uid,
                "title": saved.title,
                "url": format!("/dashboards/f/{}", saved.uid),
                "version": saved.version,
            })),
        )
            .into_response(),
        Err(e) => error_response(&e),
    }
}

async fn compat_list_folders(State(state): State<DaiyaState>, headers: HeaderMap) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    match state.store.list_folders(ctx.org_id).await {
        Ok(list) => {
            let hits: Vec<Value> = list
                .into_iter()
                .filter(|f| can_read_folder(&state, &ctx, &f.uid))
                .enumerate()
                .map(|(i, f)| {
                    json!({
                        "id": (i + 1) as i64,
                        "uid": f.uid,
                        "title": f.title,
                        "parentUid": f.parent_uid,
                    })
                })
                .collect();
            (StatusCode::OK, Json(hits)).into_response()
        }
        Err(e) => error_response(&e),
    }
}

async fn compat_get_folder_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    if !can_read_folder(&state, &ctx, &uid) {
        return error_response(&CrError::NotFound("Folder not found".into()));
    }
    match state.store.get_folder(ctx.org_id, &uid).await {
        Ok(Some(f)) => (
            StatusCode::OK,
            Json(json!({
                "id": 1,
                "uid": f.uid,
                "title": f.title,
                "parentUid": f.parent_uid,
                "version": f.version,
            })),
        )
            .into_response(),
        Ok(None) => error_response(&CrError::NotFound("Folder not found".into())),
        Err(e) => error_response(&e),
    }
}

async fn compat_delete_folder_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Folder {
            org_id: ctx.org_id,
            folder_uid: uid.clone(),
        },
    );
    if let Err(e) = dec.into_result("Folder not found") {
        return error_response(&e);
    }
    match state.store.delete_folder(ctx.org_id, &uid).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "message": "Folder deleted" }))).into_response(),
        Ok(false) => error_response(&CrError::NotFound("Folder not found".into())),
        Err(e) => error_response(&e),
    }
}

async fn compat_post_dashboard(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let folder_uid = body
        .get("folderUid")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let imp_body = ODataImportActionBody {
        dry_run: false,
        dashboard: Some(body),
        dashboards: Vec::new(),
        folder_uid: folder_uid.clone(),
    };
    match exec_odata_import(&state, &ctx, imp_body).await {
        Ok(rep) => {
            let first = &rep["dashboards"][0];
            let uid = first["uid"].as_str().unwrap_or("");
            let title = first["title"].as_str().unwrap_or("");
            let slug = title.to_ascii_lowercase().replace(' ', "-");
            (
                StatusCode::OK,
                Json(json!({
                    "status": "success",
                    "slug": slug,
                    "version": 1,
                    "id": 1,
                    "uid": uid,
                    "url": format!("/d/{uid}/{slug}"),
                    "folderUid": folder_uid.unwrap_or_default(),
                })),
            )
                .into_response()
        }
        Err(e) => error_response(&e),
    }
}

async fn compat_get_dashboard_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let Some(rec) = (match state.store.get_dashboard(ctx.org_id, &uid).await {
        Ok(r) => r,
        Err(e) => return error_response(&e),
    }) else {
        return error_response(&CrError::NotFound("Dashboard not found".into()));
    };

    let dec = state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Dashboard {
            org_id: ctx.org_id,
            dashboard_uid: rec.uid.clone(),
            folder_uid: rec.folder_uid.clone(),
        },
    );
    if let Err(e) = dec.into_result("Dashboard not found") {
        return error_response(&e);
    }

    let exported = match export_dashboard(state.store.as_ref(), ctx.org_id, &uid).await {
        Ok(v) => v,
        Err(e) => return error_response(&e),
    };

    (
        StatusCode::OK,
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(json!({
            "meta": {
                "type": "db",
                "canSave": true,
                "canEdit": true,
                "canAdmin": true,
                "canStar": true,
                "canDelete": true,
                "slug": rec.slug,
                "url": format!("/d/{}/{}", rec.uid, rec.slug),
                "version": rec.version,
                "folderUid": rec.folder_uid.unwrap_or_default(),
                "isFolder": false,
                "provisioned": rec.is_provisioned,
            },
            "dashboard": exported,
        })),
    )
        .into_response()
}

async fn compat_delete_dashboard_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let Some(rec) = (match state.store.get_dashboard(ctx.org_id, &uid).await {
        Ok(r) => r,
        Err(e) => return error_response(&e),
    }) else {
        return error_response(&CrError::NotFound("Dashboard not found".into()));
    };
    let dec = state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Dashboard {
            org_id: ctx.org_id,
            dashboard_uid: rec.uid.clone(),
            folder_uid: rec.folder_uid.clone(),
        },
    );
    if let Err(e) = dec.into_result("Dashboard not found") {
        return error_response(&e);
    }
    match state.store.delete_dashboard(ctx.org_id, &uid).await {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({
                "title": rec.title,
                "message": format!("Dashboard {} deleted", rec.title),
                "uid": rec.uid,
            })),
        )
            .into_response(),
        Ok(false) => error_response(&CrError::NotFound("Dashboard not found".into())),
        Err(e) => error_response(&e),
    }
}

async fn compat_get_dashboard_versions(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Dashboard {
            org_id: ctx.org_id,
            dashboard_uid: uid.clone(),
            folder_uid: None,
        },
    );
    if let Err(e) = dec.into_result("Dashboard not found") {
        return error_response(&e);
    }
    match state.store.list_dashboard_versions(ctx.org_id, &uid).await {
        Ok(vers) => (StatusCode::OK, Json(json!({ "versions": vers }))).into_response(),
        Err(e) => error_response(&e),
    }
}

#[derive(Debug, Default, Deserialize)]
struct SearchParams {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(rename = "type", default)]
    search_type: Option<String>,
}

async fn compat_search(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    AxumQuery(params): AxumQuery<SearchParams>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let q_lower = params.query.as_deref().unwrap_or("").to_ascii_lowercase();
    let mut hits = Vec::new();

    if params.search_type.as_deref() != Some("dash-db")
        && let Ok(folders) = state.store.list_folders(ctx.org_id).await
    {
        for f in folders {
            if !can_read_folder(&state, &ctx, &f.uid) {
                continue;
            }
            if q_lower.is_empty() || f.title.to_ascii_lowercase().contains(&q_lower) {
                hits.push(json!({
                    "uid": f.uid,
                    "title": f.title,
                    "type": "dash-folder",
                    "url": format!("/dashboards/f/{}", f.uid),
                }));
            }
        }
    }

    if params.search_type.as_deref() != Some("dash-folder")
        && let Ok(dashboards) = state.store.list_dashboards(ctx.org_id).await
    {
        for d in dashboards {
            let readable = state
                .authorizer
                .check(
                    &ctx,
                    Action::Read,
                    &ObjectTarget::Dashboard {
                        org_id: ctx.org_id,
                        dashboard_uid: d.uid.clone(),
                        folder_uid: d.folder_uid.clone(),
                    },
                )
                .allowed;
            if !readable {
                continue;
            }
            let q_ok = q_lower.is_empty() || d.title.to_ascii_lowercase().contains(&q_lower);
            let tag_ok = params
                .tag
                .as_ref()
                .is_none_or(|t| d.tags.iter().any(|dt| dt == t));
            if q_ok && tag_ok {
                hits.push(json!({
                    "uid": d.uid,
                    "title": d.title,
                    "type": "dash-db",
                    "url": format!("/d/{}/{}", d.uid, d.slug),
                    "tags": d.tags,
                    "folderUid": d.folder_uid,
                }));
            }
        }
    }

    (StatusCode::OK, Json(hits)).into_response()
}

async fn compat_ds_query(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Json(body): Json<ODataQueryActionBody>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    match exec_odata_datasource_query(&state, &ctx, None, body).await {
        Ok(odata_val) => {
            let sets: Vec<daiya_model::FrameSet> = odata_val
                .get("value")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let mut results = serde_json::Map::new();
            for fs in sets {
                results.insert(
                    fs.ref_id.clone(),
                    json!({
                        "status": 200,
                        "frames": fs.frames,
                    }),
                );
            }
            (
                StatusCode::OK,
                [(axum::http::header::CACHE_CONTROL, "private, no-store")],
                Json(json!({ "results": results })),
            )
                .into_response()
        }
        Err(e) => error_response(&e),
    }
}

#[derive(Debug, Default, Deserialize)]
struct RenderQueryParams {
    panel: Option<u32>,
    from: Option<String>,
    to: Option<String>,
}

async fn compat_render_dashboard_panel(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
    AxumQuery(params): AxumQuery<RenderQueryParams>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let panel_id = params.panel.unwrap_or(1);
    let range = PanelTimeRange {
        from: params.from,
        to: params.to,
        ..PanelTimeRange::default()
    };
    match panel_view(&state, &ctx.subject, ctx.org_id, &uid, panel_id, range).await {
        Ok(pv) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "private, no-store")],
            Json(pv),
        )
            .into_response(),
        Err(e) => error_response(&e),
    }
}

/// Builds the Grafana HTTP compatibility router (R2, R10a).
pub fn compat_router() -> Router<DaiyaState> {
    Router::new()
        .route("/api/health", get(compat_health))
        .route("/api/org", get(compat_get_current_org))
        .route("/api/orgs", post(compat_create_org))
        .route("/api/orgs/name/{name}", get(compat_get_org_by_name))
        .route(
            "/api/datasources",
            get(compat_list_datasources).post(compat_add_datasource),
        )
        .route(
            "/api/datasources/uid/{uid}",
            get(compat_get_datasource_by_uid)
                .put(compat_update_datasource_by_uid)
                .delete(compat_delete_datasource_by_uid),
        )
        .route(
            "/api/datasources/uid/{uid}/health",
            get(compat_datasource_health),
        )
        .route(
            "/api/datasources/uid/{uid}/resources/{*path}",
            get(compat_datasource_resource),
        )
        .route(
            "/api/folders",
            get(compat_list_folders).post(compat_create_folder),
        )
        .route(
            "/api/folders/{uid}",
            get(compat_get_folder_by_uid).delete(compat_delete_folder_by_uid),
        )
        .route("/api/dashboards/db", post(compat_post_dashboard))
        .route(
            "/api/dashboards/uid/{uid}",
            get(compat_get_dashboard_by_uid).delete(compat_delete_dashboard_by_uid),
        )
        .route(
            "/api/dashboards/uid/{uid}/versions",
            get(compat_get_dashboard_versions),
        )
        .route(
            "/api/dashboards/{uid}/render",
            get(compat_render_dashboard_panel),
        )
        .route("/api/search", get(compat_search))
        .route("/api/ds/query", post(compat_ds_query))
}
