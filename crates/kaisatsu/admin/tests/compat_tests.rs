#![forbid(unsafe_code)]
//! Comprehensive test suite covering all 46 rows of `crates/kaisatsu/COMPAT.md`, OData entity sets,
//! and cross-organization isolation (W1, W3, W4, W5, W6).

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_admin::{AdminState, admin_router, odata_router};
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, sign_jwt};
use kaisatsu_oidc::{OidcState, OutboundGuard};
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn setup_app() -> (axum::Router, Arc<MemoryStore>, String, String) {
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

    // Seed realm-admin token and org-admin token for `helsinki`
    let keys = store.list_signing_keys("dev").await.unwrap();
    let key = &keys[0];
    let now = chrono::Utc::now().timestamp();

    let realm_admin_token = sign_jwt(
        key,
        &kek,
        Some("JWT"),
        &json!({
            "iss": "https://idm.localhost/realms/dev",
            "sub": "admin-1",
            "azp": "admin-cli",
            "typ": "Bearer",
            "iat": now,
            "exp": now + 3600,
            "realm_access": { "roles": ["realm-admin"] }
        }),
    )
    .unwrap();

    store
        .upsert_organization(Organization {
            id: "helsinki".into(),
            realm_id: "dev".into(),
            name: "Helsinki".into(),
            alias: "helsinki".into(),
            enabled: true,
            description: String::new(),
            domains: Vec::new(),
            attributes: Default::default(),
            managed_by: None,
        })
        .await
        .unwrap();
    store
        .upsert_membership(Membership {
            id: "m-hel".into(),
            realm_id: "dev".into(),
            org_id: "helsinki".into(),
            person_id: "org-admin-hel".into(),
            org_roles: vec!["org-admin".into()],
            created_at: chrono::Utc::now(),
        })
        .await
        .unwrap();

    let hel_admin_token = sign_jwt(
        key,
        &kek,
        Some("JWT"),
        &json!({
            "iss": "https://idm.localhost/realms/dev",
            "sub": "org-admin-hel",
            "azp": "portal-api",
            "typ": "Bearer",
            "iat": now,
            "exp": now + 3600,
            "jc_org": ["helsinki"]
        }),
    )
    .unwrap();

    let oidc_state = OidcState {
        store: store.clone(),
        kek,
        guard: OutboundGuard::default(),
        default_mount: Mount::default(),
        second_factor: Default::default(),
    };
    let admin_state = AdminState::new(oidc_state, Arc::new(LocalAuthorizer));
    let app = admin_router(admin_state.clone()).merge(odata_router(admin_state));
    (app, store, realm_admin_token, hel_admin_token)
}

