// SPDX-License-Identifier: EUPL-1.2
//! NGSI-LD HTTP binding: axum routers, thin
//! handlers per spec operation.
// T-3915, T-3932: a panic on a request path is a remote DoS. Outside tests
// this crate may not unwrap, expect, panic, index or overflow.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::print_stdout,
        clippy::print_stderr,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
// the Send proof of the spawned snapshot fill walks temporal → entity map
// futures deeper than the default 128 (nightly: recursion_depth_exceeding_limit)
#![recursion_limit = "256"]

/// Ask the policy engine about one operation (ADR-0020), where the request
/// enters its handler:
///
/// ```ignore
/// gate!(st, &tenant, headers, "5.6.6", ids: &[&id]).await?;
/// ```
///
/// The named members go into the [`policy::Operation`]; every other member
/// keeps the empty default of `Operation::new`. The awaited value is the
/// `Filter` the engine narrowed the operation to, shared (`Arc`) so a
/// view's compiled narrowing reaches the handler uncopied, or the refusal that
/// becomes a 403. It is a macro because a gate reads as one line at the
/// call site and the alternative is the same eight lines in every handler.
///
/// With no engine attached there is nothing to ask, so nothing is built:
/// the operation, the subject and its headers, the boxed future and the
/// [`policy::TIMEOUT`] timer all belong to the arm that has an engine to
/// ask. A broker running the default pays one branch per operation.
macro_rules! gate {
    ($st:expr, $tenant:expr, $headers:expr, $clause:expr $(, $field:ident: $value:expr)* $(,)?) => {
        async {
            let answer = match &$st.policy {
                None => Ok($crate::policy::unnarrowed()),
                Some(engine) => {
                    $crate::policy::gate(
                        engine.as_ref(),
                        &$crate::snapshots::asking_tenant(&$st, $tenant).await,
                        $headers,
                        &$crate::policy::Operation {
                            $($field: $value,)*
                            ..$crate::policy::Operation::new($clause)
                        },
                    )
                    .await
                    .map(std::sync::Arc::new)
                }
            };
            // A state served through an endpoint view adds the view's
            // narrowing to the engine's (ADR-0023, T-3494).
            #[cfg(feature = "views")]
            let answer = $crate::views::with_view($st.view.as_deref(), $clause, answer);
            answer
        }
    };
}

pub mod admin_auth;
// Inbound bridge Channels (ADR-0031): `/x/channels`, off in the vanilla build.
pub(crate) mod attrs;
pub(crate) mod batch;
pub mod bounds;
#[cfg(all(feature = "channels", not(target_arch = "wasm32")))]
pub mod channels;
pub mod conformance;
pub(crate) mod contexts;
pub(crate) mod csource;
pub(crate) mod distsub;
pub mod egress;
pub(crate) mod entities;
pub(crate) mod entity_map;
pub(crate) mod entity_maps;
pub(crate) mod federation;
// The request parsers for the fuzz targets (T-3463); never in a broker build.
pub mod erasure;
#[cfg(feature = "fuzzing")]
pub mod fuzzing;
pub mod history;
pub mod mirror;
pub mod negotiate;
pub mod notify;
pub(crate) mod paging;
pub mod policy;
pub mod provenance;
pub(crate) mod registry;
pub(crate) mod repr;
/// The one GeoJSON geometry rule (T-5208), for the surfaces that write GeoJSON.
pub use repr::{geometry_of, rewound};
pub mod secret;
pub(crate) mod snapshots;
pub(crate) mod stamp;
pub mod state;
pub(crate) mod subscriptions;
pub(crate) mod temporal;
pub(crate) mod temporalq;
pub(crate) mod tenant_move;
pub(crate) mod types_attrs;
// The composition hook of endpoint views (ADR-0023, ADR-0027 section 5);
// never in a vanilla build.
mod ops;
mod router;
#[cfg(feature = "views")]
pub mod schema_writes;
#[cfg(test)]
mod tests;
#[cfg(feature = "views")]
pub mod views;

pub use antares_notifier::DeliveryPolicy;
// The sink contract, so a surface reaches it through this crate and names no
// antares-notifier (ADR-0027 section 1, rule M2).
pub use antares_notifier::{
    build_message, DeliveryError, DeliveryFuture, NotificationSink, Outbound,
};
pub use antares_store::{HistoryScope, ShortHistory, HISTORY_RETENTION, SHORT_HISTORY};
pub use contexts::public_base;
pub use federation::{
    AdmitFuture, BuiltForward, ForwardAnswer, ForwardFuture, ForwardTransport, NotifyFuture,
};
pub use ops::*;
pub use router::*;
pub use state::{AppState, HistoryRules, OnChange, Profile, RecordMode, Recording, TemporalRecord};

/// Spawn for both targets — tokio natively, the JS microtask queue on
/// wasm32 (no tokio runtime exists in a browser). Call sites are identical;
/// only the executor differs. Send is required natively (worker threads) and
/// meaningless on single-threaded wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    BACKGROUND.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let counted = Counted;
    tokio::spawn(async move {
        let _counted = counted;
        fut.await;
    });
}

