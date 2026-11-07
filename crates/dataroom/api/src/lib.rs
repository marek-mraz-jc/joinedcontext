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

pub mod ckan;
pub mod feeds;
pub mod odata_routes;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Datelike;
use dataroom_core::auth::constant_time_eq;
use dataroom_core::ops::{execute_operation, OpContext};
use dataroom_core::{
    text_in_lang, Action, Authorizer, Catalogue, CatalogueId, DataroomError, Dataset,
    DatasetFilter, ObjectTarget, OrgId, ProfilePin, ProfileRecord, Publisher, RecordState,
    RequestCtx, Result, Store, Subject, Visibility,
};
use dataroom_harvest::{HarvestEngine, HttpFetcher};
use dataroom_mcp::handle_mcp_jsonrpc;
use dataroom_rdf::{negotiate_profile, negotiate_rdf_format};
use dataroom_search::search_datasets;
use dataroom_shacl::ProfileRegistry;
use dataroom_web::{csp_header, render_dataset_page, render_home_page};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// Token entry mapping a bearer/API token to a verified [`Subject`].
#[derive(Debug, Clone)]
pub struct TokenBinding {
    pub token: String,
    pub subject: Subject,
}

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
        }
    }

    pub fn with_tokens(mut self, tokens: Vec<TokenBinding>) -> Self {
        self.tokens = Arc::new(tokens);
        self
    }

    pub fn with_harvester(mut self, harvester: Arc<HarvestEngine>) -> Self {
        self.harvester = harvester;
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
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h).trim());

        let cat = if let Some(h) = host.filter(|h| !h.is_empty() && *h != "localhost" && *h != "127.0.0.1") {
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

        let raw_token = headers
            .get(header::AUTHORIZATION)
            .or_else(|| headers.get("X-CKAN-API-Key"))
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .map(|t| t.strip_prefix("Bearer ").unwrap_or(t).trim());

        let subject = match raw_token.filter(|t| !t.is_empty()) {
            Some(tok) => {
                let matched = self
                    .tokens
                    .iter()
                    .find(|b| constant_time_eq(&b.token, tok));
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
        };

        let ctx = RequestCtx::new(cat.org_id, cat.id, subject);
        Ok((ctx, cat))
    }
}

