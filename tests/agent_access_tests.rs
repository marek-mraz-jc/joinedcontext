//! The Access view of an agent's ServiceAccount and revoking a grant now (API/01 §52,
//! AG-130…AG-132): who may read it, who may revoke, and what a revoke refuses before the forge.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "agent-access-csrf";
const ACCESS: &str = "/api/v1/projects/ovzdusie/serviceaccounts/vendor-ci/access";
const REVOKE: &str = "/api/v1/projects/ovzdusie/serviceaccounts/vendor-ci/revoke";

fn person(email: &str, groups: &[&str]) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: groups.iter().map(|g| (*g).to_owned()).collect(),
        second_factor: false,
    }
}

fn cookies(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let s = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

fn role(name: &str, verbs: &[&str]) -> ResourceEnvelope {
    envelope(
        "Role",
        name,
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ServiceAccount"], "verbs": verbs }] }),
    )
}

fn bound(name: &str, role: &str, email: &str) -> ResourceEnvelope {
    envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": email }], "role": role, "scope": { "project": "ovzdusie" } }),
    )
}

fn app(config: &Config, roles: Value) -> axum::Router {
    server::app(seeded(AppState::new(config.clone(), None), roles))
}

fn seeded(state: AppState, roles: Value) -> AppState {
    for env in [
        envelope(
            "Role",
            "pipeline-admin",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose", "approve", "delete"] }] }),
        ),
        role("account-reader", &["read"]),
        role("account-admin", &["propose", "approve", "delete"]),
        bound("eva-reads", "account-reader", "eva@hel.fi"),
        bound("ana-admin", "account-admin", "ana@hel.fi"),
        envelope(
            "ServiceAccount",
            "vendor-ci",
            "ovzdusie",
            json!({
                "owner": { "user": "jana@hel.fi" },
                "purpose": "proposes pipelines from the vendor's CI",
                "roles": roles,
                "credentials": [{ "kind": "oauth-client", "name": "main" }]
            }),
        ),
    ] {
        state.mirror.upsert(env);
    }
    state
}

fn two_grants() -> Value {
    json!([
        { "role": "pipeline-admin", "scope": { "project": "ovzdusie" } },
        { "role": "pipeline-admin", "scope": { "project": "doprava" },
          "validity": { "notAfter": "2020-01-01T00:00:00Z" } }
    ])
}

async fn call(
    app: axum::Router,
    config: &Config,
    who: Option<Identity>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(who) = who {
        request = request
            .header(header::COOKIE, cookies(config, who))
            .header(CSRF_HEADER, CSRF);
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).expect("json"))
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn nobody_signed_in_reads_or_revokes_nothing() {
    let config = Config::for_tests();
    let (status, _) = call(
        app(&config, two_grants()),
        &config,
        None,
        "GET",
        ACCESS,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let revoke = json!({ "index": 1, "role": "pipeline-admin" });
    let (status, _) = call(
        app(&config, two_grants()),
        &config,
        None,
        "POST",
        REVOKE,
        Some(revoke),
    )
    .await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{status}"
    );
}

