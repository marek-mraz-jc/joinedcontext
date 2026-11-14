#![forbid(unsafe_code)]
//! Admin authorization regressions from the 2026-11-07 review (W6). Each failed before its fix:
//! - any person could PUT their own user with `realmRoles: ["realm-admin"]` (self-service write),
//! - an organization admin could hand out `realm-management` roles and move people between
//!   organizations through PUT,
//! - role-mapping deletes were checked against the caller's own organization, not the person's,
//! - a same-id person of the `master` realm lent its roles to a token of another realm,
//! - a department realm role called `platform-admin` (or `admin`) made its holder realm admin,
//! - a revoked admin token kept working.
//! - (2026-11-08) an organization admin could create a person holding `realm-admin` through
//!   `realmRoles` / `clientRoles` in the POST body.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_admin::{AdminState, admin_router, scim_router};
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, sha256_hex, sign_jwt};
use kaisatsu_oidc::{OidcState, OutboundGuard};
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct Fixture {
    app: axum::Router,
    store: Arc<MemoryStore>,
}

impl Fixture {
    async fn new() -> Self {
        let kek = Kek::generate_for_tests();
        let store = Arc::new(MemoryStore::new(kek.clone()));
        for realm in ["dev", "master"] {
            store
                .upsert_realm(RealmConfig {
                    id: realm.into(),
                    realm: realm.into(),
                    ..Default::default()
                })
                .await
                .unwrap();
        }
        for (org, admin) in [("helsinki", "org-admin-hel"), ("espoo", "org-admin-esp")] {
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
            store
                .upsert_membership(Membership {
                    id: format!("m-{admin}"),
                    realm_id: "dev".into(),
                    org_id: org.into(),
                    person_id: admin.into(),
                    org_roles: vec!["org-admin".into()],
                    created_at: chrono::Utc::now(),
                })
                .await
                .unwrap();
        }
        for (id, org) in [
            ("plain-1", "helsinki"),
            ("hel-person", "helsinki"),
            ("esp-person", "espoo"),
            ("org-admin-hel", "helsinki"),
        ] {
            store
                .create_user(UserRepresentation {
                    id: id.into(),
                    realm_id: "dev".into(),
                    org_id: Some(org.into()),
                    username: id.into(),
                    ..Default::default()
                })
                .await
                .unwrap();
        }
        store
            .upsert_client(ClientRepresentation {
                id: "uuid-rm".into(),
                realm_id: "dev".into(),
                client_id: "realm-management".into(),
                bearer_only: true,
                ..Default::default()
            })
            .await
            .unwrap();
        store
            .upsert_client(ClientRepresentation {
                id: "uuid-hel-app".into(),
                realm_id: "dev".into(),
                org_id: Some("helsinki".into()),
                client_id: "hel-app".into(),
                // An App client the Portal created carries its mark (app_clients.rs).
                managed_by: Some("joinedcontext".into()),
                attributes: std::collections::BTreeMap::from([(
                    "managed-by".to_owned(),
                    "joinedcontext".to_owned(),
                )]),
                ..Default::default()
            })
            .await
            .unwrap();
        let oidc = OidcState {
            store: store.clone(),
            kek,
            guard: OutboundGuard::default(),
            default_mount: Mount::default(),
            second_factor: Default::default(),
        };
        let state = AdminState::new(oidc, Arc::new(LocalAuthorizer));
        let app = admin_router(state.clone()).merge(scim_router(state));
        Self { app, store }
    }

    async fn token(&self, realm: &str, sub: &str, extra: Value) -> String {
        let keys = self.store.list_signing_keys(realm).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut claims = json!({
            "iss": format!("https://idm.localhost/realms/{realm}"),
            "sub": sub, "azp": "admin-cli", "typ": "Bearer",
            "jti": format!("jti-{realm}-{sub}-{}", extra),
            "iat": now, "exp": now + 3600,
        });
        if let (Some(base), Some(add)) = (claims.as_object_mut(), extra.as_object()) {
            for (k, v) in add {
                base.insert(k.clone(), v.clone());
            }
        }
        sign_jwt(&keys[0], self.store.kek(), Some("JWT"), &claims).unwrap()
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> StatusCode {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"));
        let req = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                b.body(Body::from(v.to_string())).unwrap()
            }
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let _ = resp.into_body().collect().await;
        st
    }

