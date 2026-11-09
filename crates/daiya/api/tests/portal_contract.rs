//! The Grafana calls the Portal makes (PORTAL-CALLS.md), replayed against the router as the
//! Portal sends them: `crates/portal/src/grafana.rs` `Client` (Basic auth as the local
//! administrator, `X-Grafana-Org-Id`), after the deployment's `provision.sh` made the project's
//! organization, datasource and folder. Each step asserts what the Portal parses.

mod common;

use axum::body::Body;
use base64::Engine as _;
use common::{FlakyStore, ORG};
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_model::{
    GrafanaRole, InstallationId, InstanceId, PolicyAuthorizer, RoleBinding, SERVER_ADMIN_ACTOR,
    Subject,
};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const USER: &str = "admin";
const PASSWORD: &str = "grafana-admin-password-from-a-file";
const PLUGIN: &str = "ngsild-grafana-datasource";
const GATEWAY: &str = "https://api.city.example";
const CLOSED_SLUG: &str = "zt4qm7ge2xdv6ksb3ncf5arw2y";
const PUBLIC_SLUG: &str = "ab4qm7ge2xdv6ksb3ncf5arw2y";

fn basic(user: &str, password: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
    format!("Basic {encoded}")
}

struct Grafana {
    state: DaiyaState,
    app: axum::Router,
}

impl Grafana {
    /// A daiya with the server administrator, a person `viewer` (Viewer of the main
    /// organization) and the given subject resolver result.
    fn new(store: Arc<FlakyStore>, resolved: Option<Subject>) -> Self {
        let authz = Arc::new(PolicyAuthorizer::new(vec![RoleBinding::from_grafana_role(
            ORG,
            "viewer",
            GrafanaRole::Viewer,
            None,
        )]));
        let mut state = DaiyaState::new(
            InstallationId::from_u128(1),
            ORG,
            InstanceId::from_u128(1),
            store,
            authz,
        )
        .with_tokens(vec![TokenBinding::new(
            "viewer-token",
            Subject::authenticated("viewer", vec![ORG], vec![]),
        )])
        .with_server_admin(USER, PASSWORD)
        .expect("a valid administrator");
        if let Some(subject) = resolved {
            state = state.with_subject_resolver(move |_| Some(subject.clone()));
        }
        let app = api_router(
            state.clone(),
            Mount::new("dashboards", "https", "x.example", "").expect("mount"),
        );
        Self { state, app }
    }