#[tokio::test]
async fn a_person_who_reads_no_service_account_gets_403() {
    let config = Config::for_tests();
    let who = person("olli@hel.fi", &[]);
    let (status, body) = call(
        app(&config, two_grants()),
        &config,
        Some(who),
        "GET",
        ACCESS,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn a_reader_sees_each_grant_with_its_state_and_may_not_revoke() {
    let config = Config::for_tests();
    let eva = person("eva@hel.fi", &[]);
    let (status, body) = call(
        app(&config, two_grants()),
        &config,
        Some(eva.clone()),
        "GET",
        ACCESS,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answer: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(answer["grants"][0]["state"], "active", "{answer}");
    assert_eq!(answer["grants"][1]["state"], "expired", "{answer}");
    assert_eq!(answer["grants"][0]["scope"], "project:ovzdusie");
    assert_eq!(answer["mayRevoke"], false);
    assert!(
        !body.contains("\"approve\""),
        "an account's rules never carry approve: {body}"
    );
    let missing = ACCESS.replace("vendor-ci", "nobody");
    let (status, _) = call(
        app(&config, two_grants()),
        &config,
        Some(eva.clone()),
        "GET",
        &missing,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let revoke = json!({ "index": 1, "role": "pipeline-admin" });
    let (status, body) = call(
        app(&config, two_grants()),
        &config,
        Some(eva),
        "POST",
        REVOKE,
        Some(revoke),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn an_administrator_may_revoke_but_not_a_moved_grant_nor_the_last_one() {
    let config = Config::for_tests();
    let ana = person("ana@hel.fi", &[]);
    let (_, body) = call(
        app(&config, two_grants()),
        &config,
        Some(ana.clone()),
        "GET",
        ACCESS,
        None,
    )
    .await;
    assert!(body.contains("\"mayRevoke\":true"), "{body}");

    let moved = json!({ "index": 1, "role": "space-reader" });
    let (status, body) = call(
        app(&config, two_grants()),
        &config,
        Some(ana.clone()),
        "POST",
        REVOKE,
        Some(moved),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let one = json!([{ "role": "pipeline-admin", "scope": { "project": "ovzdusie" } }]);
    let last = json!({ "index": 0, "role": "pipeline-admin" });
    let (status, body) = call(
        app(&config, one),
        &config,
        Some(ana.clone()),
        "POST",
        REVOKE,
        Some(last),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("last grant"), "{body}");

    let unknown = json!({ "index": 1, "role": "pipeline-admin", "everything": true });
    let (status, _) = call(
        app(&config, two_grants()),
        &config,
        Some(ana),
        "POST",
        REVOKE,
        Some(unknown),
    )
    .await;
    assert!(
        status.is_client_error() && status != StatusCode::FORBIDDEN,
        "{status}"
    );
}

/// The allow path: an administrator's revoke proposes the account without the grant and answers
/// 202 with the Change; a forge that cannot show the merge request leaves it pending for a person.
#[tokio::test]
async fn an_administrators_revoke_proposes_the_account_without_the_grant() {
    let forge = MockServer::start().await;
    let repo = "/api/v1/repos/owner/config";
    Mock::given(method("GET"))
        .and(path(repo))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/contents/.*vendor-ci"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "old", "content": "" })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"/contents/.*vendor-ci"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "sha": "new" } })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/pulls")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 7, "html_url": "https://gitea.example.org/pulls/7",
            "state": "open", "mergeable": true, "merged": false
        })))
        .mount(&forge)
        .await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("url"),
        "owner",
        "config",
        "token",
    )
    .expect("client");
    let config = Config::for_tests();
    let state = seeded(
        AppState::new(config.clone(), None).with_gitea(Arc::new(client)),
        two_grants(),
    );
    // Ana holds no pipeline-admin: a revoke is measured against the account it replaces, not
    // the grants it leaves behind (PF-52, T-5016).
    let (status, body) = call(
        server::app(state),
        &config,
        Some(person("ana@hel.fi", &[])),
        "POST",
        REVOKE,
        Some(json!({ "index": 1, "role": "pipeline-admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(body.contains("gitea.example.org/pulls/7"), "{body}");
    let put = forge
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.method.as_str() == "PUT")
        .expect("the account was written");
    let sent: Value = serde_json::from_slice(&put.body).expect("json");
    let content = sent["content"].as_str().expect("content");
    use base64::Engine;
    let yaml = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(content)
            .expect("base64"),
    )
    .expect("utf8");
    assert!(
        yaml.contains("ovzdusie") && !yaml.contains("doprava"),
        "{yaml}"
    );
}

const ACCOUNT: &str = "/api/v1/projects/ovzdusie/serviceaccounts/vendor-ci";

fn account_with(roles: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ServiceAccount",
        "metadata": { "name": "vendor-ci", "namespace": "ovzdusie" },
        "spec": {
            "owner": { "user": "jana@hel.fi" },
            "purpose": "proposes pipelines from the vendor's CI",
            "roles": roles,
            "credentials": [{ "kind": "oauth-client", "name": "main" }]
        }
    })
}

/// The resource door (REST, MCP and the assistant propose through it): an update that only drops
/// grants passes PF-52 for an account administrator, one that adds or widens a grant while it
/// drops another is still measured whole, and a person who may not propose the kind is refused
/// before any grant is read (T-5016).
#[tokio::test]
async fn an_update_is_measured_against_the_account_it_replaces() {
    let config = Config::for_tests();
    let ana = person("ana@hel.fi", &[]);
    let kept = json!([{ "role": "pipeline-admin", "scope": { "project": "ovzdusie" } }]);
    let (status, body) = call(
        app(&config, two_grants()),
        &config,
        Some(ana.clone()),
        "PUT",
        ACCOUNT,
        Some(account_with(kept.clone())),
    )
    .await;
    assert!(
        status != StatusCode::FORBIDDEN && !body.contains("PF-52"),
        "{status} {body}"
    );

    // The expired doprava grant comes back without its window while ovzdusie goes.
    let widened = json!([{ "role": "pipeline-admin", "scope": { "project": "doprava" } }]);
    // Both grants stay and a third one comes.
    let added = json!([
        { "role": "pipeline-admin", "scope": { "project": "ovzdusie" } },
        { "role": "pipeline-admin", "scope": { "project": "doprava" },
          "validity": { "notAfter": "2020-01-01T00:00:00Z" } },
        { "role": "pipeline-admin", "scope": { "project": "zdravie" } }
    ]);
    for roles in [widened, added] {
        let (status, body) = call(
            app(&config, two_grants()),
            &config,
            Some(ana.clone()),
            "PUT",
            ACCOUNT,
            Some(account_with(roles)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.contains("PF-52"), "{body}");
    }

    let (status, body) = call(
        app(&config, two_grants()),
        &config,
        Some(person("eva@hel.fi", &[])),
        "PUT",
        ACCOUNT,
        Some(account_with(kept)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(!body.contains("PF-52"), "{body}");
}
