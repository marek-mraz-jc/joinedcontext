//! Who sees a workspace (T-1258, PF-59, CC-79): its owner always; anyone else only with `read` on
//! what it covers, and what they may not see answers like a workspace that is not there.

mod common;

use axum::http::StatusCode;
use common::{envelope, person, send};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::workspaces::{Opening, Scope, ScopedResource};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

const WS: &str = "/api/v1/projects/helsinki/workspaces";

fn grouped(name: &str, group: &str) -> Identity {
    Identity {
        client: None,
        groups: vec![group.into()],
        ..person(name)
    }
}

/// `project-readers` read everything in helsinki; `bikes-readers` read only the space `bikes`;
/// `pipeline-readers` read only Pipelines in the project.
async fn world() -> AppState {
    let state = common::state_on_forge().await;
    let role = |name: &str, kinds: Value| {
        envelope(
            "Role",
            name,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
        )
    };
    state.mirror.upsert(role(
        "reads-all",
        json!(["ContextSpace", "Endpoint", "Pipeline"]),
    ));
    state
        .mirror
        .upsert(role("reads-pipelines", json!(["Pipeline"])));
    // The space `bikes-readers` are bound to, held by helsinki (T-4589).
    state
        .mirror
        .upsert(envelope("ContextSpace", "bikes", "helsinki", json!({})));
    for (binding, role, group, scope) in [
        (
            "b1",
            "reads-all",
            "project-readers",
            json!({ "project": "helsinki" }),
        ),
        (
            "b2",
            "reads-all",
            "bikes-readers",
            json!({ "contextSpace": "bikes" }),
        ),
        (
            "b3",
            "reads-pipelines",
            "pipeline-readers",
            json!({ "project": "helsinki" }),
        ),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "role": role, "subjects": [{ "group": group }], "scope": scope }),
        ));
    }
    for (name, scope) in [
        ("whole", Scope::Project {}),
        ("air", Scope::Space { name: "air".into() }),
        (
            "endpoints",
            Scope::Resources {
                items: vec![ScopedResource {
                    kind: "Endpoint".into(),
                    name: "all".into(),
                }],
            },
        ),
    ] {
        state
            .workspaces
            .create(Opening {
                name,
                title: None,
                project: "helsinki",
                owner: "jana@hel.fi",
                base_revision: "base1",
                scope,
                ttl_hours: 24,
            })
            .await
            .unwrap();
    }
    state
}

async fn listed(state: &AppState, who: Identity) -> (StatusCode, Vec<String>) {
    let answer = send(state, who, "GET", WS, None).await;
    let body: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
    let names = body["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|w| w["name"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default();
    (answer.status, names)
}

#[tokio::test]
async fn viewer_cannot_list_workspaces_in_project_without_read() {
    let state = world().await;
    let (status, names) = listed(&state, person("nobody")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(names.is_empty());
    let one = send(
        &state,
        person("nobody"),
        "GET",
        &format!("{WS}/whole"),
        None,
    )
    .await;
    assert_eq!(one.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn viewer_cannot_read_another_persons_workspace() {
    let state = world().await;
    // Bound to the space bikes only: neither the whole-project workspace nor the one over air.
    let bikes = grouped("bo", "bikes-readers");
    let (status, names) = listed(&state, bikes.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(names.is_empty(), "{names:?}");
    for name in ["whole", "air", "endpoints"] {
        let answer = send(
            &state,
            bikes.clone(),
            "GET",
            &format!("{WS}/{name}/compare"),
            None,
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{name}: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("jana"),
            "the owner is not named: {}",
            answer.text
        );
    }
    // Reads Pipelines only: not the workspace listing Endpoints, but the project-wide one.
    let (_, names) = listed(&state, grouped("pia", "pipeline-readers")).await;
    assert_eq!(names, ["whole"]);
}

#[tokio::test]
async fn a_project_reader_sees_every_workspace_of_the_project() {
    let state = world().await;
    let (_, mut names) = listed(&state, grouped("pete", "project-readers")).await;
    names.sort();
    assert_eq!(names, ["air", "endpoints", "whole"]);
}

#[tokio::test]
async fn viewer_can_read_own_workspace() {
    let state = world().await;
    // The owner reads the project too (a list needs that), and sees every workspace of theirs,
    // here through the one grant that reads nothing but Pipelines.
    let jana = grouped("jana", "pipeline-readers");
    let (_, mut names) = listed(&state, jana.clone()).await;
    names.sort();
    assert_eq!(names, ["air", "endpoints", "whole"]);
    let one = send(&state, jana, "GET", &format!("{WS}/endpoints"), None).await;
    assert_ne!(one.status, StatusCode::NOT_FOUND, "{}", one.text);
}

#[tokio::test]
async fn a_space_reader_sees_a_workspace_of_resources_in_their_space() {
    let state = world().await;
    state.mirror.upsert(envelope(
        "Endpoint",
        "all",
        "helsinki",
        json!({ "contextSpaceRef": "bikes", "slug": "s", "audience": "organization" }),
    ));
    let (_, names) = listed(&state, grouped("bo", "bikes-readers")).await;
    assert_eq!(names, ["endpoints"], "its one Endpoint lives in bikes");
}

/// T-2562, PF-59, R20: a copy hidden from the caller is not there on the resource routes
/// either: the same 404 as a name nobody holds, nothing read.
#[tokio::test]
async fn a_hidden_copy_is_not_read_through_the_workspace_query() {
    let state = world().await;
    let bikes = grouped("bo", "bikes-readers");
    for plural in ["spaces", "pipelines", "endpoints"] {
        let hidden = send(
            &state,
            bikes.clone(),
            "GET",
            &format!("/api/v1/projects/helsinki/{plural}?workspace=whole"),
            None,
        )
        .await;
        let nobody = send(
            &state,
            bikes.clone(),
            "GET",
            &format!("/api/v1/projects/helsinki/{plural}?workspace=nobody-has-this"),
            None,
        )
        .await;
        assert_eq!(
            hidden.status,
            StatusCode::NOT_FOUND,
            "{plural}: {}",
            hidden.text
        );
        assert_eq!(
            hidden.text.replace("whole", "…"),
            nobody.text.replace("nobody-has-this", "…"),
            "{plural}"
        );
    }
    // The owner and a project reader still read it.
    for who in [
        grouped("jana", "pipeline-readers"),
        grouped("pete", "project-readers"),
    ] {
        let answer = send(
            &state,
            who,
            "GET",
            "/api/v1/projects/helsinki/pipelines?workspace=whole",
            None,
        )
        .await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    }
}
