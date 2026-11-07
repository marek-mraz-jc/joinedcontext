#![forbid(unsafe_code)]
//! Shared application state and request context extraction for `controlroom-api`.

use crate::public::PublicRateLimiter;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use controlroom_model::{
    Authorizer, ControlroomStore, CrError, InputLimits, InstallationId, InstanceId,
    OperationRegistry, OrgId, PolicyAuthorizer, RequestCtx, Subject, controlroom_registry,
};
use controlroom_query::{
    Datasource, MapSecretResolver, PassthroughAudienceExchanger, QueryCache, SecretResolver,
    TokenExchanger, WasmPluginHost,
};
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Shared state of a `controlroom` instance (R11: passed in as a struct, no globals or env reads).
#[derive(Clone)]
pub struct ControlroomState {
    pub installation_id: InstallationId,
    pub default_org_id: OrgId,
    pub default_instance_id: InstanceId,
    pub store: Arc<dyn ControlroomStore>,
    pub authorizer: Arc<dyn Authorizer>,
    /// Live datasource drivers keyed by `(org_id, datasource_uid)`.
    pub datasources: Arc<RwLock<BTreeMap<(OrgId, String), Arc<dyn Datasource>>>>,
    /// Mapping between Grafana numeric `orgId` / org name and `(numeric_id, OrgId)` for `provision.sh` and `src/grafana.rs`.
    pub org_registry: Arc<RwLock<OrgDirectory>>,
    pub query_cache: Arc<QueryCache>,
    pub wasm_host: Arc<WasmPluginHost>,
    pub secret_resolver: Arc<dyn SecretResolver>,
    pub token_exchanger: Arc<dyn TokenExchanger>,
    pub limits: InputLimits,
    pub public_rate_limiter: Arc<PublicRateLimiter>,
    pub registry: Arc<OperationRegistry>,
}

/// Directory mapping Grafana organization names and numeric IDs to UUIDv7 [`OrgId`]s.
#[derive(Debug, Default)]
pub struct OrgDirectory {
    pub by_name: BTreeMap<String, (i64, OrgId)>,
    pub by_num: BTreeMap<i64, (String, OrgId)>,
    pub next_num: i64,
}

impl OrgDirectory {
    #[must_use]
    pub fn with_main_org(main_org_id: OrgId) -> Self {
        let mut dir = Self {
            by_name: BTreeMap::new(),
            by_num: BTreeMap::new(),
            next_num: 2,
        };
        dir.by_name
            .insert("Main Org.".to_owned(), (1, main_org_id));
        dir.by_num.insert(1, ("Main Org.".to_owned(), main_org_id));
        dir
    }

    pub fn register_org(&mut self, name: &str, org_id: OrgId) -> (i64, OrgId, bool) {
        if let Some(&(num, existing_id)) = self.by_name.get(name) {
            return (num, existing_id, false);
        }
        let num = self.next_num;
        self.next_num += 1;
        self.by_name.insert(name.to_owned(), (num, org_id));
        self.by_num.insert(num, (name.to_owned(), org_id));
        (num, org_id, true)
    }
}

