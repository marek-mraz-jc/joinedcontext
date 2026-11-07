//! Assistant evals (T-2733; AG-87, AG-91, TS-26): one scripted conversation per workflow of
//! `Testing/07-workflow-coverage.md`, in `tests/assistant_evals/<workflow>.yaml`, and for a
//! workflow with cases one conversation cannot cover, more in `<workflow>--<case>.yaml` (T-2742).
//!
//! - The guard: every workflow of the matrix has its conversation, every call a conversation
//!   expects is a tool the conversation has, and a step left to a person says why.
//! - The refusal: a viewer, who may not propose an App, is refused the conversation itself.
//! - The replay: a recording (`recordings/<file>.json`) is the event log of one live run on
//!   dev, written by `ui/e2e/live/assistant-evals.spec.ts` and never by hand, with the tools'
//!   outputs left out. The model's answers are rebuilt from it — the router's pick from the
//!   `path` event, one tool fence per `tool` and `question` event, the last `thought` as the
//!   prose — and played back against this build, with no spend. The live run shows the calls
//!   succeeded on dev; the replay shows this build still makes them from the same answers.
//!   Conversations not yet recorded are counted, and the count only ever falls.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::agents::run::AgentRunEvent;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

const CSRF: &str = "test-csrf-token-assistant-evals";
const PROJECT: &str = "helsinki";

/// The queued pipeline `pipeline--run-now` runs.
const QUEUED_PIPELINE: &str = "traffic-messages";

/// Recordings missing on 2026-09-25, when the conversations were written and no live pass had
/// run yet. It only ever falls: the commit that adds a recording lowers it.
const UNRECORDED_ON_2026_09_25: usize = 23;

/// Case conversations (`<workflow>--<case>.yaml`) missing a recording on 2026-10-06, when the
/// first ones were written (T-2742). It only ever falls, like the count above.
const UNRECORDED_CASES_ON_2026_10_06: usize = 5;

/// Workflows the matrix gained after 2026-09-25, each still owing its first recording, with the
/// task that added it. They are counted apart, so the 23 of that day can still only fall; an entry
/// goes the moment its recording lands, and the guard below refuses one that is recorded.
const UNRECORDED_LATER: [(&str, &str); 8] = [
    ("knowledge", "T-3057"),
    ("navigation", "T-3238"),
    ("feedback", "T-3272"),
    ("personal-data", "T-4668"),
    ("pipeline-access", "T-4863"),
    ("mqttbroker", "T-4880"),
    ("endpoint-access", "T-5615"),
    ("grafana", "T-6509"),
];

/// Case conversations added after 2026-10-06, each owing its recording with the task that added
/// it, counted apart like [`UNRECORDED_LATER`].
const UNRECORDED_CASES_LATER: [(&str, &str); 17] = [
    ("datamodel--enum-slot", "T-6530"),
    ("datasource--api-key", "T-6488"),
    ("endpoint--unknown-class", "T-6530"),
    ("endpoint--hide-attribute", "T-6488"),
    ("pipeline--daily-kpi", "T-6532"),
    ("pipeline--from-columns", "T-6532"),
    ("pipeline--sample-source", "T-6489"),
    ("pipeline--several-resources", "T-6531"),
    ("pipeline--wrote-nothing", "T-6489"),
    ("project--new-role", "T-6489"),
    ("sharing--embed", "T-6489"),
    ("space--quality-rule", "T-6489"),
    ("space--retention", "T-6530"),
    ("space--several-resources", "T-6531"),
    ("syncsource--pause", "T-6533"),
    ("project--remove-member", "T-6533"),
    ("endpoint--what-it-shows", "T-6533"),
];

/// What separates a workflow from its case in a conversation's file name.
const CASE: &str = "--";

/// The workflow a conversation file belongs to: its name, or the part before [`CASE`].
fn workflow_of(file: &str) -> &str {
    file.split_once(CASE).map_or(file, |(workflow, _)| workflow)
}
/// The conversation's own tools beside the registry's operations (`oneshot::conversation`).
const CONVERSATION_TOOLS: [&str; 7] = [
    "change_resource",
    "draft_kpi_pipeline",
    "search_catalog",
    "describe_tool",
    "jc_ask",
    "jc_ui_navigate",
    "jc_switch_path",
];

