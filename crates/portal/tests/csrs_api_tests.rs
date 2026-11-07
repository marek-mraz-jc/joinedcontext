//! T-0304: the registrations of a project and the graph they form (MF-36, UI-27, EP-71, PF-48),
//! read-only for every role that may read the project (CC-36). A city joins or leaves a
//! federation by a registration manifest alone, proposed like any other (CC-14).
//!
//! Creating a registration is deliberately not tested against a route of its own. A
//! `ContextSourceRegistration` is a manifest, so it is written through the resource API as a
//! change proposal like everything else, and the write case below asserts exactly that: the
//! kind's own repository path, and the Red lane a federation edge always takes (CC-63).

mod common;
use common::CheckFirst;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{classify, Lane, Operation};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, Phase, ResourceEnvelope, Status, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF_TOKEN: &str = "test-csrf-token-12345";
const PROJECT: &str = "ovzdusie";
const MANIFEST_PATH: &str = "projects/ovzdusie/spaces/ovzdusie/registrations/zvolen-ovzdusie.yaml";

/// The address of an external source, which must never appear in the graph (UI-27).
const EXTERNAL_URL: &str = "https://context.zvolen.sk/ngsi-ld/v1";

fn cookies(config: &Config) -> String {
    cookies_for(config, "demo.steward", &["portal-approver"])
}

fn cookies_for(config: &Config, username: &str, groups: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@banskabystrica.sk")),
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
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
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF_TOKEN}"));
    parts.join("; ")
}

fn envelope(
    kind: &str,
    name: &str,
    namespace: &str,
    spec: Value,
    phase: Option<Phase>,
) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: phase.map(|phase| Status {
            phase,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    }
}

/// A hub space with one registration onto a local endpoint, one onto a source elsewhere, the
/// pipeline that fills the local one and the app that reads the hub.
fn seeded() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());

    for space in ["hub", "mesto-ovzdusie"] {
        mirror.upsert(envelope(
            "ContextSpace",
            space,
            PROJECT,
            json!({ "dataModelRefs": [] }),
            Some(Phase::Live),
        ));
    }
    mirror.upsert(envelope(
        "Endpoint",
        "mesto-read",
        PROJECT,
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "mesto-ovzdusie" },
            "audience": "public",
        }),
        Some(Phase::Live),
    ));
    mirror.upsert(envelope(
        "ContextSourceRegistration",
        "mesto-ovzdusie",
        PROJECT,
        json!({
            "contextSpaceRef": "hub",
            "endpointRef": { "kind": "Endpoint", "name": "mesto-read" },
            "information": [{
                "entities": [{ "type": "AirQualityObserved" }],
                "propertyNames": ["pm10"]
            }],
            "federation": {
                "identity": "serviceAccount",
                "serviceAccountRef": { "kind": "ServiceAccount", "name": "hub-reader" }
            },
            "mode": "exclusive",
        }),
        Some(Phase::Live),
    ));
    mirror.upsert(envelope(
        "ContextSourceRegistration",
        "zvolen-ovzdusie",
        PROJECT,
        json!({
            "contextSpaceRef": "hub",
            "endpoint": EXTERNAL_URL,
            "information": [{ "entities": [
                { "type": "AirQualityObserved" },
                { "type": "WeatherObserved" }
            ]}],
            "federation": { "identity": "caller" },
        }),
        Some(Phase::Error),
    ));
    mirror.upsert(envelope(
        "Pipeline",
        "aq-ingest",
        PROJECT,
        json!({
            "class": "streaming",
            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:mesto-ovzdusie:mesto-read",
        }),
        None,
    ));
    mirror.upsert(envelope(
        "App",
        "aq-map",
        PROJECT,
        json!({ "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "hub" },
            "types": ["AirQualityObserved"],
            "operations": ["retrieveEntity"]
        }]}),
        Some(Phase::Live),
    ));

    // Another project's registration, to prove the answer is scoped and not merely by kind.
    mirror.upsert(envelope(
        "ContextSourceRegistration",
        "doprava-zvolen",
        "doprava",
        json!({
            "contextSpaceRef": "hub",
            "endpoint": "https://context.zvolen.sk/doprava",
            "information": [{ "entities": [{ "type": "Vehicle" }] }],
            "federation": { "identity": "caller" },
        }),
        None,
    ));
    mirror
}

