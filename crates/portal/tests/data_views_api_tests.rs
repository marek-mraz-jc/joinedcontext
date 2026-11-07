//! Saved data views of a space (API/01 §30, ADR-N-042 §3.2, T-3104): reading the space is all a
//! view asks, and who sees, changes and deletes one is its mode's to say.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const VIEWS: &str = "/api/v1/projects/helsinki/spaces/bikes/views";

/// The space `bikes` (and `trams`): jana and eva read bikes, sami stewards bikes, tove stewards
/// trams and only reads bikes, and nobody else is bound.
fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    for space in ["bikes", "trams"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, PROJECT, json!({})));
    }
    let role = |name: &str, verbs: Value| {
        state.mirror.upsert(envelope(
            "Role",
            name,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": verbs }] }),
        ));
    };
    role("space-reader", json!(["read"]));
    role("space-steward", json!(["read", "update"]));
    let bind = |name: &str, users: &[&str], role: &str, space: &str| {
        let subjects: Vec<Value> = users
            .iter()
            .map(|u| json!({ "user": format!("{u}@hel.fi") }))
            .collect();
        state.mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": subjects, "role": role, "scope": { "contextSpace": space } }),
        ));
    };
    bind(
        "bikes-readers",
        &["jana", "eva", "tove"],
        "space-reader",
        "bikes",
    );
    bind("bikes-stewards", &["sami"], "space-steward", "bikes");
    bind("trams-stewards", &["tove"], "space-steward", "trams");
    state
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

async fn save(state: &AppState, who: &str, mode: &str) -> Value {
    let answer = send(
        state,
        person(who),
        "POST",
        VIEWS,
        Some(json!({ "type": "BikeHireDockingStation", "kind": "grid", "mode": mode, "title": format!("{who}'s {mode}"),
                     "config": { "q": "availableBikeNumber<3", "hidden": ["dateLastReported"] } })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.text);
    json_of(&answer.text)
}

fn titles(text: &str) -> Vec<String> {
    json_of(text)["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v["title"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `who` finds `view` in the space's list, the one read of views (T-4453 removed the
/// read of one).
async fn sees(state: &AppState, who: &str, view: &Value) -> bool {
    let list = send(state, person(who), "GET", VIEWS, None).await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text);
    json_of(&list.text)["items"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["id"] == view["id"]))
}

fn change(view: &Value, mode: &str, title: &str) -> Value {
    json!({ "kind": "grid", "mode": mode, "title": title, "config": view["config"], "expectedVersion": view["version"] })
}

#[tokio::test]
async fn a_personal_view_is_its_owners_alone() {
    let state = world();
    let view = save(&state, "jana", "personal").await;
    assert_eq!(view["owner"], "jana");
    assert!(
        view.get("ownerSubject").is_none(),
        "the owner's subject is never answered"
    );
    let one = format!("{VIEWS}/{}", view["id"].as_str().expect("an id"));

    let mine = send(&state, person("jana"), "GET", VIEWS, None).await;
    assert_eq!(titles(&mine.text), vec!["jana's personal"]);
    let theirs = send(&state, person("eva"), "GET", VIEWS, None).await;
    assert_eq!(theirs.status, StatusCode::OK);
    assert!(titles(&theirs.text).is_empty());
    // Not even a steward sees it, and to them it is not there at all.
    for who in ["eva", "sami"] {
        assert!(
            !sees(&state, who, &view).await,
            "{who} sees jana's personal view"
        );
        assert_eq!(
            send(&state, person(who), "DELETE", &one, None).await.status,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn a_collaborative_view_is_changed_by_everyone_and_governed_by_its_owner_or_a_steward() {
    let state = world();
    let view = save(&state, "jana", "collaborative").await;
    let one = format!("{VIEWS}/{}", view["id"].as_str().expect("an id"));

    let renamed = send(
        &state,
        person("eva"),
        "PUT",
        &one,
        Some(change(&view, "collaborative", "Renamed by eva")),
    )
    .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text);
    let renamed = json_of(&renamed.text);
    assert_eq!(
        (renamed["title"].as_str(), renamed["version"].as_i64()),
        (Some("Renamed by eva"), Some(2))
    );

    // Eva may not take it from everyone by making it personal, nor delete it.
    let taken = send(
        &state,
        person("eva"),
        "PUT",
        &one,
        Some(change(&renamed, "personal", "mine now")),
    )
    .await;
    assert_eq!(taken.status, StatusCode::FORBIDDEN, "{}", taken.text);
    assert_eq!(
        send(&state, person("eva"), "DELETE", &one, None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    // The window that read version 1 does not overwrite version 2.
    let stale = send(
        &state,
        person("jana"),
        "PUT",
        &one,
        Some(change(&view, "collaborative", "stale")),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.text);
    assert!(stale.text.contains("version 2"), "{}", stale.text);

    assert_eq!(
        send(&state, person("sami"), "DELETE", &one, None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(
        !sees(&state, "jana", &view).await,
        "the deleted view is still listed"
    );
}

#[tokio::test]
async fn a_locked_view_is_changed_only_by_its_owner_or_a_steward_of_that_space() {
    let state = world();
    let view = save(&state, "jana", "locked").await;
    let one = format!("{VIEWS}/{}", view["id"].as_str().expect("an id"));

    assert!(
        sees(&state, "eva", &view).await,
        "a locked view is everyone's to read"
    );
    let refused = send(
        &state,
        person("eva"),
        "PUT",
        &one,
        Some(change(&view, "locked", "eva's")),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    // A steward of another space is a reader here.
    let elsewhere = send(
        &state,
        person("tove"),
        "PUT",
        &one,
        Some(change(&view, "locked", "tove's")),
    )
    .await;
    assert_eq!(
        elsewhere.status,
        StatusCode::FORBIDDEN,
        "{}",
        elsewhere.text
    );
    let steward = send(
        &state,
        person("sami"),
        "PUT",
        &one,
        Some(change(&view, "locked", "sami's")),
    )
    .await;
    assert_eq!(steward.status, StatusCode::OK, "{}", steward.text);
}

#[tokio::test]
async fn refuses_what_is_wrong_and_hides_what_is_not_theirs() {
    let state = world();
    let unknown = send(
        &state,
        person("jana"),
        "POST",
        VIEWS,
        Some(json!({ "type": "Station", "kind": "grid", "mode": "personal", "title": "x", "owner": "eva" })),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST, "{}", unknown.text);
    let bad_mode = send(
        &state,
        person("jana"),
        "POST",
        VIEWS,
        Some(json!({ "type": "Station", "kind": "grid", "mode": "public", "title": "x" })),
    )
    .await;
    assert_eq!(bad_mode.status, StatusCode::BAD_REQUEST);
    assert!(bad_mode.text.contains("mode 'public'"), "{}", bad_mode.text);

    assert_eq!(
        send(
            &state,
            person("jana"),
            "DELETE",
            &format!("{VIEWS}/not-an-id"),
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    // No binding, or a space the reader is not bound to: as if it were not there.
    assert_eq!(
        send(&state, person("nobody"), "GET", VIEWS, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let trams = "/api/v1/projects/helsinki/spaces/trams/views";
    assert_eq!(
        send(&state, person("jana"), "GET", trams, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &state,
            person("jana"),
            "GET",
            "/api/v1/projects/helsinki/spaces/nowhere/views",
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}
