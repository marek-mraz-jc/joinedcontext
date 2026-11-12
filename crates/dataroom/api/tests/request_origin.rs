#![forbid(unsafe_code)]
//! Behind a host router (`jc`), a request's catalogue and links come from the router's
//! `jc_router::Mount` (the host a trusted proxy forwarded, its scheme, the entry's prefix), never
//! from the raw `Host` header; a loopback name reads the default catalogue only when the
//! installation serves one catalogue. The Portal's publisher grant of one catalogue writes no
//! other.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use dataroom_api::{portal_publisher, portal_publisher_on, router, DataroomState};
use dataroom_core::{
    Action, Authorizer, CatalogueId, ObjectTarget, OrgId, PolicyAuthorizer, RequestCtx, Store,
};
use dataroom_store_memory::MemoryStore;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const ORG: u128 = 0x0A;
const DEFAULT: u128 = 0xD0;
const OTHER: u128 = 0xE0;

async fn state() -> DataroomState {
    let org = OrgId::from_uuid(Uuid::from_u128(ORG));
    let store = Arc::new(MemoryStore::new());
    for (id, name, host) in [
        (DEFAULT, "default", "data.city.test"),
        (OTHER, "other", "other.city.test"),
    ] {
        let id = CatalogueId::from_uuid(Uuid::from_u128(id));
        dataroom_server_catalogue(store.as_ref(), org, id, name, host).await;
    }
    DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::default()),
        org,
        CatalogueId::from_uuid(Uuid::from_u128(DEFAULT)),
    )
}

/// A catalogue as `dataroom_server::ensure_catalogue` makes it, without depending on the server.
async fn dataroom_server_catalogue(
    store: &dyn Store,
    org: OrgId,
    id: CatalogueId,
    name: &str,
    host: &str,
) {
    let mut cat: dataroom_core::Catalogue = serde_json::from_value(serde_json::json!({
        "orgId": org, "id": id, "name": name, "host": host,
        "title": {"en": name}, "description": {}, "defaultLanguage": "en",
        "languages": ["en"], "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-09T00:00:00Z",
    }))
    .expect("catalogue");
    cat.host = host.into();
    store.upsert_catalogue(cat).await.expect("upsert");
}

