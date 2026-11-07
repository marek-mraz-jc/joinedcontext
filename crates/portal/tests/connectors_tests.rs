//! Personal MCP connectors (UI-98, EP-123, T-4531, T-5970): one store, the connector record of
//! API/14; the old list in a person's preferences moves into it once, read-only and never wider,
//! and the internal listener hands a person's data-reading connectors to the gateway's client
//! alone.
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; skips
//! with a note otherwise (see preferences_db_tests.rs).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

mod common;

const GATEWAY_CLIENT: &str = "context-gateway";

fn database_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

async fn state(url: &str) -> AppState {
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_GATEWAY_CLIENT_ID" => Some(GATEWAY_CLIENT),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let pool = joinedcontext_portal::db::connect(url)
        .await
        .expect("connect + migrate");
    let state = AppState::new(config, None).with_db(pool);
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    for (name, project, audience) in [
        ("air", "mesto", "public"),
        ("inner", "mesto", "organization"),
        // Another project's Endpoint none of this suite's people reads (T-6362).
        ("secret", "tajne", "organization"),
    ] {
        state.mirror.upsert(common::envelope(
            "Endpoint",
            name,
            project,
            json!({ "contextSpaceRef": project, "slug": slug(name), "audience": audience }),
        ));
    }
    let org = joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(common::envelope(
        "Role",
        "endpoint-readers",
        org,
        json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace"], "verbs": ["read"] }] }),
    ));
    state
}

/// Binds `{who}@hel.fi` to reading Endpoints in project `mesto`.
fn reader_in_mesto(state: &AppState, who: &str) {
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("{who}-reads-mesto"),
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": "endpoint-readers",
                "scope": { "project": "mesto" } }),
    ));
}

/// The signed-in read of preferences that moves `who`'s old connectors.
async fn sign_in(state: &AppState, who: &str) -> common::Answer {
    common::send(
        state,
        common::person(who),
        "GET",
        "/api/v1/preferences",
        None,
    )
    .await
}

/// A person no other run of this suite uses, so runs share one database.
fn fresh(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{tag}{nanos}")
}

/// A 26-character slug of base32 letters.
fn slug(name: &str) -> String {
    name.chars().cycle().take(26).collect()
}

