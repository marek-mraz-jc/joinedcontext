//! PL-94, T-4893: a Pipeline reaches only what the person who proposes or approves it reaches.
//! The exploit: `dev` reads entities of space `a` only and may propose Pipelines in helsinki; a
//! pipeline from the Endpoint into space `b` into space `a` would hand the pipeline account's
//! read on `b` to `dev`. Every door refuses it `403` in the same words.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{checked_send as send, encode, envelope, person, Answer, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const FROM_B: &str = "route-b";
const REFUSAL: &str = "No access to source Endpoint ep-b";
/// A merge request someone else opened with the pipeline from `b` into `a`.
const CHANGE: u64 = 0x4893;

/// Who holds what:
/// - `dev`: proposes Pipelines in helsinki, reads entities of space `a` only.
/// - `checker`: approves Pipelines in helsinki, reads entities of space `a` only.
/// - `lead`: approves Pipelines and reads entities in the whole project.
fn world(shako: &MockServer) -> AppState {
    let state = common::state_on(shako);
    for (name, rules) in [
        (
            "pipeline-author",
            json!([{ "kinds": ["Pipeline", "DataSource"], "verbs": ["propose"] }]),
        ),
        (
            "pipeline-approver",
            json!([{ "kinds": ["Pipeline", "DataSource"], "verbs": ["approve"] }]),
        ),
        (
            "entity-reader",
            json!([{ "kinds": ["Entity", "ContextSpace", "Endpoint"], "verbs": ["read"] }]),
        ),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            name,
            ORG_NAMESPACE,
            json!({ "rules": rules }),
        ));
    }
    for (holder, role, scope) in [
        ("dev", "pipeline-author", json!({ "project": PROJECT })),
        ("dev", "entity-reader", json!({ "contextSpace": "a" })),
        (
            "checker",
            "pipeline-approver",
            json!({ "project": PROJECT }),
        ),
        ("checker", "entity-reader", json!({ "contextSpace": "a" })),
        ("lead", "pipeline-approver", json!({ "project": PROJECT })),
        ("lead", "entity-reader", json!({ "project": PROJECT })),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{holder}-{role}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{holder}@hel.fi") }], "role": role, "scope": scope }),
        ));
    }
    for space in ["a", "b"] {
        state.mirror.upsert(envelope(
            "ContextSpace",
            space,
            PROJECT,
            json!({ "isSandbox": false }),
        ));
        state.mirror.upsert(envelope(
            "Endpoint",
            &format!("ep-{space}"),
            PROJECT,
            json!({ "contextSpaceRef": space, "slug": format!("{space}7r2m4xqzvbn3tdw6hcy5pajfe"), "audience": "organization" }),
        ));
    }
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Blueprint".into(),
        metadata: ObjectMeta::new("copy-b", ORG_NAMESPACE),
        spec: json!({
            "version": "1.0.0",
            "category": "pipelines",
            "riskClass": "green",
            "allowedRoles": ["pipeline-author"],
            "parameterSchema": {
                "type": "object",
                "required": ["title"],
                "properties": { "title": { "type": "string" } },
            },
            "templates": [{ "name": "pipeline", "template": BLUEPRINT_PIPELINE }],
        }),
        status: None,
    });
    state
}

const BLUEPRINT_PIPELINE: &str = "apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: {{ title }}
spec:
  class: resident
  source:
    endpointRef:
      kind: Endpoint
      name: ep-b
    query:
      type: Vehicle
  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:a:ep-a
";

/// A Pipeline of helsinki reading the Endpoint `source` and writing through `ep-a`.
fn pipeline(name: &str, source: &str) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha2",
        "kind": "Pipeline",
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": {
            "class": "resident",
            "sources": [{ "endpointRef": { "kind": "Endpoint", "name": source },
                          "query": { "type": "Vehicle" } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:a:ep-a" }],
        }
    })
}

fn refused(answer: &Answer, door: &str) {
    assert_eq!(
        answer.status,
        StatusCode::FORBIDDEN,
        "{door}: {}",
        answer.text
    );
    assert!(
        answer.text.contains(REFUSAL),
        "{door}: expected «{REFUSAL}» in {}",
        answer.text
    );
    assert!(
        !answer.text.contains("space b") && !answer.text.contains("'b'"),
        "{door}: the refusal names the space behind the Endpoint: {}",
        answer.text
    );
}

