//! Feedback from any page (T-3272, API/01 §38): a signed-in person's words arrive scrubbed and
//! without an author, an administrator reads them and their screenshot, nobody else does.

mod common;

use base64::Engine as _;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage-bytes";

/// `admin` administers the organization; `jana` reads one project and nothing more.
fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["RoleBinding"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "admins",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "admin@hel.fi" }], "role": "org-admin", "scope": { "organization": "hel" } }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "reader", "scope": { "project": "helsinki" } }),
    ));
    state
}

async fn post(state: &AppState, who: &str, body: Value) -> (u16, Value) {
    let answer = send(state, person(who), "POST", "/api/v1/feedback", Some(body)).await;
    (
        answer.status.as_u16(),
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_feedback_arrives_scrubbed_without_its_author_and_an_administrator_reads_it() {
    let state = world();
    let png = base64::engine::general_purpose::STANDARD.encode(PNG);
    // Built from parts, so no literal here reads as a key to a scanner.
    let key = format!(
        "{}{}_{}",
        "jc_", "3f9c2a7b1d4e8f06", "Zm9vYmFyYmF6cXV4MTIzNDU2"
    );
    let (status, body) = post(
        &state,
        "jana",
        json!({
            "text": format!("  The Approve button stays grey. Write to jana@hel.fi or +358 40 123 4567, token {key}  "),
            "page": "/projects/helsinki/approvals/chg-0000001c?edit=jana#top",
            "screenshot": format!("data:image/png;base64,{png}"),
        }),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let id = body["id"].as_i64().expect("an id");

    let answer = send(
        &state,
        person("admin"),
        "GET",
        "/api/v1/organization/feedback",
        None,
    )
    .await;
    assert_eq!(answer.status.as_u16(), 200, "{}", answer.text);
    let list: Value = serde_json::from_str(&answer.text).expect("json");
    let item = &list["items"][0];
    assert_eq!(item["id"], json!(id));
    assert_eq!(
        item["page"],
        json!("/projects/helsinki/approvals/chg-0000001c")
    );
    assert_eq!(item["version"], json!(joinedcontext_portal::APP_VERSION));
    assert_eq!(item["screenshot"], json!(true));
    let text = item["text"].as_str().expect("text");
    assert!(text.starts_with("The Approve button stays grey."), "{text}");
    // "+358", not "358": the digits alone turn up in createdAt's nanoseconds.
    for leaked in ["jana", "+358", "123 4567", "jc_3f9c"] {
        assert!(!answer.text.contains(leaked), "{leaked} in {}", answer.text);
    }
    // Nobody is named: the item has exactly these members.
    let mut keys: Vec<&String> = item.as_object().expect("an object").keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["createdAt", "id", "page", "screenshot", "text", "version"]
    );

    let shot = send(
        &state,
        person("admin"),
        "GET",
        &format!("/api/v1/organization/feedback/{id}/screenshot"),
        None,
    )
    .await;
    assert_eq!(shot.status.as_u16(), 200);
    let after = send(
        &state,
        person("admin"),
        "GET",
        &format!("/api/v1/organization/feedback?after={id}"),
        None,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&after.text).expect("json")["items"],
        json!([])
    );
}

#[tokio::test]
async fn only_an_administrator_reads_feedback() {
    let state = world();
    post(&state, "jana", json!({ "text": "x", "page": "/" })).await;
    for path in [
        "/api/v1/organization/feedback",
        "/api/v1/organization/feedback/1/screenshot",
    ] {
        let answer = send(&state, person("jana"), "GET", path, None).await;
        assert_eq!(answer.status.as_u16(), 403, "{path}: {}", answer.text);
    }
    let none = send(
        &state,
        person("admin"),
        "GET",
        "/api/v1/organization/feedback/1/screenshot",
        None,
    )
    .await;
    assert_eq!(
        none.status.as_u16(),
        404,
        "a feedback without a screenshot has none"
    );
}

#[tokio::test]
async fn what_cannot_be_kept_is_refused_naming_the_field() {
    let state = world();
    for (body, field) in [
        (json!({ "text": "   ", "page": "/" }), "text"),
        (json!({ "text": "x".repeat(2001), "page": "/" }), "text"),
        (
            json!({ "text": "x", "page": "https://evil.example/" }),
            "page",
        ),
        (
            json!({ "text": "x", "page": "/", "screenshot": "data:image/jpeg;base64,AAAA" }),
            "screenshot",
        ),
    ] {
        let (status, answer) = post(&state, "jana", body).await;
        assert_eq!(status, 400, "{answer}");
        assert_eq!(answer["errors"][0]["path"], json!(field), "{answer}");
        assert_eq!(
            answer["errors"].as_array().map(Vec::len),
            Some(1),
            "{answer}"
        );
    }
    let (status, _) = post(
        &state,
        "jana",
        json!({ "text": "x", "page": "/", "author": "jana" }),
    )
    .await;
    assert_eq!(status, 400, "an unknown member");
    let answer = send(
        &state,
        person("admin"),
        "GET",
        "/api/v1/organization/feedback",
        None,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&answer.text).expect("json")["items"],
        json!([]),
        "nothing refused was kept"
    );
}

#[tokio::test]
async fn a_person_sends_ten_an_hour_and_the_eleventh_is_told_when_to_try_again() {
    let state = world();
    for n in 0..10 {
        let (status, body) = post(
            &state,
            "burst",
            json!({ "text": format!("note {n}"), "page": "/" }),
        )
        .await;
        assert_eq!(status, 202, "{body}");
    }
    let answer = send(
        &state,
        person("burst"),
        "POST",
        "/api/v1/feedback",
        Some(json!({ "text": "one more", "page": "/" })),
    )
    .await;
    assert_eq!(answer.status.as_u16(), 429, "{}", answer.text);
    assert!(answer.text.contains("minutes"), "{}", answer.text);
}
