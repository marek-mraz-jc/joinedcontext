// SPDX-License-Identifier: MIT
//! [`table_router`]: any [`Table`] served read-only, with the service and metadata documents.

use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::header::{CONTENT_TYPE, ETAG};
use axum::http::{Extensions, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use odata_json::{Ctx, EntityValue, Page, Resource};
use odata_table::{
    allowed, export, key_options, query, Answer, Decision, Operation, Scope, Table, TableError,
    Value,
};
use odata_url::{EdmModel, Key, KeyValue, Literal, ODataUri, QueryOptions, Segment};

use crate::{
    json_level, json_response, metadata_router, read_only, url_error, ODataErrorResponse,
    ODataRequest, ODataState,
};

#[derive(Clone)]
struct Tables {
    state: Arc<ODataState>,
    sets: Arc<[Arc<dyn Table>]>,
    max_page: u64,
}

impl FromRef<Tables> for Arc<ODataState> {
    fn from_ref(tables: &Tables) -> Self {
        tables.state.clone()
    }
}

/// The service document, `$metadata` and each of `tables` read-only: `GET /{set}` (a page of at
/// most `max_page` rows, a `nextLink` to the next), `GET /{set}/$count`, `GET /{set}('key')`,
/// `GET /{set}/$export` (every matching row as a plain JSON array, at most `100 * max_page`) and
/// `GET /{set}/$allowed?op=<operation>&key=<key>` (the decision the call would run, as JSON).
/// The model of `state` holds every set.
///
/// A request reads the [`Scope`] its caller's middleware put in the request extensions
/// (`axum::Extension(scope)`), applied before `$filter`; a request without one is refused 403,
/// so a set is never served unscoped by accident. [`Scope::all`] serves every row. A source
/// with an entity tag (a Git commit) answers it as `ETag`.
///
/// ```
/// use std::sync::Arc;
/// use odata_axum::{table_router, ODataState};
/// use odata_table::{MemoryTable, Scope, Table, TableInfo};
/// # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef};
/// # let mut ty = EntityType::new("Thing");
/// # ty.key = vec!["id".into()];
/// # ty.properties = vec![Property::new("id", TypeRef::single("Edm.String"))];
/// # let mut schema = Schema::new("Test");
/// # schema.entity_types.push(ty);
/// # let mut container = EntityContainer::new("Service");
/// # container.entity_sets.push(EntitySet::new("Things", "Test.Thing"));
/// # schema.entity_container = Some(container);
/// # let model = Arc::new(Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![schema] });
/// let things = MemoryTable::new(TableInfo::new(model.clone(), "Things").ok_or("no set")?, vec![])
///     .map_err(|e| e.to_string())?;
/// let state = ODataState {
///     model,
///     url_model: None,
///     service_root: "https://example.org/odata/".into(),
///     url_limits: odata_url::Limits::strict(),
///     csdl_limits: odata_csdl::Limits::strict(),
///     max_header_len: 1024,
///     metadata_etag: None,
///     metadata: Arc::default(),
///     custom_options: vec![],
/// };
/// let tables: Vec<Arc<dyn Table>> = vec![Arc::new(things)];
/// let app: axum::Router = axum::Router::new()
///     .nest_service("/odata", table_router(Arc::new(state), tables, 1000))
///     .layer(axum::Extension(Scope::all()));
/// # Ok::<(), String>(())
/// ```
pub fn table_router(state: Arc<ODataState>, tables: Vec<Arc<dyn Table>>, max_page: u64) -> Router {
    let served = Tables {
        state: state.clone(),
        sets: tables.into(),
        max_page,
    };
    metadata_router(state).merge(
        Router::new()
            .route("/{*rest}", get(dispatch).fallback(written))
            .with_state(served),
    )
}

async fn written(request: ODataRequest) -> Response {
    read_only(request.version)
}

/// `/{set}/$allowed` and `/{set}/$export` (outside the OData URL grammar), else an OData read.
async fn dispatch(
    State(served): State<Tables>,
    request: axum::extract::Request,
) -> Result<Response, ODataErrorResponse> {
    let (mut parts, _) = request.into_parts();
    let path = parts.uri.path().trim_start_matches('/');
    let raw = parts.uri.query().map(str::to_owned);
    if let Some(set) = path.strip_suffix("/$allowed").filter(|s| !s.contains('/')) {
        return may(&served, set, raw, &parts.extensions).await;
    }
    if let Some(set) = path.strip_suffix("/$export").filter(|s| !s.contains('/')) {
        return exported(&served, set, raw, &parts.extensions).await;
    }
    let request = ODataRequest::from_request_parts(&mut parts, &served).await?;
    read(&served, request, &parts.headers, &parts.extensions).await
}

async fn read(
    served: &Tables,
    request: ODataRequest,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Result<Response, ODataErrorResponse> {
    let version = request.version;
    let fail = |e: ODataErrorResponse| e.with_version(version);
    let Some(scope) = extensions.get::<Scope>() else {
        return Err(fail(ODataErrorResponse::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "this request carries no read scope",
        )));
    };
    let ODataUri::Resource { path, options } = &request.uri else {
        return Err(fail(unsupported("this resource")));
    };
    let Some((Segment::EntitySet(set), rest)) = path.split_first() else {
        return Err(fail(unsupported("this resource path")));
    };
    let Some(table) = served.sets.iter().find(|t| t.info().set == *set) else {
        return Err(fail(ODataErrorResponse::new(
            StatusCode::NOT_FOUND,
            "NotFound",
            "the resource does not exist",
        )));
    };
    let mut response = match rest {
        [] => {
            decided(table.as_ref(), scope, Operation::Read, None)
                .await
                .map_err(fail)?;
            let answer = run(table.as_ref(), options, scope, served.max_page)
                .await
                .map_err(fail)?;
            let next = answer.next.map(|token| {
                let mut options = options.clone();
                options.skiptoken = Some(token.to_string());
                let uri = ODataUri::Resource {
                    path: path.clone(),
                    options,
                };
                format!("{}{uri}", served.state.service_root)
            });
            let level = json_level(&request, headers, served.state.max_header_len)?;
            write(
                &served.state,
                set,
                &answer,
                level,
                version,
                next.as_deref(),
                false,
            )?
        }
        [Segment::Count] => {
            decided(table.as_ref(), scope, Operation::Read, None)
                .await
                .map_err(fail)?;
            let counted = QueryOptions {
                filter: options.filter.clone(),
                aliases: options.aliases.clone(),
                search: options.search.clone(),
                top: Some(0),
                count: Some(true),
                ..QueryOptions::default()
            };
            let answer = run(table.as_ref(), &counted, scope, served.max_page)
                .await
                .map_err(fail)?;
            counted_response(version, answer.count.unwrap_or(0))
        }
        [Segment::Key(Key::Single(KeyValue::Literal(key)))] => {
            let key = key_value(key).map_err(fail)?;
            decided(table.as_ref(), scope, Operation::Read, Some(&key))
                .await
                .map_err(fail)?;
            let by_key = key_options(table.info(), &key, options.select.clone())
                .map_err(|e| fail(refused(e)))?;
            let answer = run(table.as_ref(), &by_key, scope, served.max_page)
                .await
                .map_err(fail)?;
            if answer.rows.is_empty() {
                return Err(fail(not_found()));
            }
            let level = json_level(&request, headers, served.state.max_header_len)?;
            write(&served.state, set, &answer, level, version, None, true)?
        }
        _ => return Err(fail(unsupported("this resource path"))),
    };
    if let Some(tag) = table
        .etag()
        .and_then(|t| HeaderValue::from_str(&format!("\"{t}\"")).ok())
    {
        response.headers_mut().insert(ETAG, tag);
    }
    Ok(response)
}

/// The `/$count` answer: the number as plain text.
fn counted_response(version: odata_json::ODataVersion, count: u64) -> Response {
    let mut response = (StatusCode::OK, count.to_string()).into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    headers.insert("odata-version", HeaderValue::from_static(version.as_str()));
    response
}

async fn run(
    table: &dyn Table,
    options: &QueryOptions,
    scope: &Scope,
    max_page: u64,
) -> Result<Answer, ODataErrorResponse> {
    query(table, options, scope, max_page)
        .await
        .map_err(refused)
}

fn refused(e: TableError) -> ODataErrorResponse {
    match e {
        TableError::NotImplemented(what) => unsupported(&what),
        TableError::UnknownProperty(_) => ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "a query option names something this service does not have",
        ),
        TableError::BadRequest(why) => {
            ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", &why)
        }
        // the source's text stays out of the answer: it can name tables, paths or hosts
        TableError::Source(_) => ODataErrorResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
            "the source of this entity set could not be read",
        ),
    }
}

