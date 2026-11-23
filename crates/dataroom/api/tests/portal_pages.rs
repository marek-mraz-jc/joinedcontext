//! A portal's pages built from blocks (T-7433), its slug addresses behind the router (T-7417),
//! the Dashboards its home lists (T-6780) and the installed DCAT profiles the Portal is told of.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use dataroom_api::{router, DashboardCard, DataroomState, PortalAddress, PortalTables};
use dataroom_core::files::FileStore;
use dataroom_core::{CatalogueId, OrgId, PolicyAuthorizer, RequestCtx, Store, Subject};
use dataroom_store_memory::MemoryStore;
use http_body_util::BodyExt;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

fn org(n: u128) -> OrgId {
    OrgId::from_uuid(Uuid::from_u128(n))
}

fn cat(n: u128) -> CatalogueId {
    CatalogueId::from_uuid(Uuid::from_u128(n))
}

async fn catalogue(store: &dyn Store, org: OrgId, id: CatalogueId, name: &str, host: &str) {
    let cat: dataroom_core::Catalogue = serde_json::from_value(json!({
        "orgId": org, "id": id, "name": name, "host": host,
        "title": {"en": name}, "description": {}, "defaultLanguage": "en",
        "languages": ["en"], "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-19T00:00:00Z",
    }))
    .expect("catalogue");
    store.upsert_catalogue(cat).await.expect("upsert");
}

async fn dataset(store: &dyn Store, org: OrgId, id: CatalogueId, name: &str) {
    let ctx = RequestCtx::new(
        org,
        id,
        Subject::authenticated("seed", vec![org], Vec::new()),
    );
    let publisher = serde_json::from_value(json!({
        "orgId": org, "catalogueId": id, "id": "city", "name": "city",
        "title": {"en": "City"}, "description": {}, "uri": "https://a.test/publisher/city",
        "state": "active", "editorGrants": [],
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z",
    }))
    .expect("publisher");
    store
        .upsert_publisher(&ctx, publisher)
        .await
        .expect("publisher");
    let ds = serde_json::from_value(json!({
        "orgId": org, "catalogueId": id, "id": format!("id-{name}"), "name": name,
        "title": {"en": format!("Dataset {name}")}, "description": {"en": "About"},
        "publisherId": "city", "visibility": "public", "state": "active",
        "issued": "2026-01-02T08:00:00Z", "modified": "2026-11-05T10:30:00Z",
        "distributions": [{ "id": format!("dist-{name}"), "datasetId": format!("id-{name}"),
            "title": {"en": "CSV"}, "accessUrl": "https://data.a.test/x.csv" }],
    }))
    .expect("dataset");
    store.upsert_dataset(&ctx, ds).await.expect("dataset");
}

