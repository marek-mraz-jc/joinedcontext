//! The CKAN view of a project: what it publishes and where (T-0318, EP-62…EP-67, UI-05).
//!
//! Configuring a catalogue is deliberately not tested against a route of its own: a
//! `CkanInstance` is a manifest, so it is written through the resource API as a change
//! proposal like everything else, and that is what the last two cases assert.

mod common;
use common::CheckFirst;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
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
const SLUG: &str = "zt4qm7ge2xdv6ksb3ncf5arw2y";
const INSTANCE_PATH: &str =
    "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/ckan/open-data.yaml";

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
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
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|raw| raw.split(';').next())
        .collect::<Vec<_>>()
        .join("; ")
}

fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        },
        spec,
        status: Some(Status {
            phase: Phase::Live,
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

fn endpoint_spec(publish: Value) -> Value {
    let mut spec = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
        "slug": SLUG,
        "audience": "public",
        "enabledRepresentations": ["ngsi-ld", "csv"],
    });
    if !publish.is_null() {
        spec["publish"] = publish;
    }
    spec
}

fn seeded() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "CkanInstance",
        "open-data",
        json!({
            "url": "https://data.banskabystrica.sk",
            "organizationDefault": "mesto-banska-bystrica",
            "apiTokenRef": { "name": "ckan-open-data", "key": "apiToken" },
        }),
    ));
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie-public",
        endpoint_spec(json!({
            "ckan": {
                "instanceRef": { "kind": "CkanInstance", "name": "open-data" },
                "organization": "mesto-banska-bystrica",
                "name": "kvalita-ovzdusia",
            }
        })),
    ));
    // An endpoint that publishes nowhere, which must not appear in the answer at all.
    mirror.upsert(envelope(
        "Endpoint",
        "internal-air",
        endpoint_spec(Value::Null),
    ));
    mirror
}

async fn status_of(mirror: Arc<Mirror>) -> (StatusCode, Value) {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/ckan/status")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .expect("request"),
        )
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

/// EP-62, EP-64: the catalogue, the dataset and one resource per enabled representation.
#[tokio::test]
async fn the_status_names_the_catalogue_and_every_published_resource() {
    let (status, body) = status_of(seeded()).await;
    assert_eq!(status, StatusCode::OK);

    let instances = body["instances"].as_array().expect("instances");
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0]["name"], json!("open-data"));
    assert_eq!(instances[0]["url"], json!("https://data.banskabystrica.sk"));
    assert_eq!(
        instances[0]["organizationDefault"],
        json!("mesto-banska-bystrica")
    );

    let publications = body["publications"].as_array().expect("publications");
    assert_eq!(
        publications.len(),
        1,
        "only the publishing endpoint: {body}"
    );
    let publication = &publications[0];
    assert_eq!(publication["endpoint"], json!("ovzdusie-public"));
    assert_eq!(publication["phase"], json!("Live"));
    assert_eq!(publication["dataset"], json!("kvalita-ovzdusia"));
    assert_eq!(
        publication["datasetUrl"],
        json!("https://data.banskabystrica.sk/dataset/kvalita-ovzdusia")
    );
    assert_eq!(publication["instanceMissing"], json!(false));
    // CKAN holds no rows (T-5060): the publication names no row mirror.
    assert!(publication.get("datastore").is_none(), "{publication}");

    let urls: Vec<&str> = publication["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .map(|resource| resource["url"].as_str().expect("a url"))
        .collect();
    assert_eq!(
        urls,
        vec![
            format!("https://localhost/api/endpoint/{SLUG}/ngsi-ld/v1/"),
            format!("https://localhost/api/endpoint/{SLUG}/file.csv"),
            // Every Endpoint serves its MCP instance unless `mcp: false` (EP-24, T-2901).
            format!("https://localhost/api/endpoint/{SLUG}/mcp"),
            format!("https://localhost/api/endpoint/{SLUG}/schema/index.json"),
            // The Endpoint's own DCAT-AP record the dataset is made from (EP-143).
            format!("https://localhost/api/endpoint/{SLUG}/"),
        ],
        "every resource is served by the endpoint itself (EP-66)"
    );
}