/// One [`BACKGROUND`] task, uncounted when it ends however it ends: a task
/// that panicked or was cancelled left the count up and every later
/// shutdown drain waiting for its whole deadline (F-ANT2-019).
#[cfg(not(target_arch = "wasm32"))]
struct Counted;

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Counted {
    fn drop(&mut self) {
        BACKGROUND.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Work a request left behind after its response — the remote leg of a
/// distributed subscription, an initial Context Source notification, a
/// forwarded notification, a retry. Counted so a shutdown drain waits for
/// it: a rolling update that killed these tasks left the chain half-built.
static BACKGROUND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Request-born tasks still running.
pub fn background_tasks() -> usize {
    BACKGROUND.load(std::sync::atomic::Ordering::SeqCst)
}

/// A task that lives as long as the process (the matcher drain, the
/// interval tick): never counted, or a drain would wait on it forever.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn_loop<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(fut);
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn spawn_loop<F>(fut: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(fut);
}

#[cfg(target_arch = "wasm32")]
pub fn spawn<F>(fut: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(fut);
}

/// The browser build's notification channel: re-exported from the HTTP
/// binding, where it belongs — the page sink is how that binding delivers
/// when the runtime has no inbound socket.
#[cfg(target_arch = "wasm32")]
pub use antares_notifier::http::page_sink;

use antares_model::{NgsiError, TenantId, API_ROOT};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, MethodRouter};
use axum::Router;
use negotiate::{echo_tenant, respond, tenant_from, ApiError};

/// 5.5.4 Fragment applied to a stored document resource — a Subscription
/// (5.8.2), a Context Source Registration (5.9.3) or a Context Source
/// Subscription (5.11.4). Each member of the already-normalized Fragment
/// replaces the target's; a member that normalization left as JSON null is
/// the removal form and takes the member out; `id` is never touched, and the
/// write stamps `modifiedAt` (4.8). Entities do NOT go through here: 5.5.12
/// Merge Patch descends into Attribute instances and value objects, which is
/// `entities::merge_instance`.
pub(crate) fn apply_doc_fragment(
    target: &mut serde_json::Map<String, serde_json::Value>,
    fragment: &serde_json::Map<String, serde_json::Value>,
    ts: &str,
) {
    for (k, v) in fragment {
        if k == "id" {
            continue;
        }
        if v.is_null() {
            target.remove(k);
        } else {
            target.insert(k.clone(), v.clone());
        }
    }
    target.insert(
        "modifiedAt".into(),
        serde_json::Value::String(ts.to_owned()),
    );
}

pub(crate) mod surface;
pub use surface::ApiSurface;

pub use antares_ql::scope::scope_matches;

/// Installs the in-process pipeline on a state: the subscription mirror and
/// matcher (5.8.6), the interval firing, and the consumer half of a
/// distributed subscription (5.8.1.4) — a notification arriving on the
/// internal endpoint is handed to `distsub` from here, so the delivery path
/// never names it. Every root that serves requests calls this once.
pub async fn wire(state: &mut AppState) {
    notify::wire_matcher(state).await;
    install_csource_notification(state);
}

/// The 5.8.1.4 consumer half on its own, for a root that wires its matcher
/// some other way. A distributed Subscription forwards no copy until this
/// handler exists: the internal Context Source Registration Subscription
/// notifies to `urn:antares:distsub:…`, and the delivery path drops that
/// notification when nothing is installed to take it.
pub fn install_csource_notification(state: &mut AppState) {
    state.csource_notification = Some(std::sync::Arc::new(|st, tenant, own_id, reason, regs| {
        Box::pin(distsub::on_csource_notification(
            st, tenant, own_id, reason, regs,
        ))
    }));
}

/// 4.22 for the documents that carry their own `expiresAt`: Registrations,
/// Snapshots and EntityMaps, and the leased Subscriptions of WebSocket
/// connections (ADR-0026 rule 7a). The store's own sweep reaps Entities, which is
/// what 4.22 names; these three are refused by every read once their expiry
/// passes but nothing collected them, so they stayed for the life of the
/// broker. Each kind is reaped by the predicate its own reader uses, so the
/// sweep removes exactly what a read already hides — never more.
///
/// Opportunistic by construction: a driver that cannot enumerate its
/// tenants reaps nothing and the reads keep hiding what is left, and a
/// tenant whose walk fails is skipped rather than failing the tick.
///
/// The walk is the client tenants. A Snapshot's frozen copy carries its own
/// expiry on the Snapshot document, and expiring it drops the whole synthetic
/// tenant (`snap_remove`), so there is nothing left inside for a per-document
/// reaper to find.
pub async fn sweep_expired_docs(st: &AppState) -> usize {
    let Ok(tenants) = st.store.tenant_ids().await else {
        return 0;
    };
    let mut n: usize = 0;
    for name in tenants {
        let Ok(tenant) = antares_model::TenantId::new(&name) else {
            continue;
        };
        // Each sweep counts documents it deleted, far below `usize::MAX`.
        n = n
            .saturating_add(csource::sweep_expired_registrations(st, &tenant).await)
            .saturating_add(snapshots::sweep_expired_snapshots(st, &tenant).await)
            .saturating_add(entity_map::sweep_expired_maps(st, &tenant).await)
            .saturating_add(subscriptions::sweep_expired_socket_subscriptions(st, &tenant).await);
    }
    n
}

/// An `AppState` with the notification pipeline installed, for tests that
/// exercise a route which notifies. The router does not wire it: a caller
/// that only reads never pays for the mirror seed.
#[cfg(any(test, feature = "test-kit"))]
pub async fn wired_state(host_alias: &str) -> AppState {
    let mut st = AppState::new(host_alias.to_owned());
    wire(&mut st).await;
    st
}

#[cfg(test)]
mod clause_5_2_8 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_reg(entities: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t528".into()));
        let body = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:528-{}",
                          entities.to_string().len()),
            "type": "ContextSourceRegistration",
            "information": [{"entities": entities}],
            "endpoint": "http://cs.example.org:1026"
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.8-1: type is "String or String[]" — the array form must be
    /// accepted; id must be a URI; idPattern must be a valid regex.
    #[tokio::test]
    async fn entity_info_type_accepts_the_array_form() {
        assert_eq!(
            post_reg(serde_json::json!([{"type": ["Building", "Vehicle"]}])).await,
            StatusCode::CREATED,
            "String[] type is legal"
        );
        assert_eq!(
            post_reg(serde_json::json!([{"type": "Building"}])).await,
            StatusCode::CREATED
        );
        assert_eq!(
            post_reg(serde_json::json!([{"type": [] }])).await,
            StatusCode::BAD_REQUEST,
            "an empty type array names no Entity Type"
        );
        assert_eq!(
            post_reg(serde_json::json!([{"type": "Building", "id": "not a uri"}])).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_reg(serde_json::json!([{"type": "Building", "idPattern": "urn:[" }])).await,
            StatusCode::BAD_REQUEST
        );
    }
}

#[cfg(test)]
mod clause_5_2_9 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_reg(extra: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t529".into()));
        let mut doc = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:529-{}",
                          extra.to_string().len()),
            "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Building"}]}],
            "endpoint": "http://cs.example.org:1026"
        });
        for (k, v) in extra.as_object().expect("obj") {
            doc[k] = v.clone();
        }
        let body = doc.to_string();
        let req = Request::post("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.9-1 value spaces: operations limited to the 4.20 names and
    /// groups; localOnly boolean; contextSourceAlias a non-empty RFC 7230
    /// pseudonym token; refreshRate an ISO 8601 duration; datasetId URIs or
    /// @none; scope per the 4.18 grammar; geometries per 4.7; description
    /// and registrationName non-empty strings.
    #[tokio::test]
    async fn registration_member_value_spaces() {
        use serde_json::json;
        let cases: &[(serde_json::Value, StatusCode)] = &[
            (json!({"operations": ["bogusOp"]}), StatusCode::BAD_REQUEST),
            (
                json!({"operations": ["updateOps", "retrieveEntity"]}),
                StatusCode::CREATED,
            ),
            (json!({"localOnly": "yes"}), StatusCode::BAD_REQUEST),
            (json!({"localOnly": true}), StatusCode::CREATED),
            (json!({"contextSourceAlias": ""}), StatusCode::BAD_REQUEST),
            (
                json!({"contextSourceAlias": "has space"}),
                StatusCode::BAD_REQUEST,
            ),
            (json!({"contextSourceAlias": "cs1"}), StatusCode::CREATED),
            (json!({"refreshRate": "5 minutes"}), StatusCode::BAD_REQUEST),
            (json!({"refreshRate": "PT5M"}), StatusCode::CREATED),
            (
                json!({"datasetId": ["urn:ds:1", "@none"]}),
                StatusCode::CREATED,
            ),
            (json!({"datasetId": ["not a uri"]}), StatusCode::BAD_REQUEST),
            (json!({"scope": "9bad"}), StatusCode::BAD_REQUEST),
            (json!({"scope": ["/Madrid", "/A/B_2"]}), StatusCode::CREATED),
            (json!({"description": ""}), StatusCode::BAD_REQUEST),
            (json!({"registrationName": ""}), StatusCode::BAD_REQUEST),
            (json!({"location": 5}), StatusCode::BAD_REQUEST),
            (
                json!({"location": {"type": "Point", "coordinates": [8, 40]}}),
                StatusCode::CREATED,
            ),
        ];
        for (extra, want) in cases {
            let got = post_reg(extra.clone()).await;
            assert_eq!(got, *want, "extra={extra}");
        }
    }

    /// 5.2.9, after Table 5.2.9-1: the Table 5.2.9-2 members "are read-only
    /// and shall be automatically generated by NGSI-LD implementations. In
    /// the event that they are provided (in update or create operations)
    /// NGSI-LD implementations shall ignore them." A tolerant reader that
    /// stores an unknown member verbatim would let a client dictate its own
    /// forward history, and every one of these is served back.
    #[tokio::test]
    async fn table_5_2_9_2_members_are_ignored_on_create_and_update() {
        use serde_json::json;
        const READ_ONLY: [&str; 5] = [
            "timesSent",
            "timesFailed",
            "lastSuccess",
            "lastFailure",
            "status",
        ];
        let id = "urn:ngsi-ld:ContextSourceRegistration:529-readonly";
        let st = AppState::new("t5292".into());
        let claimed = json!({
            "timesSent": 9_000,
            "timesFailed": 0,
            "lastSuccess": "2035-01-01T00:00:00Z",
            "lastFailure": "2035-01-01T00:00:00Z",
            "status": "ok",
        });

        let mut doc = json!({
            "id": id,
            "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Building"}]}],
            "endpoint": "http://cs.example.org:1026",
        });
        for (k, v) in claimed.as_object().expect("obj") {
            doc[k] = v.clone();
        }
        let body = doc.to_string();
        let res = router(st.clone())
            .oneshot(
                Request::post("/ngsi-ld/v1/csourceRegistrations")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len())
                    .body(Body::from(body))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(res.status(), StatusCode::CREATED, "create");

        let served = |st: AppState| async move {
            let res = router(st)
                .oneshot(
                    Request::get(format!("/ngsi-ld/v1/csourceRegistrations/{id}"))
                        .body(Body::empty())
                        .expect("req"),
                )
                .await
                .expect("resp");
            assert_eq!(res.status(), StatusCode::OK, "retrieve");
            let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .expect("body");
            serde_json::from_slice::<serde_json::Value>(&bytes).expect("json")
        };

        let got = served(st.clone()).await;
        for m in READ_ONLY {
            assert!(
                got.get(m).is_none(),
                "create must ignore the read-only member {m}: {got}"
            );
        }

        // 5.9.3 update: the same members, through the merge patch.
        let patch = claimed.to_string();
        let res = router(st.clone())
            .oneshot(
                Request::patch(format!("/ngsi-ld/v1/csourceRegistrations/{id}"))
                    .header("Content-Type", "application/json")
                    .header("Content-Length", patch.len())
                    .body(Body::from(patch))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(res.status(), StatusCode::NO_CONTENT, "update");

        let got = served(st).await;
        for m in READ_ONLY {
            assert!(
                got.get(m).is_none(),
                "update must ignore the read-only member {m}: {got}"
            );
        }
    }
}

