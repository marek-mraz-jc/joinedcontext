// SPDX-License-Identifier: MIT
//! `table_router` serving a `MemoryTable` (T-6642): pages, count, key, refusals, scope.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use odata_axum::{table_router, ODataState};
use odata_csdl::{
    CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use odata_table::{
    Applying, Batch, Fetched, Fetching, MemoryTable, Operation, Plan, Rule, Scope, Table,
    TableError, TableInfo,
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn model() -> Arc<Model> {
    let mut thing = EntityType::new("Thing");
    thing.key = vec!["id".into()];
    // a key is never null (CSDL 4.01 8.2)
    let mut id = Property::new("id", TypeRef::single("Edm.String"));
    id.nullable = false;
    thing.properties = vec![
        id,
        Property::new("org", TypeRef::single("Edm.String")),
        Property::new("size", TypeRef::single("Edm.Int64")),
    ];
    let mut counter = EntityType::new("Counter");
    counter.key = vec!["n".into()];
    let mut n = Property::new("n", TypeRef::single("Edm.Int64"));
    n.nullable = false;
    counter.properties = vec![n, Property::new("label", TypeRef::single("Edm.String"))];
    let mut schema = Schema::new("Test");
    schema.entity_types.push(thing);
    schema.entity_types.push(counter);
    let mut container = EntityContainer::new("Service");
    for (set, ty) in [
        ("Things", "Test.Thing"),
        ("Broken", "Test.Thing"),
        ("Busy", "Test.Thing"),
        ("Unserved", "Test.Thing"),
        ("Counters", "Test.Counter"),
    ] {
        container.entity_sets.push(EntitySet::new(set, ty));
    }
    schema.entity_container = Some(container);
    Arc::new(Model {
        version: CsdlVersion::V4_01,
        references: vec![],
        schemas: vec![schema],
    })
}

/// A set whose source carries an entity tag, or fails (a source text that must stay private).
struct Tagged {
    inner: MemoryTable,
    fail: Option<TableError>,
}

impl Table for Tagged {
    fn info(&self) -> &TableInfo {
        self.inner.info()
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        if let Some(e) = &self.fail {
            return Box::pin(std::future::ready(Err(e.clone())));
        }
        self.inner.fetch(plan, scope)
    }

    fn etag(&self) -> Option<String> {
        Some("0123abcd".into())
    }
}

/// A writable set whose rows are gone after the first read: a row deleted between the access
/// decision and the call that follows it.
struct Vanishing {
    inner: MemoryTable,
    read: AtomicBool,
}

impl Table for Vanishing {
    fn info(&self) -> &TableInfo {
        self.inner.info()
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        if self.read.swap(true, Ordering::SeqCst) {
            return Box::pin(std::future::ready(Ok(Fetched {
                rows: vec![],
                count: Some(0),
            })));
        }
        self.inner.fetch(plan, scope)
    }

    fn writable(&self) -> bool {
        true
    }

    fn apply<'a>(&'a self, batch: &'a Batch) -> Applying<'a> {
        self.inner.apply(batch)
    }
}

/// A set that remembers how many rows the last read asked its source for.
struct Peeking {
    inner: MemoryTable,
    limit: AtomicU64,
}

impl Table for Peeking {
    fn info(&self) -> &TableInfo {
        self.inner.info()
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        self.limit.store(plan.limit, Ordering::SeqCst);
        self.inner.fetch(plan, scope)
    }
}

/// `Counters` with every operation open: a writable set keyed by an integer.
fn counters(model: &Arc<Model>) -> Result<MemoryTable, String> {
    let mut info = TableInfo::new(model.clone(), "Counters").ok_or("no set")?;
    info.rules = Operation::ALL
        .into_iter()
        .map(|operation| Rule {
            operation,
            permission: None,
        })
        .collect();
    MemoryTable::new(info, vec![json!({"n": 1, "label": "one"})]).map_err(|e| e.to_string())
}

fn state(model: Arc<Model>, url_model: Option<Arc<Model>>) -> ODataState {
    ODataState {
        model,
        url_model,
        service_root: "https://example.org/odata/".into(),
        url_limits: odata_url::Limits::strict(),
        csdl_limits: odata_csdl::Limits::strict(),
        max_header_len: 1024,
        metadata_etag: None,
        metadata: Arc::default(),
        custom_options: vec![],
    }
}

fn app(scope: Option<Scope>) -> Result<Router, String> {
    let model = model();
    let rows = vec![
        json!({"id": "t1", "org": "a", "size": 3}),
        json!({"id": "t2", "org": "b", "size": 1}),
        json!({"id": "it's", "org": "a", "size": 2}),
    ];
    let info = |set| TableInfo::new(model.clone(), set).ok_or("no set");
    let things = MemoryTable::new(info("Things")?, rows).map_err(|e| e.to_string())?;
    let broken = MemoryTable::new(info("Broken")?, vec![]).map_err(|e| e.to_string())?;
    let busy = MemoryTable::new(info("Busy")?, vec![]).map_err(|e| e.to_string())?;
    let tables: Vec<Arc<dyn Table>> = vec![
        Arc::new(Tagged {
            inner: things,
            fail: None,
        }),
        Arc::new(Tagged {
            inner: broken,
            fail: Some(TableError::Source("db.internal:5432 refused".into())),
        }),
        Arc::new(Tagged {
            inner: busy,
            fail: Some(TableError::Conflict("the source moved; retry".into())),
        }),
        Arc::new(counters(&model)?),
    ];
    let state = state(model, None);
    let router = Router::new().nest_service("/odata", table_router(Arc::new(state), tables, 2));
    Ok(match scope {
        Some(scope) => router.layer(Extension(scope)),
        None => router,
    })
}

async fn call(
    app: Router,
    method: &str,
    uri: &str,
) -> Result<(StatusCode, Option<String>, Value), String> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    send(app, request).await
}

async fn send(
    app: Router,
    request: Request<Body>,
) -> Result<(StatusCode, Option<String>, Value), String> {
    let Ok(response) = app.oneshot(request).await;
    let status = response.status();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    Ok((status, etag, body))
}

async fn get(uri: &str) -> Result<(StatusCode, Option<String>, Value), String> {
    call(app(Some(Scope::all()))?, "GET", uri).await
}

#[tokio::test]
async fn a_set_answers_pages_with_a_next_link_to_follow() -> Result<(), String> {
    let (status, etag, body) = get("/odata/Things?$orderby=size&$count=true").await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(etag.as_deref(), Some("\"0123abcd\""));
    assert_eq!(body["@odata.count"], 3);
    assert_eq!(
        body["value"],
        json!([{"id": "t2", "org": "b", "size": 1}, {"id": "it's", "org": "a", "size": 2}])
    );
    let next = body["@odata.nextLink"].as_str().ok_or("no nextLink")?;
    let relative = next
        .strip_prefix("https://example.org")
        .ok_or(next.to_owned())?;
    let (status, _, body) = get(relative).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["value"], json!([{"id": "t1", "org": "a", "size": 3}]));
    assert!(body.get("@odata.nextLink").is_none(), "{body}");
    Ok(())
}

