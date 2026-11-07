//! `POST /internal/model-key` (AG-96, T-3065): the agent proxy's report of the model key's state
//! reaches the activity feed of the project `org` when the key stops working and when it works
//! again, only from the proxy, and only in the documented shape.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

mod common;

fn config() -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some("http://jc-agent-proxy.agents.svc.cluster.local:8080"),
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_AGENT_PROXY_CLIENT_ID" => Some(common::AGENT_PROXY_CLIENT),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

async fn listener() -> (AppState, axum::Router) {
    let state = AppState::new(config(), None);
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    (state.clone(), server::internal_app(state))
}

async fn post(app: &axum::Router, bearer: Option<&str>, body: &str) -> StatusCode {
    let mut request = Request::builder()
        .method("POST")
        .uri("/internal/model-key")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.to_owned()))
                .expect("a request"),
        )
        .await
        .expect("an answer")
        .status()
}

async fn org_events(state: &AppState) -> Vec<(String, String)> {
    let filter = ActivityFilter {
        kinds: vec!["model.key".to_owned()],
        limit: 50,
        ..ActivityFilter::default()
    };
    let mut events: Vec<(String, String)> = state
        .activity
        .list("org", &filter)
        .await
        .expect("the feed")
        .items
        .into_iter()
        .map(|event| (event.severity, event.summary))
        .collect();
    events.reverse();
    events
}

#[tokio::test]
async fn the_keys_failure_and_recovery_reach_the_org_feed_once_each() {
    let (state, app) = listener().await;
    let proxy = common::REALM.workload(common::AGENT_PROXY_CLIENT);

    let valid = r#"{"state":"valid","source":"probe","limit":10.0,"usage":2.0,"remaining":8.0}"#;
    assert_eq!(
        post(&app, Some(&proxy), valid).await,
        StatusCode::NO_CONTENT
    );
    let dead = r#"{"state":"invalid","source":"call"}"#;
    assert_eq!(post(&app, Some(&proxy), dead).await, StatusCode::NO_CONTENT);
    assert_eq!(post(&app, Some(&proxy), dead).await, StatusCode::NO_CONTENT);
    // A probe that did not reach the provider says nothing about the key.
    let unreachable = r#"{"state":"unreachable","source":"probe"}"#;
    assert_eq!(
        post(&app, Some(&proxy), unreachable).await,
        StatusCode::NO_CONTENT
    );
    let low = r#"{"state":"valid","source":"probe","limit":10.0,"usage":8.5,"remaining":1.5}"#;
    assert_eq!(post(&app, Some(&proxy), low).await, StatusCode::NO_CONTENT);

    let events = org_events(&state).await;
    let severities: Vec<&str> = events.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(severities, ["error", "info"], "{events:?}");
    assert!(
        events[0].1.contains("refuses the Portal's key"),
        "{events:?}"
    );
    assert_eq!(events[1].1, "The model key works again.");
}

#[tokio::test]
async fn only_the_proxy_reports_and_only_the_documented_shape() {
    let (state, app) = listener().await;
    let valid = r#"{"state":"invalid","source":"call"}"#;
    assert_eq!(post(&app, None, valid).await, StatusCode::UNAUTHORIZED);
    let someone = common::REALM.workload("helsinki-pipeline-proposer");
    assert_ne!(
        post(&app, Some(&someone), valid).await,
        StatusCode::NO_CONTENT
    );

    let proxy = common::REALM.workload(common::AGENT_PROXY_CLIENT);
    let with_key = r#"{"state":"valid","source":"probe","key":"sk-or-v1-0123"}"#;
    assert_eq!(
        post(&app, Some(&proxy), with_key).await,
        StatusCode::BAD_REQUEST
    );
    let negative = r#"{"state":"valid","source":"probe","remaining":-3}"#;
    assert_eq!(
        post(&app, Some(&proxy), negative).await,
        StatusCode::BAD_REQUEST
    );
    assert!(org_events(&state).await.is_empty());
}