/// The decision a call runs first: `Ok` when allowed, else the refusal it answers (404 for an
/// entity the caller cannot see, as for a missing one; 403 otherwise).
async fn decided(
    table: &dyn Table,
    scope: &Scope,
    operation: Operation,
    key: Option<&Value>,
) -> Result<Decision, ODataErrorResponse> {
    let decision = allowed(table, scope, operation, key)
        .await
        .map_err(refused)?;
    if decision.allowed {
        Ok(decision)
    } else if decision.not_found {
        Err(not_found())
    } else {
        Err(ODataErrorResponse::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            &decision.reason,
        ))
    }
}

fn not_found() -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::NOT_FOUND,
        "NotFound",
        "the resource does not exist",
    )
}

fn key_value(key: &Literal) -> Result<Value, ODataErrorResponse> {
    match key {
        Literal::String(s) => Ok(Value::String(s.clone())),
        Literal::Int(i) => Ok(Value::from(*i)),
        _ => Err(ODataErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "a key is a string or an integer",
        )),
    }
}

/// The scope and the served set of a request outside the OData URL grammar.
fn scoped<'s>(
    served: &'s Tables,
    set: &str,
    extensions: &'s Extensions,
) -> Result<(&'s dyn Table, &'s Scope), ODataErrorResponse> {
    let scope = extensions.get::<Scope>().ok_or_else(|| {
        ODataErrorResponse::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "this request carries no read scope",
        )
    })?;
    let table = served
        .sets
        .iter()
        .find(|t| t.info().set == set)
        .ok_or_else(not_found)?;
    Ok((table.as_ref(), scope))
}