async fn get(
    app: axum::Router,
    host: &str,
    path: &str,
    org: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut req = Request::get(path)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap();
    if let Some(org) = org {
        // The router decided the Organization and the forwarded host.
        req.extensions_mut()
            .insert(jc_router::Organization(org.to_owned()));
        req.extensions_mut()
            .insert(jc_router::Mount::new("dataroom", "https", host, "").unwrap());
    }
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

const PDF: &[u8] = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n";

/// A catalogue whose Portal declares a home and a further page from blocks, with a file and a chart.
async fn paged(files: bool) -> (DataroomState, Option<std::path::PathBuf>, String) {
    let (a, own) = (org(0xA), cat(0xA0));
    let store = Arc::new(MemoryStore::new());
    catalogue(store.as_ref(), a, own, "default", "opendata.a.test").await;
    dataset(store.as_ref(), a, own, "rozpocet").await;
    let mut state = DataroomState::new(store, Arc::new(PolicyAuthorizer::default()), a, own)
        .with_organization_catalogues(BTreeMap::from([("a".to_owned(), (a, own))]));
    let (mut root, mut file_path) = (None, format!("/files/{}/plan.pdf", "0".repeat(64)));
    if files {
        let dir = std::env::temp_dir().join(format!("jc-portal-pages-{}", Uuid::now_v7()));
        let store = Arc::new(FileStore::new(&dir));
        file_path = store.put((a, own), "plan.pdf", PDF, "ana").unwrap().path();
        state = state.with_files(store);
        root = Some(dir);
    }
    let pages: Vec<jc_core::kinds::portal_page::PortalPage> = serde_json::from_value(json!([
        { "slug": "home", "rows": [
            { "blocks": [{ "type": "hero", "title": { "en": "Blocks home" } }] },
            { "blocks": [{ "type": "chart", "dataset": "rozpocet" }, { "type": "chart", "dataset": "missing" }] },
            { "blocks": [{ "type": "file-list", "title": { "en": "Plans" }, "files": [file_path.clone()] }] }] },
        { "slug": "budget", "title": { "en": "Budget" }, "rows": [
            { "blocks": [{ "type": "text", "markdown": { "en": "The city budget." } }] }] },
    ]))
    .expect("pages");
    jc_core::kinds::portal_page::validate_pages(&pages).expect("valid pages");
    let mut look = dataroom_core::branding::BrandingBlock::default();
    look.layout.pages = pages;
    let state = state.with_catalogue_branding(BTreeMap::from([(own, look)]));
    (state, root, file_path)
}

#[tokio::test]
async fn a_portal_built_from_blocks_shows_its_home_its_pages_and_its_files() {
    let (state, root, file) = paged(true).await;
    let app = router(state);
    let (status, _, home) = get(app.clone(), "opendata.a.test", "/", None).await;
    assert_eq!(status, StatusCode::OK, "{home}");
    assert!(
        home.contains("Blocks home"),
        "the blocks home replaces the built-in one"
    );
    assert!(
        home.contains("plan.pdf"),
        "the file-list block names its file: {home}"
    );
    let (status, _, page) = get(app.clone(), "opendata.a.test", "/page/budget", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("The city budget."), "{page}");
    // A page the Portal never declared, and the home under its page address, are no page.
    for path in ["/page/nope", "/page/home"] {
        let (status, _, _) = get(app.clone(), "opendata.a.test", path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, headers, _) = get(app.clone(), "opendata.a.test", &file, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    // A host no catalogue holds reads no file.
    let (status, _, _) = get(app, "nobody.test", &file, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let _ = std::fs::remove_dir_all(root.expect("a file root"));
}

#[tokio::test]
async fn without_a_file_store_a_block_lists_no_file_and_a_file_address_is_not_found() {
    let (state, _, file) = paged(false).await;
    let app = router(state);
    let (status, _, home) = get(app.clone(), "opendata.a.test", "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("Blocks home"));
    assert!(
        !home.contains("plan.pdf"),
        "an unknown file is dropped: {home}"
    );
    let (status, _, _) = get(app, "opendata.a.test", &file, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_home_lists_the_dashboards_the_resolver_names_for_its_catalogue_and_language() {
    let (a, own) = (org(0xA), cat(0xA0));
    let store = Arc::new(MemoryStore::new());
    catalogue(store.as_ref(), a, own, "default", "opendata.a.test").await;
    let state = DataroomState::new(store, Arc::new(PolicyAuthorizer::default()), a, own)
        .with_dashboard_resolver(move |id, lang| {
            if *id != own {
                return Vec::new();
            }
            vec![DashboardCard {
                title: format!("Air quality ({lang})"),
                pages: vec!["Overview".into()],
                sources: vec!["air".into()],
            }]
        });
    let (status, _, home) = get(router(state), "opendata.a.test", "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("Air quality (en)"), "{home}");
    assert!(home.contains("jc-dashboards"), "{home}");
}

/// Organization `a` with its default catalogue and a further `mobility` portal.
async fn slugged() -> (DataroomState, PortalTables) {
    let (a, own, mobility) = (org(0xA), cat(0xA0), cat(0xA1));
    let store = Arc::new(MemoryStore::new());
    catalogue(store.as_ref(), a, own, "default", "opendata.a.test").await;
    catalogue(
        store.as_ref(),
        a,
        mobility,
        "mobility",
        "mobility.a.portal.invalid",
    )
    .await;
    dataset(store.as_ref(), a, mobility, "linky").await;
    let state = DataroomState::new(store, Arc::new(PolicyAuthorizer::default()), a, own)
        .with_organization_catalogues(BTreeMap::from([("a".to_owned(), (a, own))]));
    let tables = state.portals.clone();
    (state, tables)
}

#[tokio::test]
async fn a_slug_address_serves_its_portal_redirects_when_told_and_names_no_unknown_one() {
    let (state, tables) = slugged().await;
    let (a, mobility) = (org(0xA), cat(0xA1));
    let address = PortalAddress {
        org_id: a,
        id: mobility,
        redirect: None,
    };
    tables.route(mobility, "a", "mobility", None, &[], address.clone());
    let app = router(state);
    let (status, _, body) = get(
        app.clone(),
        "opendata.a.test",
        "/-/mobility/api/3/action/package_list",
        Some("a"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("linky"),
        "the mobility catalogue answers: {body}"
    );
    // The bare slug is its portal's home, with links under the slug.
    let (status, _, home) = get(app.clone(), "opendata.a.test", "/-/mobility", Some("a")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        home.contains("/-/mobility/"),
        "links stay under the slug: {home}"
    );
    // Another Organization's request, or a path no portal has, is not found; the name is not echoed.
    let (status, _, body) = get(
        app.clone(),
        "opendata.a.test",
        "/-/secret-portal/",
        Some("a"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.contains("secret-portal"), "{body}");
    let (status, _, _) = get(app.clone(), "opendata.a.test", "/-/mobility/", Some("b")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Without the router's Organization the path is the catalogue's own route table's.
    let (status, _, _) = get(app.clone(), "opendata.a.test", "/-/mobility/", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // `slug: redirect` answers 308 to the portal's own host, path and query kept.
    tables.route(
        mobility,
        "a",
        "mobility",
        Some("mobility.a.test"),
        &[],
        PortalAddress {
            redirect: Some("https://mobility.a.test/".into()),
            ..address
        },
    );
    let (status, headers, _) = get(
        app.clone(),
        "opendata.a.test",
        "/-/mobility/dataset/?q=bus",
        Some("a"),
    )
    .await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        headers[header::LOCATION],
        "https://mobility.a.test/dataset/?q=bus"
    );
    // The portal's own host names its catalogue for its own Organization.
    let (status, _, body) = get(
        app,
        "mobility.a.test",
        "/api/3/action/package_list",
        Some("a"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("linky"), "{body}");
}

#[test]
fn a_moved_portal_keeps_its_left_hosts_until_another_takes_them_and_unroute_drops_every_route() {
    let tables = PortalTables::default();
    let (a, one, two) = (org(0xA), cat(0xA1), cat(0xA2));
    let at = |id| PortalAddress {
        org_id: a,
        id,
        redirect: None,
    };
    tables.route(one, "a", "first", Some("one.a.test"), &[], at(one));
    assert_eq!(tables.host("one.a.test"), Some((a, one)));
    assert_eq!(tables.slug("a", "first").map(|p| p.id), Some(one));
    // Moved to a new slug and host: the old slug is gone, the left host still reaches it.
    tables.route(
        one,
        "a",
        "renamed",
        Some("new.a.test"),
        &["one.a.test".into()],
        at(one),
    );
    assert!(tables.slug("a", "first").is_none());
    assert_eq!(tables.slug("a", "renamed").map(|p| p.id), Some(one));
    assert_eq!(tables.host("one.a.test"), Some((a, one)));
    assert_eq!(tables.host("new.a.test"), Some((a, one)));
    // A host another portal took stays that portal's when the first one lists it as left.
    tables.route(two, "a", "second", Some("taken.a.test"), &[], at(two));
    tables.route(
        one,
        "a",
        "renamed",
        Some("new.a.test"),
        &["taken.a.test".into()],
        at(one),
    );
    assert_eq!(tables.host("taken.a.test"), Some((a, two)));
    tables.unroute(one);
    assert!(tables.slug("a", "renamed").is_none());
    assert!(tables.host("one.a.test").is_none());
    assert!(tables.host("new.a.test").is_none());
    assert_eq!(
        tables.host("taken.a.test"),
        Some((a, two)),
        "the other portal keeps its routes"
    );
    assert_eq!(tables.slug("a", "second").map(|p| p.id), Some(two));
    assert_eq!(
        dataroom_api::synthetic_host("a", "mobility"),
        "mobility.a.portal.invalid"
    );
}

#[test]
fn the_portal_is_told_only_the_selectable_profiles_with_their_publisher_rules() {
    let listed = dataroom_api::installed_profiles();
    assert!(!listed.is_empty());
    let ids: Vec<&str> = listed.iter().filter_map(|p| p["id"].as_str()).collect();
    assert!(ids.contains(&"dcat-ap-sk-2.0"), "{ids:?}");
    for profile in &listed {
        assert!(
            profile["title"].is_object() || profile["title"].is_string(),
            "{profile}"
        );
        assert!(profile["publisher"].is_boolean(), "{profile}");
        assert!(profile["publisherIriPrefixes"].is_array(), "{profile}");
        let id = profile["id"].as_str().unwrap();
        assert_eq!(
            profile["publisher"].as_bool(),
            Some(dataroom_api::requires_publisher(id)),
            "{id}"
        );
    }
    let pin = dataroom_api::installed_pin("dcat-ap-sk-2.0").expect("installed");
    assert_eq!(pin.profile_id, "dcat-ap-sk-2.0");
    assert!(!pin.version.is_empty());
    assert!(dataroom_api::installed_pin("no-such-profile").is_none());
    assert_eq!(
        dataroom_api::installed_id("dcat-ap-sk-2.0").as_deref(),
        Some("dcat-ap-sk-2.0")
    );
    assert!(dataroom_api::installed_id("no-such-profile").is_none());
    assert!(!dataroom_api::requires_publisher("no-such-profile"));
}
