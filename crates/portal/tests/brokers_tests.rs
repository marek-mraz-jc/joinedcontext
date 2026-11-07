//! T-3990 (PF-116, API/01 §43): the Brokers page lists every broker with its spaces, version,
//! health and load, asked by the Portal, for organization administrators only.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

fn state(broker: &MockServer, token: &std::path::Path, declared: bool) -> AppState {
    let mut config = Config::for_tests();
    config.broker_url = Some(broker.uri());
    config.broker_url_template = Some("http://{broker}.invalid:9".into());
    config.broker_admin_token_file = Some(token.to_path_buf());
    let state = AppState::new(config, None);
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Organization", "RoleBinding"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "org-admins",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "org-admin", "scope": { "organization": "bb" } }),
    ));
    if declared {
        let image = format!("ghcr.io/x/antares@sha256:{}", "a".repeat(64));
        state.mirror.upsert(envelope(
            "ContextBroker",
            "main",
            ORG_NAMESPACE,
            json!({ "default": true, "image": image, "features": ["ogc"] }),
        ));
        state.mirror.upsert(envelope(
            "ContextBroker",
            "canary",
            ORG_NAMESPACE,
            json!({ "image": image }),
        ));
        state.mirror.upsert(envelope(
            "ContextSpace",
            "cars",
            "doprava",
            json!({ "brokerRef": "canary" }),
        ));
    }
    state
        .mirror
        .upsert(envelope("ContextSpace", "air", "ovzdusie", json!({})));
    state
}

async fn broker() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/q/health"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"status": "UP", "version": "0.14.2"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/q/tenants/ovzdusie-air"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"counts": {"entities": 42}})))
        .mount(&server)
        .await;
    server
}

async fn brokers(state: &AppState, who: &str) -> (StatusCode, Value) {
    let answer = send(
        state,
        person(who),
        "GET",
        "/api/v1/organization/brokers",
        None,
    )
    .await;
    let body = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
    (answer.status, body)
}

/// A token file of the test `tag`. The tests run in parallel and each removes its own file, so the
/// names differ by test: with a clock-stamped name two tests can share one file, and when one test
/// removes it the other reads no token and counts 0 entities (T-4816).
fn token(tag: &str) -> std::path::PathBuf {
    let file = std::env::temp_dir().join(format!("brokers-tests-{}-{tag}", std::process::id()));
    std::fs::write(&file, "admin-token").expect("write the token");
    file
}

#[tokio::test]
async fn each_broker_shows_its_spaces_version_health_and_load() {
    let server = broker().await;
    let token = token("each-broker");
    let state = state(&server, &token, true);
    let (status, body) = brokers(&state, "jana").await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(status, StatusCode::OK, "{body}");

    let items = body["items"].as_array().expect("items");
    let names: Vec<&str> = items.iter().filter_map(|b| b["name"].as_str()).collect();
    assert_eq!(names, ["canary", "main"]);
    let (canary, main) = (&items[0], &items[1]);
    assert_eq!(
        (
            &main["health"],
            &main["version"],
            &main["entities"],
            &main["default"]
        ),
        (&json!("up"), &json!("0.14.2"), &json!(42), &json!(true)),
        "{main}"
    );
    assert_eq!(main["spaces"], json!(["ovzdusie/air"]));
    assert_eq!(main["features"], json!(["ogc"]));
    // The canary's address does not resolve: it is down, says why, and adds no load.
    assert_eq!(canary["health"], "down");
    assert_eq!(canary["spaces"], json!(["doprava/cars"]));
    assert_eq!(canary["entities"], 0);
    assert!(
        canary["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{canary}"
    );
    // No address or token is part of the answer.
    let text = body.to_string();
    assert!(
        !text.contains(&server.uri()) && !text.contains("admin-token"),
        "{text}"
    );
}

#[tokio::test]
async fn an_organization_without_brokers_shows_the_implicit_main() {
    let server = broker().await;
    let token = token("implicit-main");
    let state = state(&server, &token, false);
    let (status, body) = brokers(&state, "jana").await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().map(Vec::len), Some(1), "{body}");
    let main = &body["items"][0];
    assert_eq!(
        (
            &main["name"],
            &main["declared"],
            &main["health"],
            &main["entities"]
        ),
        (&json!("main"), &json!(false), &json!("up"), &json!(42)),
        "{main}"
    );
}

#[tokio::test]
async fn only_an_administrator_sees_the_brokers() {
    let server = broker().await;
    let token = token("administrator-only");
    let state = state(&server, &token, true);
    let (status, body) = brokers(&state, "peter").await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}
