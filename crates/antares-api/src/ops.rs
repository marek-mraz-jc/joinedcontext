// SPDX-License-Identifier: EUPL-1.2
//! The broker's operational surface (`Admin`): health, readiness, metrics, the tenant list, size and purge, and the dead-letter admin.

use super::*;

/// The broker's own operational surface: health, readiness, metrics, the
/// tenant list and purge, and the dead-letter admin. Health and readiness
/// are always open; the rest sits behind the admin credential when the
/// state carries one (`admin_auth.rs`, T-3569), `/q/metrics` unless the
/// deployment left it open for a scraper.
pub struct Admin;

impl Admin {
    /// Every path this surface mounts, and the only source of the route
    /// count `/q/health` reports. The destructuring in `router` binds one
    /// name per entry, so a route added to one and not the other does not
    /// compile.
    pub(crate) const PATHS: [&'static str; 12] = [
        "/health",
        "/ready",
        "/metrics",
        "/tenants",
        "/tenants/{tenant}",
        "/tenants/{tenant}/digest",
        "/tenants/{tenant}/export",
        "/tenants/{tenant}/import",
        "/tenants/{tenant}/types/{type}/attributes",
        "/dead-letters",
        "/dead-letters/{id}",
        "/dead-letters/{id}/replay",
    ];
}

impl ApiSurface for Admin {
    fn name(&self) -> &str {
        "admin"
    }

    fn prefix(&self) -> &str {
        "/q"
    }

    fn router(&self, st: AppState) -> Router<AppState> {
        let [health_p, ready_p, metrics_p, tenants_p, tenant_p, digest_p, export_p, import_p, attrs_p, dl_p, dl_id_p, replay_p] =
            Self::PATHS;
        let guard = axum::middleware::from_fn_with_state(st.clone(), admin_auth::require);
        let metrics = Self::metrics_routes(&st, metrics_p);
        let routes = Router::new()
            .route(tenants_p, get(tenants_list))
            .route(tenant_p, get(tenant_get).delete(tenant_purge))
            // ADR-0030: a Tenant's move to another broker.
            .route(digest_p, get(tenant_move::digest))
            .route(export_p, get(tenant_move::export))
            .route(import_p, post(tenant_move::import))
            // T-6001: the names a replace write would be judged on.
            .route(attrs_p, get(crate::types_attrs::stored_attributes))
            .route(dl_p, get(dead_letters_list))
            .route(dl_id_p, delete(dead_letter_delete))
            .route(replay_p, post(dead_letter_replay));
        // `/q/views` (ADR-0023 rule 10), when a loader set it: views are
        // written here, so they sit behind the admin credential with the
        // rest of /q.
        #[cfg(feature = "views")]
        let routes = match st.views.admin() {
            Some(views) => routes.merge(views.with_state(())),
            None => routes,
        };
        routes
            .route_layer(guard)
            .route(health_p, get(health))
            .route(ready_p, get(ready))
            .merge(metrics)
    }

    fn version_info(&self) -> serde_json::Value {
        serde_json::json!({"routes": Self::PATHS.len()})
    }
}

impl Admin {
    /// `/q/metrics` alone, under the credential rule of the rest of the
    /// surface: the admin router mounts it, and so does the metrics listener
    /// (`metrics_router`, T-5095). Prometheus text format; 404 until the
    /// broker installs the renderer, the api crate never depends on an
    /// exporter.
    fn metrics_routes(st: &AppState, path: &str) -> Router<AppState> {
        let metrics = Router::new().route(path, get(metrics_endpoint));
        if st.admin_auth.as_ref().is_some_and(|a| !a.metrics_open) {
            metrics.route_layer(axum::middleware::from_fn_with_state(
                st.clone(),
                admin_auth::require,
            ))
        } else {
            metrics
        }
    }

    /// The metrics listener's whole surface (ANTARES_METRICS_PORT, T-5095):
    /// `/q/metrics` and nothing else, so a NetworkPolicy that opens that
    /// port to a scraper opens no API or admin path.
    pub fn metrics_router(state: AppState) -> Router {
        let [_, _, metrics_p, ..] = Self::PATHS;
        Router::new()
            .nest(Admin.prefix(), Self::metrics_routes(&state, metrics_p))
            .with_state(state)
    }
}

