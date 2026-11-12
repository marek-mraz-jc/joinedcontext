#![forbid(unsafe_code)]
//! CKAN Action API v3 compatible subset (`/api/3/action/{action}`, C7, `research-ckan.md` §2).
//!
//! Answers with the exact envelope `{"help": "...", "success": true, "result": ...}` and
//! error structure `{"help": "...", "success": false, "error": {"__type": "...", "message": "...", ...}}`
//! expected by `ours/platform/jcctl-publish/ckan_http.rs`, `ours/portal/catalogue.rs`, and
//! `ours/deployment/harvest-*.py`.

use crate::{dataset_shown, decision_error, DataroomState};
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use dataroom_core::branding::{origin_of, Address};
use dataroom_core::ckan_compat::{
    ckan_license_list, dataset_to_ckan, distribution_from_ckan, distribution_to_ckan,
    resource_view_to_ckan,
};
use dataroom_core::{
    text_in_lang, Action, Catalogue, DataroomError, DatasetFilter, ObjectTarget, Publisher,
    RecordState, RequestCtx, ResourceView, Visibility,
};
use dataroom_plugin::validate_view_payload;
use dataroom_search::{apply_fq_to_filter, search_datasets};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

fn help_url(base: &str, action: &str) -> String {
    format!("{base}/api/3/action/help_show?name={action}")
}

fn ckan_ok(base: &str, action: &str, result: Value, cache_public: bool) -> Response {
    let body = json!({
        "help": help_url(base, action),
        "success": true,
        "result": result,
    });
    if cache_public {
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=60")],
            Json(body),
        )
            .into_response()
    } else {
        (StatusCode::OK, Json(body)).into_response()
    }
}

fn ckan_err(base: &str, action: &str, err: DataroomError) -> Response {
    let status = StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::BAD_REQUEST);
    let mut err_obj = Map::new();
    err_obj.insert("__type".into(), json!(err.ckan_error_type()));
    match err {
        DataroomError::Validation { message, fields } => {
            err_obj.insert("message".into(), json!(message));
            for (k, v) in fields {
                err_obj.insert(k, json!(v));
            }
        }
        other => {
            err_obj.insert("message".into(), json!(other.to_string()));
        }
    }
    let body = json!({
        "help": help_url(base, action),
        "success": false,
        "error": Value::Object(err_obj),
    });
    (status, Json(body)).into_response()
}

fn parse_query_to_value(raw_query: Option<&str>) -> Value {
    let mut map = Map::new();
    if let Some(q) = raw_query {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            map.insert(k.into_owned(), Value::String(v.into_owned()));
        }
    }
    Value::Object(map)
}

/// Handles `GET /api/3/action/{action}`.
pub async fn handle_ckan_get(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let payload = parse_query_to_value(raw_query.as_deref());
    dispatch_ckan_action(state, headers, action, payload, true).await
}

/// Handles `POST /api/3/action/{action}`.
pub async fn handle_ckan_post(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Response {
    let payload = if body.is_empty() {
        parse_query_to_value(raw_query.as_deref())
    } else {
        serde_json::from_slice::<Value>(&body)
            .unwrap_or_else(|_| parse_query_to_value(raw_query.as_deref()))
    };
    dispatch_ckan_action(state, headers, action, payload, false).await
}

async fn dispatch_ckan_action(
    state: DataroomState,
    headers: HeaderMap,
    action: String,
    payload: Value,
    is_get: bool,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => {
            // The configured base first: a request's own address is not echoed back.
            let base = state
                .public_base
                .clone()
                .or_else(|| crate::request_origin().map(|o| o.base()))
                .unwrap_or_else(|| "https://dataroom.local".into());
            return ckan_err(&base, &action, e);
        }
    };
    let viz = is_get
        .then(|| {
            cat.effective_branding(Some(&state.branding_of(&cat)))
                .address(Address::Viz)
        })
        .flatten();
    let mut response = answer_ckan_action(state, ctx, cat, action, payload, is_get).await;
    if is_get {
        allow_viz_origin(&mut response, headers.get(header::ORIGIN), viz.as_deref());
    }
    response
}

