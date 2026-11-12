//! One OData 4.01 service per project, generated from the operation registry (T-6655; the pilot
//! on one list was T-6632).
//!
//! - Every list operation the registry holds becomes an entity set: `jc_resource_list` once per
//!   manifest kind (`endpoints`, `pipelines`, ... by the kind's REST plural), `jc_change_list` as
//!   `changes` and `jc_workspace_list` as `workspaces`. Its entity type is the operation's own
//!   output schema (the scalar members of an item), so a member the operation does not answer
//!   is no property, and `$filter` cannot reach it.
//! - `endpointList` holds the rows of the Endpoints page, which the SDK grid draws (T-6645): the
//!   project's own Endpoints, those other projects share with it and its references whose
//!   Endpoint went away, each after the read check of the project it lives in
//!   ([`crate::api::odata_endpoints`]). Its rows also take `PATCH` and `DELETE` by key, through the
//!   REST doors, and `endpointList/$allowed` answers what those would decide.
//! - Every writing operation becomes an unbound action of the same name, its parameters the
//!   operation's input schema: `POST .../odata/jc_resource_propose` runs what
//!   `POST .../ops/jc_resource_propose` runs.
//!
//! One executor: a read calls the operation through [`ops::call`] (the registry's own permission
//! check, the same rows the REST door and MCP answer), then evaluates the OData options over
//! those rows with `odata-table`'s in-memory evaluator, the reference semantics every Antares
//! OData source shares (T-6642). The authorization runs before any option is looked at, so a
//! `$filter` only narrows what the caller may read; a project the caller may not read answers
//! 404 as the REST door does (R20).

use std::sync::{Arc, LazyLock};

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use odata_csdl::Model;
use odata_table::{
    key_options, query, Answer, EntityDef, Field, FieldKind, MemoryTable, Operation as TableOp,
    Rule, Scope, Service, Table, TableError,
};
use odata_url::{EdmModel, Key, KeyValue, Literal, ODataUri, QueryOptions, Segment, UrlErrorKind};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use utoipa::IntoParams;

use crate::api::dry_run::DryRunQuery;
use crate::api::odata_endpoints as endpoint_list;

use crate::api::ops::{caller_of, respond};
use crate::auth::session::{CurrentUser, Front};
use crate::error::{ApiError, ProblemDetails};
use crate::ops::{self, Caller};
use crate::resource::is_dns1123;
use crate::state::AppState;

const NAMESPACE: &str = "JoinedContext.Portal";

/// The most rows one answer carries: a longer read pages on with `@odata.nextLink`.
pub const MAX_TOP: u64 = 1000;

/// The most rows an entity set reads from its operation: past it the read is refused with a
/// sentence asking for a narrower list, never answered from a cut set.
// ponytail: every row is read per request; the operations' own filters (space, kind) pushed
// down when a project outgrows this.
const MAX_ROWS: usize = 10_000;

/// One entity set: the list operation that answers its rows and the input that names them.
struct ListSet {
    set: String,
    type_name: String,
    op: &'static str,
    input: Value,
    key: &'static str,
    /// The input member a page's `next` goes back in, for an operation that pages.
    cursor: Option<&'static str>,
    /// Whether the rows are the Endpoints page's ([`endpoint_list::rows`]), not the operation's.
    endpoint_list: bool,
}

/// The mapped list operations, in `$metadata` order. The ones left out and why are in T-6655.
fn list_sets() -> Vec<ListSet> {
    let mut sets: Vec<ListSet> = crate::resource::kinds()
        .map(|info| ListSet {
            set: info.plural.to_owned(),
            type_name: info.kind.to_owned(),
            op: "jc_resource_list",
            input: json!({ "kind": info.kind }),
            key: "name",
            cursor: None,
            endpoint_list: false,
        })
        .collect();
    sets.push(ListSet {
        set: "changes".into(),
        type_name: "Change".into(),
        op: "jc_change_list",
        input: json!({ "limit": 100 }),
        key: "name",
        cursor: Some("cursor"),
        endpoint_list: false,
    });
    sets.push(ListSet {
        set: "workspaces".into(),
        type_name: "Workspace".into(),
        op: "jc_workspace_list",
        input: json!({}),
        key: "name",
        cursor: None,
        endpoint_list: false,
    });
    sets.push(ListSet {
        set: endpoint_list::SET.into(),
        type_name: endpoint_list::TYPE.into(),
        op: "jc_resource_list",
        input: json!({ "kind": "Endpoint" }),
        key: endpoint_list::KEY,
        cursor: None,
        endpoint_list: true,
    });
    sets
}

/// The fields of a list operation's rows: the scalar members its output schema names for an
/// item. Objects and arrays are no property (not filterable here); the key is never null.
fn fields_of(op: &ops::Operation, key: &str) -> Vec<Field> {
    let schema = (op.output)();
    let Some(members) = schema
        .pointer("/properties/items/items/properties")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    members
        .iter()
        .filter(|(name, _)| identifier(name))
        .filter_map(|(name, member)| {
            let kind = match (
                member.get("type").and_then(Value::as_str),
                member.get("format").and_then(Value::as_str),
            ) {
                (Some("string"), Some("date-time")) => FieldKind::DateTime,
                (Some("string"), _) => FieldKind::String,
                (Some("integer"), _) => FieldKind::Integer,
                (Some("number"), _) => FieldKind::Number,
                (Some("boolean"), _) => FieldKind::Boolean,
                _ => return None,
            };
            let description = member.get("description").and_then(Value::as_str);
            let mut field = Field::new(name, kind, description.unwrap_or(""));
            field.nullable = name.as_str() != key;
            Some(field)
        })
        .collect()
}

