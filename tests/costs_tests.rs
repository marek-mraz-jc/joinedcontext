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