/// The start of the router's system prompt (`CHOOSE_PATH_SYSTEM`): its call is answered apart.
const ROUTER: &str = "You route a person's message";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Conversation {
    workflow: String,
    #[serde(rename = "as")]
    who: String,
    says: Vec<String>,
    expect: Expect,
    refusal: Refusal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    calls: Vec<Call>,
    outcome: String,
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    tool: String,
    /// Fields the call's input carries, compared as a subset.
    #[serde(default)]
    input: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Refusal {
    #[serde(rename = "as")]
    who: String,
    says: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Recording {
    workflow: String,
    run: String,
    recorded_at: String,
    model: String,
    events: Vec<Recorded>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    kind: String,
    payload: Value,
}

/// What the model answered in one run, rebuilt from its events.
#[derive(Debug)]
struct Script {
    router: String,
    turns: Vec<String>,
    answers: Vec<Value>,
}

fn portal_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn conversations() -> BTreeMap<String, Conversation> {
    let dir = portal_dir().join("tests/assistant_evals");
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("tests/assistant_evals") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let conversation: Conversation =
            serde_norway::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        assert_eq!(
            conversation.workflow,
            workflow_of(stem),
            "{}: the file is named after its workflow, or its workflow{CASE}its case",
            path.display()
        );
        found.insert(stem.to_owned(), conversation);
    }
    found
}

fn recording(workflow: &str) -> Option<Recording> {
    let path = portal_dir().join(format!("tests/assistant_evals/recordings/{workflow}.json"));
    let text = std::fs::read_to_string(&path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display())))
}

/// The workflows of the matrix, as `ui/tests/workflow_coverage.json` records them (T-2728).
fn workflows() -> Vec<String> {
    let text = std::fs::read_to_string(portal_dir().join("ui/tests/workflow_coverage.json"))
        .expect("ui/tests/workflow_coverage.json");
    let coverage: Value = serde_json::from_str(&text).expect("json");
    coverage["workflows"]
        .as_object()
        .expect("workflows")
        .keys()
        .cloned()
        .collect()
}

fn config(proxy_base: &str) -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some(proxy_base),
            "JC_AGENT_PROXY_TOKEN" => Some("the-token-only-jc-agent-proxy-has"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.com"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

/// The demo's people: the steward holds the bootstrap administrators' role, the viewer nothing
/// that proposes.
fn cookie(config: &Config, who: &str) -> String {
    let now = session::now_unix();
    let email = format!("demo.{who}@hel.fi");
    let roles = if who == "steward" {
        vec!["portal-approver".to_owned()]
    } else {
        Vec::new()
    };
    let s = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{email}"),
            username: email.clone(),
            email: Some(email),
            name: None,
            roles,
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

/// The organization, the profile the docked assistant runs on, and one space to talk about. The
/// profile grants every operation and every kind: what dev's profile grants is the live run's to
/// show, the replay only whether this build still calls what the model asked for.
fn mirror() -> Arc<Mirror> {
    let mirror = Mirror::new();
    mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    let operations: Vec<&str> = ops::registry().iter().map(|op| op.name).collect();
    let kinds: Vec<Value> = [
        "ContextSpace",
        "Endpoint",
        "DataSource",
        "Pipeline",
        "Policy",
        "Dashboard",
        "Layer",
        "App",
        "DataModel",
        "RoleBinding",
        "SyncSource",
        "ContextSourceRegistration",
        "CkanInstance",
        "ServiceAccount",
    ]
    .iter()
    .map(|kind| json!({ "kind": kind, "verbs": ["read", "propose"] }))
    .collect();
    mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        ORG_NAMESPACE,
        json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": "openai-compatible", "name": "google/gemini-3.8-flash", "maxTokensPerRun": 400000 },
            "limits": { "stepsPerRun": 120, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
            "egress": { "allowedHosts": [] },
            "tools": ["shell"],
            "access": { "operations": operations, "kinds": kinds },
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        }),
    ));
    mirror.upsert(envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({ "isSandbox": false }),
    ));
    mirror.upsert(envelope("Pipeline", QUEUED_PIPELINE, PROJECT, json!({})));
    Arc::new(mirror)
}