#[tokio::test]
async fn count_key_and_select_resolve() -> Result<(), String> {
    let (status, _, body) = get("/odata/Things/$count?$filter=org%20eq%20'a'").await?;
    assert_eq!((status, body), (StatusCode::OK, json!(2)));
    // the count keeps the filter's aliases and its $search (which this service refuses)
    let (status, _, body) = get("/odata/Things/$count?$filter=org%20eq%20@o&@o='a'").await?;
    assert_eq!((status, body), (StatusCode::OK, json!(2)));
    let (status, _, body) = get("/odata/Things/$count?$search=a").await?;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    // `$count` asks the source for the total only, no page of rows
    let model = model();
    let info = TableInfo::new(model.clone(), "Things").ok_or("no set")?;
    let rows = vec![json!({"id": "t1"}), json!({"id": "t2"})];
    let peeking = Arc::new(Peeking {
        inner: MemoryTable::new(info, rows).map_err(|e| e.to_string())?,
        limit: AtomicU64::new(u64::MAX),
    });
    let tables: Vec<Arc<dyn Table>> = vec![peeking.clone()];
    let router = Router::new()
        .nest_service(
            "/odata",
            table_router(Arc::new(state(model, None)), tables, 2),
        )
        .layer(Extension(Scope::all()));
    let (status, _, body) = call(router, "GET", "/odata/Things/$count").await?;
    assert_eq!((status, body), (StatusCode::OK, json!(2)));
    assert_eq!(peeking.limit.load(Ordering::SeqCst), 0);
    let (status, _, body) =
        get("/odata/Things('it''s')?$format=application/json;odata.metadata=full").await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["@odata.id"], "Things('it''s')", "{body}");
    let (status, _, body) = get("/odata/Things('it''s')?$select=size").await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["size"], 2);
    assert!(body.get("org").is_none(), "{body}");
    let (status, _, _) = get("/odata/Things('nope')").await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn refusals_are_odata_errors() -> Result<(), String> {
    for (uri, status, code) in [
        (
            "/odata/Things?$filter=length(id)%20eq%202",
            StatusCode::NOT_IMPLEMENTED,
            "NotImplemented",
        ),
        (
            "/odata/Things?$filter=nope%20eq%201",
            StatusCode::BAD_REQUEST,
            "BadRequest",
        ),
        (
            "/odata/Things?$skiptoken=x",
            StatusCode::BAD_REQUEST,
            "BadRequest",
        ),
        ("/odata/Nope", StatusCode::NOT_FOUND, "NotFound"),
        (
            "/odata/Broken",
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
        ),
    ] {
        let (got, _, body) = get(uri).await?;
        assert_eq!(got, status, "{uri}: {body}");
        assert_eq!(body["error"]["code"], code, "{uri}: {body}");
        assert!(
            !body.to_string().contains("db.internal"),
            "{uri}: the source's text leaked"
        );
    }
    let (status, _, _) = call(app(Some(Scope::all()))?, "POST", "/odata/Things").await?;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    Ok(())
}

