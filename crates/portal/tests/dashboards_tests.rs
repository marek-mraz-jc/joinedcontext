//! T-0528: a public dashboard reads only through public Endpoints (UI-19), refused at write
//! time wherever the three manifests meet.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "test-csrf-token-12345";
const PROJECT: &str = "ovzdusie";

fn cookies(config: &Config) -> String {
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:steward".into(),
            username: "steward".into(),
            email: Some("steward@example.org".into()),
            name: None,
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
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

fn live(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, PROJECT),
        spec,
        status: None,
    }
}

fn endpoint(name: &str, audience: &str) -> ResourceEnvelope {
    live(
        "Endpoint",
        name,
        json!({ "contextSpaceRef": "ovzdusie", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "audience": audience, "enabledRepresentations": ["ngsi-ld", "ogc-features"] }),
    )
}

fn layer(name: &str, endpoint: &str) -> ResourceEnvelope {
    live(
        "Layer",
        name,
        json!({ "sourceEndpointRef": endpoint, "entityType": "AirQualityObserved", "style": "circle", "filter": { "q": "pm10>0" } }),
    )
}

fn dashboard(visibility: &str, layers: &[&str]) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Dashboard",
        "metadata": { "name": "air", "namespace": PROJECT },
        "spec": { "title": { "en": "Air" }, "visibility": visibility, "pages": [{ "layout": "full-map", "layers": layers }] }
    })
}

fn manifest(env: &ResourceEnvelope) -> Value {
    serde_json::to_value(env).expect("json")
}

/// The check of one manifest (`?dryRun=All`), which is where a person meets this rule.
async fn put(
    config: &Config,
    envelopes: Vec<ResourceEnvelope>,
    plural: &str,
    body: &Value,
) -> (StatusCode, String) {
    write(config, envelopes, plural, body, true).await
}

/// The same manifest proposed for real, to prove the rule is not only in the check.
async fn propose(
    config: &Config,
    envelopes: Vec<ResourceEnvelope>,
    plural: &str,
    body: &Value,
) -> (StatusCode, String) {
    write(config, envelopes, plural, body, false).await
}

/// A red verdict's findings as one string, for a check that refused the manifest (T-2234).
fn findings(body: &str) -> String {
    let answer: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    assert_eq!(answer["verdict"]["ok"], false, "{body}");
    answer["verdict"]["findings"].to_string()
}

async fn write(
    config: &Config,
    envelopes: Vec<ResourceEnvelope>,
    plural: &str,
    body: &Value,
    dry_run: bool,
) -> (StatusCode, String) {
    let state = AppState::new(config.clone(), None);
    for env in envelopes {
        state.mirror.upsert(env);
    }
    let name = body["metadata"]["name"].as_str().expect("name");
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/projects/{PROJECT}/{plural}/{name}{}",
                    if dry_run { "?dryRun=All" } else { "" }
                ))
                .header(header::COOKIE, cookies(config))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn a_public_dashboard_on_a_public_endpoint_is_accepted() {
    let config = Config::for_tests();
    let (status, body) = put(
        &config,
        vec![endpoint("air", "public"), layer("stations", "air")],
        "dashboards",
        &dashboard("public", &["stations"]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_public_dashboard_on_a_private_endpoint_is_refused_with_the_reason() {
    let config = Config::for_tests();
    let (status, body) = put(
        &config,
        vec![endpoint("air", "organization"), layer("stations", "air")],
        "dashboards",
        &dashboard("public", &["stations"]),
    )
    .await;
    // A check that rejects the manifest answers the red verdict that says why (T-2234).
    assert_eq!(status, StatusCode::OK, "{body}");
    let said = findings(&body);
    assert!(
        said.contains("stations reads air whose audience is organization"),
        "{said}"
    );
    assert!(said.contains("UI-19"), "{said}");

    // And the rule is the door's, not the check's: proposing it for real is refused too.
    let (status, body) = propose(
        &config,
        vec![endpoint("air", "organization"), layer("stations", "air")],
        "dashboards",
        &dashboard("public", &["stations"]),
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");
    assert!(body.contains("UI-19"), "{body}");

    // The same dashboard kept inside the project reads whatever it likes.
    let (status, body) = put(
        &config,
        vec![endpoint("air", "organization"), layer("stations", "air")],
        "dashboards",
        &dashboard("project", &["stations"]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_layer_of_a_public_dashboard_cannot_move_to_a_private_endpoint() {
    let config = Config::for_tests();
    let published = live(
        "Dashboard",
        "air",
        dashboard("public", &["stations"])["spec"].clone(),
    );
    let moved = layer("stations", "internal");
    let (status, body) = put(
        &config,
        vec![
            endpoint("air", "public"),
            endpoint("internal", "organization"),
            layer("stations", "air"),
            published.clone(),
        ],
        "layers",
        &manifest(&moved),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(findings(&body).contains("public dashboard air"), "{body}");

    // And the endpoint it reads cannot leave the public audience either.
    let (status, body) = put(
        &config,
        vec![
            endpoint("air", "public"),
            layer("stations", "air"),
            published,
        ],
        "endpoints",
        &manifest(&endpoint("air", "organization")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(findings(&body).contains("public dashboard air"), "{body}");
}
