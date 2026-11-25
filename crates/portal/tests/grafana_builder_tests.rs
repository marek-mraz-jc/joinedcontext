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

/// The history of `uid`'s file on main: each `(sha, author)`, newest first.
async fn history(shako: &MockServer, uid: &str, commits: &[(&str, &str)]) {
    let rows: Vec<Value> = commits
        .iter()
        .map(|(sha, author)| json!({
            "Sha": sha, "Message": "change dashboard Bike stations\n\nbody",
            "Author": author, "Email": format!("{author}@hel.fi"), "Date": "2026-11-16T10:00:00Z"
        }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.History(Ref=@r,Path=@p,Top=@n)")))
        .and(query_param(
            "@p",
            format!("'projects/{PROJECT}/grafana/{uid}.json'"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": rows })))
        .with_priority(1)
        .mount(shako)
        .await;
}

/// T-7150: a dashboard's versions are the approved Changes of its file. A reader lists them and
/// reads one as choices, never an e-mail address and never a version naming an Endpoint they may
/// not read; only a person who may propose dashboards restores one, as a Change of the old
/// choices compiled again; restoring the version it is already is refused.
#[tokio::test]
async fn a_reader_reads_a_dashboards_versions_and_only_a_proposer_restores_one() {
    const OLD: &str = "1111111111111111111111111111111111111111";
    const NEW: &str = "2222222222222222222222222222222222222222";
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    binding(&state, "ana", "reader", json!({ "contextSpace": "air" }));
    let (uid, old_text) = created(&state, &shako).await;
    let new_text = old_text.replacen("\"Bike stations\"", "\"Bike stations, renamed\"", 1);
    assert_ne!(new_text, old_text, "the fixture renames the dashboard");
    history(&shako, &uid, &[(NEW, "eva"), (OLD, "ada")]).await;
    holds(&shako, &uid, "main", "blob-new", &new_text).await;
    holds(&shako, &uid, NEW, "blob-new", &new_text).await;
    holds(&shako, &uid, OLD, "blob-old", &old_text).await;
    let versions = format!("{BUILDER}/{uid}/versions");

    let list = send(&state, person("vera"), "GET", &versions, None).await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text);
    let items = body(&list)["items"].clone();
    assert_eq!(items[0]["sha"], NEW);
    assert_eq!(items[1]["author"], "ada");
    assert_eq!(items[0]["message"], "change dashboard Bike stations");
    assert!(
        !list.text.contains("@hel.fi"),
        "no e-mail address: {}",
        list.text
    );

    let old = send(
        &state,
        person("vera"),
        "GET",
        &format!("{versions}/{OLD}"),
        None,
    )
    .await;
    assert_eq!(old.status, StatusCode::OK, "{}", old.text);
    assert_eq!(body(&old)["board"]["title"], "Bike stations");
    let new = send(
        &state,
        person("vera"),
        "GET",
        &format!("{versions}/{NEW}"),
        None,
    )
    .await;
    assert_eq!(body(&new)["board"]["title"], "Bike stations, renamed");

    // A member who may not read the Endpoint the version names; a sha the history does not list;
    // a person of another project.
    let hidden = send(
        &state,
        person("ana"),
        "GET",
        &format!("{versions}/{OLD}"),
        None,
    )
    .await;
    assert_eq!(hidden.status, StatusCode::FORBIDDEN, "{}", hidden.text);
    assert!(!hidden.text.contains("bikes-open"), "{}", hidden.text);
    let stray = "3333333333333333333333333333333333333333";
    let unknown = send(
        &state,
        person("eva"),
        "GET",
        &format!("{versions}/{stray}"),
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    let outside = send(&state, person("ola"), "GET", &versions, None).await;
    assert_eq!(outside.status, StatusCode::NOT_FOUND, "{}", outside.text);

    // Restore: a reader is refused, the version it is already is nothing to restore, an
    // earlier one is a Change writing its choices back.
    let restore = |sha: &str| format!("{versions}/{sha}/restore");
    let refused = send(&state, person("vera"), "POST", &restore(OLD), None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    let same = send(&state, person("eva"), "POST", &restore(NEW), None).await;
    assert_eq!(same.status, StatusCode::CONFLICT, "{}", same.text);
    let restored = send(&state, person("eva"), "POST", &restore(OLD), None).await;
    assert_eq!(restored.status, StatusCode::ACCEPTED, "{}", restored.text);
    let (file, text) = committed_files(&shako, REPO)
        .await
        .last()
        .cloned()
        .expect("a committed file");
    assert_eq!(file, format!("projects/{PROJECT}/grafana/{uid}.json"));
    assert_eq!(text, old_text, "the old version, compiled again");
}

/// The saved charts' table, shared with grafana_tests: each test here clears only its own rows.
/// The tests that use the saved charts' table run one at a time: each clears its rows first.
static DATABASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<sqlx::PgPool> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let Some(url) = url else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("the test database");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrations");
    sqlx::query("DELETE FROM grafana_visualizations WHERE id LIKE 't7161%'")
        .execute(&pool)
        .await
        .expect("a clean table");
    Some(pool)
}

async fn saved_ids(pool: &sqlx::PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT id FROM grafana_visualizations WHERE id LIKE 't7161%' ORDER BY id")
        .fetch_all(pool)
        .await
        .expect("the rows")
}

/// T-7161: three charts saved one by one (rows name no person, so the dashboard is the
/// project's) move into one Saved charts dashboard. The dry run counts and writes nothing, the
/// first run proposes the dashboard, and once it is merged a run removes only the charts the
/// merged file draws: the one a reviewer retitled and the map the builder cannot draw stay.
#[tokio::test]
async fn saved_charts_move_into_one_dashboard_and_only_what_the_merged_file_draws_is_removed() {
    use joinedcontext_portal::grafana::{self, Kind, Visualization};
    let _alone = DATABASE.lock().await;
    let Some(pool) = database().await else { return };
    let shako = common::forge().await;
    let grafana_server = daiya().await;
    Mock::given(method("DELETE"))
        .and(wiremock::matchers::path_regex(
            "^/grafana/api/dashboards/uid/",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&grafana_server)
        .await;
    let mut state = world(&shako, &grafana_server);
    state.db = Some(pool.clone());
    common::keyed_project(&state, &pool, PROJECT, "grafana-tests").await;
    let key = joinedcontext_portal::project_ids::key_of(&pool, &state.mirror, PROJECT)
        .await
        .expect("the registry")
        .expect("a keyed project");
    for (id, title, kind, attribute) in [
        ("t7161a", "Free bikes", Kind::Stat, "availableBikeNumber"),
        (
            "t7161b",
            "Free bikes over time",
            Kind::Timeseries,
            "availableBikeNumber",
        ),
        ("t7161c", "Stations", Kind::Map, "location"),
    ] {
        grafana::insert(
            &pool,
            key,
            &Visualization {
                id: id.into(),
                project: PROJECT.into(),
                title: title.into(),
                endpoint: "bikes-open".into(),
                entity_type: "BikeHireDockingStation".into(),
                attribute: attribute.into(),
                kind,
                created_at: String::new(),
            },
        )
        .await
        .expect("a saved chart");
    }
    let moves = format!("{BUILDER}/saved-charts");

    let reader = send(
        &state,
        person("vera"),
        "POST",
        &moves,
        Some(json!({ "dryRun": true })),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    let unknown = send(
        &state,
        person("eva"),
        "POST",
        &moves,
        Some(json!({ "dry": true })),
    )
    .await;
    assert_eq!(
        unknown.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        unknown.text
    );

    let dry = send(
        &state,
        person("eva"),
        "POST",
        &moves,
        Some(json!({ "dryRun": true })),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    let report = body(&dry);
    assert_eq!(report["stage"], "propose");
    assert_eq!(report["before"], 3);
    assert_eq!(report["panels"], 2);
    assert_eq!(report["moved"], json!(["t7161a", "t7161b"]));
    assert_eq!(report["kept"][0]["id"], "t7161c");
    assert!(
        committed_files(&shako, REPO).await.is_empty(),
        "a dry run writes nothing"
    );

    let proposed = send(&state, person("eva"), "POST", &moves, Some(json!({}))).await;
    assert_eq!(proposed.status, StatusCode::ACCEPTED, "{}", proposed.text);
    let uid = body(&proposed)["uid"].as_str().expect("a uid").to_owned();
    assert!(builder::is_builder_uid(&uid));
    assert!(body(&proposed)["change"].is_object(), "{}", proposed.text);
    let (file, text) = committed_files(&shako, REPO)
        .await
        .last()
        .cloned()
        .expect("a file");
    assert_eq!(file, format!("projects/{PROJECT}/grafana/{uid}.json"));
    assert_eq!(
        saved_ids(&pool).await.len(),
        3,
        "nothing goes before the merge"
    );

    // The reviewer retitled one panel before approving.
    let merged = text.replacen("\"Free bikes over time\"", "\"Bikes over time\"", 1);
    assert_ne!(merged, text);
    holds(&shako, &uid, "main", "blob-merged", &merged).await;
    let dry = send(
        &state,
        person("eva"),
        "POST",
        &moves,
        Some(json!({ "dryRun": true })),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    assert_eq!(body(&dry)["stage"], "remove");
    assert_eq!(body(&dry)["moved"], json!(["t7161a"]));
    assert_eq!(body(&dry)["kept"].as_array().map(Vec::len), Some(2));
    assert_eq!(saved_ids(&pool).await.len(), 3, "a dry run removes nothing");

    let run = send(&state, person("eva"), "POST", &moves, Some(json!({}))).await;
    assert_eq!(run.status, StatusCode::OK, "{}", run.text);
    assert_eq!(saved_ids(&pool).await, ["t7161b", "t7161c"]);
    let removed: Vec<String> = grafana_server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|request| request.method.as_str() == "DELETE")
        .map(|request| request.url.path().to_owned())
        .collect();
    assert_eq!(removed, ["/grafana/api/dashboards/uid/pv-t7161a"]);
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
    let mut counted = board();
    counted["variables"][0]["attribute"] = json!("availableBikeNumber");
    cases.push((counted, "variables[0].attribute"));
    let mut linked = board();
    linked["panels"][0]["filters"][0] =
        json!({ "attribute": "refOwner", "op": "eq", "value": "urn:ngsi-ld:Owner:1" });
    cases.push((linked, "panels[0].filters[0].attribute"));
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

fn sync_to(service: &MockServer) -> DashboardSync {
    DashboardSync::new(GrafanaConfig {
        url: service.uri(),
        public_url: "https://dev.example/grafana".into(),
        user: "grafana_admin".into(),
        password: "unused".into(),
    })
}

/// T-7816: each refusal of the dashboards service costs only its own project. A service that
/// derives organizations gets one made for a project it lacks, a failed make skips that
/// project, a held dashboard that differs and cannot be saved is refused naming the cause, and
/// a search that fails removes nothing.
#[tokio::test]
async fn the_reconciler_skips_a_project_the_service_refuses_and_keeps_going() {
    let text = std::fs::read_to_string(FIXTURE).expect("the fixture");
    let service = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": "daiya" })))
        .mount(&service)
        .await;
    for project in ["made", "broken"] {
        Mock::given(method("GET"))
            .and(path(format!("/api/orgs/name/{project}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&service)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/api/orgs"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "name": "made" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "orgId": 5 })))
        .mount(&service)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/orgs"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "name": "broken" }),
        ))
        .respond_with(ResponseTemplate::new(500))
        .mount(&service)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/folders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/dashboards/uid/{FIXTURE_UID}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "dashboard": { "uid": FIXTURE_UID, "title": "older" } })),
        )
        .mount(&service)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/dashboards/db"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&service)
        .await;
    let root = staged("refusals", &[("made", FIXTURE_UID, &text)]);
    let outcomes = sync_to(&service)
        .converge(&root, &["made".to_owned(), "broken".to_owned()], &[])
        .await;
    assert!(
        matches!(&outcomes[..], [Outcome::Refused(p, u, why)]
            if p == "made" && u == FIXTURE_UID && !why.is_empty()),
        "{outcomes:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// T-7816: a service that makes no organizations skips a project it does not hold, a delete
/// it refuses is not reported as a removal, and a staged tree's stray entries (a project with
/// no dashboards folder, a folder named like a file) are passed over. A service that is down
/// stops the pass before it writes anything.
#[tokio::test]
async fn an_unknown_project_a_refused_delete_and_a_down_service_remove_nothing() {
    let service = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": "grafana" })))
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/orgs/name/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/orgs/name/kept"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 3 })))
        .mount(&service)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{ "uid": "jcd-00000000000000bb" }])),
        )
        .mount(&service)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/dashboards/uid/jcd-00000000000000bb"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&service)
        .await;
    let root = staged("strays", &[]);
    std::fs::create_dir_all(root.join("projects/gone")).expect("a project without grafana/");
    std::fs::create_dir_all(root.join("projects/kept/grafana/jcd-00000000000000cc.json"))
        .expect("a folder named like a file");
    let outcomes = sync_to(&service)
        .converge(&root, &["gone".to_owned(), "kept".to_owned()], &[])
        .await;
    assert!(outcomes.is_empty(), "{outcomes:?}");
    assert!(builder::files_in(&root).is_empty());
    let _ = std::fs::remove_dir_all(&root);

    let down = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&down)
        .await;
    let nowhere = std::env::temp_dir().join(format!("jc-dashboards-none-{}", std::process::id()));
    assert!(
        builder::files_in(&nowhere).is_empty(),
        "no projects/ at all"
    );
    let outcomes = sync_to(&down)
        .converge(&nowhere, &["a".to_owned(), "b".to_owned()], &[])
        .await;
    assert!(outcomes.is_empty(), "{outcomes:?}");
    let written = down.received_requests().await.unwrap_or_default();
    assert!(written
        .iter()
        .all(|r| r.method == wiremock::http::Method::GET));
    assert_eq!(written.len(), 1, "the pass stops at the first refusal");
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
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    let errors = &body(&answer)["errors"][0];
    assert_eq!(errors["path"], "panels[5].kind");
    assert!(
        errors["message"]
            .as_str()
            .is_some_and(|m| m
                .starts_with("a pie chart cannot draw status: the data model holds it as text")),
        "{}",
        answer.text
    );
}

