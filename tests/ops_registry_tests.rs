//! Tests for the Portal operation registry and its REST adapter (T-0636, AG-59, CC-48, PF-50).
//!
//! Asserts that:
//! 1. Every registered operation answers on `POST /api/v1/projects/{project}/ops/{name}`.
//! 2. `GET /api/v1/projects/{project}/ops` lists only operations permitted for the caller.
//! 3. An unknown field in input yields 422 Unprocessable Entity with error details.
//! 4. A principal without a binding reads a project that is not there (404, PF-59, R20).
//! 5. `jc_catalog_search` produces identical search results to the assistant catalog route.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-ops-123";

fn session_cookie(
    config: &Config,
    username: &str,
    email: Option<&str>,
    roles: Vec<&str>,
    groups: Vec<&str>,
) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: format!("sub-{username}"),
            username: username.to_string(),
            email: email.map(str::to_string),
            name: Some(username.to_string()),
            roles: roles.into_iter().map(str::to_string).collect(),
            groups: groups.into_iter().map(str::to_string).collect(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        let pair = raw.split(';').next().unwrap_or_default();
        parts.push(pair.to_string());
    }
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

#[tokio::test]
async fn every_registered_name_answers_on_post_ops() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    // The project the calls are addressed to exists (T-0922): an operation that reads or deletes
    // one answers `404` for a project that is not there, which is R20 and not a missing route,
    // and this test is about the route being there at all.
    state
        .mirror
        .upsert(joinedcontext_portal::resource::ResourceEnvelope {
            api_version: joinedcontext_portal::resource::API_VERSION.into(),
            kind: "Project".into(),
            metadata: joinedcontext_portal::resource::ObjectMeta::new(
                "ovzdusie",
                joinedcontext_portal::permissions::ORG_NAMESPACE,
            ),
            spec: serde_json::json!({ "organizationRef": { "name": "bb" } }),
            status: None,
        });
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    for op in ops::registry() {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/projects/ovzdusie/ops/{}", op.name))
            .header(header::COOKIE, &steward_cookie)
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b"{}".to_vec()))
            .expect("request");

        let resp = app.clone().oneshot(req).await.expect("response");
        let status = resp.status();

        // An operation may succeed (200/202) or reject input/conditions (400/403/409/422/503),
        // but must NEVER be 404 (unregistered route) or 500 (unhandled panic).
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "operation '{}' was not found on POST /ops/{{name}}",
            op.name
        );
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "operation '{}' failed with 500 Internal Server Error",
            op.name
        );
    }
}

#[tokio::test]
async fn get_ops_differs_between_steward_and_viewer() {
    let config = Config::for_tests();
    let mirror = std::sync::Arc::new(Mirror::new());

    // Role that only allows reading/viewing, no propose/approve/delete verbs
    let viewer_role = ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Role".to_string(),
        metadata: ObjectMeta::new("read-only-role", "org"),
        spec: json!({
            "rules": [
                { "kinds": ["Endpoint", "ContextSpace"], "verbs": [] }
            ]
        }),
        status: None,
    };
    let viewer_binding = ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "RoleBinding".to_string(),
        metadata: ObjectMeta::new("viewer-binding", "org"),
        spec: json!({
            "subjects": [{ "group": "city-viewers" }],
            "role": "read-only-role",
            "scope": { "project": "ovzdusie" }
        }),
        status: None,
    };
    mirror.upsert(viewer_role);
    mirror.upsert(viewer_binding);

    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let app = server::app(state);

    // 1. Steward has bootstrap group
    let steward_cookie = session_cookie(
        &config,
        "demo.steward",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );
    let resp_steward = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/ops")
                .header(header::COOKIE, &steward_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_steward.status(), StatusCode::OK);
    let bytes_steward = resp_steward.into_body().collect().await.unwrap().to_bytes();
    let steward_ops: Vec<Value> = serde_json::from_slice(&bytes_steward).unwrap();
    let steward_names: Vec<&str> = steward_ops
        .iter()
        .filter_map(|o| o["name"].as_str())
        .collect();

    assert!(steward_names.contains(&"jc_resource_propose"));
    assert!(steward_names.contains(&"jc_catalog_search"));
    assert!(steward_names.contains(&"jc_change_approve"));

    // 2. Viewer has only read-only grant
    let viewer_cookie = session_cookie(
        &config,
        "demo.viewer",
        Some("viewer@banskabystrica.sk"),
        vec![],
        vec!["city-viewers"],
    );
    let resp_viewer = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/ops")
                .header(header::COOKIE, &viewer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_viewer.status(), StatusCode::OK);
    let bytes_viewer = resp_viewer.into_body().collect().await.unwrap().to_bytes();
    let viewer_ops: Vec<Value> = serde_json::from_slice(&bytes_viewer).unwrap();
    let viewer_names: Vec<&str> = viewer_ops
        .iter()
        .filter_map(|o| o["name"].as_str())
        .collect();

    assert!(
        viewer_names.contains(&"jc_catalog_search"),
        "viewer can run read-only operations"
    );
    assert!(
        !viewer_names.contains(&"jc_resource_propose"),
        "viewer must not see mutating propose operations"
    );
    assert!(
        !viewer_names.contains(&"jc_change_approve"),
        "viewer must not see approve operations"
    );
}