/// An OData simple identifier: what a property, parameter and action name may be.
fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 128
}

/// The service, built once from the registry: the sets, their CSDL model and `$metadata`.
struct Served {
    sets: Vec<ListSet>,
    service: Service,
    model: Arc<Model>,
    metadata: Result<String, String>,
}

static SERVED: LazyLock<Served> = LazyLock::new(|| {
    let sets = list_sets();
    let service = Service {
        namespace: NAMESPACE.into(),
        title: "JoinedContext Portal".into(),
        sets: sets
            .iter()
            .map(|set| {
                let op = ops::find(set.op);
                let (description, fields) = if set.endpoint_list {
                    (
                        "The rows of the Endpoints page: the project's own Endpoints, those other \
                         projects share with it and its references whose Endpoint went away",
                        endpoint_list::fields(),
                    )
                } else {
                    (
                        op.map(|op| op.description).unwrap_or_default(),
                        op.map(|op| fields_of(op, set.key)).unwrap_or_default(),
                    )
                };
                EntityDef {
                    set: set.set.clone(),
                    type_name: set.type_name.clone(),
                    description: description.into(),
                    key: set.key.into(),
                    fields,
                    rules: vec![Rule {
                        operation: TableOp::Read,
                        permission: None,
                    }],
                }
            })
            .collect(),
    };
    let model = Arc::new(service.model());
    let metadata = metadata_xml(&model);
    Served {
        sets,
        service,
        model,
        metadata,
    }
});

/// The EDM type of an action parameter from its JSON schema; anything not primitive is untyped.
fn edm_type(member: &Value) -> &'static str {
    match (
        member.get("type").and_then(Value::as_str),
        member.get("format").and_then(Value::as_str),
    ) {
        (Some("string"), Some("date-time")) => "Edm.DateTimeOffset",
        (Some("string"), _) => "Edm.String",
        (Some("integer"), _) => "Edm.Int64",
        (Some("number"), _) => "Edm.Double",
        (Some("boolean"), _) => "Edm.Boolean",
        _ => "Edm.Untyped",
    }
}

/// The `<Action>` elements and their `<ActionImport>`s: one per writing operation, a parameter
/// per member of its input schema (required ones not nullable). Every name written is an
/// identifier and every type one of [`edm_type`]'s, so nothing here needs escaping.
// ponytail: written as text beside odata-csdl's XML, which has no Action yet; a model member
// when odata-csdl gains one (CSDL JSON then follows).
fn actions_xml() -> (String, String) {
    let mut actions = String::new();
    let mut imports = String::new();
    for op in ops::registry()
        .iter()
        .filter(|op| !op.annotations.read_only_hint && identifier(op.name))
    {
        let name = op.name;
        actions.push_str(&format!(r#"<Action Name="{name}">"#));
        let input = (op.input)();
        let required: Vec<&str> = input
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for (member, schema) in input
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(member, _)| identifier(member))
        {
            let nullable = if required.contains(&member.as_str()) {
                r#" Nullable="false""#
            } else {
                ""
            };
            actions.push_str(&format!(
                r#"<Parameter Name="{member}" Type="{}"{nullable}/>"#,
                edm_type(schema)
            ));
        }
        actions.push_str(r#"<ReturnType Type="Edm.Untyped"/></Action>"#);
        imports.push_str(&format!(
            r#"<ActionImport Name="{name}" Action="{NAMESPACE}.{name}"/>"#
        ));
    }
    (actions, imports)
}

/// The CSDL XML of the service: odata-csdl's model of the entity sets, the actions beside it.
fn metadata_xml(model: &Model) -> Result<String, String> {
    let mut xml = Vec::new();
    odata_csdl::write_xml(model, &odata_csdl::Limits::strict(), &mut xml)
        .map_err(|e| e.to_string())?;
    let xml = String::from_utf8(xml).map_err(|e| e.to_string())?;
    let (actions, imports) = actions_xml();
    let container = xml
        .rfind("</EntityContainer>")
        .ok_or("the model wrote no entity container")?;
    let mut xml = xml;
    xml.insert_str(container, &imports);
    let schema = xml
        .rfind("<EntityContainer")
        .ok_or("the model wrote no entity container")?;
    xml.insert_str(schema, &actions);
    Ok(xml)
}

/// The parser's bounds: a request longer, deeper or wider than these is refused before it runs.
fn limits() -> odata_url::Limits {
    odata_url::Limits {
        max_query_len: 4096,
        max_expr_depth: 16,
        max_expr_nodes: 128,
        max_select_items: 64,
        max_orderby_items: 4,
        max_in_list_len: 100,
        max_literal_len: 256,
        ..odata_url::Limits::strict()
    }
}

/// Why a query is not answered: the status, the OData error code and a sentence for a person.
#[derive(Debug, PartialEq)]
struct Refusal(StatusCode, &'static str, String);

fn bad(message: impl Into<String>) -> Refusal {
    Refusal(StatusCode::BAD_REQUEST, "BadRequest", message.into())
}

fn not_found(message: impl Into<String>) -> Refusal {
    Refusal(StatusCode::NOT_FOUND, "NotFound", message.into())
}

fn not_implemented(what: &str) -> Refusal {
    Refusal(
        StatusCode::NOT_IMPLEMENTED,
        "NotImplemented",
        format!(
            "{what} is not served here; an entity set answers $filter, $select, $orderby, $top, \
             $skip, $count and a key"
        ),
    )
}

impl From<TableError> for Refusal {
    fn from(e: TableError) -> Self {
        match e {
            TableError::NotImplemented(what) => not_implemented(&what),
            TableError::UnknownProperty(path) => bad(format!(
                "this entity set has no property {path}; $metadata lists its properties"
            )),
            TableError::BadRequest(why) => bad(why),
            // The source's own text stays in the log, never in the answer.
            TableError::Source(why) => {
                tracing::warn!("an OData entity set did not read: {why}");
                Refusal(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalServerError",
                    "the rows of this entity set could not be read".into(),
                )
            }
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "code": self.1, "message": self.2 } });
        (self.0, odata_headers("application/json"), body.to_string()).into_response()
    }
}

fn odata_headers(content_type: &'static str) -> [(header::HeaderName, HeaderValue); 2] {
    [
        (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
        (
            header::HeaderName::from_static("odata-version"),
            HeaderValue::from_static("4.01"),
        ),
    ]
}

fn service_root(state: &AppState, project: &str) -> String {
    format!(
        "{}/api/v1/projects/{project}/odata/",
        state.config.public_base_url.as_str().trim_end_matches('/')
    )
}

/// A project the caller may not read is one that is not there, as the REST reads answer (R20):
/// its service document and `$metadata` included.
fn readable_project(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    if is_dns1123(project)
        && crate::permissions::for_request(state, &user.0.identity, project).may_read_project()
    {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!("project '{project}' not found")))
    }
}