/// The dashboards service with a datasource for the closed Endpoint of `bikes` as well.
async fn daiya_reading_closed() -> MockServer {
    let server = daiya().await;
    Mock::given(method("GET"))
        .and(path("/grafana/api/datasources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "uid": DATASOURCE, "type": "ngsild-grafana-datasource",
              "url": format!("http://gateway:8080/api/endpoint/{PUBLIC_SLUG}") },
            { "uid": "bikes-closed-ds", "type": "ngsild-grafana-datasource",
              "url": format!("http://gateway:8080/api/endpoint/{CLOSED_SLUG}") },
            { "uid": "prometheus", "type": "prometheus", "url": "http://prometheus:9090" },
            { "uid": "elsewhere-ds", "type": "ngsild-grafana-datasource",
              "url": "http://gateway:8080/api/endpoint/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz" },
        ])))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// A variable's values are read as the person through the dashboards service, sorted and
/// deduplicated; a query parameter the door does not take, a reader, a closed Endpoint the
/// service cannot read and a query the service fails are each refused with the reason.
#[tokio::test]
async fn a_variables_values_are_read_as_the_person_and_each_failure_says_why() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let values = format!("{BUILDER}/values?endpoint=bikes-open&type=BikeHireDockingStation");
    let answer = send(
        &state,
        person("eva"),
        "GET",
        &format!("{values}&attribute=status"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(
        body(&answer)["options"],
        json!([{ "text": "closed", "value": "closed" }, { "text": "open", "value": "open" }])
    );
    let stray = send(
        &state,
        person("eva"),
        "GET",
        &format!("{values}&attribute=status&sql=1"),
        None,
    )
    .await;
    assert_eq!(stray.status, StatusCode::BAD_REQUEST, "{}", stray.text);
    let reader = send(
        &state,
        person("vera"),
        "GET",
        &format!("{values}&attribute=status"),
        None,
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    let number = send(
        &state,
        person("eva"),
        "GET",
        &format!("{values}&attribute=availableBikeNumber"),
        None,
    )
    .await;
    assert_eq!(
        number.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        number.text
    );
    let closed = send(
        &state,
        person("eva"),
        "GET",
        &format!(
            "{BUILDER}/values?endpoint=bikes-closed&type=BikeHireDockingStation&attribute=status"
        ),
        None,
    )
    .await;
    assert_eq!(
        closed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        closed.text
    );
    assert_eq!(
        body(&closed)["errors"][0]["path"],
        "endpoint",
        "{}",
        closed.text
    );

    let failing = daiya().await;
    Mock::given(method("POST"))
        .and(path("/grafana/api/ds/query"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "error": "the broker did not answer in time" })),
        )
        .with_priority(1)
        .mount(&failing)
        .await;
    let state = world(&shako, &failing);
    let answer = send(
        &state,
        person("eva"),
        "GET",
        &format!("{values}&attribute=status"),
        None,
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("the broker did not answer in time"),
        "{}",
        answer.text
    );
}

