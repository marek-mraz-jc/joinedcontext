//! The gallery's data and the flow it starts (T-0207, T-0208, CC-24, CC-30, CC-59).

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

/// A Dashboard, the simplest project-scoped kind: one path segment, no `{space}` placeholder.
/// Written as a plain multi-line literal on purpose: a `\` line continuation would eat the YAML
/// indentation with the newline, and the template would render a manifest with no metadata.
const DASHBOARD_TEMPLATE: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Dashboard
metadata:
  name: alert-{{ title }}
spec:
  title: {{ title }}
  webhook: {{ webhookUrl }}
";

/// The same, rendering into a namespace that is not the project the flow runs in.
const ESCAPING_TEMPLATE: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Dashboard
metadata:
  name: alert-{{ title }}
  namespace: someone-elses-project
spec:
  title: {{ title }}
  webhook: {{ webhookUrl }}
";

fn blueprint_with(
    name: &str,
    category: &str,
    risk: &str,
    allowed_roles: Value,
    template: &str,
) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Blueprint".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            // Organization-level, like `org.yaml`: `sync.rs` files it under "org" whatever
            // its path says, and the gallery lists exactly that namespace.
            namespace: Some("org".to_string()),
            ..Default::default()
        },
        spec: json!({
            "version": "1.2.0",
            "category": category,
            "riskClass": risk,
            "allowedRoles": allowed_roles,
            "parameterSchema": {
                "type": "object",
                "required": ["title", "webhookUrl"],
                "properties": {
                    "title": { "type": "string" },
                    "webhookUrl": { "type": "string" },
                },
            },
            "templates": [{ "name": "dashboard", "template": template }],
        }),
        status: None,
    }
}

fn blueprint(name: &str, category: &str, risk: &str, allowed_roles: Value) -> ResourceEnvelope {
    blueprint_with(name, category, risk, allowed_roles, DASHBOARD_TEMPLATE)
}

fn seeded_mirror() -> Arc<Mirror> {
    let mirror = Mirror::new();
    mirror.upsert(blueprint(
        "threshold-alert",
        "alerting",
        "green",
        json!(["domain-editor"]),
    ));
    mirror.upsert(blueprint(
        "cross-city-sharing",
        "federation",
        "red",
        json!(["org-admin"]),
    ));
    // `spec.allowedRoles` is required and non-empty in jc-core, so a blueprint that reaches the
    // mirror without one is malformed. The gallery reads that as "nobody", never as "everybody".
    mirror.upsert(blueprint(
        "names-no-role",
        "onboarding",
        "yellow",
        json!([]),
    ));
    mirror.upsert(blueprint_with(
        "escapes-the-project",
        "onboarding",
        "green",
        json!(["domain-editor"]),
        ESCAPING_TEMPLATE,
    ));
    Arc::new(mirror)
}

fn app_with(roles: &[&str]) -> (axum::Router, String) {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, roles);
    let app = server::app(AppState::new(config, None).with_mirror(seeded_mirror()));
    (app, cookie)
}

/// The organization repository's answer to "may this person propose a Dashboard here": the role
/// and the binding a project's steward writes (PF-50, PF-56).
fn bound_to_propose() -> Vec<ResourceEnvelope> {
    let org = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: kind.to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some("org".to_string()),
            ..Default::default()
        },
        spec,
        status: None,
    };
    vec![
        org(
            "Role",
            "dashboard-editor",
            json!({ "rules": [{ "kinds": ["Dashboard"], "verbs": ["propose"] }] }),
        ),
        org(
            "RoleBinding",
            "editors",
            json!({
                "subjects": [{ "user": "demo.steward@banskabystrica.sk" }],
                "role": "dashboard-editor",
                "scope": { "project": "ovzdusie" },
            }),
        ),
    ]
}

fn app_bound(roles: &[&str]) -> (axum::Router, String) {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, roles);
    let mirror = seeded_mirror();
    for envelope in bound_to_propose() {
        mirror.upsert(envelope);
    }
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    (app, cookie)
}

async fn post_flow_to(app: axum::Router, cookie: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/flows")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", TEST_CSRF_TOKEN)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
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

fn list_contains(list: &Value, name: &str) -> bool {
    names(list).iter().any(|found| found == name)
}

async fn get_gallery(roles: &[&str]) -> Value {
    let (app, cookie) = app_with(roles);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/blueprints")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn post_flow(roles: &[&str], body: Value) -> (StatusCode, Value) {
    let (app, cookie) = app_with(roles);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/flows")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", TEST_CSRF_TOKEN)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test]
async fn the_gallery_shows_only_what_this_caller_may_run() {
    // CC-59: `spec.allowedRoles` is the filter, in both directions.
    let list = get_gallery(&["domain-editor"]).await;
    assert_eq!(names(&list), vec!["escapes-the-project", "threshold-alert"]);

    let admin = get_gallery(&["org-admin"]).await;
    assert_eq!(names(&admin), vec!["cross-city-sharing"]);
}

