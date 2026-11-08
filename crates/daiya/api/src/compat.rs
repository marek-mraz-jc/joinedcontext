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
use daiya_model::{Action, CrError, CrResult, DatasourceRecord, FolderRecord, ObjectTarget, OrgId};
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
    let new_org_id = org_id_for_name(&state, &req.name);
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

/// The id of organization `name`: the same in every process of the installation, so the rows a
/// Postgres store keeps under it are found again when `provision.sh` creates it after a restart
/// (the directory of names is held in memory, as Grafana's database in its pod).
/// Holds organization `name` under Grafana number `num` from the start, as an operator's
/// configuration declares it, so provisioning files can name it by `orgId` and the Portal finds
/// it by name; its id is the one [`compat_create_org`] would give it.
///
/// # Errors
/// A number below 2 (1 is the main organization) or a number or name already held.
pub fn declare_organization(state: &DaiyaState, num: i64, name: &str) -> CrResult<OrgId> {
    let mut reg = state.org_registry.write();
    if num < 2
        || reg.by_num.contains_key(&num)
        || reg.by_name.contains_key(name)
        || name.trim().is_empty()
    {
        return Err(CrError::BadRequest(format!(
            "organization {num} '{name}': a number from 2 and a name no other organization holds"
        )));
    }
    let org_id = org_id_for_name(state, name);
    reg.by_num.insert(num, (name.to_owned(), org_id));
    reg.by_name.insert(name.to_owned(), (num, org_id));
    reg.next_num = reg.next_num.max(num.saturating_add(1));
    Ok(org_id)
}

fn org_id_for_name(state: &DaiyaState, name: &str) -> OrgId {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(format!("daiya-org\n{}\n{name}", state.installation_id));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    OrgId::from(uuid::Builder::from_custom_bytes(bytes).into_uuid())
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
    #[serde(default = "empty_object")]
    json_data: Value,
    #[serde(default)]
    secret_refs: BTreeMap<String, String>,
    #[serde(default)]
    allow_hosts: Vec<String>,
    #[serde(default)]
    wasm_digest: Option<String>,
    /// Raw credentials: refused, never stored (credentials are `secretRefs`, §7.1).
    #[serde(default)]
    secure_json_data: Option<Value>,
}

fn default_proxy() -> String {
    "proxy".to_owned()
}

fn empty_object() -> Value {
    json!({})
}

