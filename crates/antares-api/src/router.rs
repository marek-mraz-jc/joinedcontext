// SPDX-License-Identifier: EUPL-1.2
//! Router assembly: the NGSI-LD route table, the ops-only router and the request layers (tenant, CORS, metrics, trailing slash, 405/501/OPTIONS).

use super::*;
use tower::layer::util::Stack;
use tower_http::compression::predicate::{And, NotForContentType, Predicate, SizeAbove};
use tower_http::compression::{CompressionLayer, CompressionLevel};

/// 4.3.5 NGSI-LD API structure: Core API mandatory; Distributed API mandatory for
/// distributed/federated deployments; Temporal API and Registry API integrated
/// locally here (Table 4.3.5-2 row "integrated temporal + integrated Context
/// Registry"); JSONLDContext API implemented; optional Snapshot API offered
/// (5.16, resources 6.36-6.38, NGSILD-Snapshot scoping 6.3.22).
/// Ops-only router: what a pod WITHOUT the api role serves —
/// health, readiness and metrics, nothing else. The shipped antares-worker
/// Deployment used to answer the full read/write NGSI-LD API on its pod IP
/// (and a subscription created there was never KV-synced, because the sync
/// hooks are wired `if roles.api`); a worker now 404s the API surface.
pub fn ops_router(state: AppState) -> Router {
    // Admin only, deliberately: a worker widening its surface to whatever a
    // deployment registered is the same class of accident this router exists
    // to prevent.
    Router::new()
        .nest(Admin.prefix(), Admin.router(state.clone()))
        .with_state(state)
}

