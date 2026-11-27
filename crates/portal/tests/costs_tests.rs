//! T-5126: Administration → Costs splits a month per project from the broker's bytes and the
//! assistant's spend, for organization administrators only.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const GIB: u64 = 1024 * 1024 * 1024;

async fn broker() -> MockServer {
    let server = MockServer::start().await;
    for (tenant, bytes) in [("ovzdusie-air", 3 * GIB), ("doprava-cars", GIB)] {
        Mock::given(method("GET"))
            .and(path(format!("/q/tenants/{tenant}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "counts": {"entities": 42},
                "bytes": {"entities": bytes / 2, "attrInstances": bytes / 2, "byType": {}},
            })))
            .mount(&server)
            .await;
    }
    server
}

fn state(broker: &MockServer, token: &std::path::Path, priced: bool) -> AppState {
    let mut config = Config::for_tests();
    config.broker_url = Some(broker.uri());
    config.broker_admin_token_file = Some(token.to_path_buf());
    config.cost_prices = priced.then(|| {
        serde_json::from_value(json!({
            "currency": "EUR", "perUsd": 0.9,
            "nodes": [{"name": "cpx42 nbg1", "monthly": 30.0}],
            "fixed": [{"name": "ipv4", "monthly": 10.0}],
            "volumes": [{"name": "pg", "gigabytes": 10.0, "monthlyPerGigabyte": 0.5}],
        }))
        .expect("the table parses")
    });
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
    state
        .mirror
        .upsert(envelope("ContextSpace", "air", "ovzdusie", json!({})));
    state
        .mirror
        .upsert(envelope("ContextSpace", "cars", "doprava", json!({})));
    state.mirror.upsert(envelope(
        "AssistantDeployment",
        "help",
        "doprava",
        json!({}),
    ));
    state
}

fn token(tag: &str) -> std::path::PathBuf {
    let file = std::env::temp_dir().join(format!("costs-tests-{}-{tag}", std::process::id()));
    std::fs::write(&file, "admin-token").expect("write the token");
    file
}

async fn get(state: &AppState, who: &str, url: &str) -> (StatusCode, String) {
    let answer = send(state, person(who), "GET", url, None).await;
    (answer.status, answer.text)
}

#[tokio::test]
async fn an_administrator_reads_each_projects_parts_adding_up_to_the_table() {
    let server = broker().await;
    let token = token("parts");
    let state = state(&server, &token, true);
    let (status, text) = get(&state, "jana", "/api/v1/organization/costs?month=2026-09").await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(status, StatusCode::OK, "{text}");
    let body: Value = serde_json::from_str(&text).expect("json");
    assert_eq!(body["month"], "2026-09");
    assert_eq!(body["currency"], "EUR");
    assert_eq!(body["infrastructure"], 45.0);
    let projects = body["projects"].as_array().expect("projects");
    let names: Vec<&str> = projects
        .iter()
        .filter_map(|p| p["project"].as_str())
        .collect();
    assert_eq!(names, ["doprava", "ovzdusie"]);
    // 1 GiB of 4 measured: a quarter of the 40 node and fixed; 1 GiB at 0.5.
    assert_eq!(
        (
            projects[0]["node"].as_f64(),
            projects[0]["storage"].as_f64()
        ),
        (Some(10.0), Some(0.5))
    );
    assert_eq!(
        (
            projects[1]["node"].as_f64(),
            projects[1]["storage"].as_f64()
        ),
        (Some(30.0), Some(1.5))
    );
    assert_eq!(projects[1]["spaces"][0]["bytes"], 3 * GIB);
    // No assistant is reachable in the test: the line stays with its reason, never a made-up 0.
    let help = &projects[0]["assistants"][0];
    assert_eq!(help["assistant"], "help");
    assert!(help["costUsd"].is_null(), "{help}");
    assert!(
        help["reason"]
            .as_str()
            .is_some_and(|r| r.contains("JC_PORTAL_KNOWLEDGE_URL")),
        "{help}"
    );
    assert_eq!(body["platform"]["storage"], 3.0);
    assert!(!text.contains("admin-token"), "no token in the answer");
}

/// A realm that hands the Portal a token of its own, so the assistant can be asked.
async fn realm() -> MockServer {
    let realm = MockServer::start().await;
    let issuer = format!("{}/realms/helsinki", realm.uri());
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/protocol/openid-connect/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&realm)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/helsinki/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-portals-own-token", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(&realm)
        .await;
    realm
}

