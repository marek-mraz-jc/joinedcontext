//! `jc_model_infer` from an address (T-3250): fetched on the project's runner, under the grant a
//! data source's Check needs, and refused before anything runs when it is no http(s) address.

mod common;

use axum::http::StatusCode;
use common::doors::{self, session, steward, viewer};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;
use serde_json::json;

fn state() -> AppState {
    doors::state_with(AppState::new(Config::for_tests(), None))
}

#[tokio::test]
async fn an_address_that_is_not_http_is_refused_at_the_field() {
    let state = state();
    for url in [
        "ftp://data.example.org/a.json",
        "file:///etc/passwd",
        "not an address",
    ] {
        let answer = doors::call(
            "jc_model_infer",
            &session(steward()),
            &state,
            json!({ "url": url }),
        )
        .await;
        assert_eq!(
            StatusCode::UNPROCESSABLE_ENTITY,
            answer.status,
            "{url}: {}",
            answer.text()
        );
        assert!(answer.text().contains("url"), "{}", answer.text());
    }
}

#[tokio::test]
async fn fetching_an_address_needs_the_grant_a_data_source_check_needs() {
    // The viewer reads every kind of the project, so inferring from a file is theirs; fetching an
    // address on the project's runner is a data source's Check, which needs propose.
    let answer = doors::call(
        "jc_model_infer",
        &session(viewer()),
        &state(),
        json!({ "url": "https://data.example.org/a.json" }),
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, answer.status, "{}", answer.text());
    assert!(answer.text().contains("DataSource"), "{}", answer.text());
}

#[tokio::test]
async fn no_runner_to_fetch_with_is_unavailable_not_the_address_s_fault() {
    let answer = doors::call(
        "jc_model_infer",
        &session(steward()),
        &state(),
        json!({ "url": "https://data.example.org/a.json" }),
    )
    .await;
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        answer.status,
        "{}",
        answer.text()
    );
}

#[tokio::test]
async fn an_address_carrying_a_credential_is_refused_without_repeating_it() {
    let state = state();
    for url in [
        "https://user:hunter2hunter2@data.example.org/a.json",
        "https://user@data.example.org/a.json",
        "https://data.example.org/a.json?api_key=hunter2hunter2",
        "https://data.example.org/a.json?access-token=hunter2hunter2",
        "https://data.example.org/a.json?KEY=hunter2hunter2",
        "https://data.example.org/a.json?q=Bearer%20hunter2hunter2",
    ] {
        let answer = doors::call(
            "jc_model_infer",
            &session(steward()),
            &state,
            json!({ "url": url }),
        )
        .await;
        assert_eq!(
            StatusCode::UNPROCESSABLE_ENTITY,
            answer.status,
            "{url}: {}",
            answer.text()
        );
        assert!(!answer.text().contains("hunter2"), "{}", answer.text());
    }
    // An ordinary query stays an ordinary query: it reaches the runner, which is absent here.
    let plain = doors::call(
        "jc_model_infer",
        &session(steward()),
        &state,
        json!({ "url": "https://data.example.org/a.json?type=AirQualityObserved&limit=10" }),
    )
    .await;
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        plain.status,
        "{}",
        plain.text()
    );
}

/// JC1-27, AG-65: an address a caller types is fetched by the runner inside the cluster before
/// anybody approved it, so a loopback, private or cluster address is refused at its field first,
/// as a planned DataSource's is; a public one still reaches the runner (absent here).
#[tokio::test]
async fn an_address_inside_the_cluster_is_refused_before_the_runner_fetches_it() {
    let state = state();
    for url in [
        "http://127.0.0.1:8080/a.json",
        "http://localhost/a.json",
        "http://10.43.0.12/a.json",
        "http://portal.portal.svc.cluster.local/api/v1/version",
        "http://gitea:3000/a.json",
    ] {
        for (name, input, field) in [
            ("jc_model_infer", json!({ "url": url }), "url"),
            (
                "jc_pipeline_sample_source",
                json!({ "sample": { "url": url, "format": "json" } }),
                "sample.url",
            ),
        ] {
            let answer = doors::call(name, &session(steward()), &state, input).await;
            assert_eq!(
                StatusCode::UNPROCESSABLE_ENTITY,
                answer.status,
                "{name} {url}: {}",
                answer.text()
            );
            assert_eq!(
                field,
                answer.body["errors"][0]["path"],
                "{name} {url}: {}",
                answer.text()
            );
        }
    }
}