async fn get(uri: &str, mirror: Arc<Mirror>) -> (StatusCode, Value) {
    let config = Config::for_tests();
    let cookie = cookies(&config);
    get_as(uri, mirror, config, cookie).await
}

async fn get_as(
    uri: &str,
    mirror: Arc<Mirror>,
    config: Config,
    cookie: String,
) -> (StatusCode, Value) {
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    let response = app
        .oneshot(request(uri, Some(cookie)))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, value)
}

/// A read: a GET of a route, or a POST of a registry operation (`…/ops/{name}`, T-4453).
fn request(uri: &str, cookie: Option<String>) -> Request<Body> {
    let csrf = format!("{CSRF_COOKIE}={CSRF_TOKEN}");
    let builder = Request::builder().uri(uri);
    let builder = if uri.contains("/ops/") {
        let cookie = cookie.map_or(csrf.clone(), |c| format!("{c}; {csrf}"));
        builder
            .method("POST")
            .header(header::COOKIE, cookie)
            .header(CSRF_HEADER, CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
    } else {
        match cookie {
            Some(cookie) => builder.header(header::COOKIE, cookie),
            None => builder,
        }
    };
    let body = if uri.contains("/ops/") { "{}" } else { "" };
    builder.body(Body::from(body)).expect("request")
}

fn node<'a>(graph: &'a Value, id: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["id"] == json!(id))
        .unwrap_or_else(|| panic!("no node {id} in {graph}"))
}

