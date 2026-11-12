#![forbid(unsafe_code)]
//! Axum routes for `dataroom`:
//! - CKAN Action API v3 subset (`/api/3/action/{action}`, `/api/action/{action}`)
//! - OData v4 service (`/odata/v4/$metadata`, `/odata/v4/{set}`, `$count`, `$export`, `$allowed`)
//! - OpenAPI 3.1 document (`/api/v1/openapi.json`)
//! - Portal-compatible catalogue API (`/api/v1/catalogue`, `/api/v1/catalogue/datasets/{name}`)
//! - DCAT-AP 3.0.1, DCAT-AP-CZ and DCAT-AP-SK 2.0 feeds with `Accept-Profile` negotiation
//! - Atom feeds (`/feeds/dataset.atom`, `/feeds/organization/{id}.atom`, `/feeds/tag/{id}.atom`)
//! - Profile registry upload, dry-run and activation (`/api/v1/profiles`)
//! - Dataset custom SHACL sample validation (`/api/v1/datasets/{name}/validate-sample`)
//! - Operation registry REST & `allowed` (`/api/v1/ops/{op}`, `/api/v1/allowed`)
//! - MCP JSON-RPC 2.0 endpoint (`/mcp`)
//! - Server-rendered HTML UI (`/`, `/dataset`, `/dataset/{name}`, `/publisher/{name}`)

mod aliases;
pub mod ckan;
pub mod feeds;
pub mod odata_routes;

pub use aliases::{aliased, check_aliases, with_dataset_aliases};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dataroom_core::auth::constant_time_eq;
use dataroom_core::ops::{all_operations, execute_operation, OpContext, OperationDef};
use dataroom_core::{
    validate_and_sanitise_logo, Action, ActivityRow, Authorizer, BrandingBlock, Catalogue,
    CatalogueId, DataroomError, Dataset, DatasetFilter, GrantRule, ObjectTarget, OrgId, ProfilePin,
    ProfileRecord, Publisher, RecordState, RequestCtx, Result, SanitisedAsset, Store, Subject,
};
use dataroom_harvest::{HarvestEngine, HttpFetcher};
use dataroom_mcp::handle_mcp_jsonrpc;
use dataroom_rdf::{negotiate_profile, negotiate_rdf_format};
use dataroom_search::search_datasets;
use dataroom_shacl::ProfileRegistry;
use dataroom_web::dashboards::{is_dashboard, render_dashboard_list_page, render_dashboard_page};
use dataroom_web::helpers::{jc_robots_txt, jc_sitemap_xml};
use dataroom_web::i18n::{resolve_request_language, LanguageResolution};
use dataroom_web::{
    csp_header_for_branding, render_about_page, render_assistant_guide_page,
    render_assistants_page, render_dataset_list_page, render_home_page_mounted, render_map_page,
    render_organization_list_page, render_organization_read_page, render_resource_page,
    render_static_page, JC_ENUMS_JSON, JC_LOGO_SVG, JC_THEME_CSS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// Token entry mapping a bearer/API token to a verified [`Subject`].
#[derive(Clone)]
pub struct TokenBinding {
    pub token: String,
    pub subject: Subject,
}

/// The token never prints: a binding lands in panics and logs.
impl std::fmt::Debug for TokenBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenBinding")
            .field("token", &"<redacted>")
            .field("subject", &self.subject)
            .finish()
    }
}

/// The principal the Portal's CKAN publisher acts as (PORTAL-CALLS.md).
pub const PORTAL_PUBLISHER: &str = "portal-ckan";

/// The shortest Portal publisher token accepted: a guessable token would let anyone publish.
pub const MIN_PORTAL_TOKEN_LEN: usize = 32;

/// The Portal's CKAN publisher on one catalogue: `token` bound to [`PORTAL_PUBLISHER`] and the
/// one grant its calls need. `Read` and `Propose` are all of it: `package_delete` is a
/// withdrawal (`dataset.withdraw`), and `organization_*`/`package_*`/`resource_view_*` are
/// proposals; `Delete` (purge) and `Admin` stay out. Errors never repeat the token.
pub fn portal_publisher(
    token: &str,
    org_id: OrgId,
    catalogue_id: CatalogueId,
) -> Result<(TokenBinding, GrantRule)> {
    let token = token.trim();
    if token.len() < MIN_PORTAL_TOKEN_LEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(DataroomError::field_error(
            "portalToken",
            format!(
                "the Portal publisher token must be at least {MIN_PORTAL_TOKEN_LEN} printable \
                 ASCII characters without spaces"
            ),
        ));
    }
    let binding = TokenBinding {
        token: token.to_owned(),
        subject: Subject {
            actor_id: Some(PORTAL_PUBLISHER.into()),
            organizations: vec![org_id],
            groups: Vec::new(),
            client_id: Some("joinedcontext-portal".into()),
            read_only: false,
        },
    };
    let grant = GrantRule {
        org_id,
        catalogue_id: Some(catalogue_id),
        principal: PORTAL_PUBLISHER.into(),
        actions: vec![Action::Read, Action::Propose],
        publisher_id: None,
    };
    Ok((binding, grant))
}

/// The Portal's CKAN publisher on one Organization's catalogue of a deployment with several:
/// as [`portal_publisher`], under a principal of that catalogue alone (`portal-ckan:{id}`), so
/// no other catalogue's grant names it, and with the grant scoped to `publisher`, the CKAN
/// organisation of the JC Organization: the Portal publishes there and creates no other.
pub fn portal_publisher_on(
    token: &str,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    publisher: &str,
) -> Result<(TokenBinding, GrantRule)> {
    let (mut binding, mut grant) = portal_publisher(token, org_id, catalogue_id)?;
    let principal = format!("{PORTAL_PUBLISHER}:{catalogue_id}");
    binding.subject.actor_id = Some(principal.clone());
    grant.principal = principal;
    grant.publisher_id = Some(publisher.to_owned());
    Ok((binding, grant))
}

/// [`portal_publisher`] with the token read from `path`, the secret file the host mounts.
pub fn portal_publisher_from_file(
    path: &std::path::Path,
    org_id: OrgId,
    catalogue_id: CatalogueId,
) -> Result<(TokenBinding, GrantRule)> {
    let token = std::fs::read_to_string(path).map_err(|e| {
        DataroomError::Storage(format!(
            "the Portal publisher token file {} could not be read: {}",
            path.display(),
            e.kind()
        ))
    })?;
    portal_publisher(&token, org_id, catalogue_id)
}

/// Optional host-supplied subject resolver when mounted in-process inside the Portal (G1).
pub type SubjectResolver = Arc<dyn Fn(&HeaderMap) -> Option<Subject> + Send + Sync>;

/// The public Dashboards a catalogue's home shows, in the page's language (UIX-16, T-6780):
/// asked on every render, so the host re-checks each one's visibility and Organization.
pub use dataroom_web::DashboardCard;

pub type DashboardResolver = Arc<dyn Fn(&CatalogueId, &str) -> Vec<DashboardCard> + Send + Sync>;

