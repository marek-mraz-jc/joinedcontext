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
use odata_table::{Fetching, MemoryTable, Plan, Scope, Table, TableError, TableInfo};
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
    let mut schema = Schema::new("Test");
    schema.entity_types.push(thing);
    let mut container = EntityContainer::new("Service");
    container
        .entity_sets
        .push(EntitySet::new("Things", "Test.Thing"));
    container
        .entity_sets
        .push(EntitySet::new("Broken", "Test.Thing"));
    schema.entity_container = Some(container);
    Arc::new(Model {
        version: CsdlVersion::V4_01,
        references: vec![],
        schemas: vec![schema],
    })
}

/// A set whose source carries an entity tag, or fails with a text that must stay private.
struct Tagged {
    inner: MemoryTable,
    fail: bool,
}

impl Table for Tagged {
    fn info(&self) -> &TableInfo {
        self.inner.info()
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        if self.fail {
            return Box::pin(std::future::ready(Err(TableError::Source(
                "db.internal:5432 refused".into(),
            ))));
        }
        self.inner.fetch(plan, scope)
    }

    fn etag(&self) -> Option<&str> {
        Some("0123abcd")
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
    let state = ODataState {
        model,
        url_model: None,
        service_root: "https://example.org/odata/".into(),
        url_limits: odata_url::Limits::strict(),
        csdl_limits: odata_csdl::Limits::strict(),
        max_header_len: 1024,
        metadata_etag: None,
        metadata: Arc::default(),
        custom_options: vec![],
    };
    let tables: Vec<Arc<dyn Table>> = vec![
        Arc::new(Tagged {
            inner: things,
            fail: false,
        }),
        Arc::new(Tagged {
            inner: broken,
            fail: true,
        }),
    ];
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
    let response = app.oneshot(request).await.map_err(|e| e.to_string())?;
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
