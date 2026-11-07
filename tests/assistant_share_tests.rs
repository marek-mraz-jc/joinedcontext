//! The assistant's endpoint proposal through the router (T-0586, EP-72, PF-50): rendered, not
//! written; refused for a caller who may not propose an Endpoint; the lane says what the
//! change would be.
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "test-csrf-token-12345";

fn identity(email: &str, groups: &[&str]) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: groups.iter().map(|g| (*g).to_owned()).collect(),
        second_factor: false,
    }
}

fn cookies(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let s = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// The organization, a role that proposes endpoints and one binding for the developer.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(org(
        "Organization",
        "hel",
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    mirror.upsert(org(
        "Role",
        "endpoint-publisher",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["propose"] }] }),
    ));
    mirror.upsert(org(
        "RoleBinding",
        "dev-publishes",
        json!({
            "subjects": [{ "user": "dev@hel.fi" }],
            "role": "endpoint-publisher",
            "scope": { "project": "helsinki" }
        }),
    ));
    mirror
}

async fn post(config: &Config, who: Identity, uri: &str, body: &Value) -> (StatusCode, Value) {
    let app = server::app(AppState::new(config.clone(), None).with_mirror(mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookies(config, who))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn request() -> Value {
    json!({
        "contextSpace": "helsinki",
        "name": "bikes-regional-transport",
        "title": "City bikes for the regional transport team",
        "allowedProjects": ["regional-transport"],
        "hiddenAttributes": ["maintenanceNote"],
        "entityTypes": ["BikeHireDockingStation"]
    })
}

const URI: &str = "/api/v1/projects/helsinki/assistant/propose-endpoint";

#[tokio::test]
async fn a_publisher_gets_the_manifests_the_slug_and_the_lane_and_nothing_is_written() {
    let config = Config::for_tests();
    let (status, body) = post(&config, identity("dev@hel.fi", &[]), URI, &request()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["lane"], "yellow");
    assert_eq!(body["slug"].as_str().map(str::len), Some(26));
    assert_eq!(body["endpoint"]["kind"], "Endpoint");
    assert_eq!(body["endpoint"]["metadata"]["namespace"], "helsinki");
    assert_eq!(
        body["endpoint"]["metadata"]["title"]["en"],
        "City bikes for the regional transport team"
    );
    assert_eq!(body["endpoint"]["spec"]["slug"], body["slug"]);
    assert_eq!(body["endpoint"]["spec"]["audience"], "project-list");
    assert_eq!(
        body["endpoint"]["spec"]["enabledRepresentations"],
        json!(["ngsi-ld", "csv"])
    );
    assert_eq!(
        body["endpoint"]["spec"]["projection"]["hiddenAttributes"],
        json!(["maintenanceNote"])
    );
    assert_eq!(
        body["policies"][0]["spec"]["assigner"], "did:web:{orgDomain}",
        "the organization, as the placeholder the loader renders (CC-82)"
    );
    assert!(
        !body["policies"].to_string().contains("hel.fi"),
        "no generated policy writes the domain out: {}",
        body["policies"]
    );
    assert_eq!(body["prefill"]["name"], "bikes-regional-transport");
    assert_eq!(
        body["prefill"]["hiddenAttributes"],
        json!(["maintenanceNote"])
    );
    // T-1221: the manifest carries a language map and the form's `title` is one string. The
    // draft used to hand the map over, and the page that reads a draft's title as text crashed
    // the whole Portal to its error boundary — on camera, during the share take.
    assert!(
        body["endpoint"]["metadata"]["title"].is_object(),
        "the manifest keeps the language map"
    );
    assert!(
        body["prefill"]["title"].is_string(),
        "the form's title is text, not a map: {}",
        body["prefill"]["title"]
    );
    assert_eq!(
        body["prefill"]["title"],
        "City bikes for the regional transport team"
    );
    assert!(
        body.get("mergeRequest").is_none() && body.get("status").is_none(),
        "no Change exists: {body}"
    );
}

#[tokio::test]
async fn a_public_audience_is_the_red_lane_and_the_bootstrap_admin_may_ask() {
    let config = Config::for_tests();
    let mut body = request();
    body["audience"] = json!("public");
    body["allowedProjects"] = json!([]);
    let (status, answer) = post(
        &config,
        identity("admin@hel.fi", &["portal-approver"]),
        URI,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["lane"], "red", "CC-63: public exposure is red");
    assert_eq!(
        answer["policies"][0]["spec"]["assignee"],
        json!({ "kind": "role", "id": "public" })
    );
}

#[tokio::test]
async fn a_caller_without_a_binding_is_refused_before_anything_is_rendered_for_them() {
    let config = Config::for_tests();
    let (status, body) = post(&config, identity("viewer@hel.fi", &[]), URI, &request()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "PF-50: {body}");
    assert!(
        body.get("slug").is_none(),
        "a refused caller sees no rendering: {body}"
    );
}

#[tokio::test]
async fn what_cannot_be_rendered_is_400_with_the_reason() {
    let config = Config::for_tests();
    let mut body = request();
    body["allowedProjects"] = json!([]);
    let (status, answer) = post(&config, identity("dev@hel.fi", &[]), URI, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .is_some_and(|d| d.contains("allowedProjects")),
        "{answer}"
    );

    let mut body = request();
    body["audience"] = json!("everyone");
    let (status, _) = post(&config, identity("dev@hel.fi", &[]), URI, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn anonymous_is_401() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(URI)
                .header(header::CONTENT_TYPE, "application/json")
                // The CSRF pair without a session: CSRF is checked first, so this is what
                // proves the session check answers 401.
                .header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
                .header(CSRF_HEADER, CSRF)
                .body(Body::from(request().to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
