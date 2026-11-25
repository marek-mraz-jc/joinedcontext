//! The sweep that puts a person's visualizations back in Grafana (T-6509, T-7196, T-7823): what it
//! does with a row whose Endpoint is gone, a project Grafana holds no organization for, a
//! dashboard Grafana still holds, a closed Endpoint with no datasource, a project the registry
//! holds no key for, a Grafana that answers 500 and a database that does not answer.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names. Each test names its project
//! afresh, because the tests share one database.

mod common;

use common::unique::unique;
use common::{envelope, keyed_project};
use joinedcontext_portal::config::{Config, GrafanaConfig};
use joinedcontext_portal::grafana::{self, Client, Kind, Visualization};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A server of the test's own: the client keeps connections, and one to a pooled server dies with
/// the runtime of the test before.
async fn server() -> MockServer {
    MockServer::builder().start().await
}

fn config_for(server: &MockServer) -> Config {
    let mut config = Config::for_tests();
    config.grafana = Some(GrafanaConfig {
        url: server.uri(),
        public_url: "https://dev.example/grafana".into(),
        user: "grafana_admin".into(),
        password: "unused".into(),
    });
    config
}

/// A project of an Organization the registry holds, on the database.
async fn world(server: &MockServer) -> (AppState, sqlx::PgPool, String) {
    let state = common::on_database(AppState::new(config_for(server), None)).await;
    let pool = state.db.clone().expect("a database");
    let project = format!("p{}", unique());
    keyed_project(&state, &pool, &project, "cov-grafana").await;
    (state, pool, project)
}

/// An Endpoint of `project` and a visualization that reads it.
async fn keep(
    state: &AppState,
    pool: &sqlx::PgPool,
    project: &str,
    endpoint: &str,
    with_endpoint: bool,
) -> Visualization {
    if with_endpoint {
        state.mirror.upsert(envelope(
            "Endpoint",
            endpoint,
            project,
            json!({ "contextSpaceRef": "air", "slug": "airslug", "audience": "private" }),
        ));
    }
    let key = joinedcontext_portal::project_ids::key_of(pool, &state.mirror, project)
        .await
        .expect("a key")
        .expect("placed");
    let row = Visualization {
        id: grafana::new_id(),
        project: project.to_owned(),
        title: "Air".into(),
        endpoint: endpoint.to_owned(),
        entity_type: "AirQualityObserved".into(),
        attribute: "pm25".into(),
        kind: Kind::Stat,
        created_at: String::new(),
    };
    grafana::insert(pool, key, &row).await.expect("kept");
    row
}