/// `GET /{set}/$allowed?op=<operation>[&key=<key>]`: the decision the call would run, as JSON.
async fn may(
    served: &Tables,
    set: &str,
    raw: Option<String>,
    extensions: &Extensions,
) -> Result<Response, ODataErrorResponse> {
    let (table, scope) = scoped(served, set, extensions)?;
    let bad = |m: &str| ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", m);
    let (mut op, mut key) = (None, None);
    for pair in raw
        .as_deref()
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = odata_url::percent_decoded(value)
            .ok_or_else(|| bad("a parameter is not valid UTF-8"))?;
        match name {
            "op" => {
                op = Some(
                    Operation::parse(&value)
                        .ok_or_else(|| bad("op is one of read, export, create, update, delete"))?,
                );
            }
            "key" => key = Some(value),
            _ => return Err(bad("$allowed takes op and key only")),
        }
    }
    let op = op.ok_or_else(|| bad("op is required, e.g. op=update"))?;
    let key = match key {
        None => None,
        Some(text) if integer_key(table) => Some(Value::from(
            text.parse::<i64>()
                .map_err(|_| bad("key is an integer here"))?,
        )),
        Some(text) => Some(Value::String(text)),
    };
    let decision = allowed(table, scope, op, key.as_ref())
        .await
        .map_err(refused)?;
    let body = decision.to_json();
    let mut response = (StatusCode::OK, body.to_string()).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(response)
}

