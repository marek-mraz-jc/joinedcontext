//! The dashboard builder (owner 2026-11-08: "let the users manually create these dashboards in
//! the UI"). A person who may propose a Dashboard builds one from choices; the Portal compiles the
//! Grafana file, proposes it as a Change, and the reconciler provisions the approved file into the
//! dashboards service. Every door re-checks the rights and each Endpoint a choice names, and the
//! browser sends no query. The roles' cells are in the authorization matrix.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use base64::Engine;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{committed_files, envelope, person, send, REPO};
use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::config::{Config, GrafanaConfig};
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::grafana_builder::{self as builder, DashboardSync, Outcome};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const BUILDER: &str = "/api/v1/projects/helsinki/grafana/builder";
const PUBLIC_SLUG: &str = "jtjnhkaau64zmzz5csmcwem52icvufy2";
const CLOSED_SLUG: &str = "kvhnixtbe5dex2wt2kx3i37dzw6yzsxt";
const DATASOURCE: &str = "bikes-open-ds";
const UID: &str = "jcd-0123456789abcdef";
/// The builder file of [`board`], with this uid.
const FIXTURE: &str = "tests/fixtures/grafana-builder/bike-stations.json";
const FIXTURE_UID: &str = "jcd-b1ce57a7105f0001";

fn role(state: &AppState, name: &str, rules: Value) {
    state.mirror.upsert(envelope(
        "Role",
        name,
        ORG_NAMESPACE,
        json!({ "rules": rules }),
    ));
}

fn binding(state: &AppState, who: &str, role: &str, scope: Value) {
    state.mirror.upsert(envelope(
        "RoleBinding",
        &format!("{who}-{role}"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": scope }),
    ));
}

/// The approver: an administrator of the installation, signed in with a second factor.
fn approver() -> Identity {
    let mut who = person("ada");
    who.groups = vec!["portal-approver".into()];
    who.second_factor = true;
    who
}

/// The dashboards service: the project's organization, a datasource for the public Endpoint, and
/// a query door that answers one table of `status` values.
async fn daiya() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/grafana/api/orgs/name/helsinki"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 2 })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/grafana/api/datasources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "uid": DATASOURCE, "type": "ngsild-grafana-datasource",
              "url": format!("http://gateway:8080/api/endpoint/{PUBLIC_SLUG}") },
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/grafana/api/ds/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "results": { "A": {
                "frames": [{ "fields": [
                    { "name": "status", "values": ["open", "closed", "open"] },
                    { "name": "availableBikeNumber", "values": [3, 7, 1] },
                ]}]
            }}})),
        )
        .mount(&server)
        .await;
    server
}

/// Two spaces of one project: `bikes` with a public and a closed Endpoint and a model, `air` with
/// one closed Endpoint. `eva` builds dashboards, `vera` only reads, `zoe` builds but reads only
/// `bikes`, `ola` builds in another project.
fn world(shako: &MockServer, grafana: &MockServer) -> AppState {
    let mut config = Config::for_tests();
    config.gateway_url = Some("http://gateway:8080".into());
    config.grafana = Some(GrafanaConfig {
        url: format!("{}/grafana", grafana.uri()),
        public_url: "https://dev.example/grafana".into(),
        user: "grafana_admin".into(),
        password: "the-administrators-password".into(),
    });
    let client = ShakoClient::new(
        shako.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(config, None).with_shako(Arc::new(client));
    for space in ["bikes", "air"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, PROJECT, json!({})));
    }
    for (name, space, slug, audience) in [
        ("bikes-open", "bikes", PUBLIC_SLUG, "public"),
        ("bikes-closed", "bikes", CLOSED_SLUG, "private"),
        (
            "air-closed",
            "air",
            "ydp6nddchg4sewmylsiclg4377mlldrj",
            "private",
        ),
    ] {
        state.mirror.upsert(envelope(
            "Endpoint",
            name,
            PROJECT,
            json!({ "contextSpaceRef": space, "slug": slug, "audience": audience }),
        ));
    }
    let read =
        json!({ "kinds": ["Project", "ContextSpace", "Endpoint", "Dashboard"], "verbs": ["read"] });
    role(&state, "reader", json!([read]));
    role(
        &state,
        "drafter",
        json!([read, { "kinds": ["Dashboard"], "verbs": ["propose"] }]),
    );
    role(
        &state,
        "dashboards",
        json!([{ "kinds": ["Dashboard"], "verbs": ["propose"] }]),
    );
    binding(&state, "vera", "reader", json!({ "project": PROJECT }));
    binding(&state, "eva", "drafter", json!({ "project": PROJECT }));
    binding(&state, "zoe", "reader", json!({ "contextSpace": "bikes" }));
    binding(&state, "zoe", "dashboards", json!({ "project": PROJECT }));
    binding(&state, "ola", "drafter", json!({ "project": "bbsk" }));
    state.model_schemas.replace(std::collections::HashMap::from([(
        (PROJECT.to_owned(), "bikes".to_owned()),
        Arc::new(ModelSchema::compile(
            "bikes",
            "1.2.0",
            &json!({ "definitions": { "BikeHireDockingStation": {
                "required": ["id", "type"],
                "properties": {
                    "id": { "type": "string" }, "type": { "type": "string" },
                    "name": { "type": ["string", "null"], "x-ngsi-ld-kind": "Property" },
                    "status": { "type": ["string", "null"], "x-ngsi-ld-kind": "Property" },
                    "availableBikeNumber": { "type": ["integer", "null"], "x-ngsi-ld-kind": "Property" },
                    "refOwner": { "type": ["string", "null"], "x-ngsi-ld-kind": "Relationship" }
                }
            }}}),
            &["BikeHireDockingStation".to_owned()],
            false,
        )),
    )]));
    state
}

