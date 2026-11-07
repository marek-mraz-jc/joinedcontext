//! Issuing a ServiceAccount's client secret once (PF-37, API/01 §52, T-5626): who may, what the
//! door refuses before Keycloak, and that the secret is answered once and read nowhere again.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::reconciler::workload_clients::{desired_external, ClientCredential};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "account-secret-csrf";
const REALM: &str = "/admin/realms/bb";

fn secret_of(account: &str) -> String {
    format!("/api/v1/projects/ovzdusie/serviceaccounts/{account}/secret")
}

fn person(email: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: Vec::new(),
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

fn account(name: &str, credentials: Value, workload: Option<Value>) -> ResourceEnvelope {
    let mut spec = json!({
        "owner": { "user": "jana@hel.fi" },
        "purpose": "the vendor's air-quality feed",
        "roles": [{ "role": "space-writer", "scope": { "contextSpace": "air" } }],
        "credentials": credentials,
    });
    if let Some(workload) = workload {
        spec["workload"] = workload;
    }
    envelope("ServiceAccount", name, "ovzdusie", spec)
}

/// A Portal whose realm is `keycloak`, with readers, proposers and approvers of ServiceAccounts.
fn state(keycloak: &MockServer) -> (Config, AppState) {
    let issuer = format!("{}/realms/bb", keycloak.uri());
    let mut config = Config::for_tests();
    config.oidc = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer.clone()),
        "JC_PORTAL_COOKIE_KEY" => {
            Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
        }
        "JC_OIDC_CLIENT_ID" => Some("portal-api".to_owned()),
        "JC_OIDC_CLIENT_SECRET" => Some("portal-api-credential".to_owned()),
        _ => None,
    })
    .expect("config")
    .oidc;
    let state = AppState::new(config.clone(), None);
    let role = |name: &str, verbs: &[&str]| {
        envelope(
            "Role",
            name,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["ServiceAccount"], "verbs": verbs }] }),
        )
    };
    let bound = |name: &str, role: &str, email: &str| {
        envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": email }], "role": role, "scope": { "project": "ovzdusie" } }),
        )
    };
    let key = json!({ "kty": "EC", "crv": "P-256", "kid": "k1",
        "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
        "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" });
    for env in [
        role("account-reader", &["read"]),
        role("account-proposer", &["propose"]),
        role("account-approver", &["read", "approve"]),
        bound("eva-reads", "account-reader", "eva@hel.fi"),
        bound("pia-proposes", "account-proposer", "pia@hel.fi"),
        bound("ana-approves", "account-approver", "ana@hel.fi"),
        account(
            "vendor",
            json!([{ "kind": "oauth-client", "name": "main" }]),
            None,
        ),
        account(
            "sensor",
            json!([{ "kind": "oauth-client", "name": "device-key", "jwks": { "keys": [key] } }]),
            None,
        ),
        account(
            "adapter",
            json!([]),
            Some(json!({ "kubernetes": { "namespace": "env", "serviceAccount": "adapter" } })),
        ),
        account(
            "platform",
            json!([{ "kind": "oauth-client", "name": "main" }]),
            None,
        ),
    ] {
        state.mirror.upsert(env);
    }
    (config, state)
}

/// The realm: the Portal's token, the managed client of `vendor`, the platform's own client of
/// `platform`, and a new secret per regeneration.
async fn realm() -> MockServer {
    let keycloak = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "admin-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    let mut vendor = desired_external("ovzdusie", "vendor", &ClientCredential::Secret);
    vendor["id"] = json!("uuid-vendor");
    for (id, found) in [
        ("ovzdusie-vendor", json!([vendor])),
        (
            "ovzdusie-platform",
            json!([{ "id": "uuid-hand", "clientId": "ovzdusie-platform", "attributes": {} }]),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{REALM}/clients")))
            .and(query_param("clientId", id))
            .respond_with(ResponseTemplate::new(200).set_body_json(found))
            .mount(&keycloak)
            .await;
    }
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients/uuid-vendor/client-secret")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "type": "secret", "value": "first-new-secret" })),
        )
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients/uuid-vendor/client-secret")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "type": "secret", "value": "second-new-secret" })),
        )
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM}/clients/uuid-vendor/service-account-user"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "id": "sa-user-vendor", "username": "service-account-ovzdusie-vendor" }),
        ))
        .mount(&keycloak)
        .await;
    keycloak
}