/// A row with its `metadata` and `status` members lifted to the top (a Change answers
/// `metadata.name`), a top-level member winning: the properties are read off the top.
fn flat(row: Value) -> Value {
    let Value::Object(mut top) = row else {
        return row;
    };
    for nested in ["metadata", "status"] {
        if let Some(Value::Object(inner)) = top.remove(nested) {
            for (member, value) in inner {
                top.entry(member).or_insert(value);
            }
        }
    }
    Value::Object(top)
}

/// Every row of `set` the caller may read: its operation called through the registry door, page
/// after page. The operation's refusal is the REST door's (ProblemDetails, 403 or 404).
async fn rows_of(
    set: &ListSet,
    caller: &Caller,
    state: &AppState,
    project: &str,
) -> Result<Vec<Value>, Box<Response>> {
    if set.endpoint_list {
        return endpoint_list::rows(state, &caller.identity, project)
            .await
            .map_err(|refused| Box::new(refused.into_response()));
    }
    let Some(op) = ops::find(set.op) else {
        return Err(
            ApiError::Internal(format!("the operation {} is not registered", set.op))
                .into_response()
                .into(),
        );
    };
    let mut input = set.input.clone();
    let mut rows = Vec::new();
    loop {
        let page = ops::call(op, caller, state, project, input.clone())
            .await
            .map_err(IntoResponse::into_response)?;
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let empty = items.is_empty();
        rows.extend(items.into_iter().map(flat));
        if rows.len() > MAX_ROWS {
            return Err(bad(format!(
                "{} holds more than {MAX_ROWS} rows here, more than one OData read covers",
                set.set
            ))
            .into_response()
            .into());
        }
        match (set.cursor, page.get("next")) {
            (Some(member), Some(next)) if !next.is_null() && !empty => {
                input[member] = next.clone();
            }
            _ => return Ok(rows),
        }
    }
}

/// The value of a key literal: a string or an integer.
fn key_value(key: &Literal) -> Result<Value, Refusal> {
    match key {
        Literal::String(s) => Ok(Value::String(s.clone())),
        Literal::Int(i) => Ok(json!(i)),
        _ => Err(bad(
            "a key is a string or an integer, e.g. ('air-counters')",
        )),
    }
}

/// The OData collection answer of `answer`, with its next link when more rows follow.
fn collection(root: &str, path: &[Segment], options: &QueryOptions, answer: Answer) -> Value {
    let set = match path.first() {
        Some(Segment::EntitySet(set)) => set.as_str(),
        _ => "",
    };
    let mut body = Map::new();
    let context = match &answer.select {
        Some(names) => format!("{root}$metadata#{set}({})", names.join(",")),
        None => format!("{root}$metadata#{set}"),
    };
    body.insert("@odata.context".into(), Value::String(context));
    if let Some(count) = answer.count {
        body.insert("@odata.count".into(), json!(count));
    }
    body.insert("value".into(), Value::Array(answer.rows));
    if let Some(token) = answer.next {
        let mut next = options.clone();
        next.skiptoken = Some(token.to_string());
        let next = ODataUri::Resource {
            path: path.to_vec(),
            options: next,
        };
        body.insert(
            "@odata.nextLink".into(),
            Value::String(format!("{root}{next}")),
        );
    }
    Value::Object(body)
}

