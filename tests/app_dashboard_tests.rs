//! A dashboard App edited without code (T-6402): the editor reads the App's `src/dashboard.json`
//! and the attributes of its type the person may read; an edit is checked on the server (formula
//! parsed, every attribute one the person reads, the grid's source and type unchanged), committed
//! on a branch of the App's repository and proposed as the App's Change at that commit; approving
//! it merges that branch pinned to the commit.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{envelope, forge, mount_repository, person, send, state_on};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "/api/v1/repos/test-owner/sharing";
const APP_REPO: &str = "/api/v1/repos/test-owner/sharing_board";
const ROUTE: &str = "/api/v1/projects/sharing/apps/board/dashboard";
const SERVED: &str = "5d0f3c1f6a3e2b9d8c7a6b5e4d3c2b1a0f9e8d7c";

fn file() -> Value {
    json!({
        "title": "Shared bikes",
        "grid": {
            "source": { "kind": "endpoint", "slug": "bikes" },
            "type": "BikeHireDockingStation",
            "columns": [{ "attr": "name" }, { "attr": "availableBikeNumber" }],
            "history": { "enabled": true },
        },
    })
}

/// The project `sharing` with the App `board` built from its own repository, reading
/// BikeHireDockingStation of the `mobility` space (only `name`, `availableBikeNumber`,
/// `totalSlotNumber`). Ema proposes Apps and reads `mobility`; Peter reads the project and its
/// Apps and proposes nothing; Olga proposes Apps but reads no space.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    mount_repository(&server, PROJECT).await;
    mount_repository(&server, APP_REPO).await;
    let text = serde_json::to_string_pretty(&file()).expect("json") + "\n";
    Mock::given(method("GET"))
        .and(path(format!("{APP_REPO}/contents/src/dashboard.json")))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "d1", "content": STANDARD.encode(text),
        })))
        .with_priority(1)
        .mount(&server)
        .await;

    let state = state_on(&server);
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([(
        "sharing".to_owned(),
        "sharing".to_owned(),
    )]));
    state.mirror.upsert(envelope(
        "Project",
        "sharing",
        ORG_NAMESPACE,
        json!({ "organizationRef": "bb", "repository": { "name": "sharing" }, "ref": "main" }),
    ));
    state
        .mirror
        .upsert(envelope("ContextSpace", "mobility", "sharing", json!({})));
    state.mirror.upsert(envelope(
        "App",
        "board",
        "sharing",
        json!({
            "kind": "static",
            "source": { "git": { "url": "https://forge.example/test-owner/sharing_board.git", "ref": SERVED } },
            "build": { "node": "22" },
            "visibility": "project",
            "lifecycle": "published",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "mobility" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "availableBikeNumber", "totalSlotNumber"],
                "operations": ["retrieveEntity", "queryEntity"],
            }],
        }),
    ));
    state.model_schemas.replace(HashMap::from([(
        ("sharing".to_owned(), "mobility".to_owned()),
        Arc::new(ModelSchema::compile(
            "mobility",
            "1.0.0",
            &json!({ "definitions": { "BikeHireDockingStation": { "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "availableBikeNumber": { "type": "integer" },
                "totalSlotNumber": { "type": "integer" },
                "ownerEmail": { "type": "string" },
            }}}}),
            &["BikeHireDockingStation".to_owned()],
            false,
        )),
    )]));
    for (role, kinds, verbs) in [
        ("app-editor", json!(["App"]), json!(["read", "propose"])),
        (
            "space-reader",
            json!(["ContextSpace", "Endpoint"]),
            json!(["read"]),
        ),
        ("reader", json!(["Project", "App"]), json!(["read"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
        ));
    }
    for (name, who, role, scope) in [
        (
            "ema-edits",
            "ema",
            "app-editor",
            json!({ "project": "sharing" }),
        ),
        (
            "ema-reads",
            "ema",
            "space-reader",
            json!({ "contextSpace": "mobility" }),
        ),
        (
            "ema-project",
            "ema",
            "reader",
            json!({ "project": "sharing" }),
        ),
        (
            "peter-reads",
            "peter",
            "reader",
            json!({ "project": "sharing" }),
        ),
        (
            "peter-space",
            "peter",
            "space-reader",
            json!({ "contextSpace": "mobility" }),
        ),
        (
            "olga-edits",
            "olga",
            "app-editor",
            json!({ "project": "sharing" }),
        ),
        (
            "olga-project",
            "olga",
            "reader",
            json!({ "project": "sharing" }),
        ),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": scope }),
        ));
    }
    (server, state)
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