impl UpsertDatasourceReq {
    /// The record this request writes. `readOnly` is not taken from the API: as Grafana's
    /// `json:"-"` on the add/update commands, only provisioning marks a datasource read-only.
    fn into_record(self, state: &DaiyaState, ctx: &RequestCtx, uid: String) -> DatasourceRecord {
        let mut allow_hosts = self.allow_hosts;
        if allow_hosts.is_empty()
            && let Ok(u) = url::Url::parse(&self.url)
            && let Some(h) = u.host_str()
        {
            allow_hosts.push(h.to_owned());
        }
        let now = Utc::now();
        DatasourceRecord {
            installation_id: state.installation_id,
            org_id: ctx.org_id,
            instance_id: ctx.instance_id,
            project_id: None,
            uid,
            name: self.name,
            kind: self.kind,
            access: self.access,
            url: self.url,
            is_default: self.is_default,
            read_only: false,
            json_data: self.json_data,
            secret_refs: self.secret_refs,
            allow_hosts,
            wasm_digest: self.wasm_digest,
            version: 1,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Refuses raw `secureJsonData` (an empty object carries nothing and passes).
fn refuse_raw_secrets(secure_json_data: Option<&Value>) -> CrResult<()> {
    match secure_json_data {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(m)) if m.is_empty() => Ok(()),
        Some(_) => Err(CrError::BadRequest(
            "secureJsonData is refused: name each credential in secretRefs instead".into(),
        )),
    }
}

/// A caller-given uid checked as Grafana's `util.IsValidShortUID` (at most 40 of `a-zA-Z0-9-_`),
/// or a new random short uid when none is given.
fn uid_or_generated(uid: Option<String>) -> CrResult<String> {
    match uid.filter(|u| !u.is_empty()) {
        None => Ok(short_uid()),
        Some(u)
            if u.len() <= 40
                && u.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
        {
            Ok(u)
        }
        Some(_) => Err(CrError::BadRequest(
            "invalid uid: at most 40 characters of a-z, A-Z, 0-9, '-' and '_'".into(),
        )),
    }
}

/// 14 random lowercase letters and digits starting with a letter.
fn short_uid() -> String {
    use rand::Rng;
    const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    let mut uid = String::with_capacity(14);
    uid.push(char::from(ALNUM[rng.gen_range(0..26)]));
    for _ in 1..14 {
        uid.push(char::from(ALNUM[rng.gen_range(0..ALNUM.len())]));
    }
    uid
}

/// A datasource as Grafana answers it: credentials only as `secureJsonFields` booleans.
fn datasource_json(ds: &DatasourceRecord) -> Value {
    let secure_fields: BTreeMap<&str, bool> =
        ds.secret_refs.keys().map(|k| (k.as_str(), true)).collect();
    json!({
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
    })
}

fn answer(result: CrResult<Value>) -> Response {
    match result {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => error_response(&e),
    }
}

async fn compat_add_datasource(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Json(req): Json<UpsertDatasourceReq>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    answer(add_datasource(&state, &ctx, req).await)
}

async fn add_datasource(
    state: &DaiyaState,
    ctx: &RequestCtx,
    req: UpsertDatasourceReq,
) -> CrResult<Value> {
    // A datasource names a URL and the secrets sent to it: as Grafana's `datasources:write`,
    // only an organization Admin writes one, or an Editor could send a stored secret anywhere.
    state
        .authorizer
        .check(
            ctx,
            Action::Admin,
            &ObjectTarget::Organization { org_id: ctx.org_id },
        )
        .into_result("Organization not found")?;
    refuse_raw_secrets(req.secure_json_data.as_ref())?;
    let uid = uid_or_generated(req.uid.clone())?;
    if state
        .store
        .get_datasource(ctx.org_id, &uid)
        .await?
        .is_some()
    {
        return Err(CrError::Conflict(
            "data source with the same uid already exists".into(),
        ));
    }
    if state
        .store
        .get_datasource_by_name(ctx.org_id, &req.name)
        .await?
        .is_some()
    {
        return Err(CrError::Conflict(
            "data source with the same name already exists".into(),
        ));
    }
    let saved = state
        .store
        .upsert_datasource(req.into_record(state, ctx, uid))
        .await?;
    Ok(json!({
        "message": "Datasource added",
        "id": 1,
        "name": saved.name,
        "datasource": datasource_json(&saved),
    }))
}

async fn compat_update_datasource_by_uid(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
    Json(req): Json<UpsertDatasourceReq>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    answer(update_datasource(&state, &ctx, uid, req).await)
}

/// Updates an existing datasource; as Grafana, an unknown uid is 404 and a provisioned
/// (read-only) one 403.
async fn update_datasource(
    state: &DaiyaState,
    ctx: &RequestCtx,
    uid: String,
    req: UpsertDatasourceReq,
) -> CrResult<Value> {
    state
        .authorizer
        .check(
            ctx,
            Action::Propose,
            &ObjectTarget::Datasource {
                org_id: ctx.org_id,
                datasource_uid: uid.clone(),
            },
        )
        .into_result("Data source not found")?;
    state
        .authorizer
        .check(
            ctx,
            Action::Admin,
            &ObjectTarget::Organization { org_id: ctx.org_id },
        )
        .into_result("Data source not found")?;
    refuse_raw_secrets(req.secure_json_data.as_ref())?;
    let Some(existing) = state.store.get_datasource(ctx.org_id, &uid).await? else {
        return Err(CrError::NotFound("Data source not found".into()));
    };
    if existing.read_only {
        return Err(CrError::Forbidden(
            "Cannot update read-only data source".into(),
        ));
    }
    let saved = state
        .store
        .upsert_datasource(req.into_record(state, ctx, uid))
        .await?;
    Ok(json!({
        "message": "Datasource updated",
        "id": 1,
        "name": saved.name,
        "datasource": datasource_json(&saved),
    }))
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
        Ok(Some(ds)) => (StatusCode::OK, Json(datasource_json(&ds))).into_response(),
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
    answer(create_folder(&state, &ctx, req).await)
}

/// Creates a folder; as Grafana, an existing uid is 409 (`ErrFolderWithSameUIDExists`), never
/// an overwrite.
async fn create_folder(
    state: &DaiyaState,
    ctx: &RequestCtx,
    req: CreateFolderReq,
) -> CrResult<Value> {
    state
        .authorizer
        .check(
            ctx,
            Action::Propose,
            &ObjectTarget::Organization { org_id: ctx.org_id },
        )
        .into_result("Organization not found")?;
    let uid = uid_or_generated(req.uid)?;
    if state.store.get_folder(ctx.org_id, &uid).await?.is_some() {
        return Err(CrError::Conflict(
            "a folder with the same uid already exists".into(),
        ));
    }
    let now = Utc::now();
    let saved = state
        .store
        .upsert_folder(FolderRecord {
            installation_id: state.installation_id,
            org_id: ctx.org_id,
            instance_id: ctx.instance_id,
            project_id: None,
            uid,
            parent_uid: req.parent_uid,
            title: req.title,
            version: 1,
            managed_by: None,
            created_by: ctx.actor_name().to_owned(),
            updated_by: ctx.actor_name().to_owned(),
            created_at: now,
            updated_at: now,
        })
        .await?;
    Ok(json!({
        "id": 1,
        "uid": saved.uid,
        "title": saved.title,
        "url": format!("/dashboards/f/{}", saved.uid),
        "version": saved.version,
    }))
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
    answer(dashboard_versions(&state, &ctx, &uid).await)
}

/// The versions of a dashboard, after a read decision on the dashboard in its own folder.
async fn dashboard_versions(state: &DaiyaState, ctx: &RequestCtx, uid: &str) -> CrResult<Value> {
    let Some(rec) = state.store.get_dashboard(ctx.org_id, uid).await? else {
        return Err(CrError::NotFound("Dashboard not found".into()));
    };
    state
        .authorizer
        .check(
            ctx,
            Action::Read,
            &ObjectTarget::Dashboard {
                org_id: ctx.org_id,
                dashboard_uid: rec.uid,
                folder_uid: rec.folder_uid,
            },
        )
        .into_result("Dashboard not found")?;
    let versions = state.store.list_dashboard_versions(ctx.org_id, uid).await?;
    Ok(json!({ "versions": versions }))
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

/// The panel answer of `GET /api/dashboards/{uid}/render`: `panel`, `from`, `to`, `intervalMs`,
/// `maxDataPoints` and Grafana's `var-<name>` template variables, which the saved panel's queries
/// take in place of the dashboard's current values.
fn render_range(params: Vec<(String, String)>) -> (u32, PanelTimeRange) {
    let mut panel_id = 1;
    let mut range = PanelTimeRange::default();
    for (key, value) in params {
        match key.as_str() {
            "panel" => panel_id = value.parse().unwrap_or(1),
            "from" => range.from = Some(value),
            "to" => range.to = Some(value),
            "intervalMs" => range.interval_ms = value.parse().ok(),
            "maxDataPoints" => range.max_data_points = value.parse().ok(),
            other => {
                if let Some(name) = other.strip_prefix("var-") {
                    range.picked.entry(name.to_owned()).or_default().push(value);
                }
            }
        }
    }
    (panel_id, range)
}

async fn compat_render_dashboard_panel(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
    AxumQuery(params): AxumQuery<Vec<(String, String)>>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let (panel_id, range) = render_range(params);
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