fn panel(id: u32, kind: &str, x: u32, extra: Value) -> Value {
    let mut panel = json!({
        "id": id, "kind": kind, "title": format!("Panel {id}"),
        "grid": { "x": x, "y": 0, "w": 6, "h": 8 },
        "endpoint": "bikes-open", "type": "BikeHireDockingStation",
        "attribute": "availableBikeNumber",
    });
    for (key, value) in extra.as_object().expect("an object") {
        panel[key] = value.clone();
    }
    panel
}

/// A board with each of the four kinds and a variable a filter reads.
fn board() -> Value {
    json!({
        "title": "Bike stations",
        "refresh": "5m",
        "variables": [{ "name": "status", "label": "Status", "endpoint": "bikes-open",
                        "type": "BikeHireDockingStation", "attribute": "status", "includeAll": true }],
        "panels": [
            panel(1, "stat", 0, json!({
                "filters": [{ "attribute": "status", "op": "eq", "variable": "status" }],
                "unit": "none", "decimals": 0,
                "thresholds": { "base": "green", "steps": [{ "value": 5, "color": "red" }] },
            })),
            panel(2, "timeseries", 6, json!({ "aggregation": "avg", "interval": "PT1H" })),
            panel(3, "table", 12, json!({
                "filters": [{ "attribute": "availableBikeNumber", "op": "gt", "value": "2" }],
            })),
            panel(4, "gauge", 18, json!({ "aggregation": "max", "min": 0, "max": 40 })),
            panel(5, "bargauge", 0, json!({
                "grid": { "x": 0, "y": 8, "w": 6, "h": 8 }, "min": 0, "max": 40,
                "bargauge": { "displayMode": "lcd", "orientation": "horizontal" },
            })),
            panel(6, "piechart", 6, json!({
                "grid": { "x": 6, "y": 8, "w": 6, "h": 8 }, "aggregation": "sum",
                "piechart": { "pieType": "donut" },
            })),
            panel(7, "barchart", 12, json!({
                "grid": { "x": 12, "y": 8, "w": 6, "h": 8 }, "unit": "short",
                "barchart": { "xField": "name" },
            })),
            text_panel(8, 18, 8),
        ],
    })
}

/// A text panel: its words, and nothing it reads.
fn text_panel(id: u32, x: u32, y: u32) -> Value {
    json!({
        "id": id, "kind": "text", "title": "About these stations",
        "grid": { "x": x, "y": y, "w": 6, "h": 8 },
        "text": { "content": "## Bike stations\nFree bikes per station, read every **5 minutes**." },
    })
}

fn body(answer: &common::Answer) -> Value {
    serde_json::from_str(&answer.text).unwrap_or_default()
}

fn content(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text)
}