/// The NGSI-LD API, path by path: `router()` mounts exactly these under the
/// API root, and the policy seam's own test walks the same list, asking the
/// router (OPTIONS, the Allow set) which methods each path serves.
pub fn api_routes() -> Vec<(&'static str, MethodRouter<AppState>)> {
    vec![
        // entities (6.4/6.5)
        (
            "/entities",
            post(entities::create_entity)
                .get(entities::query_entities)
                .delete(entities::purge_entities)
                .patch(missing_entity_id)
                .put(missing_entity_id),
        ),
        (
            "/entities/{id}",
            get(entities::retrieve_entity)
                .patch(entities::merge_entity)
                .put(entities::replace_entity)
                .delete(entities::delete_entity),
        ),
        // attrs (6.6/6.7)
        (
            "/entities/{id}/attrs",
            post(attrs::append_attrs).patch(attrs::update_attrs),
        ),
        (
            "/entities/{id}/attrs/{attr}",
            // GET is the 2.0 #14 pre-adoption
            get(entities::retrieve_entity_attr)
                .patch(attrs::partial_update_attr)
                .put(attrs::replace_attr)
                .delete(attrs::delete_attr),
        ),
        // 2.0 #15 pre-adoption: the bare attribute value, and PUT is Set
        // Attribute Value (TS 104 176 7.6.3.2)
        (
            "/entities/{id}/attrs/{attr}/value",
            get(entities::retrieve_entity_attr_value).put(attrs::set_attr_value),
        ),
        // batch (6.14–6.17, 6.23, 6.31)
        ("/entityOperations/create", post(batch::batch_create)),
        ("/entityOperations/upsert", post(batch::batch_upsert)),
        ("/entityOperations/update", post(batch::batch_update)),
        ("/entityOperations/delete", post(batch::batch_delete)),
        ("/entityOperations/merge", post(batch::batch_merge)),
        ("/entityOperations/query", post(batch::batch_query)),
        // EntityMaps (5.14; 6.32/6.34/6.35)
        (
            "/entityMaps",
            get(entity_maps::create_entity_map).post(entity_maps::create_entity_map_post),
        ),
        (
            "/entityMaps/{id}",
            get(entity_maps::retrieve_entity_map)
                .patch(entity_maps::update_entity_map)
                .delete(entity_maps::delete_entity_map),
        ),
        (
            "/temporal/entityMaps",
            get(entity_maps::create_temporal_entity_map)
                .post(entity_maps::create_temporal_entity_map_post),
        ),
        // subscriptions (6.10/6.11)
        (
            "/subscriptions",
            post(subscriptions::create_subscription).get(subscriptions::query_subscriptions),
        ),
        (
            "/subscriptions/{id}",
            get(subscriptions::retrieve_subscription)
                .patch(subscriptions::update_subscription)
                .delete(subscriptions::delete_subscription),
        ),
        // csourceRegistrations (6.8/6.9)
        (
            "/csourceRegistrations",
            post(csource::create_registration).get(csource::query_registrations),
        ),
        (
            "/csourceRegistrations/{id}",
            get(csource::retrieve_registration)
                .patch(csource::update_registration)
                .delete(csource::delete_registration),
        ),
        // csourceSubscriptions (6.12/6.13)
        (
            "/csourceSubscriptions",
            post(subscriptions::create_csource_subscription)
                .get(subscriptions::query_csource_subscriptions),
        ),
        (
            "/csourceSubscriptions/{id}",
            get(subscriptions::retrieve_csource_subscription)
                .patch(subscriptions::update_csource_subscription)
                .delete(subscriptions::delete_csource_subscription),
        ),
        // temporal (6.18–6.22, 6.24)
        (
            "/temporal/entities",
            post(temporal::upsert_temporal).get(temporal::query_temporal),
        ),
        (
            "/temporal/entities/{id}",
            get(temporal::retrieve_temporal).delete(temporal::delete_temporal),
        ),
        (
            "/temporal/entities/{id}/attrs",
            post(temporal::add_temporal_attrs),
        ),
        (
            "/temporal/entities/{id}/attrs/{attr}",
            delete(temporal::delete_temporal_attr),
        ),
        (
            "/temporal/entities/{id}/attrs/{attr}/{instance}",
            patch(temporal::modify_temporal_instance).delete(temporal::delete_temporal_instance),
        ),
        (
            "/temporal/entityOperations/query",
            post(temporal::batch_temporal_query),
        ),
        // discovery (6.25–6.28)
        ("/types", get(types_attrs::entity_types)),
        ("/types/{type}", get(types_attrs::entity_type_info)),
        ("/attributes", get(types_attrs::attributes)),
        ("/attributes/{attr}", get(types_attrs::attribute_info)),
        // jsonldContexts (6.29/6.30)
        (
            "/jsonldContexts",
            post(contexts::add_context).get(contexts::list_contexts),
        ),
        (
            "/jsonldContexts/{id}",
            get(contexts::serve_context).delete(contexts::delete_context),
        ),
        // snapshots (5.16; 6.36-6.38, optional API group — offered)
        (
            "/snapshots",
            post(snapshots::create_snapshot).delete(snapshots::purge_snapshots),
        ),
        (
            "/snapshots/{id}",
            get(snapshots::retrieve_snapshot)
                .patch(snapshots::update_snapshot)
                .delete(snapshots::delete_snapshot),
        ),
        ("/snapshots/{id}/clone", post(snapshots::clone_snapshot)),
        // info (6.33)
        ("/info/sourceIdentity", get(source_identity)),
    ]
}

/// The broker's whole HTTP surface, under trailing-slash normalization: every
/// host of it (the binary, the browser build, an in-process forward, a test)
/// serves `/entities/{id}/attrs/` as `/entities/{id}/attrs`, the form 5.6.2
/// and 5.6.3 forwards are sent in (T-3701).
pub fn router(state: AppState) -> Router {
    let served = routes(state);
    Router::new().fallback_service(tower::util::MapRequest::new(
        served,
        trim_trailing_slash::<axum::body::Body>,
    ))
}

/// gzip or br for a client whose Accept-Encoding asks for it (T-5847), laid
/// over [`router()`] by the binary that serves the network and by nothing else:
/// an in-process call (`AppState::call`, a forward) parses the body it gets
/// back and must read plain JSON, and each nested frame would grow the stack
/// a façade chain spends. A body that already has a Content-Encoding or a
/// Content-Range is never touched, and `Vary: Accept-Encoding` is appended
/// once to whatever Vary the handler set. Request bodies stay as sent: no
/// decompression, so the body bound holds.
///
/// Levels (T-5849, the best size per CPU on real answers): brotli quality 2
/// and gzip level 1, each its own layer because tower-http gives one layer one
/// quality. The br layer sits inside: it packs whenever the client accepts br,
/// and the gzip layer outside leaves an encoded answer alone, so gzip serves
/// only a client without br. [`CompressWhen`] leaves a body of known size
/// under 1 KiB plain, and text/event-stream, images and gRPC.
pub fn compression() -> Stack<CompressionLayer<CompressWhen>, CompressionLayer<CompressWhen>> {
    let br = CompressionLayer::new()
        .gzip(false)
        .quality(CompressionLevel::Precise(2))
        .compress_when(compress_when());
    let gzip = CompressionLayer::new()
        .br(false)
        .quality(CompressionLevel::Precise(1))
        .compress_when(compress_when());
    Stack::new(br, gzip)
}

