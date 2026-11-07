//! T-6509: the Grafana page of a project. The dropdowns offer only what the person may read, the
//! save checks every field again, nothing of the administrator reaches an answer, and what a
//! restart of Grafana takes comes back from the Portal's rows. The roles' cells are in the
//! authorization matrix; the save itself needs PostgreSQL (`JC_PORTAL_TEST_DATABASE_URL`) and
//! skips with a note without it.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::config::{Config, GrafanaConfig};
use joinedcontext_portal::grafana::{self, Client};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const PAGE: &str = "/api/v1/projects/helsinki/grafana";
const CLOSED_SLUG: &str = "kvhnixtbe5dex2wt2kx3i37dzw6yzsxt";
const PUBLIC_SLUG: &str = "jtjnhkaau64zmzz5csmcwem52icvufy2";
const PASSWORD: &str = "the-administrators-password";

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

/// Two spaces of one project: `bikes` with a closed and a public Endpoint and a model, `air` with
/// one closed Endpoint. A person in `bikes` alone must never be offered `air`.
fn world(grafana: Option<&MockServer>) -> AppState {
    let mut config = Config::for_tests();
    config.gateway_url = Some("http://gateway:8080".into());
    config.grafana = grafana.map(|server| GrafanaConfig {
        url: format!("{}/grafana", server.uri()),
        public_url: "https://dev.example/grafana".into(),
        user: "grafana_admin".into(),
        password: PASSWORD.into(),
    });
    let state = AppState::new(config, None);
    for space in ["bikes", "air"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, PROJECT, json!({})));
    }
    for (name, title, space, slug, audience) in [
        (
            "bikes-closed",
            "Bike stations",
            "bikes",
            CLOSED_SLUG,
            "private",
        ),
        (
            "bikes-open",
            "Bike stations, open",
            "bikes",
            PUBLIC_SLUG,
            "public",
        ),
        (
            "air-closed",
            "Air quality",
            "air",
            "ydp6nddchg4sewmylsiclg4377mlldrj",
            "private",
        ),
    ] {
        let mut endpoint = envelope(
            "Endpoint",
            name,
            PROJECT,
            json!({ "contextSpaceRef": space, "slug": slug, "audience": audience }),
        );
        endpoint.metadata.title = Some(jc_core::i18n::Text::Plain(title.to_owned()));
        state.mirror.upsert(endpoint);
    }
    let read = json!({ "kinds": ["Project", "ContextSpace", "Endpoint"], "verbs": ["read"] });
    role(&state, "reader", json!([read]));
    role(
        &state,
        "drafter",
        json!([read, { "kinds": ["Dashboard"], "verbs": ["propose"] }]),
    );
    binding(&state, "vera", "reader", json!({ "project": PROJECT }));
    binding(&state, "eva", "drafter", json!({ "project": PROJECT }));
    role(
        &state,
        "dashboards",
        json!([{ "kinds": ["Dashboard"], "verbs": ["propose"] }]),
    );
    binding(&state, "zoe", "reader", json!({ "contextSpace": "bikes" }));
    binding(&state, "zoe", "dashboards", json!({ "project": PROJECT }));
    binding(&state, "ola", "drafter", json!({ "project": "bbsk" }));
    state.model_schemas.replace(std::collections::HashMap::from([(
        (PROJECT.to_owned(), "bikes".to_owned()),
        std::sync::Arc::new(ModelSchema::compile(
            "bikes",
            "1.2.0",
            &json!({ "definitions": { "BikeHireDockingStation": {
                "required": ["id", "type"],
                "properties": {
                    "id": { "type": "string" }, "type": { "type": "string" },
                    "name": { "type": ["string", "null"], "x-ngsi-ld-kind": "Property" },
                    "availableBikeNumber": { "type": ["integer", "null"], "x-ngsi-ld-kind": "Property" },
                    "location": { "type": ["object", "null"], "x-ngsi-ld-kind": "GeoProperty" },
                    "refOwner": { "type": ["string", "null"], "x-ngsi-ld-kind": "Relationship" }
                }
            }}}),
            &["BikeHireDockingStation".to_owned()],
            false,
        )),
    )]));
    state
}

