#![forbid(unsafe_code)]
//! The native API as one OData 4.01 service (owner order 2026-11-07): `$metadata` lists every
//! registry operation (entity sets, bound and unbound actions and functions), actions answer the
//! same through OData, MCP and REST, errors are OData error JSON carrying the RFC 9457 slug.

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
const BASE: &str = "/api/v1/realms/dev/odata";
const STEWARD: &str = "de300000-0000-4000-8000-000000000001";
const ANALYST: &str = "de300000-0000-4000-8000-000000000010";

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
        if let (Some(c), Some(e)) = (claims.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                c.insert(k.clone(), v.clone());
            }
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

    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, axum::http::HeaderMap, Value, String) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let req = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                b.body(Body::from(v.to_string())).unwrap()
            }
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (
            st,
            headers,
            serde_json::from_str(&text).unwrap_or(Value::Null),
            text,
        )
    }

    async fn mcp(&self, token: &str, tool: &str, arguments: Value) -> Value {
        let (st, _, body, _) = self
            .call(
                Method::POST,
                "/api/v1/realms/dev/mcp",
                token,
                Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                             "params": { "name": tool, "arguments": arguments } })),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        body
    }
}

fn mcp_value(rpc: &Value) -> Value {
    let text = rpc["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn metadata_lists_every_registry_operation() {
    let api = Api::new().await;
    let t = api.admin().await;
    let (st, headers, _, xml) = api
        .call(Method::GET, &format!("{BASE}/$metadata"), &t, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["odata-version"], "4.01");
    let registry = OperationRegistry::new();
    assert!(registry.operations().len() >= 40);
    for op in registry.operations() {
        let o = &op.odata;
        match o.kind {
            ODataKind::EntitySetRead | ODataKind::EntitySetWrite => {
                assert!(
                    xml.contains(&format!("<EntitySet Name=\"{}\"", o.name)),
                    "{} has no entity set {}",
                    op.name,
                    o.name
                );
            }
            ODataKind::Action | ODataKind::Function => {
                let tag = if o.kind == ODataKind::Action {
                    "Action"
                } else {
                    "Function"
                };
                let open = format!(
                    "<{tag} Name=\"{}\" IsBound=\"{}\">",
                    o.name,
                    o.binding.is_some()
                );
                let at = xml
                    .find(&open)
                    .unwrap_or_else(|| panic!("{} missing as {open}", op.name));
                let element = &xml[at..at + xml[at..].find(&format!("</{tag}>")).unwrap()];
                assert!(element.contains(&format!("String=\"{}\"", op.name)));
                for p in &o.params {
                    assert!(
                        element.contains(&format!("Name=\"{}\" Type=\"{}\"", p.name, p.edm_type)),
                        "{} lacks parameter {}",
                        op.name,
                        p.name
                    );
                }
                if let Some(b) = o.binding {
                    let ty = if o.binding_collection {
                        format!("Collection(JoinedContext.Wardkeep.{b})")
                    } else {
                        format!("JoinedContext.Wardkeep.{b}")
                    };
                    assert!(element.contains(&format!("Name=\"bindingParameter\" Type=\"{ty}\"")));
                } else {
                    let import = if tag == "Action" {
                        "ActionImport"
                    } else {
                        "FunctionImport"
                    };
                    assert!(xml.contains(&format!(
                        "<{import} Name=\"{}\" {tag}=\"JoinedContext.Wardkeep.{}\"/>",
                        o.name, o.name
                    )));
                }
            }
        }
    }
    // The ones the owner named.
    for name in [
        "ResetCredentials",
        "RevokeSessions",
        "SendRequiredActions",
        "Apply",
        "Export",
        "Rotate",
        "Verify",
        "Revoke",
        "Allowed",
    ] {
        assert!(xml.contains(&format!("Name=\"{name}\"")), "{name}");
    }
    // The imports and actions sit inside the one schema and container.
    assert!(xml.find("<Action ").unwrap() < xml.find("<EntityContainer").unwrap());
    assert!(xml.find("<FunctionImport").unwrap() < xml.find("</EntityContainer>").unwrap());
}

#[tokio::test]
async fn allowed_answers_the_same_through_odata_mcp_and_rest() {
    let api = Api::new().await;
    let member = api
        .token(
            "de300000-0000-4000-8000-00000000000f",
            json!({ "jc_org": ["helsinki"] }),
        )
        .await;
    let admin = api.admin().await;
    for (token, operation, object) in [
        (&admin, "person_update", format!("People('{STEWARD}')")),
        (&member, "person_update", format!("People('{STEWARD}')")),
        (&member, "keys_rotate", String::new()),
        (
            &member,
            "person_update",
            "People('no-such-person')".to_owned(),
        ),
    ] {
        let odata_uri = format!(
            "{BASE}/Allowed(operation='{operation}',object='{}')",
            object.replace('\'', "''")
        );
        let (st, _, body, raw) = api.call(Method::GET, &odata_uri, token, None).await;
        assert_eq!(st, StatusCode::OK, "{raw}");
        let odata = body["value"].clone();
        let mcp = mcp_value(
            &api.mcp(
                token,
                "allowed",
                json!({ "payload": { "operation": operation, "object": object } }),
            )
            .await,
        );
        assert_eq!(odata, mcp, "{operation} {object}");
        // The native REST route gives the same decision.
        let (kind, id) = match object.split_once("('") {
            Some((_, rest)) => ("Person", rest.trim_end_matches("')").to_owned()),
            None => ("Realm", String::new()),
        };
        let verb = OperationRegistry::new()
            .find(operation)
            .map(|o| o.name)
            .unwrap();
        let (_, _, rest, _) = api
            .call(
                Method::GET,
                &format!("/api/v1/realms/dev/allowed?op={verb}&kind={kind}&id={id}"),
                token,
                None,
            )
            .await;
        assert_eq!(rest, odata, "REST {operation} {object}");
    }
    // Admin may, a member may not, a missing person looks like a hidden one.
    let (_, _, a, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/Allowed(operation='person_update',object='People(''{STEWARD}'')')"),
            &admin,
            None,
        )
        .await;
    assert_eq!(a["value"]["allowed"], true);
}

#[tokio::test]
async fn reset_credentials_acts_the_same_through_odata_and_mcp() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let uri = format!("{BASE}/People('{ANALYST}')/JoinedContext.Wardkeep.ResetCredentials");
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &uri,
            &admin,
            Some(json!({ "value": "NewAnalystPass1!x", "temporary": true })),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{raw}");
    let rpc = api
        .mcp(
            &admin,
            "credential_reset",
            json!({ "id": ANALYST, "payload": { "value": "OtherAnalystPass2!y", "temporary": false } }),
        )
        .await;
    assert_eq!(mcp_value(&rpc), Value::Null);
    let creds = api.store.list_credentials("dev", ANALYST).await.unwrap();
    let pw = creds
        .iter()
        .filter(|c| c.credential_type == "password")
        .max_by_key(|c| c.created_date)
        .unwrap();
    assert!(
        wardkeep_crypto::verify_and_maybe_rehash("OtherAnalystPass2!y", pw)
            .unwrap()
            .valid
    );

    // A policy violation: same slug both ways.
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &uri,
            &admin,
            Some(json!({ "value": "short" })),
        )
        .await;
    assert!(st.is_client_error());
    let slug = err["error"]["code"].as_str().unwrap().to_owned();
    assert_eq!(err["error"]["innererror"]["status"], st.as_u16());
    let rpc = api
        .mcp(
            &admin,
            "credential_reset",
            json!({ "id": ANALYST, "payload": { "value": "short" } }),
        )
        .await;
    assert_eq!(rpc["error"]["data"]["slug"], slug);

    // Another organization's person and a missing one: the same OData 404 error.
    let member = api
        .token(
            "de300000-0000-4000-8000-00000000000f",
            json!({ "jc_org": ["helsinki"] }),
        )
        .await;
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('missing-id')/JoinedContext.Wardkeep.ResetCredentials"),
            &member,
            Some(json!({ "value": "NewAnalystPass1!x" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not-found");
}

#[tokio::test]
async fn bound_functions_collection_actions_and_people_writes() {
    let api = Api::new().await;
    let admin = api.admin().await;

    // Realm/Export (bound function, GET with inline parameters).
    let (st, _, body, raw) = api
        .call(
            Method::GET,
            &format!("{BASE}/Realms('dev')/JoinedContext.Wardkeep.Export(withUsers=false)"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert_eq!(body["value"]["realm"], "dev");
    assert!(
        body["@odata.context"]
            .as_str()
            .unwrap()
            .ends_with("$metadata#Edm.Untyped")
    );
    let (st, _, _, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/Realms('other')/JoinedContext.Wardkeep.Export()"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // A function is not POSTed and an action is not GET.
    let (st, _, _, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/Realms('dev')/JoinedContext.Wardkeep.Export"),
            &admin,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Realm/Apply dry run.
    let (st, _, body, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/Realms('dev')/Apply"),
            &admin,
            Some(json!({ "configuration": { "realm": "ignored", "displayName": "Dry" }, "dryRun": true })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert_eq!(body["value"]["dryRun"], true);

    // SigningKey/Rotate (bound to the collection).
    let before = api.store.list_signing_keys("dev").await.unwrap().len();
    let (st, _, body, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/SigningKeys/JoinedContext.Wardkeep.Rotate"),
            &admin,
            Some(json!({ "algorithm": "ES256" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(body["value"]["kid"].is_string());
    assert_eq!(
        api.store.list_signing_keys("dev").await.unwrap().len(),
        before + 1
    );

    // People: create, patch, revoke sessions, delete.
    let (st, _, created, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People"),
            &admin,
            Some(
                json!({ "username": "odata.person@hel.fi", "email": "odata.person@hel.fi",
                         "enabled": true, "realmRoles": ["realm-admin"] }),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{raw}");
    let id = created["id"].as_str().unwrap().to_owned();
    let stored = api
        .store
        .get_user_by_id("dev", None, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        stored.realm_roles.is_empty(),
        "create ignores role mappings"
    );
    let (st, _, patched, raw) = api
        .call(
            Method::PATCH,
            &format!("{BASE}/People('{id}')"),
            &admin,
            Some(json!({ "firstName": "Olga" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert_eq!(patched["id"], id.as_str());
    let (st, _, _, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{id}')/RevokeSessions"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _, _) = api
        .call(
            Method::DELETE,
            &format!("{BASE}/People('{id}')"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        api.store
            .get_user_by_id("dev", None, &id)
            .await
            .unwrap()
            .is_none()
    );
    // Other sets stay read-only through OData.
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/Groups"),
            &admin,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(err["error"]["code"], "not-supported");
}

#[tokio::test]
async fn self_service_actions_and_honest_not_implemented() {
    let api = Api::new().await;
    let me = api.token(ANALYST, json!({})).await;
    // EnrolTotp step 1: a secret, nothing stored.
    let uri = format!("{BASE}/People('{ANALYST}')/JoinedContext.Wardkeep.EnrolTotp");
    let (st, _, step1, raw) = api.call(Method::POST, &uri, &me, Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    let secret = step1["value"]["secret"].as_str().unwrap().to_owned();
    assert_eq!(secret.len(), 32);
    // Step 2 with the app's code.
    let realm = api.store.get_realm("dev").await.unwrap().unwrap();
    let cred = CredentialRecord {
        credential_type: "otp".into(),
        secret_data: json!({ "value": secret }).to_string(),
        credential_data: json!({ "digits": realm.otp_policy_digits, "period": realm.otp_policy_period,
                                 "algorithm": realm.otp_policy_algorithm, "secretEncoding": "BASE32" })
        .to_string(),
        ..Default::default()
    };
    let now = chrono::Utc::now().timestamp() as u64;
    let code = generate_totp_code_for_credential(&cred, now).unwrap();
    let (st, _, _, _) = api
        .call(
            Method::POST,
            &uri,
            &me,
            Some(json!({ "secret": secret, "code": "000000" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, done, raw) = api
        .call(
            Method::POST,
            &uri,
            &me,
            Some(json!({ "secret": secret, "code": code })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(done["value"]["credentialId"].is_string());
    // Nobody enrols TOTP for someone else.
    let (st, _, _, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{STEWARD}')/EnrolTotp"),
            &me,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Recovery codes: ten, shown once, stored hashed.
    let (st, _, codes, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/GenerateRecoveryCodes"),
            &me,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let list = codes["value"].as_array().unwrap();
    assert_eq!(list.len(), 10);
    let stored = api.store.list_credentials("dev", ANALYST).await.unwrap();
    let rc = stored
        .iter()
        .find(|c| c.credential_type == "recovery-authn-codes")
        .unwrap();
    assert!(!rc.secret_data.contains(list[0].as_str().unwrap()));

    // Passkeys are declared but not built: 501 with the slug, not a fake success.
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(json!({ "credential": {} })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(err["error"]["code"], "not-supported");
    // Unknown operation on a set.
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/NoSuchAction"),
            &me,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not-found");
    // Malformed function parameters.
    let (st, _, err, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/Allowed(operation='x)"),
            &me,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "invalid-request");
}
