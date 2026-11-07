//! The catalogue and model operations through every door of the registry (T-1533; AG-64, PF-50,
//! PF-59, PF-60, DM-55, MF-24, EP-67).
//!
//! What was there before this file: the REST routes are tested in `ckan_api_tests.rs` (the
//! publication status), `resources_api_tests.rs` and `project_read_tests.rs` (endpoints
//! everywhere), `model_tools_proxy_tests.rs` and `edge_model_tools_tests.rs` (inference), and
//! `datamodel_source_tests.rs` (the source write). No test called `jc_ckan_status`,
//! `jc_endpoint_list_all`, `jc_model_infer` or `jc_model_source_put` through the registry, and the
//! source write had no secret check at all: a LinkML source carrying `apiKey: "…"` was committed to
//! the repository as typed (T-1533, fixed in `check_source`).

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::{envelope, forge, REPO};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SOURCE: &str = "id: https://example.org/models/air-quality\nname: air-quality\nclasses:\n  AirQualityObserved:\n    slots: [dateObserved, pm10]\nslots:\n  dateObserved:\n    range: string\n    required: true\n  pm10:\n    range: integer\n";

const COMPILED: &str = r##"{
  "jsonSchema": {"title": "AirQualityObserved"},
  "context": {"@context": {"pm10": "https://example.org/aq/pm10"}},
  "docs": "# AirQualityObserved",
  "example": {"id": "urn:ngsi-ld:AirQualityObserved:01", "type": "AirQualityObserved"},
  "generatorVersion": "linkml-1.11.1"
}"##;

struct World {
    forge: MockServer,
    tools: MockServer,
    state: AppState,
}

async fn world() -> World {
    let forge = forge().await;
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(COMPILED, "application/json"))
        .mount(&tools)
        .await;
    Mock::given(method("POST"))
        .and(path("/infer-schema"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "linkml": "id: https://example.org/models/sample\nclasses:\n  Sample: {}\n",
            "classes": ["Sample"]
        })))
        .mount(&tools)
        .await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let state = doors::state_with(AppState::new(config, None).with_gitea(Arc::new(client)));

    // The steward also reads the catalogue instances; the viewer does not.
    state.mirror.upsert(envelope(
        "Role",
        "catalogue-role",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["CkanInstance"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "catalogue-binding",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "city-stewards" }],
            "role": "catalogue-role",
            "scope": { "project": PROJECT },
        }),
    ));
    state.mirror.upsert(envelope(
        "CkanInstance",
        "open-data",
        PROJECT,
        json!({
            "url": "https://data.banskabystrica.sk",
            "organizationDefault": "mesto-banska-bystrica",
            "apiTokenRef": { "name": "ckan-open-data", "key": "apiToken" },
        }),
    ));
    for (name, project, slug) in [
        ("air-public", PROJECT, "mluyob4nz52lok3ssk7pgn5vwt"),
        ("buses", "doprava", "k7m2qz4tv6xh3n5jb2ryd3wcfa"),
    ] {
        state.mirror.upsert(envelope(
            "Endpoint",
            name,
            project,
            json!({ "slug": slug, "contextSpaceRef": "air", "audience": "public" }),
        ));
    }
    state.mirror.upsert(envelope(
        "DataModel",
        "air-quality",
        PROJECT,
        json!({
            "contextSpaceRef": "air",
            "linkml": "./air-quality.linkml.yaml",
            "version": "1.0.0",
            "lifecycle": "published",
            "classes": ["AirQualityObserved"]
        }),
    ));
    World {
        forge,
        tools,
        state,
    }
}

fn put(source: &str) -> Value {
    json!({ "name": "air-quality", "source": source })
}

async fn forge_writes(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() != "GET" && request.url.path().starts_with(REPO))
        .count()
}

/// `PUT …/datamodels/air-quality/source` with `text` as the YAML body, as the steward.
async fn put_raw(state: &AppState, text: &str) -> StatusCode {
    use axum::body::Body;
    use axum::http::{header, Request};
    use joinedcontext_portal::auth::csrf::CSRF_HEADER;
    use tower::ServiceExt;
    joinedcontext_portal::server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, common::cookie(&state.config, steward()))
                .header(CSRF_HEADER, common::CSRF)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(text.to_owned()))
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