async fn internal(state: &AppState, sub: &str, bearer: &str) -> (StatusCode, Value) {
    let response = server::internal_app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/internal/connectors/{sub}"))
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn old_connectors_move_once_read_only_and_reach_the_gateway_alone() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let state = state(&url).await;
    let (a, b) = (fresh("a"), fresh("b"));
    let (sub_a, sub_b) = (format!("sub-{a}"), format!("sub-{b}"));
    let pool = state.db.clone().expect("a pool");
    joinedcontext_portal::db::save_preferences(
        &pool,
        &sub_a,
        &json!({
            "theme": "dark",
            "connectors": [
                { "name": "fav", "select": { "endpoints": [slug("air")] } },
                { "name": "whole", "select": {} },
                { "name": "nothing", "select": { "themes": ["AGRI"] } },
                { "name": "hidden", "select": { "endpoints": [slug("secret")] } },
            ],
        }),
    )
    .await
    .expect("an old row");
    reader_in_mesto(&state, &a);

    let gateway = common::REALM.workload(GATEWAY_CLIENT);
    let (status, body) = internal(&state, &sub_a, &gateway).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "connectors": [] })),
        "the gateway's listener knows no groups, so it moves nothing"
    );
    let prefs = sign_in(&state, &a).await;
    assert_eq!(prefs.status, StatusCode::OK, "{}", prefs.text);
    let (status, body) = internal(&state, &sub_a, &gateway).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut moved: Vec<Value> = body["connectors"].as_array().expect("a list").clone();
    moved.sort_by_key(|c| c["name"].as_str().map(str::to_owned));
    assert_eq!(
        moved.len(),
        2,
        "a selector taking nothing today moves nowhere: {body}"
    );
    assert_eq!(moved[0]["name"], "fav");
    assert_eq!(
        moved[0]["scope"],
        json!([{ "project": "mesto", "endpoint": "air", "access": "read" }]),
        "a favourite stays that one Endpoint, read-only"
    );
    assert_eq!(moved[1]["name"], "whole");
    assert_eq!(
        moved[1]["scope"],
        json!([{ "project": "mesto", "access": "read" }]),
        "an empty selector reaches only the project its owner reads, never tajne"
    );
    assert!(!body.to_string().contains("tajne") && !body.to_string().contains("secret"));
    assert!(moved
        .iter()
        .all(|c| c["id"].as_str().is_some_and(|id| id.len() == 26)));
    assert!(moved.iter().all(|c| c["allowedClients"] == json!([])));

    let mine = state
        .connectors
        .of_owner(&sub_a, "https://portal.test")
        .await
        .expect("the store");
    assert_eq!(mine.len(), 2, "moved once: {mine:?}");
    assert!(mine
        .iter()
        .all(|c| !c.writes && c.segments == ["data-read"]));
    let (status, again) = internal(&state, &sub_a, &gateway).await;
    assert_eq!(
        (status, again["connectors"].as_array().map(Vec::len)),
        (StatusCode::OK, Some(2))
    );

    let kept = joinedcontext_portal::db::load_preferences(&pool, &sub_a)
        .await
        .expect("the row")
        .expect("a row");
    assert_eq!(
        kept,
        json!({
            "theme": "dark",
            "connectorsNotMoved": [{
                "name": "nothing",
                "select": { "projects": [], "spaces": [], "domains": [], "themes": ["AGRI"], "accessLevel": [], "endpoints": [] },
                "reason": "takes no Endpoint you may read today",
            }, {
                "name": "hidden",
                "select": { "projects": [], "spaces": [], "domains": [], "themes": [], "accessLevel": [], "endpoints": [slug("secret")] },
                "reason": "takes no Endpoint you may read today",
            }],
        }),
        "the moved ones left, the ones taking nothing readable stayed aside, the rest stayed"
    );
    let prefs = sign_in(&state, &a).await;
    assert_eq!(prefs.status, StatusCode::OK);
    assert!(
        prefs.text.contains("dark")
            && !prefs.text.contains("fav")
            && !prefs.text.contains("nothing"),
        "{}",
        prefs.text
    );

    let (status, theirs) = internal(&state, &sub_b, &gateway).await;
    assert_eq!(
        (status, theirs),
        (StatusCode::OK, json!({ "connectors": [] }))
    );

    for bearer in [
        common::REALM.workload("some-app"),
        common::REALM.person_token_of(common::INTERNAL_AUDIENCE, "portal-ui", &a, &[]),
        common::REALM.person_token("x", &a, &[]),
        common::REALM.token(GATEWAY_CLIENT, "portal-api"),
    ] {
        let (status, body) = internal(&state, &sub_a, &bearer).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{status} {body}"
        );
        assert!(!body.to_string().contains("fav"));
    }
}

#[tokio::test]
async fn a_save_of_preferences_never_drops_old_connectors_and_refuses_the_old_field() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let state = state(&url).await;
    let a = fresh("a");
    let sub_a = format!("sub-{a}");
    let pool = state.db.clone().expect("a pool");
    joinedcontext_portal::db::save_preferences(
        &pool,
        &sub_a,
        &json!({ "connectors": [{ "name": "fav", "select": { "projects": ["mesto"] } }] }),
    )
    .await
    .expect("an old row");
    reader_in_mesto(&state, &a);
    let saved = common::send(
        &state,
        common::person(&a),
        "PUT",
        "/api/v1/preferences",
        Some(json!({ "theme": "light" })),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let mine = state
        .connectors
        .of_owner(&sub_a, "https://portal.test")
        .await
        .expect("the store");
    assert_eq!(
        mine.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["fav"]
    );

    let old = common::send(
        &state,
        common::person(&a),
        "PUT",
        "/api/v1/preferences",
        Some(json!({ "connectors": [{ "name": "x", "select": {} }] })),
    )
    .await;
    assert_eq!(old.status, StatusCode::BAD_REQUEST, "{}", old.text);
}