#[tokio::test]
async fn a_blueprint_that_names_no_role_is_visible_to_nobody() {
    // Fail-closed (CC-59): jc-core refuses an empty `spec.allowedRoles`, so one that turns up
    // in the mirror anyway is a broken manifest. Reading it as "open to all" would make a
    // malformed blueprint the most widely available one in the gallery.
    for roles in [&[][..], &["domain-editor"][..], &["org-admin"][..]] {
        let list = get_gallery(roles).await;
        assert!(
            !list_contains(&list, "names-no-role"),
            "a blueprint with no allowedRoles reached a caller with roles {roles:?}"
        );
    }
    assert!(get_gallery(&[]).await["items"]
        .as_array()
        .expect("items")
        .is_empty());
}

#[tokio::test]
async fn the_gallery_carries_what_a_card_needs_to_render() {
    let list = get_gallery(&["domain-editor"]).await;
    let card = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["metadata"]["name"] == "threshold-alert")
        .expect("the blueprint this caller may run");
    assert_eq!(card["spec"]["riskClass"], "green");
    assert_eq!(card["spec"]["category"], "alerting");
    assert_eq!(card["spec"]["version"], "1.2.0");
    // The form is generated from this and from nothing else (CC-24, CC-31).
    assert_eq!(card["spec"]["parameterSchema"]["type"], "object");
}

#[tokio::test]
async fn an_anonymous_call_is_refused_as_a_problem_document() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None).with_mirror(seeded_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/blueprints")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
}

#[tokio::test]
async fn running_a_blueprint_this_caller_may_not_see_answers_like_a_missing_one() {
    // Hiding a card is not an authorisation, and a different answer here would tell a caller
    // which blueprints exist (R20).
    let (status, _) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "cross-city-sharing", "version": "1.2.0", "parameters": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (missing, _) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "no-such-blueprint", "version": "1.2.0", "parameters": {} }),
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_form_filled_against_another_version_is_a_conflict() {
    // CC-26: expanding these values against the current schema would produce a manifest the
    // person filling the form never saw.
    let (status, problem) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "threshold-alert", "version": "1.0.0", "parameters": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("1.2.0"),
        "the answer must name the version that is current now: {problem}"
    );
}

#[tokio::test]
async fn parameters_that_miss_the_schema_come_back_as_one_list_of_violations() {
    // CC-24: the form marks every bad field in one pass, so a user is not sent round the loop
    // once per mistake. jcctl collects the violations; the Portal only passes them on.
    let (status, problem) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "threshold-alert", "version": "1.2.0", "parameters": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let errors = problem["errors"]
        .as_array()
        .expect("one entry per violation");
    assert_eq!(
        errors.len(),
        2,
        "both violations, not the first one: {problem}"
    );
    let joined = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("title") && joined.contains("webhookUrl"),
        "both missing parameters must be named: {joined}"
    );
}

#[tokio::test]
async fn a_template_cannot_render_into_another_project() {
    // A blueprint is authored once and run in many projects, so a template that names a
    // namespace is a way out of the project the caller chose. Refused before anything is
    // written, exactly as a hand-written manifest with a foreign namespace is.
    let (status, problem) = post_flow(
        &["domain-editor"],
        json!({
            "blueprint": "escapes-the-project",
            "version": "1.2.0",
            "parameters": { "title": "nocne-hluky", "webhookUrl": "https://example.org/hook" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("someone-elses-project") && detail.contains("ovzdusie"),
        "the refusal must name both namespaces: {detail}"
    );
}

#[tokio::test]
async fn a_flow_is_refused_in_a_project_the_caller_holds_no_binding_in() {
    // The realm role on the card decides which blueprints a person is offered (CC-59); what
    // they may propose in this project is the organization repository's bindings, and this
    // caller has none. 403 before the forge, naming the verb and the kind (PF-50, T-0799).
    let (status, problem) = post_flow(
        &["domain-editor"],
        json!({
            "blueprint": "threshold-alert",
            "version": "1.2.0",
            "parameters": { "title": "nocne-hluky", "webhookUrl": "https://example.org/hook" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("propose") && detail.contains("Dashboard"),
        "the refusal names what is missing: {detail}"
    );
}

#[tokio::test]
async fn a_valid_flow_reaches_the_forge_and_stops_there_when_there_is_none() {
    // Everything the Portal can check on its own has passed: the roles, the binding, the
    // version, the parameters and every rendered manifest. What is left is the merge request,
    // and without a forge there is nowhere to open one — 503, never a silent success (CC-32).
    let (app, cookie) = app_bound(&["domain-editor"]);
    let (status, problem) = post_flow_to(
        app,
        &cookie,
        json!({
            "blueprint": "threshold-alert",
            "version": "1.2.0",
            "parameters": { "title": "nocne-hluky", "webhookUrl": "https://example.org/hook" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("git forge"),
        "the answer must say what is missing: {problem}"
    );
}

#[tokio::test]
async fn a_flow_without_a_session_never_reaches_the_blueprint() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None).with_mirror(seeded_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/flows")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "blueprint": "threshold-alert", "version": "1.2.0", "parameters": {} })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN,
        "an unauthenticated flow must not be started: {}",
        response.status()
    );
}