/// Shared application state for the Axum router.
#[derive(Clone)]
pub struct DataroomState {
    pub store: Arc<dyn Store>,
    pub authorizer: Arc<dyn Authorizer>,
    pub profiles: Arc<ProfileRegistry>,
    pub harvester: Arc<HarvestEngine>,
    pub default_org_id: OrgId,
    pub default_catalogue_id: CatalogueId,
    pub tokens: Arc<Vec<TokenBinding>>,
    pub installation_branding: Arc<BrandingBlock>,
    pub mount_prefix: String,
    /// The default catalogue's public URL (scheme, host, mount prefix), when the installation
    /// knows it: its links follow it instead of `https://{host}`.
    pub public_base: Option<String>,
    pub subject_resolver: Option<SubjectResolver>,
    pub dashboard_resolver: Option<DashboardResolver>,
    pub assets: Arc<RwLock<BTreeMap<String, SanitisedAsset>>>,
    /// Whether a request on a loopback name (or with no host) reads the default catalogue. Off
    /// when the host serves several catalogues by host: then every host must name one.
    pub loopback_default: bool,
    /// The installation branding of one catalogue, in place of [`Self::installation_branding`]
    /// (each Organization's own live addresses).
    /// Each catalogue's installation branding; `jc` swaps one in when its Organization's
    /// appearance changes (UIX-16, T-6777).
    pub catalogue_branding: Arc<std::sync::RwLock<BTreeMap<CatalogueId, BrandingBlock>>>,
    /// The catalogue of each Organization the host router may decide for a request
    /// (`jc_router::Organization`).
    pub organization_catalogues: Arc<BTreeMap<String, CatalogueId>>,
    /// Each catalogue's canonical public URL (scheme, host, prefix): every absolute id and link
    /// names it, whichever registered host the catalogue was read on, so an id never moves.
    pub catalogue_bases: Arc<BTreeMap<CatalogueId, String>>,
}

/// Where a request came in, as the host's router resolved it (`jc_router::Mount`: the scheme
/// and host a trusted proxy forwarded, the mount prefix of the entry): set per request by
/// [`router`]'s outer layer, so no handler reads a raw `Host` header behind a router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOrigin {
    pub scheme: String,
    /// The host with its port, lowercased.
    pub host: String,
    pub path_prefix: String,
    /// The Organization the router decided for the request, if any.
    pub organization: Option<String>,
}

impl RequestOrigin {
    /// `scheme://host[:port]{prefix}`.
    pub fn base(&self) -> String {
        format!("{}://{}{}", self.scheme, self.host, self.path_prefix)
    }
}

tokio::task_local! {
    static ORIGIN: RequestOrigin;
}

/// This request's origin, when a host router placed it.
pub fn request_origin() -> Option<RequestOrigin> {
    ORIGIN.try_with(Clone::clone).ok()
}

/// Runs the rest of the request inside the origin of its `jc_router::Mount`, when it has one.
async fn with_origin(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    match req.extensions().get::<jc_router::Mount>() {
        Some(m) => {
            let origin = RequestOrigin {
                scheme: m.scheme.clone(),
                host: m.host.to_ascii_lowercase(),
                path_prefix: m.path_prefix.clone(),
                organization: req
                    .extensions()
                    .get::<jc_router::Organization>()
                    .map(|o| o.0.clone()),
            };
            ORIGIN.scope(origin, next.run(req)).await
        }
        None => next.run(req).await,
    }
}

impl DataroomState {
    pub fn new(
        store: Arc<dyn Store>,
        authorizer: Arc<dyn Authorizer>,
        default_org_id: OrgId,
        default_catalogue_id: CatalogueId,
    ) -> Self {
        let profiles = Arc::new(ProfileRegistry::new());
        let harvester = Arc::new(HarvestEngine::new(Arc::new(HttpFetcher::new())));
        Self {
            store,
            authorizer,
            profiles,
            harvester,
            default_org_id,
            default_catalogue_id,
            tokens: Arc::new(Vec::new()),
            installation_branding: Arc::new(BrandingBlock::default()),
            mount_prefix: String::new(),
            public_base: None,
            subject_resolver: None,
            dashboard_resolver: None,
            assets: Arc::new(RwLock::new(BTreeMap::new())),
            loopback_default: true,
            catalogue_branding: Arc::default(),
            organization_catalogues: Arc::new(BTreeMap::new()),
            catalogue_bases: Arc::new(BTreeMap::new()),
        }
    }

    /// The catalogue of each Organization, by its name.
    pub fn with_organization_catalogues(mut self, by: BTreeMap<String, CatalogueId>) -> Self {
        self.organization_catalogues = Arc::new(by);
        self
    }

    /// Each catalogue's canonical public URL, e.g. `http://opendata.city.test`.
    pub fn with_catalogue_bases(mut self, by: BTreeMap<CatalogueId, String>) -> Self {
        self.catalogue_bases = Arc::new(by);
        self
    }

    /// Off: a request on a loopback name reads no catalogue (several catalogues by host).
    pub fn with_loopback_default(mut self, on: bool) -> Self {
        self.loopback_default = on;
        self
    }

    /// Each catalogue's own installation branding (its live addresses).
    pub fn with_catalogue_branding(mut self, by: BTreeMap<CatalogueId, BrandingBlock>) -> Self {
        self.catalogue_branding = Arc::new(std::sync::RwLock::new(by));
        self
    }

    /// The installation branding `cat` layers its own over.
    pub fn branding_of(&self, cat: &Catalogue) -> BrandingBlock {
        self.catalogue_branding
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&cat.id)
            .unwrap_or(&self.installation_branding)
            .clone()
    }

    /// The mount prefix links are written under: this request's entry's, else the state's.
    pub fn mount_prefix(&self) -> String {
        request_origin().map_or_else(|| self.mount_prefix.clone(), |o| o.path_prefix)
    }

    pub fn with_tokens(mut self, tokens: Vec<TokenBinding>) -> Self {
        self.tokens = Arc::new(tokens);
        self
    }

    pub fn with_harvester(mut self, harvester: Arc<HarvestEngine>) -> Self {
        self.harvester = harvester;
        self
    }

    pub fn with_installation_branding(mut self, branding: BrandingBlock) -> Self {
        self.installation_branding = Arc::new(branding);
        self
    }

    pub fn with_mount_prefix(mut self, prefix: impl Into<String>) -> Self {
        let p = prefix.into();
        self.mount_prefix = if p == "/" {
            String::new()
        } else {
            p.trim_end_matches('/').to_owned()
        };
        self
    }

    /// The default catalogue's public URL, e.g. `http://city.test/opendata`.
    pub fn with_public_base(mut self, base: impl Into<String>) -> Self {
        self.public_base = Some(base.into());
        self
    }

    pub fn with_subject_resolver<F>(mut self, resolver: F) -> Self
    where
        F: Fn(&HeaderMap) -> Option<Subject> + Send + Sync + 'static,
    {
        self.subject_resolver = Some(Arc::new(resolver));
        self
    }

    /// The public Dashboards each catalogue's home shows (T-6780).
    pub fn with_dashboard_resolver<F>(mut self, resolver: F) -> Self
    where
        F: Fn(&CatalogueId, &str) -> Vec<DashboardCard> + Send + Sync + 'static,
    {
        self.dashboard_resolver = Some(Arc::new(resolver));
        self
    }

    pub fn op_context(&self) -> OpContext {
        OpContext {
            store: self.store.clone(),
            authorizer: self.authorizer.clone(),
            validator: Some(self.profiles.clone()),
            harvester: Some(self.harvester.clone()),
        }
    }

    /// Resolves the [`RequestCtx`] from the `Host` header (or verified default) and `Authorization`
    /// token (using constant-time token comparison). Never trusts client `x-jc-*` headers.
    pub async fn resolve_context(&self, headers: &HeaderMap) -> Result<(RequestCtx, Catalogue)> {
        // Behind a host router the host is the one it resolved (a trusted proxy's), never the
        // raw header; standalone, the `Host` header.
        let origin = request_origin();
        let raw_host = match &origin {
            Some(o) => Some(o.host.clone()),
            None => headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        };
        let host = raw_host
            .as_deref()
            .map(|h| h.split(':').next().unwrap_or(h).trim());
        let loopback = host.is_none_or(|h| h.is_empty() || h == "localhost" || h == "127.0.0.1");

        let decided = origin.as_ref().and_then(|o| o.organization.as_deref());
        let mut cat = if let Some(org) = decided {
            let found = match self.organization_catalogues.get(org) {
                Some(id) => self.store.get_catalogue(self.default_org_id, *id).await?,
                None => None,
            };
            // The same answer as an unknown host: the name is not echoed.
            found.ok_or_else(|| DataroomError::NotFound("no catalogue here".into()))?
        } else if let Some(h) = host.filter(|_| !loopback || !self.loopback_default) {
            match self.store.resolve_host(h).await? {
                Some(c) => c,
                None => {
                    return Err(DataroomError::NotFound(format!(
                        "unknown catalogue host '{h}'"
                    )));
                }
            }
        } else {
            self.store
                .get_catalogue(self.default_org_id, self.default_catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("default catalogue not found".into()))?
        };
        let default = cat.org_id == self.default_org_id && cat.id == self.default_catalogue_id;
        cat.public_base = match self.catalogue_bases.get(&cat.id) {
            Some(base) => Some(base.clone()),
            None if default && self.public_base.is_some() => self.public_base.clone(),
            // No canonical URL configured: the origin the router resolved (a registered host).
            None => origin.as_ref().map(RequestOrigin::base),
        };

        let raw_token = headers
            .get(header::AUTHORIZATION)
            .or_else(|| headers.get("X-CKAN-API-Key"))
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .map(|t| t.strip_prefix("Bearer ").unwrap_or(t).trim());

        let subject = if let Some(ref resolver) = self.subject_resolver {
            if let Some(s) = resolver(headers) {
                s
            } else {
                match raw_token.filter(|t| !t.is_empty()) {
                    Some(tok) => {
                        let matched = self.tokens.iter().find(|b| constant_time_eq(&b.token, tok));
                        match matched {
                            Some(binding) => binding.subject.clone(),
                            None => {
                                return Err(DataroomError::Unauthorized(
                                    "invalid or unknown API token".into(),
                                ));
                            }
                        }
                    }
                    None => Subject::anonymous(),
                }
            }
        } else {
            match raw_token.filter(|t| !t.is_empty()) {
                Some(tok) => {
                    let matched = self.tokens.iter().find(|b| constant_time_eq(&b.token, tok));
                    match matched {
                        Some(binding) => binding.subject.clone(),
                        None => {
                            return Err(DataroomError::Unauthorized(
                                "invalid or unknown API token".into(),
                            ));
                        }
                    }
                }
                None => Subject::anonymous(),
            }
        };

        let ctx = RequestCtx::new(cat.org_id, cat.id, subject);
        Ok((ctx, cat))
    }
}