/// The forge holds `text` at the dashboard's path on `git_ref`, as blob `sha`.
async fn holds(shako: &MockServer, uid: &str, git_ref: &str, sha: &str, text: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param(
            "@p",
            format!("'projects/{PROJECT}/grafana/{uid}.json'"),
        ))
        .and(query_param("@r", format!("'{git_ref}'")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": sha, "Content": content(text) })),
        )
        .with_priority(1)
        .mount(shako)
        .await;
}

/// The file a create by `eva` writes.
async fn created(state: &AppState, shako: &MockServer) -> (String, String) {
    let answer = send(
        state,
        person("eva"),
        "POST",
        BUILDER,
        Some(json!({ "board": board() })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let uid = body(&answer)["uid"].as_str().expect("a uid").to_owned();
    let files = committed_files(shako, REPO).await;
    let (file, text) = files.last().cloned().expect("a committed file");
    assert_eq!(file, format!("projects/{PROJECT}/grafana/{uid}.json"));
    (uid, text)
}

/// A merge request `number` of the builder on `uid`, adding `text`.
async fn change_of(shako: &MockServer, number: u64, uid: &str, text: &str) {
    let branch = format!("portal/grafana-{PROJECT}-{uid}");
    let pull = json!({
        "Number": number, "HtmlUrl": format!("https://shako.example/pulls/{number}"),
        "State": "open", "Title": format!("add dashboard {uid}"), "Head": branch, "Base": "main",
        "CreatedAt": "2026-11-08T09:00:00Z", "Author": "eva", "AuthorName": "eva",
        "AuthorEmail": "eva@hel.fi", "Mergeable": true, "Merged": false
    });
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests({number})")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests({number})/Files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [
            { "Filename": format!("projects/{PROJECT}/grafana/{uid}.json"), "Status": "added" }
        ]})))
        .mount(shako)
        .await;
    holds(shako, uid, &branch, "blob-head", text).await;
}

#[tokio::test]
async fn a_builder_saves_eight_kinds_as_one_grafana_file_that_reads_back_as_the_same_choices() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let (uid, text) = created(&state, &shako).await;
    assert!(builder::is_builder_uid(&uid), "{uid}");
    // The file as committed beside this test, which daiya's contract test and the Grafana
    // 12.4.14 run open: `JC_WRITE_FIXTURE=1` writes it after a change to what the builder writes.
    let fixed = text.replace(&uid, FIXTURE_UID);
    if std::env::var_os("JC_WRITE_FIXTURE").is_some() {
        std::fs::write(FIXTURE, &fixed).expect("the fixture");
    }
    assert_eq!(
        std::fs::read_to_string(FIXTURE).expect("the fixture"),
        fixed,
        "{FIXTURE} is stale: rerun with JC_WRITE_FIXTURE=1"
    );

    let file: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(file["uid"], uid.as_str());
    let kinds: Vec<&str> = file["panels"]
        .as_array()
        .expect("panels")
        .iter()
        .filter_map(|p| p["type"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "stat",
            "timeseries",
            "table",
            "gauge",
            "bargauge",
            "piechart",
            "barchart",
            "text"
        ]
    );
    for panel in file["panels"].as_array().expect("panels") {
        if panel["type"] == "text" {
            assert!(panel.get("datasource").is_none(), "{panel}");
        } else {
            assert_eq!(panel["datasource"]["uid"], DATASOURCE, "{panel}");
        }
    }
    // The variable offers what the dashboards service read as the person, deduplicated.
    let variable = &file["templating"]["list"][0];
    assert_eq!(variable["query"], "closed,open");
    assert_eq!(
        file["panels"][0]["targets"][0]["customQuery"],
        "status~=\"^${status:regex}$\""
    );

    // Main holds it now: it reads back as the choices it was built from.
    holds(&shako, &uid, "main", "blob-1", &text).await;
    let read = send(
        &state,
        person("eva"),
        "GET",
        &format!("{BUILDER}/{uid}"),
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    let mut sent = board();
    // What the server fills in for what was left out.
    sent["from"] = json!("now-6h");
    sent["to"] = json!("now");
    let read_board = body(&read)["board"].clone();
    let back: builder::Board = serde_json::from_value(read_board).expect("a board");
    let asked: builder::Board = serde_json::from_value(sent).expect("a board");
    assert_eq!(back, asked);
    assert_eq!(body(&read)["base"], "blob-1");

    // An edit from that revision is proposed; one from an older one is a conflict.
    let edit = json!({ "board": board(), "base": "blob-1" });
    let answer = send(
        &state,
        person("eva"),
        "PUT",
        &format!("{BUILDER}/{uid}"),
        Some(edit),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let stale = json!({ "board": board(), "base": "blob-0" });
    let answer = send(
        &state,
        person("eva"),
        "PUT",
        &format!("{BUILDER}/{uid}"),
        Some(stale),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);

    // A removal is one more Change, of that one file.
    let answer = send(
        &state,
        person("eva"),
        "DELETE",
        &format!("{BUILDER}/{uid}"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    assert_eq!(
        common::removed_in_one_commit(&shako).await,
        [format!("projects/{PROJECT}/grafana/{uid}.json")]
    );
}

#[tokio::test]
async fn a_dashboard_with_an_open_change_is_not_proposed_twice() {
    let shako = MockServer::start().await;
    let branch = format!("portal/grafana-{PROJECT}-{UID}");
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [{
            "Number": 4, "HtmlUrl": "https://shako.example/pulls/4", "State": "open",
            "Title": "add dashboard Bike stations", "Head": branch, "Base": "main",
            "Mergeable": true, "Merged": false
        }]})))
        .with_priority(1)
        .mount(&shako)
        .await;
    common::mount_repository(&shako, REPO).await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    holds(&shako, UID, "main", "blob-1", "{}").await;

    let start = send(&state, person("eva"), "GET", BUILDER, None).await;
    assert_eq!(body(&start)["pending"][0]["uid"], UID, "{}", start.text);
    let edit = json!({ "board": board(), "base": "blob-1" });
    let answer = send(
        &state,
        person("eva"),
        "PUT",
        &format!("{BUILDER}/{UID}"),
        Some(edit),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("open already"), "{}", answer.text);
}

