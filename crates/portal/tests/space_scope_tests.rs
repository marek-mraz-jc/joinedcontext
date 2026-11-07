//! T-5698: a grant bound to one context space reads that space's manifests alone, on every door
//! that hands one out. `may_read(kind)` is true for such a grant on any kind that lives in a space
//! (T-4593), so a door that asked only it handed out every space's Endpoints, models, drafts,
//! events and drift. Each case reads as `ana`, bound to space `air`, what space `water` holds.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::http::StatusCode;
use common::doors::{self, call, mcp, member, session, steward, PROJECT};
use joinedcontext_portal::activity::ActivityEvent;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::reconciler::drift::{Drifted, Found, Kind, Store};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

/// Reads the space-living kinds in space `air` only.
fn ana() -> Identity {
    member("ana", "air-readers")
}

/// Reads and proposes Endpoints in space `air` only.
fn air_steward() -> Identity {
    member("ivo", "air-stewards")
}

/// Proposes Pipelines in the whole project and reads no DataSource.
fn pipeline_writer() -> Identity {
    member("ola", "pipeline-writers")
}

const LINKML: &str = "id: https://example.org/model\nname: model\n";

fn world() -> AppState {
    let state = doors::state();
    for space in ["air", "water"] {
        state
            .mirror
            .upsert(common::envelope("ContextSpace", space, PROJECT, json!({})));
        state.mirror.upsert(common::envelope(
            "Endpoint",
            &format!("{space}-api"),
            PROJECT,
            json!({ "contextSpaceRef": space }),
        ));
        state.mirror.upsert(common::envelope(
            "DataModel",
            &format!("{space}-model"),
            PROJECT,
            json!({ "contextSpaceRef": space, "linkml": LINKML }),
        ));
    }
    state.mirror.upsert(common::envelope(
        "DataSource",
        "keyed-feed",
        PROJECT,
        json!({ "type": "http", "http": { "url": "https://example.org/feed", "authorization": { "secretRef": { "name": "feed", "key": "token" } } } }),
    ));
    for (role, kinds, verbs) in [
        (
            "space-reader",
            json!(["ContextSpace", "Endpoint", "DataModel", "Policy", "Project"]),
            json!(["read"]),
        ),
        (
            "endpoint-steward",
            json!(["ContextSpace", "Endpoint"]),
            json!(["read", "propose"]),
        ),
        ("pipeline-writer", json!(["Pipeline"]), json!(["propose"])),
    ] {
        state.mirror.upsert(common::envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
        ));
    }
    for (binding, group, role, scope) in [
        (
            "air-readers",
            "air-readers",
            "space-reader",
            json!({ "contextSpace": "air" }),
        ),
        (
            "air-stewards",
            "air-stewards",
            "endpoint-steward",
            json!({ "contextSpace": "air" }),
        ),
        (
            "pipeline-writers",
            "pipeline-writers",
            "pipeline-writer",
            json!({ "project": PROJECT }),
        ),
    ] {
        state.mirror.upsert(common::envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "group": group }], "role": role, "scope": scope }),
        ));
    }
    state
}