fn edges(graph: &Value) -> Vec<(String, String, String)> {
    graph["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge["from"].as_str().unwrap_or_default().to_owned(),
                edge["to"].as_str().unwrap_or_default().to_owned(),
                edge["kind"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_collection_lists_only_this_projects_registrations() {
    let (status, body) = get("/api/v1/projects/ovzdusie/csrs", seeded()).await;
    assert_eq!(status, StatusCode::OK);

    let names: Vec<&str> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["metadata"]["name"].as_str())
        .collect();
    assert_eq!(names, vec!["mesto-ovzdusie", "zvolen-ovzdusie"]);
}

#[tokio::test]
async fn one_registration_comes_back_by_name_and_an_unknown_one_is_not_found() {
    let (status, body) = get("/api/v1/projects/ovzdusie/csrs/zvolen-ovzdusie", seeded()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kind"], json!("ContextSourceRegistration"));
    assert_eq!(body["spec"]["federation"]["identity"], json!("caller"));

    let (status, _) = get("/api/v1/projects/ovzdusie/csrs/nikde", seeded()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// MF-36: the kind is addressed by the plural the specification gives it, and its manifest
/// lands under the space it registers into.
#[tokio::test]
async fn the_kind_is_addressable_by_its_plural_and_lands_in_its_space() {
    let info = joinedcontext_portal::resource::by_plural("csrs").expect("catalogued");
    assert_eq!(info.kind, "ContextSourceRegistration");
    assert_eq!(
        joinedcontext_portal::resource::repository_path(
            info,
            PROJECT,
            Some(PROJECT),
            "zvolen-ovzdusie"
        )
        .expect("a space was given"),
        MANIFEST_PATH
    );
    // A space-scoped kind with no space would be written to a path holding a literal
    // `{space}`, which the reconciler never walks and nobody would see was wrong.
    assert!(joinedcontext_portal::resource::repository_path(
        info,
        PROJECT,
        None,
        "zvolen-ovzdusie"
    )
    .is_err());
}

/// CC-63, CC-14: a federation edge is a Red-lane change however it is proposed, because it makes
/// one tenant's data answerable in another; and joining is that one registration manifest at
/// its own path, with no code and no broker step beside it.
#[tokio::test]
async fn a_write_becomes_a_red_lane_merge_request_at_the_kinds_own_path() {
    assert_eq!(
        classify("ContextSourceRegistration", Operation::Create, &Value::Null),
        Lane::Red
    );

    let gitea = MockServer::start().await;
    let base_url = gitea.uri().parse().expect("mock url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{MANIFEST_PATH}"
        )))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&gitea)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{MANIFEST_PATH}"
        )))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "commit-1" } })),
        )
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 9,
            "html_url": "https://gitea.example.sk/pulls/9",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&gitea)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let response = server::app(state)
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/csrs")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "ContextSourceRegistration",
                        "metadata": { "name": "zvolen-ovzdusie", "namespace": PROJECT },
                        "spec": {
                            "contextSpaceRef": "hub",
                            "endpoint": EXTERNAL_URL,
                            "information": [{ "entities": [{ "type": "AirQualityObserved" }] }],
                            "federation": { "identity": "caller" }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    // The mocks answer only MANIFEST_PATH: a wrong path template would have missed them and
    // the write would have failed rather than reaching a merge request.
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

/// MF-24, CC-06: a credential typed into a registration is refused before a branch exists, so
/// the secret never reaches Git even as an abandoned commit.
///
/// This is the write path's whole check on a spec. The claim rules of the kind itself — a
/// registration that names both targets or claims nothing — are enforced by jc-core when the
/// reconciler reads the merged file, not here; T-0412 is the task for closing that gap for
/// every kind at once.
#[tokio::test]
async fn a_credential_typed_into_a_registration_is_refused_before_anything_is_written() {
    let config = Config::for_tests();
    let response = server::app(AppState::new(config.clone(), None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/csrs")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "ContextSourceRegistration",
                        "metadata": { "name": "zvolen-ovzdusie", "namespace": PROJECT },
                        "spec": {
                            "contextSpaceRef": "hub",
                            "endpoint": EXTERNAL_URL,
                            "information": [{ "entities": [{ "type": "AirQualityObserved" }] }],
                            "federation": { "identity": "caller", "token": "hunter2" }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// T-0412: the kind's own invariants run before a branch exists. jc-core refuses a
/// registration with no entity selector; so does the Portal now, naming the field.
#[tokio::test]
async fn a_registration_with_no_selector_is_refused_before_anything_is_written() {
    let config = Config::for_tests();
    let response = server::app(AppState::new(config.clone(), None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/csrs")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "ContextSourceRegistration",
                        "metadata": { "name": "zvolen-ovzdusie", "namespace": PROJECT },
                        "spec": {
                            "contextSpaceRef": "hub",
                            "endpoint": EXTERNAL_URL,
                            "information": []
                        }
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let problem: Value = serde_json::from_slice(&body).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("ContextSourceRegistration"), "{detail}");
    assert!(
        detail.contains("information") || detail.contains("selector"),
        "{detail}"
    );
}

/// UI-27: every kind the federation touches is a node, and every edge names the manifest it
/// came from so a reader can open it.
#[tokio::test]
async fn the_graph_draws_every_object_and_names_the_manifest_behind_each_edge() {
    let (status, graph) = get(
        "/api/v1/projects/ovzdusie/ops/jc_federation_graph",
        seeded(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let ids: Vec<&str> = graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "App/aq-map",
            "ContextSourceRegistration/mesto-ovzdusie",
            "ContextSourceRegistration/zvolen-ovzdusie",
            "ContextSpace/hub",
            "ContextSpace/mesto-ovzdusie",
            "Endpoint/mesto-read",
            "ExternalSource/zvolen-ovzdusie",
            "Pipeline/aq-ingest",
        ],
        "another project's registration is not in this project's graph"
    );

    assert_eq!(
        edges(&graph),
        vec![
            (
                "App/aq-map".into(),
                "ContextSpace/hub".into(),
                "consumes".into()
            ),
            (
                "ContextSourceRegistration/mesto-ovzdusie".into(),
                "ContextSpace/hub".into(),
                "registers".into()
            ),
            (
                "ContextSourceRegistration/mesto-ovzdusie".into(),
                "Endpoint/mesto-read".into(),
                "registers".into()
            ),
            (
                "ContextSourceRegistration/zvolen-ovzdusie".into(),
                "ContextSpace/hub".into(),
                "registers".into()
            ),
            (
                "ContextSourceRegistration/zvolen-ovzdusie".into(),
                "ExternalSource/zvolen-ovzdusie".into(),
                "registers".into()
            ),
            (
                "Endpoint/mesto-read".into(),
                "ContextSpace/mesto-ovzdusie".into(),
                "serves".into()
            ),
            (
                "Pipeline/aq-ingest".into(),
                "Endpoint/mesto-read".into(),
                "feeds".into()
            ),
        ]
    );

    for edge in graph["edges"].as_array().expect("edges") {
        assert!(
            edge["manifest"].as_str().is_some_and(|m| m.contains('/')),
            "every edge names the manifest it came from: {edge}"
        );
    }
}

/// UI-27: health is the phase the object reported, and an object that has not reported is
/// `unknown` rather than green.
#[tokio::test]
async fn health_is_what_the_object_reported_and_never_a_guess() {
    let (_, graph) = get(
        "/api/v1/projects/ovzdusie/ops/jc_federation_graph",
        seeded(),
    )
    .await;

    assert_eq!(node(&graph, "ContextSpace/hub")["health"], json!("ok"));
    assert_eq!(
        node(&graph, "ContextSourceRegistration/zvolen-ovzdusie")["health"],
        json!("degraded")
    );
    assert_eq!(
        node(&graph, "Pipeline/aq-ingest")["health"],
        json!("unknown")
    );
    assert_eq!(
        node(&graph, "ExternalSource/zvolen-ovzdusie")["health"],
        json!("unknown"),
        "a source outside this platform reports nothing here"
    );
}

/// UI-27, PF-48, EP-71: a card says that a registration authenticates and how, never with
/// what, and an external source is named after the registration that reaches it rather than
/// after its address.
#[tokio::test]
async fn no_address_and_no_credential_is_anywhere_in_the_graph() {
    let (_, graph) = get(
        "/api/v1/projects/ovzdusie/ops/jc_federation_graph",
        seeded(),
    )
    .await;
    let text = serde_json::to_string(&graph).expect("serialises");

    for leaked in [
        EXTERNAL_URL,
        "context.zvolen.sk",
        "https://",
        "secretRef",
        "token",
    ] {
        assert!(
            !text.contains(leaked),
            "the graph carries {leaked}:\n{text}"
        );
    }

    let external = node(&graph, "ExternalSource/zvolen-ovzdusie");
    assert_eq!(external["name"], json!("zvolen-ovzdusie"));

    let card = &node(&graph, "ContextSourceRegistration/zvolen-ovzdusie")["registration"];
    assert_eq!(card["identity"], json!("caller"));
    assert_eq!(card["external"], json!(true));
    assert_eq!(
        card["mode"],
        json!("inclusive"),
        "the default is spelled out"
    );
    assert_eq!(
        card["types"],
        json!(["AirQualityObserved", "WeatherObserved"])
    );

    let local = &node(&graph, "ContextSourceRegistration/mesto-ovzdusie")["registration"];
    assert_eq!(local["identity"], json!("serviceAccount"));
    assert_eq!(local["external"], json!(false));
    assert_eq!(local["mode"], json!("exclusive"));
    assert!(
        !serde_json::to_string(local)
            .expect("serialises")
            .contains("hub-reader"),
        "the account a hub forwards as is resolved at request time, not drawn on a card"
    );
}

/// A registration whose endpoint has left the repository still draws its edge: a dangling
/// reference is what this view exists to make visible, and hiding it would make a broken
/// federation look like no federation at all.
#[tokio::test]
async fn a_registration_onto_a_missing_endpoint_still_draws_its_edge() {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "ContextSourceRegistration",
        "dangling",
        PROJECT,
        json!({
            "contextSpaceRef": "hub",
            "endpointRef": { "kind": "Endpoint", "name": "gone" },
            "information": [{ "entities": [{ "type": "AirQualityObserved" }] }],
            "federation": { "identity": "caller" },
        }),
        None,
    ));

    let (_, graph) = get("/api/v1/projects/ovzdusie/ops/jc_federation_graph", mirror).await;
    assert!(edges(&graph).contains(&(
        "ContextSourceRegistration/dangling".into(),
        "Endpoint/gone".into(),
        "registers".into()
    )));
    assert!(
        graph["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .all(|node| node["id"] != json!("Endpoint/gone")),
        "the target is drawn as missing, not invented: {graph}"
    );
}

/// CC-36: a person bound only to a read role sees the same registrations and the same graph
/// as a steward, and somebody bound to nothing in the organization sees no project at all.
#[tokio::test]
async fn a_viewer_reads_the_topology_and_a_stranger_does_not() {
    let mirror = seeded();
    mirror.upsert(envelope(
        "Role",
        "viewer",
        "org",
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint", "ContextSourceRegistration", "Pipeline", "App", "ServiceAccount", "CkanInstance"], "verbs": ["read"] }] }),
        None,
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "jana-viewer",
        "org",
        json!({ "subjects": [{ "user": "jana.viewer@banskabystrica.sk" }], "role": "viewer",
                "scope": { "organization": "bbsk" } }),
        None,
    ));
    for uri in [
        "/api/v1/projects/ovzdusie/csrs",
        "/api/v1/projects/ovzdusie/ops/jc_federation_graph",
    ] {
        let (steward, as_steward) = get(uri, mirror.clone()).await;
        assert_eq!(steward, StatusCode::OK, "{uri}: {as_steward}");

        let config = Config::for_tests();
        let viewer = cookies_for(&config, "jana.viewer", &[]);
        let (status, as_viewer) = get_as(uri, mirror.clone(), config, viewer).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {as_viewer}");
        assert_eq!(
            as_viewer, as_steward,
            "{uri}: a viewer sees what a steward sees"
        );

        let config = Config::for_tests();
        let stranger = cookies_for(&config, "cudzi.clovek", &[]);
        let (status, body) = get_as(uri, mirror.clone(), config, stranger).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
}

/// Both routes are behind the session, so an unauthenticated reader learns nothing about who
/// federates with whom.
#[tokio::test]
async fn neither_route_answers_without_a_session() {
    for uri in [
        "/api/v1/projects/ovzdusie/csrs",
        "/api/v1/projects/ovzdusie/ops/jc_federation_graph",
    ] {
        let app = server::app(AppState::new(Config::for_tests(), None).with_mirror(seeded()));
        let response = app.oneshot(request(uri, None)).await.expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

/// UI-28, EP-62: an endpoint published to CKAN draws the catalogue and a `publishes` edge; a
/// catalogue nobody publishes to stays off the picture, a card never carries its URL, and every
/// node that reported carries its phase for the status chip.
#[tokio::test]
async fn a_published_endpoint_draws_its_catalogue_and_every_node_its_phase() {
    let mirror = seeded();
    mirror.upsert(envelope(
        "Endpoint",
        "mesto-open",
        PROJECT,
        json!({
            "contextSpaceRef": "hub",
            "audience": "public",
            "publish": { "ckan": {
                "instanceRef": { "kind": "CkanInstance", "name": "open-data" },
                "organization": "mesto"
            }},
        }),
        Some(Phase::Deploying),
    ));
    for name in ["open-data", "unused-catalogue"] {
        mirror.upsert(envelope(
            "CkanInstance",
            name,
            PROJECT,
            json!({
                "url": "https://data.example.sk",
                "apiTokenRef": { "name": "ckan-token", "key": "token" }
            }),
            Some(Phase::Live),
        ));
    }

    let (status, graph) = get("/api/v1/projects/ovzdusie/ops/jc_federation_graph", mirror).await;
    assert_eq!(status, StatusCode::OK);

    assert!(edges(&graph).contains(&(
        "Endpoint/mesto-open".into(),
        "CkanInstance/open-data".into(),
        "publishes".into()
    )));
    assert_eq!(
        node(&graph, "CkanInstance/open-data")["health"],
        json!("ok")
    );
    assert!(
        !graph["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| node["id"] == json!("CkanInstance/unused-catalogue")),
        "a catalogue nobody publishes to is not part of the federation"
    );
    let text = graph.to_string();
    assert!(
        !text.contains("data.example.sk"),
        "no catalogue URL on a card"
    );
    assert!(!text.contains("ckan-token"), "no token reference on a card");

    assert_eq!(
        node(&graph, "Endpoint/mesto-open")["phase"],
        json!("Deploying")
    );
    assert_eq!(node(&graph, "ContextSpace/hub")["phase"], json!("Live"));
    assert!(
        node(&graph, "ExternalSource/zvolen-ovzdusie")
            .get("phase")
            .is_none(),
        "a source outside this platform reports no phase"
    );
}
