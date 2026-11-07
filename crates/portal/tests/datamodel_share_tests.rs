//! DM-77, PF-58 (T-2884): a project shares a published model with the organization as a red-lane
//! Change of the organization repository that only an organization-scope approver approves.

mod common;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{encode, envelope, person, send, REPO};
use joinedcontext_portal::api::changes::{approve_change_for, ApprovedBy};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::gitea::committed_text;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const SHARE: &str = "/api/v1/projects/helsinki/datamodels/air/share";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const SOURCE_PATH: &str = "projects/helsinki/spaces/ilma/datamodels/air.linkml.yaml";
const ORG_SOURCE_PATH: &str = "datamodels/air/air.linkml.yaml";

/// The project's model, as its repository holds it: a comment and key order the copy keeps.
const SOURCE: &str = "# the city's air, as Helsinki models it
id: https://hel.fi/models/air
name: air
prefixes:
  aq: https://hel.fi/aq/
default_prefix: aq
imports:
  - linkml:types
classes:
  AirQualityObserved:
    class_uri: aq:AirQualityObserved
    slots: [pm10, no2]
slots:
  pm10: { range: float, slot_uri: aq:pm10 }
  no2: { range: float, slot_uri: aq:no2 }
";

const COMPILED: &str = r##"{
  "jsonSchema": {"title": "AirQualityObserved"},
  "context": {"@context": {"pm10": "https://hel.fi/aq/pm10"}},
  "docs": "# air",
  "example": {"id": "urn:ngsi-ld:AirQualityObserved:hel:ilma:1", "type": "AirQualityObserved"},
  "generatorVersion": "linkml-1.11.1"
}"##;

/// A forge that takes every write and holds the project's source, and a Model Tools that
/// compiles anything.
async fn world() -> (MockServer, MockServer, AppState) {
    let gitea = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{SOURCE_PATH}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-src", "content": encode(SOURCE)
        })))
        .with_priority(1)
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main", "commit": { "id": COMMIT }
        })))
        .mount(&gitea)
        .await;
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(COMPILED, "application/json"))
        .mount(&tools)
        .await;
    let base = common::state_on(&gitea);
    let state = AppState::new(
        Config {
            model_tools_url: Some(tools.uri()),
            ..Config::for_tests()
        },
        None,
    )
    .with_gitea(base.gitea.clone().expect("forge"));

    let every = json!(["read", "propose", "approve", "delete"]);
    for (name, verbs) in [("model-admin", every.clone()), ("model-steward", every)] {
        state.mirror.upsert(envelope(
            "Role",
            name,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["DataModel"], "verbs": verbs }] }),
        ));
    }
    for (holder, role, scope) in [
        ("admin", "model-admin", json!({ "organization": "hel" })),
        ("steward", "model-steward", json!({ "project": "helsinki" })),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{holder}-{role}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{holder}@hel.fi") }], "role": role, "scope": scope }),
        ));
    }
    state
        .mirror
        .upsert(envelope("ContextSpace", "ilma", "helsinki", json!({})));
    seed_model(&state, "published");
    (gitea, tools, state)
}

fn seed_model(state: &AppState, lifecycle: &str) {
    state.mirror.upsert(envelope(
        "DataModel",
        "air",
        "helsinki",
        json!({
            "contextSpaceRef": "ilma",
            "linkml": "./air.linkml.yaml",
            "version": "1.2.0",
            "lifecycle": lifecycle,
            "classes": ["AirQualityObserved"]
        }),
    ));
}

/// The organization's own `air`, shared earlier from `origin` (none: made there), at `version`
/// with `source` in the organization repository.
async fn seed_organization_model(
    gitea: &MockServer,
    state: &AppState,
    origin: Option<Value>,
    version: &str,
    source: &str,
) {
    let mut spec = json!({
        "linkml": "./air.linkml.yaml",
        "version": version,
        "lifecycle": "published",
        "classes": ["AirQualityObserved"]
    });
    if let Some(origin) = origin {
        spec["origin"] = origin;
    }
    state
        .mirror
        .upsert(envelope("DataModel", "air", ORG_NAMESPACE, spec));
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{ORG_SOURCE_PATH}")))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-org", "content": encode(source)
        })))
        .with_priority(1)
        .mount(gitea)
        .await;
}