async fn tool_calls(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

/// EP-67, PF-59: the catalogue status is read by whoever may read the project's catalogue
/// instances, names the token's reference and never a token; the viewer, who may not, finds no
/// catalogue, at the door and at the route alike.
#[tokio::test]
async fn the_catalogue_status_is_read_where_the_instances_may_be_read() {
    let world = world().await;
    for caller in [
        session(steward()),
        mcp(steward()),
        run(steward(), &["jc_ckan_status"]),
    ] {
        let status = doors::call("jc_ckan_status", &caller, &world.state, json!({})).await;
        assert_eq!(StatusCode::OK, status.status, "{}", status.text());
        assert_eq!(
            "open-data",
            status.body["instances"][0]["name"],
            "{}",
            status.text()
        );
        assert_eq!("ckan-open-data", status.body["instances"][0]["apiTokenRef"]);
    }
    for identity in [viewer(), stranger()] {
        let door = doors::call(
            "jc_ckan_status",
            &mcp(identity.clone()),
            &world.state,
            json!({}),
        )
        .await;
        assert_eq!(StatusCode::NOT_FOUND, door.status, "{}", door.text());
        let rest = doors::http(
            &world.state,
            identity.clone(),
            "GET",
            "/api/v1/projects/ovzdusie/ckan/status",
            None,
        )
        .await;
        let posted = doors::post_op(&world.state, identity, "jc_ckan_status", json!({})).await;
        assert_eq!(
            rest.status,
            posted.status,
            "{} vs {}",
            rest.text(),
            posted.text()
        );
    }
}

/// PF-61, PF-03, R20, T-2877: every endpoint of every project, for an administrator of the
/// organization at every door; a project's steward and viewer and a stranger are answered `404`,
/// the same answer the REST route gives.
#[tokio::test]
async fn endpoints_everywhere_answers_an_administrator_of_the_organization_only() {
    let world = world().await;
    world.state.mirror.upsert(envelope(
        "Role",
        "administrator",
        ORG_NAMESPACE,
        json!({ "rules": [
            { "kinds": ["RoleBinding"], "verbs": ["approve", "delete"] },
            { "kinds": ["Endpoint"], "verbs": ["read"] },
        ] }),
    ));
    world.state.mirror.upsert(envelope(
        "RoleBinding",
        "administrators",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "org-admins" }],
            "role": "administrator",
            "scope": { "organization": "banskabystrica" },
        }),
    ));
    let admin = || doors::member("ada", "org-admins");
    for caller in [
        session(admin()),
        mcp(admin()),
        run(admin(), &["jc_endpoint_list_all"]),
    ] {
        let listed = doors::call("jc_endpoint_list_all", &caller, &world.state, json!({})).await;
        assert_eq!(StatusCode::OK, listed.status, "{}", listed.text());
        assert!(listed.text().contains("air-public"), "{}", listed.text());
        assert!(listed.text().contains("buses"), "{}", listed.text());
    }
    for caller in [session(viewer()), mcp(steward()), session(stranger())] {
        let refused = doors::call("jc_endpoint_list_all", &caller, &world.state, json!({})).await;
        assert_eq!(StatusCode::NOT_FOUND, refused.status, "{}", refused.text());
        assert!(!refused.text().contains("air-public"), "{}", refused.text());
    }

    let rest = doors::http(&world.state, steward(), "GET", "/api/v1/endpoints", None).await;
    assert_eq!(StatusCode::NOT_FOUND, rest.status, "{}", rest.text());
}

