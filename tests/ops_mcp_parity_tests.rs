//! The MCP parity operations through the MCP door, each with a caller it lets through and one it
//! refuses (T-6271, T-6380, T-6381, T-6389; JC2-28..34, 59, 60).
//!
//! Each operation calls its route's own function, so what is checked here is that the door asks
//! what the route asks: a stranger to the project meets the project that is not there, a caller
//! without the route's right meets its 403, and a caller with it reaches the route (the answer, or
//! the 503 of a service this test runs without).

mod common;

use axum::http::StatusCode;
use common::doors::{self, call, mcp, steward, stranger, viewer, PROJECT};
use common::envelope;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::{Bearer, Caller};
use joinedcontext_portal::state::AppState;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CATALOGUE: &str = r#"{
  "subjects": [{ "name": "dataModel.Parking", "models": [
    { "id": "dataModel.Parking/OnStreetParking", "name": "OnStreetParking",
      "description": "A street segment where vehicles park", "attributes": ["availableSpotNumber"] },
    { "id": "dataModel.Parking/ParkingSpot", "name": "ParkingSpot" }
  ] }],
  "stale": false
}"#;

const IMPORTED: &str = "id: https://smartdatamodels.org/dataModel.Parking\nname: parking\nclasses:\n  OnStreetParking:\n    slots: [id, type, availableSpotNumber]\n";

async fn tools() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CATALOGUE, "application/json"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/import-sdm"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "linkml": IMPORTED })))
        .mount(&server)
        .await;
    server
}

/// The project's objects the operations name, in a state whose model tools are `tools`.
fn world(tools: Option<&MockServer>) -> AppState {
    let config = Config {
        model_tools_url: tools.map(MockServer::uri),
        bootstrap_admins: "platform-admins".into(),
        ..Config::for_tests()
    };
    let state = doors::state_with(AppState::new(config, None));
    for (kind, name, spec) in [
        ("ContextSpace", "air", json!({})),
        (
            "Endpoint",
            "air-public",
            json!({ "slug": "mluyob4nz52lok3ssk7pgn5vwt", "contextSpaceRef": "air", "audience": "public" }),
        ),
        (
            "Pipeline",
            "counters",
            json!({ "contextSpaceRef": "air", "steps": [] }),
        ),
    ] {
        state.mirror.upsert(envelope(kind, name, PROJECT, spec));
    }
    state
}

fn admin() -> joinedcontext_portal::auth::session::Identity {
    doors::member("ana", "platform-admins")
}

/// `caller` with the bearer an MCP client presents.
fn with_bearer(mut caller: Caller) -> Caller {
    caller.bearer = Some(Bearer("person-token".into()));
    caller
}

fn assert_status(answer: &doors::Answer, wanted: StatusCode, what: &str) {
    assert_eq!(answer.status, wanted, "{what}: {}", answer.text());
}

