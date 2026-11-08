#![forbid(unsafe_code)]
//! Host routing end to end: the catalogue a request's Host names picks the tenant, and every
//! surface (CKAN Action API, OData, DCAT feeds, Atom, UI, MCP) answers from that tenant only,
//! with internal records hidden from callers who may not read them. Run on the memory store and
//! on the Postgres store reached through the request role, where forced RLS binds the tenant the
//! host chose (C1, C5, C6, contract "Security").

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use dataroom_api::{router, DataroomState, TokenBinding};
use dataroom_core::{
    Action, CatalogueId, Dataset, GrantRule, OrgId, PolicyAuthorizer, Publisher, RequestCtx, Store,
    Subject,
};
use dataroom_store_memory::MemoryStore;
use dataroom_store_pg::PgStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tower::ServiceExt;
use uuid::Uuid;

const APP_ROLE: &str = "dataroom_app_test";
static MIGRATED: OnceCell<()> = OnceCell::const_new();

/// The Postgres store on the request role, or `None` (printing `SKIP`) without a test database.
async fn pg_store(test: &str) -> Option<PgStore> {
    let Some(url) = std::env::var("DATAROOM_TEST_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    else {
        assert!(
            std::env::var("DATAROOM_REQUIRE_PG").ok().as_deref() != Some("1"),
            "{test}: DATAROOM_REQUIRE_PG=1 but DATAROOM_TEST_DATABASE_URL is unset"
        );
        println!("SKIP {test}: DATAROOM_TEST_DATABASE_URL is unset");
        return None;
    };
    MIGRATED
        .get_or_init(|| async {
            let admin = sqlx::PgPool::connect(&url).await.expect("connect admin");
            sqlx::query("SELECT pg_advisory_lock(hashtext('dataroom-test-setup'))")
                .execute(&admin)
                .await
                .expect("setup lock");
            PgStore::from_pool_unchecked(admin.clone())
                .migrate()
                .await
                .expect("migrate");
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$ BEGIN
                     IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{APP_ROLE}') THEN
                         CREATE ROLE {APP_ROLE} NOLOGIN NOSUPERUSER NOBYPASSRLS;
                     END IF;
                 END $$;
                 GRANT USAGE ON SCHEMA dataroom TO {APP_ROLE};
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO {APP_ROLE};"
            )))
            .execute(&admin)
            .await
            .expect("request role");
            sqlx::query("SELECT pg_advisory_unlock(hashtext('dataroom-test-setup'))")
                .execute(&admin)
                .await
                .expect("setup unlock");
            admin.close().await;
        })
        .await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(format!("SET ROLE {APP_ROLE}")))
                    .execute(&mut *conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(&url)
        .await
        .expect("connect as the request role");
    Some(
        PgStore::connect_and_verify(pool)
            .await
            .expect("request role passes the boot check"),
    )
}

struct Tenant {
    org: OrgId,
    cat: CatalogueId,
    host: String,
    tag: &'static str,
}

fn catalogue(t: &Tenant) -> dataroom_core::Catalogue {
    serde_json::from_value(json!({
        "orgId": t.org, "id": t.cat, "name": t.tag, "host": t.host,
        "title": {"sk": format!("Katalóg {}", t.tag)}, "description": {},
        "defaultLanguage": "sk", "languages": ["sk", "en"],
        "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap()
}

fn publisher(name: &str) -> Publisher {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"sk": name}, "description": {}, "uri": format!("https://example.org/{name}"),
        "state": "active", "editorGrants": [],
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap()
}

fn dataset(name: &str, publisher: &str, visibility: &str) -> Dataset {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"sk": format!("Dataset {name}")}, "description": {"sk": format!("Popis {name}")},
        "publisherId": publisher, "visibility": visibility, "state": "active",
        "keywords": {"sk": ["ovzdušie"]}, "tags": ["ovzdusie"],
        "distributions": [{
            "id": "", "datasetId": "", "title": {"sk": format!("CSV {name}")},
            "accessUrl": format!("https://files.example.org/{name}.csv"), "format": "CSV"
        }],
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    }))
    .unwrap()
}