#[tokio::test]
async fn a_source_conflict_is_409_and_a_set_the_model_names_but_nobody_serves_is_404(
) -> Result<(), String> {
    let (status, _, body) = get("/odata/Busy").await?;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["message"], "the source moved; retry");
    for uri in [
        "/odata/Unserved",
        "/odata/Unserved/$export",
        "/odata/Unserved/$allowed?op=read",
    ] {
        let (status, _, body) = get(uri).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
    let (status, _, body) = call(
        app(Some(Scope::all()))?,
        "DELETE",
        "/odata/Unserved?$filter=org%20eq%20'a'",
    )
    .await?;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::NOT_FOUND, json!("NotFound"))
    );
    Ok(())
}

#[tokio::test]
async fn a_resource_the_table_router_does_not_serve_is_an_odata_error() -> Result<(), String> {
    let unsupported = |what: &str| format!("{what} is not supported by this service");
    for (method, uri, status, message) in [
        (
            "GET",
            "/odata/$entity?$id=Things('t1')",
            StatusCode::NOT_IMPLEMENTED,
            unsupported("this resource"),
        ),
        (
            "GET",
            "/odata/Things('t1')/org",
            StatusCode::NOT_IMPLEMENTED,
            unsupported("this resource path"),
        ),
        (
            "GET",
            "/odata/Counters(1.5)",
            StatusCode::BAD_REQUEST,
            "a key is a string or an integer".into(),
        ),
        (
            "DELETE",
            "/odata/$entity?$id=Things('t1')",
            StatusCode::NOT_IMPLEMENTED,
            unsupported("this resource"),
        ),
        (
            "GET",
            "/odata/$crossjoin(Things,Counters)",
            StatusCode::NOT_IMPLEMENTED,
            "$crossjoin and $all: no NGSI-LD equivalent (ADR-0024 decision 5)".into(),
        ),
    ] {
        let (got, _, body) = call(app(Some(Scope::all()))?, method, uri).await?;
        assert_eq!(got, status, "{method} {uri}: {body}");
        assert_eq!(body["error"]["message"], message.as_str(), "{method} {uri}");
    }
    Ok(())
}