    async fn user(&self, id: &str) -> UserRepresentation {
        self.store
            .get_user_by_id("dev", None, id)
            .await
            .unwrap()
            .unwrap()
    }
}

fn realm_body() -> Value {
    json!({ "id": "dev", "realm": "dev", "displayName": "taken over", "enabled": true })
}

#[tokio::test]
async fn a_person_cannot_grant_themselves_roles_through_their_own_user() {
    let f = Fixture::new().await;
    let t = f
        .token("dev", "plain-1", json!({ "jc_org": ["helsinki"] }))
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/plain-1",
            &t,
            Some(json!({ "username": "plain-1", "enabled": true,
                         "realmRoles": ["realm-admin"],
                         "clientRoles": { "realm-management": ["realm-admin"] } })),
        )
        .await;
    assert!(
        matches!(st, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
        "self PUT answered {st}"
    );
    let u = f.user("plain-1").await;
    assert!(u.realm_roles.is_empty());
    assert!(u.client_roles.is_empty());
    let st = f
        .send(Method::PUT, "/admin/realms/dev", &t, Some(realm_body()))
        .await;
    assert_ne!(st, StatusCode::NO_CONTENT);
    // Reading the own account still works.
    let st = f
        .send(Method::GET, "/admin/realms/dev/users/plain-1", &t, None)
        .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn org_admin_put_keeps_role_mappings_and_organization() {
    let f = Fixture::new().await;
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/hel-person",
            &t,
            Some(
                json!({ "username": "hel-person", "firstName": "Hanna", "enabled": true,
                         "orgId": "espoo", "realmRoles": ["realm-admin"],
                         "serviceAccountClientId": "realm-management" }),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let u = f.user("hel-person").await;
    assert_eq!(u.first_name.as_deref(), Some("Hanna"));
    assert!(u.realm_roles.is_empty());
    assert_eq!(u.org_id.as_deref(), Some("helsinki"));
    assert_eq!(u.service_account_client_id, None);
}

#[tokio::test]
async fn org_admin_cannot_hand_out_admin_client_roles() {
    let f = Fixture::new().await;
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/users/hel-person/role-mappings/clients/uuid-rm",
            &t,
            Some(json!([{ "name": "realm-admin" }])),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // Its own organization's application roles stay grantable.
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/users/hel-person/role-mappings/clients/uuid-hel-app",
            &t,
            Some(json!([{ "name": "editor" }])),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

/// An Organization's admin manages the people of the Organization, never a realm administrator
/// who happens to be a member: no password reset, credential delete, update or delete. A realm
/// user manager (`manage-users`) still can, as in Keycloak; the own account stays one's own.
#[tokio::test]
async fn org_admin_cannot_reset_or_change_a_realm_admin_of_the_organization() {
    let f = Fixture::new().await;
    f.store
        .create_user(UserRepresentation {
            id: "hel-root".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            username: "hel-root".into(),
            realm_roles: vec!["realm-admin".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let reset = json!({ "type": "password", "value": "Another-pass-123", "temporary": false });
    for (method, path, body) in [
        (
            Method::PUT,
            "/admin/realms/dev/users/hel-root/reset-password",
            Some(reset.clone()),
        ),
        (
            Method::PUT,
            "/admin/realms/dev/users/hel-root",
            Some(json!({ "username": "hel-root", "firstName": "X" })),
        ),
        (
            Method::DELETE,
            "/admin/realms/dev/users/hel-root/credentials/c-1",
            None,
        ),
        (Method::DELETE, "/admin/realms/dev/users/hel-root", None),
    ] {
        let st = f.send(method.clone(), path, &t, body).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{method} {path}");
    }
    assert!(
        f.store
            .get_user_by_id("dev", None, "hel-root")
            .await
            .unwrap()
            .is_some()
    );
    // The same admin still resets a plain member, and a realm user manager resets the admin.
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/hel-person/reset-password",
            &t,
            Some(reset.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let manager = f
        .token(
            "dev",
            "user-manager",
            json!({ "resource_access": { "realm-management": { "roles": ["manage-users"] } } }),
        )
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/hel-root/reset-password",
            &manager,
            Some(reset),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn role_mapping_delete_is_checked_against_the_persons_organization() {
    let f = Fixture::new().await;
    f.store
        .add_role_mapping(RoleMappingRecord {
            id: "rm-esp".into(),
            realm_id: "dev".into(),
            org_id: Some("espoo".into()),
            principal_kind: "user".into(),
            principal_id: "esp-person".into(),
            role_id: "editor".into(),
            role_name: "editor".into(),
            client_uuid: Some("uuid-hel-app".into()),
        })
        .await
        .unwrap();
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let st = f
        .send(
            Method::DELETE,
            "/admin/realms/dev/users/esp-person/role-mappings/clients/uuid-hel-app",
            &t,
            Some(json!([{ "name": "editor" }])),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn roles_never_leak_across_realms_or_from_role_names() {
    let f = Fixture::new().await;
    // A `master` person with the same id as a dev person holds realm-admin there.
    f.store
        .create_user(UserRepresentation {
            id: "org-admin-hel".into(),
            realm_id: "master".into(),
            username: "someone-in-master".into(),
            realm_roles: vec!["realm-admin".into(), "admin".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let dev_token = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev",
            &dev_token,
            Some(realm_body()),
        )
        .await;
    assert_ne!(
        st,
        StatusCode::NO_CONTENT,
        "master roles lent to a dev token"
    );

    // `platform-admin` and `admin` are ordinary role names in a department realm.
    for role in ["platform-admin", "admin"] {
        let t = f
            .token(
                "dev",
                "plain-1",
                json!({ "realm_access": { "roles": [role] } }),
            )
            .await;
        let st = f
            .send(Method::PUT, "/admin/realms/dev", &t, Some(realm_body()))
            .await;
        assert_ne!(
            st,
            StatusCode::NO_CONTENT,
            "dev role {role} made a realm admin"
        );
    }

    // Keycloak's cross-realm admin: master `admin` administers dev ...
    let master_admin = f
        .token(
            "master",
            "root",
            json!({ "realm_access": { "roles": ["admin"] } }),
        )
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev",
            &master_admin,
            Some(realm_body()),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    // ... master's own realm-management does not reach dev ...
    let master_rm = f
        .token(
            "master",
            "ops",
            json!({ "resource_access": { "realm-management": { "roles": ["realm-admin"] } } }),
        )
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev",
            &master_rm,
            Some(realm_body()),
        )
        .await;
    assert_ne!(st, StatusCode::NO_CONTENT);
    // ... but the `dev-realm` client roles do.
    let master_dev = f
        .token(
            "master",
            "ops2",
            json!({ "resource_access": { "dev-realm": { "roles": ["realm-admin"] } } }),
        )
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev",
            &master_dev,
            Some(realm_body()),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_revoked_admin_token_is_refused() {
    let f = Fixture::new().await;
    let t = f
        .token(
            "dev",
            "boss",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;
    assert_eq!(
        f.send(Method::GET, "/admin/realms/dev/users", &t, None)
            .await,
        StatusCode::OK
    );
    let jti = format!(
        "jti-dev-boss-{}",
        json!({ "realm_access": { "roles": ["realm-admin"] } })
    );
    f.store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex(&jti),
            realm_id: "dev".into(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        })
        .await
        .unwrap();
    assert_eq!(
        f.send(Method::GET, "/admin/realms/dev/users", &t, None)
            .await,
        StatusCode::UNAUTHORIZED
    );
}

/// Deleting a person ends their sessions (refresh families revoked, clients told); a client's
/// service-account user is its own, never a person who took the name.
#[tokio::test]
async fn person_delete_and_service_account_user_follow_the_person() {
    let f = Fixture::new().await;
    let now = chrono::Utc::now();
    let fam = RefreshTokenFamily {
        id: "fam-1".into(),
        realm_id: "dev".into(),
        org_id: Some("helsinki".into()),
        sso_session_id: "sso-1".into(),
        user_id: "hel-person".into(),
        client_id: "hel-app".into(),
        scope: "openid".into(),
        mapper_scopes: Vec::new(),
        acr: "pwd".into(),
        amr: Vec::new(),
        auth_time: now.timestamp(),
        rotation_counter: 0,
        current_jti_hash: "h".into(),
        previous_hashes: Vec::new(),
        dpop_jkt: None,
        offline: false,
        revoked: false,
        expires_at: now + chrono::Duration::hours(1),
    };
    f.store.upsert_refresh_family(fam.clone()).await.unwrap();
    let admin = f
        .token(
            "dev",
            "boss",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;
    assert_eq!(
        f.send(
            Method::DELETE,
            "/admin/realms/dev/users/hel-person",
            &admin,
            None
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let family = f.store.get_refresh_family("dev", "fam-1").await.unwrap();
    assert!(family.as_ref().is_none_or(|fam| fam.revoked), "{family:?}");

    // The registry's person_delete (SCIM, OData, MCP) ends the sessions too.
    f.store
        .create_user(UserRepresentation {
            id: "hel-two".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            username: "hel-two".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    f.store
        .upsert_refresh_family(RefreshTokenFamily {
            id: "fam-2".into(),
            user_id: "hel-two".into(),
            ..fam
        })
        .await
        .unwrap();
    let status = f
        .send(
            Method::DELETE,
            "/api/v1/realms/dev/scim/v2/Users/hel-two",
            &admin,
            None,
        )
        .await;
    assert!(status.is_success(), "{status}");
    let family = f.store.get_refresh_family("dev", "fam-2").await.unwrap();
    assert!(family.as_ref().is_none_or(|fam| fam.revoked), "{family:?}");

    f.store
        .create_user(UserRepresentation {
            id: "squatter".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            username: "service-account-hel-app".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        f.send(
            Method::GET,
            "/admin/realms/dev/clients/uuid-hel-app/service-account-user",
            &admin,
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

/// Only an existing, enabled realm's keys verify an admin token: a disabled realm's tokens stop
/// working, and a token on an unknown realm's path is refused before any key is read for it.
#[tokio::test]
async fn a_disabled_or_unknown_realm_verifies_no_admin_token() {
    let f = Fixture::new().await;
    let claims = json!({ "realm_access": { "roles": ["realm-admin"] } });
    let t = f.token("dev", "boss", claims).await;
    assert_eq!(
        f.send(Method::GET, "/admin/realms/dev/users", &t, None)
            .await,
        StatusCode::OK
    );
    let mut dev = f.store.get_realm("dev").await.unwrap().unwrap();
    dev.enabled = false;
    f.store.upsert_realm(dev).await.unwrap();
    assert_eq!(
        f.send(Method::GET, "/admin/realms/dev/users", &t, None)
            .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(Method::GET, "/admin/realms/no-such-realm/users", &t, None)
            .await,
        StatusCode::UNAUTHORIZED
    );
    assert!(f.store.get_realm("no-such-realm").await.unwrap().is_none());
}

/// Scope links, mappers, role holders and group members of an application or group were served
/// to any signed-in member of its organization, and a member could attach any client scope (an
/// audience, a role) to an application of the organization.
#[tokio::test]
async fn application_and_group_subresources_are_checked_by_the_authorizer() {
    let f = Fixture::new().await;
    f.store
        .upsert_client_scope(ClientScopeRepresentation {
            id: "sc-secrets".into(),
            realm_id: "dev".into(),
            name: "secrets:read".into(),
            protocol: "openid-connect".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    f.store
        .upsert_group(GroupRepresentation {
            id: "g-hel".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            name: "ops".into(),
            path: "/helsinki/ops".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let plain = f
        .token("dev", "plain-1", json!({ "jc_org": ["helsinki"] }))
        .await;
    let base = "/admin/realms/dev/clients/uuid-hel-app";
    for (method, uri) in [
        (
            Method::PUT,
            format!("{base}/default-client-scopes/sc-secrets"),
        ),
        (
            Method::PUT,
            format!("{base}/optional-client-scopes/sc-secrets"),
        ),
        (
            Method::DELETE,
            format!("{base}/default-client-scopes/sc-secrets"),
        ),
        (
            Method::DELETE,
            format!("{base}/optional-client-scopes/sc-secrets"),
        ),
        (Method::GET, format!("{base}/default-client-scopes")),
        (Method::GET, format!("{base}/optional-client-scopes")),
        (Method::GET, format!("{base}/protocol-mappers/models")),
        (Method::GET, format!("{base}/roles/viewer/users")),
        (Method::GET, format!("{base}/roles/viewer/groups")),
        (
            Method::GET,
            "/admin/realms/dev/groups/g-hel/members".to_owned(),
        ),
    ] {
        let st = f.send(method.clone(), &uri, &plain, None).await;
        assert!(
            st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
            "{method} {uri} answered {st} to a plain member"
        );
    }
    let app = f
        .store
        .get_client_by_uuid("dev", None, "uuid-hel-app")
        .await
        .unwrap()
        .unwrap();
    assert!(
        !app.default_client_scopes
            .iter()
            .any(|s| s == "secrets:read")
    );
    assert!(
        !app.optional_client_scopes
            .iter()
            .any(|s| s == "secrets:read")
    );

    // The organization's admin still manages its application.
    let admin = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let st = f
        .send(
            Method::PUT,
            &format!("{base}/default-client-scopes/sc-secrets"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let st = f
        .send(
            Method::GET,
            "/admin/realms/dev/groups/g-hel/members",
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
}

/// K3: Proved red against pre-K3 where `view-clients` was permitted to create, update, and delete
/// client roles and client scopes on compat routes.
/// `view-clients` (portal-api) writes and reads the secret of the clients the Portal marked
/// `managed-by: joinedcontext` only: an unmarked client is not adopted, its secret stays out of
/// the client's representation and the list, and an edit cannot drop the mark.
#[tokio::test]
async fn view_clients_neither_adopts_unmarked_clients_nor_reads_their_secret() {
    let f = Fixture::new().await;
    for (id, managed) in [("uuid-bare", None), ("uuid-portal", Some("joinedcontext"))] {
        f.store
            .upsert_client(ClientRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                client_id: id.trim_start_matches("uuid-").into(),
                secret: Some(format!("secret-of-{id}")),
                managed_by: managed.map(str::to_owned),
                attributes: managed
                    .map(|m| {
                        std::collections::BTreeMap::from([("managed-by".to_owned(), m.to_owned())])
                    })
                    .unwrap_or_default(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let t = f
        .token(
            "dev",
            "portal-svc",
            json!({ "resource_access": { "realm-management": { "roles": ["view-clients"] } } }),
        )
        .await;
    let get = |uri: String| {
        let app = f.app.clone();
        let t = t.clone();
        async move {
            let req = Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {t}"))
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let st = resp.status();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            (st, String::from_utf8_lossy(&body).into_owned())
        }
    };
    let (st, _) = get("/admin/realms/dev/clients/uuid-bare/client-secret".into()).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, body) = get("/admin/realms/dev/clients/uuid-bare".into()).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!body.contains("secret-of-uuid-bare"), "{body}");
    let (st, body) = get("/admin/realms/dev/clients".into()).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        !body.contains("secret-of-uuid-bare") && body.contains("secret-of-uuid-portal"),
        "{body}"
    );
    let (st, body) = get("/admin/realms/dev/clients/uuid-portal/client-secret".into()).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("secret-of-uuid-portal"), "{body}");
    // No write on the unmarked client, no edit that drops the mark of the Portal's own.
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/clients/uuid-bare",
            &t,
            Some(json!({ "clientId": "bare", "attributes": { "managed-by": "joinedcontext" } })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/clients/uuid-portal",
            &t,
            Some(json!({ "clientId": "portal", "attributes": {} })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/clients/uuid-portal",
            &t,
            Some(json!({ "clientId": "portal", "attributes": { "managed-by": "joinedcontext" } })),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes() {
    let f = Fixture::new().await;
    // A platform client portal-api does not own (T-2725: not `managed-by: joinedcontext`).
    f.store
        .upsert_client(ClientRepresentation {
            id: "uuid-hel-edge".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            client_id: "hel-edge".into(),
            managed_by: Some("platform".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    f.store
        .upsert_client_scope(ClientScopeRepresentation {
            id: "sc-existing".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            name: "existing-scope".into(),
            protocol: "openid-connect".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let view_clients_tok = f
        .token(
            "dev",
            "portal-svc",
            json!({
                "jc_org": ["helsinki"],
                "resource_access": { "realm-management": { "roles": ["view-clients"] } }
            }),
        )
        .await;
    let manage_clients_tok = f
        .token(
            "dev",
            "client-mgr",
            json!({
                "jc_org": ["helsinki"],
                "resource_access": { "realm-management": { "roles": ["manage-clients"] } }
            }),
        )
        .await;

    // 1. view-clients may GET roles and client-scopes, but POST/PUT/DELETE are denied (404).
    assert_eq!(
        f.send(
            Method::GET,
            "/admin/realms/dev/clients/uuid-hel-app/roles",
            &view_clients_tok,
            None
        )
        .await,
        StatusCode::OK
    );
    for (method, uri, body) in [
        (
            Method::POST,
            "/admin/realms/dev/clients/uuid-hel-edge/roles",
            Some(json!({ "name": "forged-role" })),
        ),
        (
            Method::DELETE,
            "/admin/realms/dev/clients/uuid-hel-edge/roles/viewer",
            None,
        ),
    ] {
        assert_eq!(
            f.send(method.clone(), uri, &view_clients_tok, body).await,
            StatusCode::NOT_FOUND,
            "{method} {uri}"
        );
    }
    // The T-2725 grant stays: portal-api writes the roles of the App clients it manages
    // (Portal reconciler, COMPAT rows 32-33).
    assert_eq!(
        f.send(
            Method::POST,
            "/admin/realms/dev/clients/uuid-hel-app/roles",
            &view_clients_tok,
            Some(json!({ "name": "app-role" }))
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        f.send(
            Method::DELETE,
            "/admin/realms/dev/clients/uuid-hel-app/roles/app-role",
            &view_clients_tok,
            None
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.send(
            Method::GET,
            "/admin/realms/dev/client-scopes",
            &view_clients_tok,
            None
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        f.send(
            Method::POST,
            "/admin/realms/dev/client-scopes",
            &view_clients_tok,
            Some(json!({ "name": "forged-scope", "protocol": "openid-connect" }))
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.send(
            Method::PUT,
            "/admin/realms/dev/client-scopes/sc-existing",
            &view_clients_tok,
            Some(json!({ "name": "tampered-scope", "protocol": "openid-connect" }))
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.send(
            Method::DELETE,
            "/admin/realms/dev/client-scopes/sc-existing",
            &view_clients_tok,
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );

    // 2. manage-clients can create/delete roles and create/update/delete client-scopes in its org.
    assert_eq!(
        f.send(
            Method::POST,
            "/admin/realms/dev/clients/uuid-hel-app/roles",
            &manage_clients_tok,
            Some(json!({ "name": "valid-role" }))
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        f.send(
            Method::DELETE,
            "/admin/realms/dev/clients/uuid-hel-app/roles/valid-role",
            &manage_clients_tok,
            None
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.send(
            Method::PUT,
            "/admin/realms/dev/client-scopes/sc-existing",
            &manage_clients_tok,
            Some(json!({ "name": "existing-scope", "description": "updated", "protocol": "openid-connect" }))
        )
        .await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn org_admin_post_creates_no_role_mappings() {
    let f = Fixture::new().await;
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    // A realm-wide group in the body is refused (as Keycloak refuses a group the caller may not
    // manage members of), and nobody is created.
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/users",
            &t,
            Some(json!({ "username": "made-admin", "enabled": true,
                         "groups": ["/platform-admins"] })),
        )
        .await;
    assert!(st.is_client_error(), "{st}");
    assert!(
        f.store
            .get_user_by_username_or_email("dev", None, "made-admin")
            .await
            .unwrap()
            .is_none()
    );
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/users",
            &t,
            Some(
                json!({ "id": "esp-person", "username": "made-admin", "enabled": true,
                         "realmRoles": ["realm-admin"],
                         "clientRoles": { "realm-management": ["realm-admin"] },
                         "serviceAccountClientId": "realm-management" }),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let u = f
        .store
        .get_user_by_username_or_email("dev", None, "made-admin")
        .await
        .unwrap()
        .unwrap();
    // Keycloak's POST /users writes no role mapping from the body; the person gets the realm's
    // default roles only.
    assert_eq!(u.realm_roles, ["offline_access", "uma_authorization"]);
    assert!(u.client_roles.is_empty(), "{:?}", u.client_roles);
    assert_eq!(u.service_account_client_id, None);
    assert_eq!(u.org_id.as_deref(), Some("helsinki"));
    assert!(u.groups.is_empty(), "{:?}", u.groups);
    assert_ne!(u.id, "esp-person", "an organization admin picks no id");
    assert_eq!(f.user("esp-person").await.username, "esp-person");
}
