#![forbid(unsafe_code)]
//! Every Keycloak-compatible admin route (COMPAT.md) end to end, served with the mount the
//! `jc` router hands over as an extension (W1, W6, W9):
//! - each route answers `401` without a valid bearer token;
//! - an unknown id answers `404` to a realm admin, and another organization's id answers the same
//!   `404` to an organization admin (no existence oracle);
//! - the full lifecycle of people, groups, applications, roles, mappers and scopes;
//! - regressions: an organization admin never moves a group or application to another
//!   organization, never edits or deletes a realm-wide client scope, never unlocks the
//!   brute-force protection of a person outside the organization; a mapper update is checked
//!   like a create; `Allowed` judges a person on the person's own organization.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::{Extension, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_admin::{AdminState, admin_router};
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, sha256_hex, sign_jwt};
use kaisatsu_oidc::{OidcState, OutboundGuard};
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

const R: &str = "/admin/realms/dev";

struct Fixture {
    app: Router,
    store: Arc<MemoryStore>,
    admin: String,
    hel_admin: String,
    kek: Kek,
    revoked: String,
    foreign: String,
}

fn token(kek: &Kek, key: &SigningKeyRecord, claims: Value) -> String {
    sign_jwt(key, kek, Some("JWT"), &claims).expect("sign")
}

fn claims(sub: &str, extra: Value) -> Value {
    let now = Utc::now().timestamp();
    let mut c = json!({
        "iss": "https://idm.localhost/realms/dev",
        "sub": sub,
        "azp": "admin-cli",
        "typ": "Bearer",
        "iat": now,
        "exp": now + 3600,
        "jti": format!("jti-{sub}"),
    });
    for (k, v) in extra.as_object().expect("object") {
        c[k] = v.clone();
    }
    c
}

