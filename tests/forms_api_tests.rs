//! `GET /api/v1/forms`: the UiSchema manifests that arrange the Portal's forms
//! (T-0452, UI-01, UI-02, CC-31).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::CSRF_COOKIE;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";

fn session_cookie(config: &Config, roles: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: roles.iter().map(|r| (*r).to_string()).collect(),
            groups: Vec::new(),
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
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        parts.push(raw.split(';').next().unwrap_or_default().to_string());
    }
    format!("{}; {CSRF_COOKIE}={TEST_CSRF_TOKEN}", parts.join("; "))
}

/// A `UiSchema` as the mirror holds it: organization-scoped, named after the kind it arranges.
fn ui_schema(name: &str, arranges: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "UiSchema".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some("org".to_string()),
            ..Default::default()
        },
        spec: json!({
            "for": arranges,
            "order": ["name", "slug"],
            "fields": { "slug": { "widget": "text" } },
        }),
        status: None,
    }
}

/// A project-scoped resource that is not a form, to prove the handler filters rather than
/// returning whatever the mirror happens to hold.
fn a_dashboard() -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Dashboard".to_string(),
        metadata: ObjectMeta {
            name: "air".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({}),
        status: None,
    }
}

fn app_with(mirror: Arc<Mirror>) -> (axum::Router, String) {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, &["domain-editor"]);
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    (app, cookie)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

async fn get_forms(mirror: Arc<Mirror>, cookie: Option<&str>) -> (StatusCode, Value) {
    let (app, session) = app_with(mirror);
    let mut request = Request::builder().uri("/api/v1/forms");
    if let Some(header_value) = cookie.map(|_| session.as_str()) {
        request = request.header(header::COOKIE, header_value);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    if status != StatusCode::OK {
        return (status, Value::Null);
    }
    (status, body_json(response).await)
}

fn names(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            item["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_endpoint_returns_the_manifests_in_portal_forms() {
    let mirror = Mirror::new();
    mirror.upsert(ui_schema("endpoint", "Endpoint"));
    mirror.upsert(ui_schema("pipeline", "Pipeline"));
    mirror.upsert(a_dashboard());

    let (status, body) = get_forms(Arc::new(mirror), Some("yes")).await;
    assert_eq!(status, StatusCode::OK);
    let mut found = names(&body);
    found.sort();
    assert_eq!(found, ["endpoint", "pipeline"]);
    // The kind each one arranges is what the UI keys them by, so it has to survive the hop.
    let arranged: Vec<&str> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["spec"]["for"].as_str())
        .collect();
    assert!(arranged.contains(&"Endpoint") && arranged.contains(&"Pipeline"));
}

#[tokio::test]
async fn an_instance_with_no_form_manifests_answers_an_empty_list_and_not_a_404() {
    // Every instance starts here, and a 404 would make "no manifests yet" indistinguishable
    // from "the route is gone", which is the difference between a plain form and a broken UI.
    let (status, body) = get_forms(Arc::new(Mirror::new()), Some("yes")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kind"], "List");
    assert_eq!(names(&body), Vec::<String>::new());
}

#[tokio::test]
async fn the_endpoint_needs_the_same_session_as_every_other_resource_read() {
    let mirror = Mirror::new();
    mirror.upsert(ui_schema("endpoint", "Endpoint"));
    let (status, _) = get_forms(Arc::new(mirror), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