#[tokio::test]
async fn unknown_field_in_input_returns_422() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "q": "bikes",
        "extraneousField": "should-trigger-422"
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_catalog_search")
                .header(header::COOKIE, steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let error_body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        error_body["type"], "https://joinedcontext.com/errors/unprocessable",
        "{error_body}"
    );
    assert_eq!(
        error_body["errors"][0]["path"], "extraneousField",
        "{error_body}"
    );
    assert!(
        error_body["errors"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("extraneousField")),
        "{error_body}"
    );
}

/// A read in a project the caller holds no binding in is the answer of a project that is not
/// there (PF-59, R20); the 403 of PF-50 is for a write, which says what is missing.
#[tokio::test]
async fn principal_without_binding_reads_a_project_that_is_not_there() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let unprivileged_cookie = session_cookie(
        &config,
        "unprivileged.user",
        Some("unprivileged@example.com"),
        vec![],
        vec![],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_catalog_search")
                .header(header::COOKIE, unprivileged_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b"{\"q\":\"bikes\"}".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn jc_catalog_search_finds_the_public_endpoint() {
    let config = Config::for_tests();
    let mirror = std::sync::Arc::new(Mirror::new());

    // Populate mirror with a context space and an endpoint
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta::new("mobility", "ovzdusie"),
        spec: json!({ "isSandbox": false }),
        status: None,
    });
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Endpoint".to_string(),
        metadata: ObjectMeta::new("public-air", "ovzdusie"),
        spec: json!({
            "contextSpaceRef": "mobility",
            "slug": "publicair00000000000000000000",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }),
        status: None,
    });

    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let resp_ops = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_catalog_search")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b"{\"q\":\"air\"}".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_ops.status(), StatusCode::OK);
    let bytes_ops = resp_ops.into_body().collect().await.unwrap().to_bytes();
    let ops_json: Value = serde_json::from_slice(&bytes_ops).unwrap();

    let ops_items = ops_json["items"].as_array().expect("items array");
    assert_eq!(ops_items[0]["name"], "public-air");
}