#[tokio::test]
async fn a_reader_meets_no_builder_door_and_an_outsider_no_project() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let start = send(&state, person("vera"), "GET", BUILDER, None).await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.text);
    assert_eq!(body(&start)["mayEdit"], false);
    assert_eq!(body(&start)["pending"], json!([]));

    let doors: [(&str, String, Option<Value>); 6] = [
        (
            "POST",
            BUILDER.to_owned(),
            Some(json!({ "board": board() })),
        ),
        ("GET", format!("{BUILDER}/{UID}"), None),
        (
            "PUT",
            format!("{BUILDER}/{UID}"),
            Some(json!({ "board": board(), "base": "x" })),
        ),
        ("DELETE", format!("{BUILDER}/{UID}"), None),
        (
            "POST",
            format!("{BUILDER}/preview"),
            Some(json!({ "board": board(), "panel": 1 })),
        ),
        (
            "GET",
            format!(
                "{BUILDER}/values?endpoint=bikes-open&type=BikeHireDockingStation&attribute=status"
            ),
            None,
        ),
    ];
    for (verb, uri, sent) in doors {
        let answer = send(&state, person("vera"), verb, &uri, sent.clone()).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{verb} {uri}: {}",
            answer.text
        );
        let answer = send(&state, person("ola"), verb, &uri, sent).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{verb} {uri}: {}",
            answer.text
        );
    }
    let answer = send(&state, person("ola"), "GET", BUILDER, None).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    // Nothing reached the forge's write doors or the dashboards service's query.
    assert!(committed_files(&shako, REPO).await.is_empty());
}

#[tokio::test]
async fn a_query_the_browser_sends_is_refused_on_every_door_that_takes_a_board() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let mut with_targets = board();
    with_targets["panels"][0]["targets"] = json!([{ "refId": "A", "customQuery": "id~=\".*\"" }]);
    let mut with_query = board();
    with_query["panels"][1]["customQuery"] = json!("availableBikeNumber>0");
    let mut with_json = board();
    with_json["dashboard"] = json!({ "panels": [] });
    for sent in [with_targets, with_query, with_json] {
        for (verb, uri, wrapped) in [
            ("POST", BUILDER.to_owned(), json!({ "board": sent })),
            (
                "POST",
                format!("{BUILDER}/preview"),
                json!({ "board": sent, "panel": 1 }),
            ),
        ] {
            let answer = send(&state, person("eva"), verb, &uri, Some(wrapped)).await;
            assert_eq!(
                answer.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{uri}: {}",
                answer.text
            );
            assert!(answer.text.contains("unknown field"), "{}", answer.text);
        }
    }
    let raw = json!({ "board": board(), "panel": 1, "query": "x" });
    let answer = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(raw),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    assert!(committed_files(&shako, REPO).await.is_empty());
    let asked: Vec<_> = grafana
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/ds/query"))
        .collect();
    assert!(asked.is_empty(), "no query ran");
}