fn same_origin() -> Value {
    json!({ "project": "helsinki", "space": "ilma", "name": "air", "version": "1.0.0", "commit": "abcdef1" })
}

/// Every file the forge was asked to write, by path, decoded: one per-file write or each file
/// of a `POST /contents` commit.
async fn written(gitea: &MockServer) -> Vec<(String, String)> {
    let decode = |content: &Value| {
        let bytes = STANDARD.decode(content.as_str()?).ok()?;
        String::from_utf8(bytes).ok()
    };
    let mut files = Vec::new();
    for request in gitea.received_requests().await.expect("recorded") {
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            continue;
        };
        let at = request.url.path();
        if request.method.as_str() == "POST" && at == format!("{REPO}/contents") {
            for file in body["files"].as_array().into_iter().flatten() {
                if let (Some(path), Some(text)) = (file["path"].as_str(), decode(&file["content"]))
                {
                    files.push((path.to_owned(), text));
                }
            }
        } else if request.method.as_str() == "PUT" {
            if let (Some(path), Some(text)) = (
                at.strip_prefix(&format!("{REPO}/contents/")),
                decode(&body["content"]),
            ) {
                files.push((path.to_owned(), text));
            }
        }
    }
    files
}

fn file<'a>(files: &'a [(String, String)], wanted: &str) -> &'a str {
    files
        .iter()
        .find(|(path, _)| path == wanted)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| {
            panic!(
                "{wanted} not written; wrote {:?}",
                files.iter().map(|f| &f.0).collect::<Vec<_>>()
            )
        })
}

fn opened_a_change(requests: &[wiremock::Request]) -> bool {
    requests
        .iter()
        .any(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/pulls"))
}

#[tokio::test]
async fn a_project_member_proposes_a_red_change_that_copies_the_source_in_the_committed_layout() {
    let (gitea, _tools, state) = world().await;

    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change: Value = serde_json::from_str(&answer.text).expect("change");
    assert_eq!(change["status"]["lane"], "red", "{change}");
    assert_eq!(change["status"]["phase"], "PendingApproval", "{change}");

    let files = written(&gitea).await;
    assert_eq!(
        file(&files, ORG_SOURCE_PATH),
        committed_text(ORG_SOURCE_PATH, SOURCE),
        "the source changed on its way"
    );
    let manifest: Value =
        serde_norway::from_str(file(&files, "datamodels/air/air.yaml")).expect("yaml");
    assert_eq!(manifest["metadata"]["namespace"], ORG_NAMESPACE);
    assert!(
        manifest["spec"].get("contextSpaceRef").is_none(),
        "{manifest}"
    );
    assert_eq!(manifest["spec"]["version"], "1.2.0");
    assert_eq!(manifest["spec"]["lifecycle"], "published");
    assert_eq!(
        manifest["spec"]["origin"],
        json!({ "project": "helsinki", "space": "ilma", "name": "air", "version": "1.2.0", "commit": COMMIT })
    );
    file(&files, "datamodels/air/json-schema/air.v1.json");
    assert!(
        files
            .iter()
            .all(|(path, _)| path.starts_with("datamodels/air/")),
        "a share wrote outside the organization model's folder: {files:?}"
    );
}

#[tokio::test]
async fn a_draft_a_stranger_and_a_model_importing_a_project_model_are_refused_before_any_write() {
    let (gitea, tools, state) = world().await;

    let answer = send(&state, person("stranger"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);

    let answer = send(
        &state,
        person("steward"),
        "POST",
        SHARE,
        Some(json!({ "approve": true })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);

    seed_model(&state, "draft");
    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("only a published model"),
        "{}",
        answer.text
    );
    seed_model(&state, "published");

    // An organization model imports organization models only (DM-76).
    gitea.reset().await;
    common::mount_repository(&gitea, REPO).await;
    let importing = SOURCE.replace(
        "  - linkml:types",
        "  - linkml:types\n  - project.stations.v1",
    );
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{SOURCE_PATH}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-src", "content": encode(&importing)
        })))
        .with_priority(1)
        .mount(&gitea)
        .await;
    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("an organization model imports organization models only"),
        "{}",
        answer.text
    );

    assert!(!opened_a_change(
        &gitea.received_requests().await.expect("recorded")
    ));
    assert!(tools
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}