/// Lets the chart renderer's host page (`https://viz.{domain}`) read the answer: it fetches a
/// published `jc_visualization` view anonymously with `credentials: "omit"`, as it did from CKAN
/// under `ckan.cors.origin_whitelist` (T-4641). That one origin alone, never with credentials;
/// `Vary: Origin` keeps a shared cache from handing one origin's answer to another (T-6824).
fn allow_viz_origin(response: &mut Response, origin: Option<&HeaderValue>, viz: Option<&str>) {
    let headers = response.headers_mut();
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if let (Some(origin), Some(viz)) = (origin, viz) {
        if origin.as_bytes() == origin_of(viz).as_bytes() {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
        }
    }
}

async fn answer_ckan_action(
    state: DataroomState,
    ctx: RequestCtx,
    cat: Catalogue,
    action: String,
    payload: Value,
    is_get: bool,
) -> Response {
    let base = cat.base_url();
    let is_anon_get = is_get && ctx.subject.is_anonymous();

    // C3: DataStore actions are explicitly refused with a clear Not Supported answer.
    if action.starts_with("datastore_") {
        return ckan_err(
            &base,
            &action,
            DataroomError::NotSupported(format!(
                "{action} is not supported: dataroom holds no data copies and queries Endpoints live (C3)"
            )),
        );
    }

    match action.as_str() {
        "status_show" => {
            let result = json!({
                "ckan_version": "2.11.7",
                "dataroom_version": "0.1.0",
                "site_title": text_in_lang(&cat.title, Some(&cat.default_language)).unwrap_or(&cat.name),
                "site_description": text_in_lang(&cat.description, Some(&cat.default_language)).unwrap_or(""),
                "site_url": base,
                "locale_default": cat.default_language,
                "extensions": ["jc_theme", "dcat", "scheming_datasets", "fluent"],
            });
            ckan_ok(&base, &action, result, is_anon_get)
        }
        "help_show" => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("status_show");
            ckan_ok(
                &base,
                &action,
                json!(format!("CKAN Action API: {name}")),
                is_anon_get,
            )
        }
        "site_read" => ckan_ok(&base, &action, json!(true), is_anon_get),
        "license_list" => ckan_ok(
            &base,
            &action,
            Value::Array(ckan_license_list()),
            is_anon_get,
        ),
        "tag_list" => {
            let datasets = match state.store.list_datasets(&ctx).await {
                Ok(d) => d,
                Err(e) => return ckan_err(&base, &action, e),
            };
            let mut tags = std::collections::BTreeSet::new();
            for ds in datasets {
                if ds.state == RecordState::Active && ds.visibility == Visibility::Public {
                    for t in ds.tags {
                        tags.insert(t);
                    }
                }
            }
            let list: Vec<Value> = tags.into_iter().map(Value::String).collect();
            ckan_ok(&base, &action, Value::Array(list), is_anon_get)
        }
        "group_list" => ckan_ok(&base, &action, json!([]), is_anon_get),
        "package_list" => {
            let datasets = match state.store.list_datasets(&ctx).await {
                Ok(d) => d,
                Err(e) => return ckan_err(&base, &action, e),
            };
            let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
            let pub_map: BTreeMap<String, Publisher> = publishers
                .into_iter()
                .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
                .collect();
            let mut names: Vec<String> = datasets
                .into_iter()
                .filter(|d| {
                    d.state == RecordState::Active
                        && d.visibility == Visibility::Public
                        && !pub_map
                            .get(&d.publisher_id)
                            .is_some_and(|p| p.state == RecordState::Withdrawn)
                })
                .map(|d| d.name)
                .collect();
            names.sort();
            if let Some(offset) = payload
                .get("offset")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            {
                names = names.into_iter().skip(offset as usize).collect();
            }
            if let Some(limit) = payload
                .get("limit")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            {
                names.truncate(limit as usize);
            }
            ckan_ok(&base, &action, json!(names), is_anon_get)
        }
        "package_show" => {
            let Some(id_or_name) = payload
                .get("id")
                .or_else(|| payload.get("name_or_id"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(ds) = (match state.store.get_dataset(&ctx, id_or_name).await {
                Ok(d) => d,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            };
            // Anonymous callers cannot see withdrawn datasets or datasets of withdrawn publishers,
            // nor hidden datasets (`jc_hide` / `after_dataset_show` in `plugin.py` line 2849, D2, G2).
            if !dataset_shown(&state, &ctx, &cat, &ds, pub_rec.as_ref()) {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            }
            let mut views = BTreeMap::new();
            for dist in &ds.distributions {
                if let Ok(v) = state.store.list_resource_views(&ctx, &dist.id).await {
                    if !v.is_empty() {
                        views.insert(dist.id.clone(), v);
                    }
                }
            }
            let ckan_dict = dataset_to_ckan(&ds, pub_rec.as_ref(), &views, &cat.default_language);
            let cacheable = is_anon_get && ds.visibility == Visibility::Public;
            ckan_ok(&base, &action, ckan_dict, cacheable)
        }
        "package_search" => {
            let q = payload
                .get("q")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let rows = payload
                .get("rows")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(10) as usize;
            let start = payload
                .get("start")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0) as usize;
            let sort = payload
                .get("sort")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let include_private = payload
                .get("include_private")
                .and_then(|v| v.as_bool().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(false);
            let include_drafts = payload
                .get("include_drafts")
                .and_then(|v| v.as_bool().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(false);

            let mut filter = DatasetFilter {
                q,
                sort,
                offset: start,
                limit: rows,
                include_private,
                include_drafts,
                ..DatasetFilter::default()
            };
            if let Some(fq) = payload.get("fq").and_then(Value::as_str) {
                if let Err(e) = apply_fq_to_filter(fq, &mut filter) {
                    return ckan_err(&base, &action, e);
                }
            }

            let datasets = match state.store.list_datasets(&ctx).await {
                Ok(d) => d,
                Err(e) => return ckan_err(&base, &action, e),
            };
            let publishers = match state.store.list_publishers(&ctx).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            };
            let pub_map: BTreeMap<String, Publisher> = publishers
                .iter()
                .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p.clone())])
                .collect();

            // Enforce `jc_hide` on `package_search`: hidden packages are hidden from anonymous callers
            // and visible to editors of the dataset's publisher (`plugin.py` lines 168-197, D2, G2).
            let visible_datasets: Vec<dataroom_core::Dataset> = datasets
                .into_iter()
                .filter(|ds| {
                    if !cat.hidden_datasets.contains_key(&ds.name) {
                        return true;
                    }
                    if ctx.subject.is_anonymous() {
                        return false;
                    }
                    let pub_rec = pub_map.get(&ds.publisher_id);
                    let target = ObjectTarget::from_dataset(ds, pub_rec);
                    state
                        .authorizer
                        .check(&ctx, Action::Propose, &target)
                        .allowed
                })
                .collect();

            let outcome = match search_datasets(
                &ctx,
                state.authorizer.as_ref(),
                &visible_datasets,
                &publishers,
                &filter,
                &cat.default_language,
            ) {
                Ok(o) => o,
                Err(e) => return ckan_err(&base, &action, e),
            };

            let fl_only_name = payload
                .get("fl")
                .and_then(Value::as_str)
                .is_some_and(|fl| fl.trim() == "name");

            let results: Vec<Value> = outcome
                .datasets
                .iter()
                .map(|ds| {
                    if fl_only_name {
                        json!({ "name": ds.name })
                    } else {
                        let pub_rec = pub_map.get(&ds.publisher_id);
                        dataset_to_ckan(ds, pub_rec, &BTreeMap::new(), &cat.default_language)
                    }
                })
                .collect();

            let requested_facets: Vec<String> = match payload.get("facet.field") {
                Some(Value::Array(arr)) => arr
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                Some(Value::String(s)) => {
                    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
                }
                _ => Vec::new(),
            };
            let (facets, search_facets) = outcome.facets.to_ckan_facets(&requested_facets);

            let res = json!({
                "count": outcome.total,
                "sort": filter.sort.unwrap_or_else(|| "score desc, metadata_modified desc".into()),
                "facets": facets,
                "search_facets": search_facets,
                "results": results,
            });
            ckan_ok(&base, &action, res, is_anon_get && !include_private)
        }
        "package_create" => {
            let op_ctx = state.op_context();
            let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
            if !name.is_empty() {
                if let Ok(Some(_)) = state.store.get_dataset(&ctx, name).await {
                    return ckan_err(
                        &base,
                        &action,
                        DataroomError::field_error("name", "That URL is already in use."),
                    );
                }
            }
            match dataroom_core::ops::execute_operation(
                &op_ctx,
                &ctx,
                "dataset.propose",
                json!({ "dataset": payload }),
            )
            .await
            {
                Ok(created) => ckan_ok(&base, &action, created, false),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "package_update" => {
            let Some(ident) = payload
                .get("id")
                .or_else(|| payload.get("name"))
                .and_then(Value::as_str)
            else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            if state
                .store
                .get_dataset(&ctx, ident)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            }
            let op_ctx = state.op_context();
            match dataroom_core::ops::execute_operation(
                &op_ctx,
                &ctx,
                "dataset.propose",
                json!({ "dataset": payload }),
            )
            .await
            {
                Ok(updated) => ckan_ok(&base, &action, updated, false),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "package_patch" => {
            let Some(ident) = payload
                .get("id")
                .or_else(|| payload.get("name"))
                .and_then(Value::as_str)
            else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(existing) = (match state.store.get_dataset(&ctx, ident).await {
                Ok(d) => d,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = state
                .store
                .get_publisher(&ctx, &existing.publisher_id)
                .await
                .ok()
                .flatten();
            let mut merged = dataset_to_ckan(
                &existing,
                pub_rec.as_ref(),
                &BTreeMap::new(),
                &cat.default_language,
            );
            if let (Some(target_obj), Some(patch_obj)) =
                (merged.as_object_mut(), payload.as_object())
            {
                for (k, v) in patch_obj {
                    target_obj.insert(k.clone(), v.clone());
                }
            }
            let op_ctx = state.op_context();
            match dataroom_core::ops::execute_operation(
                &op_ctx,
                &ctx,
                "dataset.propose",
                json!({ "dataset": merged }),
            )
            .await
            {
                Ok(updated) => ckan_ok(&base, &action, updated, false),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "package_delete" => {
            let Some(ident) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let op_ctx = state.op_context();
            match dataroom_core::ops::execute_operation(
                &op_ctx,
                &ctx,
                "dataset.withdraw",
                json!({ "name": ident }),
            )
            .await
            {
                Ok(res) => ckan_ok(&base, &action, res, false),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "dataset_purge" => {
            let Some(ident) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(ds) = (match state.store.get_dataset(&ctx, ident).await {
                Ok(d) => d,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = state
                .store
                .get_publisher(&ctx, &ds.publisher_id)
                .await
                .ok()
                .flatten();
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            let dec = state.authorizer.check(&ctx, Action::Delete, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            if let Err(e) = state.store.purge_dataset(&ctx, &ds.id).await {
                return ckan_err(&base, &action, e);
            }
            ckan_ok(&base, &action, json!({}), false)
        }
        "resource_show" => {
            let Some(id) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some((ds, dist)) = (match state.store.get_distribution(&ctx, id).await {
                Ok(r) => r,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            };
            // A resource is shown where its dataset is (package_show).
            if !dataset_shown(&state, &ctx, &cat, &ds, pub_rec.as_ref()) {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            }
            let pos = ds
                .distributions
                .iter()
                .position(|r| r.id == dist.id)
                .unwrap_or(0);
            let views = state
                .store
                .list_resource_views(&ctx, &dist.id)
                .await
                .unwrap_or_default();
            let res_dict = distribution_to_ckan(&dist, &ds.id, pos, &views, &cat.default_language);
            ckan_ok(&base, &action, res_dict, is_anon_get)
        }
        "resource_create" => {
            let Some(package_id) = payload.get("package_id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("package_id", "Missing value"),
                );
            };
            let Some(mut ds) = (match state.store.get_dataset(&ctx, package_id).await {
                Ok(d) => d,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = state
                .store
                .get_publisher(&ctx, &ds.publisher_id)
                .await
                .ok()
                .flatten();
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            let dec = state.authorizer.check(&ctx, Action::Propose, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            let pos = ds.distributions.len();
            let fallback_id = format!("res-{}-{pos}", ds.name);
            let dist =
                distribution_from_ckan(&payload, &ds.id, &fallback_id, &cat.default_language);
            ds.distributions.push(dist.clone());
            ds.modified = Utc::now();
            if let Err(e) = state.store.upsert_dataset(&ctx, ds.clone()).await {
                return ckan_err(&base, &action, e);
            }
            let res_dict = distribution_to_ckan(&dist, &ds.id, pos, &[], &cat.default_language);
            ckan_ok(&base, &action, res_dict, false)
        }
        "resource_update" | "resource_patch" => {
            let Some(id) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some((mut ds, existing_dist)) = (match state.store.get_distribution(&ctx, id).await
            {
                Ok(r) => r,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = state
                .store
                .get_publisher(&ctx, &ds.publisher_id)
                .await
                .ok()
                .flatten();
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            let dec = state.authorizer.check(&ctx, Action::Propose, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            let pos = ds
                .distributions
                .iter()
                .position(|r| r.id == existing_dist.id)
                .unwrap_or(0);
            let mut merged_json =
                distribution_to_ckan(&existing_dist, &ds.id, pos, &[], &cat.default_language);
            if let (Some(target_obj), Some(patch_obj)) =
                (merged_json.as_object_mut(), payload.as_object())
            {
                for (k, v) in patch_obj {
                    target_obj.insert(k.clone(), v.clone());
                }
                // A plain `name` / `description` names the default language's text; the stored
                // per-language map would otherwise win over it and the change would be lost.
                for (plain, translated) in [
                    ("name", "name_translated"),
                    ("description", "description_translated"),
                ] {
                    if patch_obj.contains_key(translated) {
                        continue;
                    }
                    if let Some(text) = patch_obj.get(plain).and_then(Value::as_str) {
                        let mut texts = target_obj
                            .get(translated)
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        texts.insert(cat.default_language.clone(), json!(text));
                        target_obj.insert(translated.to_owned(), Value::Object(texts));
                    }
                }
            }
            let updated_dist = distribution_from_ckan(
                &merged_json,
                &ds.id,
                &existing_dist.id,
                &cat.default_language,
            );
            ds.distributions[pos] = updated_dist.clone();
            ds.modified = Utc::now();
            if let Err(e) = state.store.upsert_dataset(&ctx, ds.clone()).await {
                return ckan_err(&base, &action, e);
            }
            let res_dict =
                distribution_to_ckan(&updated_dist, &ds.id, pos, &[], &cat.default_language);
            ckan_ok(&base, &action, res_dict, false)
        }
        "resource_delete" => {
            let Some(id) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some((mut ds, _)) = (match state.store.get_distribution(&ctx, id).await {
                Ok(r) => r,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = state
                .store
                .get_publisher(&ctx, &ds.publisher_id)
                .await
                .ok()
                .flatten();
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            let dec = state.authorizer.check(&ctx, Action::Delete, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            ds.distributions.retain(|r| r.id != id);
            ds.modified = Utc::now();
            if let Err(e) = state.store.upsert_dataset(&ctx, ds).await {
                return ckan_err(&base, &action, e);
            }
            ckan_ok(&base, &action, json!({}), false)
        }
        "resource_view_list" => {
            let Some(id) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some((ds, _)) = (match state.store.get_distribution(&ctx, id).await {
                Ok(r) => r,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            };
            // A resource is shown where its dataset is (package_show).
            if !dataset_shown(&state, &ctx, &cat, &ds, pub_rec.as_ref()) {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            }
            let views = match state.store.list_resource_views(&ctx, id).await {
                Ok(v) => v,
                Err(e) => return ckan_err(&base, &action, e),
            };
            let list: Vec<Value> = views.iter().map(resource_view_to_ckan).collect();
            ckan_ok(&base, &action, Value::Array(list), is_anon_get)
        }
        "resource_view_show" => {
            let Some(id) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(view) = (match state.store.get_resource_view(&ctx, id).await {
                Ok(v) => v,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            // A view is shown where its resource is: a view of a resource that is gone, or of a
            // dataset the caller may not see, is not found.
            let Some((ds, _)) = (match state.store.get_distribution(&ctx, &view.resource_id).await {
                Ok(r) => r,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            };
            if !dataset_shown(&state, &ctx, &cat, &ds, pub_rec.as_ref()) {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            }
            ckan_ok(&base, &action, resource_view_to_ckan(&view), is_anon_get)
        }
        "resource_view_create" => {
            if let Err(e) = validate_view_payload(&payload) {
                return ckan_err(&base, &action, e);
            }
            let Some(resource_id) = payload.get("resource_id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("resource_id", "Missing value"),
                );
            };
            let Some(view_type) = payload.get("view_type").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("view_type", "Missing value"),
                );
            };
            let Some((ds, _)) = (match state.store.get_distribution(&ctx, resource_id).await {
                Ok(r) => r,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = state
                .store
                .get_publisher(&ctx, &ds.publisher_id)
                .await
                .ok()
                .flatten();
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            let dec = state.authorizer.check(&ctx, Action::Propose, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            let view = parse_resource_view(&payload, "", resource_id, view_type);
            match state.store.upsert_resource_view(&ctx, view).await {
                Ok(saved) => ckan_ok(&base, &action, resource_view_to_ckan(&saved), false),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "resource_view_update" => {
            if let Err(e) = validate_view_payload(&payload) {
                return ckan_err(&base, &action, e);
            }
            let Some(id) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(existing) = (match state.store.get_resource_view(&ctx, id).await {
                Ok(v) => v,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let Some((ds, _)) = (match state
                .store
                .get_distribution(&ctx, &existing.resource_id)
                .await
            {
                Ok(r) => r,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let pub_rec = state
                .store
                .get_publisher(&ctx, &ds.publisher_id)
                .await
                .ok()
                .flatten();
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            let dec = state.authorizer.check(&ctx, Action::Propose, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            let view_type = payload
                .get("view_type")
                .and_then(Value::as_str)
                .unwrap_or(&existing.view_type);
            let view =
                parse_resource_view(&payload, &existing.id, &existing.resource_id, view_type);
            match state.store.upsert_resource_view(&ctx, view).await {
                Ok(saved) => ckan_ok(&base, &action, resource_view_to_ckan(&saved), false),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "resource_view_delete" => {
            let Some(id) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(existing) = (match state.store.get_resource_view(&ctx, id).await {
                Ok(v) => v,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            // Decided on the view's dataset; a view whose resource is gone is decided on the
            // catalogue. A failed lookup refuses rather than skipping the check.
            let target = match state
                .store
                .get_distribution(&ctx, &existing.resource_id)
                .await
            {
                Ok(Some((ds, _))) => {
                    let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
                        Ok(p) => p,
                        Err(e) => return ckan_err(&base, &action, e),
                    };
                    ObjectTarget::from_dataset(&ds, pub_rec.as_ref())
                }
                Ok(None) => ObjectTarget::Catalogue {
                    org_id: ctx.org_id,
                    catalogue_id: ctx.catalogue_id,
                },
                Err(e) => return ckan_err(&base, &action, e),
            };
            let dec = state.authorizer.check(&ctx, Action::Delete, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            if let Err(e) = state.store.delete_resource_view(&ctx, id).await {
                return ckan_err(&base, &action, e);
            }
            ckan_ok(&base, &action, json!({}), false)
        }
        "organization_list" => {
            let publishers = match state.store.list_publishers(&ctx).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            };
            let all_fields = payload
                .get("all_fields")
                .and_then(|v| v.as_bool().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(false);
            let active_pubs: Vec<&Publisher> = publishers
                .iter()
                .filter(|p| p.state == RecordState::Active)
                .collect();
            let result: Vec<Value> = if all_fields {
                active_pubs
                    .into_iter()
                    .map(|p| publisher_to_ckan(p, &cat.default_language, 0))
                    .collect()
            } else {
                active_pubs
                    .into_iter()
                    .map(|p| Value::String(p.name.clone()))
                    .collect()
            };
            ckan_ok(&base, &action, Value::Array(result), is_anon_get)
        }
        "organization_show" => {
            let Some(id_or_name) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(pub_rec) = (match state.store.get_publisher(&ctx, id_or_name).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            if pub_rec.state == RecordState::Withdrawn && ctx.subject.is_anonymous() {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            }
            let datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
            let pkg_count = datasets
                .iter()
                .filter(|d| {
                    d.state == RecordState::Active
                        && d.visibility == Visibility::Public
                        && (d.publisher_id == pub_rec.name || d.publisher_id == pub_rec.id)
                })
                .count();
            ckan_ok(
                &base,
                &action,
                publisher_to_ckan(&pub_rec, &cat.default_language, pkg_count),
                is_anon_get,
            )
        }
        "organization_create" => {
            let Some(name) = payload.get("name").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("name", "Missing value"),
                );
            };
            if let Err(e) = dataroom_core::validate_slug("name", name) {
                return ckan_err(&base, &action, e);
            }
            // CKAN refuses a second organization on a taken name; updates go through
            // organization_update/patch, which decide on the stored record.
            match state.store.get_publisher(&ctx, name).await {
                Ok(None) => {}
                Ok(Some(_)) => {
                    return ckan_err(
                        &base,
                        &action,
                        DataroomError::field_error("name", "Group name already exists in database"),
                    )
                }
                Err(e) => return ckan_err(&base, &action, e),
            }
            let title_str = payload.get("title").and_then(Value::as_str).unwrap_or(name);
            let desc_str = payload
                .get("description")
                .or_else(|| payload.get("notes"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let uri = payload
                .get("uri")
                .or_else(|| payload.get("url"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{base}/organization/{name}"));
            let now = Utc::now();
            let pub_rec = Publisher {
                org_id: ctx.org_id,
                catalogue_id: ctx.catalogue_id,
                id: payload
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("org-{name}")),
                name: name.to_owned(),
                title: BTreeMap::from([(cat.default_language.clone(), title_str.to_owned())]),
                description: if desc_str.is_empty() {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([(cat.default_language.clone(), desc_str.to_owned())])
                },
                uri,
                publisher_type: payload
                    .get("dcat_type")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                identifier: payload
                    .get("identifier")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                email: payload
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                homepage: payload
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                logo_url: payload
                    .get("image_url")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: payload
                    .get("editor_grants")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                badge_initials: None,
                badge_colour: None,
                declared_by_instance: ctx.subject.client_id.clone(),
                created: now,
                modified: now,
            };
            // Decided without the grants the caller sends: naming oneself an editor grants nothing.
            if let Err(e) = dataroom_core::ops::authorize_publisher_write(
                state.authorizer.as_ref(),
                &ctx,
                None,
                &pub_rec,
            ) {
                return ckan_err(&base, &action, e);
            }
            match state.store.upsert_publisher(&ctx, pub_rec).await {
                Ok(saved) => ckan_ok(
                    &base,
                    &action,
                    publisher_to_ckan(&saved, &cat.default_language, 0),
                    false,
                ),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "organization_update" | "organization_patch" => {
            let Some(ident) = payload
                .get("id")
                .or_else(|| payload.get("name"))
                .and_then(Value::as_str)
            else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(mut existing) = (match state.store.get_publisher(&ctx, ident).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let target = ObjectTarget::from_publisher(&existing);
            let dec = state.authorizer.check(&ctx, Action::Propose, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            if let Some(title) = payload.get("title").and_then(Value::as_str) {
                // G2: an organization's title comes from the CkanInstance title, and one instance
                // never renames another's organization.
                let caller_instance = payload
                    .get("instance_name")
                    .and_then(Value::as_str)
                    .or(ctx.subject.client_id.as_deref());
                let can_rename = match (&existing.declared_by_instance, caller_instance) {
                    (Some(owner_inst), Some(caller_inst)) => owner_inst == caller_inst,
                    (None, Some(caller_inst)) => {
                        existing.declared_by_instance = Some(caller_inst.to_owned());
                        true
                    }
                    _ => true,
                };
                if can_rename {
                    existing
                        .title
                        .insert(cat.default_language.clone(), title.to_owned());
                }
            }
            if let Some(desc) = payload
                .get("description")
                .or_else(|| payload.get("notes"))
                .and_then(Value::as_str)
            {
                existing
                    .description
                    .insert(cat.default_language.clone(), desc.to_owned());
            }
            if let Some(img) = payload.get("image_url").and_then(Value::as_str) {
                existing.logo_url = Some(img.to_owned());
            }
            existing.modified = Utc::now();
            match state.store.upsert_publisher(&ctx, existing).await {
                Ok(saved) => ckan_ok(
                    &base,
                    &action,
                    publisher_to_ckan(&saved, &cat.default_language, 0),
                    false,
                ),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        "organization_delete" => {
            let Some(ident) = payload.get("id").and_then(Value::as_str) else {
                return ckan_err(
                    &base,
                    &action,
                    DataroomError::field_error("id", "Missing value"),
                );
            };
            let Some(existing) = (match state.store.get_publisher(&ctx, ident).await {
                Ok(p) => p,
                Err(e) => return ckan_err(&base, &action, e),
            }) else {
                return ckan_err(&base, &action, DataroomError::NotFound("Not found".into()));
            };
            let target = ObjectTarget::from_publisher(&existing);
            let dec = state.authorizer.check(&ctx, Action::Delete, &target);
            if !dec.allowed {
                return ckan_err(&base, &action, decision_error(dec));
            }
            match state.store.withdraw_publisher(&ctx, &existing.id).await {
                Ok(_) => ckan_ok(&base, &action, json!({}), false),
                Err(e) => ckan_err(&base, &action, e),
            }
        }
        unknown => ckan_err(
            &base,
            &action,
            DataroomError::NotFound(format!("Action name not known: {unknown}")),
        ),
    }
}

fn publisher_to_ckan(p: &Publisher, default_lang: &str, package_count: usize) -> Value {
    let title = text_in_lang(&p.title, Some(default_lang)).unwrap_or(&p.name);
    let desc = text_in_lang(&p.description, Some(default_lang)).unwrap_or("");
    json!({
        "id": p.id,
        "name": p.name,
        "title": title,
        "display_name": title,
        "description": desc,
        "image_url": p.logo_url.as_deref().unwrap_or(""),
        "created": p.created.to_rfc3339(),
        "is_organization": true,
        "approval_status": "approved",
        "type": "organization",
        "state": p.state.ckan_state(),
        "package_count": package_count,
        "uri": p.uri,
    })
}

fn parse_resource_view(
    payload: &Value,
    existing_id: &str,
    resource_id: &str,
    view_type: &str,
) -> ResourceView {
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing_id)
        .to_owned();
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("View")
        .to_owned();
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let order = payload.get("order").and_then(Value::as_i64).unwrap_or(0) as i32;

    let mut config = BTreeMap::new();
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            if !matches!(
                k.as_str(),
                "id" | "resource_id" | "view_type" | "title" | "description" | "order"
            ) {
                config.insert(k.clone(), v.clone());
            }
        }
    }

    ResourceView {
        id: if id.is_empty() {
            format!("view-{}", Uuid::now_v7())
        } else {
            id
        },
        resource_id: resource_id.to_owned(),
        view_type: view_type.to_owned(),
        title,
        description,
        order,
        config,
    }
}