/// A panel on a closed Endpoint the dashboards service holds no datasource for is refused by its
/// field before anything is written; one it holds previews with the person's token, which a
/// session without one does not carry: the panel says so instead of failing the request.
#[tokio::test]
async fn a_closed_endpoint_needs_its_datasource_and_previews_with_the_persons_token() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let mut sent = board();
    sent["panels"][1]["endpoint"] = json!("bikes-closed");
    let answer = send(
        &state,
        person("eva"),
        "POST",
        BUILDER,
        Some(json!({ "board": sent.clone() })),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    assert!(
        body(&answer)["errors"]
            .to_string()
            .contains("panels[1].endpoint"),
        "{}",
        answer.text
    );
    assert!(committed_files(&shako, REPO).await.is_empty());

    let grafana = daiya_reading_closed().await;
    let state = world(&shako, &grafana);
    let preview = json!({ "board": sent, "panel": 2 });
    let answer = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/preview"),
        Some(preview),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(answer.text.contains("sign in again"), "{}", answer.text);
}

/// A dashboard file over the size the builder writes is refused by `board` before it is proposed.
#[tokio::test]
async fn a_dashboard_over_the_file_limit_is_refused_before_it_is_proposed() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let many: Vec<String> = (0..builder::MAX_OPTIONS)
        .map(|i| format!("station-{i:03}-{}", "x".repeat(100)))
        .collect();
    Mock::given(method("POST"))
        .and(path("/grafana/api/ds/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "results": { "A": {
                "frames": [{ "fields": [{ "name": "status", "values": many }] }]
            }}})),
        )
        .with_priority(1)
        .mount(&grafana)
        .await;
    let state = world(&shako, &grafana);
    let mut sent = board();
    let variable = sent["variables"][0].clone();
    sent["variables"] = json!((0..builder::MAX_VARIABLES)
        .map(|i| {
            let mut copy = variable.clone();
            if i > 0 {
                copy["name"] = json!(format!("status{i}"));
            }
            copy
        })
        .collect::<Vec<_>>());
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
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("the dashboard is over 256 KiB")
            && answer.text.contains(r#""path":"board""#),
        "{}",
        answer.text
    );
    assert!(committed_files(&shako, REPO).await.is_empty());
}

