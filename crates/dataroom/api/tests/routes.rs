#![forbid(unsafe_code)]
//! Every route of the dataroom router, driven through HTTP: what each caller is shown, what each
//! write needs, and what a failing store answers. One catalogue holds a public, an internal, a
//! restricted, a hidden (`jc_hide`), a withdrawn dataset and one of a withdrawn publisher, so each
//! surface is checked against the rules of `package_show` (C4, C7, C11, D2, G2).

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::Router;
use dataroom_api::{mount, router, DataroomState, TokenBinding};
use dataroom_core::store::BoxFuture;
use dataroom_core::{
    Action, ActivityRow, BrandingBlock, Catalogue, CatalogueId, CataloguePlugin, DataroomError,
    Dataset, Distribution, GrantRule, HarvestRun, HarvestSource, OrgId, PolicyAuthorizer,
    ProfileRecord, Publisher, RequestCtx, ResourceView, Result, ShaclValidationReport, StaticPage,
    Store, Subject,
};
use dataroom_store_memory::MemoryStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use uuid::Uuid;

const HOST: &str = "data.city.test";

// ---------------------------------------------------------------------------------------------
// A store that fails on chosen methods and otherwise is the memory store.
// ---------------------------------------------------------------------------------------------

struct FailingStore {
    inner: MemoryStore,
    failing: Mutex<BTreeSet<&'static str>>,
}

impl FailingStore {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            failing: Mutex::new(BTreeSet::new()),
        }
    }

    fn fail(&self, methods: &[&'static str]) {
        self.failing.lock().unwrap().extend(methods.iter().copied());
    }

    fn heal(&self) {
        self.failing.lock().unwrap().clear();
    }

    fn fails(&self, method: &str) -> bool {
        self.failing.lock().unwrap().contains(method)
    }
}

macro_rules! forward {
    ($( $name:ident<$lt:lifetime>($($arg:ident : $ty:ty),*) -> $ret:ty; )*) => {
        $(
            fn $name<$lt>(&$lt self, $($arg: $ty),*) -> BoxFuture<$lt, Result<$ret>> {
                if self.fails(stringify!($name)) {
                    return Box::pin(async {
                        Err(DataroomError::Storage(concat!(stringify!($name), " failed").into()))
                    });
                }
                self.inner.$name($($arg),*)
            }
        )*
    };
}

impl Store for FailingStore {
    forward! {
        resolve_host<'a>(host: &'a str) -> Option<Catalogue>;
        list_catalogues<'a>(org_id: OrgId) -> Vec<Catalogue>;
        get_catalogue<'a>(org_id: OrgId, catalogue_id: CatalogueId) -> Option<Catalogue>;
        upsert_catalogue<'a>(catalogue: Catalogue) -> Catalogue;
        list_publishers<'a>(ctx: &'a RequestCtx) -> Vec<Publisher>;
        get_publisher<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<Publisher>;
        upsert_publisher<'a>(ctx: &'a RequestCtx, publisher: Publisher) -> Publisher;
        merge_publishers<'a>(ctx: &'a RequestCtx, source: &'a str, target: &'a str) -> Publisher;
        withdraw_publisher<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Publisher;
        list_datasets<'a>(ctx: &'a RequestCtx) -> Vec<Dataset>;
        get_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<Dataset>;
        upsert_dataset<'a>(ctx: &'a RequestCtx, dataset: Dataset) -> Dataset;
        withdraw_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Dataset;
        purge_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> ();
        get_distribution<'a>(ctx: &'a RequestCtx, id: &'a str) -> Option<(Dataset, Distribution)>;
        list_resource_views<'a>(ctx: &'a RequestCtx, resource_id: &'a str) -> Vec<ResourceView>;
        get_resource_view<'a>(ctx: &'a RequestCtx, view_id: &'a str) -> Option<ResourceView>;
        upsert_resource_view<'a>(ctx: &'a RequestCtx, view: ResourceView) -> ResourceView;
        delete_resource_view<'a>(ctx: &'a RequestCtx, view_id: &'a str) -> ();
        list_harvest_sources<'a>(ctx: &'a RequestCtx) -> Vec<HarvestSource>;
        get_harvest_source<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<HarvestSource>;
        upsert_harvest_source<'a>(ctx: &'a RequestCtx, source: HarvestSource) -> HarvestSource;
        list_harvest_runs<'a>(ctx: &'a RequestCtx, source_id: Option<&'a str>) -> Vec<HarvestRun>;
        get_harvest_run<'a>(ctx: &'a RequestCtx, run_id: &'a str) -> Option<HarvestRun>;
        upsert_harvest_run<'a>(ctx: &'a RequestCtx, run: HarvestRun) -> HarvestRun;
        record_activity<'a>(ctx: &'a RequestCtx, row: ActivityRow) -> ();
        list_activity<'a>(ctx: &'a RequestCtx, object_id: Option<&'a str>, limit: usize) -> Vec<ActivityRow>;
        upsert_shacl_report<'a>(ctx: &'a RequestCtx, report: ShaclValidationReport) -> ShaclValidationReport;
        list_shacl_reports<'a>(ctx: &'a RequestCtx, dataset_id: Option<&'a str>, profile_id: Option<&'a str>) -> Vec<ShaclValidationReport>;
        list_profiles<'a>(ctx: &'a RequestCtx) -> Vec<ProfileRecord>;
        upsert_profile<'a>(ctx: &'a RequestCtx, profile: ProfileRecord) -> ProfileRecord;
        list_pages<'a>(ctx: &'a RequestCtx) -> Vec<StaticPage>;
        upsert_page<'a>(ctx: &'a RequestCtx, page: StaticPage) -> StaticPage;
        list_plugins<'a>(ctx: &'a RequestCtx) -> Vec<CataloguePlugin>;
        upsert_plugin<'a>(ctx: &'a RequestCtx, plugin: CataloguePlugin) -> CataloguePlugin;
        odata_table<'a>(ctx: &'a RequestCtx, set_name: &'a str) -> Arc<dyn odata_table::Table>;
    }
}

// ---------------------------------------------------------------------------------------------
// The catalogue under test.
// ---------------------------------------------------------------------------------------------

struct World {
    store: Arc<FailingStore>,
    app: Router,
    org: OrgId,
    cat: CatalogueId,
}

impl World {
    fn admin_ctx(&self) -> RequestCtx {
        RequestCtx::new(
            self.org,
            self.cat,
            Subject::authenticated("admin", vec![self.org], Vec::new()),
        )
    }
}

fn catalogue(org: OrgId, cat: CatalogueId) -> Catalogue {
    serde_json::from_value(json!({
        "orgId": org, "id": cat, "name": "city", "host": HOST,
        "title": {"sk": "Mestský katalóg", "en": "City catalogue"},
        "description": {"sk": "Otvorené dáta & <mesto>"},
        "defaultLanguage": "sk", "languages": ["sk", "en"],
        "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "hiddenDatasets": {"veiled": "quality check pending"},
        "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap()
}

fn publisher(name: &str, grants: &[&str]) -> Publisher {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"sk": format!("Vydavateľ {name}")}, "description": {},
        "uri": format!("https://example.org/{name}"),
        "state": "active", "editorGrants": grants,
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap()
}

fn dataset(name: &str, publisher: &str, visibility: &str, modified_day: u32) -> Dataset {
    serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "", "name": name,
        "title": {"sk": format!("Dataset {name} & <spol>")},
        "description": {"sk": format!("Popis {name}")},
        "publisherId": publisher, "visibility": visibility, "state": "active",
        "licenceId": "cc-by", "themes": ["http://publications.europa.eu/resource/authority/data-theme/ENVI"],
        "keywords": {"sk": ["ovzdušie"]}, "tags": ["ovzdusie", name],
        "spatial": ["SK032"],
        "distributions": [
            {"id": "", "datasetId": "", "title": {"sk": format!("CSV {name}")},
             "accessUrl": format!("https://files.example.org/{name}.csv"), "format": "CSV"},
            {"id": "", "datasetId": "", "title": {"sk": format!("Mapa {name}")},
             "accessUrl": format!("https://ogc.city.test/collections/{name}"), "format": "GEOJSON"}
        ],
        "customShaclShapes": SHAPES,
        "issued": "2026-11-01T10:00:00Z",
        "modified": format!("2026-11-{modified_day:02}T10:00:00Z")
    }))
    .unwrap()
}