/// The predicate of [`compression()`]: tower-http's default with its 32-byte
/// minimum raised to 1 KiB, since a smaller body saves under 700 bytes.
pub type CompressWhen =
    And<And<And<SizeAbove, NotForContentType>, NotForContentType>, NotForContentType>;

fn compress_when() -> CompressWhen {
    SizeAbove::new(1024)
        .and(NotForContentType::GRPC)
        .and(NotForContentType::IMAGES)
        .and(NotForContentType::SSE)
}

/// tower-http's `NormalizePath::trim_trailing_slash`, except below a view's
/// OData service (`/x/views/{view}/odata/`): OData 4.01 Part 2 section 4 has
/// no empty path segment, so `Products/` is a malformed URL there and not a
/// spelling of `Products` (T-3731), and the path reaches the OData parser as
/// sent. Anywhere else, trailing slashes and a leading `//` collapse to one
/// path, `/` alone for a path of slashes.
pub fn trim_trailing_slash<B>(mut req: axum::http::Request<B>) -> axum::http::Request<B> {
    let path = req.uri().path();
    if !path.ends_with('/') && !path.starts_with("//") {
        return req;
    }
    let below_odata = path
        .strip_prefix(surface::VIEW_ROOT)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(_, rest)| rest.starts_with("odata/"));
    if below_odata {
        return req;
    }
    let trimmed = format!("/{}", path.trim_matches('/'));
    let path_and_query = match req.uri().query() {
        Some(query) => format!("{trimmed}?{query}"),
        None => trimmed,
    };
    let mut parts = req.uri().clone().into_parts();
    // A path cut from a valid one stays valid; were it ever not, the request
    // goes on as sent rather than failing here.
    if let Ok(pq) = path_and_query.parse() {
        parts.path_and_query = Some(pq);
        if let Ok(uri) = axum::http::Uri::from_parts(parts) {
            *req.uri_mut() = uri;
        }
    }
    req
}

