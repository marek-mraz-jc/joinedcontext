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
use kaisatsu_admin::{AdminState, admin_router, odata_router, scim_router};
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
                    created_at: jc_time::TimestampTz::now(),
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
        let app = admin_router(state.clone())
            .merge(scim_router(state.clone()))
            .merge(odata_router(state));
        Self { app, store }
    }

    async fn token(&self, realm: &str, sub: &str, extra: Value) -> String {
        let keys = self.store.list_signing_keys(realm).await.unwrap();
        let now = jc_time::TimestampTz::now().as_second();
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

/// An Organization's client manager links to its client only scopes it may read (built-ins or
/// its own Organization's), and never sets the attributes that put claims into the client's
/// tokens.
#[tokio::test]
async fn an_org_client_manager_links_only_its_own_scopes_and_no_reserved_attribute() {
    let f = Fixture::new().await;
    f.store
        .upsert_client_scope(ClientScopeRepresentation {
            id: "scope-esp".into(),
            realm_id: "dev".into(),
            org_id: Some("espoo".into()),
            name: "esp-scope".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    for kind in ["optional", "default"] {
        for id in ["scope-esp", "esp-scope", "no-such-scope"] {
            let path = format!("/admin/realms/dev/clients/uuid-hel-app/{kind}-client-scopes/{id}");
            let st = f.send(Method::PUT, &path, &t, None).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
        }
    }
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/clients/uuid-hel-app/optional-client-scopes/profile",
            &t,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "a built-in scope links");
    let app = f
        .store
        .get_client_by_uuid("dev", None, "uuid-hel-app")
        .await
        .unwrap()
        .unwrap();
    assert!(app.optional_client_scopes.contains(&"profile".to_owned()));

    let mark = json!({ "managed-by": "joinedcontext" });
    for (key, value) in [
        ("jc.project", "01JCPROJECT0000000000000000"),
        ("jc.kind", "serviceAccount"),
        ("kaisatsu.linked-scope-prefix", "endpoint:"),
    ] {
        let mut attributes = mark.clone();
        attributes[key] = json!(value);
        let st = f
            .send(
                Method::PUT,
                "/admin/realms/dev/clients/uuid-hel-app",
                &t,
                Some(json!({ "clientId": "hel-app", "attributes": attributes })),
            )
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{key}");
        let st = f
            .send(
                Method::POST,
                "/admin/realms/dev/clients",
                &t,
                Some(json!({ "clientId": format!("new-{key}"), "attributes": attributes })),
            )
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{key}");
    }
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/clients",
            &t,
            Some(json!({ "clientId": "hel-new", "optionalClientScopes": ["esp-scope"] })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/clients/uuid-hel-app",
            &t,
            Some(json!({ "clientId": "hel-app", "attributes": mark, "defaultClientScopes": ["esp-scope"] })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let app = f
        .store
        .get_client_by_uuid("dev", None, "uuid-hel-app")
        .await
        .unwrap()
        .unwrap();
    assert!(!app.attributes.contains_key("jc.project"));
    assert!(!app.default_client_scopes.contains(&"esp-scope".to_owned()));
}

/// A client-role mapping naming a client that does not exist is refused, and is never stored
/// as a realm role of the same name.
#[tokio::test]
async fn a_mapping_to_an_unknown_client_grants_no_realm_role() {
    let f = Fixture::new().await;
    let admin = f
        .token(
            "dev",
            "realm-op",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/users/plain-1/role-mappings/clients/stale-uuid",
            &admin,
            Some(json!([{ "name": "realm-admin" }])),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(f.user("plain-1").await.realm_roles.is_empty());
    let err = f
        .store
        .add_role_mapping(RoleMappingRecord {
            id: String::new(),
            realm_id: "dev".into(),
            org_id: None,
            principal_kind: "user".into(),
            principal_id: "plain-1".into(),
            role_id: String::new(),
            role_name: "realm-admin".into(),
            client_uuid: Some("stale-uuid".into()),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, KaisatsuError::NotFound(_)), "{err:?}");
    assert!(f.user("plain-1").await.realm_roles.is_empty());
}

/// A disabled person's bearer, or one whose SSO session ended, opens no admin route.
#[tokio::test]
async fn the_admin_api_refuses_a_disabled_person_or_an_ended_session() {
    let f = Fixture::new().await;
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let path = "/admin/realms/dev/users/hel-person";
    assert_eq!(f.send(Method::GET, path, &t, None).await, StatusCode::OK);
    let ended = f
        .token(
            "dev",
            "org-admin-hel",
            json!({ "jc_org": ["helsinki"], "sid": "gone" }),
        )
        .await;
    assert_eq!(
        f.send(Method::GET, path, &ended, None).await,
        StatusCode::UNAUTHORIZED
    );
    let mut admin = f.user("org-admin-hel").await;
    admin.enabled = false;
    f.store.update_user(admin).await.unwrap();
    assert_eq!(
        f.send(Method::GET, path, &t, None).await,
        StatusCode::UNAUTHORIZED
    );
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
            enabled: true,
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
        (
            Method::PUT,
            "/api/v1/realms/dev/scim/v2/Users/hel-root",
            Some(json!({ "userName": "hel-root", "active": false,
                         "emails": [{ "value": "other@example.test", "primary": true }] })),
        ),
        (
            Method::PATCH,
            "/api/v1/realms/dev/scim/v2/Users/hel-root",
            Some(
                json!({ "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                         "Operations": [{ "op": "replace", "path": "active", "value": false }] }),
            ),
        ),
    ] {
        let st = f.send(method.clone(), path, &t, body).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{method} {path}");
    }
    let root = f.user("hel-root").await;
    assert!(root.enabled && root.email.is_none());
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
            expires_at: jc_time::TimestampTz::now() + jiff::SignedDuration::from_hours(1),
        })
        .await
        .unwrap();
    assert_eq!(
        f.send(Method::GET, "/admin/realms/dev/users", &t, None)
            .await,
        StatusCode::UNAUTHORIZED
    );
}

/// A live refresh family of `hel-person` in `hel-app`.
fn live_family(id: &str, sso: &str) -> RefreshTokenFamily {
    let now = jc_time::TimestampTz::now();
    RefreshTokenFamily {
        id: id.into(),
        realm_id: "dev".into(),
        org_id: Some("helsinki".into()),
        sso_session_id: sso.into(),
        user_id: "hel-person".into(),
        client_id: "hel-app".into(),
        scope: "openid".into(),
        mapper_scopes: Vec::new(),
        acr: "pwd".into(),
        amr: Vec::new(),
        auth_time: now.as_second(),
        rotation_counter: 0,
        current_jti_hash: "h".into(),
        previous_hashes: Vec::new(),
        dpop_jkt: None,
        offline: false,
        revoked: false,
        expires_at: now + jiff::SignedDuration::from_hours(1),
        act: None,
        only_audience: None,
    }
}

/// Disabling a person through the Admin REST PUT or an OData update ends their sessions as SCIM
/// deactivation does (S-4-10); an update that leaves them enabled ends nothing.
#[tokio::test]
async fn disabling_a_person_by_admin_rest_or_odata_ends_their_sessions() {
    let f = Fixture::new().await;
    let admin = f
        .token(
            "dev",
            "boss",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;
    let live = async |id: &str| {
        let fam = f.store.get_refresh_family("dev", id).await.unwrap();
        fam.is_some_and(|fam| !fam.revoked)
    };

    f.store
        .upsert_refresh_family(live_family("fam-rest", "sso-rest"))
        .await
        .unwrap();
    let put = async |enabled: bool| {
        f.send(
            Method::PUT,
            "/admin/realms/dev/users/hel-person",
            &admin,
            Some(json!({ "enabled": enabled })),
        )
        .await
    };
    assert_eq!(put(true).await, StatusCode::NO_CONTENT);
    assert!(live("fam-rest").await, "an enabled update ended a session");
    assert_eq!(put(false).await, StatusCode::NO_CONTENT);
    assert!(!live("fam-rest").await, "a disabled person kept a session");

    f.store
        .upsert_refresh_family(live_family("fam-odata", "sso-odata"))
        .await
        .unwrap();
    let patched = f
        .send(
            Method::PATCH,
            "/api/v1/realms/dev/odata/People('hel-person')",
            &admin,
            Some(json!({ "enabled": false })),
        )
        .await;
    assert!(patched.is_success(), "{patched}");
    assert!(!live("fam-odata").await, "a disabled person kept a session");
}

/// Deleting a person ends their sessions (refresh families revoked, clients told); a client's
/// service-account user is its own, never a person who took the name.
#[tokio::test]
async fn person_delete_and_service_account_user_follow_the_person() {
    let f = Fixture::new().await;
    let now = jc_time::TimestampTz::now();
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
        auth_time: now.as_second(),
        rotation_counter: 0,
        current_jti_hash: "h".into(),
        previous_hashes: Vec::new(),
        dpop_jkt: None,
        offline: false,
        revoked: false,
        expires_at: now + jiff::SignedDuration::from_hours(1),
        act: None,
        only_audience: None,
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

#[tokio::test]
async fn a_people_manager_cannot_change_an_organization_admin() {
    let f = Fixture::new().await;
    f.store
        .upsert_membership(Membership {
            id: "m-pm-hel".into(),
            realm_id: "dev".into(),
            org_id: "helsinki".into(),
            person_id: "pm-hel".into(),
            org_roles: vec!["people-manager".into()],
            created_at: jc_time::TimestampTz::now(),
        })
        .await
        .unwrap();
    let t = f
        .token("dev", "pm-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let reset = json!({ "type": "password", "value": "Another-pass-123", "temporary": false });
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/org-admin-hel/reset-password",
            &t,
            Some(reset.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/hel-person/reset-password",
            &t,
            Some(reset.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    // The Organization's admin still resets its people manager.
    let admin = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    f.store
        .create_user(UserRepresentation {
            id: "pm-hel".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            username: "pm-hel".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/pm-hel/reset-password",
            &admin,
            Some(reset),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn joining_a_group_with_realm_wide_roles_needs_a_user_manager() {
    let f = Fixture::new().await;
    for (id, roles) in [
        ("g-admins", vec!["realm-admin".to_owned()]),
        ("g-staff", Vec::new()),
    ] {
        f.store
            .upsert_group(GroupRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                org_id: Some("helsinki".into()),
                name: id.into(),
                path: format!("/helsinki/{id}"),
                realm_roles: roles,
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/org-admin-hel/groups/g-admins",
            &t,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let st = f
        .send(
            Method::PUT,
            "/api/v1/realms/dev/scim/v2/Groups/g-admins",
            &t,
            Some(json!({ "displayName": "g-admins", "members": [{ "value": "org-admin-hel" }] })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(
        f.store
            .list_user_groups("dev", "org-admin-hel")
            .await
            .unwrap()
            .is_empty()
    );
    // A member gets the roles of the group's ancestors and of its role mappings too.
    for (id, parent) in [("g-child", Some("g-admins")), ("g-mapped", None)] {
        f.store
            .upsert_group(GroupRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                org_id: Some("helsinki".into()),
                name: id.into(),
                path: format!("/helsinki/{id}"),
                parent_id: parent.map(str::to_owned),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    f.store
        .add_role_mapping(RoleMappingRecord {
            id: "m-1".into(),
            realm_id: "dev".into(),
            org_id: None,
            principal_kind: "group".into(),
            principal_id: "g-mapped".into(),
            role_id: "realm-admin".into(),
            role_name: "realm-admin".into(),
            client_uuid: None,
        })
        .await
        .unwrap();
    for group in ["g-child", "g-mapped"] {
        let st = f
            .send(
                Method::PUT,
                &format!("/admin/realms/dev/users/org-admin-hel/groups/{group}"),
                &t,
                None,
            )
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{group}");
    }
    // A refused SCIM write changes nothing of the group, and a refused join creates no person.
    let st = f
        .send(
            Method::PUT,
            "/api/v1/realms/dev/scim/v2/Groups/g-admins",
            &t,
            Some(json!({ "displayName": "renamed", "members": [{ "value": "org-admin-hel" }] })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let kept = f
        .store
        .get_group("dev", None, "g-admins")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.name, "g-admins");
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/users",
            &t,
            Some(json!({ "username": "joiner", "groups": ["/helsinki/g-admins"] })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(
        f.store
            .get_user_by_username_or_email("dev", None, "joiner")
            .await
            .unwrap()
            .is_none()
    );
    let st = f
        .send(
            Method::PUT,
            "/admin/realms/dev/users/hel-person/groups/g-staff",
            &t,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_group_name_holds_no_path_separator() {
    let f = Fixture::new().await;
    let t = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    for name in ["espoo/staff", "line\nbreak"] {
        let st = f
            .send(
                Method::POST,
                "/admin/realms/dev/groups",
                &t,
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{name}");
    }
    assert!(
        f.store
            .get_group_by_path("dev", None, "/espoo/staff")
            .await
            .unwrap()
            .is_none()
    );
    let st = f
        .send(
            Method::POST,
            "/admin/realms/dev/groups",
            &t,
            Some(json!({ "name": "field-staff" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
}

/// `GET`/`POST /groups/{id}/children` were missing, so the Portal's `/{organization}/members`
/// sync answered 404 in every Organization realm (T-7385). A group's children are read and made
/// only within its realm and organization.
#[tokio::test]
async fn group_children_stay_within_their_realm_and_organization() {
    let f = Fixture::new().await;
    for (realm, id, name) in [
        ("dev", "g-root", "helsinki"),
        ("master", "g-master", "helsinki"),
    ] {
        f.store
            .upsert_group(GroupRepresentation {
                id: id.into(),
                realm_id: realm.into(),
                name: name.into(),
                path: format!("/{name}"),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let admin = f
        .token(
            "dev",
            "admin-1",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;
    let children = "/admin/realms/dev/groups/g-root/children";
    let members = json!({ "name": "members" });
    let st = f
        .send(Method::POST, children, &admin, Some(members.clone()))
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let st = f
        .send(Method::POST, children, &admin, Some(members.clone()))
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let made = f
        .store
        .get_group_by_path("dev", None, "/helsinki/members")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(made.parent_id.as_deref(), Some("g-root"));
    assert_eq!(made.org_id.as_deref(), Some("helsinki"));

    let req = Request::builder()
        .uri(format!(
            "{children}?briefRepresentation=true&first=0&max=1000"
        ))
        .header("authorization", format!("Bearer {admin}"))
        .body(Body::empty())
        .unwrap();
    let resp = f.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");
    assert_eq!(body[0]["path"], "/helsinki/members");

    // Another realm's group is not found through this realm, and nothing is made there.
    let other = "/admin/realms/dev/groups/g-master/children";
    assert_eq!(
        f.send(Method::GET, other, &admin, None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.send(Method::POST, other, &admin, Some(members.clone()))
            .await,
        StatusCode::NOT_FOUND
    );
    assert!(
        f.store
            .get_group_by_path("master", None, "/helsinki/members")
            .await
            .unwrap()
            .is_none()
    );

    // Another organization's admin neither reads nor adds children; the group's own admin reads.
    let espoo = f
        .token("dev", "org-admin-esp", json!({ "jc_org": ["espoo"] }))
        .await;
    for (method, body) in [
        (Method::GET, None),
        (Method::POST, Some(json!({ "name": "x" }))),
    ] {
        let st = f.send(method.clone(), children, &espoo, body).await;
        assert!(
            st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
            "{method} answered {st} to another organization's admin"
        );
    }
    assert!(
        f.store
            .get_group_by_path("dev", None, "/helsinki/x")
            .await
            .unwrap()
            .is_none()
    );
    let hel = f
        .token("dev", "org-admin-hel", json!({ "jc_org": ["helsinki"] }))
        .await;
    assert_eq!(
        f.send(Method::GET, children, &hel, None).await,
        StatusCode::OK
    );
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
            org_id: Some("helsinki".into()),
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
async fn view_clients_adds_only_an_audience_to_an_imported_client_marked_for_it() {
    let f = Fixture::new().await;
    let import = ProtocolMapperRepresentation {
        id: "m-import".into(),
        realm_id: "dev".into(),
        name: "aud-old".into(),
        protocol: "openid-connect".into(),
        protocol_mapper: "oidc-audience-mapper".into(),
        config: std::collections::BTreeMap::from([(
            "included.custom.audience".to_owned(),
            "old-endpoint".to_owned(),
        )]),
        ..Default::default()
    };
    for (id, mark) in [("uuid-bare", None), ("uuid-pl", Some("joinedcontext"))] {
        f.store
            .upsert_client(ClientRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                client_id: id.trim_start_matches("uuid-").into(),
                managed_by: Some("apply".into()),
                attributes: mark
                    .map(|m| {
                        std::collections::BTreeMap::from([(
                            "audiences-managed-by".to_owned(),
                            m.to_owned(),
                        )])
                    })
                    .unwrap_or_default(),
                protocol_mappers: vec![import.clone()],
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
    let audience = |name: &str, audience: &str| {
        json!({
            "name": name,
            "protocol": "openid-connect",
            "protocolMapper": "oidc-audience-mapper",
            "consentRequired": false,
            "config": {
                "included.custom.audience": audience,
                "access.token.claim": "true",
                "id.token.claim": "false",
                "introspection.token.claim": "true",
            },
        })
    };
    let models = |uuid: &str| format!("/admin/realms/dev/clients/{uuid}/protocol-mappers/models");
    // An unmarked imported client stays nobody's to write.
    let st = f
        .send(
            Method::POST,
            &models("uuid-bare"),
            &t,
            Some(audience("aud-door", "door")),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // Another mapper kind, a client audience or a replaced import mapper: refused on the mark.
    let roles = json!({
        "name": "roles",
        "protocol": "openid-connect",
        "protocolMapper": "oidc-hardcoded-claim-mapper",
        "consentRequired": false,
        "config": { "claim.name": "jc_org", "claim.value": "helsinki" },
    });
    let mut client_audience = audience("aud-client", "door");
    client_audience["config"]["included.client.audience"] = json!("realm-management");
    for body in [roles, client_audience] {
        let st = f
            .send(Method::POST, &models("uuid-pl"), &t, Some(body))
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    let st = f
        .send(
            Method::POST,
            &models("uuid-pl"),
            &t,
            Some(audience("aud-old", "door")),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let st = f
        .send(
            Method::PUT,
            &format!("{}/m-import", models("uuid-pl")),
            &t,
            Some(audience("aud-old", "door")),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // The one write the mark allows: a new audience, the import's mappers kept.
    let st = f
        .send(
            Method::POST,
            &models("uuid-pl"),
            &t,
            Some(audience("aud-door", "door")),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let held = f
        .store
        .get_client_by_uuid("dev", None, "uuid-pl")
        .await
        .unwrap()
        .unwrap();
    let names: Vec<&str> = held
        .protocol_mappers
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["aud-old", "aud-door"]);
    assert_eq!(
        held.protocol_mappers[0].config["included.custom.audience"],
        "old-endpoint"
    );
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