/// The forge holds change `CHANGE`: someone's pipeline from `b` into `a`.
async fn forge() -> MockServer {
    let shako = common::forge().await;
    let branch = "portal/create-pipeline-route-b-00004893";
    let file = format!("projects/{PROJECT}/pipelines/{FROM_B}/pipeline.yaml");
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests({CHANGE})")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Number": CHANGE,
            "HtmlUrl": format!("https://shako.example/pulls/{CHANGE}"),
            "State": "open",
            "Title": branch,
            "Head": branch,
            "Base": "main",
            "CreatedAt": "2026-10-20T09:00:00Z",
            "Author": "someone",
            "AuthorName": "someone",
            "AuthorEmail": "someone@hel.fi",
            "Mergeable": true,
            "Merged": false
        })))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests({CHANGE})/Files")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "value": [{ "Filename": file, "Status": "added" }] })),
        )
        .mount(&shako)
        .await;
    let yaml = serde_norway::to_string(&pipeline(FROM_B, "ep-b")).expect("yaml");
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", format!("'{file}'")))
        .and(query_param("@r", format!("'{}'", branch)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "sha-4893",
            "Content": encode(&yaml)
        })))
        .with_priority(1)
        .mount(&shako)
        .await;
    shako
}

fn no_change_opened(requests: &[wiremock::Request]) -> bool {
    !requests.iter().any(|request| {
        request.method.as_str() == "POST" && request.url.path().ends_with("/Shako.OpenPullRequest")
    })
}

#[tokio::test]
async fn every_propose_door_refuses_a_source_the_proposer_cannot_read() {
    let shako = forge().await;
    let state = world(&shako);
    let manifest = pipeline(FROM_B, "ep-b");

    let answer = send(
        &state,
        person("dev"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/pipelines"),
        Some(manifest.clone()),
    )
    .await;
    refused(&answer, "resource route");

    // The operations registry is the door the assistant's change tools and MCP clients use.
    let answer = send(
        &state,
        person("dev"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/ops/jc_resource_propose"),
        Some(json!({ "manifest": manifest })),
    )
    .await;
    refused(&answer, "operation");

    let answer = send(
        &state,
        person("dev"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/import"),
        Some(json!({ "manifests": [manifest] })),
    )
    .await;
    refused(&answer, "import");

    let mut blueprint_runner = person("dev");
    blueprint_runner.roles = vec!["pipeline-author".into()];
    let answer = send(
        &state,
        blueprint_runner,
        "POST",
        &format!("/api/v1/projects/{PROJECT}/flows"),
        Some(
            json!({ "blueprint": "copy-b", "version": "1.0.0", "parameters": { "title": FROM_B } }),
        ),
    )
    .await;
    refused(&answer, "blueprint");

    let requests = shako.received_requests().await.expect("requests");
    assert!(
        no_change_opened(&requests),
        "a refused pipeline reached the forge"
    );
}

#[tokio::test]
async fn a_pipeline_within_the_proposers_reads_still_passes_the_door() {
    let shako = forge().await;
    let state = world(&shako);
    let answer = send(
        &state,
        person("dev"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/pipelines"),
        Some(pipeline("route-a", "ep-a")),
    )
    .await;
    assert!(
        !answer.text.contains("No access to"),
        "a pipeline inside dev's reads was refused for them: {}",
        answer.text
    );
}

#[tokio::test]
async fn an_approver_who_cannot_read_the_source_cannot_approve_it() {
    let shako = forge().await;
    let state = world(&shako);
    let approve = format!("/api/v1/projects/{PROJECT}/changes/chg-{CHANGE:08x}/approve");

    let answer = send(&state, person("checker"), "POST", &approve, Some(json!({}))).await;
    refused(&answer, "approve");
    let merged = shako
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .any(|request| request.url.path().ends_with("/Shako.Merge"));
    assert!(!merged, "the refused approval merged");

    let answer = send(&state, person("lead"), "POST", &approve, Some(json!({}))).await;
    assert!(
        !answer.text.contains("No access to"),
        "an approver who reads both spaces was refused: {}",
        answer.text
    );
}