async fn post(
    state: &AppState,
    config: &Config,
    who: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie(config, who))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
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

async fn start(state: &AppState, config: &Config, who: &str, message: &str) -> (StatusCode, Value) {
    post(
        state,
        config,
        who,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        json!({ "message": message }),
    )
    .await
}

/// A model that answers the router with the script's pick and every other call with the next
/// turn, then the last one again.
struct Played {
    router: String,
    turns: Vec<String>,
    next: AtomicUsize,
}

impl Respond for Played {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let content = if String::from_utf8_lossy(&request.body).contains(ROUTER) {
            self.router.clone()
        } else {
            let at = self.next.fetch_add(1, Ordering::SeqCst);
            self.turns
                .get(at)
                .or_else(|| self.turns.last())
                .cloned()
                .unwrap_or_default()
        };
        ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-eval", "object": "chat.completion",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }],
            "usage": { "total_tokens": 900 }
        }))
    }
}

fn fence(value: &Value) -> String {
    format!("```json\n{value}\n```")
}

/// The events as the recorder keeps them: kind and payload, without what a tool answered.
fn kept(events: &[AgentRunEvent]) -> Vec<Recorded> {
    events
        .iter()
        .map(|e| {
            let mut payload = e.payload.clone();
            if let Value::Object(fields) = &mut payload {
                fields.remove("output");
            }
            Recorded {
                kind: e.kind.clone(),
                payload,
            }
        })
        .collect()
}

/// The search the conversation runs on the person's words before it asks the model anything.
fn is_first_search(event: &Recorded, message: &str) -> bool {
    event.kind == "tool"
        && event.payload["tool"] == "search_catalog"
        && event.payload["input"]["q"] == message
}

/// The model's answers of one run, rebuilt from its events.
fn rebuild(message: &str, events: &[Recorded]) -> Script {
    let router = events
        .iter()
        .find(|e| e.kind == "path" && e.payload["by"] == "router")
        .map_or_else(
            || json!({ "path": null }).to_string(),
            |e| json!({ "path": e.payload["path"], "reason": e.payload["reason"] }).to_string(),
        );
    let mut turns = Vec::new();
    let mut answers = Vec::new();
    let mut searched_first = false;
    for (at, event) in events.iter().enumerate() {
        match event.kind.as_str() {
            "tool" if !searched_first && is_first_search(event, message) => searched_first = true,
            // The driver reads it itself after a parse refusal: the model never asked (T-6488).
            "tool" if event.payload["tool"] == ops::schemas::GET => {}
            "tool" => {
                let tool = event.payload["tool"].as_str().unwrap_or_default();
                let input = event.payload["input"].clone();
                turns.push(fence(&if tool.starts_with("jc_") {
                    json!({ "tool": tool, "arguments": input })
                } else {
                    let mut call = if input.is_object() { input } else { json!({}) };
                    call["tool"] = json!(tool);
                    call
                }));
            }
            "path" if event.payload["by"] == "handover" => turns.push(fence(&json!({
                "tool": "jc_switch_path",
                "arguments": { "path": event.payload["path"], "reason": event.payload["reason"] }
            }))),
            // A question with a step is the Portal's own: the model's call asks it again.
            "question" if event.payload["step"].is_string() => {}
            "question" => {
                // The question's words are the thought that follows it (`ask_person`).
                let question = events[at + 1..]
                    .iter()
                    .find(|e| e.kind == "thought")
                    .and_then(|e| e.payload["text"].as_str())
                    .unwrap_or_default();
                let q = &event.payload;
                let mut arguments = json!({ "question": question });
                if q["pick"].is_string() {
                    arguments["pick"] = q["pick"].clone();
                } else {
                    let values: Vec<Value> = q["options"]
                        .as_array()
                        .map(|options| options.iter().map(|o| o["value"].clone()).collect())
                        .unwrap_or_default();
                    arguments["options"] = json!(values);
                }
                for key in ["multiple", "min", "max", "default"] {
                    if !q[key].is_null() {
                        arguments[key] = q[key].clone();
                    }
                }
                turns.push(fence(&json!({ "tool": "jc_ask", "arguments": arguments })));
            }
            "answer" => answers.push(event.payload["answers"].clone()),
            _ => {}
        }
    }
    if let Some(text) = events
        .iter()
        .rev()
        .find(|e| e.kind == "thought")
        .and_then(|e| e.payload["text"].as_str())
    {
        turns.push(text.to_owned());
    }
    Script {
        router,
        turns,
        answers,
    }
}

