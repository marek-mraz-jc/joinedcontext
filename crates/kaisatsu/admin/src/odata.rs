#![forbid(unsafe_code)]
//! The one OData 4.01 service of kaisatsu's native API (W4, W5): the 23 entity sets (`$metadata`,
//! `$count`, `$export`, `$allowed`, entity by key, collection queries, writes on `People`) and every
//! non-CRUD registry operation as a bound or unbound action/function in the same `$metadata`,
//! generated from [`kaisatsu_core::OperationRegistry`]. Actions and functions run through
//! [`crate::operations::execute`], the executor MCP and the native REST routes use too. Errors
//! answer as OData error JSON whose `code` is the RFC 9457 slug.

use crate::compat::{AdminState, extract_admin_ctx};
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use jc_router::Mount;
use kaisatsu_core::*;
use odata_table::{Operation, allowed, export, key_options, query};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn odata_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/api/v1/realms/{realm}/odata/$metadata",
            get(odata_metadata),
        )
        .route(
            "/api/v1/realms/{realm}/odata/{set}/$count",
            get(odata_set_count),
        )
        .route(
            "/api/v1/realms/{realm}/odata/{set}/$export",
            get(odata_set_export),
        )
        .route(
            "/api/v1/realms/{realm}/odata/{set}/$allowed",
            get(odata_set_allowed),
        )
        .route(
            "/api/v1/realms/{realm}/odata/{set}",
            get(odata_set_query)
                .post(odata_set_create)
                .patch(odata_entity_patch)
                .delete(odata_entity_delete),
        )
        .route(
            "/api/v1/realms/{realm}/odata/{target}/{operation}",
            get(odata_bound_function).post(odata_bound_action),
        )
        .with_state(state)
}

/// An error in OData JSON form (OData 4.01 JSON §21): `code` carries the RFC 9457 slug, the
/// problem `type` and `status` ride in `innererror`; the message is the public detail only.
#[derive(Debug)]
pub struct ODataError(pub KaisatsuError);

impl From<KaisatsuError> for ODataError {
    fn from(e: KaisatsuError) -> Self {
        Self(e)
    }
}

impl From<odata_table::TableError> for ODataError {
    fn from(e: odata_table::TableError) -> Self {
        Self(KaisatsuError::from(e))
    }
}