#[tokio::test]
async fn a_smart_data_model_is_found_and_imported_and_a_stranger_reads_none() {
    let tools = tools().await;
    let state = world(Some(&tools));
    let found = call(
        "jc_model_sdm_search",
        &mcp(viewer()),
        &state,
        json!({ "q": "street" }),
    )
    .await;
    assert_status(&found, StatusCode::OK, "search");
    assert_eq!(
        found.body["models"][0]["id"],
        "dataModel.Parking/OnStreetParking"
    );
    assert_eq!(found.body["models"].as_array().map(Vec::len), Some(1));

    let imported = call(
        "jc_model_sdm_import",
        &mcp(steward()),
        &state,
        json!({ "model": "dataModel.Parking/OnStreetParking" }),
    )
    .await;
    assert_status(&imported, StatusCode::OK, "import");
    assert_eq!(imported.body["source"], IMPORTED);
    assert_eq!(
        imported.body["classes"],
        json!([{ "name": "OnStreetParking", "slots": ["id", "type", "availableSpotNumber"] }])
    );

    // A well-formed identifier the catalogue lacks is the caller's to fix, named by the field,
    // and never reaches the import.
    let unknown = call(
        "jc_model_sdm_import",
        &mcp(steward()),
        &state,
        json!({ "model": "dataModel.Parking/NoSuchModel" }),
    )
    .await;
    assert!(unknown.status.is_client_error(), "{}", unknown.text());
    assert!(
        unknown.text().contains(r#""path":"model""#),
        "{}",
        unknown.text()
    );
    let posts = tools
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/import-sdm")
        .count();
    assert_eq!(posts, 1, "only the listed model was imported");

    for (name, input) in [
        ("jc_model_sdm_search", json!({ "q": "street" })),
        (
            "jc_model_sdm_import",
            json!({ "model": "dataModel.Parking/OnStreetParking" }),
        ),
    ] {
        let refused = call(name, &mcp(stranger()), &state, input).await;
        assert_status(&refused, StatusCode::NOT_FOUND, name);
    }
}

#[tokio::test]
async fn the_callers_own_rules_are_read_and_a_stranger_has_none() {
    let state = world(None);
    let mine = call("jc_permissions_me", &mcp(viewer()), &state, json!({})).await;
    assert_status(&mine, StatusCode::OK, "viewer");
    assert_eq!(mine.body["project"], PROJECT);
    assert!(
        !mine.body["grants"].as_array().is_none_or(Vec::is_empty),
        "{}",
        mine.text()
    );
    let none = call("jc_permissions_me", &mcp(stranger()), &state, json!({})).await;
    assert_status(&none, StatusCode::NOT_FOUND, "stranger");
}

#[tokio::test]
async fn the_change_history_is_the_project_readers() {
    let state = world(None);
    // The reader passes the door and meets the forge this test runs without.
    let read = call(
        "jc_change_history",
        &mcp(viewer()),
        &state,
        json!({ "page": 1 }),
    )
    .await;
    assert_status(&read, StatusCode::SERVICE_UNAVAILABLE, "viewer");
    let refused = call("jc_change_history", &mcp(stranger()), &state, json!({})).await;
    assert_status(&refused, StatusCode::NOT_FOUND, "stranger");
    let page_zero = call(
        "jc_change_history",
        &mcp(viewer()),
        &state,
        json!({ "page": 0 }),
    )
    .await;
    assert!(page_zero.status.is_client_error(), "{}", page_zero.text());
}

#[tokio::test]
async fn questions_about_someone_else_are_an_organization_administrators() {
    let state = world(None);
    let subject = json!({ "kind": "group", "name": "city-viewers" });
    for (name, input) in [
        (
            "jc_access_simulate",
            json!({ "endpoint": "air-public", "subject": subject, "action": "retrieveEntity" }),
        ),
        ("jc_access_explore", json!({ "subject": subject })),
    ] {
        let refused = call(name, &mcp(steward()), &state, input.clone()).await;
        assert_status(&refused, StatusCode::FORBIDDEN, name);
        assert!(
            refused.text().contains("organization administrators"),
            "{}",
            refused.text()
        );
        let stranger = call(name, &mcp(stranger()), &state, input.clone()).await;
        assert_status(&stranger, StatusCode::NOT_FOUND, name);
        // The administrator passes the route's check and meets the gateway this test lacks.
        let asked = call(name, &mcp(admin()), &state, input).await;
        assert_ne!(
            asked.status,
            StatusCode::FORBIDDEN,
            "{name}: {}",
            asked.text()
        );
        assert_ne!(
            asked.status,
            StatusCode::NOT_FOUND,
            "{name}: {}",
            asked.text()
        );
    }
}

#[tokio::test]
async fn an_endpoints_reads_are_its_readers() {
    let state = world(None);
    for (name, input) in [
        (
            "jc_attribute_explain",
            json!({ "name": "air-public", "type": "AirQualityObserved", "attribute": "pm10" }),
        ),
        (
            "jc_endpoint_preview",
            json!({ "name": "air-public", "type": "AirQualityObserved", "limit": 5 }),
        ),
    ] {
        let read = call(name, &mcp(viewer()), &state, input.clone()).await;
        assert!(
            ![StatusCode::FORBIDDEN, StatusCode::NOT_FOUND].contains(&read.status),
            "{name}: the viewer reaches the route: {}",
            read.text()
        );
        let refused = call(name, &mcp(stranger()), &state, input).await;
        assert_status(&refused, StatusCode::NOT_FOUND, name);
    }
}

#[tokio::test]
async fn a_catalogue_draft_is_for_who_may_propose_the_endpoint() {
    let state = world(None);
    let input = json!({ "endpoint": "air-public" });
    let refused = call("jc_catalogue_draft", &mcp(viewer()), &state, input.clone()).await;
    assert_status(&refused, StatusCode::FORBIDDEN, "viewer");
    // The project has no CkanInstance to publish to: the steward is past the grant.
    let drafted = call("jc_catalogue_draft", &mcp(steward()), &state, input).await;
    assert_status(&drafted, StatusCode::CONFLICT, "steward");
}

#[tokio::test]
async fn a_pipelines_panels_and_a_spaces_readings_are_their_readers() {
    let state = world(None);
    let samples = call(
        "jc_pipeline_inspect",
        &mcp(viewer()),
        &state,
        json!({ "name": "counters", "view": "samples" }),
    )
    .await;
    assert_status(&samples, StatusCode::OK, "samples");
    let rejected = call(
        "jc_pipeline_inspect",
        &mcp(viewer()),
        &state,
        json!({ "name": "counters", "view": "rejected", "limit": 5 }),
    )
    .await;
    assert_status(&rejected, StatusCode::OK, "rejected");
    let missing = call(
        "jc_pipeline_inspect",
        &mcp(viewer()),
        &state,
        json!({ "name": "nope", "view": "stats" }),
    )
    .await;
    assert_status(&missing, StatusCode::NOT_FOUND, "an unknown pipeline");
    let refused = call(
        "jc_pipeline_inspect",
        &mcp(stranger()),
        &state,
        json!({ "name": "counters", "view": "samples" }),
    )
    .await;
    assert_status(&refused, StatusCode::NOT_FOUND, "stranger");

    let drift = call(
        "jc_space_governance",
        &mcp(viewer()),
        &state,
        json!({ "space": "air", "aspect": "drift" }),
    )
    .await;
    assert_status(&drift, StatusCode::OK, "drift");
    let refused = call(
        "jc_space_governance",
        &mcp(stranger()),
        &state,
        json!({ "space": "air", "aspect": "usage" }),
    )
    .await;
    assert_status(&refused, StatusCode::NOT_FOUND, "stranger");
}

#[tokio::test]
async fn an_mqtt_dry_run_is_its_brokers_readers() {
    let state = world(None);
    let input =
        json!({ "name": "sensors", "account": "cam-west", "topic": "cam/frame", "payload": "{}" });
    let refused = call(
        "jc_mqtt_test_publish",
        &mcp(stranger()),
        &state,
        input.clone(),
    )
    .await;
    assert_status(&refused, StatusCode::NOT_FOUND, "stranger");
    let unknown = call("jc_mqtt_test_publish", &mcp(steward()), &state, input).await;
    assert_status(
        &unknown,
        StatusCode::NOT_FOUND,
        "a broker the project lacks",
    );
}

#[tokio::test]
async fn an_entity_write_goes_as_the_person_through_their_own_endpoint() {
    let state = world(None);
    let station =
        json!({ "id": "urn:ngsi-ld:BikeHireDockingStation:s1", "type": "BikeHireDockingStation" });
    let input = json!({ "endpoint": "air-public", "action": "create", "entities": [station] });
    let refused = call(
        "jc_data_write",
        &with_bearer(mcp(stranger())),
        &state,
        input.clone(),
    )
    .await;
    // A write keeps its 403, naming the read on Endpoint it lacks (PF-50).
    assert_status(&refused, StatusCode::FORBIDDEN, "stranger");
    assert!(refused.text().contains("Endpoint"), "{}", refused.text());
    // Without the token an MCP client presents, nothing is exchanged and nothing written.
    let tokenless = call("jc_data_write", &mcp(steward()), &state, input.clone()).await;
    assert_status(&tokenless, StatusCode::FORBIDDEN, "no bearer");
    let other = call(
        "jc_data_write",
        &with_bearer(mcp(steward())),
        &state,
        json!({ "endpoint": "no-such-endpoint", "action": "delete", "ids": ["urn:ngsi-ld:T:1"] }),
    )
    .await;
    assert_status(
        &other,
        StatusCode::NOT_FOUND,
        "an Endpoint the project lacks",
    );
    // The steward passes the door and meets the gateway this test runs without.
    let written = call("jc_data_write", &with_bearer(mcp(steward())), &state, input).await;
    assert_status(&written, StatusCode::SERVICE_UNAVAILABLE, "steward");
    assert!(written.text().contains("gateway"), "{}", written.text());
}