/// Plays `script` for `message` as the steward and answers the run's questions with the
/// script's answers, in order; returns the run's events once it has gone quiet after answering.
async fn play(message: &str, script: Script) -> Vec<AgentRunEvent> {
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .respond_with(Played {
            router: script.router,
            turns: script.turns,
            next: AtomicUsize::new(0),
        })
        .mount(&proxy)
        .await;
    let config = config(&proxy.uri());
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let (status, created) = start(&state, &config, "steward", message).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let run = created["id"].as_str().expect("run id").to_owned();

    let mut answers = script.answers.into_iter();
    let (mut seen, mut quiet) = (0, 0);
    for _ in 0..400 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let events = state.agents.events_since(&run, 0).await.expect("events");
        if events.len() != seen {
            (seen, quiet) = (events.len(), 0);
            continue;
        }
        quiet += 1;
        let asked = events
            .iter()
            .rposition(|e| e.kind == "question")
            .filter(|at| !events[*at..].iter().any(|e| e.kind == "answer"));
        if let Some(at) = asked {
            let Some(given) = answers.next() else {
                return events;
            };
            let (status, body) = post(
                &state,
                &config,
                "steward",
                &format!("/api/v1/projects/{PROJECT}/agent-runs/{run}/answers"),
                json!({ "questionId": events[at].payload["questionId"], "answers": given }),
            )
            .await;
            assert!(status.is_success(), "the answer was refused: {body}");
            continue;
        }
        // Quiet for half a second with something said: a draft opened for the person ends on
        // its `navigate`, an answer on its `thought`.
        if quiet >= 20 && events.iter().any(|e| e.kind == "thought") {
            return events;
        }
    }
    panic!(
        "the run never settled: {:?}",
        state.agents.events_since(&run, 0).await
    );
}

fn tools<'a>(events: impl IntoIterator<Item = &'a Recorded>, message: &str) -> Vec<&'a Recorded> {
    let mut searched_first = false;
    events
        .into_iter()
        .filter(|e| e.kind == "tool")
        .filter(|e| {
            let first = !searched_first && is_first_search(e, message);
            searched_first |= first;
            !first
        })
        .collect()
}

/// `want`'s fields, and every one of them equal, in `got`.
fn carries(got: &Value, want: &Value) -> bool {
    match (got, want) {
        (Value::Object(got), Value::Object(want)) => want
            .iter()
            .all(|(key, value)| got.get(key).is_some_and(|g| carries(g, value))),
        // An expected list is matched item by item: each one some item of the call's carries
        // (T-2742: one relationship among a call's operations).
        (Value::Array(got), Value::Array(want)) => {
            want.iter().all(|w| got.iter().any(|g| carries(g, w)))
        }
        _ => got == want,
    }
}

fn made(called: &[&Recorded], call: &Call) -> bool {
    called.iter().any(|e| {
        e.payload["tool"] == call.tool.as_str()
            && call
                .input
                .as_ref()
                .is_none_or(|want| carries(&e.payload["input"], want))
    })
}

/// A call that changes something: a draft opened for the person, or an operation that writes.
fn acts(tool: &str) -> bool {
    tool == "change_resource" || ops::find(tool).is_some_and(|op| !op.annotations.read_only_hint)
}