/// A dashboard only an open Change holds reads from its branch; a uid neither main nor a Change
/// holds is not found, and so are a uid the builder did not mint, a version list with no commit
/// and a version the history does not hold.
#[tokio::test]
async fn a_dashboard_waiting_for_approval_reads_from_its_change_and_an_absent_one_is_not_found() {
    let shako = common::forge().await;
    let branch = format!("portal/grafana-{PROJECT}-{FIXTURE_UID}");
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [{
            "Number": 5, "HtmlUrl": "https://shako.example/pulls/5", "State": "open",
            "Title": "add dashboard Bike stations", "Head": branch, "Base": "main",
            "Mergeable": true, "Merged": false
        }, {
            "Number": 6, "HtmlUrl": "https://shako.example/pulls/6", "State": "open",
            "Title": "add dashboard Gone", "Head": format!("portal/grafana-{PROJECT}-{UID}"),
            "Base": "main", "Mergeable": true, "Merged": false
        }]})))
        .with_priority(1)
        .mount(&shako)
        .await;
    let text = std::fs::read_to_string(FIXTURE).expect("the fixture");
    holds(&shako, FIXTURE_UID, &branch, "blob-pr", &text).await;
    let grafana = daiya_reading_closed().await;
    let state = world(&shako, &grafana);
    let read = send(
        &state,
        person("eva"),
        "GET",
        &format!("{BUILDER}/{FIXTURE_UID}"),
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert_eq!(body(&read)["board"]["title"], "Bike stations");
    assert_eq!(body(&read)["board"]["panels"][0]["endpoint"], "bikes-open");
    assert_eq!(
        body(&read)["base"],
        Value::Null,
        "a Change's file is no base to edit from"
    );

    let branchless = send(
        &state,
        person("eva"),
        "GET",
        &format!("{BUILDER}/{UID}"),
        None,
    )
    .await;
    assert_eq!(
        branchless.status,
        StatusCode::NOT_FOUND,
        "a Change whose branch lost the file: {}",
        branchless.text
    );
    let absent = send(
        &state,
        person("eva"),
        "GET",
        &format!("{BUILDER}/jcd-ffffffffffffffff"),
        None,
    )
    .await;
    assert_eq!(absent.status, StatusCode::NOT_FOUND, "{}", absent.text);
    let minted = send(
        &state,
        person("vera"),
        "GET",
        &format!("{BUILDER}/not-ours/versions"),
        None,
    )
    .await;
    assert_eq!(minted.status, StatusCode::NOT_FOUND, "{}", minted.text);
    history(&shako, UID, &[]).await;
    let none = send(
        &state,
        person("vera"),
        "GET",
        &format!("{BUILDER}/{UID}/versions"),
        None,
    )
    .await;
    assert_eq!(none.status, StatusCode::NOT_FOUND, "{}", none.text);
    history(
        &shako,
        FIXTURE_UID,
        &[("3333333333333333333333333333333333333333", "eva")],
    )
    .await;
    let unknown = send(
        &state,
        person("vera"),
        "GET",
        &format!("{BUILDER}/{FIXTURE_UID}/versions/4444444444444444444444444444444444444444"),
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    assert!(unknown.text.contains("has no version"), "{}", unknown.text);
}

/// Restoring a version of a dashboard main no longer holds is not found; removing one whose file
/// names no title proposes the removal under its uid.
#[tokio::test]
async fn a_restore_needs_the_saved_dashboard_and_a_removal_names_an_untitled_one_by_uid() {
    const OLD: &str = "5555555555555555555555555555555555555555";
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let text = std::fs::read_to_string(FIXTURE).expect("the fixture");
    history(&shako, FIXTURE_UID, &[(OLD, "eva")]).await;
    holds(&shako, FIXTURE_UID, OLD, "blob-old", &text).await;
    let restore = send(
        &state,
        person("eva"),
        "POST",
        &format!("{BUILDER}/{FIXTURE_UID}/versions/{OLD}/restore"),
        None,
    )
    .await;
    assert_eq!(restore.status, StatusCode::NOT_FOUND, "{}", restore.text);

    holds(&shako, UID, "main", "blob-1", "not json").await;
    let removed = send(
        &state,
        person("eva"),
        "DELETE",
        &format!("{BUILDER}/{UID}"),
        None,
    )
    .await;
    assert_eq!(removed.status, StatusCode::ACCEPTED, "{}", removed.text);
    let commits: Vec<String> = shako
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.CommitFiles"))
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect();
    assert!(
        commits
            .iter()
            .any(|c| c.contains(&format!("remove dashboard {UID}"))),
        "{commits:?}"
    );
}

/// The builder's start lists no waiting dashboard when the forge holds no pull requests; with no
/// forge configured a create says so; a body that is not JSON is a 400.
#[tokio::test]
async fn a_forge_without_pull_requests_or_none_at_all_is_answered_plainly() {
    let shako = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests")))
        .respond_with(ResponseTemplate::new(404))
        .with_priority(1)
        .mount(&shako)
        .await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let start = send(&state, person("eva"), "GET", BUILDER, None).await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.text);
    assert_eq!(body(&start)["pending"], json!([]));
    assert_eq!(body(&start)["mayEdit"], true);

    let broken = common::send_body(
        &state,
        person("eva"),
        "POST",
        BUILDER,
        "application/json",
        axum::body::Body::from("{\"board\": "),
    )
    .await;
    assert_eq!(broken.status, StatusCode::BAD_REQUEST, "{}", broken.text);

    let mut forgeless = state.clone();
    forgeless.shako = None;
    let answer = send(
        &forgeless,
        person("eva"),
        "POST",
        BUILDER,
        Some(json!({ "board": board() })),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("git forge is not configured"),
        "{}",
        answer.text
    );
}

