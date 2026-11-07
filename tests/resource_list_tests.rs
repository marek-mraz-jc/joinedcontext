use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, Phase, ResourceEnvelope, Status, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

fn make_session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        let pair = raw.split(';').next().unwrap_or_default();
        parts.push(pair.to_string());
    }
    parts.join("; ")
}

fn seed_demo_mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());

    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "ContextSpace".into(),
        metadata: ObjectMeta {
            name: "ovzdusie".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        },
        spec: serde_json::json!({ "tenant": "ovzdusie" }),
        status: Some(Status {
            phase: Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    });

    let mut labels = BTreeMap::new();
    labels.insert("joinedcontext.com/domain".into(), "environment".into());

    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Endpoint".into(),
        metadata: ObjectMeta {
            name: "public-air".into(),
            namespace: Some("ovzdusie".into()),
            labels,
            ..Default::default()
        },
        spec: serde_json::json!({
            "audience": "public",
            "representations": ["ngsi-ld", "file.geojson"]
        }),
        status: Some(Status {
            phase: Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    });

    mirror
}

#[tokio::test]
async fn anonymous_call_returns_401() {
    let config = Config::for_tests();
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn label_selector_filtering() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?labelSelector=joinedcontext.com/domain=environment")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["kind"], "List");
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["metadata"]["name"], "public-air");

    let no_match = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?labelSelector=joinedcontext.com/domain=mobility")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(no_match.status(), StatusCode::OK);
    let body = no_match.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn field_selector_filtering() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?fieldSelector=metadata.name=public-air")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["metadata"]["name"], "public-air");
}

#[tokio::test]
async fn two_page_pagination() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();

    for name in &["air-station-1", "air-station-2"] {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: (*name).into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        });
    }

    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let res1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?limit=2")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res1.status(), StatusCode::OK);
    let body1 = res1.into_body().collect().await.unwrap().to_bytes();
    let page1: serde_json::Value = serde_json::from_slice(&body1).unwrap();
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let cont_token = page1["metadata"]["continue"]
        .as_str()
        .expect("continue token");
    assert_eq!(page1["metadata"]["remainingItemCount"], 1);

    let res2 = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/ovzdusie/endpoints?limit=2&continue={cont_token}"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res2.status(), StatusCode::OK);
    let body2 = res2.into_body().collect().await.unwrap().to_bytes();
    let page2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert!(page2["metadata"].get("continue").is_none());
    assert!(page2["metadata"].get("remainingItemCount").is_none());
}

#[tokio::test]
async fn unknown_plural_returns_404_problem() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/nonexistentkind")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/resource-not-found"
    );
}

/// MF-11 (T-2375): a revision is read from the repository, so a Portal with none says so
/// instead of answering today's list as though it were the one asked for.
#[tokio::test]
async fn a_revision_on_a_portal_without_a_repository_is_unavailable() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints?revision=abc1234")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}
