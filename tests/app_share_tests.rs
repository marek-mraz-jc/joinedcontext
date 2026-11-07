//! Sharing an App (AP-173, AP-174, T-4913): the Share dialog's audience counts and the Portal
//! home's "Shared with me", which lists only Apps of other projects the caller may open.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

fn app(visibility: &str, lifecycle: &str) -> Value {
    json!({
        "kind": "static",
        "source": { "path": "./src" },
        "build": {},
        "visibility": visibility,
        "lifecycle": lifecycle,
        "dataNeeds": [],
        "roles": [{ "name": "inspector", "title": { "en": "Inspector" } }],
        "access": [
            { "role": "inspector", "subjects": [{ "group": "inspectors" }, { "user": "Jana@hel.fi" }] },
        ],
    })
}

/// `vera` in helsinki, `jana` and `piotr` in espoo, `otto` in no project; espoo holds an App of
/// every visibility and one organization App still in preview.
fn state() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    common::bind(&state.mirror, "vera", "helsinki");
    common::bind(&state.mirror, "jana", "espoo");
    common::bind(&state.mirror, "piotr", "espoo");
    state.mirror.upsert(envelope(
        "Group",
        "inspectors",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "jana@hel.fi" }, { "user": "vera@hel.fi" }] }),
    ));
    for (name, visibility, lifecycle) in [
        ("air-map", "organization", "published"),
        ("team-desk", "project", "published"),
        ("my-notes", "private", "published"),
        ("inspections", "roles", "published"),
        ("open-map", "public", "published"),
        ("next-map", "organization", "preview"),
    ] {
        state
            .mirror
            .upsert(envelope("App", name, "espoo", app(visibility, lifecycle)));
    }
    state.mirror.upsert(envelope(
        "App",
        "own-map",
        "helsinki",
        app("organization", "published"),
    ));
    state
}

fn names(text: &str) -> Vec<(String, String)> {
    let list: Vec<Value> = serde_json::from_str(text).expect("a JSON list");
    list.iter()
        .map(|app| {
            (
                app["project"].as_str().unwrap_or_default().to_owned(),
                app["name"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn shared_with_me_lists_only_organization_apps_of_other_projects_the_person_may_open() {
    let state = state();
    let answer = send(&state, person("vera"), "GET", "/api/v1/shared-apps", None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(
        names(&answer.text),
        vec![("espoo".to_owned(), "air-map".to_owned())]
    );
    // Nothing in the answer names an App the person may not open.
    for hidden in [
        "team-desk",
        "my-notes",
        "inspections",
        "next-map",
        "own-map",
    ] {
        assert!(!answer.text.contains(hidden), "{hidden}: {}", answer.text);
    }
}

#[tokio::test]
async fn a_person_outside_the_organization_gets_an_empty_list_and_an_anonymous_caller_401() {
    let state = state();
    let answer = send(&state, person("otto"), "GET", "/api/v1/shared-apps", None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(names(&answer.text).is_empty(), "{}", answer.text);

    let response = tower::ServiceExt::oneshot(
        joinedcontext_portal::server::app(state),
        axum::http::Request::get("/api/v1/shared-apps")
            .body(axum::body::Body::empty())
            .expect("a request"),
    )
    .await
    .expect("a response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_audience_counts_the_project_the_organization_and_each_role() {
    let state = state();
    let uri = "/api/v1/projects/espoo/apps/inspections/audience";
    let answer = send(&state, person("jana"), "GET", uri, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let audience: Value = serde_json::from_str(&answer.text).expect("JSON");
    assert_eq!(audience["project"], 2, "jana and piotr: {audience}");
    assert_eq!(audience["organization"], 3, "vera, jana, piotr: {audience}");
    // jana named twice, directly and through the group, counts once.
    assert_eq!(
        audience["roles"],
        json!([{ "name": "inspector", "people": 2 }])
    );
    assert_eq!(audience["publicAllowed"], true);
}

#[tokio::test]
async fn the_audience_of_an_app_the_person_may_not_read_is_not_found_like_a_missing_one() {
    let state = state();
    let other = send(
        &state,
        person("vera"),
        "GET",
        "/api/v1/projects/espoo/apps/air-map/audience",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
    let missing = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/espoo/apps/nothing/audience",
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text);
}