#[tokio::test]
async fn only_live_data_reading_connectors_reach_the_gateway() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let state = state(&url).await;
    let sub = format!("sub-{}", fresh("c"));
    let new = |name: &str, segment: &str| -> joinedcontext_portal::connectors::NewConnector {
        serde_json::from_value(json!({
            "name": name,
            "segments": [segment],
            "scope": [{ "project": "mesto", "access": "read" }],
            "expiresAt": "2999-01-01T00:00:00Z",
        }))
        .expect("a connector")
    };
    let base = "https://portal.test";
    state
        .connectors
        .create(&sub, new("reads", "data-read"), Vec::new(), base)
        .await
        .expect("created");
    state
        .connectors
        .create(&sub, new("models", "context"), Vec::new(), base)
        .await
        .expect("created");
    let gone = state
        .connectors
        .create(&sub, new("gone", "data-read"), Vec::new(), base)
        .await
        .expect("created");
    assert!(state
        .connectors
        .revoke(&gone.id, &sub)
        .await
        .expect("revoked"));
    let (status, body) = internal(&state, &sub, &common::REALM.workload(GATEWAY_CLIENT)).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["connectors"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert_eq!(names, ["reads"], "{body}");
}

#[tokio::test]
async fn an_unreadable_old_list_stays_in_place_and_a_save_keeps_it() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let state = state(&url).await;
    let a = fresh("a");
    let sub_a = format!("sub-{a}");
    let pool = state.db.clone().expect("a pool");
    let unreadable =
        json!([{ "name": "fav", "select": {} }, { "select": { "projects": ["mesto"] } }]);
    joinedcontext_portal::db::save_preferences(
        &pool,
        &sub_a,
        &json!({ "theme": "dark", "connectors": unreadable }),
    )
    .await
    .expect("an old row");

    let one = joinedcontext_portal::personal_connectors::move_one(&state, &common::person(&a))
        .await
        .expect("a move");
    assert_eq!((one.unreadable, one.moved), (1, 0), "{one:?}");
    let prefs = common::send(
        &state,
        common::person(&a),
        "GET",
        "/api/v1/preferences",
        None,
    )
    .await;
    assert_eq!(prefs.status, StatusCode::OK, "{}", prefs.text);
    assert!(
        prefs.text.contains("dark") && !prefs.text.contains("fav"),
        "the person's own settings still read, the old list hidden: {}",
        prefs.text
    );
    let saved = common::send(
        &state,
        common::person(&a),
        "PUT",
        "/api/v1/preferences",
        Some(json!({ "theme": "light" })),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let row = joinedcontext_portal::db::load_preferences(&pool, &sub_a)
        .await
        .expect("the row")
        .expect("a row");
    assert_eq!(
        row,
        json!({ "theme": "light", "connectors": unreadable }),
        "a whole-document save keeps the unreadable list"
    );
    let mine = state
        .connectors
        .of_owner(&sub_a, "https://portal.test")
        .await
        .expect("the store");
    assert!(
        mine.is_empty(),
        "nothing of an unreadable list moves: {mine:?}"
    );
}

#[tokio::test]
async fn an_old_connector_whose_name_is_taken_is_kept_aside_and_a_save_keeps_it() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let state = state(&url).await;
    let a = fresh("a");
    let sub_a = format!("sub-{a}");
    let pool = state.db.clone().expect("a pool");
    let old = json!({ "connectors": [{ "name": "fav", "select": { "projects": ["mesto"] } }] });
    reader_in_mesto(&state, &a);
    joinedcontext_portal::db::save_preferences(&pool, &sub_a, &old)
        .await
        .expect("an old row");
    let first = joinedcontext_portal::personal_connectors::move_one(&state, &common::person(&a))
        .await
        .expect("a move");
    assert_eq!(first.moved, 1);
    // A second Portal's copy of the same list arrives after the first moved it.
    joinedcontext_portal::db::save_preferences(&pool, &sub_a, &old)
        .await
        .expect("the old row again");
    let second = joinedcontext_portal::personal_connectors::move_one(&state, &common::person(&a))
        .await
        .expect("a move");
    assert_eq!((second.moved, second.present), (0, 1), "{second:?}");

    let saved = common::send(
        &state,
        common::person(&a),
        "PUT",
        "/api/v1/preferences",
        Some(json!({ "theme": "light" })),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let row = joinedcontext_portal::db::load_preferences(&pool, &sub_a)
        .await
        .expect("the row")
        .expect("a row");
    assert_eq!(row["theme"], "light");
    assert!(row.get("connectors").is_none(), "{row}");
    let kept = row["connectorsNotMoved"].as_array().expect("kept aside");
    assert_eq!(kept.len(), 1, "{row}");
    assert_eq!(kept[0]["name"], "fav");
    assert_eq!(kept[0]["select"]["projects"], json!(["mesto"]));
    assert!(
        kept[0]["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{row}"
    );
}
