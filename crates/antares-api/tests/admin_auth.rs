// SPDX-License-Identifier: EUPL-1.2
//! The `/q` admin credential (T-3569): configured, every admin route but
//! health and readiness refuses a caller without the bearer secret and
//! serves one with it exactly as an unconfigured broker does; unconfigured,
//! nothing changes. The ops router a worker pod serves carries the same
//! guard, and a refused tenant purge purges nothing.

use antares_api::admin_auth::{AdminAuth, UNAUTHORIZED_TYPE};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &str = "a-test-secret-of-at-least-32-bytes!";

/// Every guarded route, with a method it serves.
const GUARDED: [(&str, &str); 10] = [
    ("GET", "/q/tenants"),
    ("GET", "/q/tenants/acme"),
    ("DELETE", "/q/tenants/acme"),
    ("GET", "/q/tenants/acme/digest"),
    ("GET", "/q/tenants/acme/export"),
    ("POST", "/q/tenants/acme/import"),
    ("GET", "/q/dead-letters"),
    ("DELETE", "/q/dead-letters/urn:dl:1"),
    ("POST", "/q/dead-letters/urn:dl:1/replay"),
    ("GET", "/q/metrics"),
];

fn secured(metrics_open: bool) -> AppState {
    AppState::new("admin-auth".into())
        .with_admin_auth(AdminAuth::new(SECRET, metrics_open).expect("secret"))
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    authorization: Option<&str>,
) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        // the bounds wall answers a bodyless POST without a length with 411
        .header(header::CONTENT_LENGTH, "0");
    if let Some(a) = authorization {
        req = req.header(header::AUTHORIZATION, a);
    }
    let res = app
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let challenge = res
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, challenge, body)
}

#[tokio::test]
async fn a_guarded_route_refuses_without_the_secret_and_serves_with_it() {
    let st = secured(false);
    let bare = AppState::new("admin-auth".into());
    let good = format!("Bearer {SECRET}");
    for (method, path) in GUARDED {
        for wrong in [None, Some("Bearer not-the-secret"), Some(SECRET)] {
            let (status, challenge, body) =
                call(antares_api::router(st.clone()), method, path, wrong).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {path} {wrong:?}"
            );
            assert_eq!(
                challenge.as_deref(),
                Some("Bearer realm=\"antares-admin\""),
                "{method} {path}"
            );
            assert_eq!(body["type"], UNAUTHORIZED_TYPE, "{method} {path}");
            assert!(!body.to_string().contains(SECRET), "{body}");
        }
        let (with, _, _) = call(antares_api::router(st.clone()), method, path, Some(&good)).await;
        let (unconfigured, _, _) =
            call(antares_api::router(bare.clone()), method, path, None).await;
        assert_ne!(with, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_eq!(
            with, unconfigured,
            "{method} {path}: the secret changes nothing else"
        );
    }
}

#[tokio::test]
async fn unconfigured_serves_every_admin_route_as_before() {
    let st = AppState::new("admin-auth".into());
    for (method, path) in GUARDED {
        let (status, challenge, _) =
            call(antares_api::router(st.clone()), method, path, None).await;
        assert_ne!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert!(challenge.is_none(), "{method} {path}");
    }
}

#[tokio::test]
async fn health_and_readiness_answer_without_the_secret() {
    let st = secured(false);
    for path in ["/q/health", "/q/ready"] {
        for app in [
            antares_api::router(st.clone()),
            antares_api::ops_router(st.clone()),
        ] {
            let (status, _, _) = call(app, "GET", path, None).await;
            assert_eq!(status, StatusCode::OK, "{path}");
        }
    }
}

/// F-ANT2-005: the open health body keeps its status code and gives the
/// per-Tenant view, schema and rule-document reports as counts; the admin
/// bearer reads them whole.
#[cfg(feature = "views")]
#[tokio::test]
async fn health_names_tenants_only_to_the_admin() {
    let st = secured(false);
    st.views
        .set_policy_report(json!({"acme": {"revision": "r1", "state": "loaded"}}));
    let (status, _, open) = call(antares_api::router(st.clone()), "GET", "/q/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!open.to_string().contains("acme"), "{open}");
    assert_eq!(open["policy"]["documents"], json!({"count": 1}), "{open}");
    let good = format!("Bearer {SECRET}");
    let (status, _, admin) = call(antares_api::router(st), "GET", "/q/health", Some(&good)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        admin["policy"]["documents"]["acme"]["revision"], "r1",
        "{admin}"
    );
}

#[tokio::test]
async fn metrics_left_open_answer_without_the_secret() {
    let open = secured(true);
    let bare = AppState::new("admin-auth".into());
    let (status, _, _) = call(antares_api::router(open.clone()), "GET", "/q/metrics", None).await;
    let (unconfigured, _, _) = call(antares_api::router(bare), "GET", "/q/metrics", None).await;
    assert_eq!(status, unconfigured);
    // the rest stays guarded
    let (status, _, _) = call(antares_api::router(open), "GET", "/q/tenants", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_worker_pods_ops_router_carries_the_same_guard() {
    let st = secured(false);
    let (status, _, _) = call(
        antares_api::ops_router(st.clone()),
        "GET",
        "/q/tenants",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let good = format!("Bearer {SECRET}");
    let (status, _, _) = call(
        antares_api::ops_router(st),
        "GET",
        "/q/tenants",
        Some(&good),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_refused_purge_purges_nothing() {
    let st = secured(false);
    let entity = json!({"id": "urn:ngsi-ld:Vehicle:kept", "type": "Vehicle",
                        "speed": {"type": "Property", "value": 1}});
    let res = antares_api::router(st.clone())
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header(header::CONTENT_TYPE, "application/json")
                .header("NGSILD-Tenant", "acme")
                .header(header::CONTENT_LENGTH, entity.to_string().len())
                .body(Body::from(entity.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(res.status(), StatusCode::CREATED);

    let (status, _, _) = call(
        antares_api::router(st.clone()),
        "DELETE",
        "/q/tenants/acme",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let good = format!("Bearer {SECRET}");
    let (status, _, _) = call(
        antares_api::router(st),
        "GET",
        "/q/tenants/acme",
        Some(&good),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the tenant is still there");
}

/// T-5032: a broker that requires a subject serves an NGSI-LD request
/// carrying no subject header only with the admin bearer and refuses it
/// with none configured at all; a broker that does not require one serves
/// it as before. (A request with a subject is the engine's: this process
/// names no subject header, so the joinedcontext-policy engine tests cover
/// it.)
#[tokio::test]
async fn a_request_without_a_subject_needs_the_admin_bearer_where_a_subject_is_required() {
    let path = "/ngsi-ld/v1/csourceRegistrations?type=Car";
    let good = format!("Bearer {SECRET}");
    let mut st = secured(false);
    st.subject_required = true;
    let mut bare = AppState::new("admin-auth".into());
    bare.subject_required = true;
    for (state, authorization) in [
        (&st, None),
        (&st, Some("Bearer not-the-secret")),
        (&bare, None),
        (&bare, Some(good.as_str())),
    ] {
        let (status, challenge, body) = call(
            antares_api::router(state.clone()),
            "GET",
            path,
            authorization,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{authorization:?}");
        assert!(challenge.is_some_and(|c| c.starts_with("Bearer")));
        assert_eq!(body["type"], UNAUTHORIZED_TYPE);
    }
    let (status, ..) = call(antares_api::router(st), "GET", path, Some(&good)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, ..) = call(antares_api::router(secured(false)), "GET", path, None).await;
    assert_eq!(status, StatusCode::OK, "not required: served as before");
}