impl IntoResponse for ODataError {
    fn into_response(self) -> Response {
        if let KaisatsuError::Unauthorized(ref msg) = self.0
            && msg.starts_with("use_dpop_nonce:")
        {
            return self.0.into_response();
        }
        let problem = self.0.to_problem();
        let status =
            StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = json!({
            "error": {
                "code": problem.slug,
                "message": problem.detail,
                "details": [],
                "innererror": { "type": problem.r#type, "status": problem.status },
            }
        });
        (
            status,
            [
                (
                    header::CONTENT_TYPE,
                    "application/json; odata.metadata=minimal",
                ),
                (header::HeaderName::from_static("odata-version"), "4.01"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            body.to_string(),
        )
            .into_response()
    }
}

type OResult<T> = std::result::Result<T, ODataError>;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `$metadata` as CSDL XML: the entity model from the shared odata-table definition plus one
/// `Action`/`Function` per registry operation and an import for each unbound one.
pub fn metadata_xml(registry: &OperationRegistry) -> Result<String> {
    let service = kaisatsu_odata_service();
    let ns = service.namespace.clone();
    let csdl = service.model();
    let mut xml = Vec::new();
    odata_csdl::write_xml(&csdl, &odata_csdl::Limits::strict(), &mut xml)
        .map_err(|e| KaisatsuError::Storage(format!("CSDL metadata: {e}")))?;
    let mut text = String::from_utf8(xml)
        .map_err(|_| KaisatsuError::Storage("CSDL metadata is not UTF-8".into()))?;

    let container_at = text
        .find("<EntityContainer")
        .ok_or_else(|| KaisatsuError::Storage("CSDL has no entity container".into()))?;
    let line_start = text[..container_at].rfind('\n').map_or(0, |i| i + 1);
    let indent = text[line_start..container_at].to_owned();
    let inner = format!("{indent}  ");

    let mut ops_xml = String::new();
    let mut imports = String::new();
    for op in registry.operations() {
        let o = &op.odata;
        let tag = match o.kind {
            ODataKind::Action => "Action",
            ODataKind::Function => "Function",
            ODataKind::EntitySetRead | ODataKind::EntitySetWrite => continue,
        };
        let is_bound = o.binding.is_some();
        ops_xml.push_str(&format!(
            "{indent}<{tag} Name=\"{}\" IsBound=\"{is_bound}\">\n",
            o.name
        ));
        ops_xml.push_str(&format!(
            "{inner}<Annotation Term=\"Core.Description\" String=\"{}\"/>\n",
            xml_escape(op.description)
        ));
        ops_xml.push_str(&format!(
            "{inner}<Annotation Term=\"Core.OperationId\" String=\"{}\"/>\n",
            op.name
        ));
        if let Some(b) = o.binding {
            let ty = if o.binding_collection {
                format!("Collection({ns}.{b})")
            } else {
                format!("{ns}.{b}")
            };
            ops_xml.push_str(&format!(
                "{inner}<Parameter Name=\"bindingParameter\" Type=\"{ty}\" Nullable=\"false\"/>\n"
            ));
        }
        for prm in &o.params {
            let nullable = if prm.nullable {
                ""
            } else {
                " Nullable=\"false\""
            };
            ops_xml.push_str(&format!(
                "{inner}<Parameter Name=\"{}\" Type=\"{}\"{nullable}/>\n",
                prm.name, prm.edm_type
            ));
        }
        if let Some(rt) = o.return_type {
            ops_xml.push_str(&format!("{inner}<ReturnType Type=\"{rt}\"/>\n"));
        }
        ops_xml.push_str(&format!("{indent}</{tag}>\n"));
        if !is_bound {
            let (imp, attr) = if tag == "Action" {
                ("ActionImport", "Action")
            } else {
                ("FunctionImport", "Function")
            };
            imports.push_str(&format!(
                "{inner}<{imp} Name=\"{}\" {attr}=\"{ns}.{}\"/>\n",
                o.name, o.name
            ));
        }
    }
    text.insert_str(line_start, &ops_xml);
    let close = text
        .find("</EntityContainer>")
        .ok_or_else(|| KaisatsuError::Storage("CSDL entity container is empty".into()))?;
    let close_line = text[..close].rfind('\n').map_or(0, |i| i + 1);
    text.insert_str(close_line, &imports);
    Ok(text)
}

/// Splits `Set('key')` / `Set(key)` / `Set` into the set name and key.
fn split_key(segment: &str) -> (String, Option<String>) {
    match segment.split_once('(') {
        Some((set, rest)) => {
            let raw = rest.strip_suffix(')').unwrap_or(rest);
            let key = raw
                .strip_prefix('\'')
                .and_then(|k| k.strip_suffix('\''))
                .map_or_else(|| raw.to_owned(), |k| k.replace("''", "'"));
            (set.to_owned(), Some(key))
        }
        None => (segment.to_owned(), None),
    }
}

/// Parses function parameters `a='x',b=true,c=3` (OData literal syntax) into a JSON object.
fn parse_function_params(raw: &str) -> Result<Value> {
    let mut out = serde_json::Map::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let name_start = i;
        while i < chars.len() && chars[i] != '=' {
            i += 1;
        }
        if i >= chars.len() {
            return Err(KaisatsuError::BadRequest(
                "malformed function parameters".into(),
            ));
        }
        let name: String = chars[name_start..i]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned();
        i += 1;
        let value = if chars.get(i) == Some(&'\'') {
            i += 1;
            let mut v = String::new();
            loop {
                match chars.get(i) {
                    None => {
                        return Err(KaisatsuError::BadRequest(
                            "unterminated string parameter".into(),
                        ));
                    }
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                        v.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(c) => {
                        v.push(*c);
                        i += 1;
                    }
                }
            }
            Value::String(v)
        } else {
            let start = i;
            while i < chars.len() && chars[i] != ',' {
                i += 1;
            }
            let lit: String = chars[start..i].iter().collect::<String>().trim().to_owned();
            match lit.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "null" => Value::Null,
                n => n
                    .parse::<i64>()
                    .map(Value::from)
                    .map_err(|_| KaisatsuError::BadRequest(format!("bad literal {n}")))?,
            }
        };
        if name.is_empty() {
            return Err(KaisatsuError::BadRequest("empty parameter name".into()));
        }
        out.insert(name, value);
        while i < chars.len() && (chars[i] == ',' || chars[i] == ' ') {
            i += 1;
        }
    }
    Ok(Value::Object(out))
}

fn entity_type_of(set: &str) -> Option<String> {
    kaisatsu_odata_service()
        .sets
        .iter()
        .find(|d| d.set == set)
        .map(|d| d.type_name.clone())
}

fn op_response(mount: &Mount, realm: &str, op: &OperationDef, result: Option<Value>) -> Response {
    match result {
        None => (
            StatusCode::NO_CONTENT,
            [(header::HeaderName::from_static("odata-version"), "4.01")],
        )
            .into_response(),
        Some(v) => {
            let rt = op.odata.return_type.unwrap_or("Edm.Untyped");
            let body = json!({
                "@odata.context": format!(
                    "{}/api/v1/realms/{realm}/odata/$metadata#{rt}",
                    mount.public_base()
                ),
                "value": v,
            });
            (
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        "application/json; odata.metadata=minimal",
                    ),
                    (header::HeaderName::from_static("odata-version"), "4.01"),
                ],
                body.to_string(),
            )
                .into_response()
        }
    }
}

