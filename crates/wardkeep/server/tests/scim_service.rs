#![forbid(unsafe_code)]
//! SCIM 2.0 (W11). Regression: `DELETE`/`GET /Users/{id}`, `GET`/`POST /Groups` ran no
//! authorization at all, so any signed-in member could delete or read every person of their
//! organization and create groups; a group name with `/` landed in another path.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use tower::ServiceExt;
use wardkeep_core::*;
use wardkeep_crypto::*;
use wardkeep_server::router;

mod common;
use common::*;

const ISSUER: &str = "https://idm.dev.example.test/realms/dev";
const SCIM: &str = "/api/v1/realms/dev/scim/v2";
const MEMBER: &str = "de300000-0000-4000-8000-00000000000f";

struct Api {
    app: axum::Router,
    store: std::sync::Arc<wardkeep_store_memory::MemoryStore>,
}

impl Api {
    async fn new() -> Self {
        let (state, store, _) = setup_dev_realm_state().await;
        let mount = Mount::new("wardkeep", "https", "idm.dev.example.test", "").unwrap();
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
