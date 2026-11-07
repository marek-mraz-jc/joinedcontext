//! T-4849 (AG-126…AG-129, API/01 §48): Administration → Agents lists every agent identity the
//! caller may read, counts nothing they may not, carries no secret or prompt, and answers `403`
//! to a member who reads no agent kind.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::agents::run::AgentRun;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

fn bind(state: &AppState, name: &str, user: &str, role: &str, scope: Value) {
    state.mirror.upsert(envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": user }], "role": role, "scope": scope }),
    ));
}

fn role(state: &AppState, name: &str, namespace: &str, kinds: &[&str], verbs: &[&str]) {
    state.mirror.upsert(envelope(
        "Role",
        name,
        namespace,
        json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
    ));
}

fn run(id: &str, by: &str, status: &str, tokens: i64) -> AgentRun {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    AgentRun {
        id: id.to_owned(),
        project: "helsinki".to_owned(),
        app_name: "air-dashboard".to_owned(),
        title: Some("Air quality dashboard".to_owned()),
        endpoint_name: "air-quality".to_owned(),
        endpoint_slug: "slug".to_owned(),
        endpoints: json!([{ "name": "air-quality", "slug": "slug", "space": "air" }]),
        profile: "builder".to_owned(),
        kind: "application".to_owned(),
        unattended: false,
        continues: None,
        app_class: "ui".to_owned(),
        visibility: "project".to_owned(),
        prompt: "RUN-PROMPT-TEXT".to_owned(),
        prompt_digest: "sha256:abc".to_owned(),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-air-dashboard/{id}"),
        path_prefix: "projects/helsinki/apps/air-dashboard/".to_owned(),
        status: status.to_owned(),
        ticket_hash: "$argon2id$TICKET-HASH".to_owned(),
        workspace: None,
        merge_request: None,
        change_id: None,
        source_url: None,
        mirror_url: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: json!({}),
        steps: 0,
        tokens_used: tokens,
        created_by: by.to_owned(),
        origin: "person".to_owned(),
        starter: Value::Null,
        created_at: now.clone(),
        started_at: Some(now),
        finished_at: None,
        expires_at: "2099-01-01T00:00:00Z".to_owned(),
        error: None,
    }
}

async fn fixture() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    let agent_kinds = ["AssistantDeployment", "McpServer", "ServiceAccount"];
    role(&state, "agent-reader", "helsinki", &agent_kinds, &["read"]);
    role(&state, "data-reader", "helsinki", &["Endpoint"], &["read"]);
    role(
        &state,
        "profile-reader",
        ORG_NAMESPACE,
        &["AgentProfile"],
        &["read"],
    );
    bind(
        &state,
        "ana",
        "ana@hel.fi",
        "agent-reader",
        json!({ "project": "helsinki" }),
    );
    bind(
        &state,
        "eva",
        "eva@hel.fi",
        "data-reader",
        json!({ "project": "helsinki" }),
    );
    bind(
        &state,
        "olga",
        "olga@hel.fi",
        "profile-reader",
        json!({ "organization": "bb" }),
    );
    state.mirror.upsert(envelope(
        "Endpoint",
        "air-quality",
        "helsinki",
        json!({ "contextSpaceRef": "air", "audience": "public" }),
    ));
    state.mirror.upsert(envelope(
        "AssistantDeployment",
        "city-chat",
        "helsinki",
        json!({
            "publicId": "city-chat", "channel": "public", "enabled": false,
            "title": { "en": "City chat" },
            "systemPrompt": "SYSTEM-PROMPT-TEXT",
            "connectors": [{ "endpoint": "air-quality", "tools": [] }],
            "budget": { "tokensPerDay": 200000 }
        }),
    ));
    state.mirror.upsert(envelope(
        "ServiceAccount",
        "sensor-feed",
        "helsinki",
        json!({
            "owner": { "user": "jana@hel.fi" }, "purpose": "feeds sensors",
            "roles": [{ "role": "writer", "scope": { "contextSpace": "air" } }],
            "credentials": [{ "kind": "clientSecret", "name": "feed-client-secret" }]
        }),
    ));
    state.mirror.upsert(envelope(
        "McpServer",
        "city-mcp",
        "helsinki",
        json!({ "members": [{ "kind": "Endpoint", "name": "air-quality" }], "audience": "organization" }),
    ));
    state.mirror.upsert(envelope(
        "AgentProfile",
        "builder",
        ORG_NAMESPACE,
        json!({ "role": "builder", "model": { "provider": "openrouter", "name": "m", "maxTokensPerRun": 400000 } }),
    ));
    for agent_run in [
        run("run-ana", "ana", "building", 41000),
        run("run-other", "someone", "building", 7),
        run("run-done", "ana", "published", 900),
    ] {
        state
            .agents
            .create_run(&agent_run)
            .await
            .expect("store the run");
    }
    state
}