/// With an assistant to ask: an answered month is priced, a refused one keeps its reason; a
/// space the broker does not count keeps its own, and each project lists its lines by name.
#[tokio::test]
async fn an_assistants_answered_month_is_priced_and_each_refusal_keeps_its_reason() {
    use joinedcontext_portal::auth::client_auth::ClientAuth;
    use joinedcontext_portal::auth::oidc::OidcClient;
    use joinedcontext_portal::config::OidcConfig;
    use wiremock::matchers::query_param;

    let server = broker().await;
    let token = token("assistant");
    let base = state(&server, &token, true);
    for (kind, name) in [("ContextSpace", "bikes"), ("AssistantDeployment", "desk")] {
        base.mirror
            .upsert(envelope(kind, name, "doprava", json!({})));
    }
    let realm = realm().await;
    let assistant = MockServer::start().await;
    let usage = "/internal/v1/projects/doprava/knowledge/conversations/usage";
    Mock::given(method("GET"))
        .and(path(usage))
        .and(query_param("deployment", "help"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "questions": 4, "uncostedQuestions": 1, "costUsd": { "total": 2.0 }
        })))
        .mount(&assistant)
        .await;
    Mock::given(method("GET"))
        .and(path(usage))
        .and(query_param("deployment", "desk"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&assistant)
        .await;
    let mut config = (*base.config).clone();
    config.knowledge_url = Some(assistant.uri());
    let oidc = OidcClient::discover(
        &OidcConfig {
            issuer: format!("{}/realms/helsinki", realm.uri())
                .parse()
                .expect("an issuer"),
            client_id: "portal-api".into(),
            auth: ClientAuth::Secret("test-secret".into()),
            extra_ca_pem: None,
            admin_url: None,
        },
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    let state = AppState::new(config, Some(oidc)).with_mirror(std::sync::Arc::clone(&base.mirror));
    let (status, text) = get(&state, "jana", "/api/v1/organization/costs?month=2026-09").await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(status, StatusCode::OK, "{text}");
    let body: Value = serde_json::from_str(&text).expect("json");
    let doprava = &body["projects"][0];
    assert_eq!(doprava["project"], "doprava", "{body}");
    let spaces: Vec<&str> = doprava["spaces"]
        .as_array()
        .expect("spaces")
        .iter()
        .filter_map(|s| s["space"].as_str())
        .collect();
    assert_eq!(spaces, ["bikes", "cars"]);
    let bikes = &doprava["spaces"][0];
    assert!(bikes["bytes"].is_null(), "{bikes}");
    assert!(
        bikes["reason"].as_str().is_some_and(|r| r.contains("404")),
        "{bikes}"
    );
    let assistants = doprava["assistants"].as_array().expect("assistants");
    assert_eq!(assistants.len(), 2, "{doprava}");
    let (desk, help) = (&assistants[0], &assistants[1]);
    assert_eq!(
        (&desk["assistant"], &help["assistant"]),
        (&json!("desk"), &json!("help"))
    );
    assert_eq!(help["costUsd"], 2.0, "{help}");
    assert_eq!(
        (&help["questions"], &help["uncostedQuestions"]),
        (&json!(4), &json!(1))
    );
    assert!(help.get("reason").is_none(), "{help}");
    assert!(desk["costUsd"].is_null(), "{desk}");
    assert!(
        desk["reason"].as_str().is_some_and(|r| r.contains("500")),
        "{desk}"
    );
    assert!(
        !text.contains("the-portals-own-token"),
        "no token in the answer"
    );
}

#[tokio::test]
async fn the_export_is_a_csv_download_of_the_same_month() {
    let server = broker().await;
    let token = token("export");
    let state = state(&server, &token, true);
    let answer = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/organization/costs/export?month=2026-09",
        None,
    )
    .await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let lines: Vec<&str> = answer.text.lines().collect();
    assert_eq!(lines[0], "month,project,part,item,bytes,cost,currency");
    assert!(
        lines.contains(&"2026-09,doprava,storage,cars,1073741824,0.50,EUR"),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"2026-09,,platform-storage,,,3.00,EUR"),
        "{lines:?}"
    );
}

#[tokio::test]
async fn only_an_administrator_reads_the_costs() {
    let server = broker().await;
    let token = token("denied");
    let state = state(&server, &token, true);
    for url in [
        "/api/v1/organization/costs",
        "/api/v1/organization/costs/export",
    ] {
        let (status, text) = get(&state, "eva", url).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{url}: {text}");
        assert!(text.contains("organization administrators"), "{text}");
    }
    let _ = std::fs::remove_file(&token);
}

#[tokio::test]
async fn a_bad_month_or_no_price_table_says_what_to_do() {
    let server = broker().await;
    let token = token("refusals");
    let priced = state(&server, &token, true);
    for month in ["2026-13", "sept", "2999-01"] {
        let url = format!("/api/v1/organization/costs?month={month}");
        let (status, text) = get(&priced, "jana", &url).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{month}: {text}");
        assert!(text.contains("month"), "{text}");
    }
    let (status, text) = get(&priced, "jana", "/api/v1/organization/costs?months=2026-09").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unknown parameter: {text}"
    );
    let unpriced = state(&server, &token, false);
    let (status, text) = get(&unpriced, "jana", "/api/v1/organization/costs").await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("portal.costPrices"), "{text}");
}
