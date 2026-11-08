#![forbid(unsafe_code)]
//! OData v4 routes (`/odata/v4/$metadata`, `/odata/v4/{set}`, `/odata/v4/{set}({key})`,
//! `/odata/v4/{set}/$count`, `/odata/v4/{set}/$export`, `/odata/v4/{set}/$allowed`) and
//! OpenAPI 3.1 document (`/api/v1/openapi.json`) built on `odata-table` (C14).

use crate::{dataset_shown, problem_response, DataroomState};
use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use dataroom_core::odata_defs::{dataroom_service, scope_for_context};
use dataroom_core::{DataroomError, Publisher, RecordState};
use odata_table::{allowed, export, key_options, query, Operation, QueryOptions};
use odata_url::{EdmModel, Key, KeyValue, Limits, Literal, ODataUri, Segment};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

const MAX_ODATA_PAGE: u64 = 100;

/// Serves the OpenAPI 3.1 specification generated from `dataroom_service()` (`C7`, `C14`).
pub async fn handle_openapi(State(_state): State<DataroomState>) -> Response {
    let doc = dataroom_service().openapi("/odata/v4");
    (StatusCode::OK, Json(doc)).into_response()
}

/// Serves the OData v4.01 CSDL XML `$metadata` document (`C14`).
pub async fn handle_metadata(State(_state): State<DataroomState>) -> Response {
    let model = dataroom_service().model();
    let mut xml = Vec::new();
    if let Err(e) = odata_csdl::write_xml(&model, &odata_csdl::Limits::strict(), &mut xml) {
        return problem_response(DataroomError::field_error("metadata", format!("{e:?}")));
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

/// Dispatches OData v4 queries, `$count`, `$export`, and `$allowed` through `odata_table` (`C14`).
pub async fn handle_odata(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(rest): Path<String>,
    OriginalUri(orig_uri): OriginalUri,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };

    let path = rest.trim_start_matches('/');
    let raw_query = orig_uri.query().map(str::to_owned);

    // Compute visible dataset IDs for this caller so `Scope` restricts Datasets (and their
    // Distributions) before `$filter`.
    // A dataset is a row where `package_show` shows it: hidden (`jc_hide`) datasets only to
    // their editors, datasets of a withdrawn publisher not to anonymous callers.
    let visible_ds_ids = if path.starts_with("Datasets") || path.starts_with("Distributions") {
        let all_ds = match state.store.list_datasets(&ctx).await {
            Ok(d) => d,
            Err(e) => return problem_response(e),
        };
        let all_pubs = match state.store.list_publishers(&ctx).await {
            Ok(p) => p,
            Err(e) => return problem_response(e),
        };
        let pub_map: BTreeMap<String, Publisher> = all_pubs
            .into_iter()
            .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
            .collect();
        let ids: Vec<String> = all_ds
            .into_iter()
            .filter(|d| {
                d.state == RecordState::Active
                    && dataset_shown(&state, &ctx, &cat, d, pub_map.get(&d.publisher_id))
            })
            .map(|d| d.id)
            .collect();
        Some(ids)
    } else {
        None
    };

    // 1. /{set}/$allowed?op=...&key=...
    if let Some(set_name) = path.strip_suffix("/$allowed").filter(|s| !s.contains('/')) {
        let table = match state.store.odata_table(&ctx, set_name).await {
            Ok(t) => t,
            Err(e) => return problem_response(e),
        };
        let scope = scope_for_context(&ctx, state.authorizer.as_ref(), set_name, visible_ds_ids);
        let mut op = None;
        let mut key_val = None;
        for (k, v) in url::form_urlencoded::parse(raw_query.as_deref().unwrap_or("").as_bytes()) {
            match k.as_ref() {
                "op" => op = Operation::parse(&v),
                "key" => key_val = Some(Value::String(v.into_owned())),
                _ => {}
            }
        }
        let Some(op) = op else {
            return problem_response(DataroomError::field_error(
                "op",
                "op is required (read, export, create, update, delete)",
            ));
        };
        match allowed(table.as_ref(), &scope, op, key_val.as_ref()).await {
            Ok(dec) => return (StatusCode::OK, Json(dec.to_json())).into_response(),
            Err(e) => return problem_response(DataroomError::from(e)),
        }
    }

    // 2. /{set}/$export?$filter=...
    if let Some(set_name) = path.strip_suffix("/$export").filter(|s| !s.contains('/')) {
        let table = match state.store.odata_table(&ctx, set_name).await {
            Ok(t) => t,
            Err(e) => return problem_response(e),
        };
        let scope = scope_for_context(&ctx, state.authorizer.as_ref(), set_name, visible_ds_ids);
        let model = Arc::new(dataroom_service().model());
        let rel = match &raw_query {
            Some(q) if !q.is_empty() => format!("{set_name}?{q}"),
            _ => set_name.to_owned(),
        };
        let parsed = match odata_url::parse(&rel, &EdmModel(&model), &Limits::strict()) {
            Ok(u) => u,
            Err(e) => {
                return problem_response(DataroomError::field_error(
                    "$filter",
                    format!("invalid OData URL: {e:?}"),
                ))
            }
        };
        let ODataUri::Resource { options, .. } = parsed else {
            return problem_response(DataroomError::NotSupported("resource path".into()));
        };
        match export(table.as_ref(), &options, &scope, MAX_ODATA_PAGE, 10_000).await {
            Ok(rows) => return (StatusCode::OK, Json(Value::Array(rows))).into_response(),
            Err(e) => return problem_response(DataroomError::from(e)),
        }
    }

    // 3. Standard OData resource path (`/{set}`, `/{set}({key})`, `/{set}/$count`)
    let model = Arc::new(dataroom_service().model());
    let relative = match &raw_query {
        Some(q) if !q.is_empty() => format!("{path}?{q}"),
        _ => path.to_owned(),
    };
    let parsed = match odata_url::parse(&relative, &EdmModel(&model), &Limits::strict()) {
        Ok(u) => u,
        Err(e) => {
            return problem_response(DataroomError::field_error(
                "odata",
                format!("invalid OData URL: {e:?}"),
            ))
        }
    };

    let ODataUri::Resource {
        path: mut segments,
        options,
    } = parsed
    else {
        return problem_response(DataroomError::NotSupported("resource path".into()));
    };

    let is_count_segment = segments.last() == Some(&Segment::Count);
    if is_count_segment {
        segments.pop();
    }
    let Some((Segment::EntitySet(set_name), tail)) = segments.split_first() else {
        return problem_response(DataroomError::NotSupported("entity set".into()));
    };

    let table = match state.store.odata_table(&ctx, set_name).await {
        Ok(t) => t,
        Err(e) => return problem_response(e),
    };
    let scope = scope_for_context(&ctx, state.authorizer.as_ref(), set_name, visible_ds_ids);

    if is_count_segment && tail.is_empty() {
        let count_opts = QueryOptions {
            filter: options.filter.clone(),
            aliases: options.aliases.clone(),
            top: Some(0),
            count: Some(true),
            ..QueryOptions::default()
        };
        let ans = match query(table.as_ref(), &count_opts, &scope, MAX_ODATA_PAGE).await {
            Ok(a) => a,
            Err(e) => return problem_response(DataroomError::from(e)),
        };
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::HeaderName::from_static("odata-version"), "4.01"),
            ],
            ans.count.unwrap_or(0).to_string(),
        )
            .into_response();
    }

    match tail {
        [] => {
            let ans = match query(table.as_ref(), &options, &scope, MAX_ODATA_PAGE).await {
                Ok(a) => a,
                Err(e) => return problem_response(DataroomError::from(e)),
            };
            let mut body = Map::new();
            body.insert(
                "@odata.context".into(),
                json!(format!("/odata/v4/$metadata#{set_name}")),
            );
            if let Some(c) = ans.count {
                body.insert("@odata.count".into(), json!(c));
            }
            if let Some(next_tok) = ans.next {
                body.insert(
                    "@odata.nextLink".into(),
                    json!(format!("/odata/v4/{set_name}?$skiptoken={next_tok}")),
                );
            }
            body.insert("value".into(), Value::Array(ans.rows));
            (
                StatusCode::OK,
                [(header::HeaderName::from_static("odata-version"), "4.01")],
                Json(Value::Object(body)),
            )
                .into_response()
        }
        [Segment::Key(Key::Single(KeyValue::Literal(lit)))] => {
            let key_val = match lit {
                Literal::String(s) => Value::String(s.clone()),
                Literal::Int(i) => Value::from(*i),
                _ => {
                    return problem_response(DataroomError::field_error(
                        "key",
                        "key must be string or integer",
                    ))
                }
            };
            let by_key = match key_options(table.info(), &key_val, options.select.clone()) {
                Ok(o) => o,
                Err(e) => return problem_response(DataroomError::from(e)),
            };
            let ans = match query(table.as_ref(), &by_key, &scope, 1).await {
                Ok(a) => a,
                Err(e) => return problem_response(DataroomError::from(e)),
            };
            let Some(first) = ans.rows.into_iter().next() else {
                return problem_response(DataroomError::NotFound("entity not found".into()));
            };
            (
                StatusCode::OK,
                [(header::HeaderName::from_static("odata-version"), "4.01")],
                Json(first),
            )
                .into_response()
        }
        _ => problem_response(DataroomError::NotSupported(
            "navigation or nested segment".into(),
        )),
    }
}