/// AG-60, CC-47, T-0838: what a tool publishes as its output is what the tool answers.
///
/// A schema pointing at `#/components/schemas/…` describes nothing to an MCP client: the
/// pointer has no document to resolve against, so every published schema is self-contained.
#[test]
fn no_published_schema_points_at_a_document_the_caller_never_has() {
    fn refs(value: &Value, path: &str, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    if key == "$ref" {
                        found.push(format!("{path}: {inner}"));
                    }
                    refs(inner, &format!("{path}/{key}"), found);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    refs(item, &format!("{path}/{index}"), found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for op in ops::registry() {
        refs(&(op.input)(), &format!("{}.input", op.name), &mut found);
        refs(&(op.output)(), &format!("{}.output", op.name), &mut found);
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The propose and delete operations answer the wrapper, not the bare `Change`: what the
/// schema names as required is what the answer carries (T-0838).
#[test]
fn the_change_operations_publish_the_wrapper_they_answer() {
    for name in [
        "jc_resource_propose",
        "jc_resource_delete",
        "jc_change_reject",
    ] {
        let op = ops::find(name).expect("registered");
        let schema = (op.output)();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            required,
            vec!["changeId", "lane", "change"],
            "{name} answers ProposeOutcome::Change"
        );
        assert!(schema["properties"]["change"]["properties"]["status"].is_object());
    }

    // `jc_endpoint_propose` renders a proposal from parameters and answers a Change from a
    // manifest or a draft: both shapes are published, neither is hidden.
    let endpoint = (ops::find("jc_endpoint_propose").expect("registered").output)();
    let shapes = endpoint["oneOf"].as_array().expect("both shapes");
    assert_eq!(shapes[0]["required"], json!(["changeId", "lane", "change"]));
    assert!(shapes[1]["properties"]["endpoint"].is_object());

    // The approval answers the `Change` itself, and the list its envelope.
    let approve = (ops::find("jc_change_approve").expect("registered").output)();
    assert_eq!(approve["properties"]["kind"]["enum"], json!(["Change"]));
    let list = (ops::find("jc_change_list").expect("registered").output)();
    assert_eq!(list["properties"]["kind"]["enum"], json!(["ChangeList"]));
    assert_eq!(list["properties"]["items"]["type"], json!("array"));
}

/// AG-07, AG-63, T-0917: an agent runtime asks a person before a write only when the operation
/// says a write is possible. So no operation that can answer a `changeId` — the id of the merge
/// request it just opened — may be annotated `readOnlyHint`, whichever of its paths writes.
#[test]
fn nothing_that_can_open_a_change_is_annotated_read_only() {
    fn answers_a_change(value: &Value) -> bool {
        match value {
            Value::Object(map) => {
                map.get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|props| props.contains_key("changeId"))
                    || map.values().any(answers_a_change)
            }
            Value::Array(items) => items.iter().any(answers_a_change),
            _ => false,
        }
    }

    let mut checked = 0;
    for op in ops::registry() {
        if !answers_a_change(&(op.output)()) {
            continue;
        }
        checked += 1;
        assert!(
            !op.annotations.read_only_hint,
            "{} can answer a changeId and is annotated readOnlyHint: true",
            op.name
        );
    }
    assert!(
        checked >= 7,
        "expected the propose operations, checked {checked}"
    );
}

/// AG-59, CC-48, T-0840: what the Portal serves, the registry serves. These four reads had a
/// route and no operation, so an MCP client could not see a pipeline's counters, what happened
/// in the project, the federation, or a model's LinkML.
#[tokio::test]
async fn the_reads_that_had_only_a_route_answer_through_the_registry() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta::new("ovzdusie", "ovzdusie"),
        spec: json!({ "isSandbox": false }),
        status: None,
    });
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta::new("air", "ovzdusie"),
        spec: json!({
            "version": "1.0.0",
            "lifecycle": "draft",
            "contextSpaceRef": "ovzdusie",
            "linkml": "id: https://example.org/air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: [pm10]\n",
            "classes": ["AirQualityObserved"]
        }),
        status: None,
    });
    let state = AppState::new(config, None).with_mirror(mirror);
    let caller = ops::Caller {
        bearer: None,
        identity: joinedcontext_portal::auth::session::Identity {
            client: None,
            subject: "sub-steward".into(),
            username: "steward".into(),
            email: Some("steward@banskabystrica.sk".into()),
            name: None,
            roles: vec!["portal-approver".into()],
            groups: vec!["platform-admins".into()],
            second_factor: false,
        },
        via: ops::Via::Mcp,
        access: None,
    };

    let graph = ops::call(
        ops::find("jc_federation_graph").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({}),
    )
    .await
    .expect("the federation");
    assert!(graph["nodes"].is_array(), "{graph}");

    let activity = ops::call(
        ops::find("jc_activity_list").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({ "severity": "warning", "limit": 10 }),
    )
    .await
    .expect("what happened");
    assert!(activity["items"].is_array(), "{activity}");

    let source = ops::call(
        ops::find("jc_model_source_get").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({ "name": "air" }),
    )
    .await
    .expect("the model's LinkML");
    assert!(
        source["source"]
            .as_str()
            .unwrap_or_default()
            .contains("AirQualityObserved"),
        "{source}"
    );

    // A pipeline that is not there is not disclosed as existing elsewhere (R20).
    let missing = ops::call(
        ops::find("jc_pipeline_metrics").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({ "name": "no-such-pipeline" }),
    )
    .await
    .expect_err("no such pipeline");
    assert!(format!("{missing:?}").contains("not found"), "{missing:?}");
}

