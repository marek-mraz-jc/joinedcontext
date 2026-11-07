//! The run operations through every door of the registry (T-1529; AG-64, PF-50, PF-59, AG-11,
//! AG-45).
//!
//! What was there before this file: the REST routes are tested in `agent_runs_tests.rs` and
//! `edge_agent_runs_tests.rs` (reading, messaging and answering one's own run), and
//! `attack_person_only_operations_tests.rs` refuses `jc_run_answer` to a run by name through
//! `may_run`. No test called `jc_run_list`, `jc_run_get` or `jc_run_message` through the registry,
//! and none compared a door's refusal with the route's.

mod common;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::envelope;
use joinedcontext_portal::agents::run::{digest_prompt, mint_run_id, mint_ticket, AgentRun};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::Via;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

/// May read and propose in [`PROJECT`], and approves nothing: sees only the runs she started.
fn lucia() -> Identity {
    Identity {
        client: None,
        subject: "f:1:lucia".into(),
        username: "lucia".into(),
        email: Some("lucia@banskabystrica.sk".into()),
        name: Some("lucia".into()),
        roles: Vec::new(),
        groups: vec!["city-authors".into()],
        second_factor: false,
    }
}

fn a_run(created_by: &str, status: &str) -> AgentRun {
    let id = mint_run_id();
    serde_json::from_value(json!({
        "id": id,
        "project": PROJECT,
        "appName": "air-map",
        "title": "Air map",
        "endpointName": "air-public",
        "endpointSlug": "mluyob4nz52lok3ssk7pgn5vwt",
        "profile": "app-builder",
        "kind": "conversation",
        "unattended": false,
        "appClass": "ui",
        "visibility": "project",
        "prompt": "p",
        "promptDigest": digest_prompt("p"),
        "dataNeeds": [],
        "allowsWrite": false,
        "branch": format!("agent/app-air-map/{id}"),
        "pathPrefix": "",
        "status": status,
        "ticketHash": mint_ticket().1,
        "steps": 0,
        "tokensUsed": 0,
        "createdBy": created_by,
        "createdAt": "2026-09-21T06:00:00Z",
        "expiresAt": "2099-09-21T06:00:00Z"
    }))
    .expect("a run")
}

struct World {
    state: AppState,
    /// Jana's run, building, with question `q1` open.
    janas: String,
    /// Lucia's run, building.
    lucias: String,
    /// Jana's run that failed.
    finished: String,
}

async fn world() -> World {
    let state = doors::state();
    state.mirror.upsert(envelope(
        "Role",
        "author-role",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "author-binding",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "city-authors" }],
            "role": "author-role",
            "scope": { "project": PROJECT },
        }),
    ));
    let janas = a_run("jana", "building");
    let lucias = a_run("lucia", "building");
    let finished = a_run("jana", "failed");
    for one in [&janas, &lucias, &finished] {
        state.agents.create_run(one).await.expect("stored");
    }
    state
        .agents
        .append_event(
            &janas.id,
            "question",
            json!({ "questionId": "q1", "question": "Which colour for the map?" }),
        )
        .await
        .expect("asked");
    World {
        state,
        janas: janas.id,
        lucias: lucias.id,
        finished: finished.id,
    }
}

fn ids(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|run| run["id"].as_str().map(str::to_owned))
        .collect()
}

fn answer(id: &str) -> Value {
    json!({ "id": id, "questionId": "q1", "answers": { "answer": "blue" } })
}

fn message(id: &str) -> Value {
    json!({ "id": id, "text": "Make the legend larger" })
}

