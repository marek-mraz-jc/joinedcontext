//! Shared harness of the daiya-api route tests: a store that fails on named methods (to reach
//! the error answers of every handler), a raw-TCP upstream, and a router with one token per role.
#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_model::store::StoreFuture;
use daiya_model::{
    Authorizer, CrError, DaiyaStore, DashboardRecord, DashboardVersionRecord, DatasourceRecord,
    FolderRecord, GrafanaRole, ImportReportRecord, InstallationId, InstanceId, LibraryPanelRecord,
    OrgId, OrgQuotas, PolicyAuthorizer, PublicDashboardRecord, QueryCacheStatRecord, RoleBinding,
    Subject,
};
use daiya_query::OutboundPolicy;
use daiya_store_memory::MemoryStore;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

pub const ORG: OrgId = OrgId::from_u128(41);
pub const OTHER_ORG: OrgId = OrgId::from_u128(42);

/// A [`MemoryStore`] whose methods answer `CrError::Internal` while their name is in `failing`.
#[derive(Default)]
pub struct FlakyStore {
    pub inner: MemoryStore,
    failing: Mutex<BTreeSet<&'static str>>,
    /// Calls a method still answers before it starts failing.
    countdown: Mutex<BTreeMap<&'static str, usize>>,
}

impl FlakyStore {
    pub fn fail(&self, method: &'static str) {
        self.failing.lock().insert(method);
    }
    /// `method` answers `calls` more times, then fails.
    pub fn fail_after(&self, method: &'static str, calls: usize) {
        self.countdown.lock().insert(method, calls);
    }
    pub fn heal(&self) {
        self.failing.lock().clear();
        self.countdown.lock().clear();
    }
    fn down(&self, method: &'static str) -> bool {
        if let Some(left) = self.countdown.lock().get_mut(method) {
            if *left == 0 {
                return true;
            }
            *left -= 1;
        }
        self.failing.lock().contains(method)
    }
}

macro_rules! guarded {
    ($self:ident, $name:literal, $call:expr) => {{
        if $self.down($name) {
            return Box::pin(async {
                Err(CrError::Internal(concat!("store down: ", $name).into()))
            });
        }
        $call
    }};
}