/// A Grafana holding the organization, and a datasource for the closed Endpoint of `bikes`.
async fn grafana_with_organization() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/grafana/api/orgs/name/helsinki"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": 2, "name": "helsinki" })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/grafana/api/datasources"))
        .and(header("X-Grafana-Org-Id", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "uid": "bikes-closed", "type": "ngsild-grafana-datasource",
              "url": format!("http://gateway:8080/api/endpoint/{CLOSED_SLUG}") },
            { "uid": "prometheus", "type": "prometheus", "url": "http://prometheus:9090" },
        ])))
        .mount(&server)
        .await;
    server
}

fn body(answer: &common::Answer) -> Value {
    serde_json::from_str(&answer.text).unwrap_or_default()
}

fn asked(overrides: Value) -> Value {
    let mut body = json!({
        "title": "Free bikes",
        "endpoint": "bikes-closed",
        "type": "BikeHireDockingStation",
        "attribute": "availableBikeNumber",
        "kind": "stat",
    });
    for (key, value) in overrides.as_object().expect("an object") {
        body[key] = value.clone();
    }
    body
}

async fn writes_of(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() != "GET")
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

#[tokio::test]
async fn a_project_without_grafana_says_so_and_a_save_is_refused_with_the_reason() {
    let state = world(None);
    let page = send(&state, person("eva"), "GET", PAGE, None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(body(&page)["ready"], false);
    assert!(body(&page)["reason"]
        .as_str()
        .is_some_and(|why| why.contains("not set up")));
    assert_eq!(body(&page)["mayCreate"], true);
}

#[tokio::test]
async fn a_project_grafana_does_not_hold_says_who_can_add_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/grafana/api/orgs/name/helsinki"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let state = world(Some(&server));
    let page = send(&state, person("eva"), "GET", PAGE, None).await;
    assert_eq!(body(&page)["ready"], false);
    assert!(body(&page)["reason"]
        .as_str()
        .is_some_and(|why| why.contains("operator")));
}

#[tokio::test]
async fn a_grafana_that_is_away_is_a_reason_on_the_page_and_never_its_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let state = world(Some(&server));
    let page = send(&state, person("eva"), "GET", PAGE, None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(body(&page)["ready"], false);
    assert!(!page.text.clone().contains(PASSWORD));
}

#[tokio::test]
async fn the_dropdowns_offer_only_what_the_person_may_read() {
    let server = grafana_with_organization().await;
    let state = world(Some(&server));
    let names = |answer: &Value| -> Vec<String> {
        answer["endpoints"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|option| option["name"].as_str().expect("a name").to_owned())
            .collect()
    };

    let all = send(
        &state,
        person("eva"),
        "GET",
        &format!("{PAGE}/options"),
        None,
    )
    .await;
    assert_eq!(
        names(&body(&all)),
        ["air-closed", "bikes-closed", "bikes-open"]
    );
    let usable: Vec<bool> = body(&all)["endpoints"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|option| option["usable"] == true)
        .collect();
    assert_eq!(
        usable,
        [false, true, true],
        "a closed Endpoint without a datasource is not usable"
    );

    let one_space = send(
        &state,
        person("zoe"),
        "GET",
        &format!("{PAGE}/options"),
        None,
    )
    .await;
    assert_eq!(names(&body(&one_space)), ["bikes-closed", "bikes-open"]);

    let hidden = send(
        &state,
        person("zoe"),
        "GET",
        &format!("{PAGE}/options?endpoint=air-closed"),
        None,
    )
    .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);

    let chosen = send(
        &state,
        person("zoe"),
        "GET",
        &format!("{PAGE}/options?endpoint=bikes-closed&type=BikeHireDockingStation"),
        None,
    )
    .await;
    assert_eq!(body(&chosen)["types"], json!(["BikeHireDockingStation"]));
    let attributes = body(&chosen)["attributes"].clone();
    let kinds_of = |name: &str| {
        attributes
            .as_array()
            .expect("a list")
            .iter()
            .find(|attribute| attribute["name"] == name)
            .map(|attribute| attribute["kinds"].clone())
    };
    assert_eq!(
        kinds_of("availableBikeNumber"),
        Some(json!(["timeseries", "stat", "table"]))
    );
    assert_eq!(kinds_of("location"), Some(json!(["table", "map"])));
    assert_eq!(kinds_of("refOwner"), None, "a link draws nothing");
}