#[tokio::test]
async fn a_bad_accept_or_version_header_is_refused_before_any_row() -> Result<(), String> {
    for (uri, header, value) in [
        (
            "/odata/Things",
            "accept",
            "application/json;odata.metadata=lots",
        ),
        (
            "/odata/Things('t1')",
            "accept",
            "application/json;odata.metadata=lots",
        ),
        ("/odata/Things", "odata-maxversion", "3.0"),
    ] {
        let request = Request::builder()
            .uri(uri)
            .header(header, value)
            .body(Body::empty())
            .map_err(|e| e.to_string())?;
        let (status, _, body) = send(app(Some(Scope::all()))?, request).await?;
        assert!(status.is_client_error(), "{uri} {header}: {status} {body}");
        assert!(body["error"]["code"].is_string(), "{uri} {header}: {body}");
    }
    let request = Request::builder()
        .uri("/odata/Things")
        .header(
            "odata-version",
            axum::http::HeaderValue::from_bytes(b"4.0\xff").map_err(|e| e.to_string())?,
        )
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    let (status, _, body) = send(app(Some(Scope::all()))?, request).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["message"], "a request header is not valid");
    Ok(())
}

#[tokio::test]
async fn a_header_of_exactly_the_limit_is_read_and_one_byte_more_is_refused() -> Result<(), String>
{
    let model = model();
    let things = MemoryTable::new(
        TableInfo::new(model.clone(), "Things").ok_or("no set")?,
        vec![json!({"id": "t1"})],
    )
    .map_err(|e| e.to_string())?;
    let mut state = state(model, None);
    state.max_header_len = 3;
    let tables: Vec<Arc<dyn Table>> = vec![Arc::new(things)];
    let router = Router::new()
        .nest_service("/odata", table_router(Arc::new(state), tables, 2))
        .layer(Extension(Scope::all()));
    for (version, status) in [("4.0", StatusCode::OK), ("4.01", StatusCode::BAD_REQUEST)] {
        let request = Request::builder()
            .uri("/odata/Things")
            .header("odata-maxversion", version)
            .body(Body::empty())
            .map_err(|e| e.to_string())?;
        let (got, _, body) = send(router.clone(), request).await?;
        assert_eq!(got, status, "{version}: {body}");
    }
    Ok(())
}

#[tokio::test]
async fn a_row_gone_between_the_decision_and_the_call_is_not_found() -> Result<(), String> {
    let model = model();
    for method in ["GET", "DELETE"] {
        let vanishing = Vanishing {
            inner: counters(&model)?,
            read: AtomicBool::new(false),
        };
        let tables: Vec<Arc<dyn Table>> = vec![Arc::new(vanishing)];
        let router = Router::new()
            .nest_service(
                "/odata",
                table_router(Arc::new(state(model.clone(), None)), tables, 2),
            )
            .layer(Extension(Scope::all()));
        let (status, _, body) = call(router, method, "/odata/Counters(1)").await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}: {body}");
        assert_eq!(body["error"]["message"], "the resource does not exist");
    }
    Ok(())
}

#[tokio::test]
async fn an_integer_key_reads_and_answers_its_decision() -> Result<(), String> {
    let (status, _, body) = get("/odata/Counters(1)").await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["label"], "one");
    let (status, _, body) = get("/odata/Counters").await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"], json!([{"n": 1, "label": "one"}]));
    let (status, _, body) = get("/odata/Counters/$allowed?op=delete&key=1").await?;
    assert_eq!(
        (status, body["allowed"].clone()),
        (StatusCode::OK, json!(true)),
        "{body}"
    );
    for (uri, message) in [
        (
            "/odata/Counters/$allowed?op=read&key=one",
            "key is an integer here",
        ),
        (
            "/odata/Things/$allowed?op=read&key=%FF",
            "a parameter is not valid UTF-8",
        ),
        (
            "/odata/Things/$allowed?op=fly",
            "op is one of read, export, create, update, delete",
        ),
        (
            "/odata/Things/$allowed?key=t1",
            "op is required, e.g. op=update",
        ),
        (
            "/odata/Things/$allowed?op=read&who=me",
            "$allowed takes op and key only",
        ),
    ] {
        let (status, _, body) = get(uri).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert_eq!(body["error"]["message"], message, "{uri}");
    }
    Ok(())
}