/// The answer to `target` (the resource path below the service root, with its query) over the
/// rows the caller may read.
async fn answer(
    state: &AppState,
    caller: &Caller,
    project: &str,
    target: &str,
) -> Result<Response, Box<Response>> {
    let served = &*SERVED;
    // An entity set that is not there answers 404, as an unknown plural does on the REST door.
    let name = target.split(['(', '/', '?']).next().unwrap_or_default();
    if !served.sets.iter().any(|set| set.set == name) {
        return Err(not_found(format!(
            "no entity set {name}; $metadata lists the entity sets"
        ))
        .into_response()
        .into());
    }
    let parsed = odata_url::parse(target, &EdmModel(&served.model), &limits()).map_err(|e| {
        match e.kind {
            UrlErrorKind::NotImplemented(_) => not_implemented(&e.to_string()),
            UrlErrorKind::Unresolved => bad(format!(
                "{e}: $metadata lists the entity sets and their properties"
            )),
            _ => bad(e.to_string()),
        }
        .into_response()
    })?;
    let ODataUri::Resource { path, options } = parsed else {
        return Err(not_implemented("this resource").into_response().into());
    };
    let Some((Segment::EntitySet(name), rest)) = path.split_first() else {
        return Err(not_implemented("this resource path").into_response().into());
    };
    let Some(set) = served.sets.iter().find(|set| &set.set == name) else {
        return Err(not_found(format!("no entity set {name}"))
            .into_response()
            .into());
    };
    let rows = rows_of(set, caller, state, project).await?;
    let fail = |e: TableError| Refusal::from(e).into_response();
    let info = served
        .service
        .info(served.model.clone(), name)
        .ok_or_else(|| fail(TableError::Source(format!("{name} is not in the model"))))?;
    let table =
        MemoryTable::new(info, rows).map_err(|e| fail(TableError::Source(e.to_string())))?;
    let root = service_root(state, project);
    let all = Scope::all();
    match rest {
        [] => {
            let answered = query(&table, &options, &all, MAX_TOP).await.map_err(fail)?;
            Ok((
                odata_headers("application/json;odata.metadata=minimal"),
                collection(&root, &path, &options, answered).to_string(),
            )
                .into_response())
        }
        [Segment::Count] => {
            let counted = QueryOptions {
                filter: options.filter.clone(),
                aliases: options.aliases.clone(),
                top: Some(0),
                count: Some(true),
                ..QueryOptions::default()
            };
            let answered = query(&table, &counted, &all, MAX_TOP).await.map_err(fail)?;
            Ok((
                odata_headers("text/plain"),
                answered.count.unwrap_or(0).to_string(),
            )
                .into_response())
        }
        [Segment::Key(Key::Single(KeyValue::Literal(key)))] => {
            let key = key_value(key).map_err(IntoResponse::into_response)?;
            let by_key = key_options(table.info(), &key, options.select.clone()).map_err(fail)?;
            let answered = query(&table, &by_key, &all, MAX_TOP).await.map_err(fail)?;
            let Some(Value::Object(row)) = answered.rows.into_iter().next() else {
                return Err(not_found(format!("{name} holds no {key} you may read"))
                    .into_response()
                    .into());
            };
            let mut body = Map::new();
            body.insert(
                "@odata.context".into(),
                Value::String(format!("{root}$metadata#{name}/$entity")),
            );
            body.extend(row);
            Ok((
                odata_headers("application/json;odata.metadata=minimal"),
                Value::Object(body).to_string(),
            )
                .into_response())
        }
        _ => Err(not_implemented("this resource path").into_response().into()),
    }
}

/// The resource path below `/odata/` of the request, with its query, as the client sent it
/// (still percent-encoded, as the OData parser reads it).
fn target_of(uri: &axum::http::Uri, project: &str) -> Option<String> {
    let rest = uri
        .path()
        .strip_prefix(&format!("/api/v1/projects/{project}/odata/"))?;
    Some(match uri.query() {
        Some(query) => format!("{rest}?{query}"),
        None => rest.to_owned(),
    })
}

async fn read(
    user: CurrentUser,
    front: Front,
    state: AppState,
    project: String,
    uri: axum::http::Uri,
) -> Result<Response, ApiError> {
    readable_project(&state, &user, &project)?;
    let Some(target) = target_of(&uri, &project) else {
        return Err(ApiError::NotFound("no such OData resource".into()));
    };
    let caller = caller_of(user, front);
    Ok(answer(&state, &caller, &project, &target)
        .await
        .unwrap_or_else(|refused| *refused))
}