impl ControlroomState {
    #[must_use]
    pub fn new(
        installation_id: InstallationId,
        default_org_id: OrgId,
        default_instance_id: InstanceId,
        store: Arc<dyn ControlroomStore>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            installation_id,
            default_org_id,
            default_instance_id,
            store,
            authorizer,
            datasources: Arc::new(RwLock::new(BTreeMap::new())),
            org_registry: Arc::new(RwLock::new(OrgDirectory::with_main_org(default_org_id))),
            query_cache: Arc::new(QueryCache::default()),
            wasm_host: Arc::new(WasmPluginHost::default()),
            secret_resolver: Arc::new(MapSecretResolver::new()),
            token_exchanger: Arc::new(PassthroughAudienceExchanger),
            limits: InputLimits::default(),
            public_rate_limiter: Arc::new(PublicRateLimiter::new(120, 60)),
            registry: Arc::new(controlroom_registry()),
        }
    }

    /// Registers a runtime [`Datasource`] implementation for `(org_id, uid)`.
    pub fn register_datasource_driver(
        &self,
        org_id: OrgId,
        uid: impl Into<String>,
        driver: Arc<dyn Datasource>,
    ) {
        self.datasources
            .write()
            .insert((org_id, uid.into()), driver);
    }

    /// Looks up a registered [`Datasource`] driver for `(org_id, uid)`.
    #[must_use]
    pub fn get_datasource_driver(&self, org_id: OrgId, uid: &str) -> Option<Arc<dyn Datasource>> {
        self.datasources
            .read()
            .get(&(org_id, uid.to_owned()))
            .cloned()
    }

    /// Convenience constructor with a permissive policy for standalone tests.
    #[must_use]
    pub fn for_standalone(store: Arc<dyn ControlroomStore>, default_org_id: OrgId) -> Self {
        let authz = Arc::new(PolicyAuthorizer::allow_org_members(
            default_org_id,
            "admin",
        ));
        Self::new(
            InstallationId::from_u128(1),
            default_org_id,
            InstanceId::from_u128(1),
            store,
            authz,
        )
    }
}

/// Extracts [`RequestCtx`] from HTTP headers:
/// - `X-Controlroom-Org-Id` (UUID) or `X-Grafana-Org-Id` (numeric ID or UUID) selects the active organization.
/// - `X-Controlroom-Subject` (actor ID), `X-Controlroom-Orgs` (comma-separated UUIDs),
///   `X-Controlroom-Groups` (comma-separated groups), and `Authorization: Bearer <token>` or Basic auth
///   populate the [`Subject`].
#[must_use]
pub fn resolve_request_ctx(state: &ControlroomState, headers: &HeaderMap) -> RequestCtx {
    let active_org = headers
        .get("x-controlroom-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<OrgId>().ok())
        .or_else(|| {
            let gf_org = headers.get("x-grafana-org-id")?.to_str().ok()?.trim();
            if let Ok(uuid_org) = gf_org.parse::<OrgId>() {
                return Some(uuid_org);
            }
            let num: i64 = gf_org.parse().ok()?;
            state
                .org_registry
                .read()
                .by_num
                .get(&num)
                .map(|(_, oid)| *oid)
        })
        .unwrap_or(state.default_org_id);

    let bearer_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(|s| s.trim().to_owned());

    let has_basic_admin = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.starts_with("Basic "));

    let actor_id = headers
        .get("x-controlroom-subject")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            if has_basic_admin || bearer_token.is_some() {
                Some("admin".to_owned())
            } else {
                Some("admin".to_owned())
            }
        });

    let member_orgs: Vec<OrgId> = headers
        .get("x-controlroom-orgs")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .filter_map(|p| p.trim().parse::<OrgId>().ok())
                .collect()
        })
        .unwrap_or_else(|| vec![active_org]);

    let groups: Vec<String> = headers
        .get("x-controlroom-groups")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .map(|g| g.trim().to_owned())
                .filter(|g| !g.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let read_only = headers
        .get("x-controlroom-readonly")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s == "true" || s == "1");

    let subject = Subject {
        actor_id,
        organizations: member_orgs,
        groups,
        bearer_token,
        read_only,
    };

    RequestCtx::new(
        state.installation_id,
        active_org,
        state.default_instance_id,
        subject,
    )
}

/// Converts a [`CrError`] into an OData 4.01 / RFC 9457 JSON HTTP response with `Cache-Control: private, no-store`.
#[must_use]
pub fn error_response(err: &CrError) -> Response {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = err.to_odata_error();
    (
        status,
        [
            (axum::http::header::CONTENT_TYPE, "application/json"),
            (axum::http::header::CACHE_CONTROL, "private, no-store"),
        ],
        axum::Json(body),
    )
        .into_response()
}