#[tokio::test]
async fn requests_outside_the_url_grammar_need_a_scope_and_a_valid_query() -> Result<(), String> {
    for (method, uri) in [
        ("GET", "/odata/Things/$export"),
        ("GET", "/odata/Things/$allowed?op=read"),
        ("PATCH", "/odata/Counters(1)"),
    ] {
        let (status, _, body) = call(app(None)?, method, uri).await?;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
        assert_eq!(
            body["error"]["message"],
            "this request carries no read scope"
        );
    }
    let (status, _, body) = get("/odata/Things/$export?$filter=nope%20eq%201").await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    Ok(())
}

#[tokio::test]
async fn a_write_names_one_entity_or_a_filter_and_carries_a_bounded_object() -> Result<(), String> {
    let all = || app(Some(Scope::all()));
    for (method, uri, status, message) in [
        (
            "DELETE",
            "/odata/Counters(1)?dryRun=maybe",
            StatusCode::BAD_REQUEST,
            "dryRun is true or false",
        ),
        (
            "DELETE",
            "/odata/Counters(1)?$filter=n%20eq%201",
            StatusCode::BAD_REQUEST,
            "a write to one entity takes no query option but dryRun",
        ),
        (
            "DELETE",
            "/odata/Counters(1)/label",
            StatusCode::NOT_IMPLEMENTED,
            "this resource path is not supported by this service",
        ),
        (
            "DELETE",
            "/odata/Things('t1')",
            StatusCode::FORBIDDEN,
            "Things does not offer delete",
        ),
    ] {
        let (got, _, body) = call(all()?, method, uri).await?;
        assert_eq!(got, status, "{method} {uri}: {body}");
        assert_eq!(body["error"]["message"], message, "{method} {uri}");
    }
    let big = format!("{{\"label\": \"{}\"}}", "x".repeat(1 << 20));
    let request = Request::builder()
        .method("PATCH")
        .uri("/odata/Counters(1)")
        .header("content-type", "application/json")
        .body(Body::from(big))
        .map_err(|e| e.to_string())?;
    let (status, _, body) = send(all()?, request).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"]["message"],
        "the body is over 1 MiB or was cut off"
    );
    let (status, _, body) = call(all()?, "DELETE", "/odata/Counters(1)?dryRun=false").await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["dryRun"].clone(), body["keys"].clone()),
        (json!(false), json!([1]))
    );
    Ok(())
}

#[tokio::test]
async fn a_set_the_url_model_has_and_the_answer_model_lacks_is_404() -> Result<(), String> {
    let mut wider = (*model()).clone();
    if let Some(container) = wider.schemas[0].entity_container.as_mut() {
        container
            .entity_sets
            .push(EntitySet::new("Ghosts", "Test.Thing"));
    }
    let wider = Arc::new(wider);
    let ghosts = MemoryTable::new(
        TableInfo::new(wider.clone(), "Ghosts").ok_or("no set")?,
        vec![json!({"id": "g1"})],
    )
    .map_err(|e| e.to_string())?;
    let tables: Vec<Arc<dyn Table>> = vec![Arc::new(ghosts)];
    let router = Router::new()
        .nest_service(
            "/odata",
            table_router(Arc::new(state(model(), Some(wider))), tables, 2),
        )
        .layer(Extension(Scope::all()));
    let (status, _, body) = call(router, "GET", "/odata/Ghosts").await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["message"], "the resource does not exist");
    Ok(())
}

#[tokio::test]
async fn a_request_reads_only_its_scope_and_none_without_one() -> Result<(), String> {
    let (status, _, body) = call(app(None)?, "GET", "/odata/Things").await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let scoped = app(Some(Scope::all().and(&["org"], vec![json!("b")])))?;
    let (status, _, body) = call(
        scoped.clone(),
        "GET",
        "/odata/Things?$filter=org%20eq%20'a'%20or%20true&$count=true",
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (body["@odata.count"].clone(), body["value"][0]["id"].clone()),
        (json!(1), json!("t2"))
    );
    let (status, _, _) = call(scoped.clone(), "GET", "/odata/Things('t1')").await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a key outside the scope is not found"
    );
    let (_, _, body) = call(scoped, "GET", "/odata/Things/$count").await?;
    assert_eq!(body, json!(1));
    Ok(())
}