async fn call(
    state: &AppState,
    config: &Config,
    who: Option<Identity>,
    method: &str,
    uri: &str,
) -> (StatusCode, Option<String>, String) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(who) = who {
        request = request
            .header(header::COOKIE, cookies(config, who))
            .header(CSRF_HEADER, CSRF);
    }
    let response = server::app(state.clone())
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, cache, String::from_utf8_lossy(&bytes).into_owned())
}

async fn secret_calls(keycloak: &MockServer) -> usize {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().ends_with("/client-secret"))
        .count()
}

#[tokio::test]
async fn nobody_signed_in_a_reader_or_a_proposer_issues_nothing() {
    let keycloak = realm().await;
    let (config, state) = state(&keycloak);
    let (status, _, _) = call(&state, &config, None, "POST", &secret_of("vendor")).await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{status}"
    );
    for who in ["eva@hel.fi", "pia@hel.fi", "someone@else.fi"] {
        let (status, _, body) = call(
            &state,
            &config,
            Some(person(who)),
            "POST",
            &secret_of("vendor"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
        assert!(!body.contains("new-secret"), "{body}");
    }
    assert_eq!(secret_calls(&keycloak).await, 0, "Keycloak is never asked");
}

#[tokio::test]
async fn an_approver_gets_the_secret_once_and_no_read_shows_it_again() {
    let keycloak = realm().await;
    let (config, state) = state(&keycloak);
    let ana = || Some(person("ana@hel.fi"));

    let (status, cache, body) = call(&state, &config, ana(), "POST", &secret_of("vendor")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(cache.as_deref(), Some("no-store"));
    let issued: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(issued["clientId"], "ovzdusie-vendor");
    assert_eq!(issued["clientSecret"], "first-new-secret");
    assert_eq!(
        issued["tokenUrl"],
        format!("{}/realms/bb/protocol/openid-connect/token", keycloak.uri())
    );

    // The tokens the account held are refused from now on (T-5568).
    let marks = state.revocations(session::now_unix());
    assert!(
        marks.iter().any(|(subject, _)| subject == "sa-user-vendor"),
        "{marks:?}"
    );
    // Who did it, without the secret.
    let page = state
        .activity
        .list(
            "ovzdusie",
            &ActivityFilter {
                limit: 10,
                ..ActivityFilter::default()
            },
        )
        .await
        .expect("activity");
    let event = page
        .items
        .iter()
        .find(|e| e.kind == "access.revoked")
        .expect("an access.revoked event");
    assert_eq!(event.details["account"], "vendor");
    assert!(
        !serde_json::to_string(event)
            .expect("json")
            .contains("first-new-secret"),
        "{event:?}"
    );

    // A second read of the account never answers the secret; rotating answers a new one.
    let (status, _, body) = call(
        &state,
        &config,
        ana(),
        "GET",
        "/api/v1/projects/ovzdusie/serviceaccounts/vendor/access",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains("first-new-secret"), "{body}");
    let access: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(access["noSecret"], Value::Null);
    assert_eq!(access["mayIssueSecret"], true);
    let (status, _, body) = call(&state, &config, ana(), "POST", &secret_of("vendor")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("second-new-secret"), "{body}");
    assert_eq!(secret_calls(&keycloak).await, 2);
}

#[tokio::test]
async fn an_account_without_a_secret_or_with_the_platforms_client_is_refused_by_name() {
    let keycloak = realm().await;
    let (config, state) = state(&keycloak);
    for (account, says) in [
        ("sensor", "key pair"),
        ("adapter", "workload"),
        ("platform", "runbook 4"),
    ] {
        let (status, _, body) = call(
            &state,
            &config,
            Some(person("ana@hel.fi")),
            "POST",
            &secret_of(account),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{account}: {body}");
        assert!(body.contains(says), "{account}: {body}");
    }
    let (status, _, _) = call(
        &state,
        &config,
        Some(person("ana@hel.fi")),
        "POST",
        &secret_of("nobody"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, body) = call(
        &state,
        &config,
        Some(person("ana@hel.fi")),
        "GET",
        "/api/v1/projects/ovzdusie/serviceaccounts/sensor/access",
    )
    .await;
    let access: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(access["noSecret"], "key-pair", "{access}");
    assert_eq!(secret_calls(&keycloak).await, 0);
}