#[cfg(test)]
mod clause_5_2_10 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_info(info: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t5210".into()));
        let body = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:5210-{}",
                          info.to_string().len()),
            "type": "ContextSourceRegistration",
            "information": [info],
            "endpoint": "http://cs.example.org:1026"
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.10-1: empty arrays are not allowed for entities,
    /// propertyNames or relationshipNames; non-empty name lists are fine.
    #[tokio::test]
    async fn registration_info_empty_arrays_are_rejected() {
        use serde_json::json;
        assert_eq!(
            post_info(json!({"entities": []})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_info(json!({"propertyNames": []})).await,
            StatusCode::BAD_REQUEST,
            "empty propertyNames"
        );
        assert_eq!(
            post_info(json!({"relationshipNames": []})).await,
            StatusCode::BAD_REQUEST,
            "empty relationshipNames"
        );
        assert_eq!(
            post_info(json!({"entities": [{"type": "Building"}],
                "propertyNames": ["speed"],
                "relationshipNames": ["isParked"]}))
            .await,
            StatusCode::CREATED
        );
    }
}

#[cfg(test)]
mod clause_5_2_11 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_interval(iv: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t5211".into()));
        let body = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:5211-{}",
                          iv.to_string().len()),
            "type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Building"}]}],
            "endpoint": "http://cs.example.org:1026",
            "observationInterval": iv
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.11-1: startAt is a mandatory DateTime; endAt optional but a
    /// DateTime when present (absent = open interval).
    #[tokio::test]
    async fn time_interval_member_rules() {
        use serde_json::json;
        assert_eq!(
            post_interval(json!({"endAt": "2030-01-01T00:00:00Z"})).await,
            StatusCode::BAD_REQUEST,
            "startAt mandatory"
        );
        assert_eq!(
            post_interval(json!({"startAt": "2020-01-01"})).await,
            StatusCode::BAD_REQUEST,
            "a Date is not a DateTime"
        );
        assert_eq!(
            post_interval(json!({"startAt": "2020-01-01T00:00:00Z",
                "endAt": "not a date"}))
            .await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_interval(json!({"startAt": "2020-01-01T00:00:00Z"})).await,
            StatusCode::CREATED,
            "open interval"
        );
        assert_eq!(
            post_interval(json!({"startAt": "2020-01-01T00:00:00Z",
                "endAt": "2030-01-01T00:00:00Z"}))
            .await,
            StatusCode::CREATED
        );
    }
}

#[cfg(test)]
mod clause_5_2_12 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_sub(doc: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t5212".into()));
        let body = doc.to_string();
        let req = Request::post("/ngsi-ld/v1/subscriptions")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// 5.2.12: "At least one of (a) entities or (b) watchedAttributes shall
    /// be present, unless the member localOnly is set to true".
    #[tokio::test]
    async fn local_only_waives_the_selector_requirement() {
        use serde_json::json;
        let base = |extra: serde_json::Value| {
            let mut d = json!({
                "id": format!("urn:ngsi-ld:Subscription:5212-{}", extra.to_string().len()),
                "type": "Subscription",
                "notification": {"endpoint": {"uri": "http://client.example.org/cb"}}
            });
            for (k, v) in extra.as_object().expect("obj") {
                d[k] = v.clone();
            }
            d
        };
        assert_eq!(
            post_sub(base(json!({}))).await,
            StatusCode::BAD_REQUEST,
            "no selector and no localOnly"
        );
        assert_eq!(
            post_sub(base(json!({"localOnly": true}))).await,
            StatusCode::CREATED,
            "localOnly=true waives entities/watchedAttributes"
        );
        assert_eq!(
            post_sub(base(json!({"localOnly": false}))).await,
            StatusCode::BAD_REQUEST
        );
        // the exclusions stay intact
        assert_eq!(
            post_sub(base(json!({"watchedAttributes": ["speed"],
                "timeInterval": 5})))
            .await,
            StatusCode::BAD_REQUEST
        );
    }
}