/// AG-59, AG-11, T-0840: a run is started, cancelled and published through the registry, and an
/// agent run never starts another.
#[tokio::test]
async fn the_run_operations_answer_and_an_agent_is_refused_by_name() {
    let config = Config::for_tests();
    let state = AppState::new(config, None).with_mirror(Arc::new(Mirror::new()));
    let steward = ops::Caller {
        bearer: None,
        identity: joinedcontext_portal::auth::session::Identity {
            client: None,
            subject: "sub-steward".into(),
            username: "steward".into(),
            email: Some("steward@banskabystrica.sk".into()),
            name: None,
            roles: vec!["portal-approver".into()],
            groups: vec!["platform-admins".into()],
            second_factor: false,
        },
        via: ops::Via::Mcp,
        access: None,
    };
    let agent = ops::Caller {
        bearer: None,
        via: ops::Via::Agent,
        ..steward.clone()
    };

    for name in ["jc_run_create", "jc_run_cancel", "jc_run_publish"] {
        let op = ops::find(name).expect("registered");
        let input = match name {
            "jc_run_create" => json!({
                "appName": "bikes", "prompt": "show the bikes", "dataNeeds": [],
                "endpointName": "public-air"
            }),
            _ => json!({ "id": "run-1" }),
        };
        let refused = ops::call(op, &agent, &state, "ovzdusie", input.clone())
            .await
            .expect_err("an agent never starts, cancels or publishes a run");
        assert!(
            format!("{refused:?}").contains("a person does"),
            "{name}: {refused:?}"
        );

        // The person's own call reaches the route's own checks rather than this gate.
        let answered = ops::call(op, &steward, &state, "ovzdusie", input).await;
        let said = format!("{answered:?}");
        assert!(!said.contains("a person does"), "{name}: {said}");
    }
}

mod common;

/// PF-65, AG-59, T-0870: the operation is the same door as the "New project" button. Who may
/// open a project is the organization's own setting, so the registry lets the call through and
/// the route refuses it — under `org-admin` a person without `propose` on `Project` is turned
/// away, under `anyone` the same person opens one.
#[tokio::test]
async fn jc_project_create_follows_the_organizations_own_setting() {
    use wiremock::matchers::{method as http_method, path as url_path, query_param};
    use wiremock::{Mock, ResponseTemplate};

    let gitea = common::forge().await;
    Mock::given(http_method("GET"))
        .and(url_path(format!("{}/pulls", common::REPO)))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    let organization = |creation: &str| {
        state.mirror.upsert(common::envelope(
            "Organization",
            "bb",
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({ "domain": "banskabystrica.sk", "projects": { "creation": creation } }),
        ));
    };
    let caller = ops::Caller {
        bearer: None,
        identity: common::person("nobody"),
        via: ops::Via::Mcp,
        access: None,
    };
    let op = ops::find("jc_project_create").expect("registered");

    organization("org-admin");
    let refused = ops::call(
        op,
        &caller,
        &state,
        "banskabystrica",
        json!({ "name": "doprava" }),
    )
    .await
    .expect_err("a plain person does not open a project under org-admin");
    assert!(
        format!("{refused:?}").contains("propose on Project"),
        "{refused:?}"
    );

    organization("anyone");
    let opened = ops::call(
        op,
        &caller,
        &state,
        "banskabystrica",
        json!({ "name": "doprava", "displayName": "Doprava" }),
    )
    .await
    .expect("anyone opens a project");
    // The same wrapper every proposing operation answers: one Change a person approves (CC-19).
    assert_eq!(opened["lane"], "yellow", "{opened}");
    assert!(opened["changeId"].is_string(), "{opened}");
}

/// PF-65, UI-44, T-0870: `permissions/me` says whether this caller may open a project and, when
/// they may not, why — so the control is rendered disabled with the reason, never hidden.
#[tokio::test]
async fn permissions_me_says_whether_a_project_may_be_opened_and_why_not() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(common::envelope(
        "ContextSpace",
        "ovzdusie",
        "banskabystrica",
        json!({ "isSandbox": false }),
    ));
    state.mirror.upsert(common::envelope(
        "Organization",
        "bb",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "domain": "banskabystrica.sk" }),
    ));

    // The bootstrap group reads every project, and under the default `org-admin` it also holds
    // `propose` on `Project`.
    let admin = common::person("boss");
    let mut admin = admin;
    admin.groups = vec!["portal-approver".into()];
    let answer = common::send(
        &state,
        admin,
        "GET",
        "/api/v1/projects/banskabystrica/permissions/me",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let effective: Value = serde_json::from_str(&answer.text).expect("permissions");
    assert_eq!(
        effective["projects"]["creation"]["allowed"], true,
        "{effective}"
    );
    assert!(
        effective["projects"]["creation"]["reason"].is_null(),
        "{effective}"
    );
}