/// Returns all registered operations in `dataroom` (G1, F1).
pub fn operations() -> Vec<OperationDef> {
    all_operations()
}

/// Converts a [`DataroomError`] into an RFC 9457 Problem Details JSON response with a slug (A5).
pub fn problem_response(err: DataroomError) -> Response {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = json!({
        "type": err.problem_type_uri(),
        "slug": err.slug(),
        "title": err.ckan_error_type(),
        "status": status.as_u16(),
        "detail": err.to_string(),
    });
    (
        status,
        [(header::CONTENT_TYPE, "application/problem+json")],
        Json(body),
    )
        .into_response()
}

/// The error a refused [`dataroom_core::Decision`] answers with: a hidden denial is a 404, so
/// the caller never learns that the record exists (C11).
pub(crate) fn decision_error(decision: dataroom_core::Decision) -> DataroomError {
    if decision.visible {
        DataroomError::Forbidden(decision.reason)
    } else {
        DataroomError::NotFound("Not found".into())
    }
}

/// Whether `ds` is shown to the caller, by the rules of `package_show` (D2, G2): the caller may
/// read it; a dataset hidden by the publish gate (`jc_hide`) only to its editors; a withdrawn
/// dataset or one of a withdrawn publisher not to anonymous callers. Every surface that shows a
/// dataset or a part of it (resource, view, map, OData row, sample validation) asks this.
pub(crate) fn dataset_shown(
    state: &DataroomState,
    ctx: &RequestCtx,
    cat: &Catalogue,
    ds: &Dataset,
    pub_rec: Option<&Publisher>,
) -> bool {
    let target = ObjectTarget::from_dataset(ds, pub_rec);
    if !state.authorizer.check(ctx, Action::Read, &target).allowed {
        return false;
    }
    if cat.hidden_datasets.contains_key(&ds.name)
        && (ctx.subject.is_anonymous()
            || !state
                .authorizer
                .check(ctx, Action::Propose, &target)
                .allowed)
    {
        return false;
    }
    !(ctx.subject.is_anonymous()
        && (ds.state != RecordState::Active
            || pub_rec.is_some_and(|p| p.state == RecordState::Withdrawn)))
}

/// Mounts dataroom under `prefix` (G1): a router the host app merges beside its own routes.
///
/// `Router::nest(prefix, router(state))` serves `{prefix}` but not `{prefix}/` (axum 0.8 nests
/// the root route at the bare prefix only), while every page links home as `{prefix}/`;
/// `mount` serves both. The state's mount prefix is set to `prefix`.
pub fn mount(prefix: &str, state: DataroomState) -> Router {
    let state = state.with_mount_prefix(prefix);
    let prefix = state.mount_prefix.clone();
    if prefix.is_empty() {
        return router(state);
    }
    Router::new()
        .route(
            &format!("{prefix}/"),
            get(handle_ui_dispatch_root).with_state(state.clone()),
        )
        .nest(&prefix, router(state))
}

/// Builds the complete Axum [`Router`] for `dataroom` (mountable standalone or under `/catalogue`, G1).
pub fn router(state: DataroomState) -> Router {
    Router::new()
        // CKAN Action API v3
        .route(
            "/api/3/action/{action}",
            get(ckan::handle_ckan_get).post(ckan::handle_ckan_post),
        )
        .route(
            "/api/action/{action}",
            get(ckan::handle_ckan_get).post(ckan::handle_ckan_post),
        )
        // DCAT-AP catalogue feeds
        .route("/catalog", get(feeds::handle_catalog_negotiated))
        .route("/catalog.ttl", get(feeds::handle_catalog_ttl))
        .route("/catalog.n3", get(feeds::handle_catalog_ttl))
        .route("/catalog.jsonld", get(feeds::handle_catalog_jsonld))
        .route("/catalog.rdf", get(feeds::handle_catalog_rdf))
        .route("/catalog.xml", get(feeds::handle_catalog_rdf))
        .route("/catalog.nt", get(feeds::handle_catalog_nt))
        .route(
            "/profiles/{profile}/catalog.ttl",
            get(feeds::handle_profile_catalog_ttl),
        )
        .route(
            "/profiles/{profile}/catalog.jsonld",
            get(feeds::handle_profile_catalog_jsonld),
        )
        .route(
            "/publisher/{publisher}/catalog.ttl",
            get(feeds::handle_publisher_catalog_ttl),
        )
        .route(
            "/publisher/{publisher}/catalog.jsonld",
            get(feeds::handle_publisher_catalog_jsonld),
        )
        // Atom feeds
        .route("/feeds/dataset.atom", get(feeds::handle_general_atom))
        .route("/feeds/custom.atom", get(feeds::handle_general_atom))
        .route(
            "/feeds/organization/{file}",
            get(feeds::handle_organization_atom),
        )
        .route("/feeds/tag/{file}", get(feeds::handle_tag_atom))
        // OData v4 & OpenAPI 3.1
        .route("/api/v1/openapi.json", get(odata_routes::handle_openapi))
        .route("/odata/v4/$metadata", get(odata_routes::handle_metadata))
        .route("/odata/v4/{*rest}", get(odata_routes::handle_odata))
        // Branding endpoint (G4) & Logo upload (B3)
        .route(
            "/api/v1/branding",
            get(handle_get_branding).post(handle_post_branding),
        )
        .route("/api/v1/assets/logo", post(handle_upload_logo))
        .route("/assets/{name}", get(handle_serve_asset))
        // Built-in theme static assets
        .route("/jc-theme.css", get(handle_jc_theme_css))
        .route("/jc-enums.json", get(handle_jc_enums_json))
        .route("/jc-logo.svg", get(handle_jc_logo_svg))
        .route("/logo.svg", get(handle_logo_svg))
        // Portal-compatible catalogue endpoints
        .route("/api/v1/catalogue", get(handle_portal_catalogue))
        .route(
            "/api/v1/catalogue/datasets/{name}",
            get(handle_portal_dataset),
        )
        // Profile management & SHACL sample validation (C8, C17)
        .route(
            "/api/v1/profiles",
            get(handle_list_profiles).post(handle_upload_profile),
        )
        .route(
            "/api/v1/profiles/{id}/activate",
            post(handle_activate_profile),
        )
        .route(
            "/api/v1/datasets/{name}/validate-sample",
            post(handle_validate_sample),
        )
        // Operation registry REST & allowed (C11, F1-F3)
        .route("/api/v1/ops/{op}", post(handle_op_call))
        .route("/api/v1/allowed", get(handle_allowed_query))
        // MCP JSON-RPC 2.0
        .route("/mcp", post(handle_mcp))
        // Sitemap & robots.txt (D6, B4)
        .route("/sitemap.xml", get(handle_sitemap_xml))
        .route("/robots.txt", get(handle_robots_txt))
        // Language switcher redirect & Login redirect to platform identity (C2, D7)
        .route("/util/redirect", post(handle_util_redirect))
        .route("/{lang}/util/redirect", post(handle_util_redirect_lang))
        .route("/user/login", get(handle_user_login))
        .route("/{lang}/user/login", get(handle_user_login_lang))
        // Server-rendered UI and dataset RDF content negotiation (including /{lang}/... prefix)
        .route("/", get(handle_ui_dispatch_root))
        .merge(theme_scripts())
        // Every other path is a UI page: a fallback rather than `/{*path}`, because a root
        // catch-all conflicts with the `/{lang}/...` routes in the router's matcher.
        .fallback(handle_ui_dispatch_fallback)
        .with_state(state)
        .layer(axum::middleware::from_fn(with_origin))
}