fn names(items: &Value, key: &str) -> Vec<String> {
    items
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.pointer(key).and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_rest_list_and_get_answer_the_bound_space_alone() {
    let state = world();
    let listed = doors::http(
        &state,
        ana(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(names(&listed.body["items"], "/metadata/name"), ["air-api"]);
    let spaces = doors::http(
        &state,
        ana(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/spaces"),
        None,
    )
    .await;
    assert_eq!(spaces.status, StatusCode::OK, "{}", spaces.text());
    assert_eq!(names(&spaces.body["items"], "/metadata/name"), ["air"]);
    for (name, status) in [
        ("air-api", StatusCode::OK),
        ("water-api", StatusCode::NOT_FOUND),
    ] {
        let read = doors::http(
            &state,
            ana(),
            "GET",
            &format!("/api/v1/projects/{PROJECT}/endpoints/{name}"),
            None,
        )
        .await;
        assert_eq!(read.status, status, "{name}: {}", read.text());
    }
    // A project-wide reader still reads both.
    let all = doors::http(
        &state,
        doors::viewer(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        None,
    )
    .await;
    assert_eq!(
        names(&all.body["items"], "/metadata/name"),
        ["air-api", "water-api"]
    );
}

#[tokio::test]
async fn the_resource_operations_list_and_read_the_bound_space_alone() {
    let state = world();
    let listed = call(
        "jc_resource_list",
        &mcp(ana()),
        &state,
        json!({ "kind": "Endpoint" }),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(names(&listed.body["items"], "/name"), ["air-api"]);

    let read = call(
        "jc_resource_get",
        &mcp(ana()),
        &state,
        json!({ "kind": "Endpoint", "name": "water-api" }),
    )
    .await;
    assert_eq!(read.status, StatusCode::NOT_FOUND, "{}", read.text());
    // The names a miss suggests are the readable ones.
    let missed = call(
        "jc_resource_get",
        &mcp(ana()),
        &state,
        json!({ "kind": "Endpoint", "name": "nope" }),
    )
    .await;
    assert_eq!(missed.status, StatusCode::NOT_FOUND);
    assert!(missed.text().contains("air-api"), "{}", missed.text());
    assert!(!missed.text().contains("water"), "{}", missed.text());
}

#[tokio::test]
async fn a_models_source_of_another_space_is_not_there() {
    let state = world();
    let water = call(
        "jc_model_source_get",
        &session(ana()),
        &state,
        json!({ "name": "water-model" }),
    )
    .await;
    assert_eq!(water.status, StatusCode::NOT_FOUND, "{}", water.text());
    let air = call(
        "jc_model_source_get",
        &session(ana()),
        &state,
        json!({ "name": "air-model" }),
    )
    .await;
    assert_eq!(air.status, StatusCode::OK, "{}", air.text());
    assert_eq!(air.body["source"], json!(LINKML));
}

#[tokio::test]
async fn the_catalog_and_the_federation_show_the_bound_space_alone() {
    let state = world();
    let found = call(
        "jc_catalog_search",
        &session(ana()),
        &state,
        json!({ "q": "water" }),
    )
    .await;
    assert_eq!(found.status, StatusCode::OK, "{}", found.text());
    assert!(!found.text().contains("water-"), "{}", found.text());
    let air = call(
        "jc_catalog_search",
        &session(ana()),
        &state,
        json!({ "q": "air" }),
    )
    .await;
    assert!(air.text().contains("air-api"), "{}", air.text());

    let graph = call("jc_federation_graph", &session(ana()), &state, json!({})).await;
    assert_eq!(graph.status, StatusCode::OK, "{}", graph.text());
    assert!(
        graph.text().contains("Endpoint/air-api"),
        "{}",
        graph.text()
    );
    assert!(!graph.text().contains("water"), "{}", graph.text());
}

fn event(space: &str) -> ActivityEvent {
    ActivityEvent {
        time: chrono::Utc::now(),
        project: PROJECT.to_owned(),
        space: Some(space.to_owned()),
        kind: "config.drifted".to_owned(),
        source: "portal".to_owned(),
        summary: format!("a change in {space}"),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({}),
    }
}

#[tokio::test]
async fn the_activity_operation_shows_the_bound_spaces_events_alone() {
    let state = world();
    state
        .activity
        .append(&[event("air"), event("water")])
        .await
        .expect("appended");
    let listed = call("jc_activity_list", &mcp(ana()), &state, json!({})).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(names(&listed.body["items"], "/space"), ["air"]);
}

fn drifted(space: &str) -> Drifted {
    Drifted {
        space: space.to_owned(),
        id: format!("urn:ngsi-ld:Station:{space}"),
        drift: Kind::Modified,
        diff: Vec::new(),
        resolutions: vec!["revert", "adopt"],
        source: format!("projects/{PROJECT}/spaces/{space}/entities/seed/stations.json"),
        declared: json!({ "id": format!("urn:ngsi-ld:Station:{space}") }),
    }
}

#[tokio::test]
async fn the_drift_list_shows_the_bound_spaces_entities_alone() {
    let mut state = world();
    let store = Arc::new(Store::default());
    store.replace_all(BTreeMap::from([(
        PROJECT.to_owned(),
        Found {
            observed_at: chrono::Utc::now(),
            entities: vec![drifted("air"), drifted("water")],
        },
    )]));
    state.drift = store;
    let listed = doors::http(
        &state,
        ana(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/drift"),
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(names(&listed.body["items"], "/space"), ["air"]);
    let all = doors::http(
        &state,
        doors::viewer(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/drift"),
        None,
    )
    .await;
    assert_eq!(names(&all.body["items"], "/space"), ["air", "water"]);
}

#[tokio::test]
async fn a_draft_of_another_space_is_not_run_for_a_steward_of_one() {
    let state = world();
    let put = call(
        "jc_draft_put",
        &session(steward()),
        &state,
        json!({ "kind": "Endpoint", "name": "water-new", "manifest": {
            "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Endpoint",
            "metadata": { "name": "water-new", "namespace": PROJECT },
            "spec": { "contextSpaceRef": "water" } } }),
    )
    .await;
    assert_eq!(put.status, StatusCode::OK, "{}", put.text());
    let run = call(
        "jc_manifest_dry_run",
        &session(air_steward()),
        &state,
        json!({ "draft": { "kind": "Endpoint", "name": "water-new" } }),
    )
    .await;
    assert_eq!(run.status, StatusCode::NOT_FOUND, "{}", run.text());
    assert!(!run.text().contains("contextSpaceRef"), "{}", run.text());
}

#[tokio::test]
async fn a_source_the_caller_may_not_read_is_not_sampled() {
    let state = world();
    let sampled = call(
        "jc_pipeline_sample_source",
        &session(pipeline_writer()),
        &state,
        json!({ "dataSource": "keyed-feed" }),
    )
    .await;
    assert_eq!(sampled.status, StatusCode::NOT_FOUND, "{}", sampled.text());
    assert!(!sampled.text().contains("credential"), "{}", sampled.text());
}

async fn resources(state: &AppState, who: Identity, method: &str, params: Value) -> Value {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let response = joinedcontext_portal::mcp::dispatch_for(
        state.clone(),
        mcp(who),
        Bytes::from(body.to_string()),
    )
    .await;
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json-rpc")
}

#[tokio::test]
async fn the_mcp_resources_list_and_read_the_bound_space_alone() {
    let state = world();
    let listed = resources(
        &state,
        ana(),
        "resources/list",
        json!({ "project": PROJECT }),
    )
    .await;
    let uris = names(&listed["result"]["resources"], "/uri");
    assert!(
        uris.contains(&format!("jc://{PROJECT}/endpoints/air-api")),
        "{uris:?}"
    );
    assert!(!uris.iter().any(|uri| uri.contains("water")), "{uris:?}");
    for uri in [
        format!("jc://{PROJECT}/endpoints/water-api"),
        format!("jc://{PROJECT}/datamodels/water-model/linkml"),
    ] {
        let read = resources(&state, ana(), "resources/read", json!({ "uri": uri })).await;
        assert_eq!(read["error"]["code"], -32002, "{uri}: {read}");
    }
    let read = resources(
        &state,
        ana(),
        "resources/read",
        json!({ "uri": format!("jc://{PROJECT}/endpoints/air-api") }),
    )
    .await;
    assert!(read["result"]["contents"][0]["text"].is_string(), "{read}");
}

/// Adopting drift proposes the live entity as the person reads it, never as the Portal's own
/// account does: the gateway is asked with the person's token, and a session that carries none is
/// answered 404 (T-5698).
#[tokio::test]
async fn adopting_drift_reads_the_live_entity_with_the_persons_own_token() {
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let urn = "urn:ngsi-ld:Station:air";
    let gateway = MockServer::start().await;
    // What the person's Policy shows: no `secret` attribute, which the space holds.
    Mock::given(method("GET"))
        .and(path(format!("/cs/air/ngsi-ld/v1/entities/{urn}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": urn, "type": "Station" })),
        )
        .mount(&gateway)
        .await;
    let gateway_url = gateway.uri();
    let config = joinedcontext_portal::config::Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_PORTAL_GATEWAY_URL" => Some(gateway_url.as_str()),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let mut state = doors::state_with(AppState::new(config, None));
    state.mirror.upsert(common::envelope(
        "Role",
        "entity-proposer",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Entity", "Project"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "entity-proposers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "entity-proposer",
                "scope": { "project": PROJECT } }),
    ));
    let store = Arc::new(Store::default());
    store.replace_all(BTreeMap::from([(
        PROJECT.to_owned(),
        Found {
            observed_at: chrono::Utc::now(),
            entities: vec![drifted("air")],
        },
    )]));
    state.drift = store;
    let uri = format!("/api/v1/projects/{PROJECT}/drift/air/{urn}/adopt");

    let token = common::REALM.person_token_of("portal-api", "portal-api", "jana", &[]);
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    // No forge here: what matters is how the entity was read, not the change it opens.
    assert_ne!(response.status(), StatusCode::NOT_FOUND);
    let asked = gateway.received_requests().await.unwrap_or_default();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some(format!("Bearer {token}").as_str()),
        "read with the person's own token"
    );

    // The Portal's cookie session carries no access token of the person: nothing is read.
    let mut by_cookie = member("jana@hel.fi", "nobody");
    by_cookie.username = "jana@hel.fi".to_owned();
    let refused = doors::http(&state, by_cookie, "POST", &uri, None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text());
    assert_eq!(
        gateway.received_requests().await.unwrap_or_default().len(),
        1
    );
}

/// T-6273: a Role lives in the project or the organization, and `jc_resource_list` judges each
/// home with the grants the caller holds there, as `jc_resource_get` does: a person who reads
/// Roles in the project alone lists no organization Role, and every Role listed opens.
#[tokio::test]
async fn the_role_list_judges_each_home_with_its_own_grants() {
    let state = world();
    state.mirror.upsert(common::envelope(
        "Role",
        "role-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Role"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "Role",
        "local-role",
        PROJECT,
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
    ));
    for (binding, scope) in [
        ("project-role-readers", json!({ "project": PROJECT })),
        ("org-role-readers", json!({ "organization": "city" })),
    ] {
        state.mirror.upsert(common::envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "group": binding }], "role": "role-reader", "scope": scope }),
        ));
    }
    let org_roles: Vec<String> = state
        .mirror
        .list(ORG_NAMESPACE, "Role", &Default::default())
        .items
        .into_iter()
        .map(|envelope| envelope.metadata.name)
        .collect();
    assert!(
        org_roles.contains(&"viewer-role".to_owned()),
        "{org_roles:?}"
    );

    let in_project = member("pia", "project-role-readers");
    let listed = call(
        "jc_resource_list",
        &mcp(in_project.clone()),
        &state,
        json!({ "kind": "Role" }),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(names(&listed.body["items"], "/name"), ["local-role"]);
    for (name, status) in [
        ("local-role", StatusCode::OK),
        ("viewer-role", StatusCode::NOT_FOUND),
    ] {
        let read = call(
            "jc_resource_get",
            &mcp(in_project.clone()),
            &state,
            json!({ "kind": "Role", "name": name }),
        )
        .await;
        assert_eq!(read.status, status, "{name}: {}", read.text());
    }
    // A miss names only the Roles the caller may open.
    let missed = call(
        "jc_resource_get",
        &mcp(in_project),
        &state,
        json!({ "kind": "Role", "name": "nope" }),
    )
    .await;
    assert_eq!(missed.status, StatusCode::NOT_FOUND);
    assert!(missed.text().contains("local-role"), "{}", missed.text());
    assert!(!missed.text().contains("viewer-role"), "{}", missed.text());

    // A reader of Roles in the organization lists both homes, and opens every Role listed.
    let in_org = member("oto", "org-role-readers");
    let listed = call(
        "jc_resource_list",
        &mcp(in_org.clone()),
        &state,
        json!({ "kind": "Role" }),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let listed = names(&listed.body["items"], "/name");
    assert!(listed.contains(&"local-role".to_owned()), "{listed:?}");
    for name in &org_roles {
        assert!(listed.contains(name), "{name} missing from {listed:?}");
    }
    for name in &listed {
        let read = call(
            "jc_resource_get",
            &mcp(in_org.clone()),
            &state,
            json!({ "kind": "Role", "name": name }),
        )
        .await;
        assert_eq!(read.status, StatusCode::OK, "{name}: {}", read.text());
    }
}