/// `GET /api/v1/projects/{project}/odata/{resource}` (T-6655).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/odata/{resource}",
    summary = "Query an OData Entity Set",
    description = "One entity set of the project's OData service, the rows its list operation answers the caller (jc_resource_list per kind by its plural, jc_change_list as changes, jc_workspace_list as workspaces), read with $filter (eq, ne, gt, ge, lt, le, in, and, or, not, contains, startswith, endswith), $select, $orderby, $top, $skip and $count; pages of at most 1000 rows with @odata.nextLink. A key reads one row: endpoints('air-counters') (T-6655).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name, e.g. helsinki"),
        ("resource" = String, Path, description = "An entity set, e.g. endpoints, or one row of it by key, e.g. endpoints('air-counters')"),
        ("$filter" = Option<String>, Query, description = "A predicate over the properties, e.g. space eq 'air' and startswith(name,'bikes')"),
        ("$select" = Option<String>, Query, description = "Properties to return, comma-separated, e.g. name,title"),
        ("$orderby" = Option<String>, Query, description = "Properties to sort by, each with asc or desc, e.g. title desc"),
        ("$top" = Option<u64>, Query, description = "Rows to return, e.g. 50"),
        ("$skip" = Option<u64>, Query, description = "Rows to skip first, e.g. 50"),
        ("$count" = Option<bool>, Query, description = "true adds @odata.count, the rows the filter keeps"),
    ),
    responses(
        (status = 200, description = "The OData collection (@odata.context, @odata.count when asked, value, @odata.nextLink when more rows follow) or one entity", body = Object),
        (status = 400, description = "A name the entity set does not have, a bad expression or a query over its bounds, as an OData error"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The list operation refused the caller, as its REST route does", body = ProblemDetails),
        (status = 404, description = "A project the caller may not read, an entity set or key that is not there", body = ProblemDetails),
        (status = 501, description = "An OData option or function this service does not serve, as an OData error"),
    )
)]
pub async fn query_set(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, _resource)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    read(user, front, state, project, uri).await
}

/// `GET /api/v1/projects/{project}/odata/{resource}/$count` (T-6655).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/odata/{resource}/$count",
    summary = "Count an OData Entity Set",
    description = "The number of rows of the entity set the caller may read that $filter keeps, as plain text (T-6655).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name, e.g. helsinki"),
        ("resource" = String, Path, description = "An entity set, e.g. endpoints"),
        ("$filter" = Option<String>, Query, description = "A predicate over the properties, e.g. space eq 'air'"),
    ),
    responses(
        (status = 200, description = "The count as text, e.g. 12", content_type = "text/plain", body = String),
        (status = 400, description = "A bad expression, as an OData error"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The list operation refused the caller", body = ProblemDetails),
        (status = 404, description = "A project the caller may not read or an entity set that is not there", body = ProblemDetails),
    )
)]
pub async fn count_set(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, _resource)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    read(user, front, state, project, uri).await
}

/// `POST /api/v1/projects/{project}/odata/{resource}` (T-6655).
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/odata/{resource}",
    summary = "Invoke an OData Action",
    description = "Runs the writing operation of the same name with the body as its input, under the caller's grants: the action jc_resource_propose runs what POST /ops/jc_resource_propose runs. $metadata lists every action and its parameters (T-6655).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name, e.g. helsinki"),
        ("resource" = String, Path, description = "An action, a writing operation by name, e.g. jc_resource_propose"),
    ),
    request_body(
        content = Option<Value>,
        description = "The action's parameters, as the operation's input schema names them",
        content_type = "application/json",
        example = json!({ "id": "c-0001" })
    ),
    responses(
        (status = 200, description = "The operation ran", body = Object),
        (status = 202, description = "The operation opened a change proposal", body = Object),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not run it", body = ProblemDetails),
        (status = 404, description = "No action of that name, or a project the caller may not read", body = ProblemDetails),
        (status = 409, description = "Conflict or verdict required", body = Object),
        (status = 422, description = "Input the operation's schema refuses", body = ProblemDetails),
    )
)]
pub async fn invoke_action(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, resource)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let Some(op) = ops::find(&resource).filter(|op| !op.annotations.read_only_hint) else {
        return Err(ApiError::NotFound(format!(
            "no action named '{resource}' in this OData service; $metadata lists them"
        )));
    };
    let input: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?
    };
    respond(
        op,
        ops::call(op, &caller_of(user, front), &state, &project, input).await,
    )
}

/// The key of the `endpointList` row a write names, `endpointList('<key>')`, read as a GET reads
/// it; a write on any other resource is refused, since every other change is an action.
fn row_key(uri: &axum::http::Uri, project: &str) -> Result<String, Refusal> {
    let elsewhere = || {
        Refusal(
            StatusCode::METHOD_NOT_ALLOWED,
            "MethodNotAllowed",
            format!(
                "only a row of {set} changes here, e.g. {set}('air-counters'); every other change \
                 is an action $metadata lists",
                set = endpoint_list::SET
            ),
        )
    };
    let target = target_of(uri, project).ok_or_else(elsewhere)?;
    let parsed = odata_url::parse(&target, &EdmModel(&SERVED.model), &limits())
        .map_err(|e| bad(format!("{e}: write {}('<key>')", endpoint_list::SET)))?;
    let ODataUri::Resource { path, .. } = parsed else {
        return Err(elsewhere());
    };
    match path.as_slice() {
        [Segment::EntitySet(set), Segment::Key(Key::Single(KeyValue::Literal(Literal::String(key))))]
            if set == endpoint_list::SET =>
        {
            Ok(key.clone())
        }
        [Segment::EntitySet(set), Segment::Key(_)] if set == endpoint_list::SET => {
            Err(bad(format!(
                "a {} key is a text, e.g. ('air-counters')",
                endpoint_list::SET
            )))
        }
        _ => Err(elsewhere()),
    }
}

