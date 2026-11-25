//! The open changes that create a missing reference: a forge that fails is an error, no forge is
//! the plain refusal, and nothing missing asks nobody (T-7823).

mod common;

use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::references::{awaited, Missing};
use joinedcontext_portal::state::AppState;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn missing() -> Vec<Missing> {
    vec![Missing {
        path: "spec.contextSpaceRef".to_owned(),
        kind: "ContextSpace".to_owned(),
        name: "air".to_owned(),
    }]
}

#[tokio::test]
async fn a_forge_that_fails_to_list_open_changes_is_an_error_not_a_refusal_of_the_reference() {
    let shako = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&shako)
        .await;
    let state = common::state_on(&shako);
    let failed = awaited(&state, "helsinki", missing()).await;
    assert!(failed.is_err());
}

#[tokio::test]
async fn nothing_missing_asks_the_forge_nothing() {
    let state = AppState::new(Config::for_tests(), None);
    assert!(awaited(&state, "helsinki", Vec::new())
        .await
        .expect("nothing to wait on")
        .is_empty());
}

#[tokio::test]
async fn without_a_forge_a_missing_reference_is_the_plain_refusal() {
    let state = AppState::new(Config::for_tests(), None);
    let refused = awaited(&state, "helsinki", missing()).await;
    assert!(
        matches!(refused, Err(ApiError::Invalid { .. })),
        "{refused:?}"
    );
}