/// Moving saved charts needs the database that holds them; with nothing a dashboard can hold, a
/// proposing run is a conflict and writes nothing.
#[tokio::test]
async fn saved_charts_need_a_database_and_something_to_move() {
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let moves = format!("{BUILDER}/saved-charts");
    let answer = send(&state, person("eva"), "POST", &moves, Some(json!({}))).await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(answer.text.contains("no database"), "{}", answer.text);

    let _alone = DATABASE.lock().await;
    let Some(pool) = database().await else { return };
    let mut state = world(&shako, &grafana);
    state.db = Some(pool.clone());
    common::keyed_project(&state, &pool, PROJECT, "grafana-tests").await;
    let dry = send(
        &state,
        person("eva"),
        "POST",
        &moves,
        Some(json!({ "dryRun": true })),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    let answer = send(&state, person("eva"), "POST", &moves, Some(json!({}))).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("nothing to move"), "{}", answer.text);
    assert!(committed_files(&shako, REPO).await.is_empty());

    let uid = joinedcontext_portal::saved_charts::uid_of(PROJECT);
    let fixture = std::fs::read_to_string(FIXTURE).expect("the fixture");
    for (text, why) in [
        ("not json".to_owned(), "not JSON"),
        (
            fixture
                .replace(
                    "Values of status (BikeHireDockingStation, endpoint bikes-open)",
                    "Hand-written list",
                )
                .replace(FIXTURE_UID, &uid),
            "not written by the builder",
        ),
    ] {
        let shako = common::forge().await;
        holds(&shako, &uid, "main", "blob-merged", &text).await;
        let mut state = world(&shako, &grafana);
        state.db = Some(pool.clone());
        common::keyed_project(&state, &pool, PROJECT, "grafana-tests").await;
        let answer = send(&state, person("eva"), "POST", &moves, Some(json!({}))).await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{why}: {}",
            answer.text
        );
        assert!(answer.text.contains(why), "{why}: {}", answer.text);
    }
}

