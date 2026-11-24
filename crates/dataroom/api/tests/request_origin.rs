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
/// A catalogue the deployment names whose row no longer exists.
const GAMMA: u128 = 0xF0;

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
#[tokio::test]
async fn a_catalogues_uploaded_profile_is_listed_only_on_its_own_host() {
    let state = state().await;
    let org = OrgId::from_uuid(Uuid::from_u128(ORG));
    let own = CatalogueId::from_uuid(Uuid::from_u128(DEFAULT));
    let ctx = RequestCtx::new(
        org,
        own,
        dataroom_core::Subject::authenticated("seed", vec![org], Vec::new()),
    );
    let record: dataroom_core::ProfileRecord = serde_json::from_value(serde_json::json!({
        "id": "own-draft", "version": "1.0.0", "uri": "https://profiles.example.org/own-draft",
        "title": {"en": "Own draft"}, "builtIn": false,
        "shapesTurtle": "@prefix sh: <http://www.w3.org/ns/shacl#> .
    <urn:own:Shape> a sh:NodeShape .",
    }))
    .expect("profile");
    let cat = state.store.get_catalogue(org, own).await.unwrap().unwrap();
    let (saved, _) = state
        .profiles
        .upload_and_dry_run(record, &cat, &[], &Default::default())
        .expect("upload");
    state.store.upsert_profile(&ctx, saved).await.unwrap();

    let (status, body) = get(state.clone(), "other.city.test", None, "/api/v1/profiles").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains("own-draft"),
        "another catalogue's upload is listed"
    );
    assert!(body.contains("dcat-ap-sk-2.0"), "built-ins stay listed");
    let (_, body) = get(state, "data.city.test", None, "/api/v1/profiles").await;
    assert!(body.contains("own-draft"), "its own upload is listed");
}

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

