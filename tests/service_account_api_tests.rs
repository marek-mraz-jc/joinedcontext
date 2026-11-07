//! ServiceAccounts through the real router (T-0189, PF-36, PF-40, PF-104).
//!
//! A ServiceAccount authenticates with its Keycloak client and nothing else (owner 2026-10-18,
//! T-4362): the Portal mints, rotates and revokes no key, so the key routes and the operations
//! the assistant and MCP reached are gone, for the account's owner as for anyone (T-4616).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "csrf-token-value";
const OWNER: &str = "jana.kovacova";

fn session_cookie(config: &Config, username: &str, roles: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@banskabystrica.sk")),
            name: None,
            roles: roles.iter().map(|role| role.to_string()).collect(),
            groups: Vec::new(),
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
    parts.push(format!("__Host-jc_csrf={CSRF}"));
    parts.join("; ")
}

fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "ServiceAccount".into(),
        metadata: ObjectMeta {
            name: "vendorx-parking-push".into(),
            namespace: Some("banskabystrica".into()),
            ..Default::default()
        },
        spec: json!({
            "owner": { "user": OWNER },
            "purpose": "VendorX pushes ParkingSpot updates every 10 s",
            "roles": [],
            "credentials": [{ "kind": "oauth-client", "name": "main" }]
        }),
        status: None,
    });
    // The owner still works in the project: one binding that lets them read it.
    for (kind, name, spec) in [
        (
            "Role",
            "project-reader",
            json!({ "rules": [{ "kinds": ["ServiceAccount"], "verbs": ["read"] }] }),
        ),
        (
            "RoleBinding",
            "owner-reads-banskabystrica",
            json!({
                "subjects": [{ "user": OWNER }],
                "role": "project-reader",
                "scope": { "project": "banskabystrica" },
            }),
        ),
    ] {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some(ORG_NAMESPACE.into()),
                ..Default::default()
            },
            spec,
            status: None,
        });
    }
    mirror
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF);
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const ACCOUNT: &str = "/api/v1/projects/banskabystrica/serviceaccounts/vendorx-parking-push";

/// T-4616: the account's own owner finds no key route: listing, minting, rotating, revoking and
/// opening a claim all answer 404, and the account itself still reads.
#[tokio::test]
async fn the_owner_finds_no_key_route_of_the_account() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None).with_mirror(mirror()));
    let cookie = session_cookie(&config, OWNER, &[]);
    for (method, path, body) in [
        (Method::GET, format!("{ACCOUNT}/keys"), None),
        (
            Method::POST,
            format!("{ACCOUNT}/keys"),
            Some(json!({ "credential": "main" })),
        ),
        (
            Method::POST,
            format!("{ACCOUNT}/keys/0123456789abcdef/rotate"),
            Some(json!({})),
        ),
        (
            Method::DELETE,
            format!("{ACCOUNT}/keys/0123456789abcdef"),
            None,
        ),
        (
            Method::GET,
            format!("{ACCOUNT}/keys/claims/00112233445566778899aabbccddeeff"),
            None,
        ),
        (
            Method::POST,
            format!("{ACCOUNT}/keys/claims/00112233445566778899aabbccddeeff"),
            None,
        ),
    ] {
        let (status, answer) = call(&app, &cookie, method.clone(), &path, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {answer}");
        assert!(
            !answer.to_string().contains("jc_"),
            "{method} {path}: {answer}"
        );
    }
    let (status, account) = call(&app, &cookie, Method::GET, ACCOUNT, None).await;
    assert_eq!(status, StatusCode::OK, "{account}");
}

/// T-4616: the assistant and MCP reach no key operation either: none is registered.
#[test]
fn no_operation_mints_lists_rotates_or_revokes_a_key() {
    for name in [
        "jc_service_account_key_list",
        "jc_service_account_key_mint",
        "jc_service_account_key_rotate",
        "jc_service_account_key_revoke",
    ] {
        assert!(ops::find(name).is_none(), "{name} is still registered");
    }
}

/// T-1456: `helsinki` + `kpi-writer` and `helsinki-kpi` + `writer` derive one Keycloak client id,
/// which the gateway then resolves to nobody (T-1454). The Check refuses the second account and
/// names the id, not the account or project that holds it; a name that derives a free id passes
/// this rule, and re-proposing the account that holds the id is not a collision with itself.
#[tokio::test]
async fn an_account_whose_client_id_another_project_derives_is_refused_at_check() {
    let forge = common::forge().await;
    let state = common::state_on(&forge);
    let account = |project: &str, name: &str| {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "ServiceAccount",
            "metadata": { "name": name, "namespace": project },
            "spec": {
                "owner": { "user": "demo.steward" },
                "purpose": "writes indicators",
                "roles": [{ "role": "space-writer", "scope": { "project": project } }],
                "credentials": [{ "kind": "oauth-client", "name": "default" }]
            }
        })
    };
    let held = account("helsinki", "kpi-writer");
    state
        .mirror
        .upsert(serde_json::from_value(held.clone()).expect("an envelope"));
    let mut admin = common::person("admin");
    admin.groups = vec!["portal-approver".into()];
    let check = |project: &'static str, body: Value| {
        let state = state.clone();
        let admin = admin.clone();
        async move {
            common::send(
                &state,
                admin,
                "POST",
                &format!("/api/v1/projects/{project}/serviceaccounts?dryRun=All"),
                Some(body),
            )
            .await
        }
    };

    let refused = check("helsinki-kpi", account("helsinki-kpi", "writer")).await;
    // A check that rejects the manifest answers the red verdict that says why (T-2234); PF-59 is
    // the point of the case, and the holder must not be named whatever shape the refusal takes.
    assert_eq!(refused.status, StatusCode::OK, "{}", refused.text);
    assert!(
        refused.text.contains("\"ok\":false"),
        "the check refuses it: {}",
        refused.text
    );
    assert!(
        refused.text.contains("helsinki-kpi-writer"),
        "{}",
        refused.text
    );
    assert!(
        !refused.text.contains("'kpi-writer'") && !refused.text.contains("'helsinki'"),
        "the holder is not named: {}",
        refused.text
    );

    for (project, body) in [
        ("helsinki-kpi", account("helsinki-kpi", "reader")),
        ("helsinki", held),
    ] {
        let answer = check(project, body).await;
        assert!(
            !answer.text.contains("T-1456"),
            "{project}: {}",
            answer.text
        );
    }
}