const SHAPES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:S a sh:NodeShape ; sh:targetClass ex:Reading ;
  sh:property [ sh:path ex:value ; sh:minCount 1 ; sh:datatype <http://www.w3.org/2001/XMLSchema#integer> ] .
";

fn view(id: &str, resource: &str) -> ResourceView {
    serde_json::from_value(json!({
        "id": id, "resourceId": resource, "viewType": "datatables_view", "title": "Tabuľka",
        "config": {"filterable": true}
    }))
    .unwrap()
}

fn binding(token: &str, actor: &str, org: OrgId, groups: &[&str]) -> TokenBinding {
    TokenBinding {
        token: token.into(),
        subject: Subject::authenticated(
            actor,
            vec![org],
            groups.iter().map(|g| (*g).to_owned()).collect(),
        ),
    }
}

/// Tokens: `tok-admin` (Admin on the catalogue), `tok-editor` (editor of publisher `city`),
/// `tok-member` (member, no grants), `tok-proposer` (Propose on the catalogue, no Approve),
/// `tok-reader` (a read-only connector).
/// The catalogue on an installation whose platform domain is `city.test`.
async fn world() -> World {
    world_with(|s| {
        router(s.with_installation_branding(BrandingBlock {
            domain: "city.test".into(),
            ..BrandingBlock::default()
        }))
    })
    .await
}

async fn world_with(build: impl FnOnce(DataroomState) -> Router) -> World {
    let org = OrgId::from_uuid(Uuid::now_v7());
    let cat = CatalogueId::from_uuid(Uuid::now_v7());
    let store = Arc::new(FailingStore::new());
    store.upsert_catalogue(catalogue(org, cat)).await.unwrap();
    let ctx = RequestCtx::new(
        org,
        cat,
        Subject::authenticated("seed", vec![org], Vec::new()),
    );
    for p in [
        publisher("city", &["city-editors"]),
        publisher("gone", &[]),
        publisher("old", &[]),
    ] {
        store.upsert_publisher(&ctx, p).await.unwrap();
    }
    for (i, (name, publ, vis)) in [
        ("open", "city", "public"),
        ("second", "city", "public"),
        ("inner", "city", "internal"),
        ("secret", "city", "restricted"),
        ("veiled", "city", "public"),
        ("retired", "city", "public"),
        ("orphan", "gone", "public"),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .upsert_dataset(&ctx, dataset(name, publ, vis, 2 + i as u32))
            .await
            .unwrap();
    }
    store.withdraw_dataset(&ctx, "retired").await.unwrap();
    store.withdraw_publisher(&ctx, "gone").await.unwrap();
    store.merge_publishers(&ctx, "old", "city").await.unwrap();
    for (id, res) in [
        ("view-open", "res-open-0"),
        ("view-secret", "res-secret-0"),
        ("view-veiled", "res-veiled-0"),
        ("view-orphan", "res-orphan-0"),
        ("view-retired", "res-retired-0"),
    ] {
        store
            .upsert_resource_view(&ctx, view(id, res))
            .await
            .unwrap();
    }

    let authz = PolicyAuthorizer::new(vec![
        GrantRule {
            org_id: org,
            catalogue_id: None,
            principal: "admin".into(),
            actions: vec![Action::Admin],
            publisher_id: None,
        },
        GrantRule {
            org_id: org,
            catalogue_id: Some(cat),
            principal: "pat".into(),
            actions: vec![Action::Propose],
            publisher_id: None,
        },
    ]);
    let mut reader = binding("tok-reader", "connector", org, &[]);
    reader.subject.read_only = true;
    reader.subject.client_id = Some("connector".into());
    let state = DataroomState::new(store.clone(), Arc::new(authz), org, cat).with_tokens(vec![
        binding("tok-admin", "admin", org, &[]),
        binding("tok-editor", "eva", org, &["city-editors"]),
        binding("tok-member", "mo", org, &[]),
        binding("tok-proposer", "pat", org, &[]),
        reader,
    ]);
    World {
        store,
        app: build(state),
        org,
        cat,
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP helpers.
// ---------------------------------------------------------------------------------------------

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap())
            .unwrap_or("")
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: Body,
) -> Answer {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .unwrap();
    req.headers_mut()
        .insert(header::HOST, HeaderValue::from_static(HOST));
    if let Some(t) = token {
        req.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {t}").parse().unwrap(),
        );
    }
    for (k, v) in headers {
        req.headers_mut().insert(
            header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn get(app: &Router, uri: &str, token: Option<&str>) -> Answer {
    send(app, Method::GET, uri, token, &[], Body::empty()).await
}

async fn post(app: &Router, uri: &str, token: Option<&str>, body: Value) -> Answer {
    send(
        app,
        Method::POST,
        uri,
        token,
        &[("content-type", "application/json")],
        Body::from(body.to_string()),
    )
    .await
}

async fn ckan(app: &Router, action: &str, token: Option<&str>, body: Value) -> Answer {
    post(app, &format!("/api/3/action/{action}"), token, body).await
}

fn ckan_error(a: &Answer) -> Value {
    let v = a.json();
    assert_eq!(v["success"], false, "{}", a.body);
    v["error"].clone()
}

// ---------------------------------------------------------------------------------------------
// CKAN Action API.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn ckan_resources_and_views_are_shown_where_their_dataset_is() {
    let w = world().await;
    let app = &w.app;
    // Anonymous: only the public, active dataset of an active publisher.
    for (res, view, shown) in [
        ("res-open-0", "view-open", true),
        ("res-secret-0", "view-secret", false),
        ("res-veiled-0", "view-veiled", false),
        ("res-orphan-0", "view-orphan", false),
        ("res-retired-0", "view-retired", false),
    ] {
        let a = ckan(app, "resource_show", None, json!({"id": res})).await;
        assert_eq!(
            a.status == StatusCode::OK,
            shown,
            "resource_show {res}: {}",
            a.body
        );
        if !shown {
            assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
        }
        let a = ckan(app, "resource_view_list", None, json!({"id": res})).await;
        assert_eq!(
            a.status == StatusCode::OK,
            shown,
            "view_list {res}: {}",
            a.body
        );
        let a = ckan(app, "resource_view_show", None, json!({"id": view})).await;
        assert_eq!(
            a.status == StatusCode::OK,
            shown,
            "view_show {view}: {}",
            a.body
        );
    }
    // The publisher's editor sees the hidden dataset's resource and view; a member does not.
    let a = ckan(
        app,
        "resource_show",
        Some("tok-editor"),
        json!({"id": "res-veiled-0"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["result"]["id"], "res-veiled-0");
    let a = ckan(
        app,
        "resource_view_show",
        Some("tok-editor"),
        json!({"id": "view-veiled"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["result"]["view_type"], "datatables_view");
    let a = ckan(
        app,
        "resource_show",
        Some("tok-member"),
        json!({"id": "res-veiled-0"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    // A member may read internal records, not restricted ones.
    let a = ckan(
        app,
        "resource_show",
        Some("tok-member"),
        json!({"id": "res-inner-0"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    let a = ckan(
        app,
        "resource_show",
        Some("tok-admin"),
        json!({"id": "res-secret-1"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["result"]["position"], 1, "{}", a.body);
    let a = ckan(
        app,
        "resource_view_list",
        Some("tok-admin"),
        json!({"id": "res-secret-0"}),
    )
    .await;
    assert_eq!(a.json()["result"][0]["id"], "view-secret", "{}", a.body);
    // Unknown ids and missing ids.
    for action in ["resource_show", "resource_view_list", "resource_view_show"] {
        let a = ckan(app, action, Some("tok-admin"), json!({"id": "nope"})).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{action}: {}", a.body);
        let a = ckan(app, action, Some("tok-admin"), json!({})).await;
        assert_eq!(
            a.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{action}: {}",
            a.body
        );
        assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    }
    // A view whose resource is gone is not shown.
    let a = ckan(
        app,
        "resource_delete",
        Some("tok-admin"),
        json!({"id": "res-open-0"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = ckan(
        app,
        "resource_view_show",
        Some("tok-admin"),
        json!({"id": "view-open"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
}

#[tokio::test]
async fn ckan_writes_on_a_dataset_the_caller_cannot_see_answer_not_found() {
    let w = world().await;
    let app = &w.app;
    let calls: [(&str, Value); 6] = [
        ("dataset_purge", json!({"id": "secret"})),
        (
            "resource_create",
            json!({"package_id": "secret", "url": "https://x.example/a.csv"}),
        ),
        ("resource_patch", json!({"id": "res-secret-0", "name": "x"})),
        ("resource_delete", json!({"id": "res-secret-0"})),
        (
            "resource_view_create",
            json!({"resource_id": "res-secret-0", "view_type": "text_view"}),
        ),
        (
            "resource_view_update",
            json!({"id": "view-secret", "title": "x"}),
        ),
    ];
    for (action, body) in &calls {
        let a = ckan(app, action, Some("tok-member"), body.clone()).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{action}: {}", a.body);
        assert!(!a.body.contains("not visible"), "{action}: {}", a.body);
    }
    let a = ckan(
        app,
        "resource_view_delete",
        Some("tok-member"),
        json!({"id": "view-secret"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    // On a dataset the member can see, the same writes are refused as forbidden.
    for (action, body) in [
        ("dataset_purge", json!({"id": "open"})),
        (
            "resource_create",
            json!({"package_id": "open", "url": "https://x.example/a.csv"}),
        ),
        ("resource_update", json!({"id": "res-open-0", "name": "x"})),
        ("resource_delete", json!({"id": "res-open-0"})),
        (
            "resource_view_create",
            json!({"resource_id": "res-open-0", "view_type": "text_view"}),
        ),
        (
            "resource_view_update",
            json!({"id": "view-open", "title": "x"}),
        ),
        ("resource_view_delete", json!({"id": "view-open"})),
    ] {
        let a = ckan(app, action, Some("tok-member"), body).await;
        assert_eq!(a.status, StatusCode::FORBIDDEN, "{action}: {}", a.body);
        assert_eq!(ckan_error(&a)["__type"], "Authorization Error", "{action}");
    }
    // Nothing changed.
    let ds = w
        .store
        .get_dataset(&w.admin_ctx(), "secret")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ds.distributions.len(), 2);
    assert!(w
        .store
        .get_resource_view(&w.admin_ctx(), "view-secret")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn resource_view_delete_never_skips_its_check() {
    let w = world().await;
    let app = &w.app;
    let ctx = w.admin_ctx();
    // Orphan a view: its resource is deleted, the view stays.
    w.store
        .upsert_resource_view(&ctx, view("view-stale", "res-second-1"))
        .await
        .unwrap();
    let a = ckan(
        app,
        "resource_delete",
        Some("tok-editor"),
        json!({"id": "res-second-1"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    // The orphan is decided on the catalogue: anonymous and plain members are refused.
    for tok in [None, Some("tok-member"), Some("tok-editor")] {
        let a = ckan(
            app,
            "resource_view_delete",
            tok,
            json!({"id": "view-stale"}),
        )
        .await;
        assert_eq!(a.status, StatusCode::FORBIDDEN, "{tok:?}: {}", a.body);
    }
    assert!(w
        .store
        .get_resource_view(&ctx, "view-stale")
        .await
        .unwrap()
        .is_some());
    // A failing lookup refuses instead of deleting unchecked.
    w.store.fail(&["get_distribution"]);
    let a = ckan(
        app,
        "resource_view_delete",
        None,
        json!({"id": "view-open"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", a.body);
    w.store.heal();
    w.store.fail(&["get_publisher"]);
    let a = ckan(
        app,
        "resource_view_delete",
        Some("tok-editor"),
        json!({"id": "view-open"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", a.body);
    w.store.heal();
    assert!(w
        .store
        .get_resource_view(&ctx, "view-open")
        .await
        .unwrap()
        .is_some());
    // The administrator deletes the orphan; the editor deletes a view of their dataset.
    let a = ckan(
        app,
        "resource_view_delete",
        Some("tok-admin"),
        json!({"id": "view-stale"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = ckan(
        app,
        "resource_view_delete",
        Some("tok-editor"),
        json!({"id": "view-open"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(w
        .store
        .get_resource_view(&ctx, "view-open")
        .await
        .unwrap()
        .is_none());
    let a = ckan(
        app,
        "resource_view_delete",
        Some("tok-admin"),
        json!({"id": "view-open"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    let a = ckan(app, "resource_view_delete", Some("tok-admin"), json!({})).await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    // A failing delete surfaces.
    w.store.fail(&["delete_resource_view"]);
    let a = ckan(
        app,
        "resource_view_delete",
        Some("tok-admin"),
        json!({"id": "view-secret"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", a.body);
}

#[tokio::test]
async fn ckan_site_and_listing_actions() {
    let w = world().await;
    let app = &w.app;
    let a = get(app, "/api/3/action/status_show", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.header("cache-control"), "public, max-age=60");
    let v = a.json();
    assert_eq!(v["result"]["site_title"], "Mestský katalóg");
    assert_eq!(v["result"]["site_url"], format!("https://{HOST}"));
    assert_eq!(
        v["help"],
        format!("https://{HOST}/api/3/action/help_show?name=status_show")
    );
    let a = get(app, "/api/action/help_show?name=package_show", None).await;
    assert_eq!(a.json()["result"], "CKAN Action API: package_show");
    let a = get(app, "/api/3/action/help_show", None).await;
    assert_eq!(a.json()["result"], "CKAN Action API: status_show");
    assert_eq!(
        get(app, "/api/3/action/site_read", None).await.json()["result"],
        true
    );
    let a = get(app, "/api/3/action/license_list", None).await;
    assert!(a.json()["result"].as_array().unwrap().len() > 3);
    assert_eq!(
        get(app, "/api/3/action/group_list", None).await.json()["result"],
        json!([])
    );
    // An authenticated GET is not publicly cacheable.
    let a = get(app, "/api/3/action/site_read", Some("tok-member")).await;
    assert_eq!(a.header("cache-control"), "");

    // tag_list: tags of public active datasets only.
    let tags = get(app, "/api/3/action/tag_list", None).await.json()["result"].clone();
    let tags: Vec<&str> = tags
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        tags.contains(&"open") && tags.contains(&"ovzdusie"),
        "{tags:?}"
    );
    assert!(
        !tags.contains(&"inner") && !tags.contains(&"retired"),
        "{tags:?}"
    );

    // package_list: public, active, of an active publisher; offset/limit as numbers or text.
    let a = get(app, "/api/3/action/package_list", None).await;
    assert_eq!(a.json()["result"], json!(["open", "second", "veiled"]));
    let a = get(app, "/api/3/action/package_list?offset=1&limit=1", None).await;
    assert_eq!(a.json()["result"], json!(["second"]));
    let a = ckan(app, "package_list", None, json!({"offset": 2, "limit": 5})).await;
    assert_eq!(a.json()["result"], json!(["veiled"]));

    // organization_list: active publishers, names or full records (bool or text flag).
    let a = get(app, "/api/3/action/organization_list", None).await;
    assert_eq!(
        a.json()["result"],
        json!(["city"]),
        "a merged publisher is not listed"
    );
    let a = get(app, "/api/3/action/organization_list?all_fields=true", None).await;
    let v = a.json();
    assert_eq!(v["result"][0]["name"], "city");
    assert_eq!(v["result"][0]["title"], "Vydavateľ city");
    assert_eq!(v["result"][0]["is_organization"], true);
    let a = ckan(app, "organization_list", None, json!({"all_fields": true})).await;
    assert_eq!(a.json()["result"][0]["type"], "organization");

    // organization_show: package count of public active datasets; withdrawn hidden from anonymous.
    let a = get(app, "/api/3/action/organization_show?id=city", None).await;
    assert_eq!(a.json()["result"]["package_count"], 3, "{}", a.body);
    let a = get(app, "/api/3/action/organization_show?id=gone", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = get(
        app,
        "/api/3/action/organization_show?id=gone",
        Some("tok-member"),
    )
    .await;
    assert_eq!(a.json()["result"]["state"], "deleted", "{}", a.body);
    let a = get(app, "/api/3/action/organization_show?id=nobody", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = get(app, "/api/3/action/organization_show", None).await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");

    // Unknown actions and DataStore actions.
    let a = get(app, "/api/3/action/frobnicate", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert!(ckan_error(&a)["message"]
        .as_str()
        .unwrap()
        .contains("Action name not known: frobnicate"));
    let a = ckan(app, "datastore_search", None, json!({})).await;
    assert_eq!(a.status, StatusCode::NOT_IMPLEMENTED, "{}", a.body);
    assert!(a.body.contains("datastore_search is not supported"));
}

#[tokio::test]
async fn ckan_request_parsing_and_identity() {
    let w = world().await;
    let app = &w.app;
    // A POST whose body is not JSON reads the query string instead.
    let a = send(
        app,
        Method::POST,
        "/api/3/action/package_show?id=open",
        None,
        &[],
        Body::from("not json"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["result"]["name"], "open");
    // An empty POST body reads the query string too.
    let a = send(
        app,
        Method::POST,
        "/api/3/action/package_show?id=open",
        None,
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(a.json()["result"]["name"], "open");
    // `name_or_id` is accepted for package_show, an empty id is missing.
    let a = ckan(app, "package_show", None, json!({"name_or_id": "second"})).await;
    assert_eq!(a.json()["result"]["name"], "second");
    let a = ckan(app, "package_show", None, json!({"id": ""})).await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    // The CKAN API key header carries a token; an unknown token is refused.
    let a = send(
        app,
        Method::GET,
        "/api/3/action/package_show?id=inner",
        None,
        &[("x-ckan-api-key", "tok-editor")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = get(
        app,
        "/api/3/action/package_show?id=open",
        Some("tok-forged"),
    )
    .await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        a.json()["help"],
        "https://dataroom.local/api/3/action/help_show?name=package_show"
    );
    // An unknown host is not a catalogue.
    let a = send(
        app,
        Method::GET,
        "/api/3/action/status_show",
        None,
        &[("host", "elsewhere.test:8080")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert!(
        a.body.contains("unknown catalogue host 'elsewhere.test'"),
        "{}",
        a.body
    );
    // localhost falls back to the configured default catalogue.
    let a = send(
        app,
        Method::GET,
        "/api/3/action/status_show",
        None,
        &[("host", "localhost:5000")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.json()["result"]["site_title"], "Mestský katalóg");
}

#[tokio::test]
async fn ckan_package_show_and_search_hide_what_the_caller_may_not_read() {
    let w = world().await;
    let app = &w.app;
    for (name, anon, member, editor) in [
        ("open", true, true, true),
        ("inner", false, false, true),
        ("secret", false, false, true),
        ("veiled", false, false, true),
        ("retired", false, true, true),
        ("orphan", false, true, true),
    ] {
        for (tok, expect) in [
            (None, anon),
            (Some("tok-member"), member),
            (Some("tok-editor"), editor),
        ] {
            let a = ckan(app, "package_show", tok, json!({"id": name})).await;
            assert_eq!(
                a.status == StatusCode::OK,
                expect,
                "{name} {tok:?}: {}",
                a.body
            );
        }
    }
    let a = get(app, "/api/3/action/package_show?id=open", None).await;
    assert_eq!(a.header("cache-control"), "public, max-age=60");
    let a = get(app, "/api/3/action/package_show?id=missing", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);

    // package_search: paging, sort, facets (array or JSON text), fl=name and flags as text.
    let a = get(
        app,
        "/api/3/action/package_search?q=&rows=1&start=1&sort=name%20asc&fl=name&include_private=false&include_drafts=false",
        None,
    )
    .await;
    let v = a.json();
    assert_eq!(v["result"]["count"], 2, "{}", a.body);
    assert_eq!(v["result"]["results"], json!([{"name": "second"}]));
    assert_eq!(v["result"]["sort"], "name asc");
    let a = ckan(
        app,
        "package_search",
        Some("tok-editor"),
        json!({"include_private": true, "facet.field": ["organization", "tags"], "rows": 50}),
    )
    .await;
    let v = a.json();
    let names: BTreeSet<String> = v["result"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        names.contains("veiled") && names.contains("secret"),
        "{names:?}"
    );
    assert!(
        v["result"]["search_facets"]["organization"].is_object(),
        "{}",
        a.body
    );
    let a = ckan(
        app,
        "package_search",
        Some("tok-member"),
        json!({"include_private": "true", "facet.field": "[\"tags\"]"}),
    )
    .await;
    let v = a.json();
    let names: Vec<&str> = v["result"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"veiled") && !names.contains(&"secret"),
        "{names:?}"
    );
    assert!(
        v["result"]["search_facets"]["tags"].is_object(),
        "{}",
        a.body
    );
    assert_eq!(v["result"]["sort"], "score desc, metadata_modified desc");
    let a = get(
        app,
        "/api/3/action/package_search?fq=organization:city",
        None,
    )
    .await;
    assert_eq!(a.json()["result"]["count"], 2, "{}", a.body);
    let a = get(
        app,
        "/api/3/action/package_search?fq=%7B%21lucene%7Dname:x",
        None,
    )
    .await;
    assert_eq!(
        ckan_error(&a)["fq"][0],
        "Solr local parameters are not allowed in fq",
        "{}",
        a.body
    );
}

#[tokio::test]
async fn ckan_dataset_and_resource_writes() {
    let w = world().await;
    let app = &w.app;
    // package_create refuses a taken name before deciding anything.
    let a = ckan(
        app,
        "package_create",
        Some("tok-editor"),
        json!({"name": "open"}),
    )
    .await;
    assert_eq!(ckan_error(&a)["name"][0], "That URL is already in use.");
    let a = ckan(
        app,
        "package_create",
        Some("tok-editor"),
        json!({"name": "fresh", "title": "Fresh", "notes": "N", "owner_org": "city",
               "private": true, "license_id": "cc-by"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = ckan(
        app,
        "package_create",
        None,
        json!({"name": "anon-ds", "owner_org": "city"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.body);

    // package_update: by id or name; unknown is not found.
    let a = ckan(
        app,
        "package_update",
        Some("tok-editor"),
        json!({"title": "x"}),
    )
    .await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    let a = ckan(
        app,
        "package_update",
        Some("tok-editor"),
        json!({"name": "ghost"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "package_update",
        Some("tok-editor"),
        json!({"name": "fresh", "title": "Fresher", "notes": "N", "owner_org": "city", "private": true}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = ckan(
        app,
        "package_update",
        Some("tok-member"),
        json!({"name": "fresh", "owner_org": "city"}),
    )
    .await;
    assert_ne!(a.status, StatusCode::OK, "{}", a.body);

    // package_patch merges into the stored record.
    let a = ckan(
        app,
        "package_patch",
        Some("tok-editor"),
        json!({"notes": "x"}),
    )
    .await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    let a = ckan(
        app,
        "package_patch",
        Some("tok-editor"),
        json!({"name": "ghost"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "package_patch",
        Some("tok-editor"),
        json!({"name": "fresh", "title": "Patched"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let ds = w
        .store
        .get_dataset(&w.admin_ctx(), "fresh")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ds.title.get("sk").map(String::as_str), Some("Patched"));

    // Resources: create, update, patch, delete, with their errors.
    let a = ckan(
        app,
        "resource_create",
        Some("tok-editor"),
        json!({"url": "https://f.example/a.csv"}),
    )
    .await;
    assert_eq!(ckan_error(&a)["package_id"][0], "Missing value");
    let a = ckan(
        app,
        "resource_create",
        Some("tok-editor"),
        json!({"package_id": "ghost"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "resource_create",
        Some("tok-editor"),
        json!({"package_id": "fresh", "url": "https://f.example/a.csv", "name": "A", "format": "CSV"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let res_id = a.json()["result"]["id"].as_str().unwrap().to_owned();
    assert_eq!(res_id, "res-fresh-0");
    for action in ["resource_update", "resource_patch", "resource_delete"] {
        let a = ckan(app, action, Some("tok-editor"), json!({"name": "x"})).await;
        assert_eq!(ckan_error(&a)["id"][0], "Missing value", "{action}");
        let a = ckan(app, action, Some("tok-editor"), json!({"id": "res-ghost"})).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{action}");
    }
    let a = ckan(
        app,
        "resource_patch",
        Some("tok-editor"),
        json!({"id": res_id, "name": "Renamed"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["result"]["name"], "Renamed");
    assert_eq!(a.json()["result"]["url"], "https://f.example/a.csv");

    // Resource views.
    let a = ckan(
        app,
        "resource_view_create",
        Some("tok-editor"),
        json!({"resource_id": res_id, "view_type": "text_view", "script": "x"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    let a = ckan(
        app,
        "resource_view_create",
        Some("tok-editor"),
        json!({"view_type": "text_view"}),
    )
    .await;
    assert_eq!(ckan_error(&a)["resource_id"][0], "Missing value");
    let a = ckan(
        app,
        "resource_view_create",
        Some("tok-editor"),
        json!({"resource_id": res_id}),
    )
    .await;
    assert_eq!(ckan_error(&a)["view_type"][0], "Missing value");
    let a = ckan(
        app,
        "resource_view_create",
        Some("tok-editor"),
        json!({"resource_id": "ghost", "view_type": "text_view"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "resource_view_create",
        Some("tok-editor"),
        json!({"resource_id": res_id, "view_type": "image_view", "title": "Obrázok",
               "description": "d", "order": 2, "image_url": "https://f.example/i.png"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let v = a.json()["result"].clone();
    let view_id = v["id"].as_str().unwrap().to_owned();
    assert!(view_id.starts_with("view-"), "{v}");
    assert_eq!(v["image_url"], "https://f.example/i.png", "{v}");
    let a = ckan(
        app,
        "resource_view_update",
        Some("tok-editor"),
        json!({"id": view_id, "script": 1}),
    )
    .await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    let a = ckan(
        app,
        "resource_view_update",
        Some("tok-editor"),
        json!({"title": "x"}),
    )
    .await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    let a = ckan(
        app,
        "resource_view_update",
        Some("tok-editor"),
        json!({"id": "view-ghost"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "resource_view_update",
        Some("tok-editor"),
        json!({"id": view_id, "title": "Nový"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["result"]["title"], "Nový");
    assert_eq!(a.json()["result"]["view_type"], "image_view", "{}", a.body);
    // A view whose resource is gone cannot be updated.
    let a = ckan(
        app,
        "resource_delete",
        Some("tok-editor"),
        json!({"id": res_id}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = ckan(
        app,
        "resource_view_update",
        Some("tok-editor"),
        json!({"id": view_id, "title": "x"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);

    // package_delete withdraws, dataset_purge removes.
    let a = ckan(app, "package_delete", Some("tok-editor"), json!({})).await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    let a = ckan(
        app,
        "package_delete",
        Some("tok-editor"),
        json!({"id": "fresh"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = ckan(app, "dataset_purge", Some("tok-editor"), json!({})).await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    let a = ckan(
        app,
        "dataset_purge",
        Some("tok-editor"),
        json!({"id": "ghost"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "dataset_purge",
        Some("tok-editor"),
        json!({"id": "fresh"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(w
        .store
        .get_dataset(&w.admin_ctx(), "fresh")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn ckan_organization_writes() {
    let w = world().await;
    let app = &w.app;
    let a = ckan(app, "organization_create", Some("tok-admin"), json!({})).await;
    assert_eq!(ckan_error(&a)["name"][0], "Missing value");
    let a = ckan(
        app,
        "organization_create",
        Some("tok-admin"),
        json!({"name": "Bad Name"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    let a = ckan(
        app,
        "organization_create",
        Some("tok-admin"),
        json!({"name": "dept", "title": "Odbor", "description": "Popis", "url": "https://dept.example",
               "dcat_type": "http://purl.org/adms/publishertype/LocalAuthority",
               "identifier": "123", "email": "a@dept.example", "image_url": "https://dept.example/l.png",
               "editor_grants": ["dept-editors"]}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let v = a.json()["result"].clone();
    assert_eq!(
        (
            v["id"].as_str(),
            v["title"].as_str(),
            v["description"].as_str()
        ),
        (Some("org-dept"), Some("Odbor"), Some("Popis"))
    );
    assert_eq!(v["uri"], "https://dept.example");
    let a = ckan(
        app,
        "organization_create",
        Some("tok-admin"),
        json!({"name": "dept2", "notes": "N"}),
    )
    .await;
    assert_eq!(
        a.json()["result"]["uri"],
        format!("https://{HOST}/organization/dept2")
    );

    // organization_update / patch: by id or name, an instance renames only its own organization.
    let a = ckan(
        app,
        "organization_patch",
        Some("tok-admin"),
        json!({"title": "x"}),
    )
    .await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    let a = ckan(
        app,
        "organization_patch",
        Some("tok-admin"),
        json!({"name": "ghost"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "organization_update",
        Some("tok-member"),
        json!({"id": "dept", "title": "x"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.body);
    let a = ckan(
        app,
        "organization_patch",
        Some("tok-admin"),
        json!({"name": "dept", "title": "Odbor A", "instance_name": "alpha", "notes": "Nové", "image_url": "https://dept.example/n.png"}),
    )
    .await;
    assert_eq!(a.json()["result"]["title"], "Odbor A", "{}", a.body);
    assert_eq!(a.json()["result"]["description"], "Nové");
    assert_eq!(
        a.json()["result"]["image_url"],
        "https://dept.example/n.png"
    );
    let a = ckan(
        app,
        "organization_patch",
        Some("tok-admin"),
        json!({"id": "org-dept", "title": "Odbor B", "instance_name": "beta"}),
    )
    .await;
    assert_eq!(a.json()["result"]["title"], "Odbor A", "{}", a.body);
    let a = ckan(
        app,
        "organization_patch",
        Some("tok-admin"),
        json!({"id": "dept", "title": "Odbor C", "instance_name": "alpha"}),
    )
    .await;
    assert_eq!(a.json()["result"]["title"], "Odbor C", "{}", a.body);
    let a = ckan(
        app,
        "organization_patch",
        Some("tok-admin"),
        json!({"id": "dept2", "title": "Bez inštancie"}),
    )
    .await;
    assert_eq!(a.json()["result"]["title"], "Bez inštancie", "{}", a.body);

    // organization_delete withdraws.
    let a = ckan(app, "organization_delete", Some("tok-admin"), json!({})).await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value");
    let a = ckan(
        app,
        "organization_delete",
        Some("tok-admin"),
        json!({"id": "ghost"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = ckan(
        app,
        "organization_delete",
        Some("tok-member"),
        json!({"id": "dept"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let a = ckan(
        app,
        "organization_delete",
        Some("tok-admin"),
        json!({"id": "dept"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let p = w
        .store
        .get_publisher(&w.admin_ctx(), "dept")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.state, dataroom_core::RecordState::Withdrawn);
}

#[tokio::test]
async fn ckan_store_failures_answer_with_a_ckan_error_and_change_nothing() {
    let w = world().await;
    let app = &w.app;
    let cases: Vec<(&[&'static str], &str, Option<&str>, Value)> = vec![
        (&["list_datasets"], "tag_list", None, json!({})),
        (&["list_datasets"], "package_list", None, json!({})),
        (
            &["get_dataset"],
            "package_show",
            None,
            json!({"id": "open"}),
        ),
        (
            &["get_publisher"],
            "package_show",
            None,
            json!({"id": "open"}),
        ),
        (&["list_datasets"], "package_search", None, json!({})),
        (&["list_publishers"], "package_search", None, json!({})),
        (
            &["get_dataset"],
            "package_patch",
            Some("tok-editor"),
            json!({"id": "open"}),
        ),
        (
            &["get_dataset"],
            "dataset_purge",
            Some("tok-editor"),
            json!({"id": "open"}),
        ),
        (
            &["purge_dataset"],
            "dataset_purge",
            Some("tok-editor"),
            json!({"id": "open"}),
        ),
        (
            &["get_distribution"],
            "resource_show",
            None,
            json!({"id": "res-open-0"}),
        ),
        (
            &["get_publisher"],
            "resource_show",
            None,
            json!({"id": "res-open-0"}),
        ),
        (
            &["get_dataset"],
            "resource_create",
            Some("tok-editor"),
            json!({"package_id": "open"}),
        ),
        (
            &["upsert_dataset"],
            "resource_create",
            Some("tok-editor"),
            json!({"package_id": "open", "url": "https://f.example/x"}),
        ),
        (
            &["get_distribution"],
            "resource_update",
            Some("tok-editor"),
            json!({"id": "res-open-0"}),
        ),
        (
            &["upsert_dataset"],
            "resource_update",
            Some("tok-editor"),
            json!({"id": "res-open-0", "name": "x"}),
        ),
        (
            &["get_distribution"],
            "resource_delete",
            Some("tok-editor"),
            json!({"id": "res-open-0"}),
        ),
        (
            &["upsert_dataset"],
            "resource_delete",
            Some("tok-editor"),
            json!({"id": "res-open-0"}),
        ),
        (
            &["get_distribution"],
            "resource_view_list",
            None,
            json!({"id": "res-open-0"}),
        ),
        (
            &["list_resource_views"],
            "resource_view_list",
            None,
            json!({"id": "res-open-0"}),
        ),
        (
            &["get_resource_view"],
            "resource_view_show",
            None,
            json!({"id": "view-open"}),
        ),
        (
            &["get_distribution"],
            "resource_view_show",
            None,
            json!({"id": "view-open"}),
        ),
        (
            &["get_publisher"],
            "resource_view_show",
            None,
            json!({"id": "view-open"}),
        ),
        (
            &["get_distribution"],
            "resource_view_create",
            Some("tok-editor"),
            json!({"resource_id": "res-open-0", "view_type": "text_view"}),
        ),
        (
            &["upsert_resource_view"],
            "resource_view_create",
            Some("tok-editor"),
            json!({"resource_id": "res-open-0", "view_type": "text_view"}),
        ),
        (
            &["get_resource_view"],
            "resource_view_update",
            Some("tok-editor"),
            json!({"id": "view-open"}),
        ),
        (
            &["get_distribution"],
            "resource_view_update",
            Some("tok-editor"),
            json!({"id": "view-open"}),
        ),
        (
            &["upsert_resource_view"],
            "resource_view_update",
            Some("tok-editor"),
            json!({"id": "view-open", "title": "x"}),
        ),
        (
            &["get_resource_view"],
            "resource_view_delete",
            Some("tok-editor"),
            json!({"id": "view-open"}),
        ),
        (&["list_publishers"], "organization_list", None, json!({})),
        (
            &["get_publisher"],
            "organization_show",
            None,
            json!({"id": "city"}),
        ),
        (
            &["get_publisher"],
            "organization_create",
            Some("tok-admin"),
            json!({"name": "neworg"}),
        ),
        (
            &["upsert_publisher"],
            "organization_create",
            Some("tok-admin"),
            json!({"name": "neworg"}),
        ),
        (
            &["get_publisher"],
            "organization_patch",
            Some("tok-admin"),
            json!({"id": "city"}),
        ),
        (
            &["upsert_publisher"],
            "organization_patch",
            Some("tok-admin"),
            json!({"id": "city", "title": "x"}),
        ),
        (
            &["get_publisher"],
            "organization_delete",
            Some("tok-admin"),
            json!({"id": "city"}),
        ),
        (
            &["withdraw_publisher"],
            "organization_delete",
            Some("tok-admin"),
            json!({"id": "city"}),
        ),
    ];
    for (methods, action, tok, body) in cases {
        w.store.fail(methods);
        let a = ckan(app, action, tok, body).await;
        w.store.heal();
        assert_eq!(
            a.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{action} with {methods:?}: {}",
            a.body
        );
        let err = ckan_error(&a);
        assert!(
            err["message"].as_str().unwrap().contains("failed"),
            "{action}: {err}"
        );
    }
    // Nothing was written by the refused calls.
    let ctx = w.admin_ctx();
    assert!(w
        .store
        .get_publisher(&ctx, "neworg")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        w.store
            .get_dataset(&ctx, "open")
            .await
            .unwrap()
            .unwrap()
            .distributions
            .len(),
        2
    );
    assert_eq!(
        w.store
            .get_publisher(&ctx, "city")
            .await
            .unwrap()
            .unwrap()
            .state,
        dataroom_core::RecordState::Active
    );
    // A failing host lookup is an error, not a fallback catalogue.
    w.store.fail(&["resolve_host"]);
    let a = get(app, "/api/3/action/status_show", None).await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------------------------
// DCAT feeds and Atom.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn dcat_feeds_negotiate_format_and_profile_and_page() {
    let w = world().await;
    let app = &w.app;
    let a = send(
        app,
        Method::GET,
        "/catalog",
        None,
        &[("accept", "application/ld+json")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(
        a.header("content-type").starts_with("application/ld+json"),
        "{:?}",
        a.headers
    );
    let a = get(app, "/catalog", None).await;
    assert!(a.header("content-type").starts_with("text/turtle"));
    assert!(a.body.contains("files.example.org/open.csv"));
    for leak in [
        "inner.csv",
        "secret.csv",
        "veiled.csv",
        "retired.csv",
        "orphan.csv",
    ] {
        assert!(!a.body.contains(leak), "{leak} in the public feed");
    }
    // No hydra paging without a page parameter on a short feed; with one, first/last links.
    assert!(!a.body.contains("PartialCollectionView"));
    let a = get(app, "/catalog.ttl?page=2", None).await;
    assert!(a.body.contains("PartialCollectionView"), "{}", a.body);
    // The paging block comes first after the prefixes, as on CKAN (G6).
    assert!(
        a.body
            .split("\n\n")
            .nth(1)
            .is_some_and(|b| b.contains("hydra:PagedCollection")),
        "{}",
        a.body
    );
    assert!(a.body.contains("/catalog.ttl?page=1"));
    assert!(
        !a.body.contains("open.csv"),
        "page 2 of a one-page feed is empty"
    );
    for (uri, ct) in [
        ("/catalog.n3", "text/turtle"),
        ("/catalog.jsonld", "application/ld+json"),
        ("/catalog.rdf", "application/rdf+xml"),
        ("/catalog.xml", "application/rdf+xml"),
        ("/catalog.nt", "application/n-triples"),
        ("/profiles/dcat-ap-sk-2.0/catalog.ttl", "text/turtle"),
        ("/profiles/dcat-ap-cz/catalog.jsonld", "application/ld+json"),
        ("/publisher/city/catalog.ttl", "text/turtle"),
        ("/publisher/city/catalog.jsonld", "application/ld+json"),
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}: {}", a.body);
        assert!(
            a.header("content-type").starts_with(ct),
            "{uri}: {:?}",
            a.headers
        );
        assert!(a.header("link").contains("rel=\"profile\""), "{uri}");
        assert!(!a.header("content-profile").is_empty(), "{uri}");
    }
    let a = get(app, "/profiles/dcat-ap-sk-2.0/catalog.ttl", None).await;
    assert!(
        a.header("content-profile").contains("sk"),
        "{:?}",
        a.headers
    );
    let a = send(
        app,
        Method::GET,
        "/catalog.ttl?_profile=dcat-ap-cz",
        None,
        &[],
        Body::empty(),
    )
    .await;
    assert!(
        a.header("content-profile").to_lowercase().contains("cz")
            || a.header("content-profile").contains("ofn"),
        "{:?}",
        a.headers
    );
    // A merged publisher's feed redirects; a withdrawn or unknown one is not found.
    let a = get(app, "/publisher/old/catalog.ttl", None).await;
    assert_eq!(a.status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(a.header("location"), "/publisher/city/catalog.ttl");
    let a = get(app, "/publisher/old/catalog.jsonld", None).await;
    assert_eq!(a.header("location"), "/publisher/city/catalog.jsonld");
    let a = get(app, "/publisher/gone/catalog.ttl", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = get(app, "/publisher/nobody/catalog.ttl", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn long_feeds_page_with_next_and_previous_links() {
    let w = world().await;
    let ctx = w.admin_ctx();
    for i in 0..101 {
        let mut ds = dataset(&format!("bulk-{i:03}"), "city", "public", 1);
        ds.distributions.truncate(1);
        ds.custom_shacl_shapes = None;
        w.store.upsert_dataset(&ctx, ds).await.unwrap();
    }
    let app = &w.app;
    let a = get(app, "/catalog.nt", None).await;
    assert!(a.body.contains("catalog.nt?page=2"), "next page link");
    let a = get(app, "/catalog.nt?page=2", None).await;
    assert!(a.body.contains("catalog.nt?page=1"), "previous page link");
    let a = get(app, "/feeds/dataset.atom", None).await;
    assert!(
        a.body
            .contains("rel=\"next\" href=\"https://data.city.test/feeds/dataset.atom?page=2\""),
        "{}",
        &a.body[..800]
    );
    assert!(!a.body.contains("rel=\"previous\""));
    let a = get(app, "/feeds/dataset.atom?page=2", None).await;
    assert!(a
        .body
        .contains("rel=\"previous\" href=\"https://data.city.test/feeds/dataset.atom?page=1\""));
    assert_eq!(a.body.matches("<entry>").count(), 20);
}

#[tokio::test]
async fn atom_feeds_list_public_datasets_newest_first() {
    let w = world().await;
    let app = &w.app;
    let a = get(app, "/feeds/dataset.atom", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(
        a.header("content-type"),
        "application/atom+xml; charset=utf-8"
    );
    assert!(
        a.body.contains("<title>Mestský katalóg</title>"),
        "{}",
        a.body
    );
    let second = a.body.find("/dataset/second<").unwrap();
    let open = a.body.find("/dataset/open<").unwrap();
    assert!(second < open, "newest first");
    assert!(
        a.body.contains("Dataset open &amp; &lt;spol&gt;"),
        "titles are escaped"
    );
    assert!(!a.body.contains("veiled") && !a.body.contains("inner"));
    let a = get(app, "/feeds/custom.atom", None).await;
    assert_eq!(a.status, StatusCode::OK);
    let a = get(app, "/feeds/organization/city.atom", None).await;
    assert!(
        a.body.contains("<title>city - Publisher: city</title>"),
        "{}",
        a.body
    );
    assert_eq!(a.body.matches("<entry>").count(), 2);
    let a = get(app, "/feeds/organization/city.rss", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = get(app, "/feeds/tag/open.atom", None).await;
    assert_eq!(a.body.matches("<entry>").count(), 1, "{}", a.body);
    let a = get(app, "/feeds/tag/ovzdusie.atom", None).await;
    assert_eq!(a.body.matches("<entry>").count(), 2, "{}", a.body);
    let a = get(app, "/feeds/tag/ovzdusie", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    // An empty feed carries the catalogue's modification time.
    let a = get(app, "/feeds/tag/none.atom", None).await;
    assert!(
        a.body
            .contains("<updated>2026-11-01T10:00:00+00:00</updated>"),
        "{}",
        a.body
    );
}

#[tokio::test]
async fn a_failing_store_never_answers_an_empty_feed() {
    let w = world().await;
    let app = &w.app;
    for method in ["list_datasets", "list_publishers"] {
        w.store.fail(&[method]);
        for uri in [
            "/catalog.ttl",
            "/profiles/dcat-ap-sk-2.0/catalog.ttl",
            "/publisher/city/catalog.ttl",
            "/feeds/dataset.atom",
            "/feeds/organization/city.atom",
            "/feeds/tag/open.atom",
        ] {
            let a = get(app, uri, None).await;
            assert_eq!(
                a.status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{uri} with {method}: {}",
                a.body
            );
            assert_eq!(a.header("content-type"), "application/problem+json");
        }
        w.store.heal();
    }
    w.store.fail(&["resolve_host"]);
    for uri in [
        "/catalog.ttl",
        "/feeds/dataset.atom",
        "/feeds/organization/city.atom",
        "/feeds/tag/open.atom",
    ] {
        assert_eq!(
            get(app, uri, None).await.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{uri}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// OData and OpenAPI.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn odata_rows_follow_package_show() {
    let w = world().await;
    let app = &w.app;
    let names = |a: &Answer| -> BTreeSet<String> {
        a.json()["value"]
            .as_array()
            .unwrap_or_else(|| panic!("{}", a.body))
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let a = get(app, "/odata/v4/Datasets", None).await;
    assert_eq!(
        names(&a),
        BTreeSet::from(["open".to_owned(), "second".to_owned()]),
        "{}",
        a.body
    );
    assert_eq!(a.header("odata-version"), "4.01");
    let a = get(app, "/odata/v4/Datasets", Some("tok-editor")).await;
    assert!(names(&a).contains("veiled"), "{}", a.body);
    let a = get(app, "/odata/v4/Datasets", Some("tok-member")).await;
    let n = names(&a);
    assert!(
        n.contains("orphan") && !n.contains("inner") && !n.contains("veiled"),
        "{n:?}"
    );
    let a = get(app, "/odata/v4/Datasets/$count", None).await;
    assert_eq!(a.body, "2");
    let a = get(app, "/odata/v4/Datasets('pkg-veiled')", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    let a = get(app, "/odata/v4/Datasets('pkg-open')", None).await;
    assert_eq!(a.json()["name"], "open", "{}", a.body);
    let a = get(app, "/odata/v4/Distributions/$count", None).await;
    assert_eq!(a.body, "4", "two datasets with two distributions each");
    let a = get(app, "/odata/v4/Datasets?$count=true&$top=1", None).await;
    let v = a.json();
    assert_eq!(v["@odata.count"], 2, "{}", a.body);
    assert_eq!(v["@odata.context"], "/odata/v4/$metadata#Datasets");
    assert_eq!(v["value"].as_array().unwrap().len(), 1, "{}", a.body);
    // $export and $allowed.
    let a = get(
        app,
        "/odata/v4/Datasets/$export?$filter=name%20eq%20'open'",
        None,
    )
    .await;
    assert_eq!(a.json().as_array().unwrap().len(), 1, "{}", a.body);
    let a = get(app, "/odata/v4/Datasets/$export", None).await;
    assert_eq!(a.json().as_array().unwrap().len(), 2, "{}", a.body);
    let a = get(app, "/odata/v4/Datasets/$export?$filter=(((", None).await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    let a = get(
        app,
        "/odata/v4/Datasets/$allowed?op=read&key=pkg-open",
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = get(app, "/odata/v4/Datasets/$allowed?key=pkg-open&x=1", None).await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    assert!(a.body.contains("op is required"));
    // Malformed and unsupported paths.
    for (uri, status) in [
        (
            "/odata/v4/Datasets?$filter=(((",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        ("/odata/v4/Nothing", StatusCode::UNPROCESSABLE_ENTITY),
        ("/odata/v4/Datasets(1.5)", StatusCode::UNPROCESSABLE_ENTITY),
        ("/odata/v4/Datasets(7)", StatusCode::NOT_FOUND),
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(a.status, status, "{uri}: {}", a.body);
    }
    // A store failure is an error, never an empty set.
    for method in ["list_datasets", "list_publishers", "odata_table"] {
        w.store.fail(&[method]);
        for uri in [
            "/odata/v4/Datasets",
            "/odata/v4/Datasets/$count",
            "/odata/v4/Datasets/$export",
            "/odata/v4/Datasets/$allowed?op=read",
        ] {
            let a = get(app, uri, None).await;
            assert_eq!(
                a.status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{uri} {method}: {}",
                a.body
            );
        }
        w.store.heal();
    }
    w.store.fail(&["resolve_host"]);
    assert_eq!(
        get(app, "/odata/v4/Datasets", None).await.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn openapi_and_metadata_describe_the_entity_sets() {
    let w = world().await;
    let a = get(&w.app, "/api/v1/openapi.json", None).await;
    assert_eq!(a.status, StatusCode::OK);
    let v = a.json();
    assert!(
        v["openapi"].as_str().unwrap().starts_with("3.1"),
        "{}",
        a.body
    );
    let paths: Vec<&String> = v["paths"].as_object().unwrap().keys().collect();
    assert!(
        v["servers"].to_string().contains("/odata/v4"),
        "{}",
        v["servers"]
    );
    assert!(paths.iter().any(|p| p.contains("Datasets")), "{paths:?}");
    assert!(v["components"]["schemas"]["ActivityRow"].is_object());
    let a = get(&w.app, "/odata/v4/$metadata", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.header("content-type"), "application/xml; charset=utf-8");
    assert!(a.body.contains("EntitySet Name=\"Datasets\""), "{}", a.body);
}

// ---------------------------------------------------------------------------------------------
// Portal API, profiles, sample validation, branding, assets.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn portal_catalogue_and_dataset_answer_like_package_show() {
    let w = world().await;
    let app = &w.app;
    let a = get(app, "/api/v1/catalogue", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.header("cache-control"), "public, max-age=60");
    let v = a.json();
    assert_eq!(
        (
            v["total"].as_u64(),
            v["page"].as_u64(),
            v["pageSize"].as_u64()
        ),
        (Some(3), Some(1), Some(20)),
        "{}",
        a.body
    );
    let a = get(
        app,
        "/api/v1/catalogue?q=open&publisher=city&theme=ENVI&format=CSV&licence=cc-by&spatial=SK032&page=1",
        None,
    )
    .await;
    assert_eq!(a.json()["datasets"][0]["name"], "open", "{}", a.body);
    let a = get(app, "/api/v1/catalogue?page=0&year=1999", None).await;
    assert_eq!(
        (a.json()["page"].as_u64(), a.json()["total"].as_u64()),
        (Some(1), Some(0)),
        "{}",
        a.body
    );

    for (name, anon, editor) in [
        ("open", true, true),
        ("veiled", false, true),
        ("orphan", false, true),
        ("secret", false, true),
    ] {
        let a = get(app, &format!("/api/v1/catalogue/datasets/{name}"), None).await;
        assert_eq!(a.status == StatusCode::OK, anon, "{name}: {}", a.body);
        let a = get(
            app,
            &format!("/api/v1/catalogue/datasets/{name}"),
            Some("tok-editor"),
        )
        .await;
        assert_eq!(a.status == StatusCode::OK, editor, "{name}: {}", a.body);
    }
    let a = get(app, "/api/v1/catalogue/datasets/open", None).await;
    assert_eq!(a.json()["name"], "open");
    let a = get(app, "/api/v1/catalogue/datasets/ghost", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert_eq!(a.header("content-type"), "application/problem+json");
    assert_eq!(a.json()["slug"], "not-found", "{}", a.body);

    for m in ["list_datasets", "list_publishers", "resolve_host"] {
        w.store.fail(&[m]);
        assert_eq!(
            get(app, "/api/v1/catalogue", None).await.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{m}"
        );
        w.store.heal();
    }
    for m in ["get_dataset", "get_publisher", "resolve_host"] {
        w.store.fail(&[m]);
        assert_eq!(
            get(app, "/api/v1/catalogue/datasets/open", None)
                .await
                .status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{m}"
        );
        w.store.heal();
    }
}

fn profile(id: &str, version: &str) -> Value {
    json!({
        "id": id, "version": version, "uri": format!("https://profiles.example.org/{id}/{version}"),
        "title": {"en": "City profile"}, "baseProfile": "dcat-ap-3.0.1",
        "shapesTurtle": "@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct: <http://purl.org/dc/terms/> .
<urn:city:DatasetShape> a sh:NodeShape ; sh:targetClass dcat:Dataset ;
  sh:property [ sh:path dct:spatial ; sh:minCount 1 ; sh:name \"Územie\" ] .",
        "builtIn": true
    })
}

#[tokio::test]
async fn profiles_upload_dry_run_and_activation_need_their_rights() {
    let w = world().await;
    let app = &w.app;
    let a = get(app, "/api/v1/profiles", None).await;
    assert_eq!(a.status, StatusCode::OK);
    let v = a.json();
    assert_eq!(v["mainProfile"], "dcat-ap-3.0.1");
    assert!(
        v["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "dcat-ap-sk-2.0"),
        "{}",
        a.body
    );

    // Upload needs Propose; activating with the upload needs Approve as the activate route does.
    let a = post(
        app,
        "/api/v1/profiles",
        Some("tok-member"),
        json!({"profile": profile("city", "1.0.0")}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.body);
    let a = post(
        app,
        "/api/v1/profiles",
        Some("tok-proposer"),
        json!({"profile": profile("city", "1.0.0"), "activate": true}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.body);
    let cat = w.store.get_catalogue(w.org, w.cat).await.unwrap().unwrap();
    assert!(
        cat.active_profiles.is_empty(),
        "a refused upload activates nothing"
    );
    let a = post(
        app,
        "/api/v1/profiles",
        Some("tok-proposer"),
        json!({"profile": profile("city", "1.0.0")}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let v = a.json();
    assert_eq!(
        v["profile"]["builtIn"], false,
        "an upload is never built in"
    );
    assert_eq!(v["dryRun"]["totalDatasets"], 7, "{}", a.body);
    assert!(
        !v["profile"]["formFields"].as_array().unwrap().is_empty(),
        "{}",
        v["profile"]
    );
    let stored = w.store.list_profiles(&w.admin_ctx()).await.unwrap();
    assert!(stored
        .iter()
        .any(|p| p.id == "city" && p.version == "1.0.0"));

    // A published version is immutable, built-in ones included.
    let a = post(
        app,
        "/api/v1/profiles",
        Some("tok-admin"),
        json!({"profile": profile("city", "1.0.0")}),
    )
    .await;
    assert_eq!(a.status, StatusCode::CONFLICT, "{}", a.body);
    let a = post(
        app,
        "/api/v1/profiles",
        Some("tok-admin"),
        json!({"profile": profile("dcat-ap-3.0.1", "3.0.1")}),
    )
    .await;
    assert_eq!(a.status, StatusCode::CONFLICT, "{}", a.body);
    // Bad shapes are refused.
    let mut bad = profile("city", "2.0.0");
    bad["shapesTurtle"] = json!("@prefix owl: <http://www.w3.org/2002/07/owl#> . <urn:x> owl:imports <https://evil.example/s.ttl> .");
    let a = post(
        app,
        "/api/v1/profiles",
        Some("tok-admin"),
        json!({"profile": bad}),
    )
    .await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    // The administrator uploads and activates a new version at once.
    let a = post(
        app,
        "/api/v1/profiles",
        Some("tok-admin"),
        json!({"profile": profile("city", "1.1.0"), "activate": true}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let cat = w.store.get_catalogue(w.org, w.cat).await.unwrap().unwrap();
    assert_eq!(cat.active_profiles.len(), 1);
    assert_eq!(cat.active_profiles[0].version, "1.1.0");

    // Activate: Approve needed, unknown versions refused, make_main switches the main profile.
    let a = post(
        app,
        "/api/v1/profiles/city/activate",
        Some("tok-proposer"),
        json!({"version": "1.0.0"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.body);
    let a = post(
        app,
        "/api/v1/profiles/nothing/activate",
        Some("tok-admin"),
        json!({"version": "9"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    let a = post(
        app,
        "/api/v1/profiles/city/activate",
        Some("tok-admin"),
        json!({"version": "1.0.0", "make_main": true}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let v = a.json();
    assert_eq!(v["catalogue"]["mainProfile"], "city");
    assert_eq!(
        v["catalogue"]["activeProfiles"].as_array().unwrap().len(),
        1,
        "re-activating replaces the pin"
    );
    let reports = w
        .store
        .list_shacl_reports(&w.admin_ctx(), None, Some("city"))
        .await
        .unwrap();
    assert_eq!(reports.len(), 7, "the dry run's reports are stored");

    // Store failures surface instead of answering success.
    for (m, version) in [
        ("list_datasets", "3.0.0"),
        ("list_publishers", "3.1.0"),
        ("upsert_profile", "3.2.0"),
        ("upsert_catalogue", "3.3.0"),
    ] {
        w.store.fail(&[m]);
        let a = post(
            app,
            "/api/v1/profiles",
            Some("tok-admin"),
            json!({"profile": profile("city", version), "activate": true}),
        )
        .await;
        w.store.heal();
        assert_eq!(
            a.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{m}: {}",
            a.body
        );
    }
    w.store.fail(&["upsert_catalogue"]);
    let a = post(
        app,
        "/api/v1/profiles/city/activate",
        Some("tok-admin"),
        json!({"version": "1.0.0"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", a.body);
    w.store.heal();
    w.store.fail(&["resolve_host"]);
    assert_eq!(
        get(app, "/api/v1/profiles", None).await.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        post(
            app,
            "/api/v1/profiles",
            None,
            json!({"profile": profile("x", "1")})
        )
        .await
        .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        post(
            app,
            "/api/v1/profiles/city/activate",
            None,
            json!({"version": "1"})
        )
        .await
        .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn sample_validation_uses_shapes_only_of_a_dataset_the_caller_sees() {
    let w = world().await;
    let app = &w.app;
    let good = "@prefix ex: <http://example.org/> . ex:r1 a ex:Reading ; ex:value 42 .";
    let bad = "@prefix ex: <http://example.org/> . ex:r1 a ex:Reading .";
    let a = post(
        app,
        "/api/v1/datasets/open/validate-sample",
        None,
        json!({"sample_turtle": good}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["conforms"], true, "{}", a.body);
    let a = post(
        app,
        "/api/v1/datasets/open/validate-sample",
        None,
        json!({"sample_turtle": bad}),
    )
    .await;
    assert_eq!(a.json()["conforms"], false, "{}", a.body);
    // A dataset the caller may not see is not found, its shapes unused.
    for name in ["secret", "veiled", "orphan", "ghost"] {
        let a = post(
            app,
            &format!("/api/v1/datasets/{name}/validate-sample"),
            None,
            json!({"sample_turtle": good}),
        )
        .await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{name}: {}", a.body);
    }
    let a = post(
        app,
        "/api/v1/datasets/secret/validate-sample",
        Some("tok-editor"),
        json!({"sample_turtle": good}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    // Shapes sent with the request; a dataset without shapes; broken input.
    let ctx = w.admin_ctx();
    let mut plain = w.store.get_dataset(&ctx, "second").await.unwrap().unwrap();
    plain.custom_shacl_shapes = None;
    w.store.upsert_dataset(&ctx, plain).await.unwrap();
    let a = post(
        app,
        "/api/v1/datasets/second/validate-sample",
        None,
        json!({"sample_turtle": good}),
    )
    .await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    assert!(a.body.contains("dataset has no custom SHACL shapes"));
    let a = post(
        app,
        "/api/v1/datasets/second/validate-sample",
        None,
        json!({"sample_turtle": bad, "shapes_turtle": SHAPES}),
    )
    .await;
    assert_eq!(a.json()["conforms"], false, "{}", a.body);
    let a = post(
        app,
        "/api/v1/datasets/open/validate-sample",
        None,
        json!({"sample_turtle": "this is <not turtle"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    for m in ["get_dataset", "get_publisher", "resolve_host"] {
        w.store.fail(&[m]);
        let a = post(
            app,
            "/api/v1/datasets/open/validate-sample",
            None,
            json!({"sample_turtle": good}),
        )
        .await;
        w.store.heal();
        assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR, "{m}");
    }
}

#[tokio::test]
async fn branding_logo_and_static_assets() {
    let w = world().await;
    let app = &w.app;
    let a = get(app, "/api/v1/branding", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.json()["domain"], "city.test", "{}", a.body);
    let a = post(
        app,
        "/api/v1/branding",
        Some("tok-editor"),
        json!({"noindex": true}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let a = post(
        app,
        "/api/v1/branding",
        Some("tok-admin"),
        json!({"noindex": true}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.json()["noindex"], true, "{}", a.body);
    let rows = w
        .store
        .list_activity(&w.admin_ctx(), None, 10)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.operation == "branding.propose" && r.actor == "admin"),
        "{rows:?}"
    );
    let a = get(app, "/robots.txt", None).await;
    assert_eq!(a.body, "User-agent: *\nDisallow: /\n");
    w.store.fail(&["upsert_catalogue"]);
    assert_eq!(
        post(app, "/api/v1/branding", Some("tok-admin"), json!({}))
            .await
            .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    w.store.heal();

    // Logo upload: Propose on the catalogue, a scripted SVG refused, a clean one served by hash.
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"10\" height=\"10\" fill=\"#123456\"/></svg>";
    let upload = |tok: Option<&'static str>, body: &'static str| {
        send(
            app,
            Method::POST,
            "/api/v1/assets/logo",
            tok,
            &[("content-type", "image/svg+xml")],
            Body::from(body),
        )
    };
    assert_eq!(
        upload(Some("tok-member"), svg).await.status,
        StatusCode::FORBIDDEN
    );
    let a = upload(
        Some("tok-proposer"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
    )
    .await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    let a = upload(Some("tok-proposer"), svg).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let url = a.json()["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("/assets/"), "{url}");
    let cat = w.store.get_catalogue(w.org, w.cat).await.unwrap().unwrap();
    assert_eq!(cat.theme.logo.as_deref(), Some(url.as_str()));
    let a = get(app, &url, None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(a.header("content-security-policy").contains("sandbox"));
    assert_eq!(a.header("x-content-type-options"), "nosniff");
    assert_eq!(
        get(app, "/assets/nothing.svg", None).await.status,
        StatusCode::NOT_FOUND
    );
    w.store.fail(&["upsert_catalogue"]);
    let a = send(
        app,
        Method::POST,
        "/api/v1/assets/logo",
        Some("tok-admin"),
        &[],
        Body::from(svg),
    )
    .await;
    assert_eq!(
        a.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a logo the catalogue did not keep is not reported as saved: {}",
        a.body
    );
    w.store.heal();

    for (uri, ct) in [
        ("/jc-theme.css", "text/css; charset=utf-8"),
        ("/jc-enums.json", "application/json; charset=utf-8"),
        ("/jc-logo.svg", "image/svg+xml; charset=utf-8"),
        ("/logo.svg", "image/svg+xml; charset=utf-8"),
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(
            (a.status, a.header("content-type")),
            (StatusCode::OK, ct),
            "{uri}"
        );
        assert!(!a.body.is_empty());
    }

    // Login goes to the platform's identity; without a domain it goes home.
    let a = get(app, "/user/login", None).await;
    assert_eq!(a.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(a.header("location"), "https://portal.city.test/login");
    let a = get(app, "/en/user/login", None).await;
    assert_eq!(a.header("location"), "https://portal.city.test/login");
    let plain = world_with(router).await;
    let a = get(&plain.app, "/user/login", None).await;
    assert_eq!(a.header("location"), "/");
    for m in ["resolve_host"] {
        w.store.fail(&[m]);
        for uri in [
            "/api/v1/branding",
            "/user/login",
            "/robots.txt",
            "/sitemap.xml",
        ] {
            assert_eq!(
                get(app, uri, None).await.status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{uri}"
            );
        }
        assert_eq!(
            post(app, "/api/v1/branding", Some("tok-admin"), json!({}))
                .await
                .status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            upload(Some("tok-admin"), svg).await.status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        w.store.heal();
    }
}

// ---------------------------------------------------------------------------------------------
// Server-rendered pages.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn pages_show_a_dataset_its_resources_and_its_map_where_package_show_would() {
    let w = world().await;
    let app = &w.app;
    for (name, anon, editor) in [
        ("open", true, true),
        ("veiled", false, true),
        ("secret", false, true),
        ("orphan", false, true),
        ("retired", false, false),
    ] {
        for uri in [
            format!("/dataset/{name}"),
            format!("/dataset/{name}/resource/res-{name}-0"),
            format!("/map/{name}"),
            format!("/map/{name}/res-{name}-1"),
        ] {
            let a = get(app, &uri, None).await;
            assert_eq!(
                a.status == StatusCode::OK,
                anon,
                "{uri} anonymous: {}",
                &a.body[..a.body.len().min(300)]
            );
            let a = get(app, &uri, Some("tok-editor")).await;
            assert_eq!(
                a.status == StatusCode::OK,
                editor,
                "{uri} editor: {}",
                &a.body[..a.body.len().min(300)]
            );
        }
    }
    let a = get(app, "/dataset/open/resource/res-open-0", None).await;
    assert!(a.header("content-type").starts_with("text/html"));
    assert!(a.header("content-security-policy").contains("nonce-"));
    assert!(a.body.contains("files.example.org/open.csv"));
    let a = get(app, "/map/open", None).await;
    assert!(
        a.header("content-security-policy")
            .contains("frame-ancestors"),
        "{:?}",
        a.headers
    );
    for uri in [
        "/dataset/ghost/resource/x",
        "/dataset/open/resource/res-ghost",
        "/map/ghost",
        "/map/ghost/x",
    ] {
        assert_eq!(
            get(app, uri, None).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    // The dataset page in RDF, by extension or by Accept, with profile negotiation.
    for (uri, ct) in [
        ("/dataset/open.ttl", "text/turtle"),
        ("/dataset/open.jsonld", "application/ld+json"),
        ("/dataset/open.rdf", "application/rdf+xml"),
        ("/dataset/open.nt", "application/n-triples"),
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}");
        assert!(
            a.header("content-type").starts_with(ct),
            "{uri}: {:?}",
            a.headers
        );
        assert!(a.header("link").contains("rel=\"profile\""));
    }
    let a = send(
        app,
        Method::GET,
        "/dataset/open?_profile=dcat-ap-sk-2.0",
        None,
        &[("accept", "text/turtle")],
        Body::empty(),
    )
    .await;
    assert!(a.header("content-type").starts_with("text/turtle"));
    assert!(
        a.header("content-profile").contains("sk"),
        "{:?}",
        a.headers
    );
    let a = send(
        app,
        Method::GET,
        "/dataset/secret.ttl",
        None,
        &[("accept-profile", "<https://ofn.gov.cz/dcat-ap-cz/>")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    // The editor sees the SHACL panel data; store failures are errors.
    let a = get(app, "/dataset/open", Some("tok-editor")).await;
    assert_eq!(a.status, StatusCode::OK);
    w.store.fail(&["get_publisher"]);
    for uri in [
        "/dataset/open",
        "/dataset/open/resource/res-open-0",
        "/map/open",
    ] {
        let a = get(app, uri, None).await;
        assert_ne!(
            a.status,
            StatusCode::OK,
            "{uri} with a failing publisher lookup"
        );
    }
    w.store.heal();
}

#[tokio::test]
async fn topics_page_lists_the_eu_themes_and_a_topic_opens_its_search() {
    // G3: CKAN's `/group/` ("Témy"); `/group/{name}` is the topic's dataset search.
    let w = world().await;
    let app = &w.app;
    for uri in [
        "/group/",
        "/group",
        "/sk/group/",
        "/en/group/?q=env&sort=title%20desc",
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}");
        assert!(a.header("content-type").starts_with("text/html"), "{uri}");
        assert!(
            a.header("content-security-policy")
                .contains("script-src 'self' 'nonce-"),
            "{uri}: the page CSP"
        );
        assert!(a.body.contains("class=\"media-item\""), "{uri}");
    }
    let a = get(app, "/group/", None).await;
    assert_eq!(a.body.matches("class=\"media-item\"").count(), 13);
    for (uri, to) in [
        ("/group/ENVI", "/dataset/?vocab_jc_topic=ENVI"),
        ("/sk/group/ENVI", "/sk/dataset/?vocab_jc_topic=ENVI"),
        ("/group/a%26b", "/dataset/?vocab_jc_topic=a%26b"),
        (
            "/group/a&organization=x",
            "/dataset/?vocab_jc_topic=a%26organization%3Dx",
        ),
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(
            (a.status, a.header("location")),
            (StatusCode::FOUND, to),
            "{uri}"
        );
    }
    // A publisher page past its last page lists nothing and still answers.
    let a = get(app, "/organization/city?page=2", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(!a.body.contains("/dataset/open\""));
    let a = get(app, "/organization/city?page=1", None).await;
    assert!(a.body.contains("/dataset/open\""));
}

#[tokio::test]
async fn pages_lists_publishers_static_pages_and_routing() {
    let w = world().await;
    let app = &w.app;
    let a = get(app, "/", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(a.body.contains("Mestský katalóg"));
    let a = get(app, "/en/?q=open", None).await;
    assert_eq!(a.status, StatusCode::OK);
    let a = get(app, "/dataset?q=open&organization=city&vocab_jc_topic=ENVI&tags=open&res_format=CSV&license_id=cc-by&spatial=SK032&page=1&sort=name%20asc", None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(
        a.body.contains("/dataset/open"),
        "the filters keep the matching dataset"
    );
    assert!(!a.body.contains("/dataset/second\""), "and drop the other");
    let a = get(app, "/dataset?publisher=city&theme=ENVI&keyword=open&vocab_jc_format=CSV&licence=cc-by&page=x&lang=en", None).await;
    assert_eq!(a.status, StatusCode::OK);
    let a = get(app, "/dataset/", None).await;
    assert!(
        !a.body.contains("/dataset/veiled\""),
        "hidden from the public list"
    );
    let a = get(app, "/dataset", Some("tok-editor")).await;
    assert!(a.body.contains("/dataset/veiled"), "shown to its editor");
    let a = get(app, "/dataset", Some("tok-member")).await;
    assert!(!a.body.contains("/dataset/veiled\""));
    for uri in [
        "/organization",
        "/publisher?q=city&sort=title%20desc",
        "/organization/city",
        "/publisher/city?q=open&tags=open&res_format=CSV&license_id=cc-by&sort=name%20asc",
        "/about",
        "/connector",
        "/privacy",
        "/terms",
        "/support",
        "/accessibility",
        "/assistants",
        "/sk/about",
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(
            a.status,
            StatusCode::OK,
            "{uri}: {}",
            &a.body[..a.body.len().min(300)]
        );
        assert!(a.header("content-type").starts_with("text/html"), "{uri}");
    }
    let a = get(app, "/organization/city", None).await;
    assert!(a.body.contains("/dataset/open") && !a.body.contains("/dataset/veiled\""));
    // Merged publishers redirect, withdrawn ones are hidden from anonymous callers.
    let a = get(app, "/organization/old", None).await;
    assert_eq!(
        (a.status, a.header("location")),
        (StatusCode::PERMANENT_REDIRECT, "/organization/city")
    );
    let a = get(app, "/publisher/old", None).await;
    assert_eq!(a.header("location"), "/publisher/city");
    assert_eq!(
        get(app, "/organization/gone", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(app, "/organization/gone", Some("tok-member"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        get(app, "/organization/ghost", None).await.status,
        StatusCode::NOT_FOUND
    );
    // Assistant guides, unknown pages, unknown language prefixes, other methods.
    let a = get(app, "/assistants/nope", None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert_eq!(
        get(app, "/no/such/page/here", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(app, "/xx/dataset", None).await.status,
        StatusCode::NOT_FOUND
    );
    let a = send(
        app,
        Method::DELETE,
        "/dataset/open",
        None,
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(
        (a.status, a.header("allow")),
        (StatusCode::METHOD_NOT_ALLOWED, "GET, HEAD")
    );
    let a = send(app, Method::HEAD, "/about", None, &[], Body::empty()).await;
    assert_eq!(a.status, StatusCode::OK);
    // Sitemap and robots.
    let a = get(app, "/sitemap.xml", None).await;
    assert_eq!(a.header("content-type"), "application/xml; charset=utf-8");
    assert!(
        a.body.contains("/dataset/open<") && !a.body.contains("/dataset/inner<"),
        "{}",
        a.body
    );
    let a = get(app, "/robots.txt", None).await;
    assert!(
        a.body
            .contains(&format!("Sitemap: https://{HOST}/sitemap.xml")),
        "{}",
        a.body
    );
    // Store failures on a page are errors.
    w.store.fail(&["resolve_host"]);
    assert_eq!(
        get(app, "/", None).await.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        get(app, "/about", None).await.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// Every theme script a page names (`<script src="{mount}/jc-*.js">`) is served under the mount
/// as JavaScript; the home page names jc-dock, jc-copy and jc-forms, which 404ed before.
#[tokio::test]
async fn the_scripts_a_page_names_are_served_under_the_mount() {
    let w = world_with(|s| mount("/opendata", s)).await;
    let page = get(&w.app, "/opendata/", None).await;
    assert_eq!(page.status, StatusCode::OK);
    let named: Vec<&str> = page
        .body
        .split("src=\"/opendata/")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter(|src| src.ends_with(".js"))
        .collect();
    for script in ["jc-dock.js", "jc-copy.js", "jc-forms.js"] {
        assert!(
            named.contains(&script),
            "the page names {script}: {named:?}"
        );
    }
    for script in named {
        let a = get(&w.app, &format!("/opendata/{script}"), None).await;
        assert_eq!(
            (a.status, a.header("content-type")),
            (StatusCode::OK, "text/javascript; charset=utf-8"),
            "{script}"
        );
        assert!(!a.body.is_empty(), "{script}");
    }
    assert_eq!(
        get(&w.app, "/opendata/jc-unknown.js", None)
            .await
            .header("content-type"),
        get(&w.app, "/opendata/no-such-page", None)
            .await
            .header("content-type"),
        "a script not in the list is no script"
    );
}

#[tokio::test]
async fn language_redirect_stays_local() {
    let w = world_with(|s| mount("/catalogue", s)).await;
    let app = &w.app;
    let form = |body: &'static str, uri: &'static str| {
        send(
            app,
            Method::POST,
            uri,
            None,
            &[("content-type", "application/x-www-form-urlencoded")],
            Body::from(body),
        )
    };
    let a = form(
        "url=%2Fcatalogue%2Fen%2Fdataset",
        "/catalogue/util/redirect",
    )
    .await;
    assert_eq!(
        (a.status, a.header("location")),
        (StatusCode::SEE_OTHER, "/catalogue/en/dataset")
    );
    assert_eq!(
        a.header("set-cookie"),
        "ckan_lang=en; Path=/catalogue; SameSite=Lax"
    );
    let a = form("url=%2F%2Fevil.example%2F", "/catalogue/sk/util/redirect").await;
    assert_eq!(a.header("location"), "/catalogue/");
    assert_eq!(a.header("set-cookie"), "");
    let a = form("url=%2Fcatalogue%2Fdataset", "/catalogue/util/redirect").await;
    assert_eq!(a.header("location"), "/catalogue/dataset");
    assert_eq!(a.header("set-cookie"), "");
    // The mounted root answers with and without the trailing slash.
    assert_eq!(get(app, "/catalogue/", None).await.status, StatusCode::OK);
    assert_eq!(get(app, "/catalogue", None).await.status, StatusCode::OK);
    assert_eq!(
        get(app, "/catalogue/dataset/open", None).await.status,
        StatusCode::OK
    );
    let a = get(app, "/catalogue/organization/old", None).await;
    assert_eq!(a.header("location"), "/catalogue/organization/city");
}

// ---------------------------------------------------------------------------------------------
// Operation registry, allowed, MCP and the host-supplied identity.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn operations_allowed_and_mcp_over_http() {
    let w = world().await;
    let app = &w.app;
    let a = post(
        app,
        "/api/v1/ops/dataset.get",
        None,
        json!({"name": "open"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = post(
        app,
        "/api/v1/ops/dataset.get",
        None,
        json!({"name": "secret"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    let a = post(app, "/api/v1/ops/no.such", None, json!({})).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    let a = get(
        app,
        "/api/v1/allowed?operation=dataset.withdraw&object=open",
        Some("tok-editor"),
    )
    .await;
    assert_eq!(a.json()["allowed"], true, "{}", a.body);
    let a = get(
        app,
        "/api/v1/allowed?operation=dataset.withdraw&object=open",
        Some("tok-member"),
    )
    .await;
    assert_eq!(a.json()["allowed"], false, "{}", a.body);
    let a = get(app, "/api/v1/allowed?operation=nothing", None).await;
    assert_eq!(a.json()["allowed"], false, "{}", a.body);
    assert_eq!(a.json()["reason"], "unknown operation nothing");
    let a = post(
        app,
        "/mcp",
        None,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(a.body.contains("jc_dataset_search"), "{}", a.body);
    w.store.fail(&["resolve_host"]);
    assert_eq!(
        post(
            app,
            "/api/v1/ops/dataset.get",
            None,
            json!({"name": "open"})
        )
        .await
        .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        get(app, "/api/v1/allowed?operation=dataset.get", None)
            .await
            .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        post(
            app,
            "/mcp",
            None,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
        )
        .await
        .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn a_host_supplied_identity_comes_first_and_tokens_still_work() {
    let w = world_with(|s| {
        let org = s.default_org_id;
        router(s.with_subject_resolver(move |h| {
            h.get("x-portal-session")
                .filter(|v| v.as_bytes() == b"valid")
                .map(|_| Subject::authenticated("admin", vec![org], Vec::new()))
        }))
    })
    .await;
    let app = &w.app;
    let show = |headers: &'static [(&'static str, &'static str)], tok: Option<&'static str>| {
        send(
            app,
            Method::GET,
            "/api/3/action/package_show?id=secret",
            tok,
            headers,
            Body::empty(),
        )
    };
    assert_eq!(
        show(&[("x-portal-session", "valid")], None).await.status,
        StatusCode::OK
    );
    assert_eq!(
        show(&[("x-portal-session", "forged")], None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(show(&[], Some("tok-editor")).await.status, StatusCode::OK);
    assert_eq!(
        show(&[], Some("tok-forged")).await.status,
        StatusCode::UNAUTHORIZED
    );
    // A session overrides a token, even a bad one.
    assert_eq!(
        show(&[("x-portal-session", "valid")], Some("tok-forged"))
            .await
            .status,
        StatusCode::OK
    );
    // Client `x-jc-*` headers are never trusted.
    assert_eq!(
        show(&[("x-jc-org", "anything"), ("x-jc-subject", "admin")], None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_missing_default_catalogue_is_not_found() {
    let store = Arc::new(MemoryStore::new());
    let state = DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::default()),
        OrgId::from_uuid(Uuid::now_v7()),
        CatalogueId::from_uuid(Uuid::now_v7()),
    );
    let app = router(state);
    let a = send(
        &app,
        Method::GET,
        "/api/v1/catalogue",
        None,
        &[("host", "127.0.0.1:8080")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert!(a.body.contains("default catalogue not found"), "{}", a.body);
}

#[tokio::test]
async fn solr_local_parameters_in_a_query_are_refused_everywhere() {
    let w = world().await;
    let app = &w.app;
    let a = get(app, "/api/3/action/package_search?q=%7B%21lucene%7Dx", None).await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    assert!(ckan_error(&a)["q"][0]
        .as_str()
        .unwrap()
        .contains("Solr local parameters"));
    let a = get(app, "/api/v1/catalogue?q=%7B%21lucene%7Dx", None).await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    // The pages answer an empty result rather than an error page.
    for uri in [
        "/dataset?q=%7B%21lucene%7Dx",
        "/organization/city?q=%7B%21lucene%7Dx",
    ] {
        let a = get(app, uri, None).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}");
        assert!(!a.body.contains("/dataset/open\""), "{uri} lists nothing");
    }
}

#[tokio::test]
async fn edge_answers_of_feeds_odata_pages_and_ckan() {
    let w = world().await;
    let app = &w.app;
    let ctx = w.admin_ctx();
    // A publisher feed by the publisher's id, beside another publisher's dataset; ties in the
    // modification time are ordered by name.
    w.store
        .upsert_publisher(&ctx, publisher("dept", &[]))
        .await
        .unwrap();
    w.store
        .upsert_dataset(&ctx, dataset("dept-data", "dept", "public", 3))
        .await
        .unwrap();
    w.store
        .upsert_dataset(&ctx, dataset("also-open", "city", "public", 2))
        .await
        .unwrap();
    let a = get(app, "/publisher/pub-city/catalog.ttl", None).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(
        a.body.contains("open.csv") && !a.body.contains("dept-data.csv"),
        "{}",
        a.body
    );
    let a = get(app, "/feeds/organization/pub-city.atom", None).await;
    let also = a.body.find("/dataset/also-open<").unwrap();
    let open = a.body.find("/dataset/open<").unwrap();
    assert!(also < open, "same time: by name");
    let a = get(app, "/feeds/tag/ovzdusie.atom", None).await;
    assert!(a.body.find("/dataset/also-open<").unwrap() < a.body.find("/dataset/open<").unwrap());
    let a = send(
        app,
        Method::GET,
        "/catalog.ttl",
        None,
        &[(
            "accept-profile",
            "<https://data.slovensko.sk/def/dcat-ap-sk/2.0>",
        )],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(!a.header("content-profile").is_empty());

    // OData: the service root and paths without an entity set are not supported; a bad skip
    // token is refused; a long set pages with a nextLink.
    for uri in [
        "/odata/v4/",
        "/odata/v4/$count",
        "/odata/v4/Datasets('pkg-open')/Distributions",
        "/odata/v4/Datasets?$skiptoken=garbage",
    ] {
        let a = get(app, uri, None).await;
        assert!(a.status.is_client_error(), "{uri}: {} {}", a.status, a.body);
    }
    for i in 0..101 {
        let mut ds = dataset(&format!("bulk-{i:03}"), "city", "public", 1);
        ds.distributions.truncate(1);
        w.store.upsert_dataset(&ctx, ds).await.unwrap();
    }
    let a = get(app, "/odata/v4/Datasets", None).await;
    let v = a.json();
    assert_eq!(v["value"].as_array().unwrap().len(), 100);
    let next = v["@odata.nextLink"].as_str().unwrap().to_owned();
    assert!(next.starts_with("/odata/v4/Datasets?$skiptoken="), "{next}");
    let a = get(app, &next, None).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(
        !a.json()["value"].as_array().unwrap().is_empty(),
        "{}",
        a.body
    );

    // CKAN: a failing view lookup leaves package_show without views, a failing name check
    // in package_create refuses.
    w.store.fail(&["list_resource_views"]);
    let a = get(app, "/api/3/action/package_show?id=open", None).await;
    assert_eq!(a.status, StatusCode::OK);
    let a = get(app, "/dataset/open", None).await;
    assert_eq!(a.status, StatusCode::OK);
    w.store.heal();
    w.store.fail(&["get_dataset"]);
    let a = ckan(
        app,
        "package_create",
        Some("tok-editor"),
        json!({"name": "new-one", "owner_org": "city"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", a.body);
    w.store.heal();
    // A JSON body that is not an object carries no id.
    let a = send(
        app,
        Method::POST,
        "/api/3/action/package_patch?id=open",
        Some("tok-editor"),
        &[],
        Body::from("[1]"),
    )
    .await;
    assert_eq!(ckan_error(&a)["id"][0], "Missing value", "{}", a.body);
    let a = ckan(app, "resource_patch", Some("tok-editor"), json!({"id": "res-open-0", "name": "x", "name_translated": {"sk": "Preložené", "en": "Translated"}})).await;
    assert_eq!(
        a.json()["result"]["name"],
        "Preložené",
        "an explicit per-language map wins: {}",
        a.body
    );
    w.store.fail(&["get_publisher"]);
    let a = ckan(app, "resource_view_list", None, json!({"id": "res-open-0"})).await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR);
    w.store.heal();

    // allowed surfaces a failing lookup; the default catalogue lookup can fail too.
    w.store.fail(&["get_dataset"]);
    let a = get(
        app,
        "/api/v1/allowed?operation=dataset.get&object=open",
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", a.body);
    w.store.heal();
    w.store.fail(&["get_catalogue"]);
    let a = send(
        app,
        Method::GET,
        "/about",
        None,
        &[("host", "localhost")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR);
    w.store.heal();

    // Pages: a language cookie and Accept-Language pick the language.
    let a = send(
        app,
        Method::GET,
        "/about",
        None,
        &[("cookie", "ckan_lang=en"), ("accept-language", "en")],
        Body::empty(),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(a.body.contains("lang=\"en\""), "{}", &a.body[..400]);
}

#[tokio::test]
async fn mounting_at_the_root_is_the_plain_router() {
    let w = world_with(|s| mount("/", s)).await;
    let a = get(&w.app, "/", None).await;
    assert_eq!(a.status, StatusCode::OK);
    let a = get(&w.app, "/organization/old", None).await;
    assert_eq!(a.header("location"), "/organization/city");
}

#[tokio::test]
async fn a_domain_that_is_no_header_value_never_becomes_a_redirect() {
    let w = world_with(|s| {
        router(s.with_installation_branding(BrandingBlock {
            domain: "bad\u{1}domain".into(),
            ..BrandingBlock::default()
        }))
    })
    .await;
    let a = get(&w.app, "/user/login", None).await;
    assert_eq!(a.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", a.body);
    assert!(a.body.contains("redirect target is not a valid URL"));
}

#[tokio::test]
async fn odata_refuses_what_it_does_not_serve() {
    let w = world().await;
    for (uri, status, says) in [
        (
            "/odata/v4/$batch",
            StatusCode::NOT_IMPLEMENTED,
            "resource path",
        ),
        (
            "/odata/v4/Datasets/$ref",
            StatusCode::NOT_IMPLEMENTED,
            "navigation or nested segment",
        ),
        (
            "/odata/v4/Datasets(id='pkg-open')",
            StatusCode::NOT_IMPLEMENTED,
            "navigation or nested segment",
        ),
        (
            "/odata/v4/Datasets('pkg-open')/name",
            StatusCode::NOT_IMPLEMENTED,
            "navigation or nested segment",
        ),
        (
            "/odata/v4/Datasets(true)",
            StatusCode::UNPROCESSABLE_ENTITY,
            "key must be string or integer",
        ),
        (
            "/odata/v4/Datasets/$export?$skiptoken=garbage",
            StatusCode::UNPROCESSABLE_ENTITY,
            "not one this service wrote",
        ),
        (
            "/odata/v4/Datasets?$orderby=distributions",
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unresolved",
        ),
        (
            "/odata/v4/$crossjoin(Datasets)",
            StatusCode::UNPROCESSABLE_ENTITY,
            "CrossJoin",
        ),
    ] {
        let a = get(&w.app, uri, None).await;
        assert_eq!(a.status, status, "{uri}: {}", a.body);
        assert!(a.body.contains(says), "{uri}: {}", a.body);
        assert_eq!(a.header("content-type"), "application/problem+json");
    }
    // An unknown entity set is not found, whatever the suffix.
    for uri in [
        "/odata/v4/Nothing/$export",
        "/odata/v4/Nothing/$allowed?op=read",
    ] {
        assert_eq!(
            get(&w.app, uri, None).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn unknown_datasets_maps_without_a_platform_service_and_refused_patches() {
    let w = world_with(router).await;
    let app = &w.app;
    for uri in ["/dataset/ghost", "/dataset/ghost.ttl"] {
        let a = get(app, uri, None).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            a.body.contains("dataset 'ghost' not found"),
            "{uri}: {}",
            a.body
        );
    }
    // Without a platform domain no map service is on the platform, so there is no map.
    for uri in ["/map/open", "/map/open/res-open-1"] {
        let a = get(app, uri, None).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{uri}");
        assert!(a.body.contains("map not available"), "{uri}: {}", a.body);
    }
    let a = ckan(
        app,
        "package_patch",
        Some("tok-member"),
        json!({"id": "open", "title": "Hijacked"}),
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.body);
    let ds = w
        .store
        .get_dataset(&w.admin_ctx(), "open")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(ds.title.get("sk").map(String::as_str), Some("Hijacked"));
}

/// An installation that names its public URL (jc: the mount's scheme, host and prefix) answers
/// links on it: an http install and a catalogue under `/opendata` get no `https://{host}` links.
#[tokio::test]
async fn links_follow_the_configured_public_url() {
    let base = "http://city.test:8080/opendata";
    let w = world_with(|s| router(s.with_public_base(base))).await;
    let app = &w.app;
    let v = get(app, "/api/3/action/status_show", None).await.json();
    assert_eq!(v["result"]["site_url"], base);
    assert_eq!(
        v["help"],
        format!("{base}/api/3/action/help_show?name=status_show")
    );
    let v = get(
        app,
        "/api/3/action/package_show?id=open",
        Some("tok-forged"),
    )
    .await
    .json();
    assert_eq!(
        v["help"],
        format!("{base}/api/3/action/help_show?name=package_show")
    );
    let ttl = get(app, "/catalog.ttl", None).await;
    assert_eq!(ttl.status, StatusCode::OK, "{}", ttl.body);
    assert!(ttl.body.contains(&format!("<{base}/")), "{}", ttl.body);
    assert!(
        !ttl.body.contains(&format!("https://{HOST}")),
        "{}",
        ttl.body
    );
}

/// Anonymous listings show only what `package_show` shows an anonymous caller: no draft, no
/// withdrawn record or publisher (whatever `include_drafts` / `include_private` say), no dataset
/// the publish gate hid (`dataset.search`), and no hidden-dataset list or outbound allow-list in
/// the catalogue record. Editors keep what they may propose.
#[tokio::test]
async fn anonymous_listings_hold_no_draft_hidden_or_withdrawn_dataset() {
    let w = world().await;
    let app = &w.app;
    let mut draft = dataset("draft-one", "city", "public", 30);
    draft.state = dataroom_core::RecordState::Draft;
    w.store.upsert_dataset(&w.admin_ctx(), draft).await.unwrap();
    let mut cat = w.store.get_catalogue(w.org, w.cat).await.unwrap().unwrap();
    cat.allowed_outbound_hosts = vec!["harvest.internal".into()];
    w.store.upsert_catalogue(cat).await.unwrap();
    let never = [
        "draft-one",
        "retired",
        "orphan",
        "inner",
        "secret",
        "veiled",
    ];

    for flags in [
        "include_drafts=true",
        "include_private=true",
        "include_drafts=true&include_private=true",
    ] {
        let a = get(
            app,
            &format!("/api/3/action/package_search?rows=50&{flags}"),
            None,
        )
        .await;
        assert_eq!(a.status, StatusCode::OK, "{}", a.body);
        let names: Vec<String> = a.json()["result"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_owned())
            .collect();
        for name in never {
            assert!(
                !names.iter().any(|n| n == name),
                "{flags}: {name} in {names:?}"
            );
        }
    }

    let a = post(app, "/api/v1/ops/dataset.search", None, json!({})).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(!a.body.contains("veiled"), "{}", a.body);
    let a = post(
        app,
        "/api/v1/ops/dataset.search",
        Some("tok-editor"),
        json!({}),
    )
    .await;
    assert!(a.body.contains("veiled"), "the editor keeps it: {}", a.body);

    for op in ["catalogue.get", "catalogue.list"] {
        let a = post(app, &format!("/api/v1/ops/{op}"), None, json!({})).await;
        assert_eq!(a.status, StatusCode::OK, "{op}: {}", a.body);
        assert!(
            !a.body.contains("quality check pending") && !a.body.contains("harvest.internal"),
            "{op}: {}",
            a.body
        );
        let a = post(
            app,
            &format!("/api/v1/ops/{op}"),
            Some("tok-proposer"),
            json!({}),
        )
        .await;
        assert!(
            a.body.contains("quality check pending") && a.body.contains("harvest.internal"),
            "{op} for a proposer: {}",
            a.body
        );
    }
}

/// The tag or publisher id in a feed address is escaped wherever the feed writes it, and feeds
/// carry `nosniff` and a CSP that runs nothing.
#[tokio::test]
async fn feed_addresses_are_escaped_and_feeds_run_no_script() {
    let w = world().await;
    let tag = "%22%2F%3E%3Cx%3Ascript%20xmlns%3Ax%3D%22http%3A%2F%2Fwww.w3.org%2F1999%2Fxhtml%22%3Ealert(1)%3C%2Fx%3Ascript%3E";
    for path in [
        format!("/feeds/tag/{tag}.atom"),
        format!("/feeds/organization/{tag}.atom"),
    ] {
        let a = get(&w.app, &path, None).await;
        assert_eq!(a.status, StatusCode::OK, "{path}: {}", a.body);
        assert!(!a.body.contains("<x:script"), "{path}: {}", a.body);
        assert!(a.body.contains("&lt;x:script"), "{path}: {}", a.body);
        assert_eq!(a.header("x-content-type-options"), "nosniff");
        assert_eq!(a.header("content-security-policy"), "default-src 'none'");
    }
}

/// A caller-chosen page number cannot overflow the offset (debug builds panicked, release wrapped).
#[tokio::test]
async fn a_huge_page_number_answers_an_empty_page() {
    let w = world().await;
    let huge = usize::MAX;
    for uri in [
        format!("/api/v1/catalogue?page={huge}"),
        format!("/dataset?page={huge}"),
    ] {
        let a = get(&w.app, &uri, None).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}");
    }
}