#[derive(Debug, Deserialize)]
struct PortalCatalogueQuery {
    q: Option<String>,
    publisher: Option<String>,
    theme: Option<String>,
    format: Option<String>,
    licence: Option<String>,
    spatial: Option<String>,
    year: Option<i32>,
    page: Option<usize>,
}

async fn handle_portal_catalogue(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(params): Query<PortalCatalogueQuery>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let datasets = match state.store.list_datasets(&ctx).await {
        Ok(d) => d,
        Err(e) => return problem_response(e),
    };
    let publishers = match state.store.list_publishers(&ctx).await {
        Ok(p) => p,
        Err(e) => return problem_response(e),
    };

    let page = params.page.unwrap_or(1).max(1);
    let mut filter = DatasetFilter {
        q: params.q.unwrap_or_default(),
        offset: (page - 1).saturating_mul(20),
        limit: 20,
        ..DatasetFilter::default()
    };
    if let Some(p) = params.publisher {
        filter.publishers.push(p);
    }
    if let Some(t) = params.theme {
        filter.themes.push(t);
    }
    if let Some(f) = params.format {
        filter.formats.push(f);
    }
    if let Some(l) = params.licence {
        filter.licences.push(l);
    }
    if let Some(s) = params.spatial {
        filter.spatial.push(s);
    }
    if let Some(y) = params.year {
        filter.years.push(y);
    }

    let outcome = match search_datasets(
        &ctx,
        state.authorizer.as_ref(),
        &datasets,
        &publishers,
        &filter,
        &cat.default_language,
    ) {
        Ok(o) => o,
        Err(e) => return problem_response(e),
    };

    let body = json!({
        "total": outcome.total,
        "page": page,
        "pageSize": 20,
        "datasets": outcome.datasets,
        "facets": outcome.facets,
        "unavailable": [],
    });
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=60")],
        Json(body),
    )
        .into_response()
}

async fn handle_portal_dataset(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let Some(ds) = (match state.store.get_dataset(&ctx, &name).await {
        Ok(d) => d,
        Err(e) => return problem_response(e),
    }) else {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{name}' not found"
        )));
    };
    let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
        Ok(p) => p,
        Err(e) => return problem_response(e),
    };
    if !dataset_shown(&state, &ctx, &cat, &ds, pub_rec.as_ref()) {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{name}' not found"
        )));
    }
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=60")],
        Json(ds),
    )
        .into_response()
}