/// /q/metrics — Prometheus exposition, rendered by the closure the
/// broker installed; 404 when no recorder exists (tests, embedded builds).
pub(super) async fn metrics_endpoint(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Response {
    match &state.metrics_render {
        Some(render) => Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4",
            )
            .body(axum::body::Body::from(render()))
            .unwrap_or_else(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

/// The build-time git hash (build.rs) — re-exported for --version.
pub const GIT_HASH: &str = env!("ANTARES_GIT_HASH");

/// Liveness plus the deployment's own description: which drivers are
/// running, what they are, every configured cap with its rejection counters,
/// the mounted surfaces and the notification schemes they can deliver to.
/// 503 while draining, so a load balancer stops routing here before the
/// listener stops accepting, and 503 while the store is wedged
/// ([`antares_store::CurrentStateDriver::wedged`]), so the liveness probe restarts the pod.
pub(super) async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    // Drain: the load balancer decides on the STATUS CODE, so draining has
    // to be a 503 — a 200 body saying "DRAINING" would keep traffic arriving.
    // This flips BEFORE the listener stops accepting, which is the whole point
    // of the ordering: the LB must stop routing while the socket still works.
    let draining = state.draining.load(std::sync::atomic::Ordering::Relaxed);
    // A store that can no longer serve until the process restarts (a leaked
    // pool, T-6182) fails liveness, so the kubelet restarts the pod instead
    // of leaving it not-ready for hours.
    let wedged = !draining && state.store.wedged().await;
    let code = if draining || wedged {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    let mut body = serde_json::Map::new();
    body.insert(
        "status".into(),
        (if draining {
            "DRAINING"
        } else if wedged {
            "STORE_WEDGED"
        } else {
            "UP"
        })
        .into(),
    );
    body.insert("store".into(), state.store_name.clone().into());
    body.insert(
        "temporal".into(),
        state.temporal_name.as_deref().unwrap_or("none").into(),
    );
    // Version surface: workspace version + build-time git hash
    // (build.rs), both asserted against the checkout by health_is_up.
    body.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    body.insert("commit".into(), env!("ANTARES_GIT_HASH").into());
    // What each driver runs on — engine, server version, extensions — so an
    // operator can tell two deployments of the same backend name apart. Both
    // seams answer from state captured at startup; a driver with nothing to
    // add answers an empty object and the key stays off the body.
    for (key, info) in [
        ("storeInfo", state.store.version_info()),
        ("temporalInfo", state.temporal.version_info()),
    ] {
        if info.as_object().is_some_and(|m| !m.is_empty()) {
            body.insert(key.into(), info);
        }
    }
    // Where commits serialize behind one writer, the queue depth (current,
    // peak) is the signal that decides the group-commit lever. A driver
    // without such a committer reports nothing, so the branch is the
    // driver's, not a backend name read here.
    if let Some((depth, peak)) = state.store.commit_queue() {
        body.insert("commitQueueDepth".into(), depth.into());
        body.insert("commitQueuePeak".into(), peak.into());
    }
    // The policy engine this binary was started with, and how long it has
    // to answer before the seam denies for it (ADR-0020). An operator
    // reading `allow-all` here knows no engine is attached.
    let mut policy = serde_json::Map::new();
    policy.insert("engine".into(), state.policy_name().into());
    policy.insert(
        "timeoutMs".into(),
        u64::try_from(policy::TIMEOUT.as_millis())
            .unwrap_or(u64::MAX)
            .into(),
    );
    // Configured caps + rejection counters, for observability.
    body.insert("limits".into(), state.limits.snapshot());
    // History events a driver failed to record after the write stood.
    body.insert("temporalDrainErrors".into(), history::drain_errors().into());
    // What this binary serves beside the NGSI-LD API: one entry per mounted
    // surface, its prefix and whatever the surface reports about itself.
    body.insert(
        "surfaces".into(),
        serde_json::Value::Object(
            state
                .surfaces
                .iter()
                .map(|s| {
                    let mut info = match s.version_info() {
                        serde_json::Value::Object(m) => m,
                        other => {
                            let mut m = serde_json::Map::new();
                            m.insert("info".into(), other);
                            m
                        }
                    };
                    info.insert("prefix".into(), s.prefix().into());
                    (s.name().to_owned(), serde_json::Value::Object(info))
                })
                .collect(),
        ),
    );
    // Endpoint schemes this deployment can deliver notifications to: the
    // registered bindings (6.3.8, clause 7, and any a deployment added).
    // A subscription naming a scheme absent here is refused at creation.
    body.insert("notificationSchemes".into(), state.sinks.schemes().into());
    // Notifications the delivery policy gave up on (this process, since start).
    body.insert("deadLetters".into(), notify::dead_letters_written().into());
    // Each stored view, loaded or invalid with the reason (ADR-0023 rule 10).
    // only the view reports below read the credential
    #[cfg(not(feature = "views"))]
    drop(headers);
    // The reports name Tenants and views, which `/q/tenants` keeps behind
    // the admin credential: an open caller gets their sizes (F-ANT2-005).
    #[cfg(feature = "views")]
    {
        let presented = headers
            .get(axum::http::header::AUTHORIZATION)
            .map(axum::http::HeaderValue::as_bytes);
        let admitted = state
            .admin_auth
            .as_deref()
            .is_none_or(|a| a.admits(presented));
        let shown = |report: serde_json::Value| match report {
            serde_json::Value::Object(m) if !admitted => serde_json::json!({"count": m.len()}),
            other => other,
        };
        body.insert("views".into(), shown(state.views.report()));
        // Which documents the table was loaded from (threat V26): two pods
        // reporting two values serve two predicates.
        body.insert("viewsRevision".into(), state.views.revision().into());
        // The engine's rule documents by tenant and revision, loaded or
        // refused (ADR-0020 amendment); never the documents themselves.
        policy.insert("documents".into(), shown(state.views.policy_report()));
        body.insert("schemas".into(), shown(state.views.schema_report()));
    }
    // Changes the bounded matcher queue dropped (this process, since start).
    body.insert("changesDropped".into(), notify::changes_dropped().into());
    // Panics absorbed at the notification-task boundary, each one a change
    // whose notification was lost. Reported here and not only through the
    // metrics facade: the recorder is installed by ANTARES_TELEMETRY, which
    // is off by default, so this endpoint is where a default deployment can
    // see it at all.
    body.insert("taskPanics".into(), notify::task_panics().into());
    // Jemalloc heap stats (RSS ≈ live×1.2 is the target).
    if let Some(mem) = &state.mem_stats {
        let mut m = mem();
        // T-5895: what the budget's pool holds now and the @context caches'
        // hit counts, so a share set too small shows as a falling ratio
        if let Some(o) = m.as_object_mut() {
            o.insert(
                "answerPool".into(),
                serde_json::json!({"heldBytes": state.answer_pool.held(),
                    "capBytes": state.answer_pool.cap()}),
            );
            o.insert(
                "contextCaches".into(),
                antares_jsonld::CACHE_STATS.snapshot(),
            );
        }
        body.insert("memory".into(), m);
    }
    // Bus visibility: present only when the nats wiring installed the
    // provider — bus=local carries NO bus member.
    if let Some(bus) = &state.bus_stats {
        body.insert("bus".into(), bus());
    }
    body.insert("policy".into(), serde_json::Value::Object(policy));
    (code, axum::Json(serde_json::Value::Object(body)))
}

/// The tenant an admin dead-letter call addresses: `?tenant=` (default
/// tenant when absent), grammar-checked, never a reserved internal one.
pub(super) fn admin_tenant(
    q: &std::collections::HashMap<String, String>,
) -> Result<TenantId, NgsiError> {
    let raw = q.get("tenant").map_or(TenantId::DEFAULT, String::as_str);
    let bad = || NgsiError::BadRequestData(format!("invalid tenant: {raw:?}"));
    if reserved_tenant(raw) {
        return Err(bad());
    }
    TenantId::new(raw).map_err(|_| bad())
}

/// One dead letter as the listing may show it: everything on it that can be
/// a credential is blanked, the keys are not.
///
/// `receiverInfo` and `notifierInfo` are KeyValuePair arrays (5.2.22), and
/// the example content the datatype names is the HTTP Authentication header;
/// a letter written before the bindings moved behind the registry carries the
/// same values already rendered into `headers`. `notifierInfo` is opaque to
/// every sink but the endpoint's own, so the broker cannot tell which of its
/// keys names a secret and blanks them all. The stored letter keeps the
/// values a replay has to send — only what leaves through this route loses
/// them.
pub(super) fn redact_letter(l: &mut serde_json::Value) {
    // Assigning through `Value`'s index panics on anything but an object,
    // and the row comes from the store rather than from this process.
    let Some(letter) = l.as_object_mut() else {
        return;
    };
    if let Some(u) = letter.get("uri").and_then(serde_json::Value::as_str) {
        let redacted = serde_json::Value::String(notify::redact_userinfo(u));
        letter.insert("uri".into(), redacted);
    }
    for member in ["receiverInfo", "notifierInfo", "headers"] {
        let Some(pairs) = letter
            .get_mut(member)
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for pair in pairs {
            if let Some(v) = pair.get_mut(1) {
                *v = serde_json::Value::String("[redacted]".into());
            }
        }
    }
}

/// Dead letters of one tenant, newest first; `subscription=` narrows to one
/// subscription, `limit=` bounds the page (default 100). The endpoint URI is
/// shown with its userinfo redacted and every credential the letter carries
/// is blanked.
pub(super) async fn dead_letters_list(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let tenant = match admin_tenant(&q) {
        Ok(t) => t,
        Err(e) => return ApiError::from(e).into_response(),
    };
    let limit = match q.get("limit") {
        None => 100usize,
        Some(v) => match v.parse::<usize>() {
            Ok(n) if n > 0 => n,
            _ => {
                return ApiError::from(NgsiError::BadRequestData(format!(
                    "limit must be a positive integer, got {v:?}"
                )))
                .into_response()
            }
        },
    };
    let mut letters = match st
        .store
        .list(&tenant, antares_store::Kind::DeadLetter)
        .await
    {
        Ok(l) => l,
        Err(e) => return ApiError::from(e).into_response(),
    };
    if let Some(sid) = q.get("subscription") {
        letters.retain(|l| l["subscriptionId"].as_str() == Some(sid.as_str()));
    }
    letters.sort_by(|a, b| b["lastAt"].as_str().cmp(&a["lastAt"].as_str()));
    letters.truncate(limit);
    for l in &mut letters {
        redact_letter(l);
    }
    (
        StatusCode::OK,
        axum::Json(serde_json::Value::Array(letters)),
    )
        .into_response()
}

/// Re-deliver one dead letter once through its own binding: 204 and the
/// letter is gone on success; 502 with the failure text and the letter
/// kept (attempt history extended) otherwise.
pub(super) async fn dead_letter_replay(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    use antares_store::{CurrentStateDriverExt as _, Kind};
    let tenant = match admin_tenant(&q) {
        Ok(t) => t,
        Err(e) => return ApiError::from(e).into_response(),
    };
    let letter = match st.store.get(&tenant, Kind::DeadLetter, &id).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            return ApiError::from(NgsiError::ResourceNotFound(format!("dead letter {id}")))
                .into_response()
        }
        Err(e) => return ApiError::from(e).into_response(),
    };
    match notify::replay_dead_letter(&st, &tenant, &letter).await {
        Ok(()) => match st.store.delete(&tenant, Kind::DeadLetter, &id).await {
            Ok(_) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => ApiError::from(e).into_response(),
        },
        Err(why) => {
            let ts = state::now_iso();
            let w = why.clone();
            let _ = st
                .store
                .mutate(&tenant, Kind::DeadLetter, &id, move |d| {
                    let Some(letter) = d.as_object_mut() else {
                        return Ok::<(), NgsiError>(());
                    };
                    let n = letter
                        .get("attempts")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        .saturating_add(1);
                    letter.insert("attempts".into(), n.into());
                    letter.insert("lastError".into(), serde_json::Value::String(w));
                    letter.insert("lastAt".into(), serde_json::Value::String(ts));
                    Ok::<(), NgsiError>(())
                })
                .await;
            (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({"detail": why, "id": id})),
            )
                .into_response()
        }
    }
}