/// Two tenants on two hosts, each with a public and an internal dataset, a harvest source, an
/// activity row and a SHACL report; alice may administer A, bob B.
async fn seed(store: &dyn Store) -> (Tenant, Tenant) {
    let a = Tenant {
        org: OrgId::from_uuid(Uuid::now_v7()),
        cat: CatalogueId::from_uuid(Uuid::now_v7()),
        host: format!("a-{}.example.org", Uuid::now_v7().simple()),
        tag: "ta",
    };
    let b = Tenant {
        org: OrgId::from_uuid(Uuid::now_v7()),
        cat: CatalogueId::from_uuid(Uuid::now_v7()),
        host: format!("b-{}.example.org", Uuid::now_v7().simple()),
        tag: "tb",
    };
    for t in [&a, &b] {
        store.upsert_catalogue(catalogue(t)).await.unwrap();
        let ctx = RequestCtx::new(
            t.org,
            t.cat,
            Subject::authenticated("seed", vec![t.org], Vec::new()),
        );
        store
            .upsert_publisher(&ctx, publisher("city"))
            .await
            .unwrap();
        let tag = t.tag;
        store
            .upsert_dataset(&ctx, dataset(&format!("open-{tag}"), "city", "public"))
            .await
            .unwrap();
        store
            .upsert_dataset(&ctx, dataset(&format!("secret-{tag}"), "city", "internal"))
            .await
            .unwrap();
        store
            .upsert_harvest_source(
                &ctx,
                serde_json::from_value(json!({
                    "orgId": t.org, "catalogueId": t.cat, "id": "", "name": format!("src-{tag}"),
                    "kind": "dcat-ap", "url": "https://harvest.example.org/feed.ttl",
                    "publisherId": "city", "allowedHosts": ["harvest.example.org"],
                    "modified": "2026-11-01T10:00:00Z"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .record_activity(
                &ctx,
                serde_json::from_value(json!({
                    "orgId": t.org, "catalogueId": t.cat, "id": format!("act-{tag}"),
                    "at": "2026-11-01T10:00:00Z", "actor": "seed", "operation": "dataset.propose",
                    "objectType": "dataset", "objectId": format!("pkg-secret-{tag}"),
                    "summary": format!("created secret-{tag}")
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .upsert_shacl_report(
                &ctx,
                serde_json::from_value(json!({
                    "orgId": t.org, "catalogueId": t.cat, "id": "",
                    "datasetId": format!("pkg-secret-{tag}"), "profileId": "dcat-ap-3.0.1",
                    "profileVersion": "3.0.1", "conforms": false, "violationCount": 1,
                    "warningCount": 0, "findings": [], "checkedAt": "2026-11-01T10:00:00Z"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
    }
    (a, b)
}

fn app(store: Arc<dyn Store>, a: &Tenant, b: &Tenant) -> axum::Router {
    let rule = |t: &Tenant, who: &str| GrantRule {
        org_id: t.org,
        catalogue_id: Some(t.cat),
        principal: who.into(),
        actions: vec![Action::Admin],
        publisher_id: None,
    };
    let authz = PolicyAuthorizer::new(vec![rule(a, "alice"), rule(b, "bob")]);
    let state = DataroomState::new(store, Arc::new(authz), a.org, a.cat).with_tokens(vec![
        TokenBinding {
            token: "tok-alice".into(),
            subject: Subject::authenticated("alice", vec![a.org], Vec::new()),
        },
        TokenBinding {
            token: "tok-bob".into(),
            subject: Subject::authenticated("bob", vec![b.org], Vec::new()),
        },
    ]);
    router(state)
}

async fn call(
    app: &axum::Router,
    method: Method,
    uri: &str,
    host: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, host);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("json body ({e}): {body}"))
}

fn odata_ids(body: &str) -> Vec<String> {
    let mut ids: Vec<String> = json_of(body)["value"]
        .as_array()
        .unwrap_or_else(|| panic!("odata value: {body}"))
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_default().to_owned())
        .collect();
    ids.sort();
    ids
}

async fn scenario(store: Arc<dyn Store>) {
    let (a, b) = seed(store.as_ref()).await;
    let app = app(store.clone(), &a, &b);
    let get = |uri: String, host: String, token: Option<&'static str>| {
        let app = app.clone();
        async move { call(&app, Method::GET, &uri, &host, token, None).await }
    };

    // CKAN: anonymous on A's host sees A's public dataset only.
    let (s, body) = get("/api/3/action/package_list".into(), a.host.clone(), None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(json_of(&body)["result"], json!(["open-ta"]));
    let (_, body) = get("/api/3/action/package_list".into(), b.host.clone(), None).await;
    assert_eq!(json_of(&body)["result"], json!(["open-tb"]));

    // B's datasets by name or id through A's host: not found, for everyone.
    for (who, name) in [
        (None, "open-tb"),
        (None, "pkg-open-tb"),
        (Some("tok-alice"), "secret-tb"),
        (Some("tok-bob"), "secret-tb"),
        (Some("tok-bob"), "open-tb"),
    ] {
        let (s, body) = get(
            format!("/api/3/action/package_show?id={name}"),
            a.host.clone(),
            who,
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{who:?} {name} on A: {body}");
    }
    // A's internal dataset: hidden from anonymous callers and from B's member, shown to alice.
    for who in [None, Some("tok-bob")] {
        let (s, _) = get(
            "/api/3/action/package_show?id=secret-ta".into(),
            a.host.clone(),
            who,
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{who:?}");
    }
    let (s, body) = get(
        "/api/3/action/package_show?id=secret-ta".into(),
        a.host.clone(),
        Some("tok-alice"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (_, body) = get(
        "/api/3/action/package_search?q=Dataset&rows=50".into(),
        a.host.clone(),
        Some("tok-alice"),
    )
    .await;
    let found: Vec<String> = json_of(&body)["result"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(found.contains(&"secret-ta".to_owned()), "{found:?}");
    assert!(!found.iter().any(|n| n.ends_with("-tb")), "{found:?}");
    let (_, body) = get(
        "/api/3/action/package_search?q=Dataset&rows=50".into(),
        a.host.clone(),
        None,
    )
    .await;
    assert_eq!(json_of(&body)["result"]["count"], 1, "{body}");

    // alice's token on B's host writes nothing into B.
    let (s, body) = call(
        &app,
        Method::POST,
        "/api/3/action/package_create",
        &b.host,
        Some("tok-alice"),
        Some(json!({"name": "planted", "title": "Planted", "owner_org": "city"})),
    )
    .await;
    assert!(
        s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND,
        "alice creating in B: {s} {body}"
    );
    let ctx_b = RequestCtx::anonymous(b.org, b.cat);
    assert!(store
        .get_dataset(&ctx_b, "planted")
        .await
        .unwrap()
        .is_none());
    let (s, body) = call(
        &app,
        Method::POST,
        "/api/3/action/package_delete",
        &b.host,
        Some("tok-alice"),
        Some(json!({"id": "open-tb"})),
    )
    .await;
    assert!(
        s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND,
        "alice deleting in B: {s} {body}"
    );
    assert_eq!(
        store
            .get_dataset(&ctx_b, "open-tb")
            .await
            .unwrap()
            .unwrap()
            .state,
        dataroom_core::RecordState::Active
    );
    // ... while bob may write on his own host.
    let (s, body) = call(
        &app,
        Method::POST,
        "/api/3/action/package_create",
        &b.host,
        Some("tok-bob"),
        Some(json!({"name": "bobs-new", "title": "Bob's", "owner_org": "city", "private": true})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(store
        .get_dataset(&ctx_b, "bobs-new")
        .await
        .unwrap()
        .is_some());

    // OData: the host's tenant only; internal rows only for readers; members-only sets.
    let (s, body) = get("/odata/v4/Datasets".into(), a.host.clone(), None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(odata_ids(&body), ["pkg-open-ta"]);
    let (_, body) = get(
        "/odata/v4/Datasets".into(),
        a.host.clone(),
        Some("tok-alice"),
    )
    .await;
    assert_eq!(odata_ids(&body), ["pkg-open-ta", "pkg-secret-ta"]);
    let (_, body) = get("/odata/v4/Datasets".into(), a.host.clone(), Some("tok-bob")).await;
    assert_eq!(odata_ids(&body), ["pkg-open-ta"], "bob is no member of A");
    let (_, body) = get("/odata/v4/Datasets/$count".into(), a.host.clone(), None).await;
    assert_eq!(body, "1");
    let (s, _) = get(
        "/odata/v4/Datasets('pkg-secret-ta')".into(),
        a.host.clone(),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = get(
        "/odata/v4/Datasets('pkg-open-tb')".into(),
        a.host.clone(),
        Some("tok-bob"),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "B's row is not reachable on A's host"
    );
    let (_, body) = get("/odata/v4/Distributions".into(), a.host.clone(), None).await;
    assert_eq!(
        odata_ids(&body),
        ["res-open-ta-0"],
        "an internal dataset's distributions stay hidden"
    );
    let (_, body) = get(
        "/odata/v4/Distributions".into(),
        a.host.clone(),
        Some("tok-alice"),
    )
    .await;
    assert_eq!(odata_ids(&body), ["res-open-ta-0", "res-secret-ta-0"]);
    for set in ["HarvestSources", "ActivityRows", "ShaclResults"] {
        let (_, body) = get(format!("/odata/v4/{set}"), a.host.clone(), None).await;
        assert!(odata_ids(&body).is_empty(), "anonymous {set}: {body}");
        let (_, body) = get(format!("/odata/v4/{set}"), a.host.clone(), Some("tok-bob")).await;
        assert!(odata_ids(&body).is_empty(), "bob {set} on A: {body}");
        let (_, body) = get(
            format!("/odata/v4/{set}"),
            a.host.clone(),
            Some("tok-alice"),
        )
        .await;
        assert_eq!(odata_ids(&body).len(), 1, "alice {set}: {body}");
    }
    let (_, body) = get("/odata/v4/Catalogues".into(), a.host.clone(), None).await;
    assert_eq!(odata_ids(&body), [a.cat.to_string()]);
    let (_, body) = get("/odata/v4/Publishers".into(), b.host.clone(), None).await;
    let pubs = json_of(&body);
    assert_eq!(pubs["value"][0]["org_id"], b.org.to_string());

    // DCAT and Atom feeds: public records of the host's catalogue only.
    for feed in [
        "/catalog.ttl",
        "/catalog.jsonld",
        "/catalog.rdf",
        "/catalog.nt",
        "/feeds/dataset.atom",
    ] {
        let (s, body) = get(feed.into(), a.host.clone(), None).await;
        assert_eq!(s, StatusCode::OK, "{feed}");
        assert!(body.contains("open-ta"), "{feed} lists A's public dataset");
        assert!(
            !body.contains("secret-ta"),
            "{feed} hides A's internal dataset"
        );
        assert!(!body.contains("-tb"), "{feed} holds nothing of B");
    }
    let (_, body) = get("/publisher/city/catalog.ttl".into(), b.host.clone(), None).await;
    assert!(body.contains("open-tb") && !body.contains("-ta"));

    // UI pages and sitemap.
    let (s, body) = get("/dataset/secret-ta".into(), a.host.clone(), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    let (s, _) = get("/dataset/open-tb".into(), a.host.clone(), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, body) = get("/dataset/open-ta".into(), a.host.clone(), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("Dataset open-ta"));
    let (_, body) = get("/sitemap.xml".into(), a.host.clone(), None).await;
    assert!(body.contains("open-ta") && !body.contains("secret-ta") && !body.contains("-tb"));

    // MCP tools answer from the host's tenant.
    let (s, body) = call(
        &app,
        Method::POST,
        "/mcp",
        &a.host,
        None,
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "jc_dataset_search", "arguments": {"q": "Dataset"}}})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body.contains("open-ta"), "{body}");
    assert!(
        !body.contains("secret-ta") && !body.contains("-tb"),
        "{body}"
    );

    // An unknown host reaches no catalogue at all.
    let (s, _) = get(
        "/api/3/action/package_list".into(),
        "nobody.example.org".into(),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn host_routing_isolates_tenants_on_the_memory_store() {
    scenario(Arc::new(MemoryStore::new())).await;
}

#[tokio::test]
async fn host_routing_isolates_tenants_on_postgres_through_rls() {
    let Some(store) = pg_store("host_routing_isolates_tenants_on_postgres_through_rls").await
    else {
        return;
    };
    scenario(Arc::new(store)).await;
}