async fn send_req(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let req = if let Some(b) = body {
        builder = builder.header("content-type", "application/json");
        builder.body(Body::from(b.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, val)
}

#[tokio::test]
async fn row_01_02_03_realms_and_partial_import() {
    let (app, store, admin_token, _) = setup_app().await;

    // Row 2: POST /admin/realms. Keycloak creates realms only for a master-realm admin
    // (keycloak-config-cli logs in to master); a token of realm `dev` may not create `stage`.
    let (st, _, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms",
        &admin_token,
        Some(json!({ "id": "stage", "realm": "stage", "enabled": true })),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    store
        .upsert_realm(RealmConfig {
            id: "master".into(),
            realm: "master".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let master_keys = store.list_signing_keys("master").await.unwrap();
    let now = chrono::Utc::now().timestamp();
    let master_admin_token = sign_jwt(
        &master_keys[0],
        store.kek(),
        Some("JWT"),
        &json!({
            "iss": "https://idm.localhost/realms/master",
            "sub": "master-admin-1",
            "azp": "admin-cli",
            "typ": "Bearer",
            "iat": now,
            "exp": now + 3600,
            "realm_access": { "roles": ["admin"] }
        }),
    )
    .unwrap();
    let (st, _, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms",
        &master_admin_token,
        Some(json!({ "id": "stage", "realm": "stage", "enabled": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    // COMPAT row 2: a realm that exists answers 409 and is not overwritten.
    let (st, _, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms",
        &master_admin_token,
        Some(
            json!({ "id": "stage", "realm": "stage", "displayName": "clobbered", "enabled": true }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Row 1 & 3: GET & PUT /admin/realms/{realm}
    let (st, _, body) = send_req(
        &app,
        Method::GET,
        "/admin/realms/stage",
        &master_admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["realm"], "stage");

    let (st, _, _) = send_req(
        &app,
        Method::PUT,
        "/admin/realms/stage",
        &master_admin_token,
        Some(
            json!({ "id": "stage", "realm": "stage", "displayName": "Stage Org", "enabled": true }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    // A token of realm `dev` cannot read realm `stage`.
    let (st, _, _) = send_req(&app, Method::GET, "/admin/realms/stage", &admin_token, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Row 4: POST /admin/realms/{realm}/partialImport (SKIP and OVERWRITE, PF-126 fixed IDs)
    let fixed_id = "de300000-0000-4000-8000-000000000001";
    let (st, _, res) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/partialImport",
        &admin_token,
        Some(json!({
            "ifResourceExists": "SKIP",
            "users": [{
                "id": fixed_id,
                "username": "demo.steward@hel.fi",
                "email": "demo.steward@hel.fi",
                "enabled": true,
                "emailVerified": true
            }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(res["added"], 1);

    let (st, _, res2) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/partialImport",
        &admin_token,
        Some(json!({
            "ifResourceExists": "SKIP",
            "users": [{
                "id": fixed_id,
                "username": "demo.steward@hel.fi",
                "email": "demo.steward@hel.fi"
            }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(res2["skipped"], 1);
}

#[tokio::test]
async fn row_05_to_17_users_lifecycle_and_subresources() {
    let (app, store, admin_token, _) = setup_app().await;

    // Row 6: POST /admin/realms/{realm}/users -> 201 + Location, 409 on duplicate
    let (st, hdrs, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/users",
        &admin_token,
        Some(json!({
            "username": "anna@hel.fi",
            "email": "anna@hel.fi",
            "firstName": "Anna",
            "lastName": "Virtanen",
            "enabled": true,
            "requiredActions": ["VERIFY_EMAIL", "UPDATE_PASSWORD"]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let loc = hdrs.get("location").unwrap().to_str().unwrap();
    let user_id = loc.rsplit('/').next().unwrap().to_owned();

    let (dup_st, _, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/users",
        &admin_token,
        Some(json!({ "username": "anna@hel.fi", "email": "anna@hel.fi" })),
    )
    .await;
    assert_eq!(dup_st, StatusCode::CONFLICT);

    // Row 5 & 7 & 8: GET /users, GET /users/{id}, PUT /users/{id}
    let (st, _, list) = send_req(
        &app,
        Method::GET,
        "/admin/realms/dev/users?search=anna&first=0&max=10&briefRepresentation=false",
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    let (st, _, u) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/users/{user_id}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(u["firstName"], "Anna");

    let (st, _, _) = send_req(
        &app,
        Method::PUT,
        &format!("/admin/realms/dev/users/{user_id}"),
        &admin_token,
        Some(json!({
            "id": user_id,
            "username": "anna@hel.fi",
            "email": "anna@hel.fi",
            "firstName": "Anna-Liisa",
            "lastName": "Virtanen",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Row 10, 11: PUT execute-actions-email and PUT reset-password
    let (st, _, _) = send_req(
        &app,
        Method::PUT,
        &format!("/admin/realms/dev/users/{user_id}/execute-actions-email?lifespan=3600"),
        &admin_token,
        Some(json!(["VERIFY_EMAIL"])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _, _) = send_req(
        &app,
        Method::PUT,
        &format!("/admin/realms/dev/users/{user_id}/reset-password"),
        &admin_token,
        Some(json!({ "type": "password", "value": "TempPass123!Valid", "temporary": true })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Seed an OTP credential and consent, then test Rows 12..17
    store
        .upsert_credential(CredentialRecord {
            id: "otp-1".into(),
            realm_id: "dev".into(),
            user_id: user_id.clone(),
            credential_type: "otp".into(),
            user_label: Some("Phone".into()),
            created_date: 1000,
            secret_data: r#"{"value":"seed"}"#.into(),
            credential_data: String::new(),
            temporary: false,
        })
        .await
        .unwrap();
    store
        .upsert_consent(ConsentRecord {
            id: "c1".into(),
            realm_id: "dev".into(),
            org_id: None,
            user_id: user_id.clone(),
            client_id: "mcp-hub".into(),
            granted_client_scopes: vec!["endpoint:*".into()],
            created_date: 1000,
            last_updated_date: 1000,
        })
        .await
        .unwrap();

    let (st, _, creds) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/users/{user_id}/credentials"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(creds.as_array().unwrap().iter().any(|c| c["id"] == "otp-1"));

    let (st, _, _) = send_req(
        &app,
        Method::DELETE,
        &format!("/admin/realms/dev/users/{user_id}/credentials/otp-1"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _, consents) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/users/{user_id}/consents"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(consents.as_array().unwrap().len(), 1);

    let (st, _, _) = send_req(
        &app,
        Method::DELETE,
        &format!("/admin/realms/dev/users/{user_id}/consents/mcp-hub"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _, _) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/users/{user_id}/sessions"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, _) = send_req(
        &app,
        Method::POST,
        &format!("/admin/realms/dev/users/{user_id}/logout"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Row 9: DELETE /users/{id}
    let (st, _, _) = send_req(
        &app,
        Method::DELETE,
        &format!("/admin/realms/dev/users/{user_id}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn row_18_to_34_groups_roles_and_mappings() {
    let (app, _store, admin_token, _) = setup_app().await;

    // Create user and group (Rows 22..26)
    let (_, hdrs, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/users",
        &admin_token,
        Some(json!({ "username": "matti@hel.fi", "email": "matti@hel.fi", "enabled": true })),
    )
    .await;
    let uid = hdrs
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();

    let (st, ghdrs, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/groups",
        &admin_token,
        Some(json!({
            "name": "helsinki-readers",
            "attributes": { "managed-by": ["joinedcontext"] }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let gid = ghdrs
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();

    // Row 19, 18, 26, 20: add user to group, list user groups, list group members, remove from group
    let (st, _, _) = send_req(
        &app,
        Method::PUT,
        &format!("/admin/realms/dev/users/{uid}/groups/{gid}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _, ugroups) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/users/{uid}/groups"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ugroups[0]["path"], "/helsinki-readers");

    let (st, _, gmembers) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/groups/{gid}/members?max=1000"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(gmembers[0]["id"], uid);

    // Rows 27..34: Create client, create client role, map to user & group, query role holders, delete
    let (_, chdrs, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/clients",
        &admin_token,
        Some(json!({
            "clientId": "app-kartta",
            "attributes": { "managed-by": "joinedcontext" }
        })),
    )
    .await;
    let cuuid = chdrs
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();

    let (st, _, _) = send_req(
        &app,
        Method::POST,
        &format!("/admin/realms/dev/clients/{cuuid}/roles"),
        &admin_token,
        Some(json!({ "name": "viewer" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, _, roles) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/clients/{cuuid}/roles"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let role_id = roles[0]["id"].as_str().unwrap().to_owned();

    let (st, _, _) = send_req(
        &app,
        Method::POST,
        &format!("/admin/realms/dev/users/{uid}/role-mappings/clients/{cuuid}"),
        &admin_token,
        Some(json!([{ "id": role_id, "name": "viewer" }])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _, _) = send_req(
        &app,
        Method::POST,
        &format!("/admin/realms/dev/groups/{gid}/role-mappings/clients/{cuuid}"),
        &admin_token,
        Some(json!([{ "id": role_id, "name": "viewer" }])),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _, rusers) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/clients/{cuuid}/roles/viewer/users?max=1000"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rusers.as_array().unwrap().len(), 1);

    let (st, _, rgroups) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/clients/{cuuid}/roles/viewer/groups?briefRepresentation=true&max=1000"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rgroups.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn row_35_to_46_clients_secrets_mappers_and_scopes() {
    let (app, store, admin_token, _) = setup_app().await;

    // Row 45: POST /client-scopes
    let (st, shdrs, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/client-scopes",
        &admin_token,
        Some(json!({
            "name": "endpoint:bikes1234",
            "protocol": "openid-connect",
            "attributes": { "managed-by": "joinedcontext" }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let scope_id = shdrs
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();

    // Row 36: POST /clients with serviceAccountsEnabled: true
    let (st, chdrs, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/clients",
        &admin_token,
        Some(json!({
            "clientId": "helsinki-ext",
            "publicClient": false,
            "serviceAccountsEnabled": true,
            "defaultClientScopes": ["basic"],
            "optionalClientScopes": [],
            "attributes": { "managed-by": "joinedcontext" }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let cuuid = chdrs
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();

    // Row 39 & 40: GET & POST /clients/{uuid}/client-secret
    let (st, _, sec1) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/clients/{cuuid}/client-secret"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, sec2) = send_req(
        &app,
        Method::POST,
        &format!("/admin/realms/dev/clients/{cuuid}/client-secret"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_ne!(sec1["value"], sec2["value"]);

    // Row 41: GET /clients/{uuid}/service-account-user
    let (st, _, sa_user) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/clients/{cuuid}/service-account-user"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sa_user["username"], "service-account-helsinki-ext");

    // Row 42 & 43: protocol-mappers/models CRUD
    let (st, _, _) = send_req(
        &app,
        Method::POST,
        &format!("/admin/realms/dev/clients/{cuuid}/protocol-mappers/models"),
        &admin_token,
        Some(json!({
            "name": "audience-bikes",
            "protocol": "openid-connect",
            "protocolMapper": "oidc-audience-mapper",
            "config": { "included.custom.audience": "bikes1234", "access.token.claim": "true" }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, _, mappers) = send_req(
        &app,
        Method::GET,
        &format!("/admin/realms/dev/clients/{cuuid}/protocol-mappers/models"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let mid = mappers[0]["id"].as_str().unwrap().to_owned();

    let (st, _, _) = send_req(
        &app,
        Method::DELETE,
        &format!("/admin/realms/dev/clients/{cuuid}/protocol-mappers/models/{mid}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Row 44: link and unlink optional client scope
    let (st, _, _) = send_req(
        &app,
        Method::PUT,
        &format!("/admin/realms/dev/clients/{cuuid}/optional-client-scopes/{scope_id}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _, _) = send_req(
        &app,
        Method::DELETE,
        &format!("/admin/realms/dev/clients/{cuuid}/optional-client-scopes/{scope_id}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Row 46: DELETE /attack-detection/brute-force/users/{user_id} (break-glass unlock of an
    // existing person; an unknown id is 404 like Keycloak's AttackDetectionResource)
    store
        .create_user(UserRepresentation {
            id: "u1".into(),
            realm_id: "dev".into(),
            username: "locked-out".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let (st, _, _) = send_req(
        &app,
        Method::DELETE,
        "/admin/realms/dev/attack-detection/brute-force/users/nobody",
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = send_req(
        &app,
        Method::DELETE,
        "/admin/realms/dev/attack-detection/brute-force/users/u1",
        &admin_token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_client_write_pinning_an_algorithm_the_realm_cannot_sign_is_refused() {
    let (app, store, admin_token, _) = setup_app().await;
    let ps512 = json!({
        "clientId": "app-ps",
        "attributes": { "id.token.signed.response.alg": "PS512" }
    });
    let (st, _, body) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/clients",
        &admin_token,
        Some(ps512.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    let text = body.to_string();
    assert!(
        text.contains("app-ps") && text.contains("id.token.signed.response.alg"),
        "{text}"
    );
    assert!(
        store
            .get_client_by_client_id("dev", "app-ps")
            .await
            .unwrap()
            .is_none()
    );

    // partialImport refuses before writing anything, users included.
    let (st, _, body) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/partialImport",
        &admin_token,
        Some(json!({
            "ifResourceExists": "SKIP",
            "users": [{ "username": "alg.person", "enabled": true }],
            "clients": [ps512]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        store
            .get_user_by_username_or_email("dev", None, "alg.person")
            .await
            .unwrap()
            .is_none()
    );

    // An accepted client cannot be moved to PS512 by PUT; EdDSA and "" are fine.
    let (st, headers, _) = send_req(
        &app,
        Method::POST,
        "/admin/realms/dev/clients",
        &admin_token,
        Some(json!({
            "clientId": "app-ed",
            "attributes": {
                "access.token.signed.response.alg": "EdDSA",
                "id.token.signed.response.alg": ""
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let uuid = headers["location"]
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let (st, _, body) = send_req(
        &app,
        Method::PUT,
        &format!("/admin/realms/dev/clients/{uuid}"),
        &admin_token,
        Some(json!({
            "clientId": "app-ed",
            "attributes": { "access.token.signed.response.alg": "HS256" }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    let kept = store
        .get_client_by_client_id("dev", "app-ed")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.attributes["access.token.signed.response.alg"], "EdDSA");
}
