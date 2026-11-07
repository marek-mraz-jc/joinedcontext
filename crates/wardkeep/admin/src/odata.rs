#![forbid(unsafe_code)]
//! OData v4.01 router serving all 23 entity sets of `wardkeep` (`$metadata`, `$count`, `$export`,
//! `$allowed`, entity by key, and collection queries, W4, W5).

use crate::compat::{extract_admin_ctx, AdminState};
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use jc_router::Mount;
use odata_table::{allowed, export, key_options, query, Operation};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use wardkeep_core::*;

pub fn odata_router(state: AdminState) -> Router {
    Router::new()
        .route("/api/v1/realms/{realm}/odata/$metadata", get(odata_metadata))
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
        .route("/api/v1/realms/{realm}/odata/{set}", get(odata_set_query))
        .with_state(state)
}

async fn odata_metadata(
    State(_state): State<AdminState>,
    Path(_realm): Path<String>,
) -> Response {
    let service = wardkeep_odata_service();
    let csdl = service.model();
    let mut xml = Vec::new();
    if let Err(e) = odata_csdl::write_xml(&csdl, &odata_csdl::Limits::strict(), &mut xml) {
        return WardkeepError::Storage(format!("CSDL metadata: {e}")).into_response();
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (header::HeaderName::from_static("odata-version"), "4.01"),
        ],
        xml,
    )
        .into_response()
}

fn parse_odata_options(
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

    let service = wardkeep_odata_service();
    let model = service.model();
    let edm = odata_url::EdmModel(&model);
    let uri_str = match raw_query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{set_name}?{q}"),
        None => set_name.clone(),
    };
    let parsed = odata_url::parse(&uri_str, &edm, &odata_url::Limits::strict())
        .map_err(|e| WardkeepError::BadRequest(format!("invalid OData URL: {e:?}")))?;
    let odata_url::ODataUri::Resource { options, .. } = parsed else {
        return Err(WardkeepError::BadRequest("not an OData resource URI".into()));
    };
    Ok((set_name, key_in_parens, options))
}

async fn odata_set_query(
    State(state): State<AdminState>,
    Path((realm, set_part)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let (set_name, key_opt, mut options) = parse_odata_options(&set_part, uri.query())?;

    if let Some(key) = key_opt {
        let service = wardkeep_odata_service();
        let csdl = std::sync::Arc::new(service.model());
        let info = service
            .info(csdl, &set_name)
            .ok_or_else(|| WardkeepError::NotFound("unknown entity set".into()))?;
        options = key_options(&info, &Value::String(key), options.select)?;
        let ans = state
            .oidc
            .store
            .query_entity_set(&ctx, state.authorizer.as_ref(), &set_name, &options, 1)
            .await?;
        let row = ans
            .rows
            .into_iter()
            .next()
            .ok_or_else(|| WardkeepError::NotFound("entity not found".into()))?;
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
) -> Result<Response> {
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

async fn odata_set_export(
    State(state): State<AdminState>,
    Path((realm, set_name)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<Value>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let (_, _, options) = parse_odata_options(&set_name, uri.query())?;
    let ans = state
        .oidc
        .store
        .query_entity_set(&ctx, state.authorizer.as_ref(), &set_name, &options, 1000)
        .await?;
    let _ = export::<odata_table::MemoryTable>;
    Ok(Json(ans.rows))
}

async fn odata_set_allowed(
    State(state): State<AdminState>,
    Path((realm, set_name)): Path<(String, String)>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let op_str = q.get("op").map_or("read", String::as_str);
    let op = Operation::parse(op_str)
        .ok_or_else(|| WardkeepError::BadRequest(format!("unknown operation: {op_str}")))?;
    let key_val = q.get("key").map(|k| Value::String(k.clone()));

    let service = wardkeep_odata_service();
    let csdl = std::sync::Arc::new(service.model());
    let info = service
        .info(csdl, &set_name)
        .ok_or_else(|| WardkeepError::NotFound("unknown entity set".into()))?;
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