#[tokio::test]
async fn a_name_the_organization_holds_from_another_origin_is_409_naming_it() {
    let (gitea, _tools, state) = world().await;
    let other = json!({ "project": "espoo", "space": "ilma", "name": "air", "version": "1.0.0", "commit": "abcdef1" });
    seed_organization_model(&gitea, &state, Some(other), "1.0.0", SOURCE).await;

    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("already has a data model 'air', shared from project 'espoo'"),
        "{}",
        answer.text
    );

    // One made in the organization itself has no origin, and keeps its name all the same.
    seed_organization_model(&gitea, &state, None, "1.0.0", SOURCE).await;
    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(!opened_a_change(
        &gitea.received_requests().await.expect("recorded")
    ));
}

#[tokio::test]
async fn a_second_share_from_the_same_origin_proposes_the_next_version() {
    // The organization holds 1.0.0 without `no2`: this share adds a slot.
    let (gitea, _tools, state) = world().await;
    let older = SOURCE
        .replace("    slots: [pm10, no2]\n", "    slots: [pm10]\n")
        .replace("  no2: { range: float, slot_uri: aq:no2 }\n", "");
    seed_organization_model(&gitea, &state, Some(same_origin()), "1.0.0", &older).await;

    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let files = written(&gitea).await;
    let manifest: Value =
        serde_norway::from_str(file(&files, "datamodels/air/air.yaml")).expect("yaml");
    assert_eq!(manifest["spec"]["version"], "1.1.0", "{manifest}");
    assert_eq!(manifest["spec"]["origin"]["version"], "1.2.0");
    assert_eq!(
        file(&files, ORG_SOURCE_PATH),
        committed_text(ORG_SOURCE_PATH, SOURCE)
    );

    // Removing a slot the organization's copy has breaks it: a new major (DM-22).
    let (gitea, _tools, state) = world().await;
    let wider = SOURCE
        .replace("    slots: [pm10, no2]\n", "    slots: [pm10, no2, o3]\n")
        .replace(
            "  no2: { range: float, slot_uri: aq:no2 }\n",
            "  no2: { range: float, slot_uri: aq:no2 }\n  o3: { range: float, slot_uri: aq:o3 }\n",
        );
    seed_organization_model(&gitea, &state, Some(same_origin()), "1.0.0", &wider).await;
    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let files = written(&gitea).await;
    let manifest: Value =
        serde_norway::from_str(file(&files, "datamodels/air/air.yaml")).expect("yaml");
    assert_eq!(manifest["spec"]["version"], "2.0.0", "{manifest}");
    file(&files, "datamodels/air/json-schema/air.v2.json");

    // The same source again changes nothing.
    let (gitea, _tools, state) = world().await;
    seed_organization_model(&gitea, &state, Some(same_origin()), "1.2.0", SOURCE).await;
    let answer = send(&state, person("steward"), "POST", SHARE, None).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("already holds this source, at version 1.2.0"),
        "{}",
        answer.text
    );
    assert!(!opened_a_change(
        &gitea.received_requests().await.expect("recorded")
    ));
}

// --- approving the share (PF-58) -----------------------------------------------------------------

const SHARED: u64 = 0x401;