pub(super) fn routes(state: AppState) -> Router {
    let api = api_routes()
        .into_iter()
        .fold(Router::new(), |api, (path, methods)| {
            api.route(path, methods)
        });

    // 5.8.1.4 consumer half: where forwarded subscription copies point their
    // notifications; remapped to the original subscriber. CIM 009 defines no
    // path for it — 5.2.15 makes a notification endpoint any URI, and 6.2
    // standardizes only what hangs under the API root — so it lives outside
    // the `/ngsi-ld` prefix ETSI owns, under this broker's own versioned
    // peer-facing root (ADR-0019). Being outside the API nest, it carries the
    // bounds wall and the body limit itself: a peer-facing write path must
    // not be the one route where the documented caps do not apply.
    let remote_notify = Router::new()
        .route("/ex/v1/remote-notify", post(distsub::remote_notify))
        .layer(axum::extract::DefaultBodyLimit::max(
            *bounds::MAX_BODY_BYTES,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            bounds::bounds_layer,
        ));

    // Every registered surface, each under its own reserved prefix — the
    // admin one by default. `with_surface` already refused a prefix outside
    // /q and /x and any overlap, so the merge here cannot shadow a route;
    // the view surfaces share the view root (ADR-0027 section 1), where
    // axum's merge refuses two equal routes.
    // The caps are the broker's, not the API nest's: these routes carry the
    // bounds wall and the body limit for the same reason the peer-facing
    // notification endpoint above does — /q deletes Tenants and replays dead
    // letters, and a deployment's own /x routes are no less a way in.
    let mut surfaces = Router::new();
    let mut view_root: Option<Router<AppState>> = None;
    for s in state.surfaces.iter() {
        let routes = s.router(state.clone());
        if s.prefix() == surface::VIEW_ROOT {
            view_root = Some(match view_root {
                Some(shared) => shared.merge(routes),
                None => routes,
            });
        } else {
            surfaces = surfaces.nest(s.prefix(), routes);
        }
    }
    if let Some(shared) = view_root {
        surfaces = surfaces.nest(surface::VIEW_ROOT, shared);
    }
    let surfaces = surfaces
        .layer(axum::extract::DefaultBodyLimit::max(
            *bounds::MAX_BODY_BYTES,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            bounds::bounds_layer,
        ));

    surfaces
        .merge(remote_notify)
        // 6.3.6/6.3.21: Prefer: ngsi-ld=<version> → 4.3.6.8 amendment +
        // Preference-Applied (+203 when altered) on every API response.
        // OPTIONS (2.0 #59 pre-adoption): axum's MethodRouter already
        // computes the exact per-route Allow set for its 405s — the layer
        // turns an OPTIONS 405 into 204 + that same Allow. HEAD (#58) needs
        // nothing: axum's get() serves HEAD natively.
        .nest(
            API_ROOT,
            api.layer(axum::middleware::from_fn(conformance::prefer_version_layer))
                // ADR-N-066 2.4: the metadata members a read's grants pass,
                // stripped from the answer once, whichever surface asked.
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    policy::metadata::layer,
                ))
                // The temporal seam's drain: every write's history events
                // land in one driver call once the handler is done.
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    history::layer,
                ))
                // 6.3.22 / 5.5.15: NGSILD-Snapshot resolves to the
                // snapshot's synthetic tenant, so the handlers below serve
                // the frozen copy.
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    snapshots::snapshot_layer,
                ))
                // 5.5.10: non-create operations targeting a non-existing
                // Tenant answer NonexistentTenant 404; create operations
                // implicitly create the Tenant. Outside the snapshot layer,
                // so a client naming one of the broker's own internal
                // tenants is refused before that layer legitimately sets
                // one.
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    tenant_exists_layer,
                ))
                // T-5032: a request without a subject needs the admin
                // bearer, before the tenant lookup so a 404 tells such a
                // caller nothing about which Tenants exist.
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    crate::admin_auth::subject_or_admin,
                ))
                // Bounds wall: URI length, body size, JSON depth — checked
                // before any parse (size-check-before-parse), and outside the
                // tenant and snapshot lookups so 6.3.4's bare 411/414 is not
                // spent on a store round-trip or masked by its 404.
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    bounds::bounds_layer,
                ))
                // axum's built-in extractor limit defaults to 2 MiB and fires
                // BEFORE the bounds wall's documented cap — a 3 MiB body was
                // 413'd although /q/health advertises maxBodyBytes = 4 MiB
                // (found by mutation-testing the 413 wall). One
                // number governs both walls.
                .layer(axum::extract::DefaultBodyLimit::max(
                    *bounds::MAX_BODY_BYTES,
                ))
                // 6.3.14, last so it sees every exit: outside the snapshot
                // layer, which rewrites the request header to the snapshot's
                // internal tenant, and outside the bounds wall, whose bare
                // 411/414 leave through no handler at all.
                .layer(axum::middleware::from_fn(echo_tenant_layer)),
        )
        .fallback(not_found)
        // T-3531: a panicking handler answers 500 instead of unwinding the
        // connection task; inside the metrics layer so the 500 is counted.
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(panic_500))
        // outermost on purpose: axum attaches the 405 Allow header in the
        // Router itself, above any nested layer — only a layer wrapping the
        // WHOLE router sees it.
        .layer(axum::middleware::from_fn(options_204))
        // TS 104 176 6.3.2: an unrecognised verb is 501 before any routing,
        // so the method router's 405 stays the answer for a recognised one.
        .layer(axum::middleware::from_fn(unrecognised_method_501))
        // Outermost so the duration covers the full stack.
        .layer(axum::middleware::from_fn(http_metrics_layer))
        // absent knob = no layer at all: a bare OPTIONS keeps its 204 + Allow
        .layer(tower::util::option_layer(cors_layer()))
        .with_state(state)
}