/// EP-24: an Endpoint that turns its MCP instance off publishes no MCP resource either.
#[tokio::test]
async fn an_endpoint_without_mcp_publishes_no_mcp_resource() {
    let mirror = seeded();
    let mut spec = endpoint_spec(json!({
        "ckan": {
            "instanceRef": { "kind": "CkanInstance", "name": "open-data" },
            "organization": "mesto-banska-bystrica",
            "name": "kvalita-ovzdusia",
        }
    }));
    spec["mcp"] = json!(false);
    mirror.upsert(envelope("Endpoint", "ovzdusie-public", spec));
    let (status, body) = status_of(mirror).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let urls: Vec<&str> = body["publications"][0]["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .map(|resource| resource["url"].as_str().expect("a url"))
        .collect();
    assert!(!urls.is_empty(), "{body}");
    assert!(!urls.iter().any(|url| url.ends_with("/mcp")), "{urls:?}");
}

/// EP-67: the answer names the secret, never its value.
#[tokio::test]
async fn the_status_carries_the_token_reference_and_no_token() {
    let (_, body) = status_of(seeded()).await;
    let serialized = body.to_string();

    assert_eq!(
        body["instances"][0]["apiTokenRef"],
        json!("ckan-open-data"),
        "the reference is what a steward needs to see"
    );
    for forbidden in ["apiToken\":\"", "\"token\"", "\"secret\""] {
        assert!(
            !serialized.contains(forbidden),
            "the status carried {forbidden}: {serialized}"
        );
    }
}

/// A dangling reference is the most likely misconfiguration, so it is shown rather than hidden.
#[tokio::test]
async fn an_endpoint_pointing_at_a_catalogue_that_is_not_there_says_so() {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie-public",
        endpoint_spec(json!({
            "ckan": { "instanceRef": { "kind": "CkanInstance", "name": "gone" } }
        })),
    ));

    let (status, body) = status_of(mirror).await;

    assert_eq!(status, StatusCode::OK);
    let publication = &body["publications"][0];
    assert_eq!(publication["instance"], json!("gone"));
    assert_eq!(publication["instanceMissing"], json!(true));
    assert!(publication["datasetUrl"].is_null(), "{publication}");
    assert_eq!(publication["resources"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn the_status_needs_a_session() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None).with_mirror(seeded()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/ckan/status")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// CC-03, CC-08: a new catalogue is a change proposal, not a setting somebody flips.
#[tokio::test]
async fn a_catalogue_is_configured_through_the_resource_api_as_a_change_proposal() {
    let forge = MockServer::start().await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(INSTANCE_PATH))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&forge)
        .await;
    Mock::given(method("PUT"))
        .and(path(INSTANCE_PATH))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({ "commit": { "sha": "commit-sha-created" } })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 7,
            "html_url": "https://gitea.example.sk/pulls/7",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&forge)
        .await;

    let config = Config::for_tests();
    let cookies = format!("{}; {CSRF_COOKIE}={CSRF_TOKEN}", session_cookie(&config));
    let app = server::app(AppState::new(config, None).with_gitea(Arc::new(client)));

    let response = app
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ckaninstances")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookies)
                .header(CSRF_HEADER, CSRF_TOKEN)
                .body(Body::from(
                    json!({
                        "apiVersion": API_VERSION,
                        "kind": "CkanInstance",
                        "metadata": { "name": "open-data", "namespace": "ovzdusie" },
                        "spec": {
                            "url": "https://data.banskabystrica.sk",
                            "organizationDefault": "mesto-banska-bystrica",
                            "apiTokenRef": { "name": "ckan-open-data", "key": "apiToken" }
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "the catalogue lands as a proposal on the path jc-core gives the kind"
    );
}

/// EP-67, MF-24: a pasted token never reaches a commit.
#[tokio::test]
async fn a_catalogue_carrying_an_inline_token_is_refused() {
    let config = Config::for_tests();
    let cookies = format!("{}; {CSRF_COOKIE}={CSRF_TOKEN}", session_cookie(&config));
    let app = server::app(AppState::new(config, None).with_mirror(seeded()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ckaninstances")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookies)
                .header(CSRF_HEADER, CSRF_TOKEN)
                .body(Body::from(
                    json!({
                        "apiVersion": API_VERSION,
                        "kind": "CkanInstance",
                        "metadata": { "name": "open-data", "namespace": "ovzdusie" },
                        "spec": {
                            "url": "https://data.banskabystrica.sk",
                            "apiToken": "ckan-token-pasted-by-hand"
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: Value = serde_json::from_slice(&bytes).expect("problem details");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("apiToken")),
        "the steward has to learn which field was refused: {problem}"
    );
}