/// The caller's rows of the list and the key the write names, or the answer that refuses it.
async fn keyed(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    uri: &axum::http::Uri,
    op: &str,
) -> Result<String, Box<Response>> {
    readable_project(state, user, project).map_err(|e| Box::new(e.into_response()))?;
    let key = row_key(uri, project).map_err(|r| Box::new(r.into_response()))?;
    let rows = endpoint_list::rows(state, &user.0.identity, project)
        .await
        .map_err(|e| Box::new(e.into_response()))?;
    let decision = endpoint_list::decide(state, &user.0.identity, project, &rows, op, Some(&key));
    if decision.allowed {
        return Ok(key);
    }
    let reason = decision.reason.unwrap_or_default();
    // A row the caller cannot read stays the OData 404; a refused write is the 403 problem every
    // write door answers (its CSRF guard too, PF-50), the reason in its detail.
    Err(Box::new(if decision.not_found {
        not_found(reason).into_response()
    } else {
        ApiError::Denied(reason).into_response()
    }))
}

/// The query of `endpointList/$allowed`.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct AllowedQuery {
    /// read, export, create, update or delete, e.g. update
    pub op: String,
    /// The row's key, e.g. air-counters, or helsinki/bikes for a shared row
    pub key: Option<String>,
}

/// `GET /api/v1/projects/{project}/odata/{resource}/$allowed` (T-6645).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/odata/{resource}/$allowed",
    summary = "Ask Whether an Endpoint List Call Is Allowed",
    description = "Whether the caller may read, export, create, update or delete the entity set endpointList or one row of it, and why not, from the same decision the call runs: update is the propose check of the Endpoint, delete its delete check; a shared or gone row is changed elsewhere and says where. Only endpointList answers it (T-6645).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name, e.g. helsinki"),
        ("resource" = String, Path, description = "The entity set, endpointList"),
        AllowedQuery,
    ),
    responses(
        (status = 200, description = "{allowed, reason, notFound}: notFound for a key the caller does not read", body = Object),
        (status = 400, description = "op missing or an unknown query parameter", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "A project the caller may not read or where it reads no Endpoint, or another entity set", body = ProblemDetails),
    )
)]
pub async fn allowed(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, resource)): Path<(String, String)>,
    Query(query): Query<AllowedQuery>,
) -> Result<Response, ApiError> {
    readable_project(&state, &user, &project)?;
    if resource != endpoint_list::SET {
        return Err(ApiError::NotFound(format!(
            "{resource} answers no $allowed: only {} does",
            endpoint_list::SET
        )));
    }
    let identity = &user.0.identity;
    let rows = endpoint_list::rows(&state, identity, &project).await?;
    let decision = endpoint_list::decide(
        &state,
        identity,
        &project,
        &rows,
        &query.op,
        query.key.as_deref(),
    );
    let body = json!({
        "allowed": decision.allowed,
        "reason": decision.reason,
        "notFound": decision.not_found,
    });
    Ok((odata_headers("application/json"), body.to_string()).into_response())
}