/// A handler that panicked: 500 InternalError, and the broker goes on
/// serving. The panic message stays in the log (the default hook prints it),
/// never in the body, where it could carry a stored value.
pub(super) fn panic_500(_: Box<dyn std::any::Any + Send + 'static>) -> Response {
    ApiError::from(NgsiError::InternalError("the request failed".into())).into_response()
}

/// Browser access is off unless ANTARES_CORS_ORIGINS names the origins
/// (comma-separated, or `*` for any). NGSI-LD says nothing about CORS; the
/// Link header and the NGSILD-Tenant / NGSILD-Results-Count headers are
/// exposed so a browser client can read them.
pub(super) fn cors_layer() -> Option<tower_http::cors::CorsLayer> {
    use tower_http::cors::{AllowOrigin, CorsLayer};
    let spec = std::env::var("ANTARES_CORS_ORIGINS")
        .ok()
        .filter(|s| !s.trim().is_empty())?;
    let origin = if spec.trim() == "*" {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(
            spec.split(',')
                .filter_map(|o| o.trim().parse::<axum::http::HeaderValue>().ok()),
        )
    };
    let layer = CorsLayer::new()
        .allow_origin(origin)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
        .expose_headers([
            axum::http::header::LINK,
            axum::http::HeaderName::from_static("ngsild-tenant"),
            axum::http::HeaderName::from_static("ngsild-results-count"),
        ]);
    Some(layer)
}

/// Tenants the broker mints for its own bookkeeping: the snapshot module's
/// internal and per-snapshot tenants share a prefix, the distributed
/// subscription inbound index is a single fixed name. A client-supplied
/// tenant may not name any of them — it would put request-shaped writes in
/// the same keyspace the broker keeps its own state in. The names live on
/// `TenantId`, where the constructor a client's name goes through refuses
/// them; the admin surfaces below ask by name, before any parse.
pub(super) fn reserved_tenant(raw: &str) -> bool {
    TenantId::is_reserved_str(raw)
}

/// 5.5.10 Multi-Tenant Behaviour: Tenants are created implicitly by create
/// operations (Create Entity 5.6.1, Batch Create/Upsert 5.6.7/5.6.8, Create
/// Temporal 5.6.11, Create Subscription 5.8.1, Register Context Source
/// 5.9.2, Create CSource Subscription 5.11.2); "all other NGSI-LD
/// operations … that target a non-existing Tenant should raise an error of
/// type NonexistentTenant". Malformed tenant headers pass through — the
/// handler's own parse answers 400.
pub(super) async fn tenant_exists_layer(
    axum::extract::State(st): axum::extract::State<AppState>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    // 6.3.14: the tenant namespace the broker mints for itself is not a
    // legal client tenant. Snapshots run on internal tenants named
    // "snap-index" (the synthetic-tenant reverse index) and "snap-<uuid>"
    // (one per snapshot); a client naming one would read and delete another
    // tenant's snapshot bookkeeping. Rejected here, ahead of the snapshot
    // layer that legitimately rewrites the header to such a tenant.
    // Read once, and refuse a request that names two Tenants (or one in a
    // form no header can carry): the guard below and the handler further in
    // have to be deciding about the same value, or the wall inspects one
    // name while the operation runs on another.
    let named = match crate::negotiate::single_header(req.headers(), "NGSILD-Tenant") {
        Ok(named) => named,
        Err(e) => return e.into_response(),
    };
    if let Some(raw) = &named {
        if reserved_tenant(raw) {
            return crate::negotiate::ApiError::from(NgsiError::BadRequestData(format!(
                "invalid NGSILD-Tenant value: {raw:?}"
            )))
            .into_response();
        }
    }
    let path = req.uri().path().trim_start_matches(API_ROOT);
    // 7.8.2 lists the creating operations; Batch Upsert creates Entities too
    // and joins them unless ANTARES_IMPLICIT_TENANTS=listed (T-5497).
    let implicit_create = req.method() == axum::http::Method::POST
        && (matches!(
            path,
            "/entities"
                | "/entityOperations/create"
                | "/temporal/entities"
                | "/subscriptions"
                | "/csourceRegistrations"
                | "/csourceSubscriptions"
        ) || (path == "/entityOperations/upsert" && !st.profile.implicit_tenants_listed_only));
    // tenant-independent resources: broker identity (6.33) answers for any
    // tenant (the per-tenant alias exists before any data does), and the
    // @context resources are keyed by local id across tenants (5.13) — a
    // Cached row belongs to no tenant, so the existence of the requesting
    // one decides nothing here. Ownership of a Hosted row is still enforced,
    // by `contexts::row_visible` on every serve, list and delete. A POST
    // stores a Hosted row its Tenant owns, so that Tenant must exist
    // (F-ANT2-006): a row of a Tenant that never existed could not be
    // purged with it. DELETE stays free so such a row can still go.
    let tenant_free = path.starts_with("/info/")
        || (path.starts_with("/jsonldContexts") && req.method() != axum::http::Method::POST);
    if !implicit_create && !tenant_free {
        if let Some(t) = named
            .as_deref()
            .and_then(|s| antares_model::TenantId::new(s).ok())
        {
            // T-5500: a Tenant the store said exists a moment ago is not
            // asked about again; only "exists" is remembered.
            let known = st.known_tenants.holds(&t);
            let exists = if known {
                Ok(true)
            } else {
                st.store.tenant_exists(&t).await
            };
            match exists {
                Ok(false) => {
                    let mut resp = crate::negotiate::ApiError::from(NgsiError::NonexistentTenant(
                        format!("tenant {} does not exist", t.as_str()),
                    ))
                    .into_response();
                    // 6.3.14: a request-supplied NGSILD-Tenant SHALL be
                    // present in the response — error responses included.
                    crate::negotiate::echo_tenant(&t, &mut resp);
                    return resp;
                }
                Err(e) => return crate::negotiate::ApiError::from(e).into_response(),
                Ok(true) if !known => st.known_tenants.remember(&t),
                Ok(true) => {}
            }
        }
    }
    next.run(req).await
}

