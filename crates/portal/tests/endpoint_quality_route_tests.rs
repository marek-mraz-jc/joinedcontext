//! `GET …/endpoints/{name}/quality` (T-5112) answers only for an Endpoint whose Context Space the
//! caller's grants reach: a binding scoped to one space reads no score of another space's
//! Endpoint, and learns nothing of it, the 404 any unknown Endpoint gets.

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

#[tokio::test]
async fn a_reader_of_one_space_reads_no_score_of_another_spaces_endpoint() {
    let state = AppState::new(Config::for_tests(), None);
    for (space, endpoint) in [("air", "air-api"), ("water", "water-api")] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, "city", json!({})));
        state.mirror.upsert(envelope(
            "Endpoint",
            endpoint,
            "city",
            json!({ "contextSpaceRef": space, "slug": endpoint, "audience": "project-list" }),
        ));
    }
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint", "Project"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "space-reader",
                "scope": { "contextSpace": "air" } }),
    ));
    let own = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/city/endpoints/air-api/quality",
        None,
    )
    .await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text);
    let other = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/city/endpoints/water-api/quality",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
}
