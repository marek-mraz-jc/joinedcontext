//! T-4851 (AG-134…AG-136, API/01 §51): what the agents did, as the caller may see it: their
//! own runs, every person's for an approver; tool calls with verdicts, refusals, tokens and cost;
//! never a step's arguments; Stop ends a run in flight.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::agents::run::AgentRun;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

fn bind(state: &AppState, name: &str, user: &str, role: &str) {
    state.mirror.upsert(envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": user }], "role": role, "scope": { "project": "helsinki" } }),
    ));
}

fn role(state: &AppState, name: &str, verbs: &[&str]) {
    state.mirror.upsert(envelope(
        "Role",
        name,
        "helsinki",
        json!({ "rules": [{ "kinds": ["App"], "verbs": verbs }] }),
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
    state
        .mirror
        .upsert(envelope("Project", "helsinki", ORG_NAMESPACE, json!({})));
    role(&state, "builder", &["read", "propose"]);
    role(&state, "steward", &["read", "propose", "approve"]);
    bind(&state, "ana", "ana@hel.fi", "builder");
    bind(&state, "stew", "stew@hel.fi", "steward");
    for agent_run in [
        run("run-ana", "ana", "building", 41000),
        run("run-other", "someone", "building", 7),
    ] {
        state
            .agents
            .create_run(&agent_run)
            .await
            .expect("store the run");
    }
    for (kind, payload) in [
        (
            "tool",
            json!({ "tool": "jc_resource_list", "status": "ok", "durationMs": 9,
                         "input": { "kind": "Endpoint", "q": "TYPED-BY-A-PERSON" },
                         "output": { "rows": ["ROW-CONTENT"] } }),
        ),
        (
            "tool",
            json!({ "tool": "jc_endpoint_propose", "status": "failed",
                         "input": { "name": "air" }, "error": "forbidden: missing role steward" }),
        ),
        (
            "usage",
            json!({ "inputTokens": 10, "outputTokens": 5, "costUsd": 0.0125 }),
        ),
    ] {
        state
            .agents
            .append_event("run-ana", kind, payload)
            .await
            .expect("store the event");
    }
    state
}

async fn activity(state: &AppState, who: &str, query: &str) -> (StatusCode, Value) {
    let answer = send(
        state,
        person(who),
        "GET",
        &format!("/api/v1/organization/agents/activity{query}"),
        None,
    )
    .await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

fn ids(body: &Value) -> Vec<&str> {
    body["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .filter_map(|run| run["id"].as_str())
        .collect()
}

#[tokio::test]
async fn a_person_sees_their_own_runs_with_steps_refusals_and_cost_never_the_arguments() {
    let state = fixture().await;
    let (status, body) = activity(&state, "ana", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["run-ana"], "somebody else's run stays hidden");
    let run = &body["runs"][0];
    assert_eq!(run["tokens"], 41000);
    assert_eq!(run["costUsd"], 0.0125);
    assert_eq!((&run["refusals"], &run["writes"]), (&json!(1), &json!(0)));
    assert_eq!(run["mayStop"], true);
    assert_eq!(run["spaces"], json!(["air"]));
    let steps = run["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2);
    assert_eq!(
        (&steps[0]["tool"], &steps[0]["target"], &steps[0]["verdict"]),
        (&json!("jc_resource_list"), &json!("Endpoint"), &json!("ok"))
    );
    assert_eq!(steps[1]["verdict"], "refused");
    assert_eq!(steps[1]["reason"], "forbidden: missing role steward");
    let text = body.to_string();
    assert!(
        !text.contains("TYPED-BY-A-PERSON") && !text.contains("ROW-CONTENT"),
        "{text}"
    );
    assert!(
        !text.contains("RUN-PROMPT-TEXT") && !text.contains("TICKET-HASH"),
        "{text}"
    );
}

#[tokio::test]
async fn an_approver_sees_every_run_and_the_filters_narrow_it() {
    let state = fixture().await;
    let (_, all) = activity(&state, "stew", "").await;
    let mut seen = ids(&all);
    seen.sort();
    assert_eq!(seen, ["run-ana", "run-other"], "{all}");
    let (_, theirs) = activity(&state, "stew", "?person=someone").await;
    assert_eq!(ids(&theirs), ["run-other"]);
    let (_, refused) = activity(&state, "stew", "?verdict=refused").await;
    assert_eq!(ids(&refused), ["run-ana"]);
    let (_, clean) = activity(&state, "stew", "?verdict=ok&space=air&profile=builder").await;
    assert_eq!(ids(&clean), ["run-other"]);
    assert_eq!(all["choices"]["spaces"], json!(["air"]), "{all}");
    assert_eq!(all["choices"]["people"], json!(["ana", "someone"]), "{all}");
    let (_, later) = activity(&state, "stew", "?since=2099-01-01T00:00:00Z").await;
    assert_eq!(ids(&later), Vec::<&str>::new());
}

#[tokio::test]
async fn a_person_who_reads_no_project_is_refused_and_a_bad_filter_is_a_400() {
    let state = fixture().await;
    let (status, body) = activity(&state, "eva", "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["detail"].as_str().unwrap_or("").contains("grant read"),
        "{body}"
    );
    // A person or space outside the caller's runs is refused by name, and the choices a
    // person is offered hold only their own runs' values.
    let (status, body) = activity(&state, "stew", "?space=water").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["detail"].as_str().unwrap_or("").starts_with("space:"),
        "{body}"
    );
    let (status, body) = activity(&state, "ana", "?person=someone").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["detail"].as_str().unwrap_or("").starts_with("person:"),
        "{body}"
    );
    let (_, own) = activity(&state, "ana", "").await;
    assert_eq!(own["choices"]["people"], json!(["ana"]), "{own}");
    let (status, body) = activity(&state, "ana", "?verdict=maybe").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = activity(&state, "ana", "?prompt=x").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unknown filter is refused"
    );
}

#[tokio::test]
async fn stop_ends_a_run_in_flight_and_the_activity_shows_who_stopped_it() {
    let state = fixture().await;
    let stop = send(
        &state,
        person("stew"),
        "POST",
        "/api/v1/projects/helsinki/agent-runs/run-other/cancel",
        None,
    )
    .await;
    assert_eq!(stop.status, StatusCode::OK, "{}", stop.text);
    let (_, body) = activity(&state, "stew", "?run=run-other").await;
    assert_eq!(body["runs"][0]["status"], "cancelled", "{body}");
    assert_eq!(body["runs"][0]["mayStop"], false);
    // Ana may not stop a run she did not start: the route answers as for an unknown run.
    let refused = send(
        &state,
        person("ana"),
        "POST",
        "/api/v1/projects/helsinki/agent-runs/run-other/cancel",
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
}