/// 6.3.14: "If the HTTP header `NGSILD-Tenant` is present in the HTTP
/// request, it shall also be present in HTTP response." The sentence admits
/// no exception, and the responses that most need it are the ones no handler
/// shapes: a rejection from the bounds wall, a 405, a body that never parsed.
/// An echo the handlers own is an echo some handler forgets, so it is read
/// once here, from the request the CALLER sent — the snapshot layer below
/// rewrites that header to the snapshot's internal tenant, which must never
/// reach the caller.
pub(super) async fn echo_tenant_layer(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    // A repeated header names no Tenant, so there is none to echo; the
    // BadRequestData the wall answers says so on its own.
    let tenant = crate::negotiate::single_header(req.headers(), "NGSILD-Tenant")
        .ok()
        .flatten()
        .and_then(|s| TenantId::new(&s).ok());
    let mut resp = next.run(req).await;
    if let Some(t) = tenant {
        if !resp.headers().contains_key("NGSILD-Tenant") {
            crate::negotiate::echo_tenant(&t, &mut resp);
        }
    }
    resp
}

/// Bounded method label for the request metrics: standard HTTP methods
/// pass through, anything else (extension methods are minted by the
/// client) collapses to "OTHER" so the label's cardinality stays bounded.
pub(super) fn metric_method(m: &str) -> &'static str {
    match m {
        "GET" => "GET",
        "POST" => "POST",
        "PATCH" => "PATCH",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "OPTIONS" => "OPTIONS",
        "HEAD" => "HEAD",
        _ => "OTHER",
    }
}

/// Request counter + duration histogram, labelled by method and status
/// class only (bounded cardinality).
pub(super) async fn http_metrics_layer(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let method = metric_method(req.method().as_str());
    // Clock rule: std Instant panics on wasm32.
    #[cfg(not(target_arch = "wasm32"))]
    let start = std::time::Instant::now();
    #[cfg(target_arch = "wasm32")]
    let start = web_time::Instant::now();
    let resp = next.run(req).await;
    let class = match resp.status().as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    };
    metrics::counter!("antares_http_requests_total", "method" => method, "status" => class)
        .increment(1);
    metrics::histogram!("antares_http_request_duration_seconds", "method" => method)
        .record(start.elapsed().as_secs_f64());
    resp
}