/// DM-55: inference is a read of the project: a reader infers a draft model from a sample at every
/// door; a sample over the limit is refused naming it before the model tools are asked; a stranger
/// finds no project.
#[tokio::test]
async fn inference_answers_a_reader_and_refuses_an_oversized_sample() {
    let world = world().await;
    for caller in [
        session(viewer()),
        mcp(steward()),
        run(steward(), &["jc_model_infer"]),
    ] {
        let inferred = doors::call(
            "jc_model_infer",
            &caller,
            &world.state,
            json!({ "name": "sample", "sample": "station,pm10\nA,12\n", "format": "csv" }),
        )
        .await;
        assert_eq!(StatusCode::OK, inferred.status, "{}", inferred.text());
        assert!(inferred.text().contains("Sample"), "{}", inferred.text());
    }
    let asked = tool_calls(&world.tools).await;

    let oversized = "x".repeat(10 * 1024 * 1024 + 1);
    let refused = doors::call(
        "jc_model_infer",
        &session(steward()),
        &world.state,
        json!({ "sample": oversized }),
    )
    .await;
    // T-1659: the door enforces the declared bound before the operation runs, so an oversized
    // sample is an invalid input at `/sample` and never reaches Model Tools.
    assert_eq!(
        StatusCode::UNPROCESSABLE_ENTITY,
        refused.status,
        "{}",
        refused.text()
    );
    assert!(refused.text().contains("sample"), "{}", refused.text());
    assert_eq!(
        asked,
        tool_calls(&world.tools).await,
        "the oversized sample was sent on"
    );

    let hidden = doors::call(
        "jc_model_infer",
        &mcp(stranger()),
        &world.state,
        json!({ "sample": "a,b\n1,2\n" }),
    )
    .await;
    assert_eq!(StatusCode::NOT_FOUND, hidden.status, "{}", hidden.text());
    let empty = doors::call(
        "jc_model_infer",
        &session(steward()),
        &world.state,
        json!({}),
    )
    .await;
    assert_eq!(
        StatusCode::UNPROCESSABLE_ENTITY,
        empty.status,
        "{}",
        empty.text()
    );
}

/// DM-57, PF-50: the steward writes a model's source as one change at every door that may propose;
/// a viewer, a stranger and a run that does not name the operation are refused before the forge
/// is written.
#[tokio::test]
async fn the_source_is_written_as_a_change_by_whoever_may_propose() {
    let world = world().await;
    for caller in [
        session(steward()),
        mcp(steward()),
        run(steward(), &["jc_model_source_put"]),
    ] {
        let change = doors::call("jc_model_source_put", &caller, &world.state, put(SOURCE)).await;
        assert_eq!(StatusCode::OK, change.status, "{}", change.text());
        assert!(
            change.body["changeId"].as_str().is_some(),
            "{}",
            change.text()
        );
    }
    let written = forge_writes(&world.forge).await;
    for (who, caller) in [
        ("viewer", session(viewer())),
        ("viewer over mcp", mcp(viewer())),
        ("stranger", session(stranger())),
        ("run without it", run(steward(), &["jc_model_infer"])),
    ] {
        let refused = doors::call("jc_model_source_put", &caller, &world.state, put(SOURCE)).await;
        assert_eq!(
            StatusCode::FORBIDDEN,
            refused.status,
            "{who}: {}",
            refused.text()
        );
    }
    assert_eq!(
        written,
        forge_writes(&world.forge).await,
        "a refused write reached the forge"
    );
}

/// MF-24: a LinkML source with a secret typed into it is refused at every door and at the route,
/// naming the field and never echoing the value, and nothing reaches the forge. Started red: the
/// source went to the repository as typed.
#[tokio::test]
async fn a_source_with_a_secret_typed_in_is_refused_and_never_committed() {
    let world = world().await;
    let secret = format!("{SOURCE}annotations:\n  feed:\n    apiKey: sk-live-never-committed\n");
    for caller in [
        session(steward()),
        mcp(steward()),
        run(steward(), &["jc_model_source_put"]),
    ] {
        let refused = doors::call("jc_model_source_put", &caller, &world.state, put(&secret)).await;
        assert_eq!(
            StatusCode::BAD_REQUEST,
            refused.status,
            "{}",
            refused.text()
        );
        assert!(refused.text().contains("apiKey"), "{}", refused.text());
        assert!(refused.text().contains("secretRef"), "{}", refused.text());
        assert!(!refused.text().contains("sk-live"), "the secret was echoed");
    }
    let rest = put_raw(&world.state, &secret).await;
    assert_eq!(StatusCode::BAD_REQUEST, rest, "the route took the secret");
    assert_eq!(
        0,
        forge_writes(&world.forge).await,
        "the secret reached the forge"
    );
}

/// The dry run of a source checks it and proposes nothing, at the door.
#[tokio::test]
async fn a_dry_run_of_a_source_proposes_nothing() {
    let world = world().await;
    let mut input = put(SOURCE);
    input["dryRun"] = json!(true);
    let checked = doors::call(
        "jc_model_source_put",
        &session(steward()),
        &world.state,
        input,
    )
    .await;
    assert_eq!(StatusCode::OK, checked.status, "{}", checked.text());
    assert!(checked.body.get("changeId").is_none(), "{}", checked.text());
    assert_eq!(0, forge_writes(&world.forge).await);
}