/// PF-50: the steward approves in the project and reads every run of it at every door; the person
/// who started a run reads it.
#[tokio::test]
async fn every_door_lists_and_reads_the_runs_its_person_may_see() {
    let world = world().await;
    let reads = ["jc_run_list", "jc_run_get"];
    for (who, caller) in [
        ("steward", session(steward())),
        ("steward over mcp", mcp(steward())),
        ("steward's run", run(steward(), &reads)),
    ] {
        let list = doors::call("jc_run_list", &caller, &world.state, json!({})).await;
        assert_eq!(StatusCode::OK, list.status, "{who}: {}", list.text());
        let listed = ids(&list.body);
        for id in [&world.janas, &world.lucias, &world.finished] {
            assert!(listed.contains(id), "{who}: {id} not in {listed:?}");
        }
        let one = doors::call(
            "jc_run_get",
            &caller,
            &world.state,
            json!({ "id": world.lucias }),
        )
        .await;
        assert_eq!(StatusCode::OK, one.status, "{who}: {}", one.text());
        assert_eq!(world.lucias.as_str(), one.body["id"], "{who}");
    }
    let own = doors::call(
        "jc_run_get",
        &mcp(lucia()),
        &world.state,
        json!({ "id": world.lucias }),
    )
    .await;
    assert_eq!(StatusCode::OK, own.status, "{}", own.text());
}

/// PF-59, R20: a person who approves nothing sees only the runs she started: another person's run
/// is not listed and reads as not there, at every door. A stranger finds no project.
#[tokio::test]
async fn a_run_of_another_person_is_invisible() {
    let world = world().await;
    for caller in [
        session(lucia()),
        mcp(lucia()),
        run(lucia(), &["jc_run_list", "jc_run_get"]),
    ] {
        let list = doors::call("jc_run_list", &caller, &world.state, json!({})).await;
        assert_eq!(StatusCode::OK, list.status, "{}", list.text());
        assert_eq!(vec![world.lucias.clone()], ids(&list.body));

        let hidden = doors::call(
            "jc_run_get",
            &caller,
            &world.state,
            json!({ "id": world.janas }),
        )
        .await;
        assert_eq!(StatusCode::NOT_FOUND, hidden.status, "{}", hidden.text());
        assert!(!hidden.text().contains("colour"), "{}", hidden.text());
    }
    for (name, input) in [
        ("jc_run_list", json!({})),
        ("jc_run_get", json!({ "id": world.janas })),
    ] {
        let refused = doors::call(name, &session(stranger()), &world.state, input).await;
        assert_eq!(
            StatusCode::NOT_FOUND,
            refused.status,
            "{name}: {}",
            refused.text()
        );
    }
}

/// AG-45: the person a run acts for sends it a message through the session and through her own
/// MCP client, and answers its question in the session alone: over MCP the answer is refused,
/// a model driving the client never decides for her (T-5699).
#[tokio::test]
async fn the_person_messages_her_run_and_answers_its_question() {
    let world = world().await;
    for caller in [session(steward()), mcp(steward())] {
        let sent = doors::call(
            "jc_run_message",
            &caller,
            &world.state,
            message(&world.janas),
        )
        .await;
        assert_eq!(StatusCode::OK, sent.status, "{}", sent.text());
        assert_eq!(json!(true), sent.body["accepted"]);

        let over_mcp = caller.via == Via::Mcp;
        let answered =
            doors::call("jc_run_answer", &caller, &world.state, answer(&world.janas)).await;
        if over_mcp {
            assert_eq!(
                StatusCode::FORBIDDEN,
                answered.status,
                "{}",
                answered.text()
            );
        } else {
            assert_eq!(StatusCode::OK, answered.status, "{}", answered.text());
        }
    }
    let events = world
        .state
        .agents
        .events_since(&world.janas, 0)
        .await
        .expect("events");
    let answers: Vec<&Value> = events
        .iter()
        .filter(|event| event.kind == "answer")
        .map(|event| &event.payload)
        .collect();
    assert_eq!(1, answers.len(), "{answers:?}");
    assert_eq!("jana", answers[0]["answeredBy"]);
}

/// AG-45: a run that is over reads no message and asks nothing; the refusal says why and nothing
/// is recorded on it.
#[tokio::test]
async fn a_finished_run_is_sent_nothing_and_says_why() {
    let world = world().await;
    let caller = session(steward());
    let refused = doors::call(
        "jc_run_message",
        &caller,
        &world.state,
        message(&world.finished),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, refused.status, "{}", refused.text());
    assert!(
        refused.text().contains("'failed' and reads nothing"),
        "{}",
        refused.text()
    );

    let refused = doors::call(
        "jc_run_answer",
        &caller,
        &world.state,
        answer(&world.finished),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, refused.status, "{}", refused.text());
    assert!(
        refused.text().contains("asks nothing"),
        "{}",
        refused.text()
    );

    let events = world
        .state
        .agents
        .events_since(&world.finished, 0)
        .await
        .expect("events");
    assert!(events.is_empty(), "{events:?}");
}

