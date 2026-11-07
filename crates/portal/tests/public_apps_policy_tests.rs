//! T-2870 (PF-100, PF-103): an organization that refuses public Apps is held at the door. A
//! Change that makes an App public is refused before it exists, naming the setting and where it
//! changes; an App kept to its project is proposed as before, and so is everything while the
//! organization allows public Apps.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{checked_send, envelope, forge, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

fn state(gitea: &wiremock::MockServer, public: &str) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({
            "domain": "hel.fi",
            "locales": ["en"],
            "defaultLocale": "en",
            "policies": { "apps": { "public": public } },
        }),
    ));
    // The space the App's data need names, which a manifest must find in its project (MF-13).
    state.mirror.upsert(envelope(
        "ContextSpace",
        "ovzdusie",
        "ovzdusie",
        json!({ "dataModelRef": "air" }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "app-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-app-editor",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "app-editor",
            "scope": { "project": "ovzdusie" },
        }),
    ));
    state
}

fn app(visibility: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": { "name": "air-map", "namespace": "ovzdusie" },
        "spec": {
            "kind": "static",
            "source": { "path": "apps/air-map" },
            "build": { "node": "22" },
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                "types": ["AirQualityObserved"],
                "operations": ["queryEntity"],
            }],
            "visibility": visibility,
            "lifecycle": "draft",
        },
    })
}

async fn propose(state: &AppState, visibility: &str) -> common::Answer {
    checked_send(
        state,
        person("jana"),
        "POST",
        "/api/v1/projects/ovzdusie/apps",
        Some(app(visibility)),
    )
    .await
}

#[tokio::test]
async fn a_public_app_is_refused_before_a_change_exists_while_the_organization_refuses_them() {
    let gitea = forge().await;
    let state = state(&gitea, "refused");
    let answer = propose(&state, "public").await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    for part in [
        "App 'air-map' may not be public",
        "spec.policies.apps.public",
        "Organization settings",
    ] {
        assert!(answer.text.contains(part), "{part}: {}", answer.text);
    }
}

#[tokio::test]
async fn an_app_kept_to_its_project_is_proposed_while_public_apps_are_refused() {
    let gitea = forge().await;
    let state = state(&gitea, "refused");
    let answer = propose(&state, "project").await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
}

#[tokio::test]
async fn a_public_app_is_proposed_while_the_organization_allows_them() {
    let gitea = forge().await;
    let state = state(&gitea, "allowed");
    let answer = propose(&state, "public").await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
}
