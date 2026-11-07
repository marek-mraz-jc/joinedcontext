//! The four doors of one operation (AG-64, PF-50): a person's session, an MCP client, an agent
//! run, and the REST route of the same action. The `ops_*_tests.rs` files call each operation
//! through every door as a steward, a viewer and a stranger, and compare what the doors answer.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use joinedcontext_portal::agents::access::Access;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::{self, Caller, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};

use super::{envelope, send};

pub const PROJECT: &str = "ovzdusie";

/// The kinds the steward and the viewer are bound on.
const KINDS: &[&str] = &[
    "ContextSpace",
    "Endpoint",
    "Pipeline",
    "Policy",
    "DataSource",
    "DataModel",
    "App",
    "Change",
    "ServiceAccount",
    "SyncSource",
    "Project",
    "Workspace",
    "Flow",
];

pub fn member(username: &str, group: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{username}"),
        username: username.to_owned(),
        email: Some(format!("{username}@banskabystrica.sk")),
        name: Some(username.to_owned()),
        roles: Vec::new(),
        groups: vec![group.to_owned()],
        second_factor: false,
    }
}

/// May read, propose, approve and delete every kind of [`PROJECT`].
pub fn steward() -> Identity {
    member("jana", "city-stewards")
}

/// May read every kind of [`PROJECT`] and change nothing.
pub fn viewer() -> Identity {
    member("peter", "city-viewers")
}

/// Signed in, with no binding in [`PROJECT`] at all.
pub fn stranger() -> Identity {
    member("eva", "another-city")
}

/// A state whose organization binds the steward and the viewer in [`PROJECT`].
pub fn state() -> AppState {
    state_with(AppState::new(Config::for_tests(), None))
}

/// `state` with the bindings of [`state`] put into its mirror.
pub fn state_with(state: AppState) -> AppState {
    let state = state.with_mirror(Arc::new(Mirror::new()));
    bind(&state);
    state
}

/// The organization's roles and bindings, into `state`'s mirror.
pub fn bind(state: &AppState) {
    for (role, verbs) in [
        (
            "steward-role",
            json!(["read", "propose", "approve", "delete"]),
        ),
        ("viewer-role", json!(["read"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": KINDS, "verbs": verbs }] }),
        ));
    }
    for (binding, group, role) in [
        ("steward-binding", "city-stewards", "steward-role"),
        ("viewer-binding", "city-viewers", "viewer-role"),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "group": group }],
                "role": role,
                "scope": { "project": PROJECT },
            }),
        ));
    }
}

/// A person at the keyboard.
pub fn session(identity: Identity) -> Caller {
    Caller::new(identity, Via::Session)
}

/// The same person through an MCP client.
pub fn mcp(identity: Identity) -> Caller {
    Caller::new(identity, Via::Mcp)
}

/// An agent run of `identity` whose profile names `operations`, with every verb on the kinds they
/// touch.
pub fn run(identity: Identity, operations: &[&str]) -> Caller {
    let kinds: Vec<Value> = operations
        .iter()
        .filter_map(|name| ops::find(name))
        // A kind-`*` operation is granted per kind its input names (T-6366): these tests name a
        // Pipeline.
        .map(|op| if op.kind == "*" { "Pipeline" } else { op.kind })
        .map(|kind| json!({ "kind": kind, "verbs": ["read", "propose", "approve", "delete"] }))
        .collect();
    Caller::for_run(
        identity,
        Access::from_spec(&json!({ "access": { "operations": operations, "kinds": kinds } })),
    )
}

/// What a door answered: the status and the body as JSON (`Null` when it is not JSON).
#[derive(Debug)]
pub struct Answer {
    pub status: StatusCode,
    pub body: Value,
}

impl Answer {
    pub fn text(&self) -> String {
        self.body.to_string()
    }
}

/// `name` through the operation registry, as `caller` (the session, MCP and agent doors).
pub async fn call(name: &str, caller: &Caller, state: &AppState, input: Value) -> Answer {
    call_in(name, caller, state, PROJECT, input).await
}

/// [`call`] in `project`.
pub async fn call_in(
    name: &str,
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: Value,
) -> Answer {
    let op = ops::find(name).unwrap_or_else(|| panic!("{name} is not registered"));
    match ops::call(op, caller, state, project, input).await {
        Ok(body) => Answer {
            status: StatusCode::OK,
            body,
        },
        Err(err) => answer_of(err.into_response()).await,
    }
}

/// A request through the whole router, as `identity`: the REST route, or `POST …/ops/{name}`.
pub async fn http(
    state: &AppState,
    identity: Identity,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let answer = send(state, identity, method, uri, body).await;
    Answer {
        status: answer.status,
        body: serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    }
}

/// `name` through `POST /api/v1/projects/{PROJECT}/ops/{name}`.
pub async fn post_op(state: &AppState, identity: Identity, name: &str, input: Value) -> Answer {
    http(
        state,
        identity,
        "POST",
        &format!("/api/v1/projects/{PROJECT}/ops/{name}"),
        Some(input),
    )
    .await
}

async fn answer_of(response: axum::response::Response) -> Answer {
    use http_body_util::BodyExt;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    Answer {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// The operations `caller` is offered at `GET …/ops` and `tools/list`.
pub fn offered(caller: &Caller, state: &AppState) -> Vec<String> {
    ops::listing(caller, state, PROJECT)
        .into_iter()
        .map(|summary| summary.name)
        .collect()
}

/// One MCP JSON-RPC `message` from `caller` through the Portal's MCP dispatch, answered as JSON.
pub async fn mcp_rpc(caller: &Caller, state: &AppState, message: Value) -> Value {
    let body = axum::body::Bytes::from(message.to_string());
    let answer = answer_of(
        joinedcontext_portal::mcp::dispatch_for(state.clone(), caller.clone(), body).await,
    )
    .await;
    answer.body
}
