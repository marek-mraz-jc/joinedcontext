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
async fn anonymous_get_returns_401() {
    let config = Config::for_tests();
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints/public-air")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_returns_the_envelope_with_status_phase() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints/public-air")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let envelope: ResourceEnvelope = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope.api_version, API_VERSION);
    assert_eq!(envelope.kind, "Endpoint");
    assert_eq!(envelope.metadata.name, "public-air");
    assert_eq!(envelope.metadata.namespace.as_deref(), Some("ovzdusie"));
    assert_eq!(envelope.status.as_ref().map(|s| s.phase), Some(Phase::Live));
}

#[tokio::test]
async fn missing_name_and_unknown_plural_return_identical_404_body() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let mirror = seed_demo_mirror();
    let app = server::app(AppState::new(config, None).with_mirror(mirror));

    let missing_name_res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/endpoints/nonexistent")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(missing_name_res.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        missing_name_res
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap(),
        "application/problem+json"
    );
    let missing_name_body = missing_name_res
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();

    let unknown_plural_res = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/unknownplural/nonexistent")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(unknown_plural_res.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        unknown_plural_res
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap(),
        "application/problem+json"
    );
    let unknown_plural_body = unknown_plural_res
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();

    assert_eq!(missing_name_body, unknown_plural_body);

    let problem: serde_json::Value = serde_json::from_slice(&missing_name_body).unwrap();
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/resource-not-found"
    );
    assert_eq!(problem["title"], "Resource Not Found");
    assert_eq!(problem["status"], 404);
}