fn integer_key(table: &dyn Table) -> bool {
    let info = table.info();
    info.model
        .entity_type(&info.entity_type)
        .and_then(|ty| {
            info.model
                .lineage(ty)
                .into_iter()
                .find_map(|t| t.properties.iter().find(|p| p.name == info.key))
        })
        .is_some_and(|p| {
            matches!(
                p.ty.name.as_str(),
                "Edm.Int64" | "Edm.Int32" | "Edm.Int16" | "Edm.Byte" | "Edm.SByte"
            )
        })
}

/// `GET /{set}/$export?$filter=...`: every matching row as a plain JSON array.
async fn exported(
    served: &Tables,
    set: &str,
    raw: Option<String>,
    extensions: &Extensions,
) -> Result<Response, ODataErrorResponse> {
    let (table, scope) = scoped(served, set, extensions)?;
    decided(table, scope, Operation::Export, None).await?;
    let relative = format!("{set}?{}", raw.unwrap_or_default());
    let state = &served.state;
    let uri = odata_url::parse(&relative, &EdmModel(state.url_model()), &state.url_limits)
        .map_err(|e| url_error(&e, set.len()))?;
    let ODataUri::Resource { options, .. } = uri else {
        return Err(unsupported("this resource"));
    };
    let max_rows = usize::try_from(served.max_page.saturating_mul(100)).unwrap_or(usize::MAX);
    let rows = export(table, &options, scope, served.max_page, max_rows)
        .await
        .map_err(refused)?;
    let mut response = (StatusCode::OK, Value::Array(rows).to_string()).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(response)
}

fn unsupported(construct: &str) -> ODataErrorResponse {
    ODataErrorResponse::new(
        StatusCode::NOT_IMPLEMENTED,
        "NotImplemented",
        &format!("{construct} is not supported by this service"),
    )
}

/// The entity id of `key` in `set`: `Set('k')`, a quote doubled, or `Set(5)`.
fn entity_id(set: &str, key: &Value) -> String {
    match key {
        Value::String(s) => format!("{set}('{}')", s.replace('\'', "''")),
        other => format!("{set}({other})"),
    }
}

fn write(
    state: &ODataState,
    set: &str,
    answer: &Answer,
    level: odata_json::MetadataLevel,
    version: odata_json::ODataVersion,
    next_link: Option<&str>,
    single: bool,
) -> Result<Response, ODataErrorResponse> {
    let ctx = Ctx {
        service_root: &state.service_root,
        level,
        version,
        metadata_etag: state.metadata_etag.as_deref(),
    };
    let names: Vec<&str> = answer.select.iter().flatten().map(String::as_str).collect();
    let resource = Resource::new(&state.model, set).ok_or_else(|| {
        ODataErrorResponse::new(
            StatusCode::NOT_FOUND,
            "NotFound",
            "the resource does not exist",
        )
    })?;
    let resource = if answer.select.is_some() {
        resource.select(&names)
    } else {
        resource
    };
    let entities: Vec<EntityValue> = answer
        .rows
        .iter()
        .zip(&answer.keys)
        .map(|(row, key)| EntityValue {
            id: entity_id(set, key),
            properties: row
                .as_object()
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        })
        .collect();
    let mut out = Vec::new();
    let written = match (single, entities.first()) {
        (true, Some(entity)) => odata_json::write_entity(&ctx, &resource, entity, &mut out),
        _ => odata_json::write_collection(
            &ctx,
            &resource,
            &entities,
            &Page {
                count: answer.count,
                next_link,
            },
            &mut out,
        ),
    };
    written.map_err(|_| {
        ODataErrorResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
            "the answer could not be written",
        )
        .with_version(version)
    })?;
    Ok(json_response(version, level, out))
}
