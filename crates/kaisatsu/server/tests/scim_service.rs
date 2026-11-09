#![forbid(unsafe_code)]
//! SCIM 2.0 (W11). Regression: `DELETE`/`GET /Users/{id}`, `GET`/`POST /Groups` ran no
//! authorization at all, so any signed-in member could delete or read every person of their
//! organization and create groups; a group name with `/` landed in another path.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;
use common::*;

const ISSUER: &str = "https://idm.dev.example.test/realms/dev";
const SCIM: &str = "/api/v1/realms/dev/scim/v2";
const MEMBER: &str = "de300000-0000-4000-8000-00000000000f";

struct Api {
    app: axum::Router,
    store: std::sync::Arc<kaisatsu_store_memory::MemoryStore>,
}

impl Api {
    async fn new() -> Self {
        let (state, store, _) = setup_dev_realm_state().await;
        let mount = Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap();
        Self {
            app: router(state, mount),
            store,
        }
    }

    async fn token(&self, sub: &str, extra: Value) -> String {
        let keys = self.store.list_signing_keys("dev").await.unwrap();
        let key = keys.iter().find(|k| k.status == "ACTIVE").unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut claims = json!({ "iss": ISSUER, "sub": sub, "azp": "admin-cli", "typ": "Bearer",
                                 "iat": now, "exp": now + 600 });
        for (k, v) in extra.as_object().unwrap() {
            claims[k] = v.clone();
        }
        sign_jwt(key, self.store.kek(), Some("JWT"), &claims).unwrap()
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let req = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/scim+json");
                b.body(Body::from(v.to_string())).unwrap()
            }
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn call_with_if_match(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        if_match: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::IF_MATCH, if_match)
            .header(header::CONTENT_TYPE, "application/scim+json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

async fn colleague(api: &Api) -> String {
    let u = api
        .store
        .create_user(UserRepresentation {
            id: "scim-colleague".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            username: "colleague@hel.fi".into(),
            email: Some("colleague@hel.fi".into()),
            enabled: true,
            ..Default::default()
        })
        .await
        .unwrap();
    u.id
}

#[tokio::test]
async fn a_member_without_rights_can_neither_read_delete_nor_create() {
    let api = Api::new().await;
    let id = colleague(&api).await;
    let member = api.token(MEMBER, json!({ "jc_org": ["helsinki"] })).await;

    let (st, _) = api
        .call(Method::DELETE, &format!("{SCIM}/Users/{id}"), &member, None)
        .await;
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
        "{st}"
    );
    assert!(
        api.store
            .get_user_by_id("dev", None, &id)
            .await
            .unwrap()
            .is_some(),
        "a member deleted a colleague"
    );
    let (st, body) = api
        .call(Method::GET, &format!("{SCIM}/Users/{id}"), &member, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
    let (st, _) = api
        .call(Method::GET, &format!("{SCIM}/Users"), &member, None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = api
        .call(Method::GET, &format!("{SCIM}/Groups"), &member, None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Groups"),
            &member,
            Some(json!({ "displayName": "shadow-admins" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &member,
            Some(json!({ "userName": "intruder@hel.fi" })),
        )
        .await;
    // The registry's decision, the same REST and OData give: refused, nothing created.
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
        "{st}"
    );
    assert!(
        api.store
            .get_user_by_username_or_email("dev", None, "intruder@hel.fi")
            .await
            .unwrap()
            .is_none()
    );
    // A person still reads their own record.
    let (st, me) = api
        .call(
            Method::GET,
            &format!("{SCIM}/Users/{MEMBER}"),
            &member,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["userName"], "demo.orgmember@hel.fi");
}

#[tokio::test]
async fn an_admin_manages_users_and_groups_through_scim() {
    let api = Api::new().await;
    let admin = api
        .token(
            "admin-1",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;

    let (st, created) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &admin,
            Some(json!({ "userName": "scim.user@hel.fi", "active": true,
                         "name": { "givenName": "Sára", "familyName": "Kim" },
                         "emails": [{ "value": "scim.user@hel.fi" }] })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["name"]["givenName"], "Sára");
    let stored = api
        .store
        .get_user_by_id("dev", None, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !stored.email_verified,
        "SCIM carries no proof of the address"
    );
    let events = api.store.list_admin_events("dev", None).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.resource_path == format!("users/{id}"))
    );

    let (st, list) = api
        .call(
            Method::GET,
            &format!("{SCIM}/Users?filter=userName%20eq%20%22scim.user@hel.fi%22"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["totalResults"], 1);
    assert_eq!(list["Resources"][0]["id"], id.as_str());
    let (st, _) = api
        .call(Method::GET, &format!("{SCIM}/Users/{id}"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &admin,
            Some(json!({ "userName": " " })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = api
        .call(Method::DELETE, &format!("{SCIM}/Users/{id}"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = api
        .call(Method::GET, &format!("{SCIM}/Users/{id}"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = api
        .call(Method::DELETE, &format!("{SCIM}/Users/{id}"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    for bad in ["helsinki/admins", "", "  "] {
        let (st, _) = api
            .call(
                Method::POST,
                &format!("{SCIM}/Groups"),
                &admin,
                Some(json!({ "displayName": bad })),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let (st, g) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Groups"),
            &admin,
            Some(json!({ "displayName": "Night Shift" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{g}");
    let (st, groups) = api
        .call(
            Method::GET,
            &format!("{SCIM}/Groups?filter=displayName%20eq%20%22Night%20Shift%22"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(groups["totalResults"], 1);
    assert_eq!(groups["Resources"][0]["id"], g["id"]);
    // A filter nested past the depth limit is refused, not a stack overflow.
    let deep = format!("{}userName eq \"x\"{}", "(".repeat(64), ")".repeat(64));
    let (st, _) = api
        .call(
            Method::GET,
            &format!(
                "{SCIM}/Users?filter={}",
                url::form_urlencoded::byte_serialize(deep.as_bytes()).collect::<String>()
            ),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// K14: Tests SCIM PUT/PATCH on Users and Groups, group membership add/remove/replace with
/// `members[value eq "..."]`, `emails[type eq "work"].value`, `If-Match` ETags, `scimType`
/// errors (`invalidPath`, `noTarget`, `mutability`, `invalidValue`), `deny_unknown_fields`,
/// 404 outside scope, and `active=false` ending SSO sessions.
#[tokio::test]
async fn k14_scim_put_patch_groups_etags_errors_and_deactivation_session_revocation() {
    let api = Api::new().await;
    let admin = api
        .token(
            "admin-1",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;

    // 1. Create a user and a active SSO session for them.
    let (st, u_created) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &admin,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": "bjensen@hel.fi",
                "externalId": "ext-bj",
                "active": true,
                "name": { "givenName": "Barbara", "familyName": "Jensen" },
                "emails": [
                    { "value": "bjensen@hel.fi", "type": "work", "primary": true },
                    { "value": "babs@home.org", "type": "home", "primary": false }
                ]
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{u_created}");
    let uid = u_created["id"].as_str().unwrap().to_owned();
    let etag1 = u_created["meta"]["version"].as_str().unwrap().to_owned();
    assert!(etag1.starts_with("W/\""));

    let now = chrono::Utc::now();
    api.store
        .upsert_sso_session(SsoSession {
            id: "sso-bj-1".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            handle_hash: sha256_hex("h-bj"),
            user_id: uid.clone(),
            username: "bjensen@hel.fi".into(),
            ip_address: "127.0.0.1".into(),
            user_agent: String::new(),
            start: now.timestamp_millis(),
            last_access: now.timestamp_millis(),
            auth_time: now.timestamp(),
            acr: "pwd".into(),
            amr: vec!["pwd".into()],
            clients: Default::default(),
            idle_expires_at: now + chrono::Duration::hours(1),
            max_expires_at: now + chrono::Duration::hours(10),
        })
        .await
        .unwrap();

    // 2. Unknown fields on User body are refused (`deny_unknown_fields`).
    let (st_unk, err_unk) = api
        .call(
            Method::PUT,
            &format!("{SCIM}/Users/{uid}"),
            &admin,
            Some(json!({ "userName": "bjensen@hel.fi", "surpriseField": 123 })),
        )
        .await;
    assert_eq!(st_unk, StatusCode::BAD_REQUEST);
    assert_eq!(err_unk["scimType"], "invalidValue");

    // 2b. A stale If-Match ETag is refused with 412; the current one passes.
    let (st_stale, _) = api
        .call_with_if_match(
            Method::PUT,
            &format!("{SCIM}/Users/{uid}"),
            &admin,
            "W/\"0000000000000000\"",
            json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": "bjensen@hel.fi",
                "externalId": "ext-bj",
                "active": true,
                "name": { "givenName": "Barbara", "familyName": "Jensen" },
                "emails": [
                    { "value": "bjensen@hel.fi", "type": "work", "primary": true },
                    { "value": "babs@home.org", "type": "home", "primary": false }
                ]
            }),
        )
        .await;
    assert_eq!(st_stale, StatusCode::PRECONDITION_FAILED);
    let (st_cur, _) = api
        .call_with_if_match(
            Method::PUT,
            &format!("{SCIM}/Users/{uid}"),
            &admin,
            &etag1,
            json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": "bjensen@hel.fi",
                "externalId": "ext-bj",
                "active": true,
                "name": { "givenName": "Barbara", "familyName": "Jensen" },
                "emails": [
                    { "value": "bjensen@hel.fi", "type": "work", "primary": true },
                    { "value": "babs@home.org", "type": "home", "primary": false }
                ]
            }),
        )
        .await;
    assert_eq!(st_cur, StatusCode::OK);

    // 3. Changing immutable `id` on PUT -> 400 scimType="mutability".
    let (st_mut, err_mut) = api
        .call(
            Method::PUT,
            &format!("{SCIM}/Users/{uid}"),
            &admin,
            Some(json!({ "id": "different-id", "userName": "bjensen@hel.fi" })),
        )
        .await;
    assert_eq!(st_mut, StatusCode::BAD_REQUEST);
    assert_eq!(err_mut["scimType"], "mutability");

    // 4. PATCH User with valuePath `emails[type eq "work"].value`, plain attributes, and error cases.
    let (st_patch, patched_u) = api
        .call(
            Method::PATCH,
            &format!("{SCIM}/Users/{uid}"),
            &admin,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [
                    { "op": "replace", "path": "emails[type eq \"work\"].value", "value": "b.jensen@hel.fi" },
                    { "op": "replace", "path": "name.givenName", "value": "Babs" },
                    { "op": "add", "value": { "nickName": "BJ" } }
                ]
            })),
        )
        .await;
    assert_eq!(st_patch, StatusCode::OK, "{patched_u}");
    assert_eq!(patched_u["name"]["givenName"], "Babs");
    assert_eq!(patched_u["nickName"], "BJ");
    let work_email = patched_u["emails"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "work")
        .unwrap();
    assert_eq!(work_email["value"], "b.jensen@hel.fi");

    // PATCH error cases: remove without path -> noTarget, unknown path -> invalidPath, remove required userName -> mutability.
    for (ops, expected_type) in [
        (json!([{ "op": "remove" }]), "noTarget"),
        (
            json!([{ "op": "replace", "path": "emails[type eq \"fax\"].value", "value": "x@y" }]),
            "noTarget",
        ),
        (
            json!([{ "op": "replace", "path": "nonExistentAttr", "value": "x" }]),
            "invalidPath",
        ),
        (
            json!([{ "op": "remove", "path": "userName" }]),
            "mutability",
        ),
    ] {
        let (st_err, body_err) = api
            .call(
                Method::PATCH,
                &format!("{SCIM}/Users/{uid}"),
                &admin,
                Some(json!({
                    "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                    "Operations": ops
                })),
            )
            .await;
        assert_eq!(st_err, StatusCode::BAD_REQUEST, "{body_err}");
        assert_eq!(body_err["scimType"], expected_type);
    }

    // 5. Deactivating user (`active=false`) via PATCH revokes their SSO sessions.
    let (st_deact, _) = api
        .call(
            Method::PATCH,
            &format!("{SCIM}/Users/{uid}"),
            &admin,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{ "op": "replace", "path": "active", "value": false }]
            })),
        )
        .await;
    assert_eq!(st_deact, StatusCode::OK);
    assert!(
        api.store
            .get_sso_session("dev", "sso-bj-1")
            .await
            .unwrap()
            .is_none(),
        "active=false must end the user's SSO sessions"
    );

    // 6. Groups lifecycle: create with members, GET, PATCH add/remove `members[value eq "..."]`, PUT, DELETE.
    let (st_g, g_created) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Groups"),
            &admin,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
                "displayName": "Engineering",
                "members": [{ "value": uid }]
            })),
        )
        .await;
    assert_eq!(st_g, StatusCode::CREATED, "{g_created}");
    let gid = g_created["id"].as_str().unwrap().to_owned();
    assert_eq!(g_created["members"].as_array().unwrap().len(), 1);

    // PATCH Group: add MEMBER, then remove `uid` via `members[value eq "..."]`.
    let (st_gp, g_patched) = api
        .call(
            Method::PATCH,
            &format!("{SCIM}/Groups/{gid}"),
            &admin,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [
                    { "op": "add", "path": "members", "value": [{ "value": MEMBER }] },
                    { "op": "remove", "path": format!("members[value eq \"{uid}\"]") }
                ]
            })),
        )
        .await;
    assert_eq!(st_gp, StatusCode::OK, "{g_patched}");
    let g_members = g_patched["members"].as_array().unwrap();
    assert_eq!(g_members.len(), 1);
    assert_eq!(g_members[0]["value"], MEMBER);

    // PUT Group replaces displayName and members.
    let (st_gput, g_put) = api
        .call(
            Method::PUT,
            &format!("{SCIM}/Groups/{gid}"),
            &admin,
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
                "displayName": "Platform Engineering",
                "members": []
            })),
        )
        .await;
    assert_eq!(st_gput, StatusCode::OK, "{g_put}");
    assert_eq!(g_put["displayName"], "Platform Engineering");
    assert!(g_put["members"].as_array().unwrap().is_empty());

    // DELETE Group
    let (st_gdel, _) = api
        .call(
            Method::DELETE,
            &format!("{SCIM}/Groups/{gid}"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st_gdel, StatusCode::NO_CONTENT);
    let (st_g404, _) = api
        .call(Method::GET, &format!("{SCIM}/Groups/{gid}"), &admin, None)
        .await;
    assert_eq!(st_g404, StatusCode::NOT_FOUND);
}
