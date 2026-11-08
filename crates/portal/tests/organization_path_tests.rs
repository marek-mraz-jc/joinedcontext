//! PF-121 (ADR-N-058 section 2.1): an Organization still in the legacy `org.yaml` is changed
//! there, and a new one lands at `organizations/{name}.yaml`. Written anywhere else, the
//! repository would hold the Organization twice and load neither. The change goes through the
//! real propose door over a shako stub, and the test reads the paths the Portal wrote.

mod common;
use common::CheckFirst;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request as Received, ResponseTemplate};

const CSRF_TOKEN: &str = "test-csrf-token-12345";
const REPO: &str = "/odata/Repositories(Owner='test-owner',Name='test-repo')";

fn cookies(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@hel.fi".into()),
            name: None,
            roles: Vec::new(),
            // The bootstrap group: the test is about the path, not about who may write.
            groups: vec![config.bootstrap_admins.clone()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store session");
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

fn spec(domain: &str) -> Value {
    json!({ "domain": domain, "locales": ["sk", "en"], "defaultLocale": "sk" })
}

fn organization(name: &str, domain: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Organization",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": spec(domain),
    })
}

fn b64(text: &str) -> String {
    base64::engine::Engine::encode(&base64::engine::general_purpose::STANDARD, text.as_bytes())
}

/// A forge whose `main` holds `hel` in the legacy `org.yaml` and nothing else: every other file
/// is absent, every write and the pull request are accepted.
async fn forge() -> MockServer {
    let shako = MockServer::start().await;
    let legacy = serde_norway::to_string(&organization("hel", "hel.fi")).expect("yaml");
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", "'org.yaml'"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "b-org", "Content": b64(&legacy) })),
        )
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .with_priority(10)
        .mount(&shako)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.CreateBranch")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&shako)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c1" })))
        .with_priority(10)
        .mount(&shako)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.OpenPullRequest")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Number": 7,
            "HtmlUrl": "https://shako.example/pulls/7",
            "State": "open",
            "Mergeable": true,
            "Merged": false
        })))
        .mount(&shako)
        .await;
    shako
}

/// Proposes `body` as `name` through `PUT /api/v1/projects/org/organizations/{name}` and answers
/// the status and the repository paths the Portal wrote.
async fn propose(
    shako: &MockServer,
    held: bool,
    name: &str,
    body: Value,
) -> (StatusCode, Vec<String>) {
    let config = Config::for_tests();
    let client = ShakoClient::new(
        shako.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "t",
    )
    .expect("client");
    let state = AppState::new(config.clone(), None).with_shako(Arc::new(client));
    if held {
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Organization".to_owned(),
            metadata: ObjectMeta::new("hel", ORG_NAMESPACE),
            spec: spec("hel.fi"),
            status: None,
        });
    }
    let response = server::app(state)
        .oneshot_checked(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/projects/{ORG_NAMESPACE}/organizations/{name}"
                ))
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let written = shako
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r: &&Received| {
            r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/Shako.CommitFiles")
        })
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .flat_map(|commit| commit["Files"].as_array().cloned().unwrap_or_default())
        .filter_map(|file| file["path"].as_str().map(str::to_owned))
        .collect();
    (response.status(), written)
}

#[tokio::test]
async fn an_organization_in_the_legacy_org_yaml_is_changed_there() {
    let shako = forge().await;
    // The same Organization with a title: an update that changes something.
    let mut titled = organization("hel", "hel.fi");
    titled["metadata"]["title"] = json!("City of Helsinki");
    let (status, written) = propose(&shako, true, "hel", titled).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{written:?}");
    assert_eq!(written, ["org.yaml"]);
}

#[tokio::test]
async fn a_new_organization_lands_in_its_own_file() {
    let shako = forge().await;
    let (status, written) =
        propose(&shako, true, "zilina", organization("zilina", "zilina.sk")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{written:?}");
    assert_eq!(written, ["organizations/zilina.yaml"]);
}