/// A pin names the version a feed answers in; an empty one takes the newest upload. An upload's
/// URI goes into Content-Profile only when a header can carry it as a value.
#[tokio::test]
async fn a_feed_answers_in_the_pinned_version_and_names_it_only_when_a_header_can() {
    let state = state().await;
    let org = OrgId::from_uuid(Uuid::from_u128(ORG));
    let own = CatalogueId::from_uuid(Uuid::from_u128(DEFAULT));
    let ctx = RequestCtx::new(
        org,
        own,
        dataroom_core::Subject::authenticated("seed", vec![org], Vec::new()),
    );
    let mut cat = state.store.get_catalogue(org, own).await.unwrap().unwrap();
    // Version 2 asks every Dataset for a property none has; version 1 asks nothing.
    let never = "@prefix sh: <http://www.w3.org/ns/shacl#> .
    @prefix dcat: <http://www.w3.org/ns/dcat#> .
    <urn:own:Shape> a sh:NodeShape ; sh:targetClass dcat:Dataset ;
      sh:property [ sh:path <urn:own:never> ; sh:minCount 1 ] .";
    let nothing = "@prefix sh: <http://www.w3.org/ns/shacl#> .
    <urn:own:Shape> a sh:NodeShape .";
    for (id, version, uri, shapes) in [
        (
            "own-draft",
            "1.0.0",
            "https://profiles.example.org/own-draft/1",
            nothing,
        ),
        (
            "own-draft",
            "2.0.0",
            "https://profiles.example.org/own-draft/2",
            never,
        ),
        (
            "third",
            "1.0.0",
            "https://profiles.example.org/third/1",
            nothing,
        ),
        (
            "third",
            "2.0.0",
            "https://profiles.example.org/third/2",
            nothing,
        ),
        (
            "angled",
            "3.0.0",
            "https://profiles.example.org/<angled>",
            nothing,
        ),
    ] {
        let record: dataroom_core::ProfileRecord = serde_json::from_value(serde_json::json!({
            "id": id, "version": version, "uri": uri,
            "title": {"en": id}, "builtIn": false, "shapesTurtle": shapes,
        }))
        .expect("profile");
        let (saved, _) = state
            .profiles
            .upload_and_dry_run(record, &cat, &[], &Default::default())
            .expect("upload");
        state.store.upsert_profile(&ctx, saved).await.unwrap();
    }
    let profile_of = |path: &'static str, state: DataroomState| async move {
        let req = Request::get(path)
            .header(header::HOST, "data.city.test")
            .body(Body::empty())
            .unwrap();
        let resp = router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        resp.headers()
            .get("content-profile")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    for (pinned, want) in [("1.0.0", "own-draft/1"), ("", "own-draft/2")] {
        cat.active_profiles = vec![
            serde_json::from_value(
                serde_json::json!({"profileId": "own-draft", "version": pinned}),
            )
            .unwrap(),
            serde_json::from_value(serde_json::json!({"profileId": "angled", "version": ""}))
                .unwrap(),
            serde_json::from_value(serde_json::json!({"profileId": "third", "version": ""}))
                .unwrap(),
        ];
        state.store.upsert_catalogue(cat.clone()).await.unwrap();
        let named = profile_of("/profiles/own-draft/catalog.ttl", state.clone()).await;
        assert!(named.ends_with(want), "pin {pinned:?}: {named}");
        // Another profile's pinned version is never this one's.
        let third = profile_of("/profiles/third/catalog.ttl", state.clone()).await;
        assert!(third.ends_with("third/2"), "pin {pinned:?}: {third}");
    }
    let angled = profile_of("/profiles/angled/catalog.ttl", state.clone()).await;
    assert!(
        !angled.contains("angled") && angled.starts_with("http"),
        "an upload's URI with angle brackets gives way to its renderer's: {angled}"
    );
    // The admin report judges each pin in its version: own-draft 1.0.0 asks nothing, and the
    // unversioned pin of angled is its newest upload, installed.
    cat.active_profiles[0].version = "1.0.0".into();
    let dataset: dataroom_core::Dataset = serde_json::from_value(serde_json::json!({
        "orgId": org, "catalogueId": own, "id": "id-air", "name": "air",
        "title": {"en": "Air"}, "description": {}, "publisherId": "nobody",
        "visibility": "public", "state": "active",
        "issued": "2026-01-02T08:00:00Z", "modified": "2026-11-05T10:30:00Z",
    }))
    .expect("dataset");
    let gaps = dataroom_api::feeds::profile_gaps(
        &state,
        &cat,
        &[dataset],
        &std::collections::BTreeMap::new(),
    );
    let named: Vec<&str> = gaps[0].iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        !named.contains(&"own-draft") && !named.contains(&"angled"),
        "{:?}",
        gaps[0]
    );
}

/// Two Organizations, `com_alpha` on the default catalogue and `com_beta` on the other, each
/// with its canonical base: the default one https under a prefix, the other http on its host.
async fn two_organizations() -> DataroomState {
    let org = OrgId::from_uuid(Uuid::from_u128(ORG));
    let (a, b) = (
        CatalogueId::from_uuid(Uuid::from_u128(DEFAULT)),
        CatalogueId::from_uuid(Uuid::from_u128(OTHER)),
    );
    state()
        .await
        .with_public_base("https://data.city.test/opendata")
        .with_organization_catalogues(
            [
                ("com_alpha".to_owned(), (org, a)),
                ("com_beta".to_owned(), (org, b)),
                // An Organization whose catalogue row is gone is never a new home.
                (
                    "com_gamma".to_owned(),
                    (org, CatalogueId::from_uuid(Uuid::from_u128(GAMMA))),
                ),
                // Nor one whose catalogue has no configured base.
                (
                    "com_delta".to_owned(),
                    (org, CatalogueId::from_uuid(Uuid::from_u128(0xF1))),
                ),
            ]
            .into(),
        )
        .with_catalogue_bases(
            [
                (b, "http://other.city.test".to_owned()),
                (
                    CatalogueId::from_uuid(Uuid::from_u128(GAMMA)),
                    "http://gamma.city.test".to_owned(),
                ),
            ]
            .into(),
        )
}