#[cfg(test)]
mod clause_5_2_13 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_geoq(geoq: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t5213".into()));
        let body = serde_json::json!({
            "id": format!("urn:ngsi-ld:Subscription:5213-{}", geoq.to_string().len()),
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "geoQ": geoq,
            "notification": {"endpoint": {"uri": "http://client.example.org/cb"}}
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/subscriptions")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.13-1: coordinates as JSON Array OR string form, geometry
    /// from the legal set (no GeometryCollection), georel per 4.10,
    /// geoproperty optional.
    #[tokio::test]
    async fn subscription_geoquery_member_rules() {
        use serde_json::json;
        assert_eq!(
            post_geoq(json!({"georel": "near;maxDistance==2000",
                "geometry": "Point", "coordinates": [8, 40]}))
            .await,
            StatusCode::CREATED
        );
        assert_eq!(
            post_geoq(json!({"georel": "within", "geometry": "Polygon",
                "coordinates": "[[[0,0],[4,0],[4,4],[0,4],[0,0]]]",
                "geoproperty": "observationSpace"}))
            .await,
            StatusCode::CREATED,
            "string-encoded coordinates (4.7.1) are legal"
        );
        assert_eq!(
            post_geoq(json!({"georel": "within",
                "geometry": "GeometryCollection", "coordinates": []}))
            .await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_geoq(json!({"georel": "touches", "geometry": "Point",
                "coordinates": [8, 40]}))
            .await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_geoq(json!({"geometry": "Point", "coordinates": [8, 40]})).await,
            StatusCode::BAD_REQUEST,
            "georel is mandatory"
        );
    }
}

#[cfg(test)]
mod clause_5_2_14 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_notif(n: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t5214".into()));
        let mut notif = serde_json::json!({"endpoint": {"uri": "http://client.example.org/cb"}});
        for (k, v) in n.as_object().expect("obj") {
            notif[k] = v.clone();
        }
        let body = serde_json::json!({
            "id": format!("urn:ngsi-ld:Subscription:5214-{}", n.to_string().len()),
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "notification": notif
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/subscriptions")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.14.1-1: join limited to flat/inline/@none, joinLevel a
    /// positive integer, sysAttrs/showChanges booleans, attributes may not
    /// name id/type/scope (it is "a synonym for pick, except that id, type,
    /// scope are not allowed").
    #[tokio::test]
    async fn notification_params_value_spaces() {
        use serde_json::json;
        assert_eq!(
            post_notif(json!({"join": "sideways"})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_notif(json!({"join": "inline"})).await,
            StatusCode::CREATED
        );
        assert_eq!(
            post_notif(json!({"joinLevel": 0})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_notif(json!({"joinLevel": 1.5})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_notif(json!({"join": "flat", "joinLevel": 2})).await,
            StatusCode::CREATED
        );
        assert_eq!(
            post_notif(json!({"sysAttrs": "yes"})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_notif(json!({"showChanges": "yes"})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_notif(json!({"attributes": ["id"]})).await,
            StatusCode::BAD_REQUEST,
            "attributes may not name id/type/scope"
        );
        assert_eq!(
            post_notif(json!({"attributes": ["speed"]})).await,
            StatusCode::CREATED
        );
    }
}

#[cfg(test)]
mod clause_5_2_14_2 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Table 5.2.14.2-1: lastFailure/lastNotification/lastSuccess/timesSent
    /// are output-only — provided ones "shall ignore them" on create, and
    /// retrieval never echoes fabricated values.
    #[tokio::test]
    async fn output_only_members_are_ignored_on_input() {
        let st = AppState::new("t52142".into());
        let app = router(st);
        let body = serde_json::json!({
            "id": "urn:ngsi-ld:Subscription:52142",
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "notification": {
                "endpoint": {"uri": "http://client.example.org/cb"},
                "timesSent": 999,
                "timesFailed": 999,
                "lastNotification": "1999-01-01T00:00:00Z",
                "lastSuccess": "1999-01-01T00:00:00Z",
                "lastFailure": "1999-01-01T00:00:00Z"
            }
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/subscriptions")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        let resp = app.clone().oneshot(req).await.expect("resp");
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "providing them is not an error"
        );
        let resp = app
            .oneshot(
                Request::get("/ngsi-ld/v1/subscriptions/urn:ngsi-ld:Subscription:52142")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let n = &doc["notification"];
        for k in [
            "timesSent",
            "timesFailed",
            "lastNotification",
            "lastSuccess",
            "lastFailure",
        ] {
            assert!(
                n.get(k).is_none(),
                "client-fabricated {k} must be ignored, got {}",
                n[k]
            );
        }
    }
}

#[cfg(test)]
mod clause_5_2_15 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn post_ep(ep: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t5215".into()));
        let body = serde_json::json!({
            "id": format!("urn:ngsi-ld:Subscription:5215-{}", ep.to_string().len()),
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "notification": {"endpoint": ep}
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/subscriptions")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.15-1: uri mandatory, accept value space, cooldown/timeout
    /// > 0, receiverInfo/notifierInfo as KeyValuePair[] (5.2.22).
    #[tokio::test]
    async fn endpoint_member_rules() {
        use serde_json::json;
        let uri = "http://client.example.org/cb";
        assert_eq!(
            post_ep(json!({"accept": "application/json"})).await,
            StatusCode::BAD_REQUEST,
            "uri mandatory"
        );
        assert_eq!(
            post_ep(json!({"uri": uri, "accept": "text/plain"})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_ep(json!({"uri": uri, "cooldown": 0})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_ep(json!({"uri": uri, "timeout": -1})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_ep(json!({"uri": uri,
                "receiverInfo": [{"key": "Authorization", "value": "Bearer x"}],
                "cooldown": 500, "timeout": 3000}))
            .await,
            StatusCode::CREATED
        );
        assert_eq!(
            post_ep(json!({"uri": uri, "receiverInfo": ["junk"]})).await,
            StatusCode::BAD_REQUEST,
            "receiverInfo entries must be {{key, value}} pairs"
        );
        assert_eq!(
            post_ep(json!({"uri": uri, "notifierInfo": [{"novalue": true}]})).await,
            StatusCode::BAD_REQUEST,
            "notifierInfo entries must be {{key, value}} pairs"
        );
        assert_eq!(
            post_ep(json!({"uri": uri,
                "receiverInfo": [{"key": "Authorization", "value": "enc:v1:x"}]}))
            .await,
            StatusCode::BAD_REQUEST,
            "a value carrying the seal marker is refused (F-R2ANT4-002)"
        );
    }
}

#[cfg(test)]
mod clause_5_2_16 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Tables 5.2.16-1/5.2.17-1: a partial batch failure answers 207 with
    /// success = URI array and errors = BatchEntityError[] (entityId +
    /// RFC 7807 ProblemDetails).
    #[tokio::test]
    async fn batch_result_and_entity_error_shapes() {
        let app = router(AppState::new("t5216".into()));
        let body = serde_json::json!([
            {"id": "urn:ngsi-ld:V:ok", "type": "Vehicle"},
            {"id": "not a uri", "type": "Vehicle"}
        ])
        .to_string();
        let req = Request::post("/ngsi-ld/v1/entityOperations/create")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        let resp = app.oneshot(req).await.expect("resp");
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(doc["success"], serde_json::json!(["urn:ngsi-ld:V:ok"]));
        let errs = doc["errors"].as_array().expect("errors array");
        assert_eq!(errs.len(), 1);
        let e = &errs[0];
        assert!(e.get("entityId").is_some());
        let pd = &e["error"];
        for k in ["type", "title", "status"] {
            assert!(
                pd.get(k).is_some(),
                "ProblemDetails member {k} missing: {pd}"
            );
        }
        assert!(
            pd["type"].as_str().unwrap_or("").contains("errors/"),
            "error.type is the NGSI-LD error URI"
        );
    }
}

#[cfg(test)]
mod clause_5_2_18 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Tables 5.2.18-1/5.2.19-1: partial attribute update answers 207 with
    /// updated = names and notUpdated = {attributeName, reason}[].
    #[tokio::test]
    async fn update_result_and_not_updated_details_shape() {
        let app = router(AppState::new("t5218".into()));
        let body = serde_json::json!({"id": "urn:ngsi-ld:V:5218", "type": "Vehicle",
            "speed": {"type": "Property", "value": 1}})
        .to_string();
        let req = Request::post("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        assert_eq!(
            app.clone().oneshot(req).await.expect("r").status(),
            StatusCode::CREATED
        );
        // noOverwrite append: speed exists (skipped), brand is new (applied)
        let frag = serde_json::json!({
            "speed": {"type": "Property", "value": 2},
            "brand": {"type": "Property", "value": "x"}})
        .to_string();
        let req =
            Request::post("/ngsi-ld/v1/entities/urn:ngsi-ld:V:5218/attrs?options=noOverwrite")
                .header("Content-Type", "application/json")
                .header("Content-Length", frag.len())
                .body(Body::from(frag))
                .expect("req");
        let resp = app.oneshot(req).await.expect("resp");
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let updated = doc["updated"].as_array().expect("updated");
        assert!(
            updated
                .iter()
                .any(|u| u.as_str().unwrap_or("").contains("brand")),
            "brand applied: {doc}"
        );
        let nu = doc["notUpdated"].as_array().expect("notUpdated");
        assert_eq!(nu.len(), 1, "{doc}");
        assert!(nu[0]["attributeName"]
            .as_str()
            .unwrap_or("")
            .contains("speed"));
        assert!(!nu[0]["reason"].as_str().unwrap_or("").is_empty());
        assert!(
            nu[0].get("registrationId").is_none(),
            "local failure carries no registrationId"
        );
    }
}

#[cfg(test)]
mod clause_5_2_21 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    async fn temporal_query(qs: &str) -> StatusCode {
        let app = router(AppState::new("t5221".into()));
        let req = Request::get(format!(
            "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=2020-01-01T00:00:00Z{qs}"
        ))
        .body(Body::empty())
        .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.21-1: lastN is a POSITIVE integer; aggrMethods entries are
    /// limited to the 4.5.19 methods; endTimeAt is mandatory for between.
    #[tokio::test]
    async fn temporal_query_member_value_spaces() {
        assert_eq!(
            temporal_query("&lastN=0").await,
            StatusCode::BAD_REQUEST,
            "lastN=0"
        );
        assert_eq!(temporal_query("&lastN=-3").await, StatusCode::BAD_REQUEST);
        assert_eq!(temporal_query("&lastN=5").await, StatusCode::OK);
        assert_eq!(
            temporal_query("&aggrMethods=bogus").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            temporal_query("&options=aggregatedValues").await,
            StatusCode::BAD_REQUEST,
            "aggregatedValues without aggrMethods"
        );
        assert_eq!(
            temporal_query("&options=aggregatedValues&aggrMethods=avg,max").await,
            StatusCode::OK
        );
        let app = router(AppState::new("t5221b".into()));
        let req = Request::get(
            "/ngsi-ld/v1/temporal/entities?type=V&timerel=between&timeAt=2020-01-01T00:00:00Z",
        )
        .body(Body::empty())
        .expect("req");
        assert_eq!(
            app.oneshot(req).await.expect("resp").status(),
            StatusCode::BAD_REQUEST,
            "between without endTimeAt"
        );
    }

    /// POST Query (5.2.23) carrying a temporalQ object (5.2.21 JSON form).
    async fn post_tq(tq: serde_json::Value, qs: &str) -> (StatusCode, String) {
        let app = router(AppState::new("t5221j".into()));
        let body = json!({
            "type": "Query",
            "entities": [{"type": "Vehicle"}],
            "temporalQ": tq
        })
        .to_string();
        let req = Request::post(format!("/ngsi-ld/v1/temporal/entityOperations/query{qs}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        let resp = app.oneshot(req).await.expect("resp");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Table 5.2.21-1 (JSON form): lastN is a positive INTEGER — zero,
    /// negative, fractional and string values are all outside the value
    /// space; timerel is limited to before/after/between; endTimeAt is
    /// mandatory for between; timeproperty is limited to the 4.8 names
    /// (expiresAt among them, 5.2.4 NOTE 2).
    #[tokio::test]
    async fn temporal_query_json_member_value_spaces() {
        let ok = json!({"timerel": "after", "timeAt": "2020-01-01T00:00:00Z"});
        let with = |k: &str, v: serde_json::Value| {
            let mut t = ok.clone();
            t[k] = v;
            t
        };
        let (st, body) = post_tq(with("lastN", json!(5)), "").await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(!body.contains("BadRequestData"), "{body}");
        for bad in [json!(0), json!(-3), json!(2.5), json!("5")] {
            let (st, body) = post_tq(with("lastN", bad.clone()), "").await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "lastN={bad}");
            assert!(body.contains("lastN"), "{body}");
        }
        let (st, _) = post_tq(with("timerel", json!("bogus")), "").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = post_tq(with("timerel", json!("between")), "").await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "between without endTimeAt");
        let (st, body) = post_tq(with("timeproperty", json!("expiresAt")), "").await;
        assert_eq!(
            st,
            StatusCode::OK,
            "expiresAt is a 4.8 timeproperty: {body}"
        );
        let (st, _) = post_tq(with("timeproperty", json!("lastUsedAt")), "").await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "timeproperty outside 4.8 set");
        let (st, _) = post_tq(with("timeAt", json!(20200101)), "").await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "timeAt must be a string");
    }

    /// Table 5.2.21-1: aggrMethods (comma separated list of string — both
    /// the string and string-array spellings) and aggrPeriodDuration are
    /// carried by the JSON TemporalQuery and honoured when
    /// aggregatedValues is requested via format/options.
    #[tokio::test]
    async fn temporal_query_json_aggregation_members() {
        let base = json!({"timerel": "after", "timeAt": "2020-01-01T00:00:00Z"});
        let with = |k: &str, v: serde_json::Value| {
            let mut t = base.clone();
            t[k] = v;
            t
        };
        let (st, body) = post_tq(
            with("aggrMethods", json!(["avg", "max"])),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "array aggrMethods honoured: {body}");
        let (st, body) = post_tq(
            with("aggrMethods", json!("avg,max")),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "string aggrMethods honoured: {body}");
        let (st, body) = post_tq(
            with("aggrMethods", json!(["bogus"])),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(body.contains("aggrMethods"), "{body}");
        let (st, _) = post_tq(with("aggrMethods", json!(42)), "?format=aggregatedValues").await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "aggrMethods wrong JSON type");
        let mut t = base.clone();
        t["aggrMethods"] = json!(["avg"]);
        t["aggrPeriodDuration"] = json!("bogus");
        let (st, body) = post_tq(t, "?format=aggregatedValues").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(body.contains("aggrPeriodDuration"), "{body}");
    }

    /// 5.2.21 used from Subscription.temporalQ (5.2.12, CSR subscriptions):
    /// timerel and timeAt are cardinality 1 — a temporalQ violating the
    /// data type is rejected at subscription creation.
    #[tokio::test]
    async fn subscription_temporal_q_validated() {
        async fn post_sub(tq: serde_json::Value) -> StatusCode {
            let app = router(AppState::new("t5221s".into()));
            let body = json!({
                "id": format!("urn:ngsi-ld:Subscription:5221-{}", tq.to_string().len()),
                "type": "Subscription",
                "entities": [{"type": "Building"}],
                "notification": {"endpoint": {"uri": "http://client.example.org/cb"}},
                "temporalQ": tq
            })
            .to_string();
            let req = Request::post("/ngsi-ld/v1/subscriptions")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req");
            app.oneshot(req).await.expect("resp").status()
        }
        assert_eq!(
            post_sub(json!({
                "timerel": "after",
                "timeAt": "2020-06-01T22:07:00Z",
                "timeproperty": "createdAt"
            }))
            .await,
            StatusCode::CREATED,
            "official fixture shape stays creatable"
        );
        assert_eq!(
            post_sub(json!({"timerel": "after"})).await,
            StatusCode::BAD_REQUEST,
            "timeAt is cardinality 1"
        );
        assert_eq!(
            post_sub(json!({"timeAt": "2020-06-01T22:07:00Z"})).await,
            StatusCode::BAD_REQUEST,
            "timerel is cardinality 1"
        );
        assert_eq!(
            post_sub(json!({"timerel": "bogus", "timeAt": "2020-06-01T22:07:00Z"})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_sub(json!("after")).await,
            StatusCode::BAD_REQUEST,
            "temporalQ must be an object"
        );
    }
}

#[cfg(test)]
mod clause_5_2_22 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    async fn post(path: &str, doc: serde_json::Value, tenant: &str) -> StatusCode {
        let app = router(AppState::new(tenant.into()));
        let body = doc.to_string();
        let req = Request::post(format!("/ngsi-ld/v1/{path}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.22-1: key AND value are Strings, both cardinality 1 —
    /// enforced on the notification endpoint's receiverInfo/notifierInfo.
    #[tokio::test]
    async fn endpoint_info_values_must_be_strings() {
        let sub = |info_key: &str, entries: serde_json::Value| {
            let mut d = json!({
                "type": "Subscription",
                "entities": [{"type": "Building"}],
                "notification": {"endpoint": {"uri": "http://client.example.org/cb"}}
            });
            d["notification"]["endpoint"][info_key] = entries;
            d
        };
        assert_eq!(
            post(
                "subscriptions",
                sub(
                    "receiverInfo",
                    json!([{"key": "Authorization", "value": "Bearer x"}])
                ),
                "t5222a"
            )
            .await,
            StatusCode::CREATED,
            "string values stay creatable"
        );
        for bad in [
            json!(42),
            json!({"a": 1}),
            json!(["x"]),
            json!(null),
            json!(true),
        ] {
            assert_eq!(
                post(
                    "subscriptions",
                    sub("receiverInfo", json!([{"key": "K", "value": bad}])),
                    "t5222a"
                )
                .await,
                StatusCode::BAD_REQUEST,
                "receiverInfo value {bad} is not a String"
            );
        }
        assert_eq!(
            post(
                "subscriptions",
                sub("notifierInfo", json!([{"key": "K", "value": 7}])),
                "t5222a"
            )
            .await,
            StatusCode::BAD_REQUEST,
            "notifierInfo value must be a String"
        );
    }

    /// Table 5.2.22-1 via 5.2.9 contextSourceInfo: every pair's value is a
    /// String — a non-string value on a custom key is rejected at
    /// registration, not at first forward.
    #[tokio::test]
    async fn context_source_info_values_must_be_strings() {
        let csr = |info: serde_json::Value| {
            json!({
                "type": "ContextSourceRegistration",
                "endpoint": "http://peer.example/ngsi-ld/v1",
                "information": [{"entities": [{"type": "Building"}]}],
                "contextSourceInfo": info
            })
        };
        assert_eq!(
            post(
                "csourceRegistrations",
                csr(json!([{"key": "X-Auth-Token", "value": "abc"}])),
                "t5222b"
            )
            .await,
            StatusCode::CREATED,
            "string values stay registrable"
        );
        for bad in [json!(123), json!(["a"]), json!({"v": 1}), json!(null)] {
            assert_eq!(
                post(
                    "csourceRegistrations",
                    csr(json!([{"key": "X-Custom", "value": bad}])),
                    "t5222b"
                )
                .await,
                StatusCode::BAD_REQUEST,
                "contextSourceInfo value {bad} is not a String"
            );
        }
        assert_eq!(
            post(
                "csourceRegistrations",
                csr(json!([{"key": 5, "value": "v"}])),
                "t5222b"
            )
            .await,
            StatusCode::BAD_REQUEST,
            "key must be a String"
        );
    }
}

#[cfg(test)]
mod clause_5_2_44 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    /// Table 5.2.44-1: aggrParams carries aggrMethods (comma separated list
    /// of strings — string and string-array spellings) + aggrPeriodDuration,
    /// honoured on the temporal operation when aggregatedValues is requested.
    #[tokio::test]
    async fn aggregation_params_members() {
        let app = router(AppState::new("t5244".into()));
        let send = |doc: serde_json::Value, qs: &'static str| {
            let app = app.clone();
            async move {
                let body = doc.to_string();
                let req = Request::post(format!("/ngsi-ld/v1/temporal/entityOperations/query{qs}"))
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len())
                    .body(Body::from(body))
                    .expect("req");
                let resp = app.oneshot(req).await.expect("resp");
                let status = resp.status();
                let bytes = resp.into_body().collect().await.expect("body").to_bytes();
                (status, String::from_utf8_lossy(&bytes).into_owned())
            }
        };
        let with_ap = |ap: serde_json::Value| {
            json!({"type": "Query", "entities": [{"type": "Vehicle"}],
                "temporalQ": {"timerel": "after", "timeAt": "2020-01-01T00:00:00Z"},
                "aggrParams": ap})
        };
        let (st, body) = send(
            with_ap(json!({"aggrMethods": ["avg"], "aggrPeriodDuration": "PT1H"})),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "array spelling honoured: {body}");
        assert!(!body.contains("BadRequestData"), "{body}");
        let (st, _) = send(
            with_ap(json!({"aggrMethods": "avg,max"})),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "string spelling honoured");
        let (st, body) = send(
            with_ap(json!({"aggrMethods": ["bogus"]})),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(body.contains("aggrMethods"), "{body}");
        let (st, body) = send(
            with_ap(json!({"aggrMethods": ["avg"], "aggrPeriodDuration": "bogus"})),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(body.contains("aggrPeriodDuration"), "{body}");
        let (st, _) = send(
            with_ap(json!({"aggrMethods": 42})),
            "?format=aggregatedValues",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "aggrMethods wrong JSON type");
        let (st, _) = send(with_ap(json!("avg")), "?format=aggregatedValues").await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "aggrParams must be an object");
    }
}

#[cfg(test)]
mod clause_5_2_43 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    async fn send(
        app: &axum::Router,
        method: &str,
        path: &str,
        doc: Option<serde_json::Value>,
    ) -> (StatusCode, String) {
        let mut b = Request::builder()
            .method(method)
            .uri(format!("/ngsi-ld/v1/{path}"));
        let body = match doc {
            Some(d) => {
                let s = d.to_string();
                b = b
                    .header("Content-Type", "application/json")
                    .header("Content-Length", s.len());
                Body::from(s)
            }
            None => Body::empty(),
        };
        let resp = app
            .clone()
            .oneshot(b.body(body).expect("req"))
            .await
            .expect("resp");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Table 5.2.43-1: orderBy maps to the 4.23 keys, coordinates (JSON
    /// array, mandatory for dist ordering) + geometry (default Point) form
    /// the reference geometry; collation is rejected loudly while only
    /// codepoint order is offered (4.23.1 named gap).
    #[tokio::test]
    async fn ordering_params_members() {
        let app = router(AppState::new("t5243".into()));
        for (id, lon, lat) in [
            ("urn:ngsi-ld:Vehicle:near", 8.01, 40.01),
            ("urn:ngsi-ld:Vehicle:far", 10.0, 45.0),
        ] {
            let (st, body) = send(
                &app,
                "POST",
                "entities",
                Some(json!({"id": id, "type": "Vehicle",
                    "location": {"type": "GeoProperty",
                        "value": {"type": "Point", "coordinates": [lon, lat]}}})),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{body}");
        }
        let q = "entityOperations/query";
        let with_ordering = |o: serde_json::Value| json!({"type": "Query", "entities": [{"type": "Vehicle"}], "ordering": o});
        let (st, body) = send(
            &app,
            "POST",
            q,
            Some(with_ordering(json!({"orderBy": ["location;dist-asc"],
                "coordinates": [8, 40], "geometry": "Point"}))),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let near = body.find("urn:ngsi-ld:Vehicle:near").expect("near in body");
        let far = body.find("urn:ngsi-ld:Vehicle:far").expect("far in body");
        assert!(near < far, "dist-asc must order near before far: {body}");
        let (st, _) = send(
            &app,
            "POST",
            q,
            Some(with_ordering(json!({"orderBy": ["location;dist-asc"]}))),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "dist ordering without coordinates"
        );
        let (st, _) = send(
            &app,
            "POST",
            q,
            Some(with_ordering(json!({"orderBy": ["location;dist-asc"],
                "coordinates": "8,40"}))),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "coordinates must be a JSON array"
        );
        let (st, _) = send(
            &app,
            "POST",
            q,
            Some(with_ordering(json!({"orderBy": ["id;asc"], "geometry": 5}))),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "geometry must be a string");
        // 4.23.3 EXAMPLE 7: a named ICU collation is honoured (200), and a
        // non-string member stays a loud 400
        let (st, body) = send(
            &app,
            "POST",
            q,
            Some(with_ordering(json!({"orderBy": ["id;asc"],
                "collation": "de-u-co-phonebk"}))),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "named collation is honoured: {body}");
        let (st, body) = send(
            &app,
            "POST",
            q,
            Some(with_ordering(
                json!({"orderBy": ["id;asc"], "collation": 5}),
            )),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("collation"), "{body}");
        // GET twin: orderGeometry is a legal query parameter
        let (st, body) = send(
            &app,
            "GET",
            "entities?type=Vehicle&orderBy=location;dist-asc&orderFrom=[8,40]&orderGeometry=Point",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }
}

#[cfg(test)]
mod clause_5_2_34 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    async fn post_csr(management: serde_json::Value) -> StatusCode {
        let app = router(AppState::new("t5234".into()));
        let body = json!({
            "type": "ContextSourceRegistration",
            "endpoint": "http://peer.example/ngsi-ld/v1",
            "information": [{"entities": [{"type": "Building"}]}],
            "management": management
        })
        .to_string();
        let req = Request::post("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        app.oneshot(req).await.expect("resp").status()
    }

    /// Table 5.2.34-1: cacheDuration is an ISO 8601 duration, cooldown and
    /// timeout are numbers greater than 0, localOnly is a boolean, and the
    /// member itself is an object.
    #[tokio::test]
    async fn registration_management_info_value_spaces() {
        assert_eq!(
            post_csr(json!({"cacheDuration": "PT5M", "cooldown": 500,
                "timeout": 3000, "localOnly": true}))
            .await,
            StatusCode::CREATED,
            "conformant management info stays registrable"
        );
        for (label, m) in [
            ("non-object", json!("yes")),
            ("bad cacheDuration", json!({"cacheDuration": "bogus"})),
            ("cacheDuration wrong type", json!({"cacheDuration": 300})),
            ("cooldown zero", json!({"cooldown": 0})),
            ("timeout negative", json!({"timeout": -5})),
            ("timeout wrong type", json!({"timeout": "3000"})),
            ("localOnly wrong type", json!({"localOnly": "yes"})),
        ] {
            assert_eq!(
                post_csr(m).await,
                StatusCode::BAD_REQUEST,
                "management {label}"
            );
        }
    }
}

#[cfg(test)]
mod clause_5_2_33 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    async fn send(app: &axum::Router, path: &str, doc: &serde_json::Value) -> (StatusCode, String) {
        let body = doc.to_string();
        let req = Request::post(format!("/ngsi-ld/v1/{path}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        let resp = app.clone().oneshot(req).await.expect("resp");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Table 5.2.33-1: id is "String or String[]" of valid URIs, type is
    /// mandatory, and "id takes precedence over idPattern".
    #[tokio::test]
    async fn entity_selector_id_forms_and_precedence() {
        let app = router(AppState::new("t5233a".into()));
        for id in [
            "urn:ngsi-ld:Vehicle:A1",
            "urn:ngsi-ld:Vehicle:A2",
            "urn:ngsi-ld:Vehicle:B1",
        ] {
            let (st, body) = send(
                &app,
                "entities",
                &json!({"id": id, "type": "Vehicle",
                        "speed": {"type": "Property", "value": 1}}),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{body}");
        }
        let q = "entityOperations/query";
        let sel = |e: serde_json::Value| json!({"type": "Query", "entities": [e]});
        let (st, body) = send(
            &app,
            q,
            &sel(json!({"type": "Vehicle",
                "id": ["urn:ngsi-ld:Vehicle:A1", "urn:ngsi-ld:Vehicle:A2"]})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "id array form: {body}");
        assert!(body.contains("urn:ngsi-ld:Vehicle:A1"), "{body}");
        assert!(body.contains("urn:ngsi-ld:Vehicle:A2"), "{body}");
        assert!(!body.contains("urn:ngsi-ld:Vehicle:B1"), "{body}");
        let (st, body) = send(
            &app,
            q,
            &sel(json!({"type": "Vehicle", "id": "urn:ngsi-ld:Vehicle:A1",
                "idPattern": "^urn:ngsi-ld:Vehicle:B.*$"})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(
            body.contains("urn:ngsi-ld:Vehicle:A1"),
            "id takes precedence over idPattern: {body}"
        );
        let (st, _) = send(&app, q, &sel(json!({"id": "urn:ngsi-ld:Vehicle:A1"}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "type is mandatory (5.2.33)");
        let (st, _) = send(
            &app,
            q,
            &sel(json!({"type": "Vehicle", "id": ["urn:ngsi-ld:Vehicle:A1", 5]})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "id entries must be strings");
        let (st, _) = send(&app, q, &sel(json!({"type": "Vehicle", "id": "not a uri"}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "id must be a valid URI");
    }

    /// 5.2.33 in Subscription.entities: the String[] id form is accepted at
    /// creation.
    #[tokio::test]
    async fn subscription_selector_id_array() {
        let app = router(AppState::new("t5233b".into()));
        let sub = |e: serde_json::Value| {
            json!({"type": "Subscription", "entities": [e],
                "notification": {"endpoint": {"uri": "http://client.example.org/cb"}}})
        };
        let (st, body) = send(
            &app,
            "subscriptions",
            &sub(json!({"type": "Building",
                "id": ["urn:ngsi-ld:Building:a", "urn:ngsi-ld:Building:b"]})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        let (st, _) = send(
            &app,
            "subscriptions",
            &sub(json!({"type": "Building", "id": ["urn:ngsi-ld:Building:a", 5]})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}

#[cfg(test)]
mod clause_5_2_23 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    async fn send(
        app: &axum::Router,
        method: &str,
        path: &str,
        doc: &serde_json::Value,
    ) -> (StatusCode, String) {
        let body = doc.to_string();
        let req = Request::builder()
            .method(method)
            .uri(format!("/ngsi-ld/v1/{path}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req");
        let resp = app.clone().oneshot(req).await.expect("resp");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn query(extra: serde_json::Value) -> serde_json::Value {
        let mut d = json!({"type": "Query", "entities": [{"type": "Vehicle"}]});
        for (k, v) in extra.as_object().expect("obj") {
            d[k] = v.clone();
        }
        d
    }

    /// Table 5.2.23-1: member value spaces — empty arrays are not allowed
    /// (entities/attrs/pick/omit), string members must be strings, joinLevel
    /// is a positive integer, geoQ/ordering are objects, and
    /// temporalQ/aggrParams are only allowed on the temporal operation.
    #[tokio::test]
    async fn query_body_member_value_spaces() {
        let app = router(AppState::new("t5223a".into()));
        let q = "entityOperations/query";
        let (st, body) = send(&app, "POST", q, &query(json!({}))).await;
        assert_eq!(st, StatusCode::OK, "control: {body}");
        assert!(!body.contains("BadRequestData"), "{body}");
        for (label, doc) in [
            ("type not Query", json!({"type": "NotQuery"})),
            ("entities empty", json!({"type": "Query", "entities": []})),
            (
                "entities not array",
                query(json!({"entities": "Vehicle"})).clone(),
            ),
            ("attrs empty", query(json!({"attrs": []}))),
            ("attrs non-string entry", query(json!({"attrs": [7]}))),
            ("pick empty", query(json!({"pick": []}))),
            ("omit empty", query(json!({"omit": []}))),
            ("q not a string", query(json!({"q": 42}))),
            ("csf not a string", query(json!({"csf": 42}))),
            (
                "datasetId not an array",
                query(json!({"datasetId": "urn:x"})),
            ),
            (
                "joinLevel zero",
                query(json!({"join": "inline", "joinLevel": 0})),
            ),
            (
                "joinLevel fractional",
                query(json!({"join": "inline", "joinLevel": 2.5})),
            ),
            ("geoQ not an object", query(json!({"geoQ": "near"}))),
            ("ordering not an object", query(json!({"ordering": "asc"}))),
            (
                "splitEntities not a boolean",
                query(json!({"splitEntities": "yes"})),
            ),
            (
                "entityMap not a boolean",
                query(json!({"entityMap": "yes"})),
            ),
            (
                "temporalQ only for the temporal operation",
                query(json!({"temporalQ": {"timerel": "after", "timeAt": "2020-01-01T00:00:00Z"}})),
            ),
            (
                "aggrParams only for the temporal operation",
                query(json!({"aggrParams": {"aggrMethods": "avg"}})),
            ),
        ] {
            let (st, body) = send(&app, "POST", q, &doc).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{label}: {body}");
        }
        // entities selector: non-object entries violate 5.2.33 EntitySelector[]
        let (st, _) = send(
            &app,
            "POST",
            q,
            &json!({"type": "Query", "entities": ["Vehicle"]}),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "entities entries must be objects"
        );
    }

    /// Table 5.2.23-1: q/pick/omit conveyed in the body are honoured exactly
    /// like their 6.3.7 query-parameter twins.
    #[tokio::test]
    async fn query_body_members_are_honoured() {
        let app = router(AppState::new("t5223b".into()));
        for (id, speed) in [
            ("urn:ngsi-ld:Vehicle:A1", 80),
            ("urn:ngsi-ld:Vehicle:A2", 120),
        ] {
            let (st, body) = send(
                &app,
                "POST",
                "entities",
                &json!({"id": id, "type": "Vehicle",
                        "speed": {"type": "Property", "value": speed},
                        "brand": {"type": "Property", "value": "Mercedes"}}),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{body}");
        }
        let q = "entityOperations/query";
        let (st, body) = send(&app, "POST", q, &query(json!({"q": "speed>100"}))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(body.contains("urn:ngsi-ld:Vehicle:A2"), "{body}");
        assert!(
            !body.contains("urn:ngsi-ld:Vehicle:A1"),
            "q must filter: {body}"
        );
        let (st, body) = send(&app, "POST", q, &query(json!({"pick": ["speed"]}))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(body.contains("speed"), "{body}");
        assert!(!body.contains("brand"), "pick must project: {body}");
        let (st, body) = send(&app, "POST", q, &query(json!({"omit": ["speed"]}))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(body.contains("brand"), "{body}");
        assert!(!body.contains("speed"), "omit must remove: {body}");
    }

    /// Table 5.2.23-1 on the temporal operation (5.7.4): the entities
    /// selector's id member selects, temporalQ is accepted, and containedBy
    /// is "Only applicable for the Retrieve Entity and Query Entities
    /// operations".
    #[tokio::test]
    async fn temporal_query_body_entity_selector() {
        let app = router(AppState::new("t5223c".into()));
        for id in ["urn:ngsi-ld:Vehicle:T1", "urn:ngsi-ld:Vehicle:T2"] {
            let (st, body) = send(
                &app,
                "POST",
                "temporal/entities",
                &json!({"id": id, "type": "Vehicle",
                        "speed": [{"type": "Property", "value": 1,
                                   "observedAt": "2020-08-01T12:00:00Z"}]}),
            )
            .await;
            assert!(st.is_success(), "{st} {body}");
        }
        let tq = json!({"timerel": "after", "timeAt": "2020-01-01T00:00:00Z"});
        let doc = json!({"type": "Query",
            "entities": [{"type": "Vehicle", "id": "urn:ngsi-ld:Vehicle:T1"}],
            "temporalQ": tq});
        let (st, body) = send(&app, "POST", "temporal/entityOperations/query", &doc).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(body.contains("urn:ngsi-ld:Vehicle:T1"), "{body}");
        assert!(
            !body.contains("urn:ngsi-ld:Vehicle:T2"),
            "entities id selector must narrow the temporal query: {body}"
        );
        let doc = json!({"type": "Query", "entities": [{"type": "Vehicle"}],
            "temporalQ": tq, "containedBy": ["urn:ngsi-ld:Vehicle:T2"]});
        let (st, _) = send(&app, "POST", "temporal/entityOperations/query", &doc).await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "containedBy is not applicable to 5.7.4"
        );
    }
}

#[cfg(test)]
mod clause_6_3_11 {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        serde_json::from_slice(&bytes).expect("json body")
    }

    /// 6.3.11 Table 6.3.11-1: `expiresAt` is included in response payloads
    /// only when `options=sysAttrs` — entity level, attribute level and
    /// temporal instances alike (same gate as createdAt/modifiedAt).
    #[tokio::test]
    async fn clause_6_3_11_expires_at_gated_by_sysattrs() {
        let mut st = AppState::new("antares-test".into());
        crate::wire(&mut st).await;
        let app = router(st);
        let entity = serde_json::json!({
            "id": "urn:ngsi-ld:Building:exp6311", "type": "Building",
            "expiresAt": "2100-01-01T00:00:00Z",
            "name": {"type": "Property", "value": "x",
                     "observedAt": "2026-08-13T00:00:00Z",
                     "expiresAt": "2100-01-01T00:00:00Z"}
        });
        let body = entity.to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::post("/ngsi-ld/v1/entities")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len())
                    .body(Body::from(body))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::CREATED);

        let plain = body_json(
            app.clone()
                .oneshot(
                    Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:exp6311")
                        .body(Body::empty())
                        .expect("req"),
                )
                .await
                .expect("resp"),
        )
        .await;
        assert!(
            plain.get("expiresAt").is_none(),
            "entity expiresAt must be sysAttrs-gated (6.3.11): {plain}"
        );
        assert!(
            plain["name"].get("expiresAt").is_none(),
            "attribute expiresAt must be sysAttrs-gated (6.3.11): {plain}"
        );

        let sys = body_json(
            app.clone()
                .oneshot(
                    Request::get(
                        "/ngsi-ld/v1/entities/urn:ngsi-ld:Building:exp6311?options=sysAttrs",
                    )
                    .body(Body::empty())
                    .expect("req"),
                )
                .await
                .expect("resp"),
        )
        .await;
        assert_eq!(sys["expiresAt"], "2100-01-01T00:00:00Z");
        assert_eq!(sys["name"]["expiresAt"], "2100-01-01T00:00:00Z");

        let inst_of = |body: &serde_json::Value| -> serde_json::Value {
            let a = &body["name"];
            if a.is_array() {
                a[0].clone()
            } else {
                a.clone()
            }
        };
        let t_plain = body_json(
            app.clone()
                .oneshot(
                    Request::get(
                        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Building:exp6311?timerel=after&timeAt=2020-01-01T00:00:00Z",
                    )
                    .body(Body::empty())
                    .expect("req"),
                )
                .await
                .expect("resp"),
        ).await
        ;
        assert!(
            inst_of(&t_plain).get("expiresAt").is_none(),
            "temporal instance expiresAt must be sysAttrs-gated (6.3.11): {t_plain}"
        );
        let t_sys = body_json(
            app.clone()
                .oneshot(
                    Request::get(
                        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Building:exp6311?timerel=after&timeAt=2020-01-01T00:00:00Z&options=sysAttrs",
                    )
                    .body(Body::empty())
                    .expect("req"),
                )
                .await
                .expect("resp"),
        ).await
        ;
        assert_eq!(inst_of(&t_sys)["expiresAt"], "2100-01-01T00:00:00Z");
    }
}

/// The programmatic egress override, not `ANTARES_EGRESS_ALLOW_PRIVATE`: a
/// sibling test reading the environment while another rewrote it saw the
/// policy missing and refused the loopback forward. An atomic store carries
/// the same switch with no write for a reader to land in the middle of.
#[cfg(test)]
pub(crate) fn allow_private() {
    antares_jsonld::allow_private_egress(true);
}

#[cfg(test)]
mod f_ant2_019_background_count {
    /// A request-born task that panics is still uncounted when it ends: 64
    /// of them left the count 64 up for good, and a shutdown drain waited
    /// its whole deadline for them. Other tests' tasks are few and short.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_panicked_task_leaves_the_count() {
        const N: usize = 64;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(N);
        for _ in 0..N {
            let tx = tx.clone();
            super::spawn(async move {
                drop(tx);
                std::panic::resume_unwind(Box::new("background task fails"));
            });
        }
        drop(tx);
        // every sender gone: every task ran to its panic
        assert!(rx.recv().await.is_none());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            super::background_tasks() < N,
            "{}",
            super::background_tasks()
        );
    }
}
