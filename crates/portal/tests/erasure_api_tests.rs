//! API/01 §46, DM-142, T-5356: an erasure takes four eyes. The proposer never approves it, the
//! approver types the subject the proposal names, and a declined or expired proposal is approved
//! by nobody. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write
//! to; otherwise it says so and returns.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use joinedcontext_portal::config::Config;
use joinedcontext_portal::erasure;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const SPACE: &str = "trips";
const KEY: &str = "a-test-root-secret-key-of-32-bytes!";

fn role(state: &AppState, name: &str, user: &str, project: &str, verbs: &[&str]) {
    state.mirror.upsert(common::envelope(
        "Role",
        name,
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("{name}-{user}"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{user}@hel.fi") }], "role": name, "scope": { "project": project } }),
    ));
}

/// `anna` and `boris` own the space; `eve` reads it.
async fn world() -> Option<(AppState, String)> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty());
    let Some(url) = url else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let project = format!(
        "fe-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default()
    );
    let mut config = Config::for_tests();
    config.artifact_store = Some(joinedcontext_portal::artifact_store::Settings {
        endpoint: "http://127.0.0.1:9".into(),
        bucket: "artifacts".into(),
        region: "us-east-1".into(),
        root_access_key: "test".into(),
        root_secret_key: KEY.into(),
    });
    let state = AppState::new(config, None).with_db(pool);
    state.mirror.upsert(common::envelope(
        "Project",
        &project,
        ORG_NAMESPACE,
        json!({}),
    ));
    state.mirror.upsert(common::envelope(
        "ContextSpace",
        SPACE,
        &project,
        json!({ "broker": "antares", "dataModelRef": { "name": "trips" } }),
    ));
    let owner = ["read", "propose", "approve"];
    role(&state, "trips-owner", "anna", &project, &owner);
    role(&state, "trips-owner-2", "boris", &project, &owner);
    role(&state, "trips-reader", "eve", &project, &["read"]);
    Some((state, project))
}

/// anna's proposal for the subject `r-42`, `days` old.
async fn proposal(state: &AppState, project: &str, days: i64) -> String {
    let mut proposed = erasure::begin(
        (project, SPACE),
        erasure::subject_hash(KEY, project, SPACE, "r-42"),
        "Art. 17 request 7",
        "user:anna",
        vec![],
    );
    proposed.requested_at -= time::Duration::days(days);
    let pool = state.db.as_ref().expect("a database");
    erasure::insert(pool, &proposed).await.expect("logged");
    proposed.id
}

async fn send(
    state: &AppState,
    who: &str,
    verb: &str,
    uri: &str,
    body: Option<Value>,
) -> common::Answer {
    common::send(state, common::person(who), verb, uri, body).await
}

#[tokio::test]
async fn the_proposer_never_approves_and_the_approver_types_the_proposals_subject() {
    let Some((state, project)) = world().await else {
        return;
    };
    let id = proposal(&state, &project, 0).await;
    let base = format!("/api/v1/projects/{project}/spaces/{SPACE}/erasures");
    let approve = format!("{base}/{id}/approve");

    let own = send(
        &state,
        "anna",
        "POST",
        &approve,
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.text);
    assert!(own.text.contains("self-approval"), "{}", own.text);

    let reader = send(
        &state,
        "eve",
        "POST",
        &approve,
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);

    let other = send(
        &state,
        "boris",
        "POST",
        &approve,
        Some(json!({"subject": "r-43"})),
    )
    .await;
    assert_eq!(
        other.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        other.text
    );
    assert!(other.text.contains("subject"), "{}", other.text);
    assert!(
        !other.text.contains("r-43"),
        "the answer never echoes the value: {}",
        other.text
    );

    let unknown = send(
        &state,
        "boris",
        "POST",
        &format!("{base}/er-20261019-0a1b2c3d4e5f/approve"),
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);

    // The second owner finds the proposal in the list; nothing ran.
    let listed = send(&state, "boris", "GET", &base, None).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    let items: Value = serde_json::from_str(&listed.text).expect("json");
    let item = &items["items"][0];
    assert_eq!(item["id"], id.as_str());
    assert_eq!(item["state"], "proposed");
    assert_eq!(item["proposedBy"], "user:anna");
    assert_eq!(item["stores"]["broker"]["attempts"], 0);
    let refused = send(&state, "eve", "GET", &base, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
}

#[tokio::test]
async fn a_declined_or_expired_proposal_is_approved_by_nobody() {
    let Some((state, project)) = world().await else {
        return;
    };
    let base = format!("/api/v1/projects/{project}/spaces/{SPACE}/erasures");
    let id = proposal(&state, &project, 0).await;

    let bare = send(
        &state,
        "anna",
        "POST",
        &format!("{base}/{id}/decline"),
        Some(json!({"reason": " "})),
    )
    .await;
    assert_eq!(bare.status, StatusCode::BAD_REQUEST, "{}", bare.text);
    assert!(bare.text.contains("reason-required"), "{}", bare.text);

    let withdrawn = send(
        &state,
        "anna",
        "POST",
        &format!("{base}/{id}/decline"),
        Some(json!({"reason": "the person withdrew the request"})),
    )
    .await;
    assert_eq!(withdrawn.status, StatusCode::OK, "{}", withdrawn.text);
    let view: Value = serde_json::from_str(&withdrawn.text).expect("json");
    assert_eq!(view["state"], "declined");
    assert_eq!(view["declinedBy"], "user:anna");

    let late = send(
        &state,
        "boris",
        "POST",
        &format!("{base}/{id}/approve"),
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.text);
    assert!(late.text.contains("declined"), "{}", late.text);

    let old = proposal(&state, &project, erasure::PROPOSAL_DAYS).await;
    let expired = send(
        &state,
        "boris",
        "POST",
        &format!("{base}/{old}/approve"),
        Some(json!({"subject": "r-42"})),
    )
    .await;
    assert_eq!(expired.status, StatusCode::CONFLICT, "{}", expired.text);
    assert!(expired.text.contains("expired"), "{}", expired.text);
    let read = send(&state, "boris", "GET", &format!("{base}/{old}"), None).await;
    assert_eq!(
        serde_json::from_str::<Value>(&read.text).expect("json")["state"],
        "expired"
    );
}
