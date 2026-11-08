#![forbid(unsafe_code)]
//! SCIM 2.0 refusals and branches (RFC 7644 §3.4.2.2 filters, §3.5.1 PUT, §3.5.2 PATCH, §3.14
//! ETags, K14) through the router as `jc` mounts it, with the `Mount` extension layered on.
//! Regressions: PUT and PATCH changed a person's address and kept it verified, and a group PATCH
//! accepted any `op` (red before the fixes in `admin/src/scim.rs`).

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
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
const PATCH_OP: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

struct Api {
    app: Router,
    store: std::sync::Arc<kaisatsu_store_memory::MemoryStore>,
}

impl Api {
    async fn new() -> Self {
        let (state, store, _) = setup_dev_realm_state().await;
        let mount = Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap();
        Self {
            app: router(state, mount.clone()).layer(Extension(mount)),
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

    async fn admin(&self) -> String {
        self.token(
            "admin-1",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await
    }

    async fn raw(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Vec<u8>>,
        if_match: Option<&str>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(m) = if_match {
            b = b.header(header::IF_MATCH, m);
        }
        let req = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/scim+json");
                b.body(Body::from(v)).unwrap()
            }
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let h = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (st, h, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (st, _, v) = self
            .raw(
                method,
                uri,
                token,
                body.map(|b| b.to_string().into_bytes()),
                None,
            )
            .await;
        (st, v)
    }

    async fn patch(&self, uri: &str, token: &str, ops: Value) -> (StatusCode, Value) {
        self.call(
            Method::PATCH,
            uri,
            token,
            Some(json!({ "schemas": [PATCH_OP], "Operations": ops })),
        )
        .await
    }
}

fn assert_scim(st: StatusCode, body: &Value, status: StatusCode, scim_type: &str) {
    assert_eq!(st, status, "{body}");
    assert_eq!(body["scimType"], scim_type, "{body}");
}

#[tokio::test]
async fn user_filters_put_and_etags() {
    let api = Api::new().await;
    let admin = api.admin().await;

    // Create: an empty userName is refused; externalId is kept.
    let (st, _) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &admin,
            Some(json!({ "userName": "  " })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, created) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &admin,
            Some(
                json!({ "userName": "scim.err@hel.fi", "externalId": "ext-err",
                         "name": { "givenName": "Eva", "familyName": "Err" } }),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert_eq!(created["externalId"], "ext-err");
    let uid = created["id"].as_str().unwrap().to_owned();
    let user_uri = format!("{SCIM}/Users/{uid}");

    // Filters (RFC 7644 §3.4.2.2 subset): eq/co/sw, `and`, parentheses; others refused.
    for (filter, hits) in [
        (r#"userName eq "scim.err@hel.fi""#, 1),
        (r#"(userName sw "scim.err") and name.familyName co "rr""#, 1),
        (r#"emails.value eq "scim.err@hel.fi""#, 1),
        (r#"givenName eq "eva" and id eq "nope""#, 0),
        (r#"title eq "x""#, 0),
    ] {
        let q = url::form_urlencoded::byte_serialize(filter.as_bytes()).collect::<String>();
        let (st, list) = api
            .call(
                Method::GET,
                &format!("{SCIM}/Users?filter={q}"),
                &admin,
                None,
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{filter}: {list}");
        assert_eq!(list["totalResults"], hits, "{filter}");
    }
    for filter in [r#"userName gt "a""#, "userName", "userName eq unquoted", ""] {
        let q = url::form_urlencoded::byte_serialize(filter.as_bytes()).collect::<String>();
        let (st, body) = api
            .call(
                Method::GET,
                &format!("{SCIM}/Users?filter={q}"),
                &admin,
                None,
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{filter:?}: {body}");
    }

    // GET carries the weak ETag; PUT with a stale one is 412, `*` passes.
    let (st, h, got) = api.raw(Method::GET, &user_uri, &admin, None, None).await;
    assert_eq!(st, StatusCode::OK);
    let etag = h[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(got["meta"]["version"], etag.as_str());
    let put_body = |name: &str| {
        json!({ "userName": name, "active": true, "nickName": "Evi", "externalId": "ext-2",
                "name": { "givenName": "Eva", "familyName": "Put" },
                "emails": [{ "value": "home@eva.example", "type": "home" },
                           { "value": "work@eva.example", "type": "work", "primary": true }] })
        .to_string()
        .into_bytes()
    };
    let (st, _, err) = api
        .raw(
            Method::PUT,
            &user_uri,
            &admin,
            Some(put_body("scim.err@hel.fi")),
            Some("W/\"stale\""),
        )
        .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED, "{err}");

    // The address carried no proof: a verified address that PUT changes is unverified after.
    let mut stored = api
        .store
        .get_user_by_id("dev", None, &uid)
        .await
        .unwrap()
        .unwrap();
    stored.email_verified = true;
    api.store.update_user(stored).await.unwrap();
    let (st, _, put) = api
        .raw(
            Method::PUT,
            &user_uri,
            &admin,
            Some(put_body("Scim.Err@hel.fi")),
            Some("*"),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{put}");
    assert_eq!(put["nickName"], "Evi");
    let stored = api
        .store
        .get_user_by_id("dev", None, &uid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.email.as_deref(), Some("work@eva.example"));
    assert!(!stored.email_verified, "a changed address is not verified");
    assert_eq!(stored.username, "scim.err@hel.fi");

    // A PUT that leaves the address alone keeps its verification.
    let mut stored = stored;
    stored.email_verified = true;
    api.store.update_user(stored).await.unwrap();
    let (st, _, _) = api
        .raw(
            Method::PUT,
            &user_uri,
            &admin,
            Some(put_body("scim.err@hel.fi")),
            Some(&etag_of(&api, &admin, &user_uri).await),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        api.store
            .get_user_by_id("dev", None, &uid)
            .await
            .unwrap()
            .unwrap()
            .email_verified
    );

    // Body refusals: empty userName, broken JSON, unknown member, over 1 MiB.
    let (st, err) = api
        .call(
            Method::PUT,
            &user_uri,
            &admin,
            Some(json!({ "userName": " " })),
        )
        .await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let (st, _, err) = api
        .raw(
            Method::PUT,
            &user_uri,
            &admin,
            Some(b"{not json".to_vec()),
            None,
        )
        .await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let big = vec![b' '; 1024 * 1024 + 1];
    let (st, _, err) = api
        .raw(Method::PUT, &user_uri, &admin, Some(big), None)
        .await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE, "{err}");

    // Missing people answer 404 on every verb.
    let gone = format!("{SCIM}/Users/no-such-person");
    for m in [Method::GET, Method::PUT, Method::PATCH, Method::DELETE] {
        let (st, _) = api
            .call(m.clone(), &gone, &admin, Some(json!({ "userName": "x" })))
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{m}");
    }

    let (st, _) = api.call(Method::DELETE, &user_uri, &admin, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = api.call(Method::GET, &user_uri, &admin, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

async fn etag_of(api: &Api, token: &str, uri: &str) -> String {
    let (_, h, _) = api.raw(Method::GET, uri, token, None, None).await;
    h[header::ETAG].to_str().unwrap().to_owned()
}

#[tokio::test]
async fn user_patch_paths_and_refusals() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let (_, created) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &admin,
            Some(json!({ "userName": "patch.me@hel.fi",
                         "emails": [{ "value": "patch.me@hel.fi" }] })),
        )
        .await;
    let uid = created["id"].as_str().unwrap().to_owned();
    let uri = format!("{SCIM}/Users/{uid}");

    // Envelope refusals: no PatchOp schema, no operations, more than 100.
    let (st, err) = api
        .call(
            Method::PATCH,
            &uri,
            &admin,
            Some(json!({ "schemas": ["urn:other"], "Operations": [{ "op": "add", "path": "nickName", "value": "x" }] })),
        )
        .await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let (st, err) = api.patch(&uri, &admin, json!([])).await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let many: Vec<Value> = (0..101)
        .map(|_| json!({ "op": "add", "path": "nickName", "value": "x" }))
        .collect();
    let (st, err) = api.patch(&uri, &admin, Value::Array(many)).await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let (st, _, err) = api
        .raw(Method::PATCH, &uri, &admin, Some(b"[]".to_vec()), None)
        .await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let (st, _, err) = api
        .raw(
            Method::PATCH,
            &uri,
            &admin,
            Some(json!({ "schemas": [PATCH_OP], "Operations": [{ "op": "add", "path": "nickName", "value": "x" }] }).to_string().into_bytes()),
            Some("W/\"stale\""),
        )
        .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED, "{err}");

    // Operation refusals, one per rule.
    for (op, scim_type) in [
        (
            json!({ "op": "move", "path": "nickName", "value": "x" }),
            "invalidValue",
        ),
        (
            json!({ "op": "add", "value": "not an object" }),
            "invalidValue",
        ),
        (
            json!({ "op": "replace", "path": "meta.created", "value": "x" }),
            "mutability",
        ),
        (
            json!({ "op": "replace", "path": "id", "value": "x" }),
            "mutability",
        ),
        (
            json!({ "op": "replace", "path": "emails[type eq \"work\"", "value": "x" }),
            "invalidPath",
        ),
        (
            json!({ "op": "replace", "path": "emails[type gt \"work\"].value", "value": "x" }),
            "invalidPath",
        ),
        (
            json!({ "op": "remove", "path": "emails[type eq \"fax\"]" }),
            "noTarget",
        ),
        (
            json!({ "op": "replace", "path": "userName", "value": "  " }),
            "invalidValue",
        ),
        (
            json!({ "op": "replace", "path": "active", "value": "yes" }),
            "invalidValue",
        ),
        (
            json!({ "op": "add", "value": { "unknownThing": 1 } }),
            "invalidPath",
        ),
    ] {
        let (st, err) = api.patch(&uri, &admin, json!([op.clone()])).await;
        assert_scim(st, &err, StatusCode::BAD_REQUEST, scim_type);
    }

    // Plain attributes, add and remove.
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([
                { "op": "replace", "path": "userName", "value": "Patched@hel.fi" },
                { "op": "add", "path": "givenName", "value": "Gina" },
                { "op": "add", "path": "familyName", "value": "Fam" },
                { "op": "add", "path": "externalId", "value": "ext-p" },
                { "op": "add", "path": "nickName", "value": "Gi" },
            ]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");
    assert_eq!(u["userName"], "patched@hel.fi");
    assert_eq!(u["name"]["givenName"], "Gina");
    assert_eq!(u["externalId"], "ext-p");
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([
                { "op": "remove", "path": "name.givenName" },
                { "op": "remove", "path": "name.familyName" },
                { "op": "remove", "path": "externalId" },
                { "op": "remove", "path": "nickName" },
                { "op": "replace", "path": "name", "value": { "givenName": "N", "familyName": "M" } },
            ]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");
    assert_eq!(u["name"]["familyName"], "M");
    assert!(u.get("externalId").is_none_or(Value::is_null), "{u}");
    let (st, u) = api
        .patch(&uri, &admin, json!([{ "op": "remove", "path": "name" }]))
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(u["name"]["givenName"].is_null(), "{u}");

    // Addresses: PATCH changing the address clears its verification (regression).
    let mut stored = api
        .store
        .get_user_by_id("dev", None, &uid)
        .await
        .unwrap()
        .unwrap();
    stored.email_verified = true;
    api.store.update_user(stored).await.unwrap();
    let (st, _) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "add", "path": "emails",
                     "value": [{ "value": "second@hel.fi", "type": "work", "primary": true }] }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let stored = api
        .store
        .get_user_by_id("dev", None, &uid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.email.as_deref(), Some("second@hel.fi"));
    assert!(!stored.email_verified, "a PATCHed address is not verified");

    // A PATCH that leaves the address alone keeps the verification.
    let mut stored = stored;
    stored.email_verified = true;
    api.store.update_user(stored).await.unwrap();
    let (st, _) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "replace", "path": "nickName", "value": "Q" }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        api.store
            .get_user_by_id("dev", None, &uid)
            .await
            .unwrap()
            .unwrap()
            .email_verified
    );

    // valuePath on the whole entry, then remove it, then replace the list, then remove all.
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "replace", "path": "Emails[type eq \"work\"]",
                     "value": { "value": "whole@hel.fi", "type": "work" } }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "remove", "path": "emails[value eq \"whole@hel.fi\"]" }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "replace", "path": "emails", "value": [{ "value": "only@hel.fi" }] }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(u["emails"][0]["value"], "only@hel.fi");
    let (st, _) = api
        .patch(&uri, &admin, json!([{ "op": "remove", "path": "emails" }]))
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        api.store
            .get_user_by_id("dev", None, &uid)
            .await
            .unwrap()
            .unwrap()
            .email
            .is_none()
    );

    // A person without scim.emails but with an address: the valuePath works on that address.
    let mut stored = api
        .store
        .get_user_by_id("dev", None, &uid)
        .await
        .unwrap()
        .unwrap();
    stored.email = Some("plain@hel.fi".into());
    stored.attributes.remove("scim.emails");
    api.store.update_user(stored).await.unwrap();
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "replace", "path": "emails[type eq \"work\"].value", "value": "moved@hel.fi" }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");

    // active=false ends the person's sessions.
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "replace", "path": "active", "value": false }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(u["active"], false);
}

#[tokio::test]
async fn group_lifecycle_refusals() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let groups = format!("{SCIM}/Groups");

    // Create refusals: a path separator, an unknown member, an over-size body, broken JSON.
    let (st, _) = api
        .call(
            Method::POST,
            &groups,
            &admin,
            Some(json!({ "displayName": "a/b" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = api
        .call(
            Method::POST,
            &groups,
            &admin,
            Some(json!({ "displayName": "Ghosts", "members": [{ "value": "no-such-person" }] })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = api
        .raw(
            Method::POST,
            &groups,
            &admin,
            Some(vec![b' '; 1024 * 1024 + 1]),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);

    let (st, g) = api
        .call(
            Method::POST,
            &groups,
            &admin,
            Some(json!({ "displayName": "Scim Errors", "members": [{ "value": MEMBER }] })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{g}");
    let gid = g["id"].as_str().unwrap().to_owned();
    let uri = format!("{groups}/{gid}");

    // List with filters; a bad filter is refused.
    let q = url::form_urlencoded::byte_serialize(br#"displayName eq "Scim Errors""#)
        .collect::<String>();
    let (st, list) = api
        .call(Method::GET, &format!("{groups}?filter={q}"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["totalResults"], 1, "{list}");
    assert_eq!(list["Resources"][0]["members"][0]["value"], MEMBER);
    let q =
        url::form_urlencoded::byte_serialize(br#"id co "x" and title eq "y""#).collect::<String>();
    let (st, list) = api
        .call(Method::GET, &format!("{groups}?filter={q}"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["totalResults"], 0);
    let (st, _) = api
        .call(
            Method::GET,
            &format!("{groups}?filter=displayName%20ne%20%22x%22"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // PUT refusals.
    let (st, _, _) = api
        .raw(
            Method::PUT,
            &uri,
            &admin,
            Some(json!({ "displayName": "X" }).to_string().into_bytes()),
            Some("W/\"stale\""),
        )
        .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED);
    for (body, status, scim_type) in [
        (
            json!({ "id": "other", "displayName": "X" }),
            StatusCode::BAD_REQUEST,
            Some("mutability"),
        ),
        (
            json!({ "displayName": " " }),
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
        ),
        (
            json!({ "displayName": "X", "bogus": 1 }),
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
        ),
        (
            json!({ "displayName": "X", "members": [{ "value": "nobody" }] }),
            StatusCode::NOT_FOUND,
            None,
        ),
    ] {
        let (st, err) = api
            .call(Method::PUT, &uri, &admin, Some(body.clone()))
            .await;
        assert_eq!(st, status, "{body}: {err}");
        if let Some(t) = scim_type {
            assert_eq!(err["scimType"], t, "{body}");
        }
    }

    // PATCH envelope and operation refusals.
    let (st, err) = api
        .call(
            Method::PATCH,
            &uri,
            &admin,
            Some(json!({ "schemas": [], "Operations": [{ "op": "add", "path": "members", "value": [] }] })),
        )
        .await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let (st, err) = api.patch(&uri, &admin, json!([])).await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let (st, _, err) = api
        .raw(Method::PATCH, &uri, &admin, Some(b"nope".to_vec()), None)
        .await;
    assert_scim(st, &err, StatusCode::BAD_REQUEST, "invalidValue");
    let (st, _, _) = api
        .raw(
            Method::PATCH,
            &uri,
            &admin,
            Some(json!({ "schemas": [PATCH_OP], "Operations": [{ "op": "remove", "path": "members" }] }).to_string().into_bytes()),
            Some("W/\"stale\""),
        )
        .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED);
    for (op, scim_type) in [
        // Regression: an op other than add/remove/replace used to act as `add`.
        (
            json!({ "op": "move", "path": "displayName", "value": "Moved" }),
            "invalidValue",
        ),
        (json!({ "op": "remove" }), "noTarget"),
        (
            json!({ "op": "replace", "path": "id", "value": "x" }),
            "mutability",
        ),
        (
            json!({ "op": "remove", "path": "displayName" }),
            "mutability",
        ),
        (
            json!({ "op": "replace", "path": "displayName", "value": "a/b" }),
            "invalidValue",
        ),
        (
            json!({ "op": "replace", "value": { "displayName": "" } }),
            "invalidValue",
        ),
        (
            json!({ "op": "add", "path": "members", "value": { "value": MEMBER } }),
            "invalidValue",
        ),
        (
            json!({ "op": "add", "path": "members", "value": [{ "display": "no value" }] }),
            "invalidValue",
        ),
        (
            json!({ "op": "remove", "path": "members[value eq \"x\"" }),
            "invalidPath",
        ),
        (
            json!({ "op": "remove", "path": "members[value gt \"x\"]" }),
            "invalidPath",
        ),
        (
            json!({ "op": "add", "path": "members[value eq \"x\"]", "value": [] }),
            "invalidPath",
        ),
        (
            json!({ "op": "replace", "path": "owner", "value": "x" }),
            "invalidPath",
        ),
    ] {
        let (st, err) = api.patch(&uri, &admin, json!([op.clone()])).await;
        assert_scim(st, &err, StatusCode::BAD_REQUEST, scim_type);
    }
    assert_eq!(
        api.store
            .get_group("dev", None, &gid)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Scim Errors",
        "no refused PATCH renamed the group"
    );
    let (st, _) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "add", "path": "members", "value": [{ "value": "nobody" }] }]),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // PATCH that works: rename without a path, replace members, remove by valuePath, clear.
    let (st, g) = api
        .patch(
            &uri,
            &admin,
            json!([
                { "op": "replace", "value": { "displayName": "Renamed" } },
                { "op": "replace", "path": "members", "value": [{ "value": MEMBER }, { "value": MEMBER }] },
            ]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{g}");
    assert_eq!(g["displayName"], "Renamed");
    assert_eq!(g["members"].as_array().unwrap().len(), 1);
    let (st, g) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "remove", "path": format!("Members[value eq \"{MEMBER}\"]") }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(g["members"].as_array().unwrap().is_empty());
    let (st, _) = api
        .patch(
            &uri,
            &admin,
            json!([
                { "op": "add", "path": "members", "value": [{ "value": MEMBER }] },
                { "op": "remove", "path": "members" },
            ]),
        )
        .await;
    assert_eq!(st, StatusCode::OK);

    // GET with ETag, then unknown groups answer 404 on every verb.
    let (st, h, _) = api.raw(Method::GET, &uri, &admin, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(h.contains_key(header::ETAG));
    let gone = format!("{groups}/no-such-group");
    for m in [Method::GET, Method::PUT, Method::PATCH, Method::DELETE] {
        let (st, _) = api
            .call(
                m.clone(),
                &gone,
                &admin,
                Some(json!({ "displayName": "x" })),
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{m}");
    }

    // A member without rights: list and create are 403, the group itself 404.
    let member = api.token(MEMBER, json!({ "jc_org": ["helsinki"] })).await;
    let (st, _) = api.call(Method::GET, &groups, &member, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = api
        .call(
            Method::POST,
            &groups,
            &member,
            Some(json!({ "displayName": "Mine" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    for m in [Method::GET, Method::PUT, Method::PATCH, Method::DELETE] {
        let (st, _) = api
            .call(
                m.clone(),
                &uri,
                &member,
                Some(json!({ "displayName": "x" })),
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{m}");
    }

    let (st, _) = api.call(Method::DELETE, &uri, &admin, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn email_lists_merge_and_match_any_case() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let (_, created) = api
        .call(
            Method::POST,
            &format!("{SCIM}/Users"),
            &admin,
            Some(json!({ "userName": "lists@hel.fi" })),
        )
        .await;
    let uri = format!("{SCIM}/Users/{}", created["id"].as_str().unwrap());

    // PUT without a primary entry takes the first address.
    let (st, _, put) = api
        .raw(
            Method::PUT,
            &uri,
            &admin,
            Some(
                json!({ "userName": "lists@hel.fi",
                        "emails": [{ "value": "first@hel.fi" }, { "value": "second@hel.fi" }] })
                .to_string()
                .into_bytes(),
            ),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{put}");
    assert_eq!(put["emails"][0]["value"], "first@hel.fi");

    // `add` appends to the stored list instead of replacing it.
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "add", "path": "emails", "value": [{ "value": "third@hel.fi", "type": "work" }] }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");
    assert_eq!(u["emails"].as_array().unwrap().len(), 3, "{u}");

    // A valuePath filter names the sub-attribute in any case.
    let (st, u) = api
        .patch(
            &uri,
            &admin,
            json!([{ "op": "remove", "path": "emails[Type eq \"work\"]" }]),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");
    assert_eq!(u["emails"].as_array().unwrap().len(), 2, "{u}");
}