async fn run_bound(
    state: &AdminState,
    (realm, target, operation): (&str, &str, &str),
    headers: &HeaderMap,
    mount_ext: Option<&Extension<Mount>>,
    params: Value,
    want: ODataKind,
) -> OResult<Response> {
    let mount = state.oidc.mount_from(mount_ext.map(|e| &e.0));
    let ctx = extract_admin_ctx(state, &mount, realm, headers).await?;
    let (set, key) = split_key(target);
    let ty =
        entity_type_of(&set).ok_or_else(|| KaisatsuError::NotFound("unknown entity set".into()))?;
    let (op_name, inline) = match operation.split_once('(') {
        Some((n, rest)) => (n, Some(rest.strip_suffix(')').unwrap_or(rest))),
        None => (operation, None),
    };
    let op = state
        .registry
        .find_odata(op_name, Some(&ty))
        .filter(|op| op.odata.kind == want && op.odata.binding_collection == key.is_none())
        .ok_or_else(|| KaisatsuError::NotFound(format!("no operation {op_name} on {set}")))?;
    let params = match inline {
        Some(raw) if !raw.trim().is_empty() => parse_function_params(raw)?,
        _ => params,
    };
    // A realm-bound operation acts on the realm of the URL; another key is a different realm.
    if ty == "Realm" && key.as_deref().is_some_and(|k| k != realm) {
        return Err(KaisatsuError::NotFound("realm not found".into()).into());
    }
    let key = if ty == "Realm" { None } else { key };
    let result =
        crate::operations::execute(state, &mount, &ctx, op, key.as_deref(), &params).await?;
    Ok(op_response(&mount, realm, op, result))
}