/// The installation's public base names the default catalogue only: another catalogue of the
/// same Organization, with no base of its own, keeps the host it is read on.
#[tokio::test]
async fn the_public_base_is_the_default_catalogues_alone() {
    let state = state()
        .await
        .with_public_base("https://data.city.test/opendata");
    let (_, body) = get(state.clone(), "other.city.test", None, "/catalog.jsonld").await;
    assert!(body.contains("https://other.city.test"), "{body}");
    assert!(!body.contains("data.city.test/opendata"), "{body}");
    let (_, body) = get(state, "data.city.test", None, "/catalog.jsonld").await;
    assert!(body.contains("https://data.city.test/opendata"), "{body}");
}

/// `name` from the Endpoint `slug`, `visibility`, in `catalogue` of `state`'s store; withdrawn
/// when `withdrawn`.
async fn seed(
    state: &DataroomState,
    catalogue: u128,
    (name, slug, visibility): (&str, &str, &str),
    withdrawn: bool,
) {
    let ctx = RequestCtx::new(
        state.default_org_id,
        CatalogueId::from_uuid(Uuid::from_u128(catalogue)),
        dataroom_core::Subject::authenticated("seed", vec![state.default_org_id], Vec::new()),
    );
    let dataset: dataroom_core::Dataset = serde_json::from_value(serde_json::json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"en": name}, "description": {}, "publisherId": "city",
        "visibility": visibility, "state": "active",
        "endpointUrl": format!("https://api.city.test/api/endpoint/{slug}/"),
        "distributions": [], "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    }))
    .expect("dataset");
    state
        .store
        .upsert_dataset(&ctx, dataset)
        .await
        .expect("upsert");
    if withdrawn {
        state
            .store
            .withdraw_dataset(&ctx, name)
            .await
            .expect("withdraw");
    }
}

/// `GET path` on `com_alpha`'s mount: the status and the `Location`.
async fn on_alpha(state: DataroomState, path: &str) -> (StatusCode, Option<String>) {
    let mut req = Request::get(path).body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(mount("https", "city.test", "/com_alpha/opendata"));
    req.extensions_mut()
        .insert(jc_router::Organization("com_alpha".to_owned()));
    let resp = router(state).oneshot(req).await.unwrap();
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (resp.status(), location)
}

/// An authorizer that refuses every read in one catalogue and otherwise decides as the policy.
struct NoReadIn(CatalogueId);

impl Authorizer for NoReadIn {
    fn check(
        &self,
        ctx: &RequestCtx,
        action: Action,
        object: &ObjectTarget,
    ) -> dataroom_core::Decision {
        if action == Action::Read && object.catalogue_id() == self.0 {
            return dataroom_core::Decision::deny_hidden("closed");
        }
        PolicyAuthorizer::default().check(ctx, action, object)
    }
}

/// A new home the caller may not read is no new home: the old address keeps its 404 and never
/// names the other catalogue.
#[tokio::test]
async fn an_old_dataset_address_never_redirects_to_a_copy_the_caller_may_not_read() {
    let mut state = two_organizations().await;
    seed(&state, DEFAULT, ("air", "airslug", "public"), true).await;
    seed(&state, OTHER, ("air", "airslug", "public"), false).await;
    assert_eq!(
        on_alpha(state.clone(), "/dataset/air").await.0,
        StatusCode::MOVED_PERMANENTLY
    );
    state.authorizer = Arc::new(NoReadIn(CatalogueId::from_uuid(Uuid::from_u128(OTHER))));
    assert_eq!(
        on_alpha(state, "/dataset/air").await,
        (StatusCode::NOT_FOUND, None)
    );
}