/// The share's merge request as the organization repository holds it.
async fn share_pull(gitea: &MockServer) {
    let branch = "portal/create-datamodel-air-00000401";
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/{SHARED}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": SHARED,
            "html_url": "https://gitea.example/pulls/1025",
            "state": "open",
            "title": "share DataModel air with the organization",
            "head": { "ref": branch },
            "base": { "ref": "main" },
            "created_at": "2026-09-25T09:00:00Z",
            "user": { "login": "portal", "full_name": "steward", "email": "steward@hel.fi" },
            "mergeable": true,
            "merged": false
        })))
        .mount(gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/{SHARED}/files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "filename": "datamodels/air/air.yaml", "status": "added" }
        ])))
        .mount(gitea)
        .await;
    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataModel",
        "metadata": { "name": "air", "namespace": ORG_NAMESPACE },
        "spec": {
            "linkml": "./air.linkml.yaml",
            "version": "1.2.0",
            "lifecycle": "published",
            "classes": ["AirQualityObserved"],
            "origin": { "project": "helsinki", "space": "ilma", "name": "air", "version": "1.2.0", "commit": COMMIT }
        }
    });
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/datamodels/air/air.yaml")))
        .and(query_param("ref", branch))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-share",
            "content": encode(&serde_norway::to_string(&manifest).expect("yaml"))
        })))
        .with_priority(1)
        .mount(gitea)
        .await;
}

fn merged(requests: &[wiremock::Request]) -> bool {
    requests.iter().any(|r| {
        r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/pulls/{SHARED}/merge")
    })
}

#[tokio::test]
async fn only_an_organization_approver_with_the_name_typed_merges_a_share() {
    let (gitea, _tools, state) = world().await;
    share_pull(&gitea).await;
    let approve = format!("/api/v1/projects/{ORG_NAMESPACE}/changes/chg-{SHARED:08x}/approve");

    // The project's steward approves DataModel changes of helsinki, not the organization's.
    let answer = send(
        &state,
        person("steward"),
        "POST",
        &approve,
        Some(json!({ "confirm": "air" })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);

    let answer = send(&state, person("admin"), "POST", &approve, Some(json!({}))).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(
        answer.text.contains("confirm to be 'air'"),
        "{}",
        answer.text
    );
    assert!(!merged(&gitea.received_requests().await.expect("recorded")));

    let answer = send(
        &state,
        person("admin"),
        "POST",
        &approve,
        Some(json!({ "confirm": "air" })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    assert!(merged(&gitea.received_requests().await.expect("recorded")));
}

#[tokio::test]
async fn a_service_account_or_an_agent_run_never_approves_a_share() {
    let (gitea, _tools, state) = world().await;
    share_pull(&gitea).await;
    let id = format!("chg-{SHARED:08x}");

    // The administrator's own grants, presented by a service account's token.
    let mut bearer = person("admin");
    bearer.client = Some("model-sync".into());
    let refused = approve_change_for(
        &state,
        &bearer,
        ORG_NAMESPACE,
        &id,
        Some("air"),
        ApprovedBy::Person,
    )
    .await
    .expect_err("a service account approved an organization model");
    assert!(
        refused
            .to_string()
            .contains("never with a service account's or an agent's token"),
        "{refused}"
    );

    // An agent run of the administrator holding every verb on Change.
    let run = common::doors::run(person("admin"), &["jc_change_approve"]);
    let answer = common::doors::call_in(
        "jc_change_approve",
        &run,
        &state,
        ORG_NAMESPACE,
        json!({ "id": id, "confirm": "air" }),
    )
    .await;
    assert!(
        answer.text().contains("an agent never approves"),
        "{}",
        answer.text()
    );
    assert!(!merged(&gitea.received_requests().await.expect("recorded")));
}

// DM-75, DM-78: every member reads the organization's copy and where it came from, which is what
// lets the origin project offer to use it; a person with no binding reads neither.
#[tokio::test]
async fn every_member_reads_the_organization_copy_and_its_origin() {
    let (gitea, _tools, state) = world().await;
    seed_organization_model(&gitea, &state, Some(same_origin()), "1.0.0", SOURCE).await;

    let answer = send(
        &state,
        person("steward"),
        "GET",
        "/api/v1/organization/datamodels",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let list: Value = serde_json::from_str(&answer.text).expect("json");
    let shared = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["level"] == "organization")
        .expect("the organization's copy");
    assert_eq!(
        shared["origin"],
        json!({ "project": "helsinki", "space": "ilma", "name": "air", "version": "1.0.0" })
    );

    let source = format!("/api/v1/projects/{ORG_NAMESPACE}/datamodels/air/source");
    let answer = send(&state, person("steward"), "GET", &source, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(answer.text, SOURCE);

    let answer = send(&state, person("stranger"), "GET", &source, None).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
}