#[tokio::test]
async fn a_person_of_another_project_finds_nothing_here() {
    let server = grafana_with_organization().await;
    let state = world(Some(&server));
    for (http, uri, body) in [
        ("GET", PAGE.to_owned(), None),
        ("GET", format!("{PAGE}/options"), None),
        ("POST", PAGE.to_owned(), Some(asked(json!({})))),
        ("DELETE", format!("{PAGE}/abc"), None),
    ] {
        let answer = send(&state, person("ola"), http, &uri, body).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{http} {uri}");
    }
    assert!(writes_of(&server).await.is_empty());
}

#[tokio::test]
async fn a_reader_may_look_and_may_not_save_or_remove() {
    let server = grafana_with_organization().await;
    let state = world(Some(&server));
    let page = send(&state, person("vera"), "GET", PAGE, None).await;
    assert_eq!(body(&page)["mayCreate"], false);
    let saved = send(&state, person("vera"), "POST", PAGE, Some(asked(json!({})))).await;
    assert_eq!(saved.status, StatusCode::FORBIDDEN);
    let removed = send(
        &state,
        person("vera"),
        "DELETE",
        &format!("{PAGE}/abc"),
        None,
    )
    .await;
    assert_eq!(removed.status, StatusCode::FORBIDDEN);
    assert!(writes_of(&server).await.is_empty());
}