impl DaiyaStore for FlakyStore {
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()> {
        guarded!(
            self,
            "verify_rls_enforced",
            self.inner.verify_rls_enforced()
        )
    }
    fn get_quotas(&self, org_id: OrgId) -> StoreFuture<'_, OrgQuotas> {
        guarded!(self, "get_quotas", self.inner.get_quotas(org_id))
    }
    fn set_quotas(&self, org_id: OrgId, quotas: OrgQuotas) -> StoreFuture<'_, ()> {
        guarded!(self, "set_quotas", self.inner.set_quotas(org_id, quotas))
    }
    fn list_folders(&self, org_id: OrgId) -> StoreFuture<'_, Vec<FolderRecord>> {
        guarded!(self, "list_folders", self.inner.list_folders(org_id))
    }
    fn get_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<FolderRecord>> {
        guarded!(self, "get_folder", self.inner.get_folder(org_id, uid))
    }
    fn upsert_folder(&self, folder: FolderRecord) -> StoreFuture<'_, FolderRecord> {
        guarded!(self, "upsert_folder", self.inner.upsert_folder(folder))
    }
    fn delete_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        guarded!(self, "delete_folder", self.inner.delete_folder(org_id, uid))
    }
    fn list_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DashboardRecord>> {
        guarded!(self, "list_dashboards", self.inner.list_dashboards(org_id))
    }
    fn get_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<DashboardRecord>> {
        guarded!(self, "get_dashboard", self.inner.get_dashboard(org_id, uid))
    }
    fn upsert_dashboard(
        &self,
        dashboard: DashboardRecord,
        message: &str,
        overwrite: bool,
    ) -> StoreFuture<'_, DashboardRecord> {
        guarded!(
            self,
            "upsert_dashboard",
            self.inner.upsert_dashboard(dashboard, message, overwrite)
        )
    }
    fn delete_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        guarded!(
            self,
            "delete_dashboard",
            self.inner.delete_dashboard(org_id, uid)
        )
    }
    fn list_dashboard_versions(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Vec<DashboardVersionRecord>> {
        guarded!(
            self,
            "list_dashboard_versions",
            self.inner.list_dashboard_versions(org_id, dashboard_uid)
        )
    }
    fn list_datasources(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DatasourceRecord>> {
        guarded!(
            self,
            "list_datasources",
            self.inner.list_datasources(org_id)
        )
    }
    fn get_datasource(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>> {
        guarded!(
            self,
            "get_datasource",
            self.inner.get_datasource(org_id, uid)
        )
    }
    fn get_datasource_by_name(
        &self,
        org_id: OrgId,
        name: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>> {
        guarded!(
            self,
            "get_datasource_by_name",
            self.inner.get_datasource_by_name(org_id, name)
        )
    }
    fn upsert_datasource(&self, ds: DatasourceRecord) -> StoreFuture<'_, DatasourceRecord> {
        guarded!(self, "upsert_datasource", self.inner.upsert_datasource(ds))
    }
    fn delete_datasource(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        guarded!(
            self,
            "delete_datasource",
            self.inner.delete_datasource(org_id, uid)
        )
    }
    fn list_library_panels(&self, org_id: OrgId) -> StoreFuture<'_, Vec<LibraryPanelRecord>> {
        guarded!(
            self,
            "list_library_panels",
            self.inner.list_library_panels(org_id)
        )
    }
    fn get_library_panel(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<LibraryPanelRecord>> {
        guarded!(
            self,
            "get_library_panel",
            self.inner.get_library_panel(org_id, uid)
        )
    }
    fn upsert_library_panel(
        &self,
        panel: LibraryPanelRecord,
    ) -> StoreFuture<'_, LibraryPanelRecord> {
        guarded!(
            self,
            "upsert_library_panel",
            self.inner.upsert_library_panel(panel)
        )
    }
    fn delete_library_panel(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        guarded!(
            self,
            "delete_library_panel",
            self.inner.delete_library_panel(org_id, uid)
        )
    }
    fn list_public_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<PublicDashboardRecord>> {
        guarded!(
            self,
            "list_public_dashboards",
            self.inner.list_public_dashboards(org_id)
        )
    }
    fn get_public_dashboard(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>> {
        guarded!(
            self,
            "get_public_dashboard",
            self.inner.get_public_dashboard(org_id, uid)
        )
    }
    fn get_public_dashboard_by_dashboard_uid(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>> {
        guarded!(
            self,
            "get_public_dashboard_by_dashboard_uid",
            self.inner
                .get_public_dashboard_by_dashboard_uid(org_id, dashboard_uid)
        )
    }
    fn get_public_dashboard_by_token_hash(
        &self,
        token_hash: &str,
    ) -> StoreFuture<'_, Option<(PublicDashboardRecord, DashboardRecord)>> {
        guarded!(
            self,
            "get_public_dashboard_by_token_hash",
            self.inner.get_public_dashboard_by_token_hash(token_hash)
        )
    }
    fn upsert_public_dashboard(
        &self,
        pubdash: PublicDashboardRecord,
    ) -> StoreFuture<'_, PublicDashboardRecord> {
        guarded!(
            self,
            "upsert_public_dashboard",
            self.inner.upsert_public_dashboard(pubdash)
        )
    }
    fn delete_public_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        guarded!(
            self,
            "delete_public_dashboard",
            self.inner.delete_public_dashboard(org_id, uid)
        )
    }
    fn list_query_cache_stats(&self, org_id: OrgId) -> StoreFuture<'_, Vec<QueryCacheStatRecord>> {
        guarded!(
            self,
            "list_query_cache_stats",
            self.inner.list_query_cache_stats(org_id)
        )
    }
    fn upsert_query_cache_stat(&self, stat: QueryCacheStatRecord) -> StoreFuture<'_, ()> {
        guarded!(
            self,
            "upsert_query_cache_stat",
            self.inner.upsert_query_cache_stat(stat)
        )
    }
    fn list_import_reports(&self, org_id: OrgId) -> StoreFuture<'_, Vec<ImportReportRecord>> {
        guarded!(
            self,
            "list_import_reports",
            self.inner.list_import_reports(org_id)
        )
    }
    fn insert_import_report(&self, report: ImportReportRecord) -> StoreFuture<'_, ()> {
        guarded!(
            self,
            "insert_import_report",
            self.inner.insert_import_report(report)
        )
    }
}

/// Raw-TCP HTTP upstream answering `routes` by path prefix (404 `{}` otherwise), logging each
/// request head.
pub async fn upstream(routes: Vec<(&'static str, Value)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
            log.lock().push(head);
            let (status, body) = routes
                .iter()
                .find(|(prefix, _)| path.starts_with(prefix))
                .map_or((404, "{}".to_owned()), |(_, body)| (200, body.to_string()));
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), seen)
}