async fn get(
    state: DataroomState,
    host: &str,
    mount: Option<jc_router::Mount>,
    path: &str,
) -> (StatusCode, String) {
    let mut req = Request::get(path)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap();
    if let Some(m) = mount {
        req.extensions_mut().insert(m);
    }
    let resp = router(state).oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn mount(scheme: &str, host: &str, prefix: &str) -> jc_router::Mount {
    jc_router::Mount::new("dataroom", scheme, host, prefix).unwrap()
}

#[tokio::test]
async fn the_mount_names_the_catalogue_and_the_links_not_the_raw_host() {
    // The raw Host names the other catalogue; the router resolved a forwarded data.city.test
    // over http under /opendata.
    let at = Some(mount("http", "data.city.test:8080", "/opendata"));
    let (status, body) = get(
        state().await,
        "other.city.test",
        at,
        "/api/3/action/package_search?q=",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("\"help\":\"http://data.city.test:8080/opendata/api/3/action/help_show"),
        "{body}"
    );
    let (_, body) = get(
        state().await,
        "other.city.test",
        Some(mount("http", "data.city.test", "/opendata")),
        "/",
    )
    .await;
    assert!(
        body.contains("href=\"/opendata/"),
        "links under the entry's prefix: {body}"
    );
    // Without a router the Host header still names it (the standalone server).
    let (_, body) = get(state().await, "other.city.test", None, "/catalog.jsonld").await;
    assert!(body.contains("https://other.city.test"), "{body}");
    assert!(!body.contains("data.city.test"), "{body}");
    // A host the router placed that no catalogue holds is no catalogue.
    let (status, _) = get(
        state().await,
        "data.city.test",
        Some(mount("https", "nope.city.test", "")),
        "/api/3/action/package_list",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A request as jc's router hands it over: its `Mount`, and the Organization its host registry
/// decided (a bound host's row or a shared host's first segment).
async fn call(
    state: DataroomState,
    at: jc_router::Mount,
    organization: Option<&str>,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "raw.header.test")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, t);
    }
    let mut req = req
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    req.extensions_mut().insert(at);
    if let Some(org) = organization {
        req.extensions_mut()
            .insert(jc_router::Organization(org.to_owned()));
    }
    let resp = router(state).oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Two Organizations, `com_alpha` on the default catalogue and `com_beta` on the other, each
/// with its canonical base: the default one https under a prefix, the other http on its host.
async fn two_organizations() -> DataroomState {
    let (a, b) = (
        CatalogueId::from_uuid(Uuid::from_u128(DEFAULT)),
        CatalogueId::from_uuid(Uuid::from_u128(OTHER)),
    );
    state()
        .await
        .with_public_base("https://data.city.test/opendata")
        .with_organization_catalogues(
            [("com_alpha".to_owned(), a), ("com_beta".to_owned(), b)].into(),
        )
        .with_catalogue_bases([(b, "http://other.city.test".to_owned())].into())
}

/// T-6797: every absolute id and link names the catalogue's canonical base, whichever
/// registered host (bound or shared with the Organization's prefix) and scheme read it; the
/// Host header never reaches an id.
#[tokio::test]
async fn ids_and_help_links_name_the_canonical_base_on_every_host() {
    let reads = [
        // (mount, decided Organization, canonical base)
        (
            mount("http", "other.city.test", ""),
            Some("com_beta"),
            "http://other.city.test",
        ),
        (
            mount("https", "city.test", "/com_beta/opendata"),
            Some("com_beta"),
            "http://other.city.test",
        ),
        (
            mount("http", "localhost:42301", "/com_beta/opendata"),
            Some("com_beta"),
            "http://other.city.test",
        ),
        (
            mount("http", "data.city.test", "/opendata"),
            None,
            "https://data.city.test/opendata",
        ),
        (
            mount("https", "city.test", "/com_alpha/opendata"),
            Some("com_alpha"),
            "https://data.city.test/opendata",
        ),
    ];
    for (at, org, base) in reads {
        let seen = at.public_base();
        let (status, help) = call(
            two_organizations().await,
            at.clone(),
            org,
            "GET",
            "/api/3/action/package_search?q=",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{seen}: {help}");
        assert!(
            help.contains(&format!("\"help\":\"{base}/api/3/action/help_show")),
            "{seen}: {help}"
        );
        let (status, dcat) = call(
            two_organizations().await,
            at,
            org,
            "GET",
            "/catalog.jsonld",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{seen}: {dcat}");
        assert!(dcat.contains(&format!("{base}/")), "{seen}: {dcat}");
        for foreign in [
            "raw.header.test",
            "localhost",
            "https://other.city.test",
            "http://data.city.test",
        ] {
            assert!(
                !dcat.contains(foreign) && !help.contains(foreign),
                "{seen} names {foreign}"
            );
        }
        if seen.contains("/com_") {
            assert!(
                !dcat.contains("city.test/com_"),
                "{seen}: an id follows the request"
            );
        }
    }
}

/// T-6796: each Organization's datasets sit in its own catalogue's organisation alone, and an
/// Organization the router never decided reads as nothing, its name not echoed.
#[tokio::test]
async fn organization_show_of_one_organization_lists_no_dataset_of_another() {
    let org = OrgId::from_uuid(Uuid::from_u128(ORG));
    let (a, b) = (
        CatalogueId::from_uuid(Uuid::from_u128(DEFAULT)),
        CatalogueId::from_uuid(Uuid::from_u128(OTHER)),
    );
    let (token_a, token_b) = ("a".repeat(40), "b".repeat(40));
    let (bind_a, grant_a) = portal_publisher_on(&token_a, org, a, "com_alpha").unwrap();
    let (bind_b, grant_b) = portal_publisher_on(&token_b, org, b, "com_beta").unwrap();
    let state = two_organizations().await.with_tokens(vec![bind_a, bind_b]);
    let state = DataroomState {
        authorizer: Arc::new(PolicyAuthorizer::new(vec![grant_a, grant_b])),
        ..state
    };
    let on = |org: &str| mount("https", "city.test", &format!("/{org}/opendata"));
    for (who, token) in [("com_alpha", &token_a), ("com_beta", &token_b)] {
        let (status, body) = call(
            state.clone(),
            on(who),
            Some(who),
            "POST",
            "/api/3/action/organization_create",
            Some(token),
            Some(serde_json::json!({ "name": who, "title": who })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
        let (status, body) = call(
            state.clone(),
            on(who),
            Some(who),
            "POST",
            "/api/3/action/package_create",
            Some(token),
            Some(serde_json::json!({
                "name": format!("{who}-budget"), "title": "Budget", "notes": "Budget",
                "owner_org": who, "license_id": "cc-by",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
    }
    for (who, other) in [("com_alpha", "com_beta"), ("com_beta", "com_alpha")] {
        let (status, body) = call(
            state.clone(),
            on(who),
            Some(who),
            "GET",
            &format!("/api/3/action/organization_show?id={who}&include_datasets=true"),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
        assert!(body.contains("\"package_count\":1"), "{who}: {body}");
        assert!(!body.contains(other), "{who} names {other}: {body}");
        let (_, list) = call(
            state.clone(),
            on(who),
            Some(who),
            "GET",
            &format!("/api/3/action/package_search?fq=organization:{who}"),
            None,
            None,
        )
        .await;
        assert!(list.contains(&format!("{who}-budget")), "{who}: {list}");
        assert!(!list.contains(other), "{who} lists {other}: {list}");
        // The other Organization's organisation is not in this catalogue at all.
        let (status, _) = call(
            state.clone(),
            on(who),
            Some(who),
            "GET",
            &format!("/api/3/action/organization_show?id={other}"),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} shows {other}");
    }
    let (status, body) = call(
        state,
        on("com_gamma"),
        Some("com_gamma"),
        "GET",
        "/api/3/action/package_list",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.contains("com_gamma"), "{body}");
}

#[tokio::test]
async fn a_loopback_name_reads_the_default_catalogue_only_with_one_catalogue() {
    for host in ["localhost:42301", "127.0.0.1"] {
        let (status, body) = get(state().await, host, None, "/api/3/action/package_list").await;
        assert_eq!(status, StatusCode::OK, "{host}: {body}");
        let several = state().await.with_loopback_default(false);
        let (status, body) = get(several, host, None, "/api/3/action/package_list").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{host}: {body}");
        let several = state().await.with_loopback_default(false);
        let at = Some(mount("http", host, "/opendata"));
        let (status, body) = get(several, host, at, "/api/3/action/package_list").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{host} via a mount: {body}");
    }
}

#[test]
fn the_portal_grant_of_one_catalogue_writes_no_other() {
    let org = OrgId::from_uuid(Uuid::from_u128(ORG));
    let (a, b) = (
        CatalogueId::from_uuid(Uuid::from_u128(DEFAULT)),
        CatalogueId::from_uuid(Uuid::from_u128(OTHER)),
    );
    let token_a = "a".repeat(40);
    let token_b = "b".repeat(40);
    let (bind_a, grant_a) = portal_publisher_on(&token_a, org, a, "alpha").unwrap();
    let (bind_b, grant_b) = portal_publisher_on(&token_b, org, b, "beta").unwrap();
    // The installation's default publisher beside them, on catalogue a.
    let (bind_d, grant_d) = portal_publisher(&"d".repeat(40), org, a).unwrap();
    let auth = PolicyAuthorizer::new(vec![grant_a, grant_b, grant_d]);
    let dataset = |cat, publisher: &str| ObjectTarget::Publisher {
        org_id: org,
        catalogue_id: cat,
        publisher_id: publisher.into(),
        editor_grants: Vec::new(),
    };
    let may = |subject: &dataroom_core::Subject, cat: CatalogueId, publisher: &str| {
        let ctx = RequestCtx::new(org, cat, subject.clone());
        auth.check(&ctx, Action::Propose, &dataset(cat, publisher))
            .allowed
    };
    assert!(may(&bind_a.subject, a, "alpha"));
    assert!(may(&bind_b.subject, b, "beta"));
    // Not the other catalogue, under any publisher; not another publisher in its own.
    assert!(!may(&bind_a.subject, b, "beta"));
    assert!(!may(&bind_a.subject, b, "alpha"));
    assert!(!may(&bind_b.subject, a, "alpha"));
    assert!(!may(&bind_a.subject, a, "hel-fi"));
    // The default publisher, catalogue-wide on a, reaches nothing of b.
    assert!(!may(&bind_d.subject, b, "beta"));
}