async fn writes(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

fn edited() -> Value {
    let mut config = file();
    config["grid"]["columns"] = json!([
        { "attr": "name", "label": "Station" },
        { "attr": "occupancy", "label": "Occupancy %", "formula": "availableBikeNumber / totalSlotNumber * 100" },
    ]);
    config["colour"] = json!({ "attr": "occupancy", "bands": [
        { "below": 10, "tone": "danger", "label": "Almost empty" },
        { "tone": "success", "label": "Bikes available" },
    ]});
    config["filters"] = json!([{ "attr": "totalSlotNumber", "op": ">", "value": 0 }]);
    config["chart"] = json!({ "x": "name", "y": "availableBikeNumber", "agg": "sum" });
    config["kpis"] =
        json!([{ "label": "Empty stations", "agg": "count", "where": "availableBikeNumber == 0" }]);
    config
}

#[tokio::test]
async fn the_editor_reads_the_file_and_only_the_attributes_the_person_may_read() {
    let (_server, state) = world().await;
    let answer = send(&state, person("ema"), "GET", ROUTE, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let read = body(&answer.text);
    assert_eq!(read["config"], file());
    assert_eq!(read["entityType"], "BikeHireDockingStation");
    let names: Vec<&str> = read["attributes"]
        .as_array()
        .expect("attributes")
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    // The model's ownerEmail is outside the App's data need, so no dropdown offers it.
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(!names.contains(&"ownerEmail"));
    assert_eq!(read["edit"], json!({ "allowed": true }));

    // A reader sees the file and why Edit is not offered; a person reading no space gets no
    // attribute to pick.
    let peter = body(&send(&state, person("peter"), "GET", ROUTE, None).await.text);
    assert_eq!(peter["edit"]["allowed"], false);
    assert!(
        peter["edit"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("propose on App"),
        "{peter}"
    );
    let olga = body(&send(&state, person("olga"), "GET", ROUTE, None).await.text);
    assert_eq!(olga["attributes"], json!([]));
    assert_eq!(olga["edit"]["allowed"], false);
    // Anyone else does not learn the App exists.
    let stranger = send(&state, person("ivan"), "GET", ROUTE, None).await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
}

#[tokio::test]
async fn a_preview_answers_before_and_after_and_writes_nothing() {
    let (server, state) = world().await;
    let answer = send(
        &state,
        person("ema"),
        "POST",
        ROUTE,
        Some(json!({ "config": edited(), "dryRun": true })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let preview = body(&answer.text);
    assert!(preview["after"]
        .as_str()
        .unwrap_or_default()
        .contains("availableBikeNumber / totalSlotNumber * 100"));
    assert!(!preview["before"]
        .as_str()
        .unwrap_or_default()
        .contains("occupancy"));
    assert!(preview.get("change").is_none());
    assert_eq!(writes(&server).await, Vec::<String>::new());
}

#[tokio::test]
async fn an_edit_is_committed_on_its_branch_and_proposed_as_the_apps_change_at_that_commit() {
    let (server, state) = world().await;
    let answer = send(
        &state,
        person("ema"),
        "POST",
        ROUTE,
        Some(json!({ "config": edited() })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let proposal = body(&answer.text);
    assert!(
        proposal["change"]["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .starts_with("chg-"),
        "{proposal}"
    );

    let requests = server.received_requests().await.expect("requests");
    let posted = |route: String| {
        requests
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path() == route)
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap_or(Value::Null))
            .collect::<Vec<_>>()
    };
    let branch = posted(format!("{APP_REPO}/branches"));
    assert_eq!(branch.len(), 1);
    let name = branch[0]["new_branch_name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(name.starts_with("dashboard/"), "{name}");
    let commit = posted(format!("{APP_REPO}/contents"));
    assert_eq!(commit.len(), 1);
    assert_eq!(commit[0]["branch"], name.as_str());
    let files = commit[0]["files"].as_array().expect("files");
    assert_eq!(files.len(), 1, "only the dashboard file changes");
    assert_eq!(files[0]["path"], "src/dashboard.json");
    let text = String::from_utf8(
        STANDARD
            .decode(files[0]["content"].as_str().unwrap_or_default())
            .expect("base64"),
    )
    .expect("utf-8");
    assert_eq!(
        serde_json::from_str::<Value>(&text).expect("json"),
        edited()
    );
    let pulls = posted(format!("{APP_REPO}/pulls"));
    assert_eq!(pulls.len(), 1);
    assert_eq!(
        (pulls[0]["head"].as_str(), pulls[0]["base"].as_str()),
        (Some(name.as_str()), Some("main"))
    );

    // The project's Change names the App at the commit the branch holds, nothing else moved.
    let manifest = requests
        .iter()
        .filter(|r| {
            r.url.path().starts_with(&format!("{PROJECT}/contents"))
                && matches!(r.method.as_str(), "POST" | "PUT")
        })
        .flat_map(|r| {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            let mut texts: Vec<String> = body["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f["content"].as_str())
                .map(String::from)
                .collect();
            texts.extend(body["content"].as_str().map(String::from));
            texts
        })
        .map(|one| String::from_utf8_lossy(&STANDARD.decode(one).unwrap_or_default()).into_owned())
        .find(|text| text.contains("kind: App"))
        .expect("the App proposed");
    assert!(manifest.contains("ref: commit-1"), "{manifest}");
    assert!(!manifest.contains(SERVED), "{manifest}");
    assert!(manifest.contains("BikeHireDockingStation"), "{manifest}");
}

#[tokio::test]
async fn a_refused_formula_or_an_unknown_attribute_is_a_422_naming_each_field() {
    let (server, state) = world().await;
    for (config, field) in [
        (
            json!({ "attr": "x", "formula": "fetch(name)" }),
            "config/grid/columns/0/formula",
        ),
        (
            json!({ "attr": "x", "formula": "ownerEmail + 1" }),
            "config/grid/columns/0/formula",
        ),
        (
            json!({ "attr": "ownerEmail" }),
            "config/grid/columns/0/attr",
        ),
    ] {
        let mut edit = file();
        edit["grid"]["columns"] = json!([config]);
        let answer = send(
            &state,
            person("ema"),
            "POST",
            ROUTE,
            Some(json!({ "config": edit })),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            answer.text
        );
        assert!(answer.text.contains(field), "{field}: {}", answer.text);
    }
    for (key, value, field) in [
        (
            "filters",
            json!([{ "attr": "ownerEmail", "op": "==", "value": "a" }]),
            "config/filters/0/attr",
        ),
        (
            "filters",
            json!([{ "attr": "name", "op": "==", "value": "a\";id==\"b" }]),
            "config/filters/0/value",
        ),
        (
            "colour",
            json!({ "attr": "ownerEmail", "bands": [{ "tone": "info", "label": "x" }] }),
            "config/colour/attr",
        ),
        (
            "chart",
            json!({ "x": "ownerEmail", "y": "name", "agg": "sum" }),
            "config/chart/x",
        ),
        (
            "kpis",
            json!([{ "label": "x", "agg": "sum", "attr": "ownerEmail" }]),
            "config/kpis/0/attr",
        ),
        (
            "kpis",
            json!([{ "label": "x", "agg": "count", "where": "constructor()" }]),
            "config/kpis/0/where",
        ),
    ] {
        let mut edit = file();
        edit[key] = value;
        let answer = send(
            &state,
            person("ema"),
            "POST",
            ROUTE,
            Some(json!({ "config": edit, "dryRun": true })),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            answer.text
        );
        assert!(answer.text.contains(field), "{field}: {}", answer.text);
    }
    // The grid keeps reading what it read, and a key the editor does not edit stays as it was.
    for (pointer, value, field) in [
        (
            "/grid/source",
            json!({ "kind": "endpoint", "slug": "everything" }),
            "config/grid/source",
        ),
        ("/grid/type", json!("Person"), "config/grid/type"),
        (
            "/grid/history",
            json!({ "enabled": false }),
            "config/grid/history",
        ),
    ] {
        let mut edit = file();
        if let Some(at) = edit.pointer_mut(pointer) {
            *at = value;
        }
        let answer = send(
            &state,
            person("ema"),
            "POST",
            ROUTE,
            Some(json!({ "config": edit })),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            answer.text
        );
        assert!(answer.text.contains(field), "{field}: {}", answer.text);
    }
    assert_eq!(
        writes(&server).await,
        Vec::<String>::new(),
        "a refused edit wrote to the forge"
    );
}

#[tokio::test]
async fn without_propose_or_without_a_readable_space_nothing_is_proposed() {
    let (server, state) = world().await;
    let peter = send(
        &state,
        person("peter"),
        "POST",
        ROUTE,
        Some(json!({ "config": edited() })),
    )
    .await;
    assert_eq!(peter.status, StatusCode::FORBIDDEN, "{}", peter.text);
    let olga = send(
        &state,
        person("olga"),
        "POST",
        ROUTE,
        Some(json!({ "config": edited() })),
    )
    .await;
    assert_eq!(olga.status, StatusCode::FORBIDDEN, "{}", olga.text);
    let ivan = send(
        &state,
        person("ivan"),
        "POST",
        ROUTE,
        Some(json!({ "config": edited() })),
    )
    .await;
    assert_eq!(ivan.status, StatusCode::NOT_FOUND, "{}", ivan.text);
    let same = send(
        &state,
        person("ema"),
        "POST",
        ROUTE,
        Some(json!({ "config": file() })),
    )
    .await;
    assert_eq!(same.status, StatusCode::CONFLICT, "{}", same.text);
    let unknown = send(
        &state,
        person("ema"),
        "POST",
        ROUTE,
        Some(json!({ "config": edited(), "script": 1 })),
    )
    .await;
    assert!(unknown.status.is_client_error(), "{}", unknown.text);
    assert_eq!(writes(&server).await, Vec::<String>::new());
}

#[tokio::test]
async fn an_approved_app_change_merges_the_dashboard_branch_at_its_commit_and_nothing_else() {
    let (server, state) = world().await;
    let pull = |number: u64, branch: &str, sha: &str| {
        json!({ "number": number, "html_url": "", "state": "open", "title": "t", "body": "",
                "head": { "ref": branch, "sha": sha }, "base": { "ref": "main", "sha": "b" } })
    };
    Mock::given(method("GET"))
        .and(path(format!("{APP_REPO}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            pull(4, "dashboard/1", "aaa"),
            pull(5, "agent/app-board/run", "bbb"),
            pull(6, "dashboard/2", "bbb"),
        ])))
        .with_priority(1)
        .mount(&server)
        .await;
    let mut approved = state
        .mirror
        .get("sharing", "App", "board")
        .expect("the App");
    approved.spec["source"]["git"]["ref"] = json!("bbb");
    joinedcontext_portal::api::app_dashboard::merge_approved_edit(
        &state,
        "sharing",
        Some(&approved),
        "anna",
    )
    .await;
    let merges: Vec<(String, Value)> = server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/merge"))
        .map(|r| {
            (
                r.url.path().to_owned(),
                serde_json::from_slice(&r.body).unwrap_or(Value::Null),
            )
        })
        .collect();
    assert_eq!(merges.len(), 1, "{merges:?}");
    assert_eq!(merges[0].0, format!("{APP_REPO}/pulls/6/merge"));
    assert_eq!(merges[0].1["head_commit_id"], "bbb");

    // An App Change at a commit no dashboard branch holds merges nothing.
    approved.spec["source"]["git"]["ref"] = json!(SERVED);
    joinedcontext_portal::api::app_dashboard::merge_approved_edit(
        &state,
        "sharing",
        Some(&approved),
        "anna",
    )
    .await;
    let count = server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .filter(|r| r.url.path().ends_with("/merge"))
        .count();
    assert_eq!(count, 1);
}