/// A Prometheus answering every range query with two points of `job=edge`.
pub async fn prometheus() -> (String, Arc<Mutex<Vec<String>>>) {
    upstream(vec![
        (
            "/api/v1/query_range",
            json!({ "status": "success", "data": { "resultType": "matrix", "result": [
                { "metric": { "job": "edge" }, "values": [[1700000000, "41"], [1700000060, "42"]] }
            ]}}),
        ),
        (
            "/api/v1/labels",
            json!({ "status": "success", "data": ["job"] }),
        ),
        (
            "/api/v1/query",
            json!({ "status": "success", "data": { "resultType": "vector", "result": [] }}),
        ),
    ])
    .await
}

pub fn loopback() -> OutboundPolicy {
    OutboundPolicy {
        allow_http: true,
        allowed_networks: vec!["127.0.0.0/8".parse().unwrap()],
        ..OutboundPolicy::default()
    }
}

/// Tokens: `admin` (Admin of ORG), `editor` (Editor of ORG), `viewer` (Viewer of ORG),
/// `ops-viewer` (Viewer of folder `ops` only), `outsider` (Admin of OTHER_ORG).
pub fn bindings() -> Vec<RoleBinding> {
    vec![
        RoleBinding::from_grafana_role(ORG, "admin", GrafanaRole::Admin, None),
        RoleBinding::from_grafana_role(ORG, "editor", GrafanaRole::Editor, None),
        RoleBinding::from_grafana_role(ORG, "viewer", GrafanaRole::Viewer, None),
        RoleBinding::from_grafana_role(ORG, "ops-viewer", GrafanaRole::Viewer, Some("ops".into())),
        RoleBinding::from_grafana_role(OTHER_ORG, "outsider", GrafanaRole::Admin, None),
    ]
}

pub fn tokens() -> Vec<TokenBinding> {
    let member = |who: &str| Subject::authenticated(who, vec![ORG], vec![]);
    vec![
        TokenBinding::new("admin", member("admin")),
        TokenBinding::new("editor", member("editor")),
        TokenBinding::new("viewer", member("viewer")),
        TokenBinding::new("ops-viewer", member("ops-viewer")),
        TokenBinding::new(
            "outsider",
            Subject::authenticated("outsider", vec![OTHER_ORG], vec![]),
        ),
    ]
}

pub struct World {
    pub store: Arc<FlakyStore>,
    pub state: DaiyaState,
    pub app: Router,
}

pub fn world_with(authz: Arc<dyn Authorizer>, policy: OutboundPolicy) -> World {
    let store = Arc::new(FlakyStore::default());
    let mut state = DaiyaState::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        store.clone(),
        authz,
    )
    .with_tokens(tokens());
    state.outbound_policy = policy;
    let app = api_router(
        state.clone(),
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    World { store, state, app }
}

pub fn world() -> World {
    world_with(Arc::new(PolicyAuthorizer::new(bindings())), loopback())
}

impl World {
    /// Sends `method uri` as `token` with an optional JSON body; the answer body as JSON (or
    /// as a JSON string when it is not JSON).
    pub async fn call(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        self.send(req.body(body).unwrap()).await
    }

    pub async fn send(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())),
        )
    }

    pub async fn ok(&self, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (st, v) = self.call(method, uri, Some("admin"), body).await;
        assert_eq!(st, StatusCode::OK, "{method} {uri}: {v}");
        v
    }

    pub async fn add_prometheus(&self, uid: &str, url: &str) {
        self.ok(
            "POST",
            "/api/datasources",
            Some(json!({ "uid": uid, "name": format!("Prom {uid}"), "type": "prometheus", "url": url })),
        )
        .await;
    }

    pub async fn add_folder(&self, uid: &str, title: &str) {
        self.ok(
            "POST",
            "/api/folders",
            Some(json!({ "uid": uid, "title": title })),
        )
        .await;
    }

    pub async fn add_dashboard(&self, dashboard: Value, folder: Option<&str>) -> Value {
        let mut body = json!({ "dashboard": dashboard, "overwrite": true });
        if let Some(f) = folder {
            body["folderUid"] = json!(f);
        }
        self.ok("POST", "/api/dashboards/db", Some(body)).await
    }
}

/// A Grafana dashboard with one timeseries panel (id 1) reading `ds` and one text panel (id 2).
pub fn dashboard(uid: &str, ds: &str) -> Value {
    json!({
        "uid": uid,
        "title": format!("Board {uid}"),
        "schemaVersion": 42,
        "tags": ["city"],
        "time": { "from": "now-1h", "to": "now" },
        "panels": [
            {
                "id": 1, "type": "timeseries", "title": "Edge",
                "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
                "datasource": { "type": "prometheus", "uid": ds },
                "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": ds } }]
            },
            {
                "id": 2, "type": "text", "title": "Note",
                "gridPos": { "x": 12, "y": 0, "w": 12, "h": 8 },
                "options": { "mode": "markdown", "content": "**bold** <script>x</script>" }
            }
        ]
    })
}