/// AG-11, AG-45: an agent run never answers a question, its own or another's, whatever its profile
/// names, and is never offered the operation; a viewer may not message or answer at all.
#[tokio::test]
async fn an_agent_never_answers_a_question_and_a_viewer_never_writes_to_a_run() {
    let world = world().await;
    let agent = run(steward(), &["jc_run_answer", "jc_run_message"]);
    assert!(!doors::offered(&agent, &world.state).contains(&"jc_run_answer".to_owned()));
    let refused = doors::call("jc_run_answer", &agent, &world.state, answer(&world.janas)).await;
    assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
    assert!(refused.text().contains("AG-11"), "{}", refused.text());

    for caller in [session(viewer()), mcp(viewer())] {
        for (name, input) in [
            ("jc_run_message", message(&world.janas)),
            ("jc_run_answer", answer(&world.janas)),
        ] {
            let refused = doors::call(name, &caller, &world.state, input).await;
            assert_eq!(
                StatusCode::FORBIDDEN,
                refused.status,
                "{name}: {}",
                refused.text()
            );
        }
    }
    let events = world
        .state
        .agents
        .events_since(&world.janas, 0)
        .await
        .expect("events");
    assert_eq!(
        vec!["question"],
        events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        "a refused call wrote to the run",
    );
}

/// Input is validated at the door: an empty message, an unknown field and a limit of zero.
#[tokio::test]
async fn an_input_the_route_would_refuse_is_refused_at_the_door() {
    let world = world().await;
    let caller = session(steward());
    for (name, input) in [
        (
            "jc_run_message",
            json!({ "id": world.janas, "text": "   " }),
        ),
        (
            "jc_run_message",
            json!({ "id": world.janas, "text": "x", "urgent": true }),
        ),
        ("jc_run_list", json!({ "limit": 0 })),
    ] {
        let refused = doors::call(name, &caller, &world.state, input.clone()).await;
        assert!(
            refused.status.is_client_error() && refused.status != StatusCode::NOT_FOUND,
            "{name} {input}: {} {}",
            refused.status,
            refused.text()
        );
    }
}

/// AG-64: the REST routes and `POST …/ops/{name}` refuse each person alike.
#[tokio::test]
async fn the_rest_routes_and_the_ops_door_refuse_alike() {
    let world = world().await;
    let base = "/api/v1/projects/ovzdusie/agent-runs";
    for identity in [lucia(), stranger()] {
        let cases = [
            ("jc_run_list", "GET", base.to_owned(), None, json!({})),
            (
                "jc_run_get",
                "GET",
                format!("{base}/{}", world.janas),
                None,
                json!({ "id": world.janas }),
            ),
            (
                "jc_run_message",
                "POST",
                format!("{base}/{}/messages", world.janas),
                Some(json!({ "text": "Make the legend larger" })),
                message(&world.janas),
            ),
            (
                "jc_run_answer",
                "POST",
                format!("{base}/{}/answers", world.janas),
                Some(json!({ "questionId": "q1", "answers": { "answer": "blue" } })),
                answer(&world.janas),
            ),
        ];
        for (name, rest_method, uri, body, input) in cases {
            let rest = doors::http(&world.state, identity.clone(), rest_method, &uri, body).await;
            let door = doors::post_op(&world.state, identity.clone(), name, input).await;
            if name == "jc_run_list" && identity.username == "lucia" {
                assert_eq!(StatusCode::OK, rest.status, "{}", rest.text());
            } else {
                assert!(rest.status.is_client_error(), "{name}: {}", rest.text());
            }
            if identity.username == "eva" && rest_method == "POST" {
                // PF-50: the door refuses a write with no binding 403; the route answers 404
                // because it never asks for the binding (T-2486). Both refuse.
                assert_eq!(
                    StatusCode::FORBIDDEN,
                    door.status,
                    "{name}: {}",
                    door.text()
                );
                continue;
            }
            assert_eq!(
                rest.status,
                door.status,
                "{} {name}: {} vs {}",
                identity.username,
                rest.text(),
                door.text()
            );
        }
    }
}