async fn odata_bound_action(
    State(state): State<AdminState>,
    Path((realm, target, operation)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    body: Option<Json<Value>>,
) -> OResult<Response> {
    let params = body.map_or(Value::Object(Default::default()), |b| b.0);
    run_bound(
        &state,
        (&realm, &target, &operation),
        &headers,
        mount_ext.as_ref(),
        params,
        ODataKind::Action,
    )
    .await
}

async fn odata_bound_function(
    State(state): State<AdminState>,
    Path((realm, target, operation)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> OResult<Response> {
    run_bound(
        &state,
        (&realm, &target, &operation),
        &headers,
        mount_ext.as_ref(),
        Value::Object(Default::default()),
        ODataKind::Function,
    )
    .await
}

async fn people_write(
    state: &AdminState,
    realm: &str,
    set_part: &str,
    headers: &HeaderMap,
    mount_ext: Option<&Extension<Mount>>,
    op_name: &str,
    body: Value,
) -> OResult<Response> {
    let (set, key) = split_key(set_part);
    if set != "People" {
        return Err(KaisatsuError::NotSupported(format!(
            "{set} is read-only through OData; use its actions"
        ))
        .into());
    }
    let mount = state.oidc.mount_from(mount_ext.map(|e| &e.0));
    let ctx = extract_admin_ctx(state, &mount, realm, headers).await?;
    let op = state
        .registry
        .find(op_name)
        .ok_or_else(|| KaisatsuError::NotSupported(op_name.to_owned()))?;
    let result = crate::operations::execute(state, &mount, &ctx, op, key.as_deref(), &body).await?;
    let status = if op_name == "person_create" {
        StatusCode::CREATED
    } else if result.is_none() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    } else {
        StatusCode::OK
    };
    let mut row = result.unwrap_or(Value::Null);
    if let Some(obj) = row.as_object_mut() {
        obj.insert(
            "@odata.context".into(),
            Value::String(format!(
                "{}/api/v1/realms/{realm}/odata/$metadata#People/$entity",
                mount.public_base()
            )),
        );
    }
    Ok((
        status,
        [(header::HeaderName::from_static("odata-version"), "4.01")],
        Json(row),
    )
        .into_response())
}

async fn odata_set_create(
    State(state): State<AdminState>,
    Path((realm, set_part)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(body): Json<Value>,
) -> OResult<Response> {
    if split_key(&set_part).1.is_some() {
        return Err(KaisatsuError::BadRequest("POST goes to the collection".into()).into());
    }
    people_write(
        &state,
        &realm,
        &set_part,
        &headers,
        mount_ext.as_ref(),
        "person_create",
        body,
    )
    .await
}

async fn odata_entity_patch(
    State(state): State<AdminState>,
    Path((realm, set_part)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(body): Json<Value>,
) -> OResult<Response> {
    people_write(
        &state,
        &realm,
        &set_part,
        &headers,
        mount_ext.as_ref(),
        "person_update",
        body,
    )
    .await
}

async fn odata_entity_delete(
    State(state): State<AdminState>,
    Path((realm, set_part)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> OResult<Response> {
    people_write(
        &state,
        &realm,
        &set_part,
        &headers,
        mount_ext.as_ref(),
        "person_delete",
        Value::Null,
    )
    .await
}

async fn odata_metadata(
    State(state): State<AdminState>,
    Path(_realm): Path<String>,
) -> OResult<Response> {
    let xml = metadata_xml(&state.registry)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (header::HeaderName::from_static("odata-version"), "4.01"),
        ],
        xml,
    )
        .into_response())
}

pub(crate) fn parse_odata_options(
    set_part: &str,
    raw_query: Option<&str>,
) -> Result<(String, Option<String>, odata_table::QueryOptions)> {
    let (set_name, key_in_parens) = if let Some((s, rest)) = set_part.split_once('(') {
        let k = rest
            .strip_suffix(')')
            .unwrap_or(rest)
            .trim_matches('\'')
            .to_owned();
        (s.to_owned(), Some(k))
    } else {
        (set_part.to_owned(), None)
    };

    let service = kaisatsu_odata_service();
    let model = service.model();
    let edm = odata_url::EdmModel(&model);
    let uri_str = match raw_query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{set_name}?{q}"),
        None => set_name.clone(),
    };
    let parsed = odata_url::parse(&uri_str, &edm, &odata_url::Limits::strict())
        .map_err(|e| KaisatsuError::BadRequest(format!("invalid OData URL: {e:?}")))?;
    let odata_url::ODataUri::Resource { options, .. } = parsed else {
        return Err(KaisatsuError::BadRequest(
            "not an OData resource URI".into(),
        ));
    };
    Ok((set_name, key_in_parens, options))
}

async fn odata_set_query(
    State(state): State<AdminState>,
    Path((realm, set_part)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> OResult<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    // Unbound function import (`Allowed(operation='...',object='...')`).
    let (head, _) = split_key(&set_part);
    if let Some(op) = state.registry.find_odata(&head, None) {
        if op.odata.kind != ODataKind::Function {
            return Err(KaisatsuError::BadRequest(format!("{head} is an action; POST it")).into());
        }
        let raw = set_part[head.len()..]
            .strip_prefix('(')
            .and_then(|r| r.strip_suffix(')'))
            .unwrap_or_default();
        let params = parse_function_params(raw)?;
        let result = crate::operations::execute(&state, &mount, &ctx, op, None, &params).await?;
        return Ok(op_response(&mount, &realm, op, result));
    }
    let (set_name, key_opt, mut options) = parse_odata_options(&set_part, uri.query())?;

    if let Some(key) = key_opt {
        let service = kaisatsu_odata_service();
        let csdl = std::sync::Arc::new(service.model());
        let info = service
            .info(csdl, &set_name)
            .ok_or_else(|| KaisatsuError::NotFound("unknown entity set".into()))?;
        options =
            key_options(&info, &Value::String(key), options.select).map_err(KaisatsuError::from)?;
        let ans = state
            .oidc
            .store
            .query_entity_set(&ctx, state.authorizer.as_ref(), &set_name, &options, 1)
            .await?;
        let row = ans
            .rows
            .into_iter()
            .next()
            .ok_or_else(|| KaisatsuError::NotFound("entity not found".into()))?;
        return Ok(Json(row).into_response());
    }

    let ans = state
        .oidc
        .store
        .query_entity_set(&ctx, state.authorizer.as_ref(), &set_name, &options, 100)
        .await?;

    let mut body = serde_json::Map::new();
    if let Some(c) = ans.count {
        body.insert("@odata.count".into(), json!(c));
    }
    body.insert("value".into(), Value::Array(ans.rows));
    if let Some(next) = ans.next {
        body.insert(
            "@odata.nextLink".into(),
            Value::String(format!(
                "{}/api/v1/realms/{realm}/odata/{set_name}?$skiptoken={next}",
                mount.public_base()
            )),
        );
    }
    Ok(Json(Value::Object(body)).into_response())
}

async fn odata_set_count(
    State(state): State<AdminState>,
    Path((realm, set_name)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> OResult<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let (_, _, mut options) = parse_odata_options(&set_name, uri.query())?;
    options.count = Some(true);
    options.top = Some(0);
    let ans = state
        .oidc
        .store
        .query_entity_set(&ctx, state.authorizer.as_ref(), &set_name, &options, 1)
        .await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        ans.count.unwrap_or(0).to_string(),
    )
        .into_response())
}

/// K15: Streams pages of 250 rows (up to `MAX_FALLBACK_SCAN_ROWS` = 5000) through
/// `query_entity_set` (`PgScopedTable` on PostgreSQL) instead of capping at a single 1000-row fetch.
async fn odata_set_export(
    State(state): State<AdminState>,
    Path((realm, set_name)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> OResult<Json<Vec<Value>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let (_, _, mut options) = parse_odata_options(&set_name, uri.query())?;
    let mut all_rows = Vec::new();
    loop {
        let ans = state
            .oidc
            .store
            .query_entity_set(&ctx, state.authorizer.as_ref(), &set_name, &options, 250)
            .await?;
        all_rows.extend(ans.rows);
        if all_rows.len() > 5_000 {
            return Err(KaisatsuError::BadRequest(
                "the export is over 5000 rows: narrow it with $filter".into(),
            )
            .into());
        }
        match ans.next {
            Some(token) => options.skiptoken = Some(token.to_string()),
            None => break,
        }
    }
    let _ = export::<odata_table::MemoryTable>;
    Ok(Json(all_rows))
}

async fn odata_set_allowed(
    State(state): State<AdminState>,
    Path((realm, set_name)): Path<(String, String)>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> OResult<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let op_str = q.get("op").map_or("read", String::as_str);
    let op = Operation::parse(op_str)
        .ok_or_else(|| KaisatsuError::BadRequest(format!("unknown operation: {op_str}")))?;
    let key_val = q.get("key").map(|k| Value::String(k.clone()));

    let service = kaisatsu_odata_service();
    let csdl = std::sync::Arc::new(service.model());
    let info = service
        .info(csdl, &set_name)
        .ok_or_else(|| KaisatsuError::NotFound("unknown entity set".into()))?;
    let scope = scope_for_context(&ctx, state.authorizer.as_ref(), &set_name);

    // Fetch visible rows for key existence check so hidden/cross-org records answer identical to missing (W5)
    let ans = state
        .oidc
        .store
        .query_entity_set(
            &ctx,
            state.authorizer.as_ref(),
            &set_name,
            &odata_table::QueryOptions::default(),
            1000,
        )
        .await
        .unwrap_or_default();
    let table = odata_table::MemoryTable::new(info, ans.rows)?;
    let dec = allowed(&table, &scope, op, key_val.as_ref()).await?;
    let _ = query::<odata_table::MemoryTable>;
    Ok(Json(dec.to_json()))
}