/// TS 104 176 6.3.2: "Not Implemented (501) ... when a client invokes an
/// HTTP verb that is unrecognized or not implemented". The recognised set is
/// the one [`metric_method`] names; CONNECT, TRACE and every extension
/// method are 501. A bare status like 6.3.4's 411 and 415: the error table
/// names no error type for it.
pub(super) async fn unrecognised_method_501(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    if metric_method(req.method().as_str()) == "OTHER" {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    }
    next.run(req).await
}

/// 2.0 #59 pre-adoption: OPTIONS → 204 No Content + the Allow set the
/// method router computed. Non-OPTIONS traffic passes through untouched.
pub(super) async fn options_204(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let is_options = req.method() == axum::http::Method::OPTIONS;
    let resp = next.run(req).await;
    if is_options && resp.status() == axum::http::StatusCode::METHOD_NOT_ALLOWED {
        // Keep the response PARTS: axum carries the computed Allow set as a
        // response extension and turns it into the header above all layers —
        // preserving the extensions preserves the Allow header.
        let (mut parts, _) = resp.into_parts();
        parts.status = axum::http::StatusCode::NO_CONTENT;
        return Response::from_parts(parts, axum::body::Body::empty());
    }
    resp
}

/// PATCH/PUT on the entities collection: the entity id is missing — 400.
pub(super) async fn missing_entity_id(headers: HeaderMap) -> Response {
    let tenant = tenant_from(&headers).unwrap_or_default();
    let mut resp = ApiError::from(NgsiError::BadRequestData(
        "entity id is required in the request path".into(),
    ))
    .into_response();
    echo_tenant(&tenant, &mut resp);
    resp
}

pub(super) async fn not_found(headers: HeaderMap, uri: axum::http::Uri) -> Response {
    let path = uri.path().to_owned();
    let tenant = tenant_from(&headers).unwrap_or_default();
    // A path with an empty segment (…/attrs//{x}) names a resource whose
    // methods don't apply — 405 per the suite (016_02_04/06).
    if path.starts_with(API_ROOT) && path.contains("//") {
        let mut resp = axum::http::StatusCode::METHOD_NOT_ALLOWED.into_response();
        echo_tenant(&tenant, &mut resp);
        return resp;
    }
    let mut resp =
        ApiError::from(NgsiError::ResourceNotFound(format!("unknown path {path}"))).into_response();
    echo_tenant(&tenant, &mut resp);
    resp
}

#[cfg(test)]
mod trim_trailing_slash_tests {
    use super::trim_trailing_slash;

    fn trimmed(uri: &str) -> String {
        let req = axum::http::Request::builder()
            .uri(uri)
            .body(())
            .unwrap_or_default();
        trim_trailing_slash(req).uri().to_string()
    }

    /// The NGSI-LD API and every other surface: tower-http's trim, the query
    /// kept. A view's OData service: as sent (T-3731).
    #[test]
    fn trims_everywhere_but_a_views_odata_service() {
        for (sent, served) in [
            ("/ngsi-ld/v1/entities/", "/ngsi-ld/v1/entities"),
            (
                "/ngsi-ld/v1/entities//?type=Car",
                "/ngsi-ld/v1/entities?type=Car",
            ),
            ("//ngsi-ld/v1/entities", "/ngsi-ld/v1/entities"),
            ("/", "/"),
            ("///", "/"),
            ("/x/views/", "/x/views"),
            ("/x/views/a/", "/x/views/a"),
            (
                "/x/views/a/ngsi-ld/v1/entities/",
                "/x/views/a/ngsi-ld/v1/entities",
            ),
            ("/x/views/a/odata", "/x/views/a/odata"),
            ("/x/views/a/odata/v4/", "/x/views/a/odata/v4/"),
            ("/x/views/a/odata/v4/Car/", "/x/views/a/odata/v4/Car/"),
            (
                "/x/views/a/odata/v4/Car//?$top=1",
                "/x/views/a/odata/v4/Car//?$top=1",
            ),
            ("/q/views/odata/", "/q/views/odata"),
        ] {
            assert_eq!(trimmed(sent), served, "{sent}");
        }
    }
}