/// AG-11 and the owner's decision of 2026-09-17 (T-1005): deciding a change is a person's, and
/// an MCP client is refused with an agent run. The client is a tool a model drives even when it
/// carries the person's token, so leaving `Via::Mcp` open would let an agent approve by choosing
/// another transport. A session caller passes this gate and meets the route's own checks.
#[tokio::test]
async fn a_change_is_never_decided_over_mcp_or_by_an_agent() {
    let config = Config::for_tests();
    let state = AppState::new(config, None).with_mirror(Arc::new(Mirror::new()));
    let person = ops::Caller {
        bearer: None,
        identity: joinedcontext_portal::auth::session::Identity {
            client: None,
            subject: "sub-steward".into(),
            username: "steward".into(),
            email: Some("steward@banskabystrica.sk".into()),
            name: None,
            roles: vec!["portal-approver".into()],
            groups: vec!["platform-admins".into()],
            second_factor: false,
        },
        via: ops::Via::Session,
        access: None,
    };
    let over_mcp = ops::Caller {
        bearer: None,
        via: ops::Via::Mcp,
        ..person.clone()
    };
    let agent = ops::Caller {
        bearer: None,
        via: ops::Via::Agent,
        ..person.clone()
    };

    for name in ["jc_change_approve", "jc_change_reject"] {
        let Some(op) = ops::find(name) else { continue };
        let input = json!({ "id": "chg-0000beef" });

        for (who, caller) in [("mcp", &over_mcp), ("agent", &agent)] {
            let refused = ops::call(op, caller, &state, "ovzdusie", input.clone())
                .await
                .unwrap_err();
            let said = format!("{refused:?}");
            assert!(
                said.contains("a person does"),
                "{name} over {who} must be refused by the AG-11 gate: {said}"
            );
        }

        // The person is not stopped here; whatever answers comes from the route's own checks.
        let answered = ops::call(op, &person, &state, "ovzdusie", input).await;
        let said = format!("{answered:?}");
        assert!(
            !said.contains("a person does"),
            "{name}: a session caller must pass the AG-11 gate: {said}"
        );
    }
}

/// T-1019, AG-70: an agent run is offered the intersection of its profile and the person who
/// started it, never the person's whole reach. `listing` asks both — the profile through
/// `may_run` and the grants through `permitted` — so a run declared for one operation is not
/// handed the rest because its starter could run them.
#[tokio::test]
async fn a_run_is_offered_its_profile_and_not_its_starter_whole_reach() {
    use joinedcontext_portal::agents::access::Access;

    let config = Config::for_tests();
    let state = AppState::new(config, None).with_mirror(Arc::new(Mirror::new()));
    let identity = joinedcontext_portal::auth::session::Identity {
        client: None,
        subject: "sub-steward".into(),
        username: "steward".into(),
        email: Some("steward@banskabystrica.sk".into()),
        name: None,
        roles: vec!["portal-approver".into()],
        groups: vec!["platform-admins".into()],
        second_factor: false,
    };

    let person = ops::Caller {
        bearer: None,
        identity: identity.clone(),
        via: ops::Via::Session,
        access: None,
    };
    let whole = ops::listing(&person, &state, "ovzdusie");
    assert!(
        whole.len() > 1,
        "the starter reaches more than one operation, or the test proves nothing"
    );

    // The same person, running under a profile that declares one operation.
    let narrow = ops::Caller::for_run(
        identity,
        Access::from_spec(&json!({ "access": { "operations": ["jc_catalog_search"] } })),
    );
    let listed = ops::listing(&narrow, &state, "ovzdusie");
    let offered: Vec<&str> = listed.iter().map(|op| op.name.as_str()).collect();
    assert!(
        offered.len() < whole.len(),
        "the profile narrows what the run is offered: {offered:?}"
    );
    assert!(
        !offered.contains(&"jc_change_approve"),
        "an operation the profile does not name is not offered: {offered:?}"
    );
}