/// An Organization the router decided that has no catalogue here answers 404 without echoing
/// its name.
#[tokio::test]
async fn an_organization_without_a_catalogue_here_is_not_found() {
    let (status, body) = call(
        two_organizations().await,
        mount("https", "city.test", "/com_zeta/opendata"),
        Some("com_zeta"),
        "GET",
        "/api/3/action/status_show",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body.contains("no catalogue here"), "{body}");
    assert!(!body.contains("com_zeta"), "{body}");
}

/// T-6853: a project moved from `com_alpha` to `com_beta` withdrew its dataset here and published
/// it there; its old address answers 301 to the new one, its file format and query kept. Only the
/// same Endpoint's dataset, public and active, in a catalogue of this deployment is a new home:
/// another Endpoint's dataset of the name, a non-public one, or none keep the 404.
#[tokio::test]
async fn an_old_dataset_address_redirects() {
    let state = two_organizations().await;
    for (name, slug, beta) in [
        ("air", "airslug", Some(("airslug", "public"))),
        ("noise", "noiseslug", Some(("otherslug", "public"))),
        ("inner", "innerslug", Some(("innerslug", "internal"))),
        ("gone", "goneslug", None),
    ] {
        seed(&state, DEFAULT, (name, slug, "public"), true).await;
        if let Some((slug, visibility)) = beta {
            seed(&state, OTHER, (name, slug, visibility), false).await;
        }
    }
    assert_eq!(
        on_alpha(state.clone(), "/dataset/air").await,
        (
            StatusCode::MOVED_PERMANENTLY,
            Some("http://other.city.test/dataset/air".to_owned())
        )
    );
    assert_eq!(
        on_alpha(state.clone(), "/dataset/air.ttl?_profile=dcat-ap-sk-2.0").await,
        (
            StatusCode::MOVED_PERMANENTLY,
            Some("http://other.city.test/dataset/air.ttl?_profile=dcat-ap-sk-2.0".to_owned())
        )
    );
    for kept in ["noise", "inner", "gone", "never"] {
        let (status, location) = on_alpha(state.clone(), &format!("/dataset/{kept}")).await;
        assert_eq!((status, location), (StatusCode::NOT_FOUND, None), "{kept}");
    }
    // A copy withdrawn there too is no new home.
    seed(&state, DEFAULT, ("tram", "tramslug", "public"), true).await;
    seed(&state, OTHER, ("tram", "tramslug", "public"), true).await;
    assert_eq!(
        on_alpha(state.clone(), "/dataset/tram").await,
        (StatusCode::NOT_FOUND, None)
    );
    // Moved the other way, into the default catalogue: its base is the installation's public base.
    seed(&state, OTHER, ("bus", "busslug", "public"), true).await;
    seed(&state, DEFAULT, ("bus", "busslug", "public"), false).await;
    let mut req = Request::get("/dataset/bus").body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(mount("https", "city.test", "/com_beta/opendata"));
    req.extensions_mut()
        .insert(jc_router::Organization("com_beta".to_owned()));
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        resp.headers()[header::LOCATION],
        "https://data.city.test/opendata/dataset/bus"
    );
    // A new home that hides the dataset, or whose publisher is withdrawn, is no new home.
    let beta = CatalogueId::from_uuid(Uuid::from_u128(OTHER));
    let mut cat = state
        .store
        .get_catalogue(state.default_org_id, beta)
        .await
        .unwrap()
        .unwrap();
    // A catalogue with no base of its own is no new home, though the installation has one.
    let mut delta = cat.clone();
    delta.id = CatalogueId::from_uuid(Uuid::from_u128(0xF1));
    delta.name = "delta".into();
    delta.host = "delta.city.test".into();
    state.store.upsert_catalogue(delta).await.unwrap();
    seed(&state, DEFAULT, ("van", "vanslug", "public"), true).await;
    seed(&state, 0xF1, ("van", "vanslug", "public"), false).await;
    assert_eq!(
        on_alpha(state.clone(), "/dataset/van").await,
        (StatusCode::NOT_FOUND, None)
    );
    seed(&state, GAMMA, ("air", "airslug", "public"), false).await;
    cat.hidden_datasets.insert("air".into(), "kept out".into());
    state.store.upsert_catalogue(cat.clone()).await.unwrap();
    assert_eq!(
        on_alpha(state.clone(), "/dataset/air").await,
        (StatusCode::NOT_FOUND, None)
    );
    cat.hidden_datasets.clear();
    state.store.upsert_catalogue(cat).await.unwrap();
    let ctx = RequestCtx::new(
        state.default_org_id,
        beta,
        dataroom_core::Subject::authenticated("seed", vec![state.default_org_id], Vec::new()),
    );
    let publisher = serde_json::from_value(serde_json::json!({
        "orgId": state.default_org_id, "catalogueId": beta, "id": "city", "name": "city",
        "title": {"en": "City"}, "description": {}, "uri": "https://city.test/publisher",
        "state": "withdrawn", "editorGrants": [],
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z",
    }))
    .expect("publisher");
    state.store.upsert_publisher(&ctx, publisher).await.unwrap();
    assert_eq!(
        on_alpha(state.clone(), "/dataset/air").await,
        (StatusCode::NOT_FOUND, None)
    );
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
    let issuer = jc_door::testing::Issuer::start("https://id.city.test", "k1").await;
    let clients = ["portal-publisher".to_owned()];
    let verifier = jc_door::Verifier::new(&issuer.issuer, &issuer.jwks_url, &clients).unwrap();
    // One service token: who it is follows from the catalogue the address names.
    let token = issuer.token("portal-publisher", "dataroom", 300);
    let (subject_a, grant_a) = portal_publisher_on(org, a, "com_alpha");
    let (subject_b, grant_b) = portal_publisher_on(org, b, "com_beta");
    let state = two_organizations()
        .await
        .with_verifier(Arc::new(verifier))
        .with_publishers([((org, a), subject_a), ((org, b), subject_b)].into());
    let state = DataroomState {
        authorizer: Arc::new(PolicyAuthorizer::new(vec![grant_a, grant_b])),
        ..state
    };
    let on = |org: &str| mount("https", "city.test", &format!("/{org}/opendata"));
    for who in ["com_alpha", "com_beta"] {
        let (status, body) = call(
            state.clone(),
            on(who),
            Some(who),
            "POST",
            "/api/3/action/organization_create",
            Some(&token),
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
            Some(&token),
            Some(serde_json::json!({
                "name": format!("{who}-budget"), "title": "Budget", "notes": "Budget",
                "owner_org": who, "license_id": "cc-by",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
    }
    for (who, other) in [("com_alpha", "com_beta"), ("com_beta", "com_alpha")] {
        // The token writes as the publisher of the catalogue it was sent to, and as no other's.
        let (status, body) = call(
            state.clone(),
            on(who),
            Some(who),
            "POST",
            "/api/3/action/organization_create",
            Some(&token),
            Some(serde_json::json!({ "name": other, "title": other })),
        )
        .await;
        assert!(status.is_client_error(), "{who} creates {other}: {body}");
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
    let (as_a, grant_a) = portal_publisher_on(org, a, "alpha");
    let (as_b, grant_b) = portal_publisher_on(org, b, "beta");
    // The installation's default publisher beside them, on catalogue a.
    let (as_d, grant_d) = portal_publisher(org, a);
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
    assert!(may(&as_a, a, "alpha"));
    assert!(may(&as_b, b, "beta"));
    // Not the other catalogue, under any publisher; not another publisher in its own.
    assert!(!may(&as_a, b, "beta"));
    assert!(!may(&as_a, b, "alpha"));
    assert!(!may(&as_b, a, "alpha"));
    assert!(!may(&as_a, a, "hel-fi"));
    // The default publisher, catalogue-wide on a, reaches nothing of b.
    assert!(!may(&as_d, b, "beta"));
}