/// Drop one dead letter without replaying it: 204 when it was there, 404
/// when it was not. The letter is the only copy of a notification the
/// delivery policy gave up on, so this is the one route that discards it.
pub(super) async fn dead_letter_delete(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let tenant = match admin_tenant(&q) {
        Ok(t) => t,
        Err(e) => return ApiError::from(e).into_response(),
    };
    match st
        .store
        .delete(&tenant, antares_store::Kind::DeadLetter, &id)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => {
            ApiError::from(NgsiError::ResourceNotFound(format!("dead letter {id}"))).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

/// Tenant inventory: the names the backends know, sorted. Names only —
/// at the 10 000-tenant target (ADR-0001) a list carrying per-kind counts
/// would cost a count per kind per tenant, so a client picks a name here and
/// reads its detail from `GET /q/tenants/{tenant}`. Admin surface, never
/// under the API root.
pub(super) async fn tenants_list(
    axum::extract::State(st): axum::extract::State<AppState>,
) -> Response {
    match st.store.tenant_ids().await {
        Ok(ids) => (StatusCode::OK, axum::Json(ids)).into_response(),
        Err(e) => crate::negotiate::ApiError::from(e).into_response(),
    }
}

/// What one tenant holds: 400 for a name no request could carry, 404 when it
/// does not exist (5.5.10 keeps the default Tenant existing even when
/// empty), 200 with its counts otherwise. The path names the tenant; the
/// NGSILD-Tenant header never redirects the read.
pub(super) async fn tenant_get(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> Response {
    use crate::negotiate::ApiError;
    let bad = || {
        ApiError::from(NgsiError::BadRequestData(format!(
            "invalid tenant: {raw:?}"
        )))
        .into_response()
    };
    // the broker's internal snapshot tenants are not addressable
    if reserved_tenant(&raw) {
        return bad();
    }
    let Ok(tenant) = TenantId::new(&raw) else {
        return bad();
    };
    let stats = match st.store.tenant_stats_one(&tenant).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return ApiError::from(NgsiError::ResourceNotFound(format!("tenant {raw}")))
                .into_response()
        }
        Err(e) => return ApiError::from(e).into_response(),
    };
    let usage = tenant_usage(&st, &tenant, stats.entities).await;
    let mut row = serde_json::json!({
        "tenant": stats.tenant,
        "counts": {
            "entities": stats.entities,
            "subscriptions": stats.subscriptions,
            "csourceSubscriptions": stats.csource_subscriptions,
            "registrations": stats.registrations,
            "snapshots": stats.snapshots,
            "entityMaps": stats.entity_maps,
            "distSubs": stats.dist_subs,
        },
    });
    // a count that was neither measured nor estimated is left out, never 0
    if let (Some(n), Some(counts)) = (
        usage.as_ref().and_then(|u| u.get("attrInstances")),
        row.get_mut("counts")
            .and_then(serde_json::Value::as_object_mut),
    ) {
        counts.insert("attrInstances".into(), n.clone());
    }
    if let Some(row) = row.as_object_mut() {
        if let Some(c) = stats.created_at {
            row.insert("createdAt".into(), serde_json::Value::String(c));
        }
        for key in ["bytes", "estimated", "measuredAt"] {
            if let Some(v) = usage.as_ref().and_then(|u| u.get(key)) {
                row.insert(key.into(), v.clone());
            }
        }
    }
    (StatusCode::OK, axum::Json(row)).into_response()
}

/// How long `GET /q/tenants/{tenant}` waits for the exact history count and
/// size (T-5304) before it estimates them from the store's statistics
/// (T-6085): the read stays under a second on a tenant of millions of rows.
pub const TENANT_COUNT_BOUND: std::time::Duration = std::time::Duration::from_secs(1);

/// How long `GET /q/tenants/{tenant}` reuses a measured size (T-2890).
pub const TENANT_BYTES_TTL: std::time::Duration = std::time::Duration::from_secs(300);
/// How long a size that could not be measured, or is being measured, stays
/// unanswered before the next read tries again.
pub const TENANT_BYTES_RETRY: std::time::Duration = std::time::Duration::from_secs(60);

/// One tenant's entry in [`crate::state::Shared::tenant_bytes`]: when it was
/// written and the size, `None` while it is measured or after it failed.
pub type BytesEntry = (std::time::Instant, Option<serde_json::Value>);

/// What a size read takes from the cache: `Some(answer)` while an entry is
/// fresh (a measured size for [`TENANT_BYTES_TTL`], a failure or a
/// measurement in flight for [`TENANT_BYTES_RETRY`]), or `None` after
/// claiming the measurement — the claim is an in-flight entry, so concurrent
/// misses scan once and the others answer without a size.
pub(super) fn bytes_cached_or_claim(
    map: &mut std::collections::HashMap<String, BytesEntry>,
    tenant: &str,
    now: std::time::Instant,
) -> Option<Option<serde_json::Value>> {
    if let Some((at, v)) = map.get(tenant) {
        let ttl = if v.is_some() {
            TENANT_BYTES_TTL
        } else {
            TENANT_BYTES_RETRY
        };
        if now.saturating_duration_since(*at) < ttl {
            return Some(v.clone());
        }
    }
    map.insert(tenant.to_owned(), (now, None));
    None
}

/// What `GET /q/tenants/{tenant}` adds to the counts, kept as
/// [`bytes_cached_or_claim`] says: `attrInstances`, `bytes` as
/// `{entities, attrInstances, byType}`, `estimated` and `measuredAt`. The
/// history is counted and sized in one pass under [`TENANT_COUNT_BOUND`];
/// past it both numbers come from the store's statistics and `estimated` is
/// true, as it is when the Entities' exact size scan fails and their size
/// is estimated too (T-6159). `None` when nothing could be measured or another read is
/// measuring: the counts still answer, without these.
pub(super) async fn tenant_usage(
    st: &AppState,
    tenant: &TenantId,
    entities: u64,
) -> Option<serde_json::Value> {
    let cache = || {
        st.tenant_bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    if let Some(answer) =
        bytes_cached_or_claim(&mut cache(), tenant.as_str(), std::time::Instant::now())
    {
        return answer;
    }
    let t = tenant.as_str();
    let history = match st
        .temporal
        .attr_instance_usage(tenant, TENANT_COUNT_BOUND)
        .await
    {
        Ok(h) => Some((h, false)),
        Err(NgsiError::OperationNotSupported(_)) => None,
        Err(e) => {
            tracing::warn!(tenant = t, error = %e, "tenant history not measured, estimating");
            match st.temporal.attr_instance_estimate(tenant).await {
                Ok(h) => Some((h, true)),
                Err(e) => {
                    tracing::warn!(tenant = t, error = %e, "tenant history not estimated");
                    None
                }
            }
        }
    };
    let entities = match st.store.tenant_bytes(tenant).await {
        Ok(b) => Some((b, false)),
        Err(NgsiError::OperationNotSupported(_)) => None,
        Err(e) => {
            // a big tenant's exact scan passes its bound (T-6159)
            tracing::warn!(tenant = t, error = %e, "tenant size not measured, estimating");
            match st.store.tenant_bytes_estimate(tenant, entities).await {
                Ok(b) => Some((b, true)),
                Err(e) => {
                    tracing::warn!(tenant = t, error = %e, "tenant size not estimated");
                    None
                }
            }
        }
    };
    let v = history.map(|((count, history_bytes), history_estimated)| {
        let estimated = history_estimated || entities.as_ref().is_some_and(|(_, e)| *e);
        let mut v = serde_json::json!({
            "attrInstances": count,
            "estimated": estimated,
            "measuredAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });
        if let (Some((e, _)), Some(m)) = (entities, v.as_object_mut()) {
            m.insert(
                "bytes".into(),
                serde_json::json!({
                    "entities": e.entities,
                    "attrInstances": history_bytes,
                    "byType": e.by_type,
                }),
            );
        }
        v
    });
    cache().insert(t.to_owned(), (std::time::Instant::now(), v.clone()));
    v
}

/// Purge one tenant from both backends: 400 for a name no request could
/// carry, 404 when it does not exist (5.5.10), 409 while it still holds
/// distributed subscriptions (unsubscribe them first), 204 once every
/// document of it is gone. Deleted subscriptions and registrations are
/// announced to the bus mirrors the way an API delete is.
pub(super) async fn tenant_purge(
    axum::extract::State(st): axum::extract::State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> Response {
    use crate::negotiate::ApiError;
    use antares_store::Kind;
    let bad = || {
        ApiError::from(NgsiError::BadRequestData(format!(
            "invalid tenant: {raw:?}"
        )))
        .into_response()
    };
    if reserved_tenant(&raw) {
        return bad();
    }
    let Ok(tenant) = TenantId::new(&raw) else {
        return bad();
    };
    match st.store.tenant_exists(&tenant).await {
        Ok(true) => {}
        Ok(false) => {
            return ApiError::from(NgsiError::ResourceNotFound(format!("tenant {raw}")))
                .into_response()
        }
        Err(e) => return ApiError::from(e).into_response(),
    }
    // The paged walk, not `list`: the all-at-once read carries a ceiling
    // (the Postgres arm refuses past MAX_UNDECIDED_ROWS), and a purge behind
    // that ceiling means the Tenants most worth reclaiming are the ones that
    // can never be reclaimed — while their rows stay readable to anyone
    // sending the Tenant header.
    async fn ids(st: &AppState, tenant: &TenantId, kind: Kind) -> Result<Vec<String>, NgsiError> {
        let mut out = Vec::new();
        crate::csource::walk_docs(st, tenant, kind, |doc| {
            if let Some(id) = doc.get("id").and_then(|v| v.as_str()) {
                out.push(id.to_owned());
            }
            Ok(())
        })
        .await?;
        Ok(out)
    }
    let run = async {
        // 5.8.1.4 stores one mapping document per distributed Subscription;
        // its `remotes` names the subscription copies that live AT context
        // sources. Those are deleted at their source by deleting the
        // Subscription (5.8.5.4), which a purge does not do, so a tenant
        // still holding them is refused rather than orphaning them. The
        // mapping shell of a Subscription no source has matched yet, and
        // the internal Registration Subscription beside it, hold nothing
        // remote and block nothing — the id-shaped read this used to do
        // saw neither, since a mapping document carries no `id` member.
        let mut remote_copies = false;
        crate::csource::walk_docs(&st, &tenant, Kind::DistSub, |doc| {
            if doc
                .get("remotes")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|m| !m.is_empty())
            {
                remote_copies = true;
            }
            Ok(())
        })
        .await?;
        if remote_copies {
            return Err(NgsiError::Conflict(
                "tenant holds active distributed subscriptions".into(),
            ));
        }
        let subs = ids(&st, &tenant, Kind::Subscription).await?;
        let csubs = ids(&st, &tenant, Kind::CSourceSubscription).await?;
        let regs = ids(&st, &tenant, Kind::Registration).await?;
        // 5.2.41: a Snapshot's isolated copy does not live under the Tenant.
        // It lives under the synthetic tenant its internal `__tenant` member
        // names, and the Snapshot document is the only pointer to it — so the
        // two purges below would delete that pointer and leave the copy
        // behind, reachable by nothing and freeable by nothing. Each snapshot
        // goes through the same removal a DELETE of it does, first.
        let mut snaps = Vec::new();
        crate::csource::walk_docs(&st, &tenant, Kind::Snapshot, |meta| {
            snaps.push(meta);
            Ok(())
        })
        .await?;
        for meta in &snaps {
            if let Some(id) = meta.get("id").and_then(|v| v.as_str()) {
                crate::snapshots::snap_remove(&st, &tenant, id, meta).await;
            }
        }
        // 5.13.1: a Hosted @context belongs to the Tenant that stored it.
        // Its rows go first, while the store still lists them: this pass
        // also drops the loader's warm copy of each, which the store purge
        // below cannot reach (F-ANT2-004: run after it, the pass listed
        // nothing and a Tenant recreated under the name resolved the old
        // term mappings).
        crate::contexts::purge_tenant(&st, &tenant).await?;
        st.temporal.purge_tenant(&tenant).await?;
        st.store.purge_tenant(&tenant).await?;
        st.known_tenants.forget(&tenant);
        if let Some(sync) = &st.sub_sync {
            // Kept apart: the mirror entry is keyed per kind, so one id
            // naming both a Subscription and a Context Source Registration
            // Subscription needs a tombstone for each.
            for id in &subs {
                sync(&tenant, Kind::Subscription, id, None);
            }
            for id in &csubs {
                sync(&tenant, Kind::CSourceSubscription, id, None);
            }
        }
        if let Some(sync) = &st.reg_sync {
            for id in &regs {
                sync(&tenant, id, None);
            }
        }
        Ok::<(), NgsiError>(())
    };
    if let Err(e) = run.await {
        return ApiError::from(e).into_response();
    }
    st.tenant_bytes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(tenant.as_str());
    StatusCode::NO_CONTENT.into_response()
}

/// Readiness, distinct from `/q/health` liveness: ready = not draining AND
/// the store answers a trivial request AND (when bus=nats) the bus is
/// connected. On a Postgres failover or a NATS partition the pod is still
/// alive (liveness stays 200 — a restart fixes nothing) but must stop
/// receiving traffic, so the readinessProbe points HERE.
pub(super) async fn ready(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    let draining = state.draining.load(std::sync::atomic::Ordering::Relaxed);
    let store_ok = state.store.ping().await.is_ok();
    let bus = state.bus_stats.as_ref().map(|b| b());
    let bus_ok = bus
        .as_ref()
        .is_none_or(|b| b.get("connected").and_then(serde_json::Value::as_bool) == Some(true));
    let ready = !draining && store_ok && bus_ok;
    let mut body = serde_json::json!({
        "status": if ready { "READY" } else { "NOT_READY" },
        "store": store_ok,
    });
    if let (Some(b), Some(body)) = (bus, body.as_object_mut()) {
        body.insert("bus".into(), b);
    }
    let code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, axum::Json(body))
}

/// CIM 009 5.15.1 / 6.33 — Context Source identity.
/// 5.15.1 Retrieve Context Source Identity Information: the 5.2.40
/// ContextSourceIdentity object for this source (per tenant in the
/// multi-tenancy case). The NotImplemented arm of 5.15.1.4 is vacuous here —
/// this broker can always supply its identity.
pub(super) async fn source_identity(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: HeaderMap,
) -> Response {
    let go = async {
        let tenant = tenant_from(&headers)?;
        gate!(state, &tenant, &headers, "5.15.1").await?;
        let accept = negotiate::parse_accept(&headers)?;
        let ctx = state.loader.core();
        let uptime = state.started.elapsed().as_secs();
        // Table 5.2.40-1: contextSourceAlias, contextSourceUptime and
        // contextSourceTimeAt are all cardinality 1. `hostAlias`/`uptime` were
        // not spec members at all — neither expands to an NGSI-LD IRI, so the
        // payload was not valid JSON-LD against the core context either.
        // Table 5.2.40-1: in the multi-tenancy case the alias "shall be
        // identifying a specific Tenant within a registered Context Source",
        // so what this resource serves depends on NGSILD-Tenant — a peer
        // retrieves it per tenant and registers it as `contextSourceAlias`.
        let alias = crate::federation::alias_for(&state.host_alias, &tenant);
        let body = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceIdentity:{alias}"),
            "type": "ContextSourceIdentity",
            "contextSourceAlias": alias,
            "contextSourceUptime": format!("PT{uptime}S"),
            "contextSourceTimeAt": crate::state::now_iso(),
        });
        Ok::<_, ApiError>(respond(
            axum::http::StatusCode::OK,
            body,
            &ctx,
            accept,
            &tenant,
        ))
    };
    go.await.unwrap_or_else(|e| e.into_response())
}