#[tokio::test]
async fn each_shape_rule_is_refused_by_its_field_before_anything_is_written() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let mut cases: Vec<(Value, &str)> = Vec::new();
    let mut long = board();
    long["title"] = json!("x".repeat(builder::MAX_TITLE + 1));
    cases.push((long, "title"));
    let mut many = board();
    many["panels"] = json!((1..=builder::MAX_PANELS as u32 + 1)
        .map(|id| panel(
            id,
            "stat",
            0,
            json!({ "grid": { "x": 0, "y": id * 8, "w": 6, "h": 8 } })
        ))
        .collect::<Vec<_>>());
    cases.push((many, "panels"));
    let mut wide = board();
    wide["panels"][0]["grid"] = json!({ "x": 20, "y": 0, "w": 6, "h": 8 });
    cases.push((wide, "panels[0].grid"));
    let mut overlap = board();
    overlap["panels"][1]["grid"] = json!({ "x": 3, "y": 2, "w": 6, "h": 8 });
    cases.push((overlap, "panels[1].grid"));
    let mut name = board();
    name["variables"][0]["name"] = json!("1-status");
    cases.push((name, "variables[0].name"));
    let mut unknown = board();
    unknown["panels"][0]["filters"][0]["variable"] = json!("nobody");
    cases.push((unknown, "panels[0].filters[0].variable"));
    let mut injected = board();
    injected["panels"][2]["filters"][0]["value"] = json!("2\";id~=\".*");
    cases.push((injected, "panels[2].filters[0].value"));
    let mut refresh = board();
    refresh["refresh"] = json!("1s");
    cases.push((refresh, "refresh"));
    let mut text = board();
    text["panels"][0]["attribute"] = json!("status");
    cases.push((text, "panels[0].kind"));
    let mut link = board();
    link["panels"][2]["attribute"] = json!("refOwner");
    cases.push((link, "panels[2].kind"));
    let mut missing = board();
    missing["panels"][3]["attribute"] = json!("noSuchAttribute");
    cases.push((missing, "panels[3].attribute"));
    let mut scale = board();
    scale["panels"][4]["min"] = json!(40);
    cases.push((scale, "panels[4].max"));
    let mut unscaled = board();
    unscaled["panels"][0]["max"] = json!(10);
    cases.push((unscaled, "panels[0].max"));
    let mut stray = board();
    stray["panels"][0]["piechart"] = json!({ "pieType": "pie" });
    cases.push((stray, "panels[0].piechart"));
    let mut bare = board();
    bare["panels"][4]
        .as_object_mut()
        .expect("a panel")
        .remove("bargauge");
    cases.push((bare, "panels[4].bargauge"));
    let mut numbered = board();
    numbered["panels"][6]["barchart"]["xField"] = json!("availableBikeNumber");
    cases.push((numbered, "panels[6].barchart.xField"));
    let mut absent = board();
    absent["panels"][6]["barchart"]["xField"] = json!("noSuchAttribute");
    cases.push((absent, "panels[6].barchart.xField"));
    let mut over = board();
    over["panels"][6]["aggregation"] = json!("avg");
    cases.push((over, "panels[6].aggregation"));
    let mut wordy = board();
    wordy["panels"][7]["text"]["content"] = json!("x".repeat(builder::MAX_CONTENT + 1));
    cases.push((wordy, "panels[7].text.content"));
    let mut reading = board();
    reading["panels"][7]["endpoint"] = json!("bikes-open");
    cases.push((reading, "panels[7].endpoint"));
    let mut silent = board();
    silent["panels"][7]["text"]["content"] = json!("  ");
    cases.push((silent, "panels[7].text.content"));
    let mut keyed = board();
    keyed["panels"][7]["text"]["content"] = json!(format!(
        "Readings from https://counters.example/v1/readings?token={}",
        "q7Zr".repeat(6)
    ));
    cases.push((keyed, "panels[7].text.content"));
    // A kind's own choices take only their own members and words.
    for (at, block) in [
        ("bargauge", json!({ "displayMode": "lcd", "fill": 1 })),
        ("bargauge", json!({ "displayMode": "retro" })),
        ("piechart", json!({ "pieType": "pie", "labels": ["name"] })),
        (
            "barchart",
            json!({ "xField": "name", "stacking": "normal" }),
        ),
    ] {
        let index = ["bargauge", "piechart", "barchart"]
            .iter()
            .position(|kind| *kind == at)
            .expect("a kind")
            + 4;
        let mut sent = board();
        sent["panels"][index][at] = block;
        cases.push((sent, at));
    }
    for (sent, field) in cases {
        let answer = send(
            &state,
            person("eva"),
            "POST",
            BUILDER,
            Some(json!({ "board": sent })),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {}",
            answer.text
        );
        let errors = body(&answer)["errors"].to_string();
        assert!(errors.contains(field), "{field}: {}", answer.text);
    }
    assert!(committed_files(&shako, REPO).await.is_empty());
}