async fn fixture() -> Fixture {
    let kek = Kek::generate_for_tests();
    let store = Arc::new(MemoryStore::new(kek.clone()));
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            smtp_server: Some(SmtpServerConfig::default()),
            ..Default::default()
        })
        .await
        .unwrap();
    for org in ["helsinki", "espoo"] {
        store
            .upsert_organization(Organization {
                id: org.into(),
                realm_id: "dev".into(),
                name: org.into(),
                alias: org.into(),
                enabled: true,
                description: String::new(),
                domains: Vec::new(),
                attributes: Default::default(),
                managed_by: None,
            })
            .await
            .unwrap();
    }
    store
        .upsert_membership(Membership {
            id: "m-hel-admin".into(),
            realm_id: "dev".into(),
            org_id: "helsinki".into(),
            person_id: "org-admin-hel".into(),
            org_roles: vec!["org-admin".into()],
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    for (id, org) in [
        ("hel-user", Some("helsinki")),
        ("esp-user", Some("espoo")),
        ("realm-user", None),
    ] {
        store
            .create_user(UserRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                org_id: org.map(str::to_owned),
                username: id.into(),
                email: Some(format!("{id}@example.test")),
                enabled: true,
                ..Default::default()
            })
            .await
            .unwrap();
    }
    // A person without a home organization who is a member of Helsinki.
    store
        .upsert_membership(Membership {
            id: "m-realm-user".into(),
            realm_id: "dev".into(),
            org_id: "helsinki".into(),
            person_id: "realm-user".into(),
            org_roles: Vec::new(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    for (id, org, path) in [
        ("hel-group", "helsinki", "/helsinki/members"),
        ("esp-group", "espoo", "/espoo/members"),
    ] {
        store
            .upsert_group(GroupRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                org_id: Some(org.into()),
                name: "members".into(),
                path: path.into(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    for (id, client_id, org) in [
        ("c-hel", "hel-app", Some("helsinki")),
        ("c-esp", "esp-app", Some("espoo")),
        ("c-rm", "realm-management", None),
    ] {
        store
            .upsert_client(ClientRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                org_id: org.map(str::to_owned),
                client_id: client_id.into(),
                enabled: true,
                secret: Some(format!("{client_id}-secret")),
                service_accounts_enabled: true,
                ..Default::default()
            })
            .await
            .unwrap();
    }
    for (id, name, org) in [
        ("sc-roles", "roles", None),
        ("sc-hel", "hel-scope", Some("helsinki")),
    ] {
        store
            .upsert_client_scope(ClientScopeRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                org_id: org.map(str::to_owned),
                name: name.into(),
                protocol: "openid-connect".into(),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    let keys = store.list_signing_keys("dev").await.unwrap();
    let key = &keys[0];
    let admin = token(
        &kek,
        key,
        claims(
            "admin-1",
            json!({"realm_access": {"roles": ["realm-admin"]}}),
        ),
    );
    let hel_admin = token(
        &kek,
        key,
        claims(
            "org-admin-hel",
            json!({"azp": "portal-api", "jc_org": ["helsinki"]}),
        ),
    );
    let revoked = token(
        &kek,
        key,
        claims(
            "admin-2",
            json!({"realm_access": {"roles": ["realm-admin"]}}),
        ),
    );
    store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex("jti-admin-2"),
            realm_id: "dev".into(),
            expires_at: Utc::now() + Duration::hours(1),
        })
        .await
        .unwrap();
    // Signed with a key of another KEK/realm: never verifies here.
    let other_kek = Kek::generate_for_tests();
    let other = MemoryStore::new(other_kek.clone());
    other
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let other_keys = other.list_signing_keys("dev").await.unwrap();
    let foreign = token(
        &other_kek,
        &other_keys[0],
        claims(
            "admin-1",
            json!({"realm_access": {"roles": ["realm-admin"]}}),
        ),
    );

    let oidc_kek = kek.clone();
    let oidc = OidcState {
        store: store.clone(),
        kek,
        guard: OutboundGuard::default(),
        default_mount: Mount::default(),
    };
    let app = admin_router(AdminState::new(oidc, Arc::new(LocalAuthorizer)))
        .layer(Extension(Mount::default()));
    Fixture {
        app,
        store,
        admin,
        hel_admin,
        kek: oidc_kek,
        revoked,
        foreign,
    }
}

async fn call(
    app: &Router,
    method: &Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut b = Request::builder().method(method.clone()).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Every route with ids taken from `ids` (user, group, client uuid, scope, credential, mapper).
fn routes(
    user: &str,
    group: &str,
    client: &str,
    scope: &str,
) -> Vec<(Method, String, Option<Value>)> {
    let roles = json!([{ "name": "viewer" }]);
    let user_body = json!({ "username": "x" });
    let group_body = json!({ "name": "x" });
    let client_body = json!({ "clientId": "x" });
    let mapper = json!({ "name": "m", "protocol": "openid-connect", "protocolMapper": "oidc-hardcoded-claim-mapper" });
    let scope_body = json!({ "name": "x", "protocol": "openid-connect" });
    vec![
        (Method::GET, format!("{R}/users/{user}"), None),
        (Method::PUT, format!("{R}/users/{user}"), Some(user_body)),
        (Method::DELETE, format!("{R}/users/{user}"), None),
        (
            Method::PUT,
            format!("{R}/users/{user}/execute-actions-email"),
            Some(json!(["UPDATE_PASSWORD"])),
        ),
        (
            Method::PUT,
            format!("{R}/users/{user}/reset-password"),
            Some(json!({ "value": "Some-Long-Pass-123!", "temporary": false })),
        ),
        (Method::POST, format!("{R}/users/{user}/logout"), None),
        (Method::GET, format!("{R}/users/{user}/credentials"), None),
        (
            Method::DELETE,
            format!("{R}/users/{user}/credentials/cred-1"),
            None,
        ),
        (Method::GET, format!("{R}/users/{user}/sessions"), None),
        (Method::GET, format!("{R}/users/{user}/consents"), None),
        (
            Method::DELETE,
            format!("{R}/users/{user}/consents/hel-app"),
            None,
        ),
        (Method::GET, format!("{R}/users/{user}/groups"), None),
        (
            Method::PUT,
            format!("{R}/users/{user}/groups/{group}"),
            None,
        ),
        (
            Method::DELETE,
            format!("{R}/users/{user}/groups/{group}"),
            None,
        ),
        (
            Method::GET,
            format!("{R}/users/{user}/role-mappings/realm/composite"),
            None,
        ),
        (
            Method::POST,
            format!("{R}/users/{user}/role-mappings/clients/{client}"),
            Some(roles.clone()),
        ),
        (
            Method::DELETE,
            format!("{R}/users/{user}/role-mappings/clients/{client}"),
            Some(roles.clone()),
        ),
        (Method::GET, format!("{R}/groups/{group}"), None),
        (Method::PUT, format!("{R}/groups/{group}"), Some(group_body)),
        (Method::DELETE, format!("{R}/groups/{group}"), None),
        (Method::GET, format!("{R}/groups/{group}/members"), None),
        (
            Method::POST,
            format!("{R}/groups/{group}/role-mappings/clients/{client}"),
            Some(roles.clone()),
        ),
        (
            Method::DELETE,
            format!("{R}/groups/{group}/role-mappings/clients/{client}"),
            Some(roles),
        ),
        (Method::GET, format!("{R}/clients/{client}"), None),
        (
            Method::PUT,
            format!("{R}/clients/{client}"),
            Some(client_body),
        ),
        (Method::DELETE, format!("{R}/clients/{client}"), None),
        (
            Method::GET,
            format!("{R}/clients/{client}/client-secret"),
            None,
        ),
        (
            Method::POST,
            format!("{R}/clients/{client}/client-secret"),
            None,
        ),
        (
            Method::GET,
            format!("{R}/clients/{client}/service-account-user"),
            None,
        ),
        (Method::GET, format!("{R}/clients/{client}/roles"), None),
        (
            Method::POST,
            format!("{R}/clients/{client}/roles"),
            Some(json!({ "name": "viewer" })),
        ),
        (
            Method::DELETE,
            format!("{R}/clients/{client}/roles/viewer"),
            None,
        ),
        (
            Method::GET,
            format!("{R}/clients/{client}/roles/viewer/groups"),
            None,
        ),
        (
            Method::GET,
            format!("{R}/clients/{client}/roles/viewer/users"),
            None,
        ),
        (
            Method::GET,
            format!("{R}/clients/{client}/protocol-mappers/models"),
            None,
        ),
        (
            Method::POST,
            format!("{R}/clients/{client}/protocol-mappers/models"),
            Some(mapper.clone()),
        ),
        (
            Method::PUT,
            format!("{R}/clients/{client}/protocol-mappers/models/m1"),
            Some(mapper),
        ),
        (
            Method::DELETE,
            format!("{R}/clients/{client}/protocol-mappers/models/m1"),
            None,
        ),
        (
            Method::GET,
            format!("{R}/clients/{client}/default-client-scopes"),
            None,
        ),
        (
            Method::PUT,
            format!("{R}/clients/{client}/default-client-scopes/{scope}"),
            None,
        ),
        (
            Method::DELETE,
            format!("{R}/clients/{client}/default-client-scopes/{scope}"),
            None,
        ),
        (
            Method::GET,
            format!("{R}/clients/{client}/optional-client-scopes"),
            None,
        ),
        (
            Method::PUT,
            format!("{R}/clients/{client}/optional-client-scopes/{scope}"),
            None,
        ),
        (
            Method::DELETE,
            format!("{R}/clients/{client}/optional-client-scopes/{scope}"),
            None,
        ),
        (Method::GET, format!("{R}/client-scopes/{scope}"), None),
        (
            Method::PUT,
            format!("{R}/client-scopes/{scope}"),
            Some(scope_body),
        ),
        (Method::DELETE, format!("{R}/client-scopes/{scope}"), None),
        (
            Method::DELETE,
            format!("{R}/attack-detection/brute-force/users/{user}"),
            None,
        ),
    ]
}

fn collection_routes() -> Vec<(Method, String, Option<Value>)> {
    vec![
        (Method::GET, R.to_owned(), None),
        (Method::PUT, R.to_owned(), Some(json!({ "realm": "dev" }))),
        (
            Method::POST,
            "/admin/realms".to_owned(),
            Some(json!({ "realm": "dev2" })),
        ),
        (
            Method::POST,
            format!("{R}/partialImport"),
            Some(json!({ "users": [] })),
        ),
        (Method::GET, format!("{R}/users"), None),
        (
            Method::POST,
            format!("{R}/users"),
            Some(json!({ "username": "x" })),
        ),
        (Method::GET, format!("{R}/groups"), None),
        (
            Method::POST,
            format!("{R}/groups"),
            Some(json!({ "name": "x" })),
        ),
        (Method::GET, format!("{R}/clients"), None),
        (
            Method::POST,
            format!("{R}/clients"),
            Some(json!({ "clientId": "x" })),
        ),
        (Method::GET, format!("{R}/client-scopes"), None),
        (
            Method::POST,
            format!("{R}/client-scopes"),
            Some(json!({ "name": "x", "protocol": "openid-connect" })),
        ),
        (
            Method::GET,
            "/api/v1/realms/dev/allowed?op=read".to_owned(),
            None,
        ),
        (
            Method::POST,
            "/api/v1/realms/dev/keys/rotate".to_owned(),
            Some(json!({})),
        ),
    ]
}

#[tokio::test]
async fn every_route_refuses_missing_forged_and_revoked_tokens() {
    let f = fixture().await;
    let mut all = routes("hel-user", "hel-group", "c-hel", "sc-hel");
    all.extend(collection_routes());
    for (method, uri, body) in &all {
        for tok in [
            None,
            Some("not-a-jwt"),
            Some(f.foreign.as_str()),
            Some(f.revoked.as_str()),
        ] {
            let (st, _, _) = call(&f.app, method, uri, tok, body.as_ref()).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{method} {uri} with {tok:?}");
        }
    }
    // Nothing was written.
    assert!(
        f.store
            .get_user_by_id("dev", None, "hel-user")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn unknown_ids_answer_404_to_a_realm_admin() {
    let f = fixture().await;
    for (method, uri, body) in routes("nobody", "no-group", "no-client", "no-scope") {
        // Scope links on an existing client with an unknown scope id are idempotent (204).
        if uri.contains("client-scopes/no-scope") && uri.contains("/clients/") {
            continue;
        }
        let (st, _, ans) = call(&f.app, &method, &uri, Some(&f.admin), body.as_ref()).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {uri}: {ans}");
    }
    // Existing user, unknown group / credential; existing client, unknown mapper on delete.
    for (method, uri) in [
        (Method::PUT, format!("{R}/users/hel-user/groups/no-group")),
        (
            Method::DELETE,
            format!("{R}/users/hel-user/groups/no-group"),
        ),
        (
            Method::DELETE,
            format!("{R}/users/hel-user/credentials/no-cred"),
        ),
    ] {
        let (st, _, _) = call(&f.app, &method, &uri, Some(&f.admin), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    let (st, _, _) = call(
        &f.app,
        &Method::GET,
        "/admin/realms/nope",
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "no keys, no token for that realm"
    );
}

#[tokio::test]
async fn another_organizations_objects_are_404_to_an_org_admin() {
    let f = fixture().await;
    for (method, uri, body) in routes("esp-user", "esp-group", "c-esp", "sc-roles") {
        let (st, _, ans) = call(&f.app, &method, &uri, Some(&f.hel_admin), body.as_ref()).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {uri}: {ans}");
    }
    // Nothing of Espoo changed.
    let esp = f
        .store
        .get_client_by_uuid("dev", None, "c-esp")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(esp.secret.as_deref(), Some("esp-app-secret"));
    assert!(
        f.store
            .get_user_by_id("dev", None, "esp-user")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        f.store
            .get_client_scope("dev", "sc-roles")
            .await
            .unwrap()
            .is_some()
    );
    // The realm itself is realm-admin business.
    for (method, uri, body) in [
        (Method::GET, R.to_owned(), None),
        (Method::PUT, R.to_owned(), Some(json!({ "realm": "dev" }))),
        (Method::POST, format!("{R}/partialImport"), Some(json!({}))),
        (
            Method::POST,
            "/api/v1/realms/dev/keys/rotate".to_owned(),
            Some(json!({})),
        ),
    ] {
        // The realm object is realm-level: hidden from an organization caller.
        let (st, _, _) = call(&f.app, &method, &uri, Some(&f.hel_admin), body.as_ref()).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

#[tokio::test]
async fn people_lifecycle_through_the_compat_routes() {
    let f = fixture().await;
    let a = Some(f.admin.as_str());
    let (st, h, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/users"),
        a,
        Some(&json!({ "username": "new.person", "email": "new.person@example.test", "enabled": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let loc = h[header::LOCATION].to_str().unwrap().to_owned();
    assert!(
        loc.starts_with("https://idm.localhost/admin/realms/dev/users/"),
        "{loc}"
    );
    let id = loc.rsplit('/').next().unwrap().to_owned();
    let u = format!("{R}/users/{id}");

    let (st, _, list) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/users?search=new.person&first=0&max=5"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (st, _, list) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/users?username=new.person&exact=true"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list[0]["id"], Value::String(id.clone()));
    let (st, _, got) = call(&f.app, &Method::GET, &u, a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["username"], "new.person");
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &u,
        a,
        Some(&json!({ "username": "new.person", "firstName": "New", "orgId": "espoo", "realmRoles": ["realm-admin"] })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let stored = f
        .store
        .get_user_by_id("dev", None, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.first_name.as_deref(), Some("New"));
    assert_eq!(
        stored.org_id.as_deref(),
        Some("espoo"),
        "a realm admin moves a person"
    );
    // The realm's default roles from the create, and no `realm-admin` from the PUT body.
    assert_eq!(
        stored.realm_roles,
        ["offline_access", "uma_authorization"],
        "role mappings are not written by PUT"
    );

    // Password, required actions, credentials.
    let (st, _, ans) = call(
        &f.app,
        &Method::PUT,
        &format!("{u}/reset-password"),
        a,
        Some(&json!({ "value": "Fresh-Pass-1234!", "temporary": true })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{ans}");
    let (st, _, creds) = call(&f.app, &Method::GET, &format!("{u}/credentials"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    let creds = creds.as_array().unwrap().clone();
    assert_eq!(creds.len(), 1);
    assert!(
        !creds[0].to_string().contains("argon2"),
        "no hash leaves the API: {}",
        creds[0]
    );
    let cred_id = creds[0]["id"].as_str().unwrap().to_owned();
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{u}/execute-actions-email"),
        a,
        Some(&json!(["UPDATE_PASSWORD", "VERIFY_EMAIL"])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let stored = f
        .store
        .get_user_by_id("dev", None, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.required_actions.contains(&"VERIFY_EMAIL".to_owned()));
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{u}/credentials/{cred_id}"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{u}/credentials/{cred_id}"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Sessions, consents, logout.
    f.store
        .upsert_sso_session(SsoSession {
            id: "sso-new".into(),
            realm_id: "dev".into(),
            org_id: None,
            handle_hash: "h".into(),
            user_id: id.clone(),
            username: "new.person".into(),
            ip_address: "192.0.2.1".into(),
            start: 1,
            last_access: 1,
            auth_time: 1,
            acr: "1".into(),
            amr: vec!["pwd".into()],
            clients: HashMap::new(),
            idle_expires_at: Utc::now() + Duration::hours(1),
            max_expires_at: Utc::now() + Duration::hours(10),
        })
        .await
        .unwrap();
    f.store
        .upsert_consent(ConsentRecord {
            id: String::new(),
            realm_id: "dev".into(),
            org_id: None,
            user_id: id.clone(),
            client_id: "hel-app".into(),
            granted_client_scopes: vec!["profile".into()],
            created_date: 1,
            last_updated_date: 1,
        })
        .await
        .unwrap();
    let (st, _, sessions) = call(&f.app, &Method::GET, &format!("{u}/sessions"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert!(!sessions.to_string().contains("handleHash"), "{sessions}");
    let (st, _, consents) = call(&f.app, &Method::GET, &format!("{u}/consents"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(consents.as_array().unwrap().len(), 1);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{u}/consents/hel-app"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(f.store.list_consents("dev", &id).await.unwrap().is_empty());
    let (st, _, _) = call(&f.app, &Method::POST, &format!("{u}/logout"), a, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        f.store
            .list_user_sso_sessions("dev", &id)
            .await
            .unwrap()
            .is_empty()
    );

    // Groups and effective realm roles.
    f.store
        .upsert_role(RoleRepresentation {
            id: "r-reader".into(),
            realm_id: "dev".into(),
            name: "reader".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut group = f
        .store
        .get_group("dev", None, "esp-group")
        .await
        .unwrap()
        .unwrap();
    group.realm_roles = vec!["reader".into(), "ghost-role".into()];
    f.store.upsert_group(group).await.unwrap();
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{u}/groups/esp-group"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, groups) = call(&f.app, &Method::GET, &format!("{u}/groups"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(groups[0]["id"], "esp-group");
    let (st, _, roles) = call(
        &f.app,
        &Method::GET,
        &format!("{u}/role-mappings/realm/composite"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let names: Vec<&str> = roles
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert!(names.contains(&"reader"), "{names:?}");
    assert!(
        names.contains(&"ghost-role"),
        "a role name without a role object is still listed"
    );
    let (st, _, members) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/groups/esp-group/members?max=10"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(members.as_array().unwrap().len(), 1);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{u}/groups/esp-group"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        f.store
            .list_user_groups("dev", &id)
            .await
            .unwrap()
            .is_empty()
    );

    // Client role mappings by id or by name.
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{u}/role-mappings/clients/c-hel"),
        a,
        Some(&json!([{ "name": "viewer" }, { "id": "r-edit", "name": "editor" }])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let holders = f
        .store
        .list_principals_with_client_role("dev", "c-hel", "editor", "user")
        .await
        .unwrap();
    assert_eq!(holders, std::slice::from_ref(&id));
    let (st, _, users) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/clients/c-hel/roles/viewer/users"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(users[0]["id"], Value::String(id.clone()));
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{u}/role-mappings/clients/c-hel"),
        a,
        Some(&json!([{ "name": "viewer" }, { "name": "editor" }])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        f.store
            .list_principals_with_client_role("dev", "c-hel", "editor", "user")
            .await
            .unwrap()
            .is_empty()
    );

    // Unlock, then delete.
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/attack-detection/brute-force/users/{id}"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = call(&f.app, &Method::DELETE, &u, a, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = call(&f.app, &Method::GET, &u, a, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let events = f.store.list_admin_events("dev", None).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.operation_type == "DELETE" && e.resource_type == "USER")
    );
    assert!(
        events.iter().all(|e| !e
            .representation
            .as_ref()
            .is_some_and(|r| r.to_string().contains("Fresh-Pass"))),
        "admin events never carry a password"
    );
}

#[tokio::test]
async fn groups_clients_roles_mappers_and_scopes_lifecycle() {
    let f = fixture().await;
    let a = Some(f.admin.as_str());

    // Groups.
    let (st, h, _) = call(&f.app, &Method::POST, &format!("{R}/groups"), a, Some(&json!({ "name": "editors", "path": "/editors", "attributes": { "managed-by": ["joinedcontext"] } }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let gid = h[header::LOCATION]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let g = format!("{R}/groups/{gid}");
    let (st, _, list) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/groups?search=editors&exact=false"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == Value::String(gid.clone()))
    );
    let (st, _, got) = call(&f.app, &Method::GET, &g, a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["managedBy"], "joinedcontext");
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &g,
        a,
        Some(&json!({ "name": "editors2", "attributes": { "managed-by": ["portal"] } })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let stored = f.store.get_group("dev", None, &gid).await.unwrap().unwrap();
    assert_eq!(stored.name, "editors2");
    assert_eq!(
        stored.path, "/editors",
        "an empty path keeps the stored one"
    );
    assert_eq!(stored.managed_by.as_deref(), Some("portal"));
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{g}/role-mappings/clients/c-hel"),
        a,
        Some(&json!([{ "name": "viewer" }])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, groups) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/clients/c-hel/roles/viewer/groups"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(groups[0]["id"], Value::String(gid.clone()));
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{g}/role-mappings/clients/c-hel"),
        a,
        Some(&json!([{ "name": "viewer" }])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = call(&f.app, &Method::DELETE, &g, a, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        f.store
            .get_group("dev", None, &gid)
            .await
            .unwrap()
            .is_none()
    );

    // Applications.
    let (st, h, _) = call(&f.app, &Method::POST, &format!("{R}/clients"), a, Some(&json!({ "clientId": "new-app", "secret": "initial", "defaultClientScopes": ["roles"], "serviceAccountsEnabled": false }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = h[header::LOCATION]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let c = format!("{R}/clients/{cid}");
    let (st, _, list) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/clients?clientId=new-app&search=false"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (st, _, got) = call(&f.app, &Method::GET, &c, a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["clientId"], "new-app");
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &c,
        a,
        Some(&json!({ "clientId": "new-app", "name": "Renamed" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let stored = f
        .store
        .get_client_by_uuid("dev", None, &cid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.default_client_scopes,
        ["roles"],
        "omitted default scopes are kept"
    );
    let (st, _, sec) = call(&f.app, &Method::GET, &format!("{c}/client-secret"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sec["type"], "secret");
    let (st, _, fresh) = call(
        &f.app,
        &Method::POST,
        &format!("{c}/client-secret"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let fresh = fresh["value"].as_str().unwrap().to_owned();
    assert_eq!(fresh.len(), 43);
    let (st, _, sec) = call(&f.app, &Method::GET, &format!("{c}/client-secret"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sec["value"], Value::String(fresh));
    let (st, _, _) = call(
        &f.app,
        &Method::GET,
        &format!("{c}/service-account-user"),
        a,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "no service-account person exists for new-app"
    );
    let (st, _, sa) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/clients/c-hel/service-account-user"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sa["username"], "service-account-hel-app");

    // Client roles.
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{c}/roles"),
        a,
        Some(&json!({ "name": "viewer", "description": "reads" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _, roles) = call(&f.app, &Method::GET, &format!("{c}/roles"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(roles[0]["name"], "viewer");
    assert_eq!(roles[0]["clientRole"], true);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{c}/roles/viewer"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, roles) = call(&f.app, &Method::GET, &format!("{c}/roles"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(roles.as_array().unwrap().is_empty());

    // Protocol mappers: create, update, refuse unknown kinds, delete.
    let mapper = json!({ "name": "tenant", "protocol": "openid-connect", "protocolMapper": "oidc-hardcoded-claim-mapper", "config": { "claim.name": "tenant", "claim.value": "x" } });
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{c}/protocol-mappers/models"),
        a,
        Some(&mapper),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _, mappers) = call(
        &f.app,
        &Method::GET,
        &format!("{c}/protocol-mappers/models"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let mid = mappers[0]["id"].as_str().unwrap().to_owned();
    let mut renamed = mapper.clone();
    renamed["name"] = json!("tenant2");
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{c}/protocol-mappers/models/{mid}"),
        a,
        Some(&renamed),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let stored = f
        .store
        .get_client_by_uuid("dev", None, &cid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.protocol_mappers.len(), 1);
    assert_eq!(stored.protocol_mappers[0].name, "tenant2");
    let mut unknown = mapper.clone();
    unknown["protocolMapper"] = json!("script-mapper");
    for (method, uri) in [
        (Method::POST, format!("{c}/protocol-mappers/models")),
        (Method::PUT, format!("{c}/protocol-mappers/models/{mid}")),
    ] {
        let (st, _, ans) = call(&f.app, &method, &uri, a, Some(&unknown)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{method} {uri}: {ans}");
    }
    let stored = f
        .store
        .get_client_by_uuid("dev", None, &cid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.protocol_mappers[0].protocol_mapper,
        "oidc-hardcoded-claim-mapper"
    );
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{c}/protocol-mappers/models/{mid}"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let stored = f
        .store
        .get_client_by_uuid("dev", None, &cid)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.protocol_mappers.is_empty());

    // Client scopes and links (by id, by name, unknown ids kept as names).
    let (st, h, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/client-scopes"),
        a,
        Some(&json!({ "name": "endpoint:x", "protocol": "openid-connect" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = h[header::LOCATION]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let (st, _, got) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/client-scopes/{sid}"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["name"], "endpoint:x");
    let (st, _, all) = call(&f.app, &Method::GET, &format!("{R}/client-scopes"), a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(all.as_array().unwrap().len() >= 3);
    for kind in ["default", "optional"] {
        let (st, _, _) = call(
            &f.app,
            &Method::PUT,
            &format!("{c}/{kind}-client-scopes/{sid}"),
            a,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _, _) = call(
            &f.app,
            &Method::PUT,
            &format!("{c}/{kind}-client-scopes/{sid}"),
            a,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT, "linking twice is idempotent");
        let (st, _, _) = call(
            &f.app,
            &Method::PUT,
            &format!("{c}/{kind}-client-scopes/unknown-scope"),
            a,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _, linked) = call(
            &f.app,
            &Method::GET,
            &format!("{c}/{kind}-client-scopes"),
            a,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let names: Vec<&str> = linked
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect();
        assert!(names.contains(&"endpoint:x"), "{kind}: {names:?}");
        assert!(names.contains(&"unknown-scope"), "{kind}: {names:?}");
        for id in [sid.as_str(), "unknown-scope"] {
            let (st, _, _) = call(
                &f.app,
                &Method::DELETE,
                &format!("{c}/{kind}-client-scopes/{id}"),
                a,
                None,
            )
            .await;
            assert_eq!(st, StatusCode::NO_CONTENT);
        }
    }
    let stored = f
        .store
        .get_client_by_uuid("dev", None, &cid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.default_client_scopes, ["roles"]);
    assert!(stored.optional_client_scopes.is_empty());
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/client-scopes/{sid}"),
        a,
        Some(&json!({ "name": "endpoint:y", "protocol": "openid-connect" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        f.store
            .get_client_scope("dev", &sid)
            .await
            .unwrap()
            .unwrap()
            .name,
        "endpoint:y"
    );
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/client-scopes/{sid}"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/client-scopes/{sid}"),
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _, _) = call(&f.app, &Method::DELETE, &c, a, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        f.store
            .get_client_by_uuid("dev", None, &cid)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn realm_partial_import_and_native_routes() {
    let f = fixture().await;
    let a = Some(f.admin.as_str());
    let (st, _, realm) = call(&f.app, &Method::GET, R, a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(realm["realm"], "dev");
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        R,
        a,
        Some(&json!({ "realm": "ignored", "displayName": "Dev realm" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let stored = f.store.get_realm("dev").await.unwrap().unwrap();
    assert_eq!(stored.display_name, "Dev realm");
    assert_eq!(
        stored.realm, "dev",
        "the path names the realm, not the body"
    );
    // A realm token never creates another realm, and an existing one conflicts.
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        "/admin/realms",
        a,
        Some(&json!({ "realm": "dev" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(f.store.get_realm("joinedcontext").await.unwrap().is_none());
    // A master admin creates a realm from `{"realm": ...}` alone; its id is its name.
    f.store
        .upsert_realm(RealmConfig {
            id: "master".into(),
            realm: "master".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let master_keys = f.store.list_signing_keys("master").await.unwrap();
    let mut master_claims = claims("root", json!({"realm_access": {"roles": ["admin"]}}));
    master_claims["iss"] = json!("https://idm.localhost/realms/master");
    let master = token(f.store.kek(), &master_keys[0], master_claims);
    let (st, _, ans) = call(
        &f.app,
        &Method::POST,
        "/admin/realms",
        Some(&master),
        Some(&json!({ "realm": "stage2", "displayName": "Stage 2" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{ans}");
    let stage = f
        .store
        .get_realm("stage2")
        .await
        .unwrap()
        .expect("realm stage2");
    assert_eq!(stage.id, "stage2");
    assert!(f.store.get_realm("joinedcontext").await.unwrap().is_none());
    for bad in [json!({ "displayName": "no name" }), json!({ "realm": 7 })] {
        let (st, _, _) = call(
            &f.app,
            &Method::POST,
            "/admin/realms",
            Some(&master),
            Some(&bad),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }

    let users = json!([
        { "id": "pi-1", "username": "pi.one" },
        { "username": "hel-user" }
    ]);
    let (st, _, ans) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/partialImport"),
        a,
        Some(&json!({ "ifResourceExists": "FAIL", "users": users })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{ans}");
    assert!(
        f.store
            .get_user_by_id("dev", None, "pi-1")
            .await
            .unwrap()
            .is_none(),
        "FAIL writes nothing when any user conflicts"
    );
    let (st, _, ans) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/partialImport"),
        a,
        Some(&json!({
            "ifResourceExists": "skip",
            "users": users,
            "groups": [{ "id": "pi-group", "name": "imported", "path": "/imported" }],
            "clients": [{ "id": "pi-client", "clientId": "imported-app" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ans}");
    assert_eq!(ans["added"], 3);
    assert_eq!(ans["skipped"], 1);
    let (st, _, ans) = call(&f.app, &Method::POST, &format!("{R}/partialImport"), a, Some(&json!({ "ifResourceExists": "OVERWRITE", "users": [{ "id": "pi-1", "username": "pi.one", "firstName": "Again" }] }))).await;
    assert_eq!(st, StatusCode::OK, "{ans}");
    assert_eq!(ans["overwritten"], 1);
    let pi = f
        .store
        .get_user_by_id("dev", None, "pi-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pi.first_name.as_deref(),
        Some("Again"),
        "the fixed id survives (PF-126)"
    );
    let (st, _, _) = call(&f.app, &Method::POST, &format!("{R}/partialImport"), a, Some(&json!({ "ifResourceExists": "MERGE", "users": [{ "id": "pi-1", "username": "pi.one" }] }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _, ops) = call(
        &f.app,
        &Method::GET,
        "/api/v1/realms/dev/operations",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        ops.as_array()
            .unwrap()
            .iter()
            .any(|o| o["name"] == "keys_rotate")
    );
    let (st, _, ans) = call(
        &f.app,
        &Method::GET,
        "/api/v1/realms/dev/allowed?op=update&kind=Person&id=hel-user",
        a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ans["allowed"], true);
    for (kind, id) in [("Group", "hel-group"), ("Application", "c-hel")] {
        let (st, _, ans) = call(
            &f.app,
            &Method::GET,
            &format!("/api/v1/realms/dev/allowed?op=delete&kind={kind}&id={id}"),
            Some(&f.hel_admin),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ans["allowed"], true, "{kind}: {ans}");
        let (st, _, ans) = call(
            &f.app,
            &Method::GET,
            &format!("/api/v1/realms/dev/allowed?op=delete&kind={kind}&id=it's-missing"),
            Some(&f.hel_admin),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ans["notFound"], true, "{kind}: {ans}");
    }
    let (st, _, ans) = call(
        &f.app,
        &Method::GET,
        "/api/v1/realms/dev/allowed?kind=Realm&op=apply",
        Some(&f.hel_admin),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ans["allowed"], false);
    assert_eq!(
        ans["notFound"], true,
        "the realm is hidden from an organization caller, as in the call"
    );

    let before = f.store.list_signing_keys("dev").await.unwrap().len();
    let (st, _, key) = call(
        &f.app,
        &Method::POST,
        "/api/v1/realms/dev/keys/rotate",
        a,
        Some(&json!({ "algorithm": "RS256", "immediate": false })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{key}");
    assert_eq!(key["algorithm"], "RS256");
    assert!(key.get("encryptedPrivateKey").is_none());
    assert_eq!(
        f.store.list_signing_keys("dev").await.unwrap().len(),
        before + 1
    );
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        "/api/v1/realms/dev/keys/rotate",
        a,
        Some(&json!({ "algorithm": "HS256" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "no symmetric token keys");
}

#[tokio::test]
async fn an_org_admin_works_inside_its_organization_only() {
    let f = fixture().await;
    let h = Some(f.hel_admin.as_str());

    // Inside Helsinki: list, read, write.
    let (st, _, users) = call(&f.app, &Method::GET, &format!("{R}/users"), h, None).await;
    assert_eq!(st, StatusCode::OK);
    let ids: Vec<&str> = users
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|u| u["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"hel-user") && !ids.contains(&"esp-user"),
        "{ids:?}"
    );
    let (st, h2, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/users"),
        h,
        Some(&json!({ "username": "hel.new" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let new_id = h2[header::LOCATION]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let created = f
        .store
        .get_user_by_id("dev", None, &new_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.org_id.as_deref(), Some("helsinki"));
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/users"),
        h,
        Some(&json!({ "username": "esp.new", "orgId": "espoo" })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "creating into another organization is refused"
    );
    let (st, _, groups) = call(&f.app, &Method::GET, &format!("{R}/groups"), h, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        groups
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["orgId"] == "helsinki")
    );
    let (st, _, clients) = call(&f.app, &Method::GET, &format!("{R}/clients"), h, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        clients
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["orgId"] == "helsinki"),
        "{clients}"
    );

    // Regression: no move of a group or application to another organization.
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/groups/hel-group"),
        h,
        Some(&json!({ "name": "members", "orgId": "espoo" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let g = f
        .store
        .get_group("dev", None, "hel-group")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(g.org_id.as_deref(), Some("helsinki"));
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/clients/c-hel"),
        h,
        Some(&json!({ "clientId": "hel-app", "orgId": "espoo" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let c = f
        .store
        .get_client_by_uuid("dev", None, "c-hel")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c.org_id.as_deref(), Some("helsinki"));
    // A realm admin may move them.
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/groups/hel-group"),
        Some(&f.admin),
        Some(&json!({ "name": "members", "orgId": "espoo" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let g = f
        .store
        .get_group("dev", None, "hel-group")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(g.org_id.as_deref(), Some("espoo"));

    // Regression: realm-wide client scopes are out of reach; an own scope is in reach.
    let (st, _, _) = call(&f.app, &Method::PUT, &format!("{R}/client-scopes/sc-roles"), h, Some(&json!({ "name": "roles", "protocol": "openid-connect", "attributes": { "include.in.token.scope": "false" } }))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/client-scopes/sc-roles"),
        h,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let roles = f
        .store
        .get_client_scope("dev", "sc-roles")
        .await
        .unwrap()
        .unwrap();
    assert!(roles.attributes.is_empty());
    let (st, _, _) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/client-scopes/sc-roles"),
        h,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/client-scopes/sc-hel"),
        h,
        Some(&json!({ "name": "hel-scope-2", "protocol": "openid-connect", "orgId": "espoo" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let own = f
        .store
        .get_client_scope("dev", "sc-hel")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(own.name, "hel-scope-2");
    assert_eq!(
        own.org_id.as_deref(),
        Some("helsinki"),
        "no move to another organization"
    );
    let (st, h3, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/client-scopes"),
        h,
        Some(&json!({ "name": "hel-new", "protocol": "openid-connect" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = h3[header::LOCATION]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        f.store
            .get_client_scope("dev", &sid)
            .await
            .unwrap()
            .unwrap()
            .org_id
            .as_deref(),
        Some("helsinki"),
        "an organization admin's scope belongs to the organization"
    );
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/client-scopes"),
        h,
        Some(&json!({ "name": "esp-new", "protocol": "openid-connect", "orgId": "espoo" })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/client-scopes/{sid}"),
        h,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Regression: brute-force unlock only inside the organization.
    for target in ["user:esp-user", "user:hel-user"] {
        f.store
            .upsert_login_failure(LoginFailureRecord {
                id: target.into(),
                realm_id: "dev".into(),
                target_key: target.into(),
                num_failures: 30,
                last_failure_ms: 1,
                last_ip_failure: "192.0.2.9".into(),
                locked_until: Some(Utc::now() + Duration::hours(1)),
                window_started_at: Utc::now(),
            })
            .await
            .unwrap();
    }
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/attack-detection/brute-force/users/esp-user"),
        h,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/attack-detection/brute-force/users/admin-1"),
        h,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(
        &f.app,
        &Method::DELETE,
        &format!("{R}/attack-detection/brute-force/users/hel-user"),
        h,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        f.store
            .get_login_failure("dev", "user:esp-user")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        f.store
            .get_login_failure("dev", "user:hel-user")
            .await
            .unwrap()
            .is_none()
    );

    // Regression: `Allowed` judges a person on the person's own organization, like the call.
    let (st, _, _) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/users/realm-user"),
        h,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "a realm-level person is not an org admin's to read"
    );
    let (st, _, ans) = call(
        &f.app,
        &Method::GET,
        "/api/v1/realms/dev/allowed?op=read&kind=Person&id=realm-user",
        h,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ans["allowed"], false, "{ans}");

    // Admin client roles are realm-admin business.
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/users/hel-user/role-mappings/clients/c-rm"),
        h,
        Some(&json!([{ "name": "manage-users" }])),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/users/hel-user/role-mappings/clients/c-hel"),
        h,
        Some(&json!([{ "name": "viewer" }])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn reset_password_follows_the_policy_and_required_actions_need_smtp() {
    let f = fixture().await;
    let a = Some(f.admin.as_str());
    let mut realm = f.store.get_realm("dev").await.unwrap().unwrap();
    realm.password_policy = "length(12)".into();
    realm.smtp_server = None;
    f.store.upsert_realm(realm).await.unwrap();
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/users/hel-user/reset-password"),
        a,
        Some(&json!({ "value": "short" })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the policy answers field errors"
    );
    assert!(
        f.store
            .list_credentials("dev", "hel-user")
            .await
            .unwrap()
            .is_empty()
    );
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/users/hel-user/execute-actions-email"),
        a,
        Some(&json!(["UPDATE_PASSWORD"])),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_IMPLEMENTED,
        "without SMTP the Portal falls back (PF-92)"
    );
    let u = f
        .store
        .get_user_by_id("dev", None, "hel-user")
        .await
        .unwrap()
        .unwrap();
    assert!(u.required_actions.is_empty());
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/users/hel-user/execute-actions-email"),
        a,
        Some(&json!({ "not": "a list" })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
}

/// POST /users with `groups` joins each named group, as Keycloak does: an unknown path answers
/// 404, and a group the caller may not manage members of answers as PUT /users/{id}/groups/{gid}
/// does; in both cases nobody is created.
#[tokio::test]
async fn create_user_joins_the_groups_its_body_names() {
    let f = fixture().await;
    f.store
        .upsert_group(GroupRepresentation {
            id: "admins-group".into(),
            realm_id: "dev".into(),
            org_id: None,
            name: "platform-admins".into(),
            path: "/platform-admins".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let user = |name: &str, groups: Value| json!({"username": name, "email": format!("{name}@example.test"), "enabled": true, "groups": groups});
    let created = |name: &'static str| {
        let store = f.store.clone();
        async move {
            !store
                .list_users("dev", None, None, None, Some(name), true, 0, 10)
                .await
                .unwrap()
                .is_empty()
        }
    };
    let users = format!("{R}/users");
    let (status, headers, _) = call(
        &f.app,
        &Method::POST,
        &users,
        Some(&f.admin),
        Some(&user(
            "joiner",
            json!(["/platform-admins", "/helsinki/members"]),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = headers[header::LOCATION]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let (status, _, groups) = call(
        &f.app,
        &Method::GET,
        &format!("{R}/users/{id}/groups"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut paths: Vec<&str> = groups
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["path"].as_str().unwrap())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, ["/helsinki/members", "/platform-admins"], "{groups}");

    let (status, _, _) = call(
        &f.app,
        &Method::POST,
        &users,
        Some(&f.admin),
        Some(&user("ghost", json!(["/no/such/group"]))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!created("ghost").await, "nobody is created");

    // A caller its authorizer refuses group writes to is refused as on the PUT route, before
    // the person exists.
    let refusing = admin_router(AdminState::new(
        OidcState {
            store: f.store.clone(),
            kek: f.kek.clone(),
            guard: OutboundGuard::default(),
            default_mount: Mount::default(),
        },
        Arc::new(NoGroupWrites),
    ))
    .layer(Extension(Mount::default()));
    let (put_status, _, _) = call(
        &refusing,
        &Method::PUT,
        &format!("{R}/users/hel-user/groups/admins-group"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(put_status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(
        &refusing,
        &Method::POST,
        &users,
        Some(&f.admin),
        Some(&user("climber", json!(["/platform-admins"]))),
    )
    .await;
    assert_eq!(status, put_status, "the same answer as the PUT route");
    assert!(!created("climber").await, "nobody is created");
    // An organization admin does not reach a realm-wide group either.
    let (status, _, _) = call(
        &f.app,
        &Method::POST,
        &users,
        Some(&f.hel_admin),
        Some(&user("riser", json!(["/platform-admins"]))),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    assert!(!created("riser").await, "nobody is created");
    // Another organization's group is not found, as on the PUT route.
    let (status, _, _) = call(
        &f.app,
        &Method::POST,
        &users,
        Some(&f.hel_admin),
        Some(&user("wanderer", json!(["/espoo/members"]))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!created("wanderer").await, "nobody is created");
}

/// The local rules, except that no group may be written.
struct NoGroupWrites;

impl Authorizer for NoGroupWrites {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision {
        if action.is_write() && matches!(object, ObjectTarget::Group { .. }) {
            return Decision::deny_visible("no group writes");
        }
        LocalAuthorizer.check(ctx, action, object)
    }
}

/// A create or update cannot land on another object through the stores' upsert (by id, path or
/// clientId), carry role grants or sub-groups in a group body, or let an organization admin set
/// the group attributes the token mappers read as organization membership.
#[tokio::test]
async fn writes_cannot_take_over_other_objects_or_grant_through_group_bodies() {
    let f = fixture().await;
    let (a, h) = (Some(f.admin.as_str()), Some(f.hel_admin.as_str()));

    // Group create: the body's id, path, roles and sub-groups are not written.
    let body = json!({
        "id": "esp-group", "name": "taken", "path": "/espoo/members",
        "realmRoles": ["realm-admin"], "clientRoles": { "realm-management": ["realm-admin"] },
        "subGroups": [{ "name": "inner", "path": "/espoo/members/inner" }]
    });
    let (st, hdr, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/groups"),
        h,
        Some(&body),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let id = hdr["location"]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    assert_ne!(id, "esp-group");
    let made = f.store.get_group("dev", None, &id).await.unwrap().unwrap();
    assert_eq!(made.path, "/taken");
    assert!(
        made.realm_roles.is_empty() && made.client_roles.is_empty(),
        "{made:?}"
    );
    let esp = f
        .store
        .get_group("dev", None, "esp-group")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (esp.path.as_str(), esp.org_id.as_deref()),
        ("/espoo/members", Some("espoo"))
    );
    assert!(
        f.store
            .get_group_by_path("dev", None, "/espoo/members/inner")
            .await
            .unwrap()
            .is_none()
    );
    // A taken top-level name is 409, as Keycloak.
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/groups"),
        h,
        Some(&json!({ "name": "taken" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Membership attributes: an organization admin cannot set or change them, a realm admin can.
    let foreign = json!({ "name": "squat", "attributes": { "jc_org": ["espoo"] } });
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/groups"),
        h,
        Some(&foreign),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/groups/hel-group"),
        h,
        Some(&json!({ "name": "members", "attributes": { "jc_groups": ["/espoo/members"] } })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/groups"),
        a,
        Some(&foreign),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    // Role fields in an update keep the stored grants.
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/groups/hel-group"),
        h,
        Some(&json!({ "name": "members", "realmRoles": ["realm-admin"] })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let hel = f
        .store
        .get_group("dev", None, "hel-group")
        .await
        .unwrap()
        .unwrap();
    assert!(hel.realm_roles.is_empty(), "{hel:?}");
    assert_eq!(hel.path, "/helsinki/members");

    // Clients: a taken clientId or id is 409 and the stored client keeps its organization and
    // secret; an update cannot rename onto another client's clientId.
    for body in [
        json!({ "clientId": "esp-app" }),
        json!({ "clientId": "fresh", "id": "c-rm" }),
    ] {
        let (st, _, _) = call(
            &f.app,
            &Method::POST,
            &format!("{R}/clients"),
            h,
            Some(&body),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
    }
    let esp = f
        .store
        .get_client_by_client_id("dev", "esp-app")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(esp.org_id.as_deref(), Some("espoo"));
    assert_eq!(esp.secret.as_deref(), Some("esp-app-secret"));
    let rm = f
        .store
        .get_client_by_uuid("dev", None, "c-rm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rm.client_id, "realm-management");
    let (st, _, _) = call(
        &f.app,
        &Method::PUT,
        &format!("{R}/clients/c-hel"),
        h,
        Some(&json!({ "clientId": "realm-management" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Client scopes: another scope's id is 409, the stored scope stays.
    let (st, _, _) = call(
        &f.app,
        &Method::POST,
        &format!("{R}/client-scopes"),
        a,
        Some(&json!({ "id": "sc-hel", "name": "other", "protocol": "openid-connect" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let sc = f
        .store
        .get_client_scope("dev", "sc-hel")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (sc.name.as_str(), sc.org_id.as_deref()),
        ("hel-scope", Some("helsinki"))
    );
}