    /// `grafana.rs` `Client::request`: `{url}/api{path}`, Basic auth, `X-Grafana-Org-Id`.
    async fn call(
        &self,
        method: &str,
        path: &str,
        org: Option<i64>,
        auth: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(format!("/api{path}"));
        if let Some(auth) = auth {
            req = req.header("authorization", auth);
        }
        if let Some(org) = org {
            req = req.header("x-grafana-org-id", org.to_string());
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .expect("request");
        let res = self.app.clone().oneshot(req).await.expect("answer");
        let status = res.status();
        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn admin(
        &self,
        method: &str,
        path: &str,
        org: Option<i64>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.call(method, path, org, Some(&basic(USER, PASSWORD)), body)
            .await
    }

    /// A browser GET of the page the Portal links (`api/grafana.rs:214`): status, CSP, HTML.
    async fn page(&self, uri: &str, auth: Option<&str>) -> (StatusCode, String, String) {
        let mut req = Request::builder().uri(uri);
        if let Some(auth) = auth {
            req = req.header("authorization", auth);
        }
        let res = self
            .app
            .clone()
            .oneshot(req.body(Body::empty()).expect("request"))
            .await
            .expect("answer");
        let status = res.status();
        let csp = res
            .headers()
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        (status, csp, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// `provision.sh` for one project: organization, closed Endpoint datasource, folder,
    /// project dashboard. Answers the organization's number.
    async fn provision(&self, project: &str) -> i64 {
        let (status, created) = self
            .admin("POST", "/orgs", None, Some(json!({ "name": project })))
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let org = created["orgId"].as_i64().expect("orgId");
        let (status, again) = self
            .admin("POST", "/orgs", None, Some(json!({ "name": project })))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        let source = format!("{project}-closed");
        let (status, body) = self
            .admin(
                "POST",
                "/datasources",
                Some(org),
                Some(json!({
                    "uid": source, "name": source, "type": PLUGIN, "access": "proxy",
                    "url": format!("{GATEWAY}/api/endpoint/{CLOSED_SLUG}"),
                    "jsonData": { "pageSize": 1000, "oauthPassThru": true },
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = self
            .admin(
                "POST",
                "/folders",
                Some(org),
                Some(json!({ "uid": format!("project-{project}"), "title": project })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = self
            .admin(
                "POST",
                "/dashboards/db",
                Some(org),
                Some(json!({
                    "dashboard": {
                        "uid": format!("project-{project}"), "title": format!("{project} live"),
                        "schemaVersion": 39, "panels": [],
                    },
                    "folderUid": format!("project-{project}"),
                    "overwrite": true,
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        org
    }
}

/// `grafana.rs` `dashboard_of` for a timeseries visualization.
fn portal_dashboard(uid: &str, project: &str, datasource: &str) -> Value {
    let reference = json!({ "type": PLUGIN, "uid": datasource });
    json!({
        "uid": uid,
        "title": "NO2 over time",
        "description": "Saved from the Portal. It reads through the Endpoint with your own rights.",
        "tags": ["project", project, "portal"],
        "schemaVersion": 39,
        "editable": false,
        "time": { "from": "now-7d", "to": "now" },
        "refresh": "1h",
        "timezone": "browser",
        "panels": [{
            "id": 1,
            "title": "NO2 over time",
            "type": "timeseries",
            "datasource": reference,
            "gridPos": { "x": 0, "y": 0, "w": 24, "h": 18 },
            "targets": [{
                "refId": "A", "datasource": reference, "entityType": "AirQualityObserved",
                "limit": 100, "queryType": "temporal", "attributes": ["no2"],
            }],
            "options": {},
        }],
    })
}

/// `grafana.rs` `Client::datasources`: `(slug, uid)` of every NGSI-LD datasource reading an
/// Endpoint (`slug_of`).
fn portal_datasources(listed: &Value) -> Vec<(String, String)> {
    listed
        .as_array()
        .expect("a datasource list")
        .iter()
        .filter(|s| s["type"] == PLUGIN)
        .filter_map(|s| {
            let url = s["url"].as_str()?.trim_end_matches('/');
            let (_, slug) = url.rsplit_once("/api/endpoint/")?;
            Some((slug.to_owned(), s["uid"].as_str()?.to_owned()))
        })
        .collect()
}

#[tokio::test]
async fn the_portal_saves_and_removes_a_visualization_in_a_provisioned_project() {
    let grafana = Grafana::new(Arc::new(FlakyStore::default()), None);
    let helsinki = grafana.provision("helsinki").await;
    let bbsk = grafana.provision("bbsk").await;
    assert_ne!(helsinki, bbsk);

    // `organization`: GET /orgs/name/{project} -> `id`; an unknown project is 404 (`None`).
    let (status, org) = grafana
        .admin("GET", "/orgs/name/helsinki", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{org}");
    assert_eq!(org["id"].as_i64(), Some(helsinki));
    let (status, _) = grafana.admin("GET", "/orgs/name/nowhere", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // `datasources`: the provisioned closed Endpoint datasource, by slug and uid.
    let (status, listed) = grafana
        .admin("GET", "/datasources", Some(helsinki), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        portal_datasources(&listed),
        [(CLOSED_SLUG.to_owned(), "helsinki-closed".to_owned())]
    );
    // Another project's organization holds its own datasources only.
    let (_, other) = grafana.admin("GET", "/datasources", Some(bbsk), None).await;
    assert_eq!(
        portal_datasources(&other),
        [(CLOSED_SLUG.to_owned(), "bbsk-closed".to_owned())]
    );

    // `create_public_datasource`: POST /datasources, anonymous datasource of a public Endpoint.
    let (status, body) = grafana
        .admin(
            "POST",
            "/datasources",
            Some(helsinki),
            Some(json!({
                "uid": "air-public", "name": "air-public", "type": PLUGIN, "access": "proxy",
                "url": format!("{GATEWAY}/api/endpoint/{PUBLIC_SLUG}"),
                "jsonData": { "pageSize": 1000 },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, listed) = grafana
        .admin("GET", "/datasources", Some(helsinki), None)
        .await;
    assert!(
        portal_datasources(&listed).contains(&(PUBLIC_SLUG.to_owned(), "air-public".to_owned())),
        "{listed}"
    );

    // `save`: POST /dashboards/db into folder project-{project}, overwrite; twice is fine.
    let uid = "portal-0123456789abcdef";
    for _ in 0..2 {
        let (status, body) = grafana
            .admin(
                "POST",
                "/dashboards/db",
                Some(helsinki),
                Some(json!({
                    "dashboard": portal_dashboard(uid, "helsinki", "air-public"),
                    "folderUid": "project-helsinki",
                    "overwrite": true,
                    "message": "saved from the Portal",
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // The link the Portal shows (`{public_url}/d/{uid}?orgId={org}`) opens the dashboard page
    // for a member of the organization, and for nobody else.
    let link = format!("/d/{uid}?orgId={helsinki}");
    let (status, csp, page) = grafana.page(&link, Some(&basic(USER, PASSWORD))).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(csp.starts_with("default-src 'none'"), "{csp}");
    // The page's own style is admitted by its hash; nothing else may load or run.
    assert!(
        csp.contains("style-src 'sha256-") && !csp.contains("unsafe") && !csp.contains("script"),
        "{csp}"
    );
    let style = page
        .split_once("<style>")
        .and_then(|(_, rest)| rest.split_once("</style>"))
        .map(|(style, _)| style)
        .expect("a style element");
    let hash = {
        use base64::Engine as _;
        use sha2::Digest as _;
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(style))
    };
    assert!(csp.contains(&format!("style-src 'sha256-{hash}'")), "{csp}");
    assert!(page.contains("<h1>NO2 over time</h1>"), "{page}");
    assert!(page.contains("<section><h2>NO2 over time</h2>"), "{page}");
    assert!(!page.contains("<script"), "{page}");
    let (status, _, _) = grafana
        .page(
            &format!("/d/{uid}/no2-over-time?orgId={helsinki}"),
            Some(&basic(USER, PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for auth in [None, Some("Bearer viewer-token")] {
        let (status, _, page) = grafana.page(&link, auth).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{auth:?}");
        assert!(!page.contains("NO2"), "{page}");
        // A refusal says what to do next, and reads the same as a dashboard that is not there.
        assert!(
            page.contains("open the dashboard again from its project"),
            "{page}"
        );
        let (_, _, missing) = grafana
            .page("/d/no-such-board", Some(&basic(USER, PASSWORD)))
            .await;
        assert_eq!(page, missing);
    }
    let (status, _, _) = grafana
        .page(
            &format!("/d/{uid}?orgId={bbsk}"),
            Some(&basic(USER, PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = grafana
        .page(&format!("/d/{uid}?orgId=99"), Some(&basic(USER, PASSWORD)))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // `holds`: GET /dashboards/uid/{uid} 200 in its organization, 404 in another.
    let (status, held) = grafana
        .admin(
            "GET",
            &format!("/dashboards/uid/{uid}"),
            Some(helsinki),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(held["dashboard"]["uid"], uid);
    let (status, _) = grafana
        .admin("GET", &format!("/dashboards/uid/{uid}"), Some(bbsk), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // `remove`: DELETE /dashboards/uid/{uid}; one already gone is 404, which the Portal takes.
    let (status, body) = grafana
        .admin(
            "DELETE",
            &format!("/dashboards/uid/{uid}"),
            Some(helsinki),
            None,
        )
        .await;
    assert!(status.is_success(), "{status} {body}");
    let (status, _) = grafana
        .admin(
            "DELETE",
            &format!("/dashboards/uid/{uid}"),
            Some(helsinki),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = grafana
        .admin(
            "GET",
            &format!("/dashboards/uid/{uid}"),
            Some(helsinki),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_the_server_admin_credential_reaches_a_project_organization() {
    let grafana = Grafana::new(Arc::new(FlakyStore::default()), None);
    let helsinki = grafana.provision("helsinki").await;

    // A wrong password is refused 401, as Grafana refuses it: no organization, no datasource,
    // no write, and never read as anonymous.
    let wrong = basic(USER, "not-the-password-at-all");
    let (status, _) = grafana
        .call("GET", "/orgs/name/helsinki", None, Some(&wrong), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = grafana
        .call(
            "POST",
            "/orgs",
            None,
            Some(&wrong),
            Some(json!({ "name": "evil" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = grafana
        .call("GET", "/datasources", Some(helsinki), Some(&wrong), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.get("error").is_some(), "a refusal, no list: {body}");

    // A person of the main organization does not learn about or write into the project.
    let viewer = "Bearer viewer-token";
    let (status, _) = grafana
        .call("GET", "/orgs/name/helsinki", None, Some(viewer), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = grafana
        .call(
            "POST",
            "/dashboards/db",
            Some(helsinki),
            Some(viewer),
            Some(
                json!({ "dashboard": portal_dashboard("x-1", "helsinki", "helsinki-closed"),
                         "folderUid": "project-helsinki", "overwrite": true }),
            ),
        )
        .await;
    assert!(status.is_client_error(), "{status} {body}");
}

#[tokio::test]
async fn a_host_subject_naming_the_server_admin_actor_is_anonymous() {
    let forged = Subject::authenticated(SERVER_ADMIN_ACTOR, vec![ORG], vec![]);
    let grafana = Grafana::new(Arc::new(FlakyStore::default()), Some(forged));
    // The resolver answers for every request: the forged subject must not create organizations.
    let (status, body) = grafana
        .call(
            "POST",
            "/orgs",
            None,
            None,
            Some(json!({ "name": "forged" })),
        )
        .await;
    assert!(status.is_client_error(), "{status} {body}");
    assert!(
        !grafana
            .state
            .org_registry
            .read()
            .by_name
            .contains_key("forged")
    );
}

#[tokio::test]
async fn an_organization_keeps_its_id_when_provisioned_again_after_a_restart() {
    let store = Arc::new(FlakyStore::default());
    let first = Grafana::new(store.clone(), None);
    let org = first.provision("helsinki").await;
    let id = first.state.org_registry.read().by_name["helsinki"].1;

    // A new process on the same store: provision.sh creates the organization again and finds
    // what the store kept under it.
    let second = Grafana::new(store, None);
    let (status, created) = second
        .admin("POST", "/orgs", None, Some(json!({ "name": "helsinki" })))
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(second.state.org_registry.read().by_name["helsinki"].1, id);
    let again = created["orgId"].as_i64().expect("orgId");
    assert_eq!(again, org);
    let (status, held) = second
        .admin("GET", "/dashboards/uid/project-helsinki", Some(again), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{held}");
}

#[test]
fn a_weak_server_admin_is_refused_without_echoing_the_password() {
    let state = || {
        DaiyaState::new(
            InstallationId::from_u128(1),
            ORG,
            InstanceId::from_u128(1),
            Arc::new(FlakyStore::default()),
            Arc::new(PolicyAuthorizer::default()),
        )
    };
    let short = "short-password";
    let err = state()
        .with_server_admin(USER, short)
        .err()
        .expect("refused");
    assert!(!err.to_string().contains(short), "{err}");
    assert!(state().with_server_admin("", PASSWORD).is_err());
    assert!(state().with_server_admin("a:b", PASSWORD).is_err());
}

#[tokio::test]
async fn the_dashboard_page_escapes_what_a_dashboard_holds() {
    let grafana = Grafana::new(Arc::new(FlakyStore::default()), None);
    let org = grafana.provision("helsinki").await;
    let (status, body) = grafana
        .admin(
            "POST",
            "/dashboards/db",
            Some(org),
            Some(json!({
                "dashboard": {
                    "uid": "hostile", "title": "<script>alert(1)</script>", "schemaVersion": 39,
                    "panels": [{
                        "id": 1, "type": "text", "title": "<img src=x onerror=alert(2)>",
                        "options": { "content": "<script>alert(3)</script> **bold**" },
                    }],
                },
                "folderUid": "project-helsinki",
                "overwrite": true,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, page) = grafana
        .page(
            &format!("/d/hostile?orgId={org}"),
            Some(&basic(USER, PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(!page.contains("<script>"), "{page}");
    assert!(!page.contains("<img"), "{page}");
    assert!(
        page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{page}"
    );
}

/// PORTAL-CALLS.md §5: a host that verifies a kaisatsu token (here: a stand-in check on the
/// bearer) makes its caller the server administrator; any other bearer stays anonymous.
#[tokio::test]
async fn a_host_check_can_name_the_server_admin() {
    let grafana = Grafana::new(Arc::new(FlakyStore::default()), None);
    let state = grafana.state.clone().with_server_admin_check(|headers| {
        headers.get("authorization").and_then(|v| v.to_str().ok())
            == Some("Bearer verified-kaisatsu-token")
    });
    let app = api_router(
        state.clone(),
        Mount::new("dashboards", "https", "x.example", "").expect("mount"),
    );
    let host = Grafana { state, app };
    let (status, body) = host
        .call(
            "POST",
            "/orgs",
            None,
            Some("Bearer verified-kaisatsu-token"),
            Some(json!({ "name": "helsinki" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = host
        .call(
            "GET",
            "/orgs/name/helsinki",
            None,
            Some("Bearer another-token"),
            None,
        )
        .await;
    // A token the host's check does not name is refused, never read as anonymous.
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The Basic credential set before still works beside it.
    let (status, _) = host.admin("GET", "/orgs/name/helsinki", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

/// What the Portal's dashboard builder commits (`crates/portal/tests/grafana_builder_tests.rs`
/// writes it): the reconciler saves it as it is, and the dashboard page reads it back.
const BUILDER_FILE: &str =
    include_str!("../../../portal/tests/fixtures/grafana-builder/bike-stations.json");

#[tokio::test]
async fn a_builder_file_reads_back_from_daiya_as_the_portal_saved_it() {
    let grafana = Grafana::new(Arc::new(FlakyStore::default()), None);
    let helsinki = grafana.provision("helsinki").await;
    let file: Value = serde_json::from_str(BUILDER_FILE).expect("the builder file");
    let uid = file["uid"].as_str().expect("a uid").to_owned();
    // `DashboardSync`'s save: `Client::save`, into the project's folder, overwriting.
    let (status, body) = grafana
        .admin(
            "POST",
            "/dashboards/db",
            Some(helsinki),
            Some(json!({
                "dashboard": file, "folderUid": "project-helsinki", "overwrite": true,
                "message": "saved from the Portal",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // `Client::dashboard`: what `meaning` compares the file with on the next pass, every member
    // the builder writes equal.
    let (status, held) = grafana
        .admin(
            "GET",
            &format!("/dashboards/uid/{uid}"),
            Some(helsinki),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{held}");
    for (key, value) in file.as_object().expect("an object") {
        assert_eq!(&held["dashboard"][key], value, "{key} came back otherwise");
    }
    assert_eq!(held["meta"]["folderUid"], "project-helsinki", "{held}");
    // `Client::dashboards`: the builder's uid is listed, which the sync's orphan sweep reads.
    let (status, listed) = grafana
        .admin("GET", "/search?type=dash-db", Some(helsinki), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(
        listed
            .as_array()
            .is_some_and(|hits| hits.iter().any(|hit| hit["uid"] == uid.as_str())),
        "{listed}"
    );
    // `Client::remove`: the sweep's delete, and the dashboard is gone.
    let (status, _) = grafana
        .admin(
            "DELETE",
            &format!("/dashboards/uid/{uid}"),
            Some(helsinki),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = grafana
        .admin(
            "GET",
            &format!("/dashboards/uid/{uid}"),
            Some(helsinki),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