#[test]
fn every_workflow_has_its_conversation_and_every_expected_call_is_a_tool() {
    let conversations = conversations();
    let workflows = workflows();
    let missing: Vec<&String> = workflows
        .iter()
        .filter(|w| !conversations.contains_key(*w))
        .collect();
    // A case goes beside its workflow's one conversation, never instead of it.
    assert!(
        missing.is_empty(),
        "workflows with no conversation: {missing:?}"
    );
    // A workflow counted apart is one of the matrix, has its conversation, and names its task.
    for (workflow, task) in UNRECORDED_LATER {
        assert!(
            workflows.iter().any(|w| w == workflow) && conversations.contains_key(workflow),
            "UNRECORDED_LATER names {workflow}, which is no workflow with a conversation"
        );
        assert!(
            task.len() == 6
                && task.starts_with("T-")
                && task[2..].bytes().all(|b| b.is_ascii_digit()),
            "UNRECORDED_LATER: {task} is no task id"
        );
    }
    for (name, conversation) in &conversations {
        assert!(
            workflows.iter().any(|w| w == workflow_of(name)),
            "{name}.yaml names no workflow of the matrix"
        );
        assert!(
            !conversation.says.is_empty(),
            "{name}: the person says nothing"
        );
        assert_eq!(
            conversation.who, "steward",
            "{name}: the conversation is the steward's"
        );
        assert_eq!(
            conversation.refusal.who, "viewer",
            "{name}: the refusal is the viewer's"
        );
        for call in &conversation.expect.calls {
            assert!(
                ops::find(&call.tool).is_some() || CONVERSATION_TOOLS.contains(&call.tool.as_str()),
                "{name}: {} is no tool of the conversation",
                call.tool
            );
        }
        match conversation.expect.outcome.as_str() {
            "change" | "form" | "answer" => assert!(
                !conversation.expect.calls.is_empty(),
                "{name}: a {} comes from a call, so the conversation names it",
                conversation.expect.outcome
            ),
            "person-only" => assert!(
                conversation
                    .expect
                    .why
                    .as_deref()
                    .is_some_and(|why| why.len() >= 20),
                "{name}: a step the assistant leaves to a person says why"
            ),
            other => panic!("{name}: outcome {other} is change, form, answer or person-only"),
        }
    }
}

/// T-2742: a case file belongs to its workflow, and an expected list is matched item by item.
#[test]
fn a_case_belongs_to_its_workflow_and_a_list_matches_by_its_items() {
    assert_eq!(workflow_of("datamodel"), "datamodel");
    assert_eq!(workflow_of("datamodel--one-to-many"), "datamodel");
    let call = json!({ "kind": "DataModel", "operations": [
        { "op": "addSlot", "name": "x" },
        { "op": "addRelationship", "from": "CityDistrict", "to": "PointOfInterest", "cardinality": "one-to-many" }
    ] });
    assert!(carries(
        &call,
        &json!({ "operations": [{ "op": "addRelationship", "cardinality": "one-to-many" }] })
    ));
    assert!(!carries(
        &call,
        &json!({ "operations": [{ "op": "addRelationship", "cardinality": "many-to-many" }] })
    ));
    assert!(!carries(
        &json!({ "operations": [] }),
        &json!({ "operations": [{ "op": "addRelationship" }] })
    ));
    assert!(
        carries(&call, &json!({ "operations": [] })),
        "an empty list asks for nothing"
    );
}

fn recorded(kind: &str, payload: Value) -> Recorded {
    Recorded {
        kind: kind.to_owned(),
        payload,
    }
}