/// `PATCH /api/v1/projects/{project}/odata/{resource}` (T-6645).
#[utoipa::path(
    patch,
    path = "/api/v1/projects/{project}/odata/{resource}",
    summary = "Change One Endpoint with OData",
    description = "Proposes a change of contextSpace, audience or mcp of one of the project's own Endpoints, a row of endpointList by key: the same checked proposal the Endpoints page sends (a dry run that records the verdict, then the proposal through PATCH /projects/{project}/endpoints/{name}), so the change waits for approval like any other. A shared or gone row is refused 403 with where it is changed (T-6645).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name, e.g. helsinki"),
        ("resource" = String, Path, description = "A row of endpointList by key, e.g. endpointList('air-counters')"),
        ("If-Match" = Option<String>, Header, description = "The ETag of the manifest as read; a stale one is 412"),
    ),
    request_body(content = Object, content_type = "application/json", description = "The properties to change, e.g. {\"audience\": \"public\", \"mcp\": false}", example = json!({ "audience": "public", "mcp": false })),
    responses(
        (status = 202, description = "The Change that proposes it", body = Object),
        (status = 400, description = "A property the row does not change or a value of the wrong type, as an OData error"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not propose it, or the row is not this project's: the reason in the problem's detail", body = ProblemDetails),
        (status = 404, description = "No such row the caller may read, as an OData error"),
        (status = 405, description = "Not a row of endpointList, as an OData error"),
        (status = 422, description = "The checked manifest is refused: each finding named", body = ProblemDetails),
    )
)]
pub async fn change_row(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, _resource)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let key = match keyed(&state, &user, &project, &uri, "update").await {
        Ok(key) => key,
        Err(answer) => return Ok(*answer),
    };
    let patch = match endpoint_list::spec_patch(&body) {
        Ok(patch) => patch,
        Err(message) => return Ok(bad(message).into_response()),
    };
    let mut forwarded = HeaderMap::new();
    forwarded.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/merge-patch+json"),
    );
    if let Some(tag) = headers.get(header::IF_MATCH) {
        forwarded.insert(header::IF_MATCH, tag.clone());
    }
    let bytes = Bytes::from(patch.to_string());
    let door = |dry_run: Option<String>| {
        crate::api::mutate::patch(
            user.clone(),
            front,
            State(state.clone()),
            Path((project.clone(), "endpoints".to_owned(), key.clone())),
            Query(DryRunQuery {
                dry_run,
                ..DryRunQuery::default()
            }),
            forwarded.clone(),
            bytes.clone(),
        )
    };
    // The check first, as the page's form runs it: it records the verdict the proposal needs
    // (PF-57), and a red one is answered with its findings and proposes nothing.
    let checked = door(Some("All".to_owned())).await?;
    let verdict: Value = serde_json::from_slice(
        &axum::body::to_bytes(checked.into_body(), 1 << 20)
            .await
            .map_err(|e| ApiError::Internal(format!("the check's answer was not read: {e}")))?,
    )
    .map_err(|e| ApiError::Internal(format!("the check's answer is not JSON: {e}")))?;
    if verdict.get("valid") != Some(&Value::Bool(true)) {
        let errors: Vec<String> = verdict
            .pointer("/verdict/findings")
            .and_then(Value::as_array)
            .map(|findings| {
                findings
                    .iter()
                    .filter_map(|f| {
                        Some(format!(
                            "{}: {}",
                            f.get("path")?.as_str()?,
                            f.get("message")?.as_str()?
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Err(ApiError::Unprocessable {
            detail: format!("the change of Endpoint '{key}' is refused by its check"),
            errors,
        });
    }
    door(None).await
}

/// `DELETE /api/v1/projects/{project}/odata/{resource}` (T-6645).
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/odata/{resource}",
    summary = "Remove One Endpoint with OData",
    description = "Proposes removing one of the project's own Endpoints, a row of endpointList by key, through the same door as DELETE /projects/{project}/endpoints/{name}: refused 409 while other resources reference it; a shared or gone row is refused 403 with where it is removed (T-6645).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name, e.g. helsinki"),
        ("resource" = String, Path, description = "A row of endpointList by key, e.g. endpointList('air-counters')"),
        ("If-Match" = Option<String>, Header, description = "The ETag of the manifest as read; a stale one is 412"),
    ),
    responses(
        (status = 202, description = "The Change that proposes it", body = Object),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not delete it, or the row is not this project's: the reason in the problem's detail", body = ProblemDetails),
        (status = 404, description = "No such row the caller may read, as an OData error"),
        (status = 405, description = "Not a row of endpointList, as an OData error"),
        (status = 409, description = "Other resources reference it", body = ProblemDetails),
    )
)]
pub async fn remove_row(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, _resource)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let key = match keyed(&state, &user, &project, &uri, "delete").await {
        Ok(key) => key,
        Err(answer) => return Ok(*answer),
    };
    crate::api::delete::delete_resource(
        user,
        front,
        State(state),
        Path((project, "endpoints".to_owned(), key)),
        Query(DryRunQuery::default()),
        headers,
    )
    .await
}