/// T-3585, ADR-N-045: an agent reads the SDK's services catalog over MCP, read-only, and the
/// operation takes no input.
#[tokio::test]
async fn the_services_catalog_is_one_read_only_operation() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    state
        .mirror
        .upsert(joinedcontext_portal::resource::ResourceEnvelope {
            api_version: joinedcontext_portal::resource::API_VERSION.into(),
            kind: "Project".into(),
            metadata: joinedcontext_portal::resource::ObjectMeta::new(
                "ovzdusie",
                joinedcontext_portal::permissions::ORG_NAMESPACE,
            ),
            spec: json!({ "organizationRef": { "name": "bb" } }),
            status: None,
        });
    // A reader of the project, nothing more: the catalog is the SDK's, not the project's data.
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Role".to_string(),
        metadata: ObjectMeta::new("reader", "org"),
        spec: json!({ "rules": [{ "kinds": ["App"], "verbs": ["read"] }] }),
        status: None,
    });
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "RoleBinding".to_string(),
        metadata: ObjectMeta::new("agent-reader", "org"),
        spec: json!({
            "subjects": [{ "user": "agent" }],
            "role": "reader",
            "scope": { "project": "ovzdusie" }
        }),
        status: None,
    });
    let caller = ops::Caller {
        bearer: None,
        identity: joinedcontext_portal::auth::session::Identity {
            client: None,
            subject: "sub-agent".into(),
            username: "agent".into(),
            email: None,
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        },
        via: ops::Via::Mcp,
        access: None,
    };
    let op = ops::find("jc_app_services").expect("registered");
    assert!(op.annotations.read_only_hint && op.annotations.idempotent_hint);
    let catalog = ops::call(op, &caller, &state, "ovzdusie", json!({}))
        .await
        .expect("the catalog");
    let services: Vec<&str> = catalog["services"]
        .as_array()
        .expect("services")
        .iter()
        .filter_map(|s| s["service"].as_str())
        .collect();
    assert_eq!(
        services,
        ["identity", "data", "files", "email", "jobs", "ai"]
    );
    assert!(catalog["kit"].as_array().is_some_and(|kit| !kit.is_empty()));
    let refused = ops::call(
        op,
        &caller,
        &state,
        "ovzdusie",
        json!({ "service": "email" }),
    )
    .await;
    assert!(
        format!("{refused:?}").contains("'service' was unexpected"),
        "{refused:?}"
    );
}

/// T-5948, AG-146: `get_schema` answers every kind jc-core registers through the same door every
/// operation has, and a kind the platform lacks is refused at the input schema with its path.
#[tokio::test]
async fn get_schema_answers_every_kind_of_jc_core() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Project".into(),
        metadata: ObjectMeta::new("ovzdusie", joinedcontext_portal::permissions::ORG_NAMESPACE),
        spec: json!({ "organizationRef": { "name": "bb" } }),
        status: None,
    });
    let app = server::app(state);
    let cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );
    let post = |body: Value| {
        Request::builder()
            .method("POST")
            .uri("/api/v1/projects/ovzdusie/ops/jc_schema_get")
            .header(header::COOKIE, &cookie)
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request")
    };
    let kinds: Vec<&str> = jc_core::registry::KINDS.iter().map(|k| k.kind).collect();
    assert!(kinds.len() >= 30, "{kinds:?}");
    for kind in &kinds {
        let resp = app
            .clone()
            .oneshot(post(json!({ "kind": kind })))
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::OK, "{kind}");
        let body: Value =
            serde_json::from_slice(&resp.into_body().collect().await.expect("body").to_bytes())
                .expect("json");
        assert_eq!(body["kind"], *kind);
        assert!(
            body["fields"]
                .as_str()
                .is_some_and(|f| f.contains("spec: object, required")),
            "{kind}: {body}"
        );
        assert_eq!(body["example"]["kind"], *kind);
    }
    let resp = app
        .clone()
        .oneshot(post(json!({ "kind": "Pipeline", "path": "/spec/class" })))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(post(json!({ "kind": "Pipeline", "path": "/spec/nosuch" })))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let resp = app
        .clone()
        .oneshot(post(json!({ "kind": "NoSuchKind" })))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// T-6291: a 200-row activity page answered 196 KB, which Claude Code would not read. The