async fn agents(state: &AppState, who: &str) -> (StatusCode, Value) {
    let answer = send(
        state,
        person(who),
        "GET",
        "/api/v1/organization/agents",
        None,
    )
    .await;
    let body = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
    (answer.status, body)
}

fn find<'a>(body: &'a Value, kind: &str, name: &str) -> Option<&'a Value> {
    body["items"]
        .as_array()?
        .iter()
        .find(|item| item["kind"] == kind && item["name"] == name)
}

#[tokio::test]
async fn a_reader_of_the_agent_kinds_finds_each_agent_with_its_reach_spend_and_state() {
    let state = fixture().await;
    let (status, body) = agents(&state, "ana").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let chat = find(&body, "AssistantDeployment", "city-chat").expect("the deployment");
    assert_eq!(chat["title"], "City chat");
    assert_eq!(chat["state"], "paused");
    assert_eq!(chat["actsAs"], "self");
    assert_eq!(chat["endpoints"], json!(["air-quality"]));
    assert_eq!(chat["spaces"], json!(["air"]));
    assert_eq!(
        chat["spend"],
        json!({ "used": null, "cap": 200000, "per": "day" })
    );

    let account = find(&body, "ServiceAccount", "sensor-feed").expect("the service account");
    assert_eq!(
        account["owner"],
        json!({ "type": "person", "name": "jana@hel.fi" })
    );
    assert_eq!(account["spaces"], json!(["air"]));

    let mcp = find(&body, "McpServer", "city-mcp").expect("the MCP server");
    assert_eq!(
        (&mcp["actsAs"], &mcp["endpoints"]),
        (&json!("caller"), &json!(["air-quality"]))
    );

    // Her own run in flight, with its profile's cap; neither somebody else's nor an ended one.
    let runs: Vec<&Value> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|item| item["kind"] == "AgentRun")
        .collect();
    assert_eq!(runs.len(), 1, "{body}");
    assert_eq!(runs[0]["runId"], "run-ana");
    assert_eq!(runs[0]["owner"]["name"], "ana");
    assert_eq!(
        runs[0]["spend"],
        json!({ "used": 41000, "cap": 400000, "per": "run" })
    );
    assert_eq!(runs[0]["state"], "running");
}

#[tokio::test]
async fn a_kind_the_caller_may_not_read_is_neither_listed_nor_counted() {
    let state = fixture().await;
    let (_, ana) = agents(&state, "ana").await;
    assert!(find(&ana, "AgentProfile", "builder").is_none(), "{ana}");

    // Olga reads agent profiles of the organization and nothing of helsinki.
    let (status, olga) = agents(&state, "olga").await;
    assert_eq!(status, StatusCode::OK, "{olga}");
    let kinds: Vec<&str> = olga["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["AgentProfile"], "{olga}");
    assert_eq!(olga["items"][0]["project"], ORG_NAMESPACE);
    assert_eq!(olga["items"][0]["actsAs"], "caller");
}

#[tokio::test]
async fn a_member_without_read_on_any_agent_kind_is_refused_with_who_can_grant_it() {
    let state = fixture().await;
    let (status, body) = agents(&state, "eva").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("organization administrator")),
        "{body}"
    );
    let (status, _) = agents(&state, "nobody").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn no_prompt_secret_or_credential_name_is_part_of_the_answer() {
    let state = fixture().await;
    let (_, body) = agents(&state, "ana").await;
    let text = body.to_string();
    for needle in [
        "SYSTEM-PROMPT-TEXT",
        "RUN-PROMPT-TEXT",
        "feed-client-secret",
        "TICKET-HASH",
    ] {
        assert!(!text.contains(needle), "{needle} leaked: {text}");
    }
}