/// `GET /api/v1/projects/{project}/odata/$metadata` (T-6632, T-6655).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/odata/$metadata",
    summary = "Read the OData Metadata",
    description = "The CSDL XML document of the project's OData service: an entity type and set per list operation, an action per writing operation (T-6655).",
    tag = "resources",
    params(("project" = String, Path, description = "Project name, e.g. helsinki")),
    responses(
        (status = 200, description = "CSDL XML, OData 4.01", content_type = "application/xml", body = String),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "A project the caller may not read", body = ProblemDetails),
    )
)]
pub async fn metadata(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Response, ApiError> {
    readable_project(&state, &user, &project)?;
    let xml = SERVED
        .metadata
        .clone()
        .map_err(|e| ApiError::Internal(format!("the OData metadata did not write: {e}")))?;
    Ok((odata_headers("application/xml"), xml).into_response())
}

/// `GET /api/v1/projects/{project}/odata/` (T-6632, T-6655).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/odata/",
    summary = "Read the OData Service Document",
    description = "The service document of the project's OData service: every entity set it serves (T-6655).",
    tag = "resources",
    params(("project" = String, Path, description = "Project name, e.g. helsinki")),
    responses(
        (status = 200, description = "The OData service document", body = Object),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "A project the caller may not read", body = ProblemDetails),
    )
)]
pub async fn service_document(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Response, ApiError> {
    readable_project(&state, &user, &project)?;
    let sets: Vec<Value> = SERVED
        .sets
        .iter()
        .map(|set| json!({ "name": set.set, "kind": "EntitySet", "url": set.set }))
        .collect();
    let body = json!({
        "@odata.context": format!("{}$metadata", service_root(&state, &project)),
        "value": sets,
    });
    Ok((
        odata_headers("application/json;odata.metadata=minimal"),
        body.to_string(),
    )
        .into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/odata/", get(service_document))
        .route("/projects/{project}/odata/$metadata", get(metadata))
        .route(
            "/projects/{project}/odata/{resource}",
            get(query_set)
                .post(invoke_action)
                .patch(change_row)
                .delete(remove_row),
        )
        .route(
            "/projects/{project}/odata/{resource}/$count",
            get(count_set),
        )
        .route(
            "/projects/{project}/odata/{resource}/$allowed",
            get(allowed),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A write names one row of endpointList by key, a shared row's `/` percent-encoded as the
    /// SDK grid sends it; any other resource is refused 405, a bad key 400 (T-6645).
    #[test]
    fn a_write_names_one_row_of_the_endpoint_list() {
        let key = |path: &str| {
            let uri: axum::http::Uri = format!("/api/v1/projects/p/odata/{path}")
                .parse()
                .unwrap_or_else(|e| panic!("{e}"));
            row_key(&uri, "p").map_err(|Refusal(status, ..)| status)
        };
        assert_eq!(key("endpointList('air')"), Ok("air".to_owned()));
        assert_eq!(
            key("endpointList('espoo%2Fair')"),
            Ok("espoo/air".to_owned())
        );
        assert_eq!(key("endpointList('it''s')"), Ok("it's".to_owned()));
        assert_eq!(key("endpoints('air')"), Err(StatusCode::METHOD_NOT_ALLOWED));
        assert_eq!(key("endpointList"), Err(StatusCode::METHOD_NOT_ALLOWED));
        assert_eq!(key("endpointList(1)"), Err(StatusCode::BAD_REQUEST));
        assert_eq!(key("nope('air')"), Err(StatusCode::BAD_REQUEST));
        let xml = SERVED.metadata.as_ref().expect("$metadata writes");
        assert!(xml.contains(r#"EntitySet Name="endpointList""#), "{xml}");
        assert!(xml.contains(r#"Property Name="calls7d""#), "{xml}");
    }

    /// Every mapped set names a distinct set and type, has its key among its properties, and
    /// `$metadata` writes; every action has a unique name (T-6655).
    #[test]
    fn the_service_is_one_consistent_model() {
        let served = &*SERVED;
        let mut sets: Vec<&str> = served.sets.iter().map(|s| s.set.as_str()).collect();
        let mut types: Vec<&str> = served.sets.iter().map(|s| s.type_name.as_str()).collect();
        let total = sets.len();
        sets.sort_unstable();
        sets.dedup();
        types.sort_unstable();
        types.dedup();
        assert_eq!(
            (sets.len(), types.len()),
            (total, total),
            "a set or type twice"
        );
        for def in &served.service.sets {
            assert!(
                def.fields.iter().any(|f| f.name == def.key && !f.nullable),
                "{} has no key property {}",
                def.set,
                def.key
            );
        }
        let xml = served.metadata.as_ref().expect("$metadata writes");
        for set in ["endpoints", "pipelines", "changes", "workspaces"] {
            assert!(xml.contains(&format!(r#"EntitySet Name="{set}""#)), "{set}");
        }
        assert!(
            xml.contains(r#"<Action Name="jc_resource_propose">"#),
            "{xml}"
        );
        assert!(xml.contains(&format!(
            r#"<ActionImport Name="jc_resource_propose" Action="{NAMESPACE}.jc_resource_propose"/>"#
        )));
        assert!(
            !xml.contains(r#"<Action Name="jc_resource_list">"#),
            "a read is no action"
        );
    }

    /// Only an operation's scalar members become properties: an object or array member (where
    /// a manifest's spec or a secretRef could sit) is no property, so `$filter` cannot name it.
    #[test]
    fn a_set_has_the_scalar_members_of_its_operation_alone() {
        let op = ops::find("jc_resource_list").expect("registered");
        let names: Vec<String> = fields_of(op, "name").into_iter().map(|f| f.name).collect();
        assert!(names.contains(&"name".to_owned()) && names.contains(&"space".to_owned()));
        assert!(
            !names.contains(&"types".to_owned()),
            "an array is no property"
        );
        assert!(!names.iter().any(|n| n == "spec" || n == "secretRef"));
    }

    #[test]
    fn a_change_reads_its_name_off_its_metadata() {
        let row = flat(json!({
            "kind": "Change",
            "metadata": { "name": "c-1", "namespace": "helsinki" },
            "status": { "lane": "green", "phase": "Open" },
            "url": "https://portal/changes/c-1"
        }));
        assert_eq!(row["name"], json!("c-1"));
        assert_eq!(row["lane"], json!("green"));
        assert_eq!(row["url"], json!("https://portal/changes/c-1"));
        assert!(row.get("metadata").is_none());
    }

    #[test]
    fn identifiers_are_odata_simple_identifiers() {
        assert!(identifier("jc_resource_propose") && identifier("dryRun"));
        assert!(!identifier("") && !identifier("1a") && !identifier("a-b") && !identifier("a\"b"));
    }
}