#[tokio::test]
async fn a_choice_naming_an_endpoint_the_person_cannot_read_is_refused_by_its_field() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let mut sent = board();
    sent["panels"][1]["endpoint"] = json!("air-closed");
    let answer = send(
        &state,
        person("zoe"),
        "POST",
        BUILDER,
        Some(json!({ "board": sent.clone() })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(
        answer.text.contains("panels[1].endpoint"),
        "{}",
        answer.text
    );
    let preview = json!({ "board": sent, "panel": 1 });
    let answer = send(
        &state,
        person("zoe"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(preview),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    let answer = send(
        &state,
        person("zoe"),
        "GET",
        &format!("{BUILDER}/values?endpoint=air-closed&type=AirQualityObserved&attribute=name"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(
        answer.text.contains("variables[0].endpoint"),
        "{}",
        answer.text
    );
    // An Endpoint of another project reads as one this project does not have.
    let mut foreign = board();
    foreign["variables"][0]["endpoint"] = json!("bbsk-open");
    let answer = send(
        &state,
        person("eva"),
        "POST",
        BUILDER,
        Some(json!({ "board": foreign })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(committed_files(&shako, REPO).await.is_empty());
}

#[tokio::test]
async fn a_uid_the_builder_did_not_mint_or_another_project_holds_is_not_found() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    // Another project's file of that uid is not this project's.
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param(
            "@p",
            format!("'projects/bbsk/grafana/{UID}.json'"),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "b", "Content": content("{}") })),
        )
        .with_priority(1)
        .mount(&shako)
        .await;
    // A dashboard made by hand in this project's folder is there, but no builder uid: only the
    // uid rule keeps the builder's doors off it.
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param(
            "@p",
            format!("'projects/{PROJECT}/grafana/abc-0123456789abcdef.json'"),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "b", "Content": content("{}") })),
        )
        .with_priority(1)
        .mount(&shako)
        .await;
    for uid in [UID, "abc-0123456789abcdef", "jcd-0123", "jcd-..%2Fsecrets"] {
        for verb in ["GET", "DELETE"] {
            let answer = send(
                &state,
                person("eva"),
                verb,
                &format!("{BUILDER}/{uid}"),
                None,
            )
            .await;
            assert_eq!(
                answer.status,
                StatusCode::NOT_FOUND,
                "{verb} {uid}: {}",
                answer.text
            );
        }
        let edit = json!({ "board": board(), "base": "b" });
        let answer = send(
            &state,
            person("eva"),
            "PUT",
            &format!("{BUILDER}/{uid}"),
            Some(edit),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "PUT {uid}: {}",
            answer.text
        );
    }
    assert!(committed_files(&shako, REPO).await.is_empty());
}