async fn answer(server: &MockServer, verb: &str, route: &str, status: u16, body: Value) {
    Mock::given(method(verb))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

async fn paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

async fn sweep(state: &AppState, pool: &sqlx::PgPool) {
    let config = state.config.grafana.clone().expect("configured");
    grafana::sweep(state, pool, &Client::new(&config)).await;
}

#[tokio::test]
async fn a_row_whose_endpoint_is_gone_asks_grafana_nothing() {
    let server = server().await;
    let (state, pool, project) = world(&server).await;
    keep(&state, &pool, &project, "removed", false).await;
    sweep(&state, &pool).await;
    assert_eq!(paths(&server).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_project_grafana_holds_no_organization_for_is_left_alone() {
    let server = server().await;
    let (state, pool, project) = world(&server).await;
    keep(&state, &pool, &project, "air", true).await;
    answer(
        &server,
        "GET",
        &format!("/api/orgs/name/{project}"),
        404,
        json!({}),
    )
    .await;
    sweep(&state, &pool).await;
    assert_eq!(
        paths(&server).await,
        [format!("GET /api/orgs/name/{project}")]
    );
}

#[tokio::test]
async fn a_dashboard_grafana_still_holds_is_not_saved_again() {
    let server = server().await;
    let (state, pool, project) = world(&server).await;
    let row = keep(&state, &pool, &project, "air", true).await;
    answer(
        &server,
        "GET",
        &format!("/api/orgs/name/{project}"),
        200,
        json!({ "id": 3 }),
    )
    .await;
    answer(
        &server,
        "GET",
        &format!("/api/dashboards/uid/{}", row.uid()),
        200,
        json!({ "dashboard": {} }),
    )
    .await;
    sweep(&state, &pool).await;
    let seen = paths(&server).await;
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(!seen.iter().any(|request| request.starts_with("POST")));
}

#[tokio::test]
async fn a_closed_endpoint_with_no_datasource_is_not_put_back() {
    let server = server().await;
    let (state, pool, project) = world(&server).await;
    let row = keep(&state, &pool, &project, "air", true).await;
    answer(
        &server,
        "GET",
        &format!("/api/orgs/name/{project}"),
        200,
        json!({ "id": 3 }),
    )
    .await;
    answer(
        &server,
        "GET",
        &format!("/api/dashboards/uid/{}", row.uid()),
        404,
        json!({}),
    )
    .await;
    answer(&server, "GET", "/api/datasources", 200, json!([])).await;
    sweep(&state, &pool).await;
    let seen = paths(&server).await;
    assert!(
        seen.contains(&"GET /api/datasources".to_owned()),
        "{seen:?}"
    );
    assert!(!seen.iter().any(|request| request.starts_with("POST")));
}

#[tokio::test]
async fn a_lost_dashboard_of_a_closed_endpoint_is_saved_through_its_datasource() {
    let server = server().await;
    let (state, pool, project) = world(&server).await;
    let row = keep(&state, &pool, &project, "air", true).await;
    answer(
        &server,
        "GET",
        &format!("/api/orgs/name/{project}"),
        200,
        json!({ "id": 3 }),
    )
    .await;
    answer(
        &server,
        "GET",
        &format!("/api/dashboards/uid/{}", row.uid()),
        404,
        json!({}),
    )
    .await;
    answer(
        &server,
        "GET",
        "/api/datasources",
        200,
        json!([{ "type": "ngsild-grafana-datasource", "uid": "air", "url": "http://gw/api/endpoint/airslug" }]),
    )
    .await;
    answer(&server, "POST", "/api/dashboards/db", 200, json!({})).await;
    sweep(&state, &pool).await;
    assert!(paths(&server)
        .await
        .contains(&"POST /api/dashboards/db".to_owned()));
}

#[tokio::test]
async fn a_grafana_that_answers_500_stops_the_sweep_at_the_first_row() {
    let server = server().await;
    let (state, pool, project) = world(&server).await;
    keep(&state, &pool, &project, "air", true).await;
    keep(&state, &pool, &project, "air", true).await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    sweep(&state, &pool).await;
    assert_eq!(
        paths(&server).await,
        [format!("GET /api/orgs/name/{project}")],
        "the second row was never tried"
    );
}

#[tokio::test]
async fn a_project_the_registry_holds_no_key_for_is_skipped_and_the_next_is_swept() {
    let server = server().await;
    let state = AppState::new(config_for(&server), None);
    let unplaced = format!("p{}", unique());
    let state = common::unplaced(state, &unplaced).await;
    let pool = state.db.clone().expect("a database");
    let project = format!("p{}", unique());
    keyed_project(&state, &pool, &project, "cov-grafana").await;
    keep(&state, &pool, &project, "air", true).await;
    answer(
        &server,
        "GET",
        &format!("/api/orgs/name/{project}"),
        404,
        json!({}),
    )
    .await;
    sweep(&state, &pool).await;
    let seen = paths(&server).await;
    assert_eq!(seen, [format!("GET /api/orgs/name/{project}")]);
    assert!(
        grafana::list(
            &pool,
            joinedcontext_portal::project_ids::key_of(&pool, &state.mirror, &project)
                .await
                .expect("a key")
                .expect("placed"),
            &project
        )
        .await
        .expect("rows")
        .len()
            == 1
    );
}

#[tokio::test]
async fn a_database_that_does_not_answer_stops_the_sweep_before_it_asks_grafana() {
    let server = server().await;
    let state = AppState::new(config_for(&server), None);
    state.mirror.upsert(envelope(
        "Project",
        "doprava",
        ORG_NAMESPACE,
        json!({ "organizationRef": "somewhere" }),
    ));
    let dead = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool");
    sweep(&state, &dead).await;
    assert_eq!(paths(&server).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_visualization_is_found_listed_and_deleted_only_in_its_own_project() {
    let server = server().await;
    let (state, pool, project) = world(&server).await;
    let other = format!("p{}", unique());
    keyed_project(&state, &pool, &other, "cov-grafana-other").await;
    let row = keep(&state, &pool, &project, "air", true).await;
    let key_of = |name: String| {
        let (pool, mirror) = (pool.clone(), state.mirror.clone());
        async move {
            joinedcontext_portal::project_ids::key_of(&pool, &mirror, &name)
                .await
                .expect("a key")
                .expect("placed")
        }
    };
    let (mine, theirs) = (key_of(project.clone()).await, key_of(other.clone()).await);
    let found = grafana::find(&pool, mine, &project, &row.id)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(found.title, "Air");
    assert_eq!(found.kind, Kind::Stat);
    assert!(found.created_at.ends_with('Z'), "{}", found.created_at);
    assert_eq!(
        grafana::find(&pool, theirs, &other, &row.id)
            .await
            .expect("read"),
        None,
        "another project's key does not see it"
    );
    assert_eq!(
        grafana::find(&pool, mine, &project, "no-such-id")
            .await
            .expect("read"),
        None
    );
    assert!(grafana::list(&pool, theirs, &other)
        .await
        .expect("rows")
        .is_empty());
    // The test database connects as its owner, which row-level security does not bind, so a
    // delete under another key is judged by project_ids_tests' policy checks, not here.
    grafana::delete(&pool, mine, &project, &row.id)
        .await
        .expect("deleted");
    assert!(grafana::list(&pool, mine, &project)
        .await
        .expect("rows")
        .is_empty());
}