#[test]
fn a_rebuilt_answer_is_the_call_the_event_shows() {
    let events = [
        recorded(
            "tool",
            json!({ "tool": "search_catalog", "input": { "q": "hi" }, "status": "ok" }),
        ),
        recorded(
            "path",
            json!({ "path": "share-data", "by": "router", "reason": "share" }),
        ),
        recorded(
            "tool",
            json!({ "tool": "search_catalog", "input": { "q": "air" }, "status": "ok" }),
        ),
        recorded(
            "tool",
            json!({ "tool": "jc_resource_get", "input": { "kind": "ContextSpace", "name": "air" }, "status": "failed" }),
        ),
        recorded(
            "question",
            json!({ "questionId": "q-1", "options": [{ "value": "a" }, { "value": "b" }], "pick": null, "multiple": null, "min": null, "max": null, "default": null }),
        ),
        recorded("thought", json!({ "text": "Which one?" })),
        recorded(
            "answer",
            json!({ "questionId": "q-1", "answers": { "answer": "a" } }),
        ),
        recorded("thought", json!({ "text": "Done." })),
    ];
    let script = rebuild("hi", &events);
    assert_eq!(script.router, r#"{"path":"share-data","reason":"share"}"#);
    assert_eq!(
        script.turns,
        [
            fence(&json!({ "q": "air", "tool": "search_catalog" })),
            fence(
                &json!({ "tool": "jc_resource_get", "arguments": { "kind": "ContextSpace", "name": "air" } })
            ),
            fence(
                &json!({ "tool": "jc_ask", "arguments": { "question": "Which one?", "options": ["a", "b"] } })
            ),
            "Done.".to_owned(),
        ],
        "the first search is the conversation's own, never a turn of the model"
    );
    assert_eq!(script.answers, [json!({ "answer": "a" })]);
    let empty = rebuild("hi", &[]);
    assert!(empty.turns.is_empty(), "no events, no turns");
    assert_eq!(empty.router, r#"{"path":null}"#);
}

#[tokio::test]
async fn a_run_replayed_from_its_own_events_makes_the_same_calls() {
    // A run of this build stands in for a live one: its events, kept as the recorder keeps
    // them, must play back to the same calls, or a recording of dev could not be trusted. A
    // draft opened for the person ends the turn, so it is the script's last call.
    let message = "Draft a space eval-air for air quality.";
    let scripted = Script {
        router: json!({ "path": null }).to_string(),
        turns: vec![
            fence(
                &json!({ "tool": "jc_resource_get", "arguments": { "kind": "ContextSpace", "name": "air" } }),
            ),
            fence(
                &json!({ "tool": "jc_ask", "arguments": { "question": "Which unit?", "options": ["ug", "ppm"] } }),
            ),
            fence(
                &json!({ "tool": "change_resource", "kind": "ContextSpace", "name": "eval-air", "create": true, "patch": { "spec": { "isSandbox": false } } }),
            ),
            "Opened the draft of eval-air for you.".to_owned(),
        ],
        answers: vec![json!({ "answer": "ug" })],
    };
    let live = kept(&play(message, scripted).await);
    let first: Vec<&Value> = tools(&live, message)
        .iter()
        .map(|e| &e.payload["tool"])
        .collect();
    assert_eq!(
        first,
        ["jc_resource_get", "change_resource"],
        "the scripted run itself: {live:?}"
    );
    assert!(
        live.iter().any(|e| e.kind == "answer"),
        "the question was answered"
    );

    let replayed = kept(&play(message, rebuild(message, &live)).await);
    let inputs = |events: &[Recorded]| -> Vec<Value> {
        tools(events, message)
            .iter()
            .map(|e| json!([e.payload["tool"], e.payload["input"]]))
            .collect()
    };
    assert_eq!(inputs(&replayed), inputs(&live), "{replayed:?}");
}

#[tokio::test]
async fn a_viewer_is_refused_every_conversation() {
    let proxy = MockServer::start().await;
    let config = config(&proxy.uri());
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    for (name, conversation) in conversations() {
        let (status, body) = start(&state, &config, "viewer", &conversation.refusal.says).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{name}: {body}");
        assert!(
            body.to_string().contains("propose"),
            "{name}: the refusal names what is missing: {body}"
        );
    }
    assert!(
        proxy
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a refused conversation reached the model"
    );
}

#[tokio::test]
async fn a_question_the_portal_asks_replays_through_the_models_call() {
    let event = |kind: &str, payload: Value| Recorded {
        kind: kind.to_owned(),
        payload,
    };
    let message = "Run pipeline traffic-messages now.";
    let input = json!({ "name": QUEUED_PIPELINE });
    let call = json!({ "tool": "jc_pipeline_run", "input": input, "status": "ok" });
    let script = rebuild(
        message,
        &[
            event("question", json!({ "step": "run-now", "options": [] })),
            event(
                "thought",
                json!({ "text": "Run pipeline traffic-messages now?" }),
            ),
            event(
                "answer",
                json!({ "answers": { "answer": format!("run:{QUEUED_PIPELINE}") } }),
            ),
            event("tool", call),
            event(
                "thought",
                json!({ "text": "Pipeline traffic-messages is queued." }),
            ),
        ],
    );
    assert_eq!(
        script.turns.first(),
        Some(&fence(&json!({
            "tool": "jc_pipeline_run",
            "arguments": input
        })))
    );
    assert!(
        script.turns.iter().all(|turn| !turn.contains("jc_ask")),
        "{:?}",
        script.turns
    );
    let replayed = kept(&play(message, script).await);
    assert!(
        replayed
            .iter()
            .any(|e| e.kind == "question" && e.payload["step"] == "run-now"),
        "the Portal did not ask: {replayed:?}"
    );
    assert!(
        replayed
            .iter()
            .skip_while(|e| e.kind != "question")
            .any(|e| e.kind == "tool" && e.payload["tool"] == "jc_pipeline_run"),
        "the person's answer ran nothing: {replayed:?}"
    );
}

#[tokio::test]
async fn every_recorded_conversation_did_on_dev_and_still_does_what_it_expects() {
    let (mut unrecorded, mut unrecorded_cases) = (0, 0);
    let later = |name: &str| {
        UNRECORDED_LATER
            .iter()
            .chain(&UNRECORDED_CASES_LATER)
            .any(|(workflow, _)| *workflow == name)
    };
    for (name, conversation) in conversations() {
        let Some(recorded) = recording(&name) else {
            if later(&name) {
                continue;
            }
            if name.contains(CASE) {
                unrecorded_cases += 1;
            } else {
                unrecorded += 1;
            }
            continue;
        };
        assert!(
            !later(&name),
            "{name} is recorded now: remove it from UNRECORDED_LATER"
        );
        assert_eq!(
            recorded.workflow, conversation.workflow,
            "recordings/{name}.json is another workflow's"
        );
        assert!(
            !recorded.run.is_empty()
                && !recorded.recorded_at.is_empty()
                && !recorded.model.is_empty(),
            "{name}: a recording names its run, its time and its model"
        );
        let message = &conversation.says[0];
        let on_dev = tools(&recorded.events, message);
        let succeeded: Vec<&Recorded> = on_dev
            .iter()
            .copied()
            .filter(|e| e.payload["status"] == "ok")
            .collect();
        for call in &conversation.expect.calls {
            assert!(
                made(&succeeded, call),
                "{name}: on dev, {} did not succeed: {on_dev:?}",
                call.tool
            );
        }
        if conversation.expect.outcome == "person-only" {
            let acting: Vec<&Value> = on_dev
                .iter()
                .map(|e| &e.payload["tool"])
                .filter(|t| t.as_str().is_some_and(acts))
                .collect();
            assert!(
                acting.is_empty(),
                "{name}: a person's step was taken by {acting:?}"
            );
        }

        let replayed = kept(&play(message, rebuild(message, &recorded.events)).await);
        let now = tools(&replayed, message);
        for call in &conversation.expect.calls {
            assert!(
                made(&now, call),
                "{name}: this build no longer calls {}: {now:?}",
                call.tool
            );
        }
    }
    assert!(
        unrecorded <= UNRECORDED_ON_2026_09_25,
        "{unrecorded} conversations have no recording, more than the {UNRECORDED_ON_2026_09_25} of 2026-09-25"
    );
    assert!(
        unrecorded_cases <= UNRECORDED_CASES_ON_2026_10_06,
        "{unrecorded_cases} case conversations have no recording, more than the {UNRECORDED_CASES_ON_2026_10_06} of 2026-10-06"
    );
}