/// operation's page stops at `PAGE_BYTES` and its `next` reads the rest; nothing is lost or
/// repeated across the pages.
#[tokio::test]
async fn an_activity_page_stays_under_its_byte_bound_and_next_reads_the_rest() {
    use joinedcontext_portal::activity::ActivityEvent;
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta::new("ovzdusie", "ovzdusie"),
        spec: json!({ "isSandbox": false }),
        status: None,
    });
    let state = AppState::new(Config::for_tests(), None).with_mirror(mirror);
    let start = chrono::Utc::now();
    let events: Vec<ActivityEvent> = (0..200)
        .map(|n| ActivityEvent {
            time: start - chrono::Duration::seconds(n),
            project: "ovzdusie".into(),
            space: None,
            kind: "pipeline.run".into(),
            source: "reconciler".into(),
            summary: format!("run {n} of counters-hourly wrote {}", "x".repeat(700)),
            severity: "info".into(),
            correlation_id: None,
            details: json!({ "object": "pipelines/counters-hourly", "n": n }),
        })
        .collect();
    state.activity.append(&events).await.expect("appended");
    let caller = ops::Caller {
        bearer: None,
        identity: Identity {
            client: None,
            subject: "sub-steward".into(),
            username: "steward".into(),
            email: Some("steward@banskabystrica.sk".into()),
            name: None,
            roles: vec!["portal-approver".into()],
            groups: vec!["platform-admins".into()],
            second_factor: false,
        },
        via: ops::Via::Mcp,
        access: None,
    };

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let mut input = json!({ "limit": 200 });
        if let Some(cursor) = &cursor {
            input["cursor"] = json!(cursor);
        }
        let page = ops::call(
            ops::find("jc_activity_list").expect("registered"),
            &caller,
            &state,
            "ovzdusie",
            input,
        )
        .await
        .expect("a page");
        let bytes = page["items"].to_string().len();
        assert!(bytes <= ops::PAGE_BYTES, "{bytes} bytes");
        let items = page["items"].as_array().expect("items");
        assert!(!items.is_empty());
        seen.extend(
            items
                .iter()
                .map(|item| item["details"]["n"].as_i64().expect("n")),
        );
        match page["next"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(
        seen,
        (0..200).collect::<Vec<i64>>(),
        "every event once, newest first"
    );
}

fn change(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Change",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "status": { "lane": "green", "phase": "PendingApproval", "plan": { "create": 1 } },
        "summary": { "key": "changes.create", "params": {} },
        "author": { "name": "steward" },
        "createdAt": "2026-10-31T23:00:00Z"
    })
}

/// T-6291: `jc_change_list` answered every open change at once (60 proposals reach 49 KB).
/// `limit` 5 answers five and a `next` that reads on, and a cursor naming a
/// change no longer listed is refused by its field.
#[test]
fn the_change_list_pages_by_limit_and_cursor() {
    use joinedcontext_portal::api::changes::ChangeList;
    use joinedcontext_portal::ops::changes::{page_of, ChangeListInput};
    let list: ChangeList = serde_json::from_value(json!({
        "apiVersion": API_VERSION,
        "kind": "ChangeList",
        "items": (0..12)
            .map(|n| change(&format!("chg-{n:08x}")))
            .collect::<Vec<_>>()
    }))
    .expect("a list");
    let names = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["metadata"]["name"].as_str().expect("name").to_owned())
            .collect()
    };
    let input = |value: Value| -> ChangeListInput { serde_json::from_value(value).expect("input") };

    let first = page_of(list.clone(), &input(json!({ "limit": 5 })), |item| item).expect("a page");
    assert_eq!(names(&first).len(), 5);
    assert_eq!(first["next"], json!("chg-00000004"));
    let second = page_of(
        list.clone(),
        &input(json!({ "limit": 5, "cursor": "chg-00000004" })),
        |item| item,
    )
    .expect("a page");
    assert_eq!(names(&second)[0], "chg-00000005");
    let last = page_of(
        list.clone(),
        &input(json!({ "limit": 5, "cursor": "chg-00000009" })),
        |item| item,
    )
    .expect("a page");
    assert_eq!(names(&last), vec!["chg-0000000a", "chg-0000000b"]);
    assert_eq!(last.get("next"), None, "the end names no next");

    assert_eq!(
        names(&page_of(list.clone(), &input(json!({})), |item| item).expect("a page")).len(),
        12
    );

    let gone = page_of(list, &input(json!({ "cursor": "chg-ffffffff" })), |item| {
        item
    });
    assert!(
        matches!(gone, Err(ops::OpError::InvalidInput { ref path, .. }) if path == "cursor"),
        "{gone:?}"
    );
    assert!(serde_json::from_value::<ChangeListInput>(json!({ "page": 2 })).is_err());
}
