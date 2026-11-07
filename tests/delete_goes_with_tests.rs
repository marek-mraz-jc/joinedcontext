//! A removal says what leaves with it before anyone types the name (T-3247): the dry run of a
//! `DELETE` names a space's entities, a pipeline's refused records and runs, and nothing at all
//! for a resource that takes nothing with it.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, forge, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_log::{NewLine, Outcome};
use joinedcontext_portal::pipeline_outcomes::Reason;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";

/// The broker's admin token, mounted as the deployment mounts it (T-3983).
fn token_file() -> std::path::PathBuf {
    let file = std::env::temp_dir().join(format!("goes-with-token-{}", std::process::id()));
    std::fs::write(&file, "admin-token\n").expect("write the token file");
    file
}

/// The broker's count of one tenant (`GET /q/tenants/{tenant}`, T-3983).
fn tenant_count(tenant: &str, entities: u64) -> Mock {
    Mock::given(method("GET"))
        .and(path(format!("/q/tenants/{tenant}")))
        .and(header("authorization", "Bearer admin-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "tenant": tenant, "counts": { "entities": entities } })),
        )
}

/// `remover` reads and deletes spaces, pipelines and endpoints of `ovzdusie`, against a forge and
/// a broker whose `ovzdusie-air` tenant holds 1 234 entities.
async fn world(gitea: &MockServer, broker: &MockServer) -> AppState {
    let client = GiteaClient::new(
        gitea.uri().parse().expect("a url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("a client");
    let state = AppState::new(
        Config {
            broker_url: Some(broker.uri()),
            broker_admin_token_file: Some(token_file()),
            ..Config::for_tests()
        },
        None,
    )
    .with_gitea(std::sync::Arc::new(client));
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({ "isSandbox": false }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "air-ingest",
        PROJECT,
        json!({ "contextSpaceRef": { "name": "air" } }),
    ));
    state.mirror.upsert(envelope(
        "Endpoint",
        "public-air",
        PROJECT,
        json!({ "contextSpaceRef": { "name": "air" }, "slug": "public-air" }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "remover",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Pipeline", "Endpoint"], "verbs": ["read", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "removers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "remover@hel.fi" }], "role": "remover", "scope": { "project": PROJECT } }),
    ));
    tenant_count("ovzdusie-air", 1234).mount(broker).await;
    state
}

async fn dry_run(state: &AppState, plural: &str, name: &str) -> (StatusCode, Value) {
    let answer = send(
        state,
        person("remover"),
        "DELETE",
        &format!("/api/v1/projects/{PROJECT}/{plural}/{name}?dryRun=All"),
        None,
    )
    .await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_space_names_the_entities_it_holds() {
    let (gitea, broker) = (forge().await, MockServer::start().await);
    let state = world(&gitea, &broker).await;
    // `air` is still named by its pipeline and endpoint, which the dry run refuses as the removal
    // would (MF-07); `lonely` is a space nothing references.
    tenant_count("ovzdusie-lonely", 1234).mount(&broker).await;
    state.mirror.upsert(envelope(
        "ContextSpace",
        "lonely",
        PROJECT,
        json!({ "isSandbox": false }),
    ));

    let (status, body) = dry_run(&state, "spaces", "lonely").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["goesWith"],
        json!([{ "what": "entities", "count": 1234 }]),
        "{body}"
    );
}

#[tokio::test]
async fn a_pipeline_names_its_refused_records_and_its_runs() {
    let (gitea, broker) = (forge().await, MockServer::start().await);
    let state = world(&gitea, &broker).await;
    let reason = Reason {
        rule: "sh:datatype".into(),
        path: "pm10".into(),
        message: "not a number".into(),
        step: None,
        run: None,
    };
    for n in 0..3 {
        state
            .rejected
            .reject(PROJECT, "air-ingest", &json!({ "id": n }), &reason)
            .await
            .expect("kept");
    }
    for run in ["run-1", "run-2"] {
        state
            .pipeline_log
            .append(
                PROJECT,
                "air-ingest",
                run,
                &[NewLine {
                    record_id: "r".into(),
                    step: None,
                    outcome: Outcome::Sent,
                    message: String::new(),
                }],
            )
            .await
            .expect("logged");
    }

    let (status, body) = dry_run(&state, "pipelines", "air-ingest").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["goesWith"],
        json!([{ "what": "rejectedRecords", "count": 3 }, { "what": "runs", "count": 2 }]),
        "{body}"
    );
}

#[tokio::test]
async fn a_resource_that_takes_nothing_with_it_says_nothing_and_a_dry_run_writes_nothing() {
    let (gitea, broker) = (forge().await, MockServer::start().await);
    let state = world(&gitea, &broker).await;
    let (status, body) = dry_run(&state, "endpoints", "public-air").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("goesWith").is_none(), "{body}");
    let (status, body) = dry_run(&state, "pipelines", "air-ingest").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.get("goesWith").is_none(),
        "a pipeline that never ran takes nothing: {body}"
    );

    let writes: Vec<String> = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() != "GET")
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
}

/// A broker that does not answer leaves the count out instead of failing the dry run: what a
/// removal takes with it is information, never a reason to refuse it.
#[tokio::test]
async fn a_count_the_broker_cannot_give_is_left_out_and_the_dry_run_still_answers() {
    let (gitea, broker) = (forge().await, MockServer::start().await);
    let state = world(&gitea, &broker).await;
    state.mirror.upsert(envelope(
        "ContextSpace",
        "silent",
        PROJECT,
        json!({ "isSandbox": false }),
    ));
    Mock::given(method("GET"))
        .and(path("/q/tenants/ovzdusie-silent"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&broker)
        .await;
    let (status, body) = dry_run(&state, "spaces", "silent").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("goesWith").is_none(), "{body}");
}