async fn handle_list_profiles(State(state): State<DataroomState>, headers: HeaderMap) -> Response {
    let (_ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let profiles = state.profiles.list();
    Json(json!({
        "mainProfile": cat.main_profile,
        "activeProfiles": cat.active_profiles,
        "profiles": profiles,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadProfileRequest {
    pub profile: ProfileRecord,
    #[serde(default)]
    pub activate: bool,
}

async fn handle_upload_profile(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Json(req): Json<UploadProfileRequest>,
) -> Response {
    let (ctx, mut cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let target = ObjectTarget::Profile {
        org_id: ctx.org_id,
        catalogue_id: ctx.catalogue_id,
        profile_id: req.profile.id.clone(),
    };
    let dec = state.authorizer.check(&ctx, Action::Propose, &target);
    if !dec.allowed {
        return problem_response(DataroomError::Forbidden(dec.reason));
    }
    // Activating changes what the catalogue validates against, which the activate route
    // decides with Approve; uploading with `activate` may not skip that.
    if req.activate {
        let dec = state.authorizer.check(&ctx, Action::Approve, &target);
        if !dec.allowed {
            return problem_response(DataroomError::Forbidden(dec.reason));
        }
    }

    // A published version is immutable (C17: catalogues pin versions). The registry serves
    // every catalogue, so replacing a pair would change what another catalogue (or a built-in
    // profile) validates against.
    if state
        .profiles
        .list()
        .iter()
        .any(|p| p.id == req.profile.id && p.version == req.profile.version)
    {
        return problem_response(DataroomError::Conflict(format!(
            "profile '{}' version '{}' already exists; upload a new version",
            req.profile.id, req.profile.version
        )));
    }

    let datasets = match state.store.list_datasets(&ctx).await {
        Ok(d) => d,
        Err(e) => return problem_response(e),
    };
    let publishers = match state.store.list_publishers(&ctx).await {
        Ok(p) => p,
        Err(e) => return problem_response(e),
    };
    let pub_map: BTreeMap<String, Publisher> = publishers
        .into_iter()
        .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
        .collect();

    let (saved_profile, dry_run) =
        match state
            .profiles
            .upload_and_dry_run(req.profile, &cat, &datasets, &pub_map)
        {
            Ok(res) => res,
            Err(e) => return problem_response(e),
        };

    if let Err(e) = state
        .store
        .upsert_profile(&ctx, saved_profile.clone())
        .await
    {
        return problem_response(e);
    }

    if req.activate {
        cat.active_profiles
            .retain(|p| p.profile_id != saved_profile.id);
        cat.active_profiles.push(ProfilePin {
            profile_id: saved_profile.id.clone(),
            version: saved_profile.version.clone(),
        });
        if let Err(e) = state.store.upsert_catalogue(cat).await {
            return problem_response(e);
        }
    }

    Json(json!({
        "profile": saved_profile,
        "dryRun": dry_run,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ActivateProfileBody {
    version: String,
    #[serde(default)]
    make_main: bool,
}

async fn handle_activate_profile(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ActivateProfileBody>,
) -> Response {
    let (ctx, mut cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let target = ObjectTarget::Profile {
        org_id: ctx.org_id,
        catalogue_id: ctx.catalogue_id,
        profile_id: id.clone(),
    };
    let dec = state.authorizer.check(&ctx, Action::Approve, &target);
    if !dec.allowed {
        return problem_response(DataroomError::Forbidden(dec.reason));
    }

    let datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
    let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
    let pub_map: BTreeMap<String, Publisher> = publishers
        .into_iter()
        .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
        .collect();

    let dry_run = match state
        .profiles
        .dry_run(&id, &body.version, &cat, &datasets, &pub_map)
    {
        Ok(r) => r,
        Err(e) => return problem_response(e),
    };

    cat.active_profiles.retain(|p| p.profile_id != id);
    cat.active_profiles.push(ProfilePin {
        profile_id: id.clone(),
        version: body.version.clone(),
    });
    if body.make_main {
        cat.main_profile = id;
    }
    let saved_cat = match state.store.upsert_catalogue(cat).await {
        Ok(c) => c,
        Err(e) => return problem_response(e),
    };
    for rep in &dry_run.dataset_reports {
        let _ = state.store.upsert_shacl_report(&ctx, rep.clone()).await;
    }

    Json(json!({
        "catalogue": saved_cat,
        "dryRun": dry_run,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ValidateSampleBody {
    sample_turtle: String,
    shapes_turtle: Option<String>,
}

async fn handle_validate_sample(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<ValidateSampleBody>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let Some(ds) = (match state.store.get_dataset(&ctx, &name).await {
        Ok(d) => d,
        Err(e) => return problem_response(e),
    }) else {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{name}' not found"
        )));
    };
    let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
        Ok(p) => p,
        Err(e) => return problem_response(e),
    };
    // The shapes of a dataset the caller may not see are not used for it either.
    if !dataset_shown(&state, &ctx, &cat, &ds, pub_rec.as_ref()) {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{name}' not found"
        )));
    }
    let shapes = body
        .shapes_turtle
        .or_else(|| ds.custom_shacl_shapes.clone())
        .ok_or_else(|| {
            DataroomError::field_error("shapes_turtle", "dataset has no custom SHACL shapes")
        });
    let shapes = match shapes {
        Ok(s) => s,
        Err(e) => return problem_response(e),
    };
    match state.profiles.validate_custom_sample(
        &shapes,
        &body.sample_turtle,
        ctx.org_id,
        ctx.catalogue_id,
        &ds.id,
    ) {
        Ok(report) => Json(report).into_response(),
        Err(e) => problem_response(e),
    }
}

async fn handle_op_call(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(op): Path<String>,
    Json(input): Json<Value>,
) -> Response {
    let (ctx, _cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let op_ctx = state.op_context();
    match execute_operation(&op_ctx, &ctx, &op, input).await {
        Ok(val) => Json(val).into_response(),
        Err(e) => problem_response(e),
    }
}

#[derive(Debug, Deserialize)]
struct AllowedQuery {
    operation: String,
    object: Option<String>,
}

async fn handle_allowed_query(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(q): Query<AllowedQuery>,
) -> Response {
    let (ctx, _cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let op_ctx = state.op_context();
    match execute_operation(
        &op_ctx,
        &ctx,
        "allowed",
        json!({ "operation": q.operation, "object": q.object }),
    )
    .await
    {
        Ok(val) => Json(val).into_response(),
        Err(e) => problem_response(e),
    }
}

async fn handle_mcp(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Json(rpc): Json<Value>,
) -> Response {
    let (ctx, _cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let op_ctx = state.op_context();
    let answer = handle_mcp_jsonrpc(&op_ctx, &ctx, &rpc).await;
    Json(answer).into_response()
}

async fn handle_get_branding(State(state): State<DataroomState>, headers: HeaderMap) -> Response {
    let (_ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let effective = cat.effective_branding(Some(&state.branding_of(&cat)));
    Json(effective).into_response()
}

async fn handle_post_branding(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Json(override_val): Json<Value>,
) -> Response {
    let (ctx, mut cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let target = ObjectTarget::Catalogue {
        org_id: ctx.org_id,
        catalogue_id: ctx.catalogue_id,
    };
    let dec = state.authorizer.check(&ctx, Action::Admin, &target);
    if !dec.allowed {
        return problem_response(DataroomError::Forbidden(dec.reason));
    }
    cat.branding = Some(override_val);
    cat.modified = chrono::Utc::now();
    let saved = match state.store.upsert_catalogue(cat).await {
        Ok(c) => c,
        Err(e) => return problem_response(e),
    };
    let _ = state
        .store
        .record_activity(
            &ctx,
            ActivityRow {
                org_id: ctx.org_id,
                catalogue_id: ctx.catalogue_id,
                id: format!("act-{}", Uuid::now_v7()),
                at: chrono::Utc::now(),
                actor: ctx.actor_name().to_owned(),
                operation: "branding.propose".into(),
                object_type: "Catalogue".into(),
                object_id: saved.id.to_string(),
                summary: format!("Updated branding on catalogue {}", saved.name),
                details: json!({}),
            },
        )
        .await;
    let effective = saved.effective_branding(Some(&state.branding_of(&saved)));
    Json(effective).into_response()
}

async fn handle_upload_logo(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (ctx, mut cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let target = ObjectTarget::Catalogue {
        org_id: ctx.org_id,
        catalogue_id: ctx.catalogue_id,
    };
    let dec = state.authorizer.check(&ctx, Action::Propose, &target);
    if !dec.allowed {
        return problem_response(DataroomError::Forbidden(dec.reason));
    }
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/svg+xml");
    let asset = match validate_and_sanitise_logo(&body, ct) {
        Ok(a) => a,
        Err(e) => return problem_response(e),
    };
    let fname = asset.url_path.trim_start_matches("/assets/").to_owned();
    state
        .assets
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(fname, asset.clone());
    cat.theme.logo = Some(asset.url_path.clone());
    if let Err(e) = state.store.upsert_catalogue(cat).await {
        return problem_response(e);
    }
    let full_url = format!("{}{}", state.mount_prefix(), asset.url_path);
    Json(json!({
        "url": full_url,
        "sha256": asset.sha256_hex,
        "contentType": asset.content_type,
    }))
    .into_response()
}

async fn handle_serve_asset(
    State(state): State<DataroomState>,
    Path(name): Path<String>,
) -> Response {
    let found = state
        .assets
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&name)
        .cloned();
    match found {
        Some(asset) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, asset.content_type),
                (
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".to_owned(),
                ),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
                // An uploaded SVG opened on its own runs nothing and loads nothing (B3, defence
                // in depth behind the upload check).
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; style-src 'unsafe-inline'; sandbox".to_owned(),
                ),
            ],
            asset.bytes,
        )
            .into_response(),
        None => problem_response(DataroomError::NotFound(format!("asset '{name}' not found"))),
    }
}

async fn handle_jc_theme_css() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        JC_THEME_CSS,
    )
        .into_response()
}

/// One route per theme script (`dataroom_web::JC_SCRIPTS`): a fixed list, so no path reaches a
/// file the crate does not embed.
fn theme_scripts() -> Router<DataroomState> {
    dataroom_web::JC_SCRIPTS
        .iter()
        .fold(Router::new(), |router, &(name, body)| {
            router.route(
                &format!("/{name}"),
                get(move || async move {
                    (
                        StatusCode::OK,
                        [
                            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                            (header::CACHE_CONTROL, "public, max-age=3600"),
                        ],
                        body,
                    )
                        .into_response()
                }),
            )
        })
}

async fn handle_jc_enums_json() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        JC_ENUMS_JSON,
    )
        .into_response()
}

async fn handle_jc_logo_svg() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        JC_LOGO_SVG,
    )
        .into_response()
}

async fn handle_sitemap_xml(State(state): State<DataroomState>, headers: HeaderMap) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let mut datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
    datasets.retain(|d| !is_dashboard(d));
    let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
    let xml = jc_sitemap_xml(&cat, &datasets, &publishers);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

/// The installation's own logo (`logo: logo.svg`, `logoSvg` of its branding, as old prod's chart
/// wrote it into the theme's public directory): the favicon and the footer mark. Only an SVG the
/// logo upload accepts is served on this origin; otherwise, or for no catalogue, the lockup.
async fn handle_logo_svg(State(state): State<DataroomState>, headers: HeaderMap) -> Response {
    let own = match state.resolve_context(&headers).await {
        Ok((_ctx, cat)) => {
            cat.effective_branding(Some(&state.branding_of(&cat)))
                .logo_svg
        }
        Err(_) => None,
    };
    let body = own
        .filter(|svg| dataroom_core::branding::is_safe_logo_svg(svg))
        .unwrap_or_else(|| JC_LOGO_SVG.to_owned());
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn handle_robots_txt(State(state): State<DataroomState>, headers: HeaderMap) -> Response {
    let (_ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let branding = cat.effective_branding(Some(&state.branding_of(&cat)));
    let txt = jc_robots_txt(&cat, &branding);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        txt,
    )
        .into_response()
}

async fn handle_util_redirect(State(state): State<DataroomState>, body: Bytes) -> Response {
    redirect_from_form(&state, &body)
}

async fn handle_util_redirect_lang(
    State(state): State<DataroomState>,
    Path(_lang): Path<String>,
    body: Bytes,
) -> Response {
    redirect_from_form(&state, &body)
}

fn redirect_from_form(state: &DataroomState, body: &[u8]) -> Response {
    let mut url_target = format!("{}/", state.mount_prefix());
    let mut chosen_lang = None;
    for (k, v) in url::form_urlencoded::parse(body) {
        if k == "url" && is_safe_local_target(&v) {
            url_target = v.into_owned();
            let trimmed = url_target
                .strip_prefix(&state.mount_prefix())
                .unwrap_or(&url_target)
                .trim_start_matches('/');
            if let Some(seg) = trimmed.split('/').next() {
                if seg.len() == 2 && seg.bytes().all(|b| b.is_ascii_lowercase()) {
                    chosen_lang = Some(seg.to_owned());
                }
            }
        }
    }
    let cookie_path = if state.mount_prefix().is_empty() {
        "/"
    } else {
        &state.mount_prefix()
    };
    if let Some(l) = chosen_lang {
        let mut response = safe_redirect(StatusCode::SEE_OTHER, &url_target);
        if response.status() == StatusCode::SEE_OTHER {
            match axum::http::HeaderValue::try_from(format!(
                "ckan_lang={l}; Path={cookie_path}; SameSite=Lax"
            )) {
                Ok(cookie) => {
                    response.headers_mut().insert(header::SET_COOKIE, cookie);
                }
                Err(_) => {
                    return problem_response(DataroomError::Validation {
                        message: "mount prefix is not a valid cookie path".into(),
                        fields: Default::default(),
                    })
                }
            }
        }
        response
    } else {
        safe_redirect(StatusCode::SEE_OTHER, &url_target)
    }
}

async fn handle_user_login(State(state): State<DataroomState>, headers: HeaderMap) -> Response {
    login_redirect(&state, &headers).await
}

async fn handle_user_login_lang(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(_lang): Path<String>,
) -> Response {
    login_redirect(&state, &headers).await
}

async fn login_redirect(state: &DataroomState, headers: &HeaderMap) -> Response {
    let (_, cat) = match state.resolve_context(headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let branding = cat.effective_branding(Some(&state.branding_of(&cat)));
    let target = if let Some(portal) = branding.address(dataroom_core::branding::Address::Portal) {
        format!("{portal}/login")
    } else {
        format!("{}/", state.mount_prefix())
    };
    safe_redirect(StatusCode::TEMPORARY_REDIRECT, &target)
}

async fn handle_ui_dispatch_root(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
) -> Response {
    dispatch_ui_route(state, headers, "/", raw_query).await
}

async fn handle_ui_dispatch_fallback(
    State(state): State<DataroomState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> Response {
    if method != axum::http::Method::GET && method != axum::http::Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD")],
        )
            .into_response();
    }
    // Inside `Router::nest` the URI arrives with the mount prefix stripped.
    let full = format!("/{}", uri.path().trim_start_matches('/'));
    let raw_query = uri.query().map(str::to_owned);
    dispatch_ui_route(state, headers, &full, raw_query).await
}

/// Builds a redirect without panicking on a target that is not a valid header value
/// (`axum::response::Redirect` panics on one).
pub(crate) fn safe_redirect(status: StatusCode, target: &str) -> Response {
    match axum::http::HeaderValue::try_from(target) {
        Ok(location) => (status, [(header::LOCATION, location)]).into_response(),
        Err(_) => problem_response(DataroomError::Validation {
            message: "redirect target is not a valid URL".into(),
            fields: Default::default(),
        }),
    }
}

/// A same-origin redirect target from user input: an absolute path, no scheme-relative
/// `//` or `/\` (browsers read both as another host), no control characters.
fn is_safe_local_target(v: &str) -> bool {
    v.starts_with('/')
        && !v.starts_with("//")
        && !v.contains('\\')
        && !v.chars().any(char::is_control)
}

fn html_response_with_csp(
    html: String,
    nonce: &str,
    branding: &BrandingBlock,
    framable_map: bool,
) -> Response {
    let csp = csp_header_for_branding(nonce, branding, framable_map);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp),
        ],
        html,
    )
        .into_response()
}

/// A map shows a dataset's distributions, so it is shown where the dataset page is (D3).
async fn map_shown(state: &DataroomState, ctx: &RequestCtx, cat: &Catalogue, ds: &Dataset) -> bool {
    // A failed publisher lookup shows nothing rather than skipping the withdrawn check.
    let Ok(pub_rec) = state.store.get_publisher(ctx, &ds.publisher_id).await else {
        return false;
    };
    ds.state != RecordState::Withdrawn && dataset_shown(state, ctx, cat, ds, pub_rec.as_ref())
}

fn publisher_of<'a>(publishers: &'a [Publisher], ds: &Dataset) -> Option<&'a Publisher> {
    publishers
        .iter()
        .find(|p| p.id == ds.publisher_id || p.name == ds.publisher_id)
}

