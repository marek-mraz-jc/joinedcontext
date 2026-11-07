#![forbid(unsafe_code)]
//! Admin authorization regressions from the 2026-11-07 review (W6). Each failed before its fix:
//! - any person could PUT their own user with `realmRoles: ["realm-admin"]` (self-service write),
//! - an organization admin could hand out `realm-management` roles and move people between
//!   organizations through PUT,
//! - role-mapping deletes were checked against the caller's own organization, not the person's,
//! - a same-id person of the `master` realm lent its roles to a token of another realm,
//! - a department realm role called `platform-admin` (or `admin`) made its holder realm admin,
//! - a revoked admin token kept working.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
use wardkeep_admin::{AdminState, admin_router};
use wardkeep_core::*;
use wardkeep_crypto::{Kek, sha256_hex, sign_jwt};
use wardkeep_oidc::{OidcState, OutboundGuard};
use wardkeep_store_memory::MemoryStore;

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
                ..Default::default()
            })
            .await
            .unwrap();
        let oidc = OidcState {
            store: store.clone(),
            kek,
            guard: OutboundGuard::default(),
            default_mount: Mount::default(),
        };
        let app = admin_router(AdminState::new(oidc, Arc::new(LocalAuthorizer)));
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