#[tokio::test]
async fn each_field_is_checked_again_on_the_save() {
    let server = grafana_with_organization().await;
    let state = world(Some(&server));
    for (who, overrides, status, field, message) in [
        (
            "eva",
            json!({ "title": "  " }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "title",
            "1 to 120",
        ),
        (
            "eva",
            json!({ "title": "x".repeat(121) }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "title",
            "1 to 120",
        ),
        (
            "eva",
            json!({ "title": "ghp_0123456789abcdefghijklmnopqrstuvwxyz" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "title",
            "credential",
        ),
        (
            "eva",
            json!({ "endpoint": "no-such" }),
            StatusCode::FORBIDDEN,
            "endpoint",
            "choose one from the list",
        ),
        (
            "zoe",
            json!({ "endpoint": "air-closed" }),
            StatusCode::FORBIDDEN,
            "endpoint",
            "choose one from the list",
        ),
        (
            "eva",
            json!({ "endpoint": "air-closed" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "endpoint",
            "pins no data model",
        ),
        (
            "eva",
            json!({ "type": "Ship" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "type",
            "choose one from the list",
        ),
        (
            "eva",
            json!({ "attribute": "secretNote" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "attribute",
            "choose one from the list",
        ),
        (
            "eva",
            json!({ "attribute": "location", "kind": "stat" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "kind",
            "choose another kind",
        ),
        (
            "eva",
            json!({ "attribute": "availableBikeNumber", "kind": "map" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "kind",
            "choose another kind",
        ),
        (
            "eva",
            json!({ "attribute": "name" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "kind",
            "choose another kind",
        ),
    ] {
        let answer = send(
            &state,
            person(who),
            "POST",
            PAGE,
            Some(asked(overrides.clone())),
        )
        .await;
        assert_eq!(answer.status, status, "{overrides}: {}", answer.text);
        let text = answer.text.clone();
        assert!(
            text.contains(field) && text.contains(message),
            "{overrides}: {text}"
        );
    }
    let unknown = send(
        &state,
        person("eva"),
        "POST",
        PAGE,
        Some(asked(json!({ "folder": "x" }))),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    let kind = send(
        &state,
        person("eva"),
        "POST",
        PAGE,
        Some(asked(json!({ "kind": "pie" }))),
    )
    .await;
    assert_eq!(kind.status, StatusCode::BAD_REQUEST);
    assert!(
        writes_of(&server).await.is_empty(),
        "nothing reached Grafana"
    );
}

/// The two tests below share the table.
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
    sqlx::query("DELETE FROM grafana_visualizations")
        .execute(&pool)
        .await
        .expect("a clean table");
    Some(pool)
}

/// Grafana takes a save, then behaves as after a restart: the dashboard is gone.
async fn mount_saves(server: &MockServer) {
    for (http, route) in [
        ("POST", "/grafana/api/dashboards/db"),
        ("POST", "/grafana/api/datasources"),
    ] {
        Mock::given(method(http))
            .and(path(route))
            .and(header("X-Grafana-Org-Id", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            "^/grafana/api/dashboards/uid/",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(wiremock::matchers::path_regex(
            "^/grafana/api/dashboards/uid/",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
}

fn bodies_to(requests: &[wiremock::Request], route: &str) -> Vec<Value> {
    requests
        .iter()
        .filter(|request| request.method.as_str() == "POST" && request.url.path() == route)
        .map(|request| serde_json::from_slice(&request.body).expect("json"))
        .collect()
}

#[tokio::test]
async fn a_save_lands_in_the_project_folder_comes_back_after_a_restart_and_can_be_removed() {
    let _alone = DATABASE.lock().await;
    let Some(pool) = database().await else { return };
    let server = grafana_with_organization().await;
    mount_saves(&server).await;
    let mut state = world(Some(&server));
    state.db = Some(pool.clone());

    let saved = send(&state, person("eva"), "POST", PAGE, Some(asked(json!({})))).await;
    assert_eq!(saved.status, StatusCode::CREATED, "{}", saved.text);
    let id = body(&saved)["id"].as_str().expect("an id").to_owned();
    assert_eq!(
        body(&saved)["openUrl"],
        format!("https://dev.example/grafana/d/pv-{id}?orgId=2")
    );
    assert!(!saved.text.clone().contains(PASSWORD));

    let requests = server.received_requests().await.expect("recorded");
    let dashboards = bodies_to(&requests, "/grafana/api/dashboards/db");
    assert_eq!(dashboards[0]["folderUid"], "project-helsinki");
    assert_eq!(
        dashboards[0]["dashboard"]["panels"][0]["datasource"]["uid"], "bikes-closed",
        "a closed Endpoint reads through the datasource that forwards the person's token"
    );
    assert!(bodies_to(&requests, "/grafana/api/datasources").is_empty());
    assert!(!dashboards[0].to_string().contains(PASSWORD));

    let listed = send(&state, person("vera"), "GET", PAGE, None).await;
    assert_eq!(body(&listed)["ready"], true);
    assert_eq!(body(&listed)["items"][0]["title"], "Free bikes");
    assert_eq!(
        body(&listed)["items"][0]["endpointTitle"],
        "Bike stations",
        "a saved row names the Endpoint as its dropdown does"
    );

    let config = state.config.grafana.clone().expect("configured");
    grafana::sweep(&state, &pool, &Client::new(&config)).await;
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(
        bodies_to(&requests, "/grafana/api/dashboards/db").len(),
        2,
        "the sweep puts back the dashboard Grafana lost"
    );

    let removed = send(
        &state,
        person("eva"),
        "DELETE",
        &format!("{PAGE}/{id}"),
        None,
    )
    .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let empty = send(&state, person("eva"), "GET", PAGE, None).await;
    assert_eq!(body(&empty)["items"], json!([]));
}

#[tokio::test]
async fn a_public_endpoint_gets_an_anonymous_datasource_and_a_closed_one_without_a_datasource_is_refused(
) {
    let _alone = DATABASE.lock().await;
    let Some(pool) = database().await else { return };
    let server = grafana_with_organization().await;
    mount_saves(&server).await;
    let mut state = world(Some(&server));
    state.db = Some(pool);
    state.model_schemas.replace({
        let model = state.model_schemas.get(PROJECT, "bikes").expect("a model");
        std::collections::HashMap::from([
            ((PROJECT.to_owned(), "bikes".to_owned()), model.clone()),
            ((PROJECT.to_owned(), "air".to_owned()), model),
        ])
    });

    let closed = send(
        &state,
        person("eva"),
        "POST",
        PAGE,
        Some(asked(json!({ "endpoint": "air-closed" }))),
    )
    .await;
    assert_eq!(closed.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(closed.text.contains("operator"));
    assert!(
        closed.text.contains("Air quality") && !closed.text.contains("air-closed"),
        "the refusal uses the name the dropdown shows: {}",
        closed.text
    );

    let open = send(
        &state,
        person("eva"),
        "POST",
        PAGE,
        Some(asked(json!({ "endpoint": "bikes-open" }))),
    )
    .await;
    assert_eq!(open.status, StatusCode::CREATED, "{}", open.text);
    let requests = server.received_requests().await.expect("recorded");
    let sources = bodies_to(&requests, "/grafana/api/datasources");
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources[0]["url"],
        format!("http://gateway:8080/api/endpoint/{PUBLIC_SLUG}")
    );
    assert!(sources[0]["jsonData"].get("oauthPassThru").is_none());
}

/// The client against a real Grafana (`JC_PORTAL_TEST_GRAFANA_URL`, admin/admin, an organization
/// `helsinki` and folder `project-helsinki` made first): the wire shapes the mock above assumes.
#[tokio::test]
async fn the_client_speaks_a_real_grafanas_api() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_GRAFANA_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_GRAFANA_URL is not set");
        return;
    };
    let config = GrafanaConfig {
        url,
        public_url: "https://dev.example/grafana".into(),
        user: "admin".into(),
        password: "admin".into(),
    };
    let client = Client::new(&config);
    assert_eq!(
        client
            .organization("no-such-project")
            .await
            .expect("an answer"),
        None
    );
    let org = client
        .organization(PROJECT)
        .await
        .expect("an answer")
        .expect("the organization");
    let source = grafana::Source {
        name: "bikes-open",
        slug: PUBLIC_SLUG,
        public: true,
    };
    let uid = client
        .datasource_for(org, &source, Some("http://gateway:8080"))
        .await
        .expect("an answer")
        .expect("a datasource");
    let again = client
        .datasource_for(org, &source, Some("http://gateway:8080"))
        .await
        .expect("an answer");
    assert_eq!(
        again.as_deref(),
        Some(uid.as_str()),
        "a second read finds the first by its address"
    );
    for kind in grafana::Kind::ALL {
        let row = grafana::Visualization {
            id: grafana::new_id(),
            project: PROJECT.to_owned(),
            title: format!("Check {}", kind.as_str()),
            endpoint: "bikes-open".to_owned(),
            entity_type: "BikeHireDockingStation".to_owned(),
            attribute: "location".to_owned(),
            kind,
            created_at: String::new(),
        };
        let dashboard = grafana::dashboard_of(&row, &uid, true);
        client.save(org, &dashboard, PROJECT).await.expect("saved");
        client
            .save(org, &dashboard, PROJECT)
            .await
            .expect("saved again");
        client.remove(org, &row.uid()).await.expect("removed");
        client
            .remove(org, &row.uid())
            .await
            .expect("already gone is fine");
    }
}