/// A file on main the builder cannot read back, as JSON or as choices, is refused by `file` on
/// every door that reads one; a listed version whose file is gone is not found, and one that does
/// not read back is refused by `version`.
#[tokio::test]
async fn a_file_the_builder_did_not_write_is_refused_by_the_door_that_reads_it() {
    const OLD: &str = "6666666666666666666666666666666666666666";
    const GONE: &str = "7777777777777777777777777777777777777777";
    let shako = common::forge().await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let fixture = std::fs::read_to_string(FIXTURE).expect("the fixture");
    let hand_written = fixture.replace(
        "Values of status (BikeHireDockingStation, endpoint bikes-open)",
        "Hand-written list",
    );
    assert_ne!(
        hand_written, fixture,
        "the fixture names its variable's source"
    );
    let other = "jcd-b1ce57a7105f0002";
    holds(&shako, FIXTURE_UID, "main", "blob-1", "not json").await;
    holds(
        &shako,
        other,
        "main",
        "blob-2",
        &hand_written.replace(FIXTURE_UID, other),
    )
    .await;
    for (uid, why) in [
        (FIXTURE_UID, "not JSON"),
        (other, "not written by the builder"),
    ] {
        let read = send(
            &state,
            person("eva"),
            "GET",
            &format!("{BUILDER}/{uid}"),
            None,
        )
        .await;
        assert_eq!(
            read.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            read.text
        );
        assert_eq!(body(&read)["errors"][0]["path"], "file", "{}", read.text);
        assert!(read.text.contains(why), "{why}: {}", read.text);
    }

    history(&shako, FIXTURE_UID, &[(GONE, "eva"), (OLD, "ada")]).await;
    holds(&shako, FIXTURE_UID, OLD, "blob-old", "[]").await;
    let versions = format!("{BUILDER}/{FIXTURE_UID}/versions");
    let gone = send(
        &state,
        person("vera"),
        "GET",
        &format!("{versions}/{GONE}"),
        None,
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.text);
    let old = send(
        &state,
        person("vera"),
        "GET",
        &format!("{versions}/{OLD}"),
        None,
    )
    .await;
    assert_eq!(old.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", old.text);
    assert_eq!(body(&old)["errors"][0]["path"], "version", "{}", old.text);
    history(&shako, other, &[(OLD, "ada")]).await;
    holds(
        &shako,
        other,
        OLD,
        "blob-old",
        &hand_written.replace(FIXTURE_UID, other),
    )
    .await;
    let old = send(
        &state,
        person("vera"),
        "GET",
        &format!("{BUILDER}/{other}/versions/{OLD}"),
        None,
    )
    .await;
    assert_eq!(old.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", old.text);
    assert!(
        old.text.contains("not written by the builder"),
        "{}",
        old.text
    );
}

/// A forge that fails listing pull requests fails the builder's start with the reason, rather
/// than hiding the dashboards that wait for approval; a reader lists no Change and is answered.
#[tokio::test]
async fn a_forge_failing_to_list_changes_fails_the_start() {
    let shako = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests")))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(1)
        .mount(&shako)
        .await;
    let grafana = daiya().await;
    let state = world(&shako, &grafana);
    let start = send(&state, person("eva"), "GET", BUILDER, None).await;
    assert!(start.status.is_server_error(), "{}", start.text);
    let reader = send(&state, person("vera"), "GET", BUILDER, None).await;
    assert_eq!(reader.status, StatusCode::OK, "{}", reader.text);
    assert_eq!(body(&reader)["mayEdit"], false);
}