/// Converts a [`DataroomError`] into an RFC 9457 Problem Details JSON response.
pub fn problem_response(err: DataroomError) -> Response {
    let status = StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = json!({
        "type": "about:blank",
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

/// Builds the complete Axum [`Router`] for `dataroom`.
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
        // Operation registry REST & allowed (C11)
        .route("/api/v1/ops/{op}", post(handle_op_call))
        .route("/api/v1/allowed", get(handle_allowed_query))
        // MCP JSON-RPC 2.0
        .route("/mcp", post(handle_mcp))
        // Server-rendered UI and dataset RDF content negotiation
        .route("/", get(handle_ui_home))
        .route("/dataset", get(handle_ui_home))
        .route("/dataset/{name_or_ext}", get(handle_dataset_or_rdf))
        .route("/publisher/{name}", get(handle_publisher_page))
        .with_state(state)
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
        offset: (page - 1) * 20,
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
    let (ctx, _cat) = match state.resolve_context(&headers).await {
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
    let pub_rec = state
        .store
        .get_publisher(&ctx, &ds.publisher_id)
        .await
        .ok()
        .flatten();
    let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
    if !state.authorizer.check(&ctx, Action::Read, &target).allowed {
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

async fn handle_list_profiles(
    State(state): State<DataroomState>,
    headers: HeaderMap,
) -> Response {
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

    let (saved_profile, dry_run) = match state
        .profiles
        .upload_and_dry_run(req.profile, &cat, &datasets, &pub_map)
    {
        Ok(res) => res,
        Err(e) => return problem_response(e),
    };

    if let Err(e) = state.store.upsert_profile(&ctx, saved_profile.clone()).await {
        return problem_response(e);
    }

    if req.activate {
        cat.active_profiles
            .retain(|p| p.profile_id != saved_profile.id);
        cat.active_profiles.push(ProfilePin {
            profile_id: saved_profile.id.clone(),
            version: saved_profile.version.clone(),
        });
        let _ = state.store.upsert_catalogue(cat).await;
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
    let (ctx, _cat) = match state.resolve_context(&headers).await {
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

#[derive(Debug, Deserialize)]
struct UiHomeQuery {
    q: Option<String>,
    lang: Option<String>,
}

async fn handle_ui_home(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(params): Query<UiHomeQuery>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let lang = params
        .lang
        .as_deref()
        .unwrap_or(&cat.default_language);
    let datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
    let publishers = state.store.list_publishers(&ctx).await.unwrap_or_default();
    let filter = DatasetFilter {
        q: params.q.clone().unwrap_or_default(),
        limit: 50,
        ..DatasetFilter::default()
    };
    let outcome = search_datasets(
        &ctx,
        state.authorizer.as_ref(),
        &datasets,
        &publishers,
        &filter,
        lang,
    )
    .unwrap_or_else(|_| dataroom_search::SearchOutcome {
        total: 0,
        offset: 0,
        limit: 50,
        datasets: Vec::new(),
        facets: Default::default(),
    });
    let nonce = Uuid::now_v7().simple().to_string();
    let html = render_home_page(
        &cat,
        &outcome.datasets,
        &publishers,
        params.q.as_deref().unwrap_or(""),
        lang,
        &nonce,
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp_header(&nonce, false)),
        ],
        html,
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct DatasetRouteQuery {
    #[serde(rename = "_profile")]
    profile: Option<String>,
    lang: Option<String>,
}

async fn handle_dataset_or_rdf(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(name_or_ext): Path<String>,
    Query(query): Query<DatasetRouteQuery>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };

    let (ds_name, ext) = match name_or_ext.rsplit_once('.') {
        Some((stem, e)) if matches!(e, "ttl" | "jsonld" | "rdf" | "xml" | "nt") => {
            (stem.to_owned(), Some(e))
        }
        _ => (name_or_ext, None),
    };

    let Some(ds) = (match state.store.get_dataset(&ctx, &ds_name).await {
        Ok(d) => d,
        Err(e) => return problem_response(e),
    }) else {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{ds_name}' not found"
        )));
    };
    if ds.state == RecordState::Withdrawn {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{ds_name}' not found"
        )));
    }

    let pub_rec = state
        .store
        .get_publisher(&ctx, &ds.publisher_id)
        .await
        .ok()
        .flatten();
    if pub_rec.as_ref().is_some_and(|p| p.state == RecordState::Withdrawn)
        && ctx.subject.is_anonymous()
    {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{ds_name}' not found"
        )));
    }
    let target = ObjectTarget::from_dataset(&ds, pub_rec.as_ref());
    if !state.authorizer.check(&ctx, Action::Read, &target).allowed {
        return problem_response(DataroomError::NotFound(format!(
            "dataset '{ds_name}' not found"
        )));
    }

    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok());
    if let Some(rdf_fmt) = negotiate_rdf_format(ext, accept) {
        let accept_profile = headers
            .get("Accept-Profile")
            .and_then(|v| v.to_str().ok());
        let (prof_kind, prof_uri) =
            negotiate_profile(query.profile.as_deref(), accept_profile, &cat.main_profile);
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
                (
                    header::HeaderName::from_static("content-profile"),
                    prof_uri,
                ),
            ],
            body,
        )
            .into_response();
    }

    let lang = query.lang.as_deref().unwrap_or(&cat.default_language);
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
    let nonce = Uuid::now_v7().simple().to_string();
    let html = render_dataset_page(
        &cat,
        &ds,
        pub_rec.as_ref(),
        &reports,
        can_edit,
        lang,
        &nonce,
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp_header(&nonce, false)),
        ],
        html,
    )
        .into_response()
}

async fn handle_publisher_page(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let all_pubs = state.store.list_publishers(&ctx).await.unwrap_or_default();
    if let Some(raw_pub) = all_pubs.iter().find(|p| p.name == name || p.id == name) {
        if let Some(ref target_name) = raw_pub.merged_into {
            return Redirect::permanent(&format!("/publisher/{target_name}")).into_response();
        }
    }
    let Some(pub_rec) = (match state.store.get_publisher(&ctx, &name).await {
        Ok(p) => p,
        Err(e) => return problem_response(e),
    }) else {
        return problem_response(DataroomError::NotFound(format!(
            "publisher '{name}' not found"
        )));
    };
    let datasets = state.store.list_datasets(&ctx).await.unwrap_or_default();
    let pub_datasets: Vec<Dataset> = datasets
        .into_iter()
        .filter(|d| {
            d.state == RecordState::Active
                && d.visibility == Visibility::Public
                && (d.publisher_id == pub_rec.name || d.publisher_id == pub_rec.id)
        })
        .collect();
    let nonce = Uuid::now_v7().simple().to_string();
    let title = text_in_lang(&pub_rec.title, Some(&cat.default_language))
        .unwrap_or(&pub_rec.name)
        .to_owned();
    let html = render_home_page(
        &cat,
        &pub_datasets,
        std::slice::from_ref(&pub_rec),
        &title,
        &cat.default_language,
        &nonce,
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp_header(&nonce, false)),
        ],
        html,
    )
        .into_response()
}