/// The dashboards of the catalogue the reader may see (T-6790).
async fn shown_dashboards(
    state: &DataroomState,
    ctx: &RequestCtx,
    cat: &Catalogue,
    publishers: &[Publisher],
) -> Vec<Dataset> {
    state
        .store
        .list_datasets(ctx)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|d| {
            is_dashboard(d)
                && d.state != RecordState::Withdrawn
                && dataset_shown(state, ctx, cat, d, publisher_of(publishers, d))
        })
        .collect()
}

async fn dispatch_ui_route(
    state: DataroomState,
    headers: HeaderMap,
    raw_path: &str,
    raw_query: Option<String>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let branding = cat.effective_branding(Some(&state.branding_of(&cat)));
    let cookie_hdr = headers.get(header::COOKIE).and_then(|v| v.to_str().ok());
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());

    let mut q_params: BTreeMap<String, String> = BTreeMap::new();
    if let Some(ref rq) = raw_query {
        for (k, v) in url::form_urlencoded::parse(rq.as_bytes()) {
            q_params.insert(k.into_owned(), v.into_owned());
        }
    }

    let (lang, stripped_path, had_url_prefix) = match resolve_request_language(
        raw_path,
        cookie_hdr,
        accept_lang,
        &branding.languages.offered,
        &branding.languages.default,
    ) {
        LanguageResolution::Resolved {
            lang,
            stripped_path,
            had_url_prefix,
        } => {
            let chosen = q_params
                .get("lang")
                .cloned()
                .filter(|_| !had_url_prefix)
                .unwrap_or(lang);
            (chosen, stripped_path, had_url_prefix)
        }
        LanguageResolution::UnknownUrlPrefix(bad) => {
            return problem_response(DataroomError::NotFound(format!(
                "unknown language prefix '{bad}'"
            )));
        }
    };

    let nonce = Uuid::now_v7().simple().to_string();
    let trimmed = stripped_path.trim_matches('/');
    let segments: Vec<&str> = if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    };

    match segments.as_slice() {
        // D1: Home page (`/`, `/sk/`)
        [] => {
            let mut datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
            datasets.retain(|d| !is_dashboard(d));
            let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
            let q = q_params.get("q").map(String::as_str).unwrap_or("");
            let dashboards = state
                .dashboard_resolver
                .as_ref()
                .map(|resolve| resolve(&cat.id, &lang))
                .unwrap_or_default();
            let html = render_home_page_mounted(
                &cat,
                Some(&state.branding_of(&cat)),
                &datasets,
                &publishers,
                &dashboards,
                q,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D2: Dataset list & search (`/dataset`, `/dataset/`)
        ["dataset"] => {
            let mut all_ds = state.store.list_datasets(&ctx).await.unwrap_or_default();
            all_ds.retain(|d| !is_dashboard(d));
            let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
            let pub_map: BTreeMap<String, Publisher> = publishers
                .iter()
                .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p.clone())])
                .collect();

            // `jc_hide` rule: hidden datasets are hidden from the public, visible to editors of the publisher
            let visible_ds: Vec<Dataset> = all_ds
                .into_iter()
                .filter(|ds| {
                    if !cat.hidden_datasets.contains_key(&ds.name) {
                        return true;
                    }
                    if ctx.subject.is_anonymous() {
                        return false;
                    }
                    let pub_rec = pub_map.get(&ds.publisher_id);
                    let target = ObjectTarget::from_dataset(ds, pub_rec);
                    state
                        .authorizer
                        .check(&ctx, Action::Propose, &target)
                        .allowed
                })
                .collect();

            let page = q_params
                .get("page")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let q = q_params.get("q").cloned().unwrap_or_default();
            let sort = q_params
                .get("sort")
                .cloned()
                .unwrap_or_else(|| "score desc, metadata_modified desc".into());
            let mut filter = DatasetFilter {
                q: q.clone(),
                sort: Some(sort.clone()),
                offset: (page - 1).saturating_mul(20),
                limit: 20,
                ..DatasetFilter::default()
            };
            if let Some(org) = q_params
                .get("organization")
                .or_else(|| q_params.get("publisher"))
            {
                filter.publishers.push(org.clone());
            }
            if let Some(topic) = q_params
                .get("vocab_jc_topic")
                .or_else(|| q_params.get("theme"))
            {
                filter.themes.push(topic.clone());
            }
            if let Some(tag) = q_params.get("tags").or_else(|| q_params.get("keyword")) {
                filter.tags.push(tag.clone());
            }
            if let Some(fmt) = q_params
                .get("vocab_jc_format")
                .or_else(|| q_params.get("res_format"))
            {
                filter.formats.push(fmt.clone());
            }
            if let Some(lic) = q_params
                .get("license_id")
                .or_else(|| q_params.get("licence"))
            {
                filter.licences.push(lic.clone());
            }
            if let Some(sp) = q_params.get("spatial") {
                filter.spatial.push(sp.clone());
            }

            let outcome = search_datasets(
                &ctx,
                state.authorizer.as_ref(),
                &visible_ds,
                &publishers,
                &filter,
                &lang,
            )
            .unwrap_or_else(|_| dataroom_search::SearchOutcome {
                total: 0,
                offset: 0,
                limit: 20,
                datasets: Vec::new(),
                facets: Default::default(),
            });

            let html = render_dataset_list_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &outcome.datasets,
                &publishers,
                &outcome.facets,
                outcome.total,
                page,
                &q,
                &sort,
                &q_params,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D3: Dataset page or RDF (`/dataset/{name_or_ext}`)
        ["dataset", name_or_ext] => {
            let (ds_name, ext) = match name_or_ext.rsplit_once('.') {
                Some((stem, e)) if matches!(e, "ttl" | "n3" | "jsonld" | "rdf" | "xml" | "nt") => {
                    (stem.to_owned(), Some(e))
                }
                _ => ((*name_or_ext).to_owned(), None),
            };
            let Some(ds) = state.store.get_dataset(&ctx, &ds_name).await.ok().flatten() else {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_name}' not found"
                )));
            };
            if ds.state == RecordState::Withdrawn {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_name}' not found"
                )));
            }
            let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
                Ok(p) => p,
                Err(e) => return problem_response(e),
            };
            if pub_rec
                .as_ref()
                .is_some_and(|p| p.state == RecordState::Withdrawn)
                && ctx.subject.is_anonymous()
            {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_name}' not found"
                )));
            }
            let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
            if cat.hidden_datasets.contains_key(&ds.name) {
                let can_edit = !ctx.subject.is_anonymous()
                    && state
                        .authorizer
                        .check(&ctx, Action::Propose, &target)
                        .allowed;
                if !can_edit {
                    return problem_response(DataroomError::NotFound(format!(
                        "dataset '{ds_name}' not found"
                    )));
                }
            }
            if !state.authorizer.check(&ctx, Action::Read, &target).allowed {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_name}' not found"
                )));
            }

            let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok());
            if let Some(rdf_fmt) = negotiate_rdf_format(ext, accept) {
                let accept_profile = headers.get("Accept-Profile").and_then(|v| v.to_str().ok());
                let q_prof = q_params.get("_profile").map(String::as_str);
                let (prof_kind, prof_uri) =
                    negotiate_profile(q_prof, accept_profile, &cat.main_profile);
                let body = dataroom_rdf::serialize_dataset(
                    &ds,
                    &cat,
                    pub_rec.as_ref(),
                    rdf_fmt,
                    prof_kind,
                );
                return (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, rdf_fmt.content_type().to_owned()),
                        (header::LINK, format!("<{prof_uri}>; rel=\"profile\"")),
                        (header::HeaderName::from_static("content-profile"), prof_uri),
                    ],
                    body,
                )
                    .into_response();
            }

            let mut all_views = Vec::new();
            for d in &ds.distributions {
                if let Ok(v) = state.store.list_resource_views(&ctx, &d.id).await {
                    all_views.extend(v);
                }
            }
            let can_edit = state
                .authorizer
                .check(&ctx, Action::Propose, &target)
                .allowed;
            let reports = if can_edit {
                state
                    .store
                    .list_shacl_reports(&ctx, Some(&ds.id), None)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            // ponytail: one catalogue listing per dataset page; a store query by type when
            // catalogues hold thousands of packages.
            let dashboards: Vec<Dataset> = state
                .store
                .list_datasets(&ctx)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|d| {
                    is_dashboard(d)
                        && d.state != RecordState::Withdrawn
                        && dataset_shown(&state, &ctx, &cat, d, None)
                })
                .collect();
            let html = dataroom_web::render_dataset_page_linked(
                &cat,
                Some(&state.branding_of(&cat)),
                &ds,
                pub_rec.as_ref(),
                &all_views,
                &dashboards,
                &reports,
                can_edit,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // Dashboards (T-6790): `/dashboards`, `/dashboards/{name}`.
        ["dashboards"] => {
            let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
            let dashboards = shown_dashboards(&state, &ctx, &cat, &publishers).await;
            let html = render_dashboard_list_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &dashboards,
                &publishers,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        ["dashboards", name] => {
            let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
            let found = state.store.get_dataset(&ctx, name).await.ok().flatten();
            let Some(ds) = found.filter(|d| {
                is_dashboard(d)
                    && d.state != RecordState::Withdrawn
                    && dataset_shown(&state, &ctx, &cat, d, publisher_of(&publishers, d))
            }) else {
                return problem_response(DataroomError::NotFound(format!(
                    "dashboard '{name}' not found"
                )));
            };
            let mut views = Vec::new();
            for d in &ds.distributions {
                if let Ok(v) = state.store.list_resource_views(&ctx, &d.id).await {
                    views.extend(v);
                }
            }
            let mut datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
            datasets.retain(|d| !is_dashboard(d));
            let html = render_dashboard_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &ds,
                &views,
                &datasets,
                &publishers,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D4: Resource page (`/dataset/{name}/resource/{id}`)
        ["dataset", ds_name, "resource", res_id] => {
            let Some(ds) = state.store.get_dataset(&ctx, ds_name).await.ok().flatten() else {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_name}' not found"
                )));
            };
            let Some(dist) = ds.distributions.iter().find(|d| d.id == *res_id) else {
                return problem_response(DataroomError::NotFound(format!(
                    "resource '{res_id}' not found"
                )));
            };
            let pub_rec = match state.store.get_publisher(&ctx, &ds.publisher_id).await {
                Ok(p) => p,
                Err(e) => return problem_response(e),
            };
            // A resource is shown where its dataset page is (D3).
            if ds.state == RecordState::Withdrawn
                || !dataset_shown(&state, &ctx, &cat, &ds, pub_rec.as_ref())
            {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_name}' not found"
                )));
            }
            let html = render_resource_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &ds,
                dist,
                pub_rec.as_ref(),
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D5: Publisher list (`/organization`, `/publisher`)
        ["organization"] | ["publisher"] => {
            let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
            let mut datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
            datasets.retain(|d| !is_dashboard(d));
            let q = q_params.get("q").map(String::as_str).unwrap_or("");
            let sort = q_params
                .get("sort")
                .map(String::as_str)
                .unwrap_or("title asc");
            let html = render_organization_list_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &publishers,
                &datasets,
                q,
                sort,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D5: Publisher page (`/organization/{name}`, `/publisher/{name}`)
        ["organization", name] | ["publisher", name] => {
            let all_pubs = state.store.list_publishers(&ctx).await.unwrap_or_default();
            if let Some(raw_pub) = all_pubs.iter().find(|p| p.name == *name || p.id == *name) {
                if let Some(ref target_name) = raw_pub.merged_into {
                    let prefix_part = if segments[0] == "publisher" {
                        "publisher"
                    } else {
                        "organization"
                    };
                    return safe_redirect(
                        StatusCode::PERMANENT_REDIRECT,
                        &format!("{}/{prefix_part}/{target_name}", state.mount_prefix()),
                    );
                }
            }
            let Some(pub_rec) = state.store.get_publisher(&ctx, name).await.ok().flatten() else {
                return problem_response(DataroomError::NotFound(format!(
                    "publisher '{name}' not found"
                )));
            };
            if pub_rec.state == RecordState::Withdrawn && ctx.subject.is_anonymous() {
                return problem_response(DataroomError::NotFound(format!(
                    "publisher '{name}' not found"
                )));
            }
            let mut all_ds = state.store.list_datasets(&ctx).await.unwrap_or_default();
            all_ds.retain(|d| !is_dashboard(d));
            let visible_ds: Vec<Dataset> = all_ds
                .into_iter()
                .filter(|d| !cat.hidden_datasets.contains_key(&d.name))
                .collect();
            let q = q_params.get("q").cloned().unwrap_or_default();
            let sort = q_params
                .get("sort")
                .cloned()
                .unwrap_or_else(|| "score desc, metadata_modified desc".into());
            let page = q_params
                .get("page")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let mut filter = DatasetFilter {
                q: q.clone(),
                publishers: vec![pub_rec.name.clone()],
                sort: Some(sort.clone()),
                offset: (page - 1).saturating_mul(dataroom_web::listing::PER_PAGE),
                limit: dataroom_web::listing::PER_PAGE,
                ..DatasetFilter::default()
            };
            if let Some(t) = q_params.get("tags") {
                filter.tags.push(t.clone());
            }
            if let Some(f) = q_params.get("res_format") {
                filter.formats.push(f.clone());
            }
            if let Some(l) = q_params.get("license_id") {
                filter.licences.push(l.clone());
            }
            let outcome = search_datasets(
                &ctx,
                state.authorizer.as_ref(),
                &visible_ds,
                std::slice::from_ref(&pub_rec),
                &filter,
                &lang,
            )
            .unwrap_or_else(|_| dataroom_search::SearchOutcome {
                total: 0,
                offset: 0,
                limit: dataroom_web::listing::PER_PAGE,
                datasets: Vec::new(),
                facets: Default::default(),
            });

            let html = render_organization_read_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &pub_rec,
                &outcome.datasets,
                &outcome.facets,
                outcome.total,
                page,
                &q,
                &sort,
                &q_params,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // G3: Topics (`/group/`), CKAN's group list built from the EU data themes.
        ["group"] => {
            let mut datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
            datasets.retain(|d| !is_dashboard(d));
            let q = q_params.get("q").map(String::as_str).unwrap_or("");
            let sort = q_params
                .get("sort")
                .map(String::as_str)
                .unwrap_or("title asc");
            let html = dataroom_web::groups::render_group_list_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &datasets,
                q,
                sort,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // A topic's page is its dataset search (CKAN's `/group/{name}`).
        ["group", code] => {
            // The path segment arrives percent-encoded: decode it once (a `+` stays a `+`, a raw
            // `&` stays in the value), then encode it as one query value.
            let raw = format!("v={}", code.replace('+', "%2B").replace('&', "%26"));
            let decoded = url::form_urlencoded::parse(raw.as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let enc: String = url::form_urlencoded::byte_serialize(decoded.as_bytes()).collect();
            let prefix = if had_url_prefix {
                format!("{}/{lang}", state.mount_prefix())
            } else {
                state.mount_prefix().clone()
            };
            safe_redirect(
                StatusCode::FOUND,
                &format!("{prefix}/dataset/?vocab_jc_topic={enc}"),
            )
        }
        // D6: About page (`/about`)
        ["about"] => {
            let html = render_about_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D6: Static pages (`/connector`, `/privacy`, `/terms`, `/support`, `/accessibility`)
        [page_name]
            if matches!(
                *page_name,
                "connector" | "privacy" | "terms" | "support" | "accessibility"
            ) =>
        {
            let Some(html) = render_static_page(
                &cat,
                Some(&state.branding_of(&cat)),
                page_name,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            ) else {
                return problem_response(DataroomError::NotFound("page not found".into()));
            };
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D6: Assistants page (`/assistants`)
        ["assistants"] => {
            let html = render_assistants_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            );
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D6: Assistant guide page (`/assistants/{id}`)
        ["assistants", guide_id] => {
            let Some(html) = render_assistant_guide_page(
                &cat,
                Some(&state.branding_of(&cat)),
                guide_id,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            ) else {
                return problem_response(DataroomError::NotFound(format!(
                    "assistant guide '{guide_id}' not found"
                )));
            };
            html_response_with_csp(html, &nonce, &branding, false)
        }
        // D6: Framable map page (`/map/{id}` and `/map/{id}/{resource_id}`)
        ["map", ds_id] => {
            let Some(ds) = state.store.get_dataset(&ctx, ds_id).await.ok().flatten() else {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_id}' not found"
                )));
            };
            if !map_shown(&state, &ctx, &cat, &ds).await {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_id}' not found"
                )));
            }
            let Some(html) = render_map_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &ds,
                None,
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            ) else {
                return problem_response(DataroomError::NotFound("map not available".into()));
            };
            html_response_with_csp(html, &nonce, &branding, true)
        }
        ["map", ds_id, res_id] => {
            let Some(ds) = state.store.get_dataset(&ctx, ds_id).await.ok().flatten() else {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_id}' not found"
                )));
            };
            if !map_shown(&state, &ctx, &cat, &ds).await {
                return problem_response(DataroomError::NotFound(format!(
                    "dataset '{ds_id}' not found"
                )));
            }
            let Some(html) = render_map_page(
                &cat,
                Some(&state.branding_of(&cat)),
                &ds,
                Some(res_id),
                &lang,
                had_url_prefix,
                &state.mount_prefix(),
                &nonce,
            ) else {
                return problem_response(DataroomError::NotFound("map not available".into()));
            };
            html_response_with_csp(html, &nonce, &branding, true)
        }
        _ => problem_response(DataroomError::NotFound(format!(
            "route '{raw_path}' not found"
        ))),
    }
}