#[tokio::test]
async fn a_preview_runs_the_compiled_panel_as_the_person_and_names_a_failure() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let preview = json!({ "board": board(), "panel": 1, "vars": { "status": ["open"] } });
    let answer = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(preview),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(body(&answer)["panel"]["type"], "stat", "{}", answer.text);
    assert!(body(&answer)["frames"]
        .as_array()
        .is_some_and(|f| !f.is_empty()));
    let asked: Vec<Value> = grafana
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().ends_with("/ds/query"))
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect();
    let sent = asked.last().expect("a query");
    assert_eq!(
        sent["queries"][0]["customQuery"],
        "status~=\"^${status:regex}$\""
    );
    assert_eq!(sent["variables"]["status"], "open");

    // A panel the board does not hold.
    let missing = json!({ "board": board(), "panel": 9 });
    let answer = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(missing),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);

    // The service refusing the query is the panel's error, said without credentials.
    let failing = MockServer::start().await;
    for (verb, at, answer) in [
        (
            "GET",
            "/grafana/api/orgs/name/helsinki",
            ResponseTemplate::new(200).set_body_json(json!({ "id": 2 })),
        ),
        (
            "GET",
            "/grafana/api/datasources",
            ResponseTemplate::new(200).set_body_json(json!([
            { "uid": DATASOURCE, "type": "ngsild-grafana-datasource",
              "url": format!("http://gateway:8080/api/endpoint/{PUBLIC_SLUG}") }])),
        ),
        (
            "POST",
            "/grafana/api/ds/query",
            ResponseTemplate::new(400)
                .set_body_json(json!({ "message": "the broker refused the query" })),
        ),
    ] {
        Mock::given(method(verb))
            .and(path(at))
            .respond_with(answer)
            .mount(&failing)
            .await;
    }
    let state = world(&shako, &failing);
    let preview = json!({ "board": board(), "panel": 2 });
    let answer = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(preview),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(
        body(&answer)["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty()),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("the-administrators-password"));
}

#[tokio::test]
async fn a_builder_change_reads_as_a_dashboard_and_approves_only_as_the_builder_wrote_it() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let (uid, text) = created(&state, &shako).await;
    change_of(&shako, 21, &uid, &text).await;
    let foreign_uid = "jcd-fedcba9876543210";
    let foreign = json!({
        "uid": foreign_uid, "title": "Imported", "panels": [{ "id": 1, "type": "timeseries",
            "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
            "datasource": { "type": "prometheus", "uid": "prometheus" },
            "targets": [{ "refId": "A", "expr": "up" }] }]
    })
    .to_string();
    change_of(&shako, 22, foreign_uid, &foreign).await;

    let read = send(
        &state,
        approver(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000015"),
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert!(read.text.contains("\"Dashboard\""), "{}", read.text);

    let refused = send(
        &state,
        approver(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000016/approve"),
        Some(json!({})),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        refused
            .text
            .contains("not a dashboard the Portal's builder wrote"),
        "{}",
        refused.text
    );

    let approved = send(
        &state,
        approver(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000015/approve"),
        Some(json!({})),
    )
    .await;
    assert!(
        approved.status.is_success(),
        "{}: {}",
        approved.status,
        approved.text
    );
}

/// A scratch staging tree holding `files`, as the reconciler stages the repository.
fn staged(name: &str, files: &[(&str, &str, &str)]) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jc-dashboards-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (project, uid, text) in files {
        let dir = root.join("projects").join(project).join("grafana");
        std::fs::create_dir_all(&dir).expect("a dir");
        std::fs::write(dir.join(format!("{uid}.json")), text).expect("a file");
    }
    root
}

#[tokio::test]
async fn the_reconciler_provisions_approved_files_keeps_equal_ones_and_removes_orphans() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let (uid, text) = created(&state, &shako).await;
    let held: Value = serde_json::from_str(&text).expect("JSON");

    let service = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/orgs/name/helsinki"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 2 })))
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/dashboards/uid/{uid}")))
        .respond_with(ResponseTemplate::new(404))
        .up_to_n_times(1)
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/dashboards/uid/{uid}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "dashboard": held })))
        .mount(&service)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/dashboards/db"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "success" })))
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "uid": uid }, { "uid": "jcd-00000000000000aa" }, { "uid": "made-by-hand" },
            { "uid": "jc-assistant" }
        ])))
        .mount(&service)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/dashboards/uid/jcd-00000000000000aa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&service)
        .await;
    let sync = DashboardSync::new(GrafanaConfig {
        url: service.uri(),
        public_url: "https://dev.example/grafana".into(),
        user: "grafana_admin".into(),
        password: "unused".into(),
    });
    let foreign = r#"{"uid":"jcd-1111111111111111","title":"x","panels":[{"id":1,"type":"text"}]}"#;
    // Dev's assistant dashboard with its two Prometheus bar gauges: by its own name the sync
    // never reads it, and copied under a builder uid it is refused naming its datasource.
    let dev = include_str!("fixtures/grafana-builder/dev-assistant.json");
    let copied = dev.replacen("\"jc-assistant\"", "\"jcd-2222222222222222\"", 1);
    let root = staged(
        "sync",
        &[
            (PROJECT, &uid, &text),
            (PROJECT, "jcd-1111111111111111", foreign),
            (PROJECT, "jc-assistant", dev),
            (PROJECT, "jcd-2222222222222222", &copied),
        ],
    );
    let projects = [PROJECT.to_owned()];

    let first = sync.converge(&root, &projects, &[]).await;
    assert!(
        first.contains(&Outcome::Saved(PROJECT.into(), uid.clone())),
        "{first:?}"
    );
    assert!(
        first.contains(&Outcome::Removed(
            PROJECT.into(),
            "jcd-00000000000000aa".into()
        )),
        "{first:?}"
    );
    assert!(
        first
            .iter()
            .any(|o| matches!(o, Outcome::Refused(_, u, _) if u == "jcd-1111111111111111")),
        "{first:?}"
    );
    assert!(
        first.iter().any(|o| matches!(o, Outcome::Refused(_, u, why)
            if u == "jcd-2222222222222222" && why.contains("prometheus datasource prometheus"))),
        "{first:?}"
    );
    // A hand-made dashboard is never the reconciler's to save or remove.
    for hand in ["made-by-hand", "jc-assistant"] {
        assert!(
            !first.iter().any(|o| matches!(o,
                Outcome::Removed(_, u) | Outcome::Saved(_, u) | Outcome::Kept(_, u) | Outcome::Refused(_, u, _)
                if u == hand)),
            "{hand}: {first:?}"
        );
    }
    assert!(!service
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.url.path().contains("jc-assistant")));
    let saved: Vec<Value> = service
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/api/dashboards/db")
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect();
    assert_eq!(saved.len(), 1, "only the builder's file is saved");
    assert_eq!(
        builder::meaning(&saved[0]["dashboard"]),
        builder::meaning(&held)
    );
    assert_eq!(saved[0]["folderUid"], format!("project-{PROJECT}"));

    // The next pass finds it held as the file says, and an unstaged project removes nothing.
    let second = sync.converge(&root, &projects, &projects).await;
    assert!(
        second.contains(&Outcome::Kept(PROJECT.into(), uid.clone())),
        "{second:?}"
    );
    assert!(
        !second.iter().any(|o| matches!(o, Outcome::Removed(..))),
        "{second:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_preview_of_a_bar_chart_reads_one_row_per_entity_and_a_text_panel_reads_nothing() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let preview = json!({ "board": board(), "panel": 7 });
    let answer = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(preview),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(body(&answer)["panel"]["options"]["xField"], "name");
    let queries = || async {
        grafana
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path().ends_with("/ds/query"))
            .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .collect::<Vec<_>>()
    };
    let asked = queries().await;
    let sent = &asked.last().expect("a query")["queries"][0];
    assert_eq!(sent["attributes"], json!(["name", "availableBikeNumber"]));
    assert_eq!(sent["format"], "table");
    assert_eq!(sent["queryType"], "entity");

    let preview = json!({ "board": board(), "panel": 8 });
    let answer = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(preview),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(body(&answer)["panel"]["type"], "text");
    assert!(body(&answer)["panel"]["options"]["content"]
        .as_str()
        .is_some_and(|c| c.starts_with("## Bike stations")));
    assert_eq!(
        queries().await.len(),
        asked.len(),
        "a text panel runs no query"
    );

    // A new kind on an Endpoint the person cannot read is refused by its field, as the others.
    let mut sent = board();
    sent["panels"][6]["endpoint"] = json!("air-closed");
    let answer = send(
        &state,
        person("zoe"),
        "POST",
        BUILDER,
        Some(json!({ "board": sent })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(
        answer.text.contains("panels[6].endpoint"),
        "{}",
        answer.text
    );
    assert!(committed_files(&shako, REPO).await.is_empty());

    // A pie chart over a text attribute names the kind and what the model holds, in words.
    let mut sent = board();
    sent["panels"][5]["attribute"] = json!("status");
    let answer = send(
        &state,
        person("eva"),
        "POST",
        BUILDER,
        Some(json!({ "board": sent })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", answer.text);
    assert!(
        answer.text.contains(
            "panels[5].kind: a pie chart cannot draw status: the data model holds it as text"
        ),
        "{}",
        answer.text
    );
}
